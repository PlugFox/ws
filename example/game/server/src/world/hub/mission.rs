//! The end of a mission (COMBAT.md §4.6) and the HUD extras around it: the
//! boss's bar, each player's buff icons, the result screen.
//!
//! Victory: the boss dies, or the party lasts the mission's time limit.
//! Defeat: nobody on the map is standing. Either way everyone gets
//! `STAGE_END` (time, party kills, their experience) and leaves through the
//! stage exit a few seconds later.
//!
//! Mission modifiers (`world::modifiers`) live in the horde's config; this
//! module shows them (`MODIFIERS` on joining, for the HUD) and pays a won
//! run's clear bonus for their risk (`MODIFIERS` reward, before
//! `STAGE_END`).

use super::{HubEvent, HubInstance, InstanceKind, MapIdx, Outgoing, PortalLink};
use crate::world::combat::items;
use crate::world::horde::storage::FLAG_BOSS;
use crate::world::combat::{loot, Archetype};
use crate::world::missions;
use crate::world::modifiers::{self, Modifiers};
use crate::world::protocol::{ServerMsg, MONSTER_ENTITY_BASE};

/// Result screen time before everyone leaves (seconds).
const VICTORY_SECS: f32 = 12.0;
const DEFEAT_SECS: f32 = 5.0;
/// The boss bar is resent at most this often (ticks).
const BOSS_EVERY: u32 = 4;
/// Without a boss, look for one this often (ticks).
const BOSS_SCAN: u32 = 20;
/// The run's clock is resent this often (seconds) so the timeline cannot
/// drift.
const CLOCK_SECS: u32 = 10;

/// The boss the party is fighting.
#[derive(Clone, Copy, Debug)]
pub(super) struct BossBar {
    slot: u16,
    generation: u8,
    sent_hp: i32,
    wait: u32,
}

impl BossBar {
    /// Sends the bar again with the next check (a player re-attached).
    pub(super) fn resend(&mut self) {
        self.sent_hp = i32::MIN;
        self.wait = 0;
    }
}

/// How far the mission is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Outcome {
    #[default]
    Running,
    /// Everyone leaves at this tick.
    Over { leave_at: u32 },
}

impl HubInstance {
    /// A stage under mission modifiers: the fog follows Creeping Fog (the
    /// horde, made with `HordeConfig::with_modifiers`, carries the rest).
    pub fn with_modifiers(mut self, mods: Modifiers) -> Self {
        if let InstanceKind::Stage { mission, .. } = &self.kind {
            self.fog = super::fog::Fog::new(missions::find(mission).and_then(|m| m.fog).map(|f| mods.fog(f)));
        }
        self
    }

    /// The run's modifiers (none without a horde).
    pub(super) fn modifiers(&self) -> Modifiers {
        self.horde.as_ref().map_or(Modifiers::NONE, |h| h.modifiers())
    }

    /// After the combat step: boss bar, buffs, victory and leaving.
    /// `boss_down`: the boss died this tick.
    pub(super) fn mission_step(&mut self, map: MapIdx, boss_down: bool, out: &mut Outgoing) {
        if let Outcome::Over { leave_at } = self.outcome {
            if self.tick >= leave_at {
                self.leave_stage(map, out);
            }
            return;
        }
        self.boss_bar(map, out);
        self.monster_plates(map, out);
        self.buff_icons(map, out);
        if self.tick.is_multiple_of(BOSS_SCAN)
            && let Some(h) = self.horde.as_ref()
        {
            // The result screen's timeline: when each boss showed up.
            let (store, secs) = (h.store(), h.director_elapsed());
            for &s in store.alive().iter().filter(|&&s| store.flags[s as usize] & FLAG_BOSS != 0) {
                self.run.boss_seen(s, store.generation[s as usize], secs, &h.catalog().get(store.kind[s as usize]).name);
            }
        }
        if self.tick.is_multiple_of(CLOCK_SECS * self.config.tick_hz.max(1)) {
            let msg = self.stage_clock();
            for p in self.players.iter().filter(|p| p.map == map) {
                out.push((p.profile.user_id, HubEvent::Control(msg.clone())));
            }
        }
        let limit = match &self.kind {
            InstanceKind::Stage { mission, .. } => missions::find(mission).map_or(f32::INFINITY, |m| m.time_limit_secs),
            InstanceKind::Hub { .. } => f32::INFINITY,
        };
        let elapsed = self.horde.as_ref().map_or(0.0, |h| h.director_elapsed());
        if boss_down || elapsed >= limit {
            self.end_stage(map, true, out);
        }
    }

