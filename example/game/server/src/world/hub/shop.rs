//! Items in an instance (COMBAT.md §2.6): merchants check that the player
//! stands at them and sells what is asked, potions are drunk on missions,
//! and a new weapon is shown to everyone on the map.
//!
//! The bag itself lives with the session's character (the socket task):
//! orders come back to it as [`HubEvent::Shop`], potions as
//! [`HubEvent::ItemUsed`], so the stage never writes belongings it does
//! not own.

use super::{hub_error, HubEvent, HubInstance, Outgoing, REFINER, SHOP};
use crate::world::combat::items::{self, Effect, Kind, POTION_COOLDOWN_MS, POTION_REGEN_MS};
use crate::world::combat::{Bag, Condition, Slot, Weapon};
use crate::world::protocol::ServerMsg;
use crate::world::trade::{OrderError, ShopOrder};

impl super::HubPlayer {
    /// Held weapon id (what other clients draw).
    pub(super) fn weapon_id(&self) -> &str {
        if let Some(f) = &self.fighter {
            return f.weapon.id;
        }
        let class = crate::world::combat::Class::from_id(&self.profile.archetype).unwrap_or(crate::world::combat::Class::Swordsman);
        let worn = self.profile.bag.weapon();
        Weapon::held(class, if worn.is_empty() { &self.profile.weapon } else { worn }).id
    }
}

impl HubInstance {
    /// A merchant order at `npc` (the shop window's cart, or one item): the
    /// player must stand at it, and every buy must be something it sells.
    /// The socket task applies the order to the bag ([`HubEvent::Shop`]),
    /// all of it or nothing, at the prices the player's haggling makes
    /// (Discount, Overcharge: read here off the loadout, never taken from
    /// the client).
    pub fn shop(&mut self, user_id: u32, npc: &str, order: ShopOrder, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        if self.is_trading(user_id) {
            out.push((user_id, HubEvent::Control(hub_error("trade_busy", "Finish your trade first."))));
            return;
        }
        let p = &self.players[i];
        let event = match self.npc_here(p.map, npc).filter(|n| n.kind == SHOP) {
            None => HubEvent::Control(hub_error("no_such_npc", format!("no merchant {npc} here"))),
            Some(n) if !self.at_merchant(n, p.x, p.y) => HubEvent::Control(hub_error("npc_too_far", format!("walk closer to {}", n.name))),
            Some(n) => match order.check(items::shop(&n.shop).unwrap_or_default()) {
                Err(OrderError::NotSoldHere) => HubEvent::Control(hub_error("not_sold_here", format!("{} does not sell that", n.name))),
                Err(e) => HubEvent::Control(hub_error(e.code(), e.message())),
                Ok(()) => HubEvent::Shop(order, self.haggle_of(i)),
            },
        };
        out.push((user_id, event));
    }

    /// One refine attempt at the smith `npc` on the item worn in `slot` or
    /// (0xFF) off the bag stack `item`: the player must stand at it and not
    /// be trading. The smith rolls the d100 and the slip die here; the socket task applies
    /// the attempt to the bag ([`HubEvent::Refine`]) and answers it.
    pub fn refine(&mut self, user_id: u32, npc: &str, item: String, slot: u8, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        if self.is_trading(user_id) {
            out.push((user_id, HubEvent::Control(hub_error("trade_busy", "Finish your trade first."))));
            return;
        }
        let p = &self.players[i];
        let slot = match slot {
            0xFF => None,
            s => match Slot::from_index(s) {
                Some(s) => Some(s),
                None => return out.push((user_id, HubEvent::Control(hub_error("bad_message", "no such slot")))),
            },
        };
        let event = match self.npc_here(p.map, npc).filter(|n| n.kind == REFINER) {
            None => HubEvent::Control(hub_error("no_such_npc", format!("no smith {npc} here"))),
            Some(n) if !self.at_merchant(n, p.x, p.y) => HubEvent::Control(hub_error("npc_too_far", format!("walk closer to {}", n.name))),
            Some(_) => {
                let (roll, slip) = self.rigged_refine.take().unwrap_or_else(|| (self.dice.die(100), self.dice.die(crate::world::combat::refine::SLIP_ROLL)));
                HubEvent::Refine { item, slot, roll, slip }
            }
        };
        out.push((user_id, event));
    }

    /// The shop prices player `i` gets (Discount, Overcharge).
    pub(super) fn haggle_of(&self, i: usize) -> items::Haggle {
        let (buy, sell) = self.skills.haggle(&self.players[i].profile.loadout);
        items::Haggle { buy, sell }
    }

    /// Standing at merchant `n` (anywhere, with the `GAME_DEV` test hooks:
    /// the e2e shop opens without a walk).
    pub(super) fn at_merchant(&self, n: &crate::world::map::NpcDef, x: f32, y: f32) -> bool {
        n.in_reach(x, y) || self.config.missions_anywhere
    }

    /// The player's belongings changed (the socket task's copy): the next
    /// stage starts from them, and the map sees the new appearance (weapon,
    /// headgear) if it changed.
    pub fn set_bag(&mut self, user_id: u32, bag: Bag, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let p = &mut self.players[i];
        p.profile.weapon = bag.weapon().to_string();
        p.profile.bag = bag;
        self.refresh_appearance(i, out);
        self.trade_bag_changed(user_id, out);
    }

