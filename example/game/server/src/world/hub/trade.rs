//! Player-to-player trade in town (COMBAT.md §2.6.2), RO style: a request
//! the other player accepts or declines, then a shared table where each
//! side puts items and zeny, presses OK (lock) and, once both locked,
//! Trade (confirm). Any change to an offer unlocks both sides.
//!
//! Hub channels only (stages refuse). Walking apart, a portal, picking a
//! mission or leaving the channel cancels. Offers are checked against the
//! hub's copy of each bag at every step; a deal both confirmed is handed
//! to the instance's [`TradeVault`](crate::world::trade::TradeVault)
//! ([`HubInstance::take_deals`]), which re-checks it against the live
//! belongings and stores both bags in one transaction. Until it answers
//! ([`HubInstance::trade_settled`]) the table is frozen: no change, no
//! cancel, and a second confirm (same tick or not) does nothing.

use super::{hub_error, HubEvent, HubInstance, InstanceKind, Outgoing};
use crate::world::protocol::ServerMsg;
use crate::world::trade::{exchange, Deal, Offer, Party, Settlement, TradeError, TradeOp};

/// Players at most this far apart (cells) may ask and accept.
pub const TRADE_RANGE: f32 = 8.0;
/// Further apart than this (cells), an open trade is cancelled.
pub const TRADE_KEEP_RANGE: f32 = 14.0;
/// An unanswered request lapses after this long.
const REQUEST_SECS: f32 = 20.0;

/// Requests, open tables and deals waiting for the vault.
#[derive(Default)]
pub(super) struct Trades {
    next_id: u32,
    requests: Vec<Request>,
    open: Vec<Trade>,
    deals: Vec<Deal>,
}

/// `from` asked `to` (user ids); lapses at tick `until`.
struct Request {
    from: u32,
    to: u32,
    until: u32,
}

struct Trade {
    id: u32,
    users: [u32; 2],
    offers: [Offer; 2],
    locked: [bool; 2],
    confirmed: [bool; 2],
    /// Both confirmed: the vault has the deal; the table is frozen.
    settling: bool,
}

impl Trade {
    fn side(&self, user_id: u32) -> Option<usize> {
        self.users.iter().position(|u| *u == user_id)
    }

    fn unlock(&mut self) {
        self.locked = [false; 2];
        self.confirmed = [false; 2];
    }
}

/// Closing reasons (`trade_closed`), besides the [`TradeError`] codes.
pub mod reason {
    pub const DONE: &str = "done";
    pub const CANCELLED: &str = "cancelled";
    pub const DECLINED: &str = "declined";
    pub const EXPIRED: &str = "expired";
    pub const TOO_FAR: &str = "too_far";
    pub const LEFT: &str = "left";
    pub const MAP: &str = "map_changed";
    pub const MISSION: &str = "mission";
}

fn closed(reason: &str, message: impl Into<String>) -> HubEvent {
    HubEvent::Control(ServerMsg::TradeClosed { reason: reason.into(), message: message.into() })
}

impl HubInstance {
    /// One trade step from `user_id`.
    pub fn trade_op(&mut self, user_id: u32, op: TradeOp, out: &mut Outgoing) {
        if self.find(user_id).is_none() {
            return;
        }
        if !matches!(self.kind, InstanceKind::Hub { .. }) {
            out.push((user_id, HubEvent::Control(hub_error("not_in_hub", "Trade with other players in town."))));
            return;
        }
        match op {
            TradeOp::Request { entity_id } => self.trade_request(user_id, entity_id, out),
            TradeOp::Accept => self.trade_accept(user_id, out),
            TradeOp::Decline => self.trade_decline(user_id, out),
            TradeOp::Cancel => self.trade_cancel(user_id, out),
            TradeOp::Lock => self.trade_lock(user_id, out),
            TradeOp::Confirm => self.trade_confirm(user_id, out),
            TradeOp::Zeny(zeny) => self.trade_change(user_id, out, |o, bag| o.set_zeny(bag, zeny)),
            TradeOp::Item { item, count } => self.trade_change(user_id, out, |o, bag| o.set_item(bag, &item, count)),
        }
    }

    /// Deals both sides confirmed since the last call, for the vault.
    pub fn take_deals(&mut self) -> Vec<Deal> {
        std::mem::take(&mut self.trades.deals)
    }

    /// The vault's answer for deal `id`: the new bags (both sides get
    /// `trade_closed done` and their `inventory`) or why nothing moved.
    pub fn trade_settled(&mut self, id: u32, result: Settlement, out: &mut Outgoing) {
        let Some(t) = self.trades.open.iter().position(|t| t.id == id) else { return };
        let t = self.trades.open.swap_remove(t);
        match result {
            Ok(bags) => {
                for (s, bag) in bags.into_iter().enumerate() {
                    let user = t.users[s];
                    if let Some(i) = self.find(user) {
                        self.players[i].profile.bag = bag;
                    }
                    out.push((user, closed(reason::DONE, "Trade complete.")));
                    out.push((user, HubEvent::BagChanged));
                }
            }
            Err(e) => {
                for user in t.users {
                    out.push((user, closed(e.code(), e.message())));
                }
            }
        }
    }

