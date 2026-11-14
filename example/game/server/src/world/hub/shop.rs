//! Items in an instance (COMBAT.md §2.6): merchants check that the player
//! stands at them and sells what is asked, potions are drunk on missions,
//! and a new weapon is shown to everyone on the map.
//!
//! The bag itself lives with the session's character (the socket task):
//! orders come back to it as [`HubEvent::Shop`], potions as
//! [`HubEvent::ItemUsed`], so the stage never writes belongings it does
//! not own.

use super::{hub_error, HubEvent, HubInstance, Outgoing, SHOP};
use crate::world::combat::items::{self, Effect, Kind, POTION_COOLDOWN_MS, POTION_REGEN_MS};
use crate::world::combat::{Bag, Condition, Weapon};
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
    /// all of it or nothing.
    pub fn shop(&mut self, user_id: u32, npc: &str, order: ShopOrder, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        if self.is_trading(user_id) {
            out.push((user_id, HubEvent::Control(hub_error("trade_busy", "Finish your trade first."))));
            return;
        }
        let p = &self.players[i];
        let map = self.world.map(p.map);
        let event = match map.manifest.npcs.iter().find(|n| n.id == npc && n.kind == SHOP) {
            None => HubEvent::Control(hub_error("no_such_npc", format!("no merchant {npc} here"))),
            Some(n) if !self.at_merchant(n, p.x, p.y) => HubEvent::Control(hub_error("npc_too_far", format!("walk closer to {}", n.name))),
            Some(n) => match order.check(items::shop(&n.shop).unwrap_or_default()) {
                Err(OrderError::NotSoldHere) => HubEvent::Control(hub_error("not_sold_here", format!("{} does not sell that", n.name))),
                Err(e) => HubEvent::Control(hub_error(e.code(), e.message())),
                Ok(()) => HubEvent::Shop(order),
            },
        };
        out.push((user_id, event));
    }

    /// Standing at merchant `n` (anywhere, with the `GAME_DEV` test hooks:
    /// the e2e shop opens without a walk).
    pub(super) fn at_merchant(&self, n: &crate::world::map::NpcDef, x: f32, y: f32) -> bool {
        n.in_reach(x, y) || self.config.missions_anywhere
    }

    /// The player's belongings changed (the socket task's copy): the next
    /// stage starts from them, and a new weapon is shown to the map.
    pub fn set_bag(&mut self, user_id: u32, bag: Bag, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let before = self.players[i].weapon_id().to_string();
        let worn = self.players[i].headgear();
        let p = &mut self.players[i];
        p.profile.weapon = bag.weapon().to_string();
        p.profile.bag = bag;
        let after = self.players[i].weapon_id().to_string();
        if after != before {
            let p = &self.players[i];
            let msg = ServerMsg::PlayerGear { entity_id: p.profile.character_id, weapon: after };
            self.broadcast(p.map, &msg, out);
        }
        let p = &self.players[i];
        if p.headgear() != worn {
            self.broadcast(p.map, &p.headgear_msg(), out);
        }
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
                let total = d.roll(&mut self.dice, false).max(1);
                f.regen[pool] = Some(Regen { item: id, left: total, total, ticks, span: ticks, shown: 0, started: tick });
            }
            Effect::Cure => {
                f.cure(Condition::Poisoned);
                f.cure(Condition::Blinded);
            }
            Effect::Antidote => f.cure(Condition::Poisoned),
        }
        f.potions[slot].1 -= 1;
        f.potion_ready = tick + (POTION_COOLDOWN_MS * hz).div_ceil(1000);
        out.push((user_id, HubEvent::Control(ServerMsg::ItemUsed { item: item.into(), cooldown_ms: POTION_COOLDOWN_MS })));
        out.push((user_id, HubEvent::ItemUsed { item: item.into() }));
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

    /// One tick: what it gives now.
    fn step(&mut self) -> i32 {
        let give = (self.left + self.ticks as i32 - 1) / self.ticks.max(1) as i32;
        self.left -= give;
        self.ticks = self.ticks.saturating_sub(1);
        give
    }
}