    /// A player joined the stage: what the mission spawns and drops (from
    /// the horde's own catalogue and rules) and how far the run is, for
    /// the timeline.
    pub(super) fn mission_hello(&self, user_id: u32, out: &mut Outgoing) {
        let (InstanceKind::Stage { mission, .. }, Some(horde)) = (&self.kind, self.horde.as_ref()) else { return };
        // The run's modifiers for the HUD (the table to name them first).
        out.push((user_id, HubEvent::Control(modifiers::defs_msg())));
        out.push((user_id, HubEvent::Control(horde.modifiers().state_msg(modifiers::scope::STAGE))));
        // The affix table and the affixed monsters alive (affix.rs).
        self.affix_hello(user_id, out);
        let Some(m) = missions::find(mission) else { return };
        let brief = missions::brief_with(m, horde.rules(), horde.catalog(), &horde.modifiers().monsters());
        out.push((user_id, HubEvent::Control(ServerMsg::MissionInfo { brief })));
        out.push((user_id, HubEvent::Control(self.stage_clock())));
    }

    /// The stage's mission (`None` in a hub).
    pub(super) fn mission_info(&self) -> Option<&'static missions::MissionInfo> {
        match &self.kind {
            InstanceKind::Stage { mission, .. } => missions::find(mission),
            InstanceKind::Hub { .. } => None,
        }
    }

    /// The level a won run's clear bonus is paid at (`LevelBand::clear_level`).
    fn clear_level(&self, level: u32) -> u32 {
        self.mission_info().map_or(level, |m| m.levels.clear_level(level))
    }

    fn stage_clock(&self) -> ServerMsg {
        let secs = self.horde.as_ref().map_or(0.0, |h| h.director_elapsed());
        ServerMsg::StageClock { elapsed_ms: (secs * 1000.0) as u32 }
    }

    /// Sends the result to everyone on the map and schedules the exit.
    pub(super) fn end_stage(&mut self, map: MapIdx, victory: bool, out: &mut Outgoing) {
        if self.outcome != Outcome::Running {
            return;
        }
        let hz = self.config.tick_hz as f32;
        let wait = if victory { VICTORY_SECS } else { DEFEAT_SECS };
        self.outcome = Outcome::Over { leave_at: self.tick + (wait * hz) as u32 };
        let secs = self.horde.as_ref().map_or(0.0, |h| h.director_elapsed()) as u32;
        if let Some(h) = self.horde.as_mut()
            && victory
        {
            h.halt();
        }
        if let Some(b) = self.boss.take() {
            self.send_boss(map, b.slot, String::new(), 0, 0, out);
        }
        let mods = self.modifiers();
        let chest = if victory { self.chest_tables() } else { Vec::new() };
        // The clear bonus is paid at the mission band's floor at least.
        let band = self.mission_info().map(|m| m.levels);
        let clear_level = |level: u32| band.map_or(level, |b| b.clear_level(level));
        // The modifiers' clear bonus per player (user, exp, zeny), for the
        // result screen.
        let mut mod_pay: Vec<(u32, u64, u64)> = Vec::new();
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let uid = p.profile.user_id;
            // The risk's pay, before the result screen: a won run only.
            if victory
                && !mods.is_plain()
                && let Some(f) = p.fighter.as_mut()
            {
                let (msg, events) = clear_bonus(f, mods, &chest, &mut self.dice);
                if let ServerMsg::ModifierReward { exp, zeny, .. } = &msg {
                    mod_pay.push((uid, *exp, *zeny));
                }
                out.push((uid, HubEvent::Control(msg)));
                out.extend(events.into_iter().map(|e| (uid, e)));
            }
            let exp = p.fighter.as_ref().map_or(0, |f| f.run_exp);
            let msg = ServerMsg::StageEnd { victory, secs, kills: self.kills, exp, leave_ms: (wait * 1000.0) as u32 };
            out.push((uid, HubEvent::Control(msg)));
            // The clear bonus, with what the last kills dropped.
            if let Some(f) = p.fighter.as_mut() {
                let bonus = if victory { items::victory_zeny(clear_level(f.level)) } else { 0 };
                let amount = std::mem::take(&mut f.zeny) + bonus;
                if bonus > 0 {
                    out.push((uid, HubEvent::Control(ServerMsg::Notification { message: format!("Mission bonus: {bonus} zeny") })));
                }
                if amount > 0 {
                    out.push((uid, HubEvent::Zeny { amount }));
                }
                if let Some(loot) = f.take_loot() {
                    self.run.looted(uid, &loot);
                    out.push((uid, loot));
                }
                // The record: a win counts for everyone still here.
                if victory {
                    f.tally().missions += 1;
                }
                if let Some(record) = f.take_record() {
                    out.push((uid, record));
                }
            }
        }
        self.send_results(map, victory, secs, &mod_pay, out);
    }

    /// The result screen (`stats.rs`): the party's numbers to everyone on
    /// the map (link-dead ones get it on re-attaching), with their own
    /// rewards. Players who left early are in the table, marked.
    fn send_results(&self, map: MapIdx, victory: bool, secs: u32, mod_pay: &[(u32, u64, u64)], out: &mut Outgoing) {
        let live = |uid: u32| {
            let here = self.players.iter().find(|p| p.profile.user_id == uid).and_then(|p| p.fighter.as_ref().map(|f| (f, p.link_dead)));
            let gone = || self.departed.iter().find(|(u, _)| *u == uid).map(|(_, f)| (f, false));
            here.or_else(gone).map_or((0, 0, false), |(f, link_dead)| (f.level, f.run_exp, link_dead))
        };
        let seats = self.run.seats_msg(secs, &self.skills, live);
        let stats = ServerMsg::RunStats { victory, secs, bucket_secs: self.run.bucket_secs(), seats, marks: self.run.marks().to_vec() };
        for p in self.players.iter().filter(|p| p.map == map) {
            let uid = p.profile.user_id;
            let (Some(f), Some(seat)) = (p.fighter.as_ref(), self.run.index(uid)) else { continue };
            let (mod_exp, mod_zeny) = mod_pay.iter().find(|m| m.0 == uid).map_or((0, 0), |m| (m.1, m.2));
            let rewards = ServerMsg::RunRewards {
                seat: seat as u8,
                exp: f.run_exp,
                zeny: f.run_zeny,
                clear_zeny: if victory { items::victory_zeny(self.clear_level(f.level)) } else { 0 },
                mod_exp,
                mod_zeny,
                items: self.run.seats()[seat].loot.iter().map(|&(id, n)| (id.to_string(), n)).collect(),
            };
            out.push((uid, HubEvent::Results(Box::new([stats.clone(), rewards]))));
        }
    }

    /// Drop tables the reward chest rolls on: the elites the mission
    /// spawns before its time limit (else every non-boss kind with drops).
    fn chest_tables(&self) -> Vec<Vec<loot::Drop>> {
        let (InstanceKind::Stage { mission, .. }, Some(horde)) = (&self.kind, self.horde.as_ref()) else { return Vec::new() };
        let limit = missions::find(mission).map_or(f32::INFINITY, |m| m.time_limit_secs);
        let catalog = horde.catalog();
        let pick = |elite: bool| -> Vec<Vec<loot::Drop>> {
            catalog
                .defs()
                .iter()
                .filter(|d| !d.boss && !d.drops.is_empty() && d.unlock_secs < limit && (!elite || d.archetype == Archetype::Elite))
                .map(|d| d.drops.clone())
                .collect()
        };
        let elites = pick(true);
        if elites.is_empty() { pick(false) } else { elites }
    }

    /// Everyone on the map leaves through the stage exit.
    pub(super) fn leave_stage(&mut self, map: MapIdx, out: &mut Outgoing) {
        let exit = (0..self.world.map(map).manifest.portals.len())
            .find(|&i| matches!(self.world.link(map, i), PortalLink::External { .. }));
        let Some(portal) = exit else { return };
        let leaving: Vec<usize> = (0..self.players.len()).filter(|&i| self.players[i].map == map).collect();
        for idx in leaving.into_iter().rev() {
            self.use_portal(idx, portal, out);
        }
    }

    fn send_boss(&self, map: MapIdx, slot: u16, name: String, hp: u32, max_hp: u32, out: &mut Outgoing) {
        let msg = ServerMsg::Boss { entity_id: MONSTER_ENTITY_BASE + slot as u32, name, hp, max_hp };
        for p in self.players.iter().filter(|p| p.map == map) {
            out.push((p.profile.user_id, HubEvent::Control(msg.clone())));
        }
    }

    /// The boss's bar to the map: when it shows up, as its HP changes, and
    /// 0 when it is gone.
    fn boss_bar(&mut self, map: MapIdx, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_ref() else { return };
        let store = horde.store();
        match self.boss {
            None => {
                if !self.tick.is_multiple_of(BOSS_SCAN) {
                    return;
                }
                // A scripted boss shows its own encounter bar (boss.rs).
                let plain = |s: u16| store.flags[s as usize] & FLAG_BOSS != 0 && !self.bosses.is_scripted(store.kind[s as usize]);
                let Some(&slot) = store.alive().iter().find(|&&s| plain(s)) else { return };
                self.boss = Some(BossBar { slot, generation: store.generation[slot as usize], sent_hp: i32::MIN, wait: 0 });
                self.boss_bar(map, out);
            }
            Some(mut b) => {
                let s = b.slot as usize;
                let alive = store.is_alive(b.slot) && store.generation[s] == b.generation && store.hp[s] > 0;
                if !alive {
                    self.boss = None;
                    self.send_boss(map, b.slot, String::new(), 0, 0, out);
                    return;
                }
                b.wait = b.wait.saturating_sub(1);
                let hp = store.hp[s];
                if hp != b.sent_hp && b.wait == 0 {
                    b.sent_hp = hp;
                    b.wait = BOSS_EVERY;
                    let name = horde.catalog().get(store.kind[s]).name.clone();
                    let max = horde.block(b.slot).hp.max(hp);
                    self.send_boss(map, b.slot, name, hp.max(0) as u32, max as u32, out);
                }
                self.boss = Some(b);
            }
        }
    }

    /// Each player's running buffs, when the set changes.
    fn buff_icons(&mut self, map: MapIdx, out: &mut Outgoing) {
        let now = self.tick;
        let hz = self.config.tick_hz.max(1);
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let Some(f) = p.fighter.as_mut() else { continue };
            let sig = f.buff_signature() ^ f.regen_signature();
            if sig == f.buffs_sent {
                continue;
            }
            f.buffs_sent = sig;
            let mut buffs: Vec<(String, u32)> = f
                .buff_list(now)
                .filter(|&(skill, _)| (skill as usize) < self.skills.len() || super::bonus::buff_id(skill).is_some())
                .map(|(skill, left)| {
                    // Toggles run until cast again: 0 = no timer.
                    let ms = if left > u32::MAX / 4 { 0 } else { left.saturating_mul(1000) / hz };
                    // A pickup bonus's own buff: `bonus:<id>`.
                    let id = match super::bonus::buff_id(skill) {
                        Some(b) => format!("bonus:{b}"),
                        None => self.skills.get(skill).id.clone(),
                    };
                    (id, ms)
                })
                .collect();
            // Potion regenerations: `item:<id>`.
            buffs.extend(f.regen_list().map(|(id, left)| (id, left.saturating_mul(1000) / hz)));
            out.push((p.profile.user_id, HubEvent::Control(ServerMsg::Buffs { buffs })));
        }
    }
}