    /// Whether the player has a trade window open (shops wait).
    pub(super) fn is_trading(&self, user_id: u32) -> bool {
        self.trades.open.iter().any(|t| t.side(user_id).is_some())
    }

    fn name_of(&self, user_id: u32) -> String {
        self.find(user_id).map_or_else(|| "Your partner".into(), |i| self.players[i].profile.name.clone())
    }

    /// Both on the same map within `range` cells.
    fn near(&self, a: u32, b: u32, range: f32) -> bool {
        let (Some(i), Some(j)) = (self.find(a), self.find(b)) else { return false };
        let (p, q) = (&self.players[i], &self.players[j]);
        let (dx, dy) = (p.x - q.x, p.y - q.y);
        p.map == q.map && dx * dx + dy * dy <= range * range
    }

    fn trade_request(&mut self, user_id: u32, entity_id: u32, out: &mut Outgoing) {
        let err = |code: &str, msg: String| (user_id, HubEvent::Control(hub_error(code, msg)));
        let Some(target) = self.players.iter().find(|p| p.profile.character_id == entity_id && p.profile.user_id != user_id) else {
            out.push(err("trade_no_target", "Nobody to trade with there.".into()));
            return;
        };
        let (to, name) = (target.profile.user_id, target.profile.name.clone());
        if self.is_trading(user_id) {
            out.push(err("trade_busy", "You are already trading.".into()));
            return;
        }
        if !self.near(user_id, to, TRADE_RANGE) {
            out.push(err("trade_too_far", format!("Walk closer to {name} to trade.")));
            return;
        }
        let tick = self.tick;
        // They asked first: asking back accepts.
        if self.trades.requests.iter().any(|r| r.from == to && r.to == user_id && r.until > tick) {
            self.trade_accept(user_id, out);
            return;
        }
        let asked = self.trades.requests.iter().any(|r| r.to == to && r.from != user_id && r.until > tick);
        if self.is_trading(to) || asked {
            out.push(err("trade_busy", format!("{name} is busy.")));
            return;
        }
        // Asked again while waiting: nothing new. Asking someone else
        // withdraws the earlier request.
        if self.trades.requests.iter().any(|r| r.from == user_id && r.to == to && r.until > tick) {
            return;
        }
        self.withdraw_requests(user_id, out);
        let until = tick + self.ticks_of(REQUEST_SECS);
        self.trades.requests.push(Request { from: user_id, to, until });
        let me = &self.players[self.find(user_id).unwrap()];
        let msg = ServerMsg::TradeRequest { entity_id: me.profile.character_id, name: me.profile.name.clone(), level: me.profile.level };
        out.push((to, HubEvent::Control(msg)));
    }

    /// Drops the requests `user_id` made; their targets' prompts close.
    fn withdraw_requests(&mut self, user_id: u32, out: &mut Outgoing) {
        self.trades.requests.retain(|r| {
            if r.from == user_id {
                out.push((r.to, closed(reason::CANCELLED, "")));
                false
            } else {
                true
            }
        });
    }

    fn trade_accept(&mut self, user_id: u32, out: &mut Outgoing) {
        let tick = self.tick;
        let Some(r) = self.trades.requests.iter().position(|r| r.to == user_id && r.until > tick) else {
            out.push((user_id, closed(reason::EXPIRED, "The trade request is gone.")));
            return;
        };
        let from = self.trades.requests.remove(r).from;
        if self.is_trading(from) || self.is_trading(user_id) {
            out.push((user_id, closed(reason::CANCELLED, format!("{} is busy.", self.name_of(from)))));
            return;
        }
        if !self.near(user_id, from, TRADE_RANGE) {
            let far = "You are too far apart to trade.";
            out.push((user_id, closed(reason::TOO_FAR, far)));
            out.push((from, closed(reason::TOO_FAR, far)));
            return;
        }
        // Anything else either of them asked or was asked lapses.
        for user in [from, user_id] {
            self.withdraw_requests(user, out);
            self.trades.requests.retain(|r| {
                if r.to == user {
                    out.push((r.from, closed(reason::CANCELLED, "")));
                    false
                } else {
                    true
                }
            });
        }
        self.trades.next_id = self.trades.next_id.wrapping_add(1);
        let t = Trade {
            id: self.trades.next_id,
            users: [from, user_id],
            offers: Default::default(),
            locked: [false; 2],
            confirmed: [false; 2],
            settling: false,
        };
        self.trades.open.push(t);
        self.send_trade(self.trades.open.len() - 1, out);
    }

    fn trade_decline(&mut self, user_id: u32, out: &mut Outgoing) {
        let name = self.name_of(user_id);
        self.trades.requests.retain(|r| {
            if r.to == user_id {
                out.push((r.from, closed(reason::DECLINED, format!("{name} declined the trade."))));
                false
            } else {
                true
            }
        });
    }

    fn trade_cancel(&mut self, user_id: u32, out: &mut Outgoing) {
        if let Some(t) = self.trades.open.iter().find(|t| t.side(user_id).is_some()) {
            if t.settling {
                out.push((user_id, HubEvent::Control(hub_error("trade_settling", "The trade is already going through."))));
                return;
            }
            self.close_trade(user_id, reason::CANCELLED, "Trade cancelled.", "cancelled the trade", out);
            return;
        }
        if self.trades.requests.iter().any(|r| r.from == user_id) {
            self.withdraw_requests(user_id, out);
            out.push((user_id, closed(reason::CANCELLED, "")));
            return;
        }
        self.trade_decline(user_id, out);
    }