    /// Drinks a potion on a mission: standing, not held, off cooldown,
    /// and one left. Healing and SP potions start a regeneration (one of
    /// each runs at a time): a potion that would not beat the running one
    /// is refused and kept.
    pub fn use_item(&mut self, user_id: u32, item: &str, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let tick = self.tick;
        let hz = self.config.tick_hz;
        let p = &mut self.players[i];
        let Some(f) = p.fighter.as_mut() else {
            out.push((user_id, HubEvent::Control(hub_error("not_in_stage", "potions are for missions"))));
            return;
        };
        // The quick-use keys: the server picks ([`super::Fighter::best_potion`]).
        let pool = match item {
            QUICK_HP => Some(0),
            QUICK_SP => Some(1),
            _ => None,
        };
        let item = match pool.map(|pool| f.best_potion(pool)) {
            None => item,
            Some(Ok(id)) => id,
            Some(Err(why)) => {
                let (code, text) = match (why, pool == Some(0)) {
                    (NoPick::NoneLeft, true) => ("not_in_bag", "No healing potions left."),
                    (NoPick::NoneLeft, false) => ("not_in_bag", "No SP potions left."),
                    (NoPick::Full, true) => ("hp_full", "Your HP is full (or a potion is still working)."),
                    (NoPick::Full, false) => ("sp_full", "Your SP is full (or a potion is still working)."),
                };
                out.push((user_id, HubEvent::Control(hub_error(code, text))));
                return;
            }
        };
        let (id, effect) = match items::find(item).map(|e| (e.id, e.kind)) {
            Some((id, Kind::Consumable(e))) => (id, e),
            _ => {
                out.push((user_id, HubEvent::Control(hub_error("not_usable", "that cannot be used"))));
                return;
            }
        };
        let Some(slot) = f.potions.iter().position(|(p, n)| p == item && *n > 0) else {
            out.push((user_id, HubEvent::Control(hub_error("not_in_bag", "none left"))));
            return;
        };
        // The downed and the dead drink nothing (only a revive raises them);
        // the potion stays in the bag.
        if !f.is_up() {
            out.push((user_id, HubEvent::Control(hub_error("downed", "You cannot use items while down."))));
            return;
        }
        if f.curses.intersects(crate::world::modifiers::ModFlags::NO_POTIONS) {
            out.push((user_id, HubEvent::Control(hub_error("no_potions", "Abstinence: no potions on this run."))));
            return;
        }
        if f.status.held() || tick < f.potion_ready {
            return;
        }
        let ticks = (POTION_REGEN_MS * hz).div_ceil(1000).max(1);
        match effect {
            Effect::Heal(d) | Effect::Spirit(d) => {
                let pool = matches!(effect, Effect::Spirit(_)) as usize;
                // Compared by the average, rolled once accepted.
                let average = (d.average_x2() + 1) / 2;
                if let Some(r) = &f.regen[pool]
                    && !r.beaten_by(average, ticks)
                {
                    let what = if pool == 0 { "healing" } else { "SP" };
                    out.push((user_id, HubEvent::Control(hub_error("potion_weaker", format!("A stronger {what} potion is still working.")))));
                    return;
                }
                // A Periapt of Wound Closure makes every drink count for more.
                let total = ((d.roll(&mut self.dice, false).max(1) as f32) * (1.0 + f.mods.potion.max(0.0))).round() as i32;
                f.regen[pool] = Some(Regen { item: id, left: total, total, ticks, span: ticks, shown: 0, started: tick });
            }
            Effect::Cure => {
                f.cure(Condition::Poisoned);
                f.cure(Condition::Blinded);
            }
            Effect::Antidote => f.cure(Condition::Poisoned),
        }
        // The Black Mushroom's card: spores against poison.
        if f.procs().spore_cure && matches!(effect, Effect::Heal(_) | Effect::Spirit(_)) {
            f.cure(Condition::Poisoned);
        }
        f.potions[slot].1 -= 1;
        // The Picky's card: quicker sips.
        let cooldown = POTION_COOLDOWN_MS * (100 - f.procs().potion_cd as u32) / 100;
        f.potion_ready = tick + (cooldown * hz).div_ceil(1000);
        out.push((user_id, HubEvent::Control(ServerMsg::ItemUsed { item: item.into(), cooldown_ms: cooldown })));
        out.push((user_id, HubEvent::ItemUsed { item: item.into() }));
    }
}

/// `ITEM_USE` of the quick-use keys: the best HP / SP consumable.
pub const QUICK_HP: &str = "@hp";
pub const QUICK_SP: &str = "@sp";

/// Why [`super::Fighter::best_potion`] picked nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoPick {
    /// Nothing of that pool carried (or nothing that beats the running one).
    NoneLeft,
    /// The bar is full, or the running potion already covers the rest.
    Full,
}

