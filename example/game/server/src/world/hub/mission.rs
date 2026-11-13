//! The end of a mission (COMBAT.md §4.6) and the HUD extras around it: the
//! boss's bar, each player's buff icons, the result screen.
//!
//! Victory: the boss dies, or the party lasts the mission's time limit.
//! Defeat: nobody on the map is standing. Either way everyone gets
//! `STAGE_END` (time, party kills, their experience) and leaves through the
//! stage exit a few seconds later.

use super::{HubEvent, HubInstance, InstanceKind, MapIdx, Outgoing, PortalLink};
use crate::world::horde::storage::FLAG_BOSS;
use crate::world::missions;
use crate::world::protocol::{ServerMsg, MONSTER_ENTITY_BASE};

/// Result screen time before everyone leaves (seconds).
const VICTORY_SECS: f32 = 12.0;
const DEFEAT_SECS: f32 = 5.0;
/// The boss bar is resent at most this often (ticks).
const BOSS_EVERY: u32 = 4;
/// Without a boss, look for one this often (ticks).
const BOSS_SCAN: u32 = 20;

/// The boss the party is fighting.
#[derive(Clone, Copy, Debug)]
pub(super) struct BossBar {
    slot: u16,
    generation: u8,
    sent_hp: i32,
    wait: u32,
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
        self.buff_icons(map, out);
        let limit = match &self.kind {
            InstanceKind::Stage { mission, .. } => missions::find(mission).map_or(f32::INFINITY, |m| m.time_limit_secs),
            InstanceKind::Hub { .. } => f32::INFINITY,
        };
        let elapsed = self.horde.as_ref().map_or(0.0, |h| h.director_elapsed());
        if boss_down || elapsed >= limit {
            self.end_stage(map, true, out);
        }
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
        for p in self.players.iter().filter(|p| p.map == map) {
            let exp = p.fighter.as_ref().map_or(0, |f| f.run_exp);
            let msg = ServerMsg::StageEnd { victory, secs, kills: self.kills, exp, leave_ms: (wait * 1000.0) as u32 };
            out.push((p.profile.user_id, HubEvent::Control(msg)));
        }
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
                let Some(&slot) = store.alive().iter().find(|&&s| store.flags[s as usize] & FLAG_BOSS != 0) else { return };
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
            let sig = f.buff_signature();
            if sig == f.buffs_sent {
                continue;
            }
            f.buffs_sent = sig;
            let buffs = f
                .buff_list(now)
                .filter(|&(skill, _)| (skill as usize) < self.skills.len())
                .map(|(skill, left)| {
                    // Toggles run until cast again: 0 = no timer.
                    let ms = if left > u32::MAX / 4 { 0 } else { left.saturating_mul(1000) / hz };
                    (self.skills.get(skill).id.clone(), ms)
                })
                .collect();
            out.push((p.profile.user_id, HubEvent::Control(ServerMsg::Buffs { buffs })));
        }
    }
}