    /// Closes `user_id`'s open (not settling) trade: they read `mine`, the
    /// partner "<name> `theirs`."
    fn close_trade(&mut self, user_id: u32, why: &str, mine: &str, theirs: &str, out: &mut Outgoing) {
        let Some(at) = self.trades.open.iter().position(|t| t.side(user_id).is_some() && !t.settling) else { return };
        let t = self.trades.open.swap_remove(at);
        let name = self.name_of(user_id);
        for u in t.users {
            let msg = if u == user_id { mine.to_string() } else { format!("{name} {theirs}.") };
            out.push((u, closed(why, msg)));
        }
    }

    /// Ends everything trade-related for a player who leaves the channel,
    /// takes a portal or picks a mission (a settling deal still lands).
    pub(super) fn trade_end_for(&mut self, user_id: u32, why: &str, out: &mut Outgoing) {
        let (mine, theirs) = match why {
            reason::LEFT => ("Trade cancelled.", "left"),
            reason::MAP => ("Trade cancelled: you left the area.", "left the area"),
            reason::MISSION => ("Trade cancelled: you are off on a mission.", "is off on a mission"),
            _ => ("Trade cancelled.", "cancelled the trade"),
        };
        self.close_trade(user_id, why, mine, theirs, out);
        self.withdraw_requests(user_id, out);
        self.trades.requests.retain(|r| {
            if r.to == user_id {
                out.push((r.from, closed(why, "")));
                false
            } else {
                true
            }
        });
    }

    /// Item or zeny change on `user_id`'s side: checked against their bag;
    /// a change unlocks both sides.
    fn trade_change(&mut self, user_id: u32, out: &mut Outgoing, change: impl FnOnce(&mut Offer, &crate::world::combat::Bag) -> Result<(), TradeError>) {
        let Some(at) = self.open_side(user_id, out) else { return };
        let (t, s) = at;
        let bag = &self.players[self.find(user_id).unwrap()].profile.bag;
        let trade = &mut self.trades.open[t];
        let mut offer = trade.offers[s].clone();
        if let Err(e) = change(&mut offer, bag) {
            out.push((user_id, HubEvent::Control(hub_error(e.code(), e.message()))));
            return;
        }
        if offer == trade.offers[s] {
            return;
        }
        trade.offers[s] = offer;
        trade.unlock();
        self.send_trade(t, out);
    }

    /// The open, not yet settling trade of `user_id` and their side;
    /// otherwise says why not.
    fn open_side(&self, user_id: u32, out: &mut Outgoing) -> Option<(usize, usize)> {
        let Some(t) = self.trades.open.iter().position(|t| t.side(user_id).is_some()) else {
            out.push((user_id, HubEvent::Control(hub_error("trade_none", "You are not trading."))));
            return None;
        };
        if self.trades.open[t].settling {
            out.push((user_id, HubEvent::Control(hub_error("trade_settling", "The trade is already going through."))));
            return None;
        }
        Some((t, self.trades.open[t].side(user_id).unwrap()))
    }

    /// Would the exchange go through on the hub's copies of the bags?
    fn trade_dry_run(&self, t: usize) -> Result<(), TradeError> {
        let trade = &self.trades.open[t];
        let (Some(a), Some(b)) = (self.find(trade.users[0]), self.find(trade.users[1])) else { return Err(TradeError::Gone) };
        exchange(&self.players[a].profile.bag, &self.players[b].profile.bag, &trade.offers[0], &trade.offers[1]).map(|_| ())
    }

    fn trade_lock(&mut self, user_id: u32, out: &mut Outgoing) {
        let Some((t, s)) = self.open_side(user_id, out) else { return };
        if self.trades.open[t].locked[s] {
            return;
        }
        self.trades.open[t].locked[s] = true;
        if self.trades.open[t].locked == [true; 2]
            && let Err(e) = self.trade_dry_run(t)
        {
            self.trade_refused(t, e, out);
            return;
        }
        self.send_trade(t, out);
    }

    fn trade_confirm(&mut self, user_id: u32, out: &mut Outgoing) {
        let Some((t, s)) = self.open_side(user_id, out) else { return };
        let trade = &mut self.trades.open[t];
        if trade.locked != [true; 2] {
            out.push((user_id, HubEvent::Control(hub_error("trade_not_locked", "Both sides press OK before trading."))));
            return;
        }
        if trade.confirmed[s] {
            return;
        }
        trade.confirmed[s] = true;
        if trade.confirmed == [true; 2] {
            if let Err(e) = self.trade_dry_run(t) {
                self.trade_refused(t, e, out);
                return;
            }
            let ids = self.trades.open[t].users.map(|u| self.find(u).map_or(0, |i| self.players[i].profile.character_id));
            let trade = &mut self.trades.open[t];
            trade.settling = true;
            let party = |s: usize| Party { user_id: trade.users[s], character_id: ids[s], offer: trade.offers[s].clone() };
            let deal = Deal { id: trade.id, sides: [party(0), party(1)] };
            self.trades.deals.push(deal);
        }
        self.send_trade(t, out);
    }