impl HubInstance {
    /// Auto-use (`USE_PREFS`): a fighter below their HP (then SP) threshold
    /// drinks the best consumable, through the same checks as a key press
    /// (off cooldown, up, not held, no Abstinence). Never replaces a running
    /// regeneration of that pool.
    pub(super) fn auto_use(&mut self, out: &mut Outgoing) {
        let tick = self.tick;
        let mut drinks: Vec<(u32, &'static str)> = Vec::new();
        for p in &self.players {
            let (Some(f), uses) = (&p.fighter, p.uses) else { continue };
            if uses.hp == 0 && uses.sp == 0 {
                continue;
            }
            if !f.is_up() || f.status.held() || tick < f.potion_ready || f.curses.intersects(crate::world::modifiers::ModFlags::NO_POTIONS) {
                continue;
            }
            let low = |cur: i32, max: i32, pct: u8| pct > 0 && max > 0 && i64::from(cur) * 100 < i64::from(max) * i64::from(pct);
            let want = [low(f.hp, f.max_hp, uses.hp), low(f.sp, f.max_sp, uses.sp)];
            if let Some(id) = (0..2).filter(|&pool| want[pool] && f.regen[pool].is_none()).find_map(|pool| f.best_potion(pool).ok()) {
                drinks.push((p.profile.user_id, id));
            }
        }
        for (user_id, id) in drinks {
            self.use_item(user_id, id, out);
        }
    }
}

/// A potion's regeneration: `left` HP (or SP) over `ticks` more ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Regen {
    pub item: &'static str,
    pub left: i32,
    pub total: i32,
    pub ticks: u32,
    /// Full length (ticks).
    pub span: u32,
    /// Healed and not yet shown as a number.
    pub shown: i32,
    pub started: u32,
}

impl Regen {
    /// A new potion of `average` over `ticks` replaces this one only if it
    /// is stronger (more to come, or faster) or runs longer.
    pub fn beaten_by(&self, average: i32, ticks: u32) -> bool {
        let rate = |amount: i32, t: u32| amount as f32 / t.max(1) as f32;
        average > self.left || rate(average, ticks) > rate(self.total, self.span) || ticks > self.ticks + self.span / 2
    }

    /// One tick: what it gives now (an even share, rounded up; the rest
    /// at once when no ticks are left, which the old `ticks.max(1)` form
    /// got one short).
    fn step(&mut self) -> i32 {
        let give = match self.ticks {
            0 => self.left,
            t => (self.left + t as i32 - 1) / t as i32,
        };
        self.left -= give;
        self.ticks = self.ticks.saturating_sub(1);
        give
    }
}

impl super::Fighter {
    /// The consumable a quick-use key (and auto-use) drinks from `pool` (0
    /// HP, 1 SP), among those carried that would beat the running one.
    ///
    /// What is wanted is the missing amount (less what the running potion
    /// still gives), but at least a quarter of the bar: a drink heals over
    /// six seconds while the hits keep coming, and only one goes down a
    /// second, so a fruit at a scratch is a wasted press. The weakest one
    /// that covers it wins (least waste); if none does, the strongest. Equal
    /// amounts: a potion before food, then by id.
    pub(super) fn best_potion(&self, pool: usize) -> Result<&'static str, NoPick> {
        let (cur, max) = if pool == 0 { (self.hp, self.max_hp) } else { (self.sp, self.max_sp) };
        let running = self.regen[pool].as_ref().map_or(0, |r| r.left);
        let scale = 1.0 + self.mods.potion.max(0.0);
        let food = items::shop("food").unwrap_or_default();
        let mut owned = false;
        let mut best: Option<(i32, bool, &'static str)> = None;
        let missing = max - cur - running;
        let want = missing.max(max / 4);
        for (id, n) in &self.potions {
            if *n == 0 {
                continue;
            }
            let Some(e) = items::find(id) else { continue };
            let d = match (e.kind, pool) {
                (Kind::Consumable(Effect::Heal(d)), 0) | (Kind::Consumable(Effect::Spirit(d)), 1) => d,
                _ => continue,
            };
            owned = true;
            let average = (d.average_x2() + 1) / 2;
            // Every drink runs the same span: a new one is that long.
            if let Some(r) = &self.regen[pool]
                && !r.beaten_by(average, r.span)
            {
                continue;
            }
            let value = (average as f32 * scale).round() as i32;
            let is_food = food.contains(&e.id);
            let better = match best {
                None => true,
                Some((bv, bf, bid)) => {
                    let (covers, best_covers) = (value >= want, bv >= want);
                    match (covers, best_covers) {
                        (true, false) => true,
                        (false, true) => false,
                        // Both cover: the smaller; neither: the larger.
                        _ if value != bv => (value < bv) == covers,
                        _ if is_food != bf => !is_food,
                        _ => e.id < bid,
                    }
                }
            };
            if better {
                best = Some((value, is_food, e.id));
            }
        }
        if missing <= 0 {
            return Err(if owned { NoPick::Full } else { NoPick::NoneLeft });
        }
        best.map(|(_, _, id)| id).ok_or(NoPick::NoneLeft)
    }

    /// One tick of the running potions; once a second (`every` ticks) and
    /// at the end, the HP healed since the last number.
    pub(super) fn potion_tick(&mut self, every: u32) -> Option<i32> {
        let mut shown = None;
        if let Some(r) = self.regen[0].as_mut() {
            let give = r.step().min(self.max_hp - self.hp).max(0);
            self.hp += give;
            r.shown += give;
            if (r.ticks.is_multiple_of(every.max(1)) || r.ticks == 0) && r.shown > 0 {
                shown = Some(std::mem::take(&mut r.shown));
            }
            if r.ticks == 0 {
                self.regen[0] = None;
            }
        }
        if let Some(r) = self.regen[1].as_mut() {
            self.sp = (self.sp + r.step()).min(self.max_sp);
            if r.ticks == 0 {
                self.regen[1] = None;
            }
        }
        shown
    }