impl super::Fighter {
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
    use super::super::test_support::*;
    use super::super::{HubEvent, HubInstance, HubSimConfig, HubWorld, PlayerProfile};
    use crate::world::combat::{self, Bag, Class, Condition};
    use crate::world::map::{CameraConstraint, LocationKind};
    use crate::world::protocol::ServerMsg;
    use crate::world::trade::ShopOrder;
    use std::sync::Arc;
    use uuid::Uuid;

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
        assert!(matches!(&events(&out, 1)[..], [HubEvent::Shop(got)] if *got == o), "{out:?}");
        out.clear();
        // Anything can be sold; the board is no merchant.
        hub.shop(1, "merchant", order(&[], &[("sword", 1)]), &mut out);
        assert!(matches!(&events(&out, 1)[..], [HubEvent::Shop(_)]), "{out:?}");
        out.clear();
        hub.shop(1, "board", order(&[], &[("sword", 1)]), &mut out);
        assert!(refused(&out, "no_such_npc"), "{out:?}");
        out.clear();
        // Talking to it opens the goods.
        hub.interact(1, "merchant", &mut out);
        assert!(matches!(&events(&out, 1)[..], [HubEvent::Control(ServerMsg::Shop { items, .. })] if items.contains(&"red_potion".to_string())), "{out:?}");
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
        assert!(matches!(&events(&out, 1)[..], [HubEvent::Shop(got)] if *got == cart), "{out:?}");
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
        assert!(matches!(&events(&out, 1)[..], [HubEvent::Control(ServerMsg::Shop { .. })]), "{out:?}");
        out.clear();
        dev.shop(1, "merchant", order(&[("red_potion", 1)], &[]), &mut out);
        assert!(matches!(&events(&out, 1)[..], [HubEvent::Shop(_)]), "{out:?}");
    }

    #[test]
    fn a_new_weapon_is_shown_to_the_map() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        let mut out = Vec::new();
        let bag = Bag::starting(Class::Swordsman, "");
        hub.join(PlayerProfile { archetype: "swordsman".into(), bag: bag.clone(), ..profile(1) }, Uuid::nil(), None, 0, &mut out);
        assert!(events(&out, 1).iter().any(|e| matches!(e, HubEvent::Control(ServerMsg::MapEnter { .. }))));
        hub.join(profile(2), Uuid::nil(), None, 0, &mut out);
        // The newcomer sees the sword.
        assert!(events(&out, 2).iter().any(|e| matches!(e, HubEvent::Control(ServerMsg::MapEnter { players, .. }) if players.iter().any(|p| p.weapon == "sword"))), "{out:?}");
        out.clear();
        let mut katana = bag.clone();
        katana.zeny = 10_000;
        katana.buy("katana", 1, &["katana"]).unwrap();
        katana.equip(Class::Swordsman, "katana", None).unwrap();
        hub.set_bag(1, katana.clone(), &mut out);
        assert!(events(&out, 2).iter().any(|e| matches!(e, HubEvent::Control(ServerMsg::PlayerGear { weapon, .. }) if weapon == "katana")), "{out:?}");
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
                matches!(e, HubEvent::Control(ServerMsg::PlayerHeadgear { entity_id: 101, items: got }) if got.iter().map(String::as_str).eq(items.iter().copied()))
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
        assert!(!events(&out, 2).iter().any(|e| matches!(e, HubEvent::Control(ServerMsg::PlayerHeadgear { .. }))), "{out:?}");
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
        assert!(matches!(&events(&out, 1)[..], [HubEvent::Control(ServerMsg::HubError { code, .. })] if code == "potion_weaker"), "{out:?}");
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
        assert!(matches!(&events(&out, 1)[..], [HubEvent::Control(ServerMsg::HubError { code, .. })] if code == "not_in_bag"), "{out:?}");
        out.clear();
        // Not in town.
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        out.clear();
        hub.use_item(1, "red_potion", &mut out);
        assert!(matches!(&events(&out, 1)[..], [HubEvent::Control(ServerMsg::HubError { code, .. })] if code == "not_in_stage"), "{out:?}");
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
}