    /// The exchange would fail as it stands: both unlock and hear why.
    fn trade_refused(&mut self, t: usize, e: TradeError, out: &mut Outgoing) {
        self.trades.open[t].unlock();
        for u in self.trades.open[t].users {
            out.push((u, HubEvent::Control(hub_error(e.code(), e.message()))));
        }
        self.send_trade(t, out);
    }

    /// `trade_state` to both sides, each from their own point of view.
    fn send_trade(&self, t: usize, out: &mut Outgoing) {
        let trade = &self.trades.open[t];
        for s in 0..2 {
            let o = 1 - s;
            let Some(pi) = self.find(trade.users[o]) else { continue };
            let partner = &self.players[pi].profile;
            let flags = trade.locked[s] as u8 | (trade.locked[o] as u8) << 1 | (trade.confirmed[s] as u8) << 2 | (trade.confirmed[o] as u8) << 3;
            let msg = ServerMsg::TradeState {
                entity_id: partner.character_id,
                name: partner.name.clone(),
                mine: trade.offers[s].clone(),
                theirs: trade.offers[o].clone(),
                flags,
            };
            out.push((trade.users[s], HubEvent::Control(msg)));
        }
    }

    /// The hub's copy of a trader's bag changed (equipment): an offer it
    /// no longer covers shrinks, and both sides unlock.
    pub(super) fn trade_bag_changed(&mut self, user_id: u32, out: &mut Outgoing) {
        let Some(t) = self.trades.open.iter().position(|t| t.side(user_id).is_some() && !t.settling) else { return };
        let s = self.trades.open[t].side(user_id).unwrap();
        let bag = &self.players[self.find(user_id).unwrap()].profile.bag;
        let trade = &mut self.trades.open[t];
        if trade.offers[s].clamp_to(bag) {
            trade.unlock();
            self.send_trade(t, out);
        }
    }