    /// Running regenerations as ("item:<id>", ticks left), for the buff bar.
    pub(super) fn regen_list(&self) -> impl Iterator<Item = (String, u32)> + '_ {
        self.regen.iter().flatten().map(|r| (format!("item:{}", r.item), r.ticks))
    }

    /// Changes when a regeneration starts or ends.
    pub(super) fn regen_signature(&self) -> u64 {
        self.regen.iter().flatten().fold(0, |h, r| h.rotate_left(7) ^ (u64::from(r.started) << 8 | r.item.len() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use super::super::test_support::*;
    use super::super::{HubEvent, HubInstance, HubSimConfig, HubWorld, PlayerProfile};
    use crate::world::combat::{self, Bag, Class, Condition};
    use crate::world::map::{CameraConstraint, LocationKind};
    use crate::world::protocol::ServerMsg;
    use crate::world::trade::ShopOrder;
    use std::sync::Arc;
    use uuid::Uuid;

    /// A regeneration hands out all it has: in even shares rounded up, and
    /// whatever is left when no ticks are (never one short).
    #[test]
    fn regen_steps_give_everything() {
        let mut r = super::Regen { item: "red_potion", left: 10, total: 10, ticks: 3, span: 3, shown: 0, started: 0 };
        let given: Vec<i32> = (0..3).map(|_| r.step()).collect();
        assert_eq!(given, vec![4, 3, 3]);
        assert_eq!((r.left, r.ticks), (0, 0));
        let mut r = super::Regen { left: 7, ticks: 0, ..r };
        assert_eq!(r.step(), 7);
        assert_eq!(r.left, 0);
    }

    use super::super::tests::{profile, quiet};

    fn events(out: &[(u32, HubEvent)], user: u32) -> Vec<HubEvent> {
        out.iter().filter(|(u, _)| *u == user).map(|(_, e)| e.clone()).collect()
    }

    fn place(hub: &mut HubInstance, user: u32, x: f32, y: f32) {
        let i = hub.find(user).unwrap();
        (hub.players[i].x, hub.players[i].y) = (x, y);
    }

    /// A field with one monster far away and a swordsman in it.
    fn stage() -> HubInstance {
        use crate::world::horde::catalog::{test_def, MonsterCatalog};
        use crate::world::horde::{Horde, HordeConfig};
        let field = map(
            "field",
            LocationKind::Field,
            CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
            &["#########", "#.......#", "#.......#", "#.......#", "#########"],
            vec![portal("to_town", [1, 2, 1], "town", [3, 3])],
        );
        let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
        let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 2.0, 1, 0.0, false)]).unwrap());
        let cfg = HordeConfig { light_radius: 6.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
        let horde = Horde::new(Arc::clone(&world), catalog, cfg);
        let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde);
        stage.dice = combat::Rng::new(3);
        stage.join(PlayerProfile { archetype: "swordsman".into(), ..profile(1) }, Uuid::nil(), None, 0, &mut Vec::new());
        stage
    }

    fn fighter(stage: &mut HubInstance) -> &mut super::super::Fighter {
        let i = stage.find(1).unwrap();
        stage.players[i].fighter.as_mut().unwrap()
    }

    fn order(buy: &[(&str, u32)], sell: &[(&str, u32)]) -> ShopOrder {
        let lines = |l: &[(&str, u32)]| l.iter().map(|(i, n)| (i.to_string(), *n)).collect();
        ShopOrder { buy: lines(buy), sell: lines(sell) }
    }

    fn refused(out: &[(u32, HubEvent)], want: &str) -> bool {
        matches!(&events(out, 1)[..], [HubEvent::Control(ServerMsg::HubError { code, .. })] if code == want)
    }

    #[test]
    fn a_smith_rolls_a_refine_only_for_players_standing_at_it() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        place(&mut hub, 1, 2.5, 2.5);
        out.clear();
        hub.refine(1, "smith", "sword".into(), 4, &mut out);
        assert!(refused(&out, "npc_too_far"), "{out:?}");
        out.clear();
        // The smith stands at (8,1); the merchant is no smith.
        place(&mut hub, 1, 8.5, 2.0);
        hub.refine(1, "merchant", "sword".into(), 4, &mut out);
        assert!(refused(&out, "no_such_npc"), "{out:?}");
        out.clear();
        hub.refine(1, "smith", "sword".into(), 42, &mut out);
        assert!(refused(&out, "bad_message"), "{out:?}");
        out.clear();
        hub.refine(1, "smith", "sword".into(), 4, &mut out);
        assert!(
            matches!(&events(&out, 1)[..], [HubEvent::Refine { item, slot: Some(crate::world::combat::Slot::Weapon), roll, slip }] if item == "sword" && (1..=100).contains(roll) && (1..=24).contains(slip)),
            "{out:?}"
        );
        out.clear();
        hub.refine(1, "smith", "coat".into(), 0xFF, &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Refine { slot: None, .. }], "{out:?}");
    }

    #[test]
    fn the_e2e_hooks_rig_the_next_refine_and_hand_out_refined_copies() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), HubSimConfig { missions_anywhere: true, ..quiet() });
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        place(&mut hub, 1, 8.5, 2.0);
        out.clear();
        hub.chat(1, "/give sword+8".into(), &mut out);
        hub.chat(1, "/give zeny 5000".into(), &mut out);
        hub.chat(1, "/give sword+11".into(), &mut out);
        let got = events(&out, 1);
        assert_matches!(&got[..], [HubEvent::DevGive { key }, HubEvent::Zeny { amount: 5000 }, HubEvent::Control(ServerMsg::Chat { .. })] if key == "sword+8", "{got:?}");
        out.clear();
        // One rigged roll, then the dice again.
        hub.chat(1, "/rig 100 3".into(), &mut out);
        assert!(out.is_empty(), "{out:?}");
        hub.refine(1, "smith", "sword+8".into(), 0xFF, &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Refine { roll: 100, slip: 3, .. }], "{out:?}");
        out.clear();
        hub.refine(1, "smith", "sword+8".into(), 0xFF, &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Refine { .. }], "{out:?}");
        // Without the test hooks it is only chat.
        let mut plain = HubInstance::new(1, Arc::new(test_world()), quiet());
        plain.join(profile(1), Uuid::nil(), None, 0, &mut out);
        out.clear();
        plain.chat(1, "/rig 100 3".into(), &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Control(ServerMsg::Chat { .. })], "{out:?}");
    }

    #[test]
    fn merchants_trade_only_with_players_standing_at_them() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        place(&mut hub, 1, 2.5, 2.5);
        out.clear();
        hub.shop(1, "merchant", order(&[("red_potion", 2)], &[]), &mut out);
        assert!(refused(&out, "npc_too_far"), "{out:?}");
        out.clear();
        // The merchant stands at (5,5).
        place(&mut hub, 1, 5.5, 4.5);
        hub.shop(1, "merchant", order(&[("sword", 1)], &[]), &mut out);
        assert!(refused(&out, "not_sold_here"), "{out:?}");
        out.clear();
        let o = order(&[("red_potion", 2)], &[]);
        hub.shop(1, "merchant", o.clone(), &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Shop(got, _)] if *got == o, "{out:?}");
        out.clear();
        // Anything can be sold; the board is no merchant.
        hub.shop(1, "merchant", order(&[], &[("sword", 1)]), &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Shop(..)], "{out:?}");
        out.clear();
        hub.shop(1, "board", order(&[], &[("sword", 1)]), &mut out);
        assert!(refused(&out, "no_such_npc"), "{out:?}");
        out.clear();
        // Talking to it opens the goods.
        hub.interact(1, "merchant", &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Control(ServerMsg::Shop { items, .. })] if items.contains(&"red_potion".to_string()), "{out:?}");
    }

    #[test]
    fn a_cart_goes_to_the_bag_whole_or_is_refused_whole() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        place(&mut hub, 1, 5.5, 4.5);
        out.clear();
        // Several buys and sells at once, one event.
        let cart = order(&[("red_potion", 12), ("blue_potion", 1)], &[("jellopy", 300), ("sword+7", 1)]);
        hub.shop(1, "merchant", cart.clone(), &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Shop(got, _)] if *got == cart, "{out:?}");
        // One line the merchant does not sell refuses the whole cart.
        out.clear();
        hub.shop(1, "merchant", order(&[("red_potion", 12), ("sword", 1)], &[("jellopy", 3)]), &mut out);
        assert!(refused(&out, "not_sold_here"), "{out:?}");
        for (bad, code) in [
            (order(&[], &[]), "shop_empty"),
            (order(&[("red_potion", 0)], &[]), "shop_bad_count"),
            (order(&[], &[("jellopy", 1000)]), "shop_bad_count"),
            (order(&[], &vec![("jellopy", 1); 21]), "shop_too_many"),
        ] {
            out.clear();
            hub.shop(1, "merchant", bad.clone(), &mut out);
            assert!(refused(&out, code), "{bad:?}: {out:?}");
        }
        // Test hooks (GAME_DEV): merchants trade from anywhere in town.
        let mut cfg = quiet();
        cfg.missions_anywhere = true;
        let mut dev = HubInstance::new(1, Arc::new(test_world()), cfg);
        dev.join(profile(1), Uuid::nil(), None, 0, &mut out);
        place(&mut dev, 1, 2.5, 2.5);
        out.clear();
        dev.interact(1, "merchant", &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Control(ServerMsg::Shop { .. })], "{out:?}");
        out.clear();
        dev.shop(1, "merchant", order(&[("red_potion", 1)], &[]), &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Shop(..)], "{out:?}");
    }

    #[test]
    fn a_new_weapon_is_shown_to_the_map() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        let mut out = Vec::new();
        let bag = Bag::starting(Class::Swordsman, "");
        hub.join(PlayerProfile { archetype: "swordsman".into(), bag: bag.clone(), ..profile(1) }, Uuid::nil(), None, 0, &mut out);
        assert!(events(&out, 1).iter().any(|e| matches!(e, HubEvent::Control(ServerMsg::MapEnter { .. }))));
        hub.join(profile(2), Uuid::nil(), None, 0, &mut out);
        // The newcomer sees the sword (family and item, the server's table).
        assert!(events(&out, 2).iter().any(|e| matches!(e, HubEvent::Control(ServerMsg::MapEnter { players, .. }) if players.iter().any(|p| p.appearance.weapon == "sword:sword"))), "{out:?}");
        out.clear();
        let mut katana = bag.clone();
        katana.zeny = 10_000;
        katana.buy("katana", 1, &["katana"]).unwrap();
        katana.equip(Class::Swordsman, "katana", None).unwrap();
        hub.set_bag(1, katana.clone(), &mut out);
        for to in [1, 2] {
            assert!(
                events(&out, to).iter().any(|e| matches!(e, HubEvent::Control(ServerMsg::PlayerAppearance { entity_id: 101, appearance }) if appearance.weapon == "two_hand_sword:katana")),
                "{to}: {out:?}"
            );
        }
        out.clear();
        hub.set_bag(1, katana, &mut out);
        assert!(out.is_empty(), "same weapon: nothing to say");
    }

    #[test]
    fn headgear_is_shown_to_the_map() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        let mut out = Vec::new();
        let mut bag = Bag::starting(Class::Swordsman, "");
        bag.zeny = 10_000;
        bag.buy("hat", 1, &["hat"]).unwrap();
        bag.equip(Class::Swordsman, "hat", None).unwrap();
        hub.join(PlayerProfile { archetype: "swordsman".into(), bag: bag.clone(), ..profile(1) }, Uuid::nil(), None, 0, &mut out);
        let worn = |out: &[(u32, HubEvent)], to: u32, items: &[&str]| {
            events(out, to).iter().any(|e| {
                matches!(e, HubEvent::Control(ServerMsg::PlayerAppearance { entity_id: 101, appearance: a }) if a.headgear.iter().map(String::as_str).eq(items.iter().copied()))
                    || matches!(e, HubEvent::Control(ServerMsg::MapEnter { players, .. }) if players.iter().any(|p| p.entity_id == 101 && p.appearance.headgear.iter().map(String::as_str).eq(items.iter().copied())))
            })
        };
        // Its own client learns it on arrival, a newcomer too.
        assert!(worn(&out, 1, &["hat"]), "{out:?}");
        out.clear();
        hub.join(profile(2), Uuid::nil(), None, 0, &mut out);
        assert!(worn(&out, 2, &["hat"]), "{out:?}");
        out.clear();
        // Taking it off tells the map; the same bag again says nothing.
        bag.unequip(crate::world::combat::items::Slot::UpperHead).unwrap();
        hub.set_bag(1, bag.clone(), &mut out);
        assert!(worn(&out, 2, &[]) && worn(&out, 1, &[]), "{out:?}");
        out.clear();
        hub.set_bag(1, bag, &mut out);
        assert!(!events(&out, 2).iter().any(|e| matches!(e, HubEvent::Control(ServerMsg::PlayerAppearance { .. }))), "{out:?}");
    }

    #[test]
    fn potions_heal_over_time_never_stack_and_cure() {
        let mut stage = stage();
        let mut bag = Bag::starting(Class::Swordsman, "");
        bag.add("green_potion", 1).unwrap();
        bag.add("yellow_potion", 1).unwrap();
        fighter(&mut stage).wear(&bag);
        let mut out = Vec::new();
        {
            let f = fighter(&mut stage);
            (f.hp, f.max_hp) = (1, 500);
            f.afflict(Condition::Poisoned, 1000);
        }
        stage.use_item(1, "red_potion", &mut out);
        assert_eq!(fighter(&mut stage).hp, 1, "nothing at once");
        assert!(events(&out, 1).iter().any(|e| matches!(e, HubEvent::ItemUsed { item } if item == "red_potion")));
        let total = fighter(&mut stage).regen[0].unwrap().total;
        assert!((8..=20).contains(&total), "4d4+4: {total}");
        out.clear();
        // Cooldown: the next drink waits; after it, another Red Potion
        // does not beat the running one (a top roll) and is kept.
        if let Some(r) = fighter(&mut stage).regen[0].as_mut() {
            (r.left, r.total) = (20, 20);
        }
        stage.use_item(1, "red_potion", &mut out);
        assert!(out.is_empty(), "{out:?}");
        stage.tick += 25;
        stage.use_item(1, "red_potion", &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Control(ServerMsg::HubError { code, .. })] if code == "potion_weaker", "{out:?}");
        assert_eq!(fighter(&mut stage).potions.iter().find(|p| p.0 == "red_potion").unwrap().1, 4);
        out.clear();
        // A Yellow Potion is stronger: it replaces it.
        stage.use_item(1, "yellow_potion", &mut out);
        assert_eq!(fighter(&mut stage).regen[0].unwrap().item, "yellow_potion");
        // The regeneration heals over its span and ends.
        let regen = fighter(&mut stage).regen[0].unwrap();
        let mut healed = 0;
        let mut shown = 0;
        for _ in 0..regen.ticks {
            let f = fighter(&mut stage);
            let before = f.hp;
            shown += f.potion_tick(20).unwrap_or(0);
            healed += f.hp - before;
        }
        assert_eq!((healed, shown), (regen.total, regen.total));
        assert!(fighter(&mut stage).regen[0].is_none());
        out.clear();
        stage.tick += 25;
        stage.use_item(1, "green_potion", &mut out);
        assert!(!fighter(&mut stage).status.has(Condition::Poisoned));
        out.clear();
        stage.tick += 25;
        stage.use_item(1, "green_potion", &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Control(ServerMsg::HubError { code, .. })] if code == "not_in_bag", "{out:?}");
        out.clear();
        // Not in town.
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        out.clear();
        hub.use_item(1, "red_potion", &mut out);
        assert_matches!(&events(&out, 1)[..], [HubEvent::Control(ServerMsg::HubError { code, .. })] if code == "not_in_stage", "{out:?}");
    }

    #[test]
    fn worn_gear_reaches_the_fighter() {
        let mut stage = stage();
        let mut bag = Bag::starting(Class::Swordsman, "");
        bag.zeny = 100_000;
        for id in ["full_plate", "helm"] {
            bag.buy(id, 1, &[id]).unwrap();
            bag.equip(Class::Swordsman, id, None).unwrap();
        }
        let f = fighter(&mut stage);
        let (hp, ac) = (f.max_hp, f.ac);
        assert_eq!(ac, 16);
        f.wear(&bag);
        // Plate 18 + guard 2 + helm 1, instead of shirt 11 + defence 3 + guard 2.
        assert_eq!((f.ac, f.max_hp, f.hp), (ac + 5, hp + 10, hp + 10));
    }

    #[test]
    fn the_downed_and_the_dead_drink_nothing_and_a_running_potion_stops() {
        use super::super::Vital;
        let mut stage = stage();
        fighter(&mut stage).wear(&Bag::starting(Class::Swordsman, ""));
        let mut out = Vec::new();
        let reds = |stage: &mut HubInstance| fighter(stage).potions.iter().find(|(id, _)| id == "red_potion").map_or(0, |p| p.1);
        let before = reds(&mut stage);
        assert!(before > 0, "the starting kit carries red potions");
        // A potion running when the blow lands stops healing.
        fighter(&mut stage).hp = 1;
        stage.use_item(1, "red_potion", &mut out);
        assert!(fighter(&mut stage).regen[0].is_some());
        {
            let f = fighter(&mut stage);
            f.hp = 0;
            f.fall_if_spent(20.0);
            assert!(matches!(f.state, Vital::Downed { .. }) && f.regen[0].is_none());
        }
        for state in [Vital::Downed { ticks: 400 }, Vital::Dead] {
            fighter(&mut stage).state = state;
            stage.tick += 100;
            out.clear();
            stage.use_item(1, "red_potion", &mut out);
            assert!(
                matches!(&events(&out, 1)[..], [HubEvent::Control(ServerMsg::HubError { code, .. })] if code == "downed"),
                "{state:?}: {out:?}"
            );
            assert_eq!(reds(&mut stage), before - 1, "{state:?}: the potion stays in the bag");
            assert!(fighter(&mut stage).regen.iter().all(Option::is_none));
        }
        // Nothing heals them while they lie there: no regeneration either
        // (alone, the run is lost; the result screen holds for 5 s).
        fighter(&mut stage).state = Vital::Downed { ticks: 400 };
        for _ in 0..60 {
            stage.tick(&mut out);
        }
        assert_eq!(fighter(&mut stage).hp, 0);
    }

    fn carry(stage: &mut HubInstance, potions: &[(&str, u32)]) {
        let f = fighter(stage);
        f.potions = potions.iter().map(|(id, n)| (id.to_string(), *n)).collect();
        f.regen = [None, None];
    }

    fn set_hp(stage: &mut HubInstance, hp: i32, max_hp: i32) {
        let f = fighter(stage);
        (f.hp, f.max_hp) = (hp, max_hp);
    }

    #[test]
    fn the_quick_heal_picks_the_potion_that_covers_the_wound_with_least_waste() {
        use super::NoPick;
        let mut stage = stage();
        let kit = [("apple", 20), ("red_potion", 5), ("yellow_potion", 3), ("white_potion", 2), ("blue_potion", 1)];
        carry(&mut stage, &kit);
        // A scratch still drinks something that matters: at least a quarter
        // of the bar (125 of 500) is wanted, nothing covers it, the
        // strongest goes — never the apple.
        set_hp(&mut stage, 495, 500);
        assert_eq!(fighter(&mut stage).best_potion(0), Ok("white_potion"));
        // 200 max, 150 missing: the White Potion (90) is the strongest.
        set_hp(&mut stage, 50, 200);
        assert_eq!(fighter(&mut stage).best_potion(0), Ok("white_potion"));
        // 200 max, 50 missing: the Yellow Potion (56) covers it with least waste.
        set_hp(&mut stage, 150, 200);
        assert_eq!(fighter(&mut stage).best_potion(0), Ok("yellow_potion"));
        // A small bar: the Red Potion (14) covers a quarter of 40 and more.
        set_hp(&mut stage, 30, 40);
        assert_eq!(fighter(&mut stage).best_potion(0), Ok("red_potion"));
        // Only food: the fruit it is.
        carry(&mut stage, &[("apple", 3), ("meat", 1)]);
        set_hp(&mut stage, 100, 200);
        assert_eq!(fighter(&mut stage).best_potion(0), Ok("meat"));
        // Equal amounts (4d4+4 both): by id, deterministic.
        carry(&mut stage, &[("yellow_herb", 1), ("red_potion", 1)]);
        set_hp(&mut stage, 30, 40);
        assert_eq!(fighter(&mut stage).best_potion(0), Ok("red_potion"));
        // Full, or nothing carried; SP has its own pool.
        set_hp(&mut stage, 40, 40);
        assert_eq!(fighter(&mut stage).best_potion(0), Err(NoPick::Full));
        carry(&mut stage, &[("blue_potion", 1), ("apple", 0)]);
        set_hp(&mut stage, 10, 40);
        assert_eq!(fighter(&mut stage).best_potion(0), Err(NoPick::NoneLeft));
        let f = fighter(&mut stage);
        (f.sp, f.max_sp) = (0, 100);
        assert_eq!(f.best_potion(1), Ok("blue_potion"));
        // A running White Potion: weaker drinks would be refused, so they
        // are not picked; what it still gives counts as healed.
        carry(&mut stage, &kit);
        set_hp(&mut stage, 100, 200);
        let mut out = Vec::new();
        stage.use_item(1, "white_potion", &mut out);
        assert_eq!(fighter(&mut stage).regen[0].unwrap().item, "white_potion");
        // Half of it left (60 of a 60 roll): only another White Potion beats it.
        if let Some(r) = fighter(&mut stage).regen[0].as_mut() {
            (r.left, r.total) = (60, 60);
        }
        assert_eq!(fighter(&mut stage).best_potion(0), Ok("white_potion"), "beats a running one with 60 left");
        if let Some(r) = fighter(&mut stage).regen[0].as_mut() {
            r.left = 100;
        }
        assert_eq!(fighter(&mut stage).best_potion(0), Err(NoPick::Full), "the running one covers the rest");
    }

    #[test]
    fn the_quick_use_keys_drink_the_servers_pick() {
        let mut stage = stage();
        carry(&mut stage, &[("apple", 20), ("white_potion", 2)]);
        set_hp(&mut stage, 480, 500);
        let mut out = Vec::new();
        stage.use_item(1, super::QUICK_HP, &mut out);
        assert!(events(&out, 1).iter().any(|e| matches!(e, HubEvent::ItemUsed { item } if item == "white_potion")), "{out:?}");
        assert_eq!(fighter(&mut stage).potions[0], ("apple".to_string(), 20), "no apple eaten");
        // Out of SP potions: told so, nothing drunk.
        out.clear();
        stage.tick += 25;
        let f = fighter(&mut stage);
        (f.sp, f.max_sp) = (0, 100);
        stage.use_item(1, super::QUICK_SP, &mut out);
        assert!(refused(&out, "not_in_bag"), "{out:?}");
        // Full HP: refused, kept.
        out.clear();
        carry(&mut stage, &[("white_potion", 1)]);
        set_hp(&mut stage, 500, 500);
        stage.use_item(1, super::QUICK_HP, &mut out);
        assert!(refused(&out, "hp_full"), "{out:?}");
        assert_eq!(fighter(&mut stage).potions[0].1, 1);
    }

    #[test]
    fn auto_use_drinks_below_the_threshold_and_respects_the_cooldown() {
        use crate::world::protocol::UsePrefs;
        let mut stage = stage();
        carry(&mut stage, &[("red_potion", 5), ("blue_potion", 5)]);
        set_hp(&mut stage, 60, 100);
        {
            let f = fighter(&mut stage);
            (f.sp, f.max_sp) = (10, 100);
        }
        let mut out = Vec::new();
        // Off: nothing.
        stage.auto_use(&mut out);
        assert!(out.is_empty());
        // HP 60% is not below 50%; SP 10% is below 20%.
        stage.use_prefs(1, UsePrefs { hp: 50, sp: 20 });
        stage.auto_use(&mut out);
        assert!(events(&out, 1).iter().any(|e| matches!(e, HubEvent::ItemUsed { item } if item == "blue_potion")), "{out:?}");
        // HP drops below: the shared potion cooldown holds it back.
        out.clear();
        set_hp(&mut stage, 40, 100);
        stage.auto_use(&mut out);
        assert!(out.is_empty(), "cooldown: {out:?}");
        stage.tick += 25;
        stage.auto_use(&mut out);
        assert!(events(&out, 1).iter().any(|e| matches!(e, HubEvent::ItemUsed { item } if item == "red_potion")), "{out:?}");
        // A running regeneration is never replaced; the SP one is still
        // running, so SP is not topped up again either.
        out.clear();
        stage.tick += 25;
        stage.auto_use(&mut out);
        assert!(out.is_empty(), "both regenerations still run: {out:?}");
        // Abstinence and the downed: nothing, and no error spam.
        carry(&mut stage, &[("red_potion", 5)]);
        stage.tick += 25;
        fighter(&mut stage).curses = crate::world::modifiers::ModFlags::NO_POTIONS;
        stage.auto_use(&mut out);
        assert!(out.is_empty(), "{out:?}");
        fighter(&mut stage).curses = crate::world::modifiers::ModFlags::NONE;
        // Not in town: no fighter, nothing.
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        hub.use_prefs(1, UsePrefs { hp: 90, sp: 90 });
        out.clear();
        hub.auto_use(&mut out);
        assert!(out.is_empty());
        // Live: a tick of the stage drinks on its own.
        stage.tick(&mut out);
        assert!(events(&out, 1).iter().any(|e| matches!(e, HubEvent::ItemUsed { item } if item == "red_potion")), "{out:?}");
    }
}