/// A won run's clear bonus for its modifiers (COMBAT.md "Mission modifiers
/// and rewards"): shares of the run's zeny (clear and kills) and
/// experience, and the chest's rolls on elite drop tables. The shares are
/// set by the risk score alone; what they apply to is the run's take,
/// which more monsters do not raise (their kills are worth less).
fn clear_bonus(
    f: &mut super::fight::Fighter,
    mods: Modifiers,
    chest: &[Vec<loot::Drop>],
    rng: &mut crate::world::combat::Rng,
) -> (ServerMsg, Vec<HubEvent>) {
    let r = mods.rewards();
    let zeny = (items::victory_zeny(f.level) + f.run_zeny) * r.zeny_pct as u64 / 100;
    let exp = f.run_exp * r.exp_pct as u64 / 100;
    f.zeny += zeny;
    let mut items: Vec<(String, u32)> = Vec::new();
    if !chest.is_empty() {
        for _ in 0..r.chest {
            let table = &chest[(rng.die(chest.len() as u32) - 1) as usize];
            loot::roll(table, rng, |d, n| {
                if d.is_zeny() {
                    f.zeny += n as u64;
                } else {
                    f.add_loot(d.item, n);
                }
                match items.iter_mut().find(|(id, _)| id == d.item) {
                    Some(e) => e.1 += n,
                    None => items.push((d.item.into(), n)),
                }
            });
        }
    }
    let events = if exp > 0 { f.grant_exp(exp).to_vec() } else { Vec::new() };
    let msg = ServerMsg::ModifierReward { tier: mods.tier, mask: mods.flags.bits(), risk: mods.risk(), zeny, exp, items };
    (msg, events)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::super::tests::profile;
    use super::super::{HubEvent, HubInstance, HubSimConfig, HubWorld, PlayerProfile};
    use crate::world::combat::{self, items, loot::Drop, Archetype, Bag, Class, MonsterBlock};
    use crate::world::horde::catalog::{test_def, MonsterCatalog, MonsterDef};
    use crate::world::horde::{Horde, HordeConfig};
    use crate::world::map::{CameraConstraint, LocationKind};
    use crate::world::modifiers::{ModFlags, Modifiers};
    use crate::world::protocol::ServerMsg;
    use std::sync::Arc;
    use uuid::Uuid;

    fn world() -> Arc<HubWorld> {
        let row = format!("#{}#", ".".repeat(38));
        let mut rows = vec!["#".repeat(40)];
        rows.extend((0..8).map(|_| row.clone()));
        rows.push("#".repeat(40));
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let field = map(
            "field",
            LocationKind::Field,
            CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
            &rows,
            vec![portal("to_town", [1, 2, 1], "town", [3, 3])],
        );
        Arc::new(HubWorld::new(vec![field], "field").unwrap())
    }

    fn catalog() -> Arc<MonsterCatalog> {
        let blob = MonsterDef { drops: vec![Drop::new("jellopy", 100.0, None).unwrap()], ..test_def("blob", 2.0, 1, 0.0, false) };
        Arc::new(MonsterCatalog::new(vec![blob]).unwrap())
    }

    /// A stage under `mods` with a swordsman (user 1) in it.
    fn stage(mods: Modifiers) -> HubInstance {
        let world = world();
        let cfg = HordeConfig { light_radius: 6.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() }.with_modifiers(mods);
        let horde = Horde::new(Arc::clone(&world), catalog(), cfg);
        let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_modifiers(mods).with_horde(horde);
        stage.dice = combat::Rng::new(3);
        let mut out = Vec::new();
        stage.join(PlayerProfile { archetype: "swordsman".into(), level: 12, ..profile(1) }, Uuid::nil(), None, 0, &mut out);
        assert!(
            out.iter().any(|(_, e)| matches!(e, HubEvent::Control(ServerMsg::Modifiers { scope: 1, tier, mask, .. }) if *tier == mods.tier && *mask == mods.flags.bits())),
            "the HUD hears the run's modifiers"
        );
        stage
    }

    fn fighter(stage: &mut HubInstance) -> &mut super::super::fight::Fighter {
        let i = stage.find(1).unwrap();
        stage.players[i].fighter.as_mut().unwrap()
    }

    fn curse(flags: ModFlags) -> Modifiers {
        Modifiers { tier: 0, flags }
    }

    #[test]
    fn tiers_make_monsters_tougher_and_frenzy_faster() {
        let mut plain = stage(Modifiers::NONE);
        let mut hard = stage(Modifiers { tier: 6, flags: ModFlags::FRENZY });
        let mut moved = [0.0f32; 2];
        for (i, st) in [&mut plain, &mut hard].into_iter().enumerate() {
            let h = st.horde.as_mut().unwrap();
            let slot = h.spawn(0, 30.5, 4.5).unwrap();
            let block = h.block(slot);
            let base = MonsterBlock::new(block.level, Archetype::Swarm, 1);
            if i == 0 {
                assert_eq!(block, base);
            } else {
                assert_eq!(block.hp, (base.hp as f32 * 1.9).round() as i32, "+15% HP per tier");
                assert_eq!((block.ac, block.save), (base.ac + 3, base.save + 2));
                assert_eq!(h.store().hp[slot as usize], block.hp, "spawns at the tougher HP");
            }
            for t in 0..10 {
                h.tick(t, 20, &[(2.5, 4.5)]);
            }
            moved[i] = 30.5 - h.monsters().next().unwrap().3;
        }
        let ratio = moved[1] / moved[0];
        assert!((ratio - 1.18 * 1.25).abs() < 0.05, "speed ×{ratio}");
    }

    #[test]
    fn glass_bones_downs_on_any_blow_to_hp() {
        let mut st = stage(curse(ModFlags::ONE_HIT));
        let f = fighter(&mut st);
        assert!(f.curses.contains(ModFlags::ONE_HIT), "set on arrival");
        f.temp_hp = 5;
        f.take(3, true);
        assert_eq!(f.hp, f.max_hp, "temporary HP still soaks");
        f.take(10, true);
        assert!(f.hp <= 0, "the blow that reached HP took it all");
        let mut plain = stage(Modifiers::NONE);
        let f = fighter(&mut plain);
        f.take(1, true);
        assert!(f.hp > 0 && f.hp < f.max_hp);
    }

    #[test]
    fn abstinence_keeps_the_potions_in_the_bag() {
        let mut st = stage(curse(ModFlags::NO_POTIONS));
        let bag = Bag::starting(Class::Swordsman, "");
        fighter(&mut st).wear(&bag);
        fighter(&mut st).hp = 1;
        let before = fighter(&mut st).potions.clone();
        assert!(!before.is_empty());
        let mut out = Vec::new();
        let potion = before[0].0.clone();
        st.use_item(1, &potion, &mut out);
        assert!(out.iter().any(|(_, e)| matches!(e, HubEvent::Control(ServerMsg::HubError { code, .. }) if code == "no_potions")), "{out:?}");
        assert_eq!(fighter(&mut st).potions, before);
        assert!(fighter(&mut st).regen[0].is_none());
    }

    #[test]
    fn leaden_feet_and_darkness() {
        let mut moved = [0.0f32; 2];
        for (i, mods) in [Modifiers::NONE, curse(ModFlags(ModFlags::SLOW.0 | ModFlags::DARKNESS.0))].into_iter().enumerate() {
            let mut st = stage(mods);
            let x0 = st.position_of(1).unwrap().1;
            st.input(1, 1, 1.0, 0.0);
            let mut out = Vec::new();
            st.tick(&mut out);
            moved[i] = st.position_of(1).unwrap().1 - x0;
            let light = st.horde.as_ref().unwrap().light_radius();
            assert_eq!(light, if i == 0 { 6.0 } else { 4.5 });
        }
        assert!((moved[1] / moved[0] - 0.8).abs() < 1e-3, "{moved:?}");
    }

    fn reward(out: &[(u32, HubEvent)]) -> Option<ServerMsg> {
        out.iter().find_map(|(_, e)| match e {
            HubEvent::Control(m @ ServerMsg::ModifierReward { .. }) => Some(m.clone()),
            _ => None,
        })
    }

    #[test]
    fn a_won_run_pays_its_risk_and_a_lost_one_nothing() {
        let mods = Modifiers { tier: 5, flags: ModFlags::NONE };
        let mut st = stage(mods);
        fighter(&mut st).run_exp = 1000;
        fighter(&mut st).run_zeny = 500;
        let level = fighter(&mut st).level;
        let mut out = Vec::new();
        let map = st.players[0].map;
        st.end_stage(map, true, &mut out);
        let Some(ServerMsg::ModifierReward { risk, zeny, exp, items: chest, .. }) = reward(&out) else { panic!("{out:?}") };
        assert_eq!(risk, 20);
        assert_eq!((zeny, exp), ((items::victory_zeny(level) + 500) * 20 / 100, 100), "+R% of the run's zeny, +R/2% of its exp");
        assert_eq!(chest, vec![("jellopy".to_string(), 1)], "one chest roll per 20 risk");
        let at = |pred: fn(&HubEvent) -> bool| out.iter().position(|(_, e)| pred(e)).unwrap();
        assert!(at(|e| matches!(e, HubEvent::Control(ServerMsg::ModifierReward { .. }))) < at(|e| matches!(e, HubEvent::Control(ServerMsg::StageEnd { .. }))));
        assert!(out.iter().any(|(_, e)| matches!(e, HubEvent::Zeny { amount } if *amount >= zeny)));
        assert!(out.iter().any(|(_, e)| matches!(e, HubEvent::Progress { .. })));

        let mut lost = stage(mods);
        let mut out = Vec::new();
        lost.end_stage(map, false, &mut out);
        assert!(reward(&out).is_none(), "defeat: no bonus");
        let mut plain = stage(Modifiers::NONE);
        let mut out = Vec::new();
        plain.end_stage(map, true, &mut out);
        assert!(reward(&out).is_none(), "no modifiers: nothing extra");
    }

    fn results(out: &[(u32, HubEvent)], uid: u32) -> Option<(ServerMsg, ServerMsg)> {
        out.iter().find_map(|(u, e)| match e {
            HubEvent::Results(r) if *u == uid => Some((r[0].clone(), r[1].clone())),
            _ => None,
        })
    }

    #[test]
    fn the_result_screen_counts_the_run_for_everyone_who_stood_on_it() {
        use crate::world::protocol::{seat_flags, RunSeat};
        let mut st = stage(Modifiers::NONE);
        let mut out = Vec::new();
        // A blob in reach of the swordsman: the basic attack kills it.
        let (_, x, y) = st.position_of(1).unwrap();
        st.horde.as_mut().unwrap().spawn(0, x + 0.8, y).unwrap();
        for _ in 0..200 {
            st.tick(&mut out);
            if st.kills > 0 {
                break;
            }
        }
        assert_eq!(st.kills, 1, "the blob fell");
        st.join(PlayerProfile { archetype: "mage".into(), level: 12, ..profile(2) }, Uuid::nil(), None, 0, &mut out);
        st.join(PlayerProfile { archetype: "archer".into(), level: 12, ..profile(3) }, Uuid::nil(), None, 0, &mut out);
        // Player 3 leaves early, player 2 drops (link-dead).
        assert!(st.leave(3, Uuid::nil(), &mut out));
        assert_eq!(st.detach(2, Uuid::nil(), &mut out), Some(true));
        let map = st.players[0].map;
        let mut out = Vec::new();
        st.end_stage(map, true, &mut out);
        let (ServerMsg::RunStats { victory, seats, bucket_secs, .. }, ServerMsg::RunRewards { seat, items: loot, clear_zeny, .. }) =
            results(&out, 1).expect("the swordsman's result screen")
        else {
            panic!()
        };
        assert!(victory);
        assert_eq!(bucket_secs, 10);
        assert_eq!(seats.len(), 3);
        let me: &RunSeat = &seats[seat as usize];
        assert_eq!((me.name.as_str(), me.kills, me.flags), (profile(1).name.as_str(), 1, 0));
        assert!(me.dealt > 0 && me.skills[0].0 == "attack", "{me:?}");
        assert_eq!(me.buckets.len(), 1);
        assert_eq!(me.buckets[0].kills, 1);
        assert!(me.exp > 0);
        assert_eq!(loot, vec![("jellopy".to_string(), 1)], "the drop is in the loot list");
        assert!(clear_zeny > 0);
        assert_eq!(seats[1].flags, seat_flags::LINK_DEAD);
        assert_eq!(seats[2].flags, seat_flags::LEFT);
        // The link-dead player's screen waits for them; the leaver gets none.
        assert!(results(&out, 2).is_some());
        assert!(results(&out, 3).is_none());
        // The frame stays small: three seats, one bucket each.
        assert!(results(&out, 1).unwrap().0.encode().len() < 200);
    }

    #[test]
    fn the_dev_hook_ends_a_mission_only_under_game_dev() {
        let ended = |dev: bool| {
            let mut st = stage(Modifiers::NONE);
            st.config.missions_anywhere = dev;
            let mut out = Vec::new();
            st.chat(1, "/end lose".into(), &mut out);
            out.iter().any(|(_, e)| matches!(e, HubEvent::Control(ServerMsg::StageEnd { victory: false, .. })))
                && out.iter().any(|(_, e)| matches!(e, HubEvent::Results(_)))
        };
        assert!(ended(true));
        assert!(!ended(false), "a plain chat line otherwise");
    }

    #[test]
    fn swarm_kills_are_worth_less() {
        let st = stage(curse(ModFlags::SWARM));
        let h = st.horde.as_ref().unwrap();
        assert_eq!(h.loot_permille(Archetype::Swarm), 784, "1.5^-0.6: a swarm pays ~+18 %, not +50 %");
        assert_eq!(h.rules().budget, crate::world::horde::director::WaveRules::NORMAL.budget * 1.5);
    }
}