    /// Once a tick: requests lapse, traders who walked apart (or changed
    /// map) are cancelled.
    pub(super) fn trade_tick(&mut self, out: &mut Outgoing) {
        let tick = self.tick;
        if self.trades.requests.iter().any(|r| r.until <= tick) {
            let names: Vec<(u32, String)> = self.trades.requests.iter().filter(|r| r.until <= tick).map(|r| (r.from, self.name_of(r.to))).collect();
            self.trades.requests.retain(|r| {
                if r.until <= tick {
                    out.push((r.to, closed(reason::EXPIRED, "")));
                    false
                } else {
                    true
                }
            });
            for (from, name) in names {
                out.push((from, closed(reason::EXPIRED, format!("{name} did not answer."))));
            }
        }
        let apart: Vec<u32> = self
            .trades
            .open
            .iter()
            .filter(|t| !t.settling && !self.near(t.users[0], t.users[1], TRADE_KEEP_RANGE))
            .map(|t| t.users[0])
            .collect();
        for user in apart {
            let Some(at) = self.trades.open.iter().position(|t| t.side(user).is_some()) else { continue };
            let t = self.trades.open.swap_remove(at);
            for u in t.users {
                out.push((u, closed(reason::TOO_FAR, "Trade cancelled: you walked too far apart.")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::super::tests::{profile, quiet};
    use super::super::{HubEvent, HubInstance, HubWorld, Outgoing, PlayerProfile};
    use super::{reason, TRADE_KEEP_RANGE, TRADE_RANGE};
    use crate::world::combat::Bag;
    use crate::world::map::{CameraConstraint, LocationKind};
    use crate::world::protocol::ServerMsg;
    use crate::world::trade::{Deal, Offer, TradeError, TradeOp};
    use std::sync::Arc;
    use uuid::Uuid;

    const A: u32 = 1;
    const B: u32 = 2;
    const C: u32 = 3;

    fn bag(zeny: u64, items: &[(&str, u32)]) -> Bag {
        let mut b = Bag { zeny, ..Bag::default() };
        for (id, n) in items {
            b.add(id, *n).unwrap();
        }
        b.equip[4] = "knife".into();
        b
    }

    /// A 40-cell-wide town square (players can walk apart) and the
    /// standard test house behind a door.
    fn plaza(players: &[(u32, Bag)]) -> HubInstance {
        let row = format!("#{}#", ".".repeat(38));
        let wall = "#".repeat(40);
        let rows = [wall.as_str(), row.as_str(), row.as_str(), row.as_str(), wall.as_str()];
        let free = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
        let town = map("town", LocationKind::Hub, free, &rows, vec![portal("to_house", [38, 2, 1], "house", [2, 2])]);
        let house = map(
            "house",
            LocationKind::Indoor,
            CameraConstraint::Fixed { yaw: 0.0, pitch: -45.0, zoom: 18.0 },
            &["######", "#....#", "#....#", "#....#", "######"],
            vec![portal("exit", [1, 2, 1], "town", [36, 2])],
        );
        let world = Arc::new(HubWorld::new(vec![town, house], "town").unwrap());
        let mut hub = HubInstance::new(1, world, quiet());
        for (u, b) in players {
            hub.join(PlayerProfile { bag: b.clone(), ..profile(*u) }, Uuid::nil(), None, 0, &mut Vec::new());
            put(&mut hub, *u, 10.0 + *u as f32, 2.5);
        }
        hub
    }

    fn put(hub: &mut HubInstance, user: u32, x: f32, y: f32) {
        let i = hub.find(user).unwrap();
        (hub.players[i].x, hub.players[i].y) = (x, y);
    }

    fn op(hub: &mut HubInstance, user: u32, op: TradeOp) -> Outgoing {
        let mut out = Vec::new();
        hub.trade_op(user, op, &mut out);
        out
    }

    fn entity(user: u32) -> u32 {
        100 + user
    }

    fn requests(out: &Outgoing, user: u32) -> Vec<(u32, String)> {
        out.iter()
            .filter_map(|(u, e)| match e {
                HubEvent::Control(ServerMsg::TradeRequest { entity_id, name, .. }) if *u == user => Some((*entity_id, name.clone())),
                _ => None,
            })
            .collect()
    }

    /// (partner, mine, theirs, flags) of every `trade_state` to `user`.
    fn states(out: &Outgoing, user: u32) -> Vec<(u32, Offer, Offer, u8)> {
        out.iter()
            .filter_map(|(u, e)| match e {
                HubEvent::Control(ServerMsg::TradeState { entity_id, mine, theirs, flags, .. }) if *u == user => {
                    Some((*entity_id, mine.clone(), theirs.clone(), *flags))
                }
                _ => None,
            })
            .collect()
    }

    fn closed(out: &Outgoing, user: u32) -> Vec<(String, String)> {
        out.iter()
            .filter_map(|(u, e)| match e {
                HubEvent::Control(ServerMsg::TradeClosed { reason, message }) if *u == user => Some((reason.clone(), message.clone())),
                _ => None,
            })
            .collect()
    }

    fn errors(out: &Outgoing, user: u32) -> Vec<String> {
        out.iter()
            .filter_map(|(u, e)| match e {
                HubEvent::Control(ServerMsg::HubError { code, .. }) if *u == user => Some(code.clone()),
                _ => None,
            })
            .collect()
    }

    fn flags(out: &Outgoing, user: u32) -> u8 {
        states(out, user).last().expect("a trade_state").3
    }

    /// A asks B, B accepts: the table is open.
    fn open(hub: &mut HubInstance) {
        let out = op(hub, A, TradeOp::Request { entity_id: entity(B) });
        assert_eq!(requests(&out, B), vec![(entity(A), "p1".into())]);
        let out = op(hub, B, TradeOp::Accept);
        assert_eq!(states(&out, A), vec![(entity(B), Offer::default(), Offer::default(), 0)]);
        assert_eq!(states(&out, B), vec![(entity(A), Offer::default(), Offer::default(), 0)]);
    }

    fn offer(zeny: u64, items: &[(&str, u32)]) -> Offer {
        Offer { zeny, items: items.iter().map(|(i, n)| (i.to_string(), *n)).collect() }
    }

    fn rich() -> HubInstance {
        plaza(&[(A, bag(1000, &[("red_potion", 10), ("jellopy", 4)])), (B, bag(50, &[("sword", 1)]))])
    }

    #[test]
    fn happy_path_offers_lock_confirm_and_settle() {
        let mut hub = rich();
        open(&mut hub);
        let out = op(&mut hub, A, TradeOp::Item { item: "red_potion".into(), count: 4 });
        assert_eq!(states(&out, B).last().unwrap().2, offer(0, &[("red_potion", 4)]), "B sees A's offer as theirs");
        op(&mut hub, A, TradeOp::Zeny(300));
        op(&mut hub, B, TradeOp::Item { item: "sword".into(), count: 1 });
        let out = op(&mut hub, A, TradeOp::Lock);
        assert_eq!((flags(&out, A), flags(&out, B)), (0b01, 0b10));
        let out = op(&mut hub, B, TradeOp::Lock);
        assert_eq!(flags(&out, A), 0b11);
        let out = op(&mut hub, A, TradeOp::Confirm);
        assert_eq!((flags(&out, A), flags(&out, B)), (0b0111, 0b1011));
        assert!(hub.take_deals().is_empty(), "one confirm is not a deal");
        let out = op(&mut hub, B, TradeOp::Confirm);
        assert_eq!(flags(&out, A), 0b1111);
        let deals = hub.take_deals();
        assert_eq!(deals.len(), 1);
        let Deal { id, sides } = deals[0].clone();
        assert_eq!((sides[0].user_id, sides[0].character_id, &sides[0].offer), (A, entity(A), &offer(300, &[("red_potion", 4)])));
        assert_eq!((sides[1].user_id, sides[1].character_id, &sides[1].offer), (B, entity(B), &offer(0, &[("sword", 1)])));

        // The vault stored both bags: everyone hears it, bags follow.
        let a = bag(700, &[("red_potion", 6), ("jellopy", 4), ("sword", 1)]);
        let b = bag(350, &[("red_potion", 4)]);
        let mut out = Vec::new();
        hub.trade_settled(id, Ok([a.clone(), b.clone()]), &mut out);
        for u in [A, B] {
            assert_eq!(closed(&out, u), vec![(reason::DONE.into(), "Trade complete.".into())]);
            assert!(out.iter().any(|(x, e)| *x == u && matches!(e, HubEvent::BagChanged)));
        }
        assert_eq!(hub.players[hub.find(A).unwrap()].profile.bag, a);
        assert_eq!(hub.players[hub.find(B).unwrap()].profile.bag, b);
        assert!(!hub.is_trading(A) && !hub.is_trading(B));
        // A late answer for the same deal changes nothing.
        let mut out = Vec::new();
        hub.trade_settled(id, Ok([Bag::default(), Bag::default()]), &mut out);
        assert!(out.is_empty());
        assert_eq!(hub.players[hub.find(A).unwrap()].profile.bag, a);
    }

    #[test]
    fn any_change_after_a_lock_unlocks_both_sides() {
        let mut hub = rich();
        open(&mut hub);
        op(&mut hub, A, TradeOp::Zeny(100));
        op(&mut hub, A, TradeOp::Lock);
        op(&mut hub, B, TradeOp::Lock);
        op(&mut hub, A, TradeOp::Confirm);
        // B (who locked and did not confirm) adds something: all undone.
        let out = op(&mut hub, B, TradeOp::Item { item: "sword".into(), count: 1 });
        assert_eq!((flags(&out, A), flags(&out, B)), (0, 0));
        let out = op(&mut hub, B, TradeOp::Confirm);
        assert_eq!(errors(&out, B), vec!["trade_not_locked"]);
        // The locked side changing its own offer unlocks too.
        op(&mut hub, A, TradeOp::Lock);
        op(&mut hub, B, TradeOp::Lock);
        let out = op(&mut hub, A, TradeOp::Zeny(99));
        assert_eq!((flags(&out, A), flags(&out, B)), (0, 0));
        // Setting the same thing again is no change.
        op(&mut hub, A, TradeOp::Lock);
        let out = op(&mut hub, A, TradeOp::Zeny(99));
        assert!(out.is_empty());
        assert!(hub.take_deals().is_empty());
    }

    #[test]
    fn a_bag_change_that_shrinks_an_offer_unlocks_both() {
        let mut hub = rich();
        open(&mut hub);
        op(&mut hub, A, TradeOp::Item { item: "jellopy".into(), count: 4 });
        op(&mut hub, A, TradeOp::Lock);
        op(&mut hub, B, TradeOp::Lock);
        let mut out = Vec::new();
        hub.set_bag(A, bag(1000, &[("red_potion", 10), ("jellopy", 1)]), &mut out);
        let (_, mine, _, f) = states(&out, A).pop().unwrap();
        assert_eq!((mine, f), (offer(0, &[("jellopy", 1)]), 0));
        // A change that leaves the offer whole keeps the locks.
        op(&mut hub, A, TradeOp::Lock);
        let mut out = Vec::new();
        hub.set_bag(A, bag(1000, &[("jellopy", 1)]), &mut out);
        assert!(states(&out, A).is_empty());
    }

    #[test]
    fn confirming_twice_or_together_makes_one_deal_and_freezes_the_table() {
        let mut hub = rich();
        open(&mut hub);
        op(&mut hub, A, TradeOp::Zeny(10));
        op(&mut hub, A, TradeOp::Lock);
        op(&mut hub, B, TradeOp::Lock);
        // Both confirm within one tick, A twice.
        op(&mut hub, A, TradeOp::Confirm);
        op(&mut hub, B, TradeOp::Confirm);
        op(&mut hub, A, TradeOp::Confirm);
        op(&mut hub, B, TradeOp::Confirm);
        assert_eq!(hub.take_deals().len(), 1);
        // Settling: no change, no cancel, no new trade.
        for (who, step) in [(A, TradeOp::Zeny(500)), (B, TradeOp::Item { item: "sword".into(), count: 1 }), (A, TradeOp::Lock), (B, TradeOp::Cancel)] {
            let out = op(&mut hub, who, step);
            assert_eq!(errors(&out, who), vec!["trade_settling"]);
        }
        let out = op(&mut hub, A, TradeOp::Request { entity_id: entity(B) });
        assert_eq!(errors(&out, A), vec!["trade_busy"]);
        // Walking apart while settling does not cancel either.
        put(&mut hub, B, 37.0, 2.5);
        let mut out = Vec::new();
        hub.tick(&mut out);
        assert!(closed(&out, A).is_empty());
        assert!(hub.take_deals().is_empty());
    }

    #[test]
    fn a_failed_settlement_closes_the_trade_and_moves_nothing() {
        let mut hub = rich();
        open(&mut hub);
        op(&mut hub, A, TradeOp::Zeny(10));
        op(&mut hub, A, TradeOp::Lock);
        op(&mut hub, B, TradeOp::Lock);
        op(&mut hub, A, TradeOp::Confirm);
        op(&mut hub, B, TradeOp::Confirm);
        let id = hub.take_deals()[0].id;
        let before = hub.players[hub.find(A).unwrap()].profile.bag.clone();
        let mut out = Vec::new();
        hub.trade_settled(id, Err(TradeError::NotOwned), &mut out);
        for u in [A, B] {
            assert_eq!(closed(&out, u).len(), 1);
            assert_eq!(closed(&out, u)[0].0, "trade_not_owned");
        }
        assert!(!out.iter().any(|(_, e)| matches!(e, HubEvent::BagChanged)));
        assert_eq!(hub.players[hub.find(A).unwrap()].profile.bag, before);
        assert!(!hub.is_trading(A));
    }

    #[test]
    fn offers_are_checked_against_the_bag() {
        let mut hub = rich();
        open(&mut hub);
        let out = op(&mut hub, B, TradeOp::Zeny(51));
        assert_eq!(errors(&out, B), vec!["not_enough_zeny"]);
        let out = op(&mut hub, A, TradeOp::Item { item: "red_potion".into(), count: 11 });
        assert_eq!(errors(&out, A), vec!["trade_not_owned"]);
        // The worn weapon is not in the bag: it cannot be offered.
        let out = op(&mut hub, B, TradeOp::Item { item: "knife".into(), count: 1 });
        assert_eq!(errors(&out, B), vec!["trade_not_owned"]);
        let out = op(&mut hub, A, TradeOp::Zeny(u64::MAX));
        assert_eq!(errors(&out, A), vec!["not_enough_zeny"]);
        // Taking an item off the table.
        op(&mut hub, A, TradeOp::Item { item: "red_potion".into(), count: 2 });
        let out = op(&mut hub, A, TradeOp::Item { item: "red_potion".into(), count: 0 });
        assert_eq!(states(&out, A).pop().unwrap().1, Offer::default());
        // Nothing to change without a trade.
        let out = op(&mut hub, C, TradeOp::Lock);
        assert!(out.is_empty(), "unknown player");
    }

    #[test]
    fn a_receiving_bag_without_room_unlocks_at_the_second_lock() {
        let full = Bag { items: (0..crate::world::combat::items::MAX_STACKS).map(|i| crate::world::combat::items::Stack { id: format!("x{i}"), count: 1, ..Default::default() }).collect(), ..bag(0, &[]) };
        let mut hub = plaza(&[(A, bag(0, &[("sword", 1)])), (B, full)]);
        open(&mut hub);
        op(&mut hub, A, TradeOp::Item { item: "sword".into(), count: 1 });
        op(&mut hub, A, TradeOp::Lock);
        let out = op(&mut hub, B, TradeOp::Lock);
        assert_eq!(errors(&out, A), vec!["trade_bag_full"]);
        assert_eq!(errors(&out, B), vec!["trade_bag_full"]);
        assert_eq!(flags(&out, B), 0);
    }

    #[test]
    fn requests_need_town_range_and_free_players() {
        let mut hub = rich();
        hub.join(PlayerProfile { bag: bag(5, &[]), ..profile(C) }, Uuid::nil(), None, 0, &mut Vec::new());
        put(&mut hub, C, 13.0, 2.5);
        let out = op(&mut hub, A, TradeOp::Request { entity_id: entity(A) });
        assert_eq!(errors(&out, A), vec!["trade_no_target"], "not with oneself");
        let out = op(&mut hub, A, TradeOp::Request { entity_id: 999 });
        assert_eq!(errors(&out, A), vec!["trade_no_target"]);
        put(&mut hub, B, 11.0 + TRADE_RANGE + 1.0, 2.5);
        let out = op(&mut hub, A, TradeOp::Request { entity_id: entity(B) });
        assert_eq!(errors(&out, A), vec!["trade_too_far"]);
        put(&mut hub, B, 12.0, 2.5);
        op(&mut hub, A, TradeOp::Request { entity_id: entity(B) });
        // Asking again while waiting sends nothing new.
        assert!(op(&mut hub, A, TradeOp::Request { entity_id: entity(B) }).is_empty());
        // B is being asked: C must wait.
        let out = op(&mut hub, C, TradeOp::Request { entity_id: entity(B) });
        assert_eq!(errors(&out, C), vec!["trade_busy"]);
        op(&mut hub, B, TradeOp::Accept);
        let out = op(&mut hub, C, TradeOp::Request { entity_id: entity(A) });
        assert_eq!(errors(&out, C), vec!["trade_busy"]);
        // Shops wait for the trade.
        let mut out = Vec::new();
        hub.trade(A, "merchant", "red_potion", 1, false, &mut out);
        assert_eq!(errors(&out, A), vec!["trade_busy"]);
    }

    #[test]
    fn asking_back_accepts_and_asking_someone_else_withdraws() {
        let mut hub = rich();
        hub.join(PlayerProfile { bag: bag(5, &[]), ..profile(C) }, Uuid::nil(), None, 0, &mut Vec::new());
        put(&mut hub, C, 13.0, 2.5);
        op(&mut hub, A, TradeOp::Request { entity_id: entity(B) });
        let out = op(&mut hub, A, TradeOp::Request { entity_id: entity(C) });
        assert_eq!(closed(&out, B), vec![(reason::CANCELLED.into(), String::new())], "B's prompt closes");
        assert_eq!(requests(&out, C).len(), 1);
        let out = op(&mut hub, C, TradeOp::Request { entity_id: entity(A) });
        assert_eq!(states(&out, A).len(), 1, "C asking A back opens the table");
        assert!(hub.is_trading(C));
    }

    #[test]
    fn decline_cancel_and_expiry() {
        let mut hub = rich();
        op(&mut hub, A, TradeOp::Request { entity_id: entity(B) });
        let out = op(&mut hub, B, TradeOp::Decline);
        assert_eq!(closed(&out, A), vec![(reason::DECLINED.into(), "p2 declined the trade.".into())]);
        let out = op(&mut hub, B, TradeOp::Accept);
        assert_eq!(closed(&out, B)[0].0, reason::EXPIRED, "nothing to accept");

        // The asker withdraws.
        op(&mut hub, A, TradeOp::Request { entity_id: entity(B) });
        let out = op(&mut hub, A, TradeOp::Cancel);
        assert_eq!(closed(&out, B), vec![(reason::CANCELLED.into(), String::new())]);

        // Nobody answers for 20 s.
        op(&mut hub, A, TradeOp::Request { entity_id: entity(B) });
        let mut out = Vec::new();
        for _ in 0..hub.ticks_of(20.0) {
            hub.tick(&mut out);
        }
        assert_eq!(closed(&out, A), vec![(reason::EXPIRED.into(), "p2 did not answer.".into())]);
        assert_eq!(closed(&out, B), vec![(reason::EXPIRED.into(), String::new())]);

        // An open table cancelled by either side.
        open(&mut hub);
        let out = op(&mut hub, B, TradeOp::Cancel);
        assert_eq!(closed(&out, B), vec![(reason::CANCELLED.into(), "Trade cancelled.".into())]);
        assert_eq!(closed(&out, A), vec![(reason::CANCELLED.into(), "p2 cancelled the trade.".into())]);
        assert!(!hub.is_trading(A));
    }

    #[test]
    fn walking_apart_a_portal_a_mission_or_leaving_cancels() {
        let mut hub = rich();
        open(&mut hub);
        put(&mut hub, B, 11.0 + TRADE_KEEP_RANGE - 0.5, 2.5);
        let mut out = Vec::new();
        hub.tick(&mut out);
        assert!(closed(&out, A).is_empty(), "still in range");
        put(&mut hub, B, 11.0 + TRADE_KEEP_RANGE + 0.5, 2.5);
        hub.tick(&mut out);
        assert_eq!(closed(&out, A)[0].0, reason::TOO_FAR);
        assert_eq!(closed(&out, B)[0].0, reason::TOO_FAR);

        put(&mut hub, B, 12.0, 2.5);
        open(&mut hub);
        let i = hub.find(B).unwrap();
        let mut out = Vec::new();
        hub.use_portal(i, 0, &mut out);
        assert_eq!(closed(&out, A), vec![(reason::MAP.into(), "p2 left the area.".into())]);
        assert_eq!(closed(&out, B)[0].0, reason::MAP);

        let i = hub.find(B).unwrap();
        hub.players[i].map = hub.world.entry();
        put(&mut hub, B, 12.0, 2.5);
        open(&mut hub);
        let mut cfg = quiet();
        cfg.missions_anywhere = true;
        hub.config = cfg;
        let mut out = Vec::new();
        hub.select_mission(A, crate::world::missions::MISSIONS[0].id, true, &mut out);
        assert_eq!(closed(&out, B), vec![(reason::MISSION.into(), "p1 is off on a mission.".into())]);
        assert!(out.iter().any(|(u, e)| *u == A && matches!(e, HubEvent::MissionChosen { .. })));

        open(&mut hub);
        let mut out = Vec::new();
        assert!(hub.leave(A, Uuid::nil(), &mut out));
        assert_eq!(closed(&out, B), vec![(reason::LEFT.into(), "p1 left.".into())]);
        assert!(!hub.is_trading(B));
    }

    #[test]
    fn a_partner_leaving_while_the_deal_settles_still_gets_it() {
        let mut hub = rich();
        open(&mut hub);
        op(&mut hub, A, TradeOp::Zeny(10));
        op(&mut hub, A, TradeOp::Lock);
        op(&mut hub, B, TradeOp::Lock);
        op(&mut hub, A, TradeOp::Confirm);
        op(&mut hub, B, TradeOp::Confirm);
        let id = hub.take_deals()[0].id;
        let mut out = Vec::new();
        hub.leave(A, Uuid::nil(), &mut out);
        assert!(closed(&out, B).is_empty(), "the deal is in the vault's hands");
        let mut out = Vec::new();
        hub.trade_settled(id, Ok([bag(990, &[]), bag(60, &[])]), &mut out);
        assert_eq!(closed(&out, B)[0].0, reason::DONE);
        assert_eq!(hub.players[hub.find(B).unwrap()].profile.bag.zeny, 60);
    }

    #[test]
    fn stages_have_no_trading() {
        let world = Arc::new(test_world());
        let mut stage = HubInstance::stage(1, "m1", world, quiet());
        for u in [A, B] {
            stage.join(profile(u), Uuid::nil(), None, 0, &mut Vec::new());
        }
        let out = op(&mut stage, A, TradeOp::Request { entity_id: entity(B) });
        assert_eq!(errors(&out, A), vec!["not_in_hub"]);
        assert!(requests(&out, B).is_empty());
    }
}
