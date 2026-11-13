//! Stage combat: basic attacks, monster melee, kills and experience,
//! downed / revive / defeat (COMBAT.md §4). Rules come from
//! [`crate::world::combat`]; this file only applies them to the instance's
//! players and horde once per tick.

use super::cast::Casting;
use super::{facing_of, HubEvent, HubInstance, Outgoing};
use crate::world::combat::skills::SLOTS;
use crate::world::combat::rules::{attack_roll, AttackOutcome};
use crate::world::combat::{self, exp_to_next, kill_value, share_kill, Class, Damage, DamageType, Reach, Weapon};
use crate::world::horde::storage::FLAG_FULL;
use crate::world::hub::PortalLink;
use crate::world::protocol::{hit, HitEvent, ServerMsg, Swing, MONSTER_ENTITY_BASE};

/// Standing still this many ticks (0.15 s at 20 Hz) starts the basic attack:
/// releasing the stick between steps must not trigger a swing.
const STILL_TICKS: u32 = 3;
/// Monsters reach this far beyond touching (cells).
const MONSTER_REACH: f32 = 0.35;
/// Player body radius as the horde uses it (cells).
const PLAYER_RADIUS: f32 = 0.4;
/// An ally standing this close revives a downed player (cells; 1.5 m).
const REVIVE_RANGE: f32 = 3.0;
const REVIVE_SECS: f32 = 3.0;
const DOWNED_SECS: f32 = 20.0;
/// No damage dealt or taken for this long: HP regenerates.
const QUIET_SECS: f32 = 5.0;
/// Progress messages at most this often (ticks), kills are summed.
const PROGRESS_EVERY: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vital {
    Up,
    /// Out of the fight; an ally can revive before `ticks` run out.
    Downed { ticks: u32 },
    /// Back at the next wave.
    Dead,
}

impl Vital {
    fn code(self) -> u8 {
        match self {
            Vital::Up => 0,
            Vital::Downed { .. } => 1,
            Vital::Dead => 2,
        }
    }
}

/// A player's combat state inside a stage.
#[derive(Clone, Debug)]
pub struct Fighter {
    pub class: Class,
    /// What the basic attack swings (die, reach, speed).
    pub weapon: &'static Weapon,
    pub level: u32,
    /// Experience into the current level.
    pub exp: u64,
    pub hp: i32,
    pub max_hp: i32,
    pub sp: i32,
    pub max_sp: i32,
    pub ac: i32,
    pub state: Vital,
    /// Learned level per skill of the instance's book (0 = not learned).
    pub skills: Vec<u8>,
    /// Skill (book index) per hotbar slot.
    pub hotbar: [Option<u16>; SLOTS],
    /// Ticks left per skill.
    pub(super) cooldowns: Vec<u32>,
    pub(super) casting: Option<Casting>,
    /// Rooted after a cast; the basic attack waits.
    pub(super) recovery: u32,
    /// Passive SP regeneration, fraction of max SP per second.
    pub(super) sp_regen: f32,
    /// Level changed: skill points to resend.
    pub(super) skills_dirty: bool,
    attack_cd: u32,
    still: u32,
    pub(super) quiet: u32,
    revive: u32,
    regen_hp: f32,
    regen_sp: f32,
    /// Last vitals sent (hp, max_hp, sp, max_sp, state).
    sent: Option<(i32, i32, i32, i32, u8)>,
    gained: u64,
    progress_dirty: bool,
    progress_wait: u32,
}

impl Fighter {
    pub fn new(class: Class, level: u32, exp: u64, weapon: &str) -> Self {
        let level = level.clamp(1, combat::MAX_LEVEL);
        let (max_hp, max_sp) = (class.max_hp(level), class.max_sp(level));
        Self {
            class,
            weapon: Weapon::held(class, weapon),
            level,
            exp,
            hp: max_hp,
            max_hp,
            sp: max_sp,
            max_sp,
            ac: class.base_ac(level),
            state: Vital::Up,
            skills: Vec::new(),
            hotbar: [None; SLOTS],
            cooldowns: Vec::new(),
            casting: None,
            recovery: 0,
            sp_regen: 0.0,
            skills_dirty: false,
            attack_cd: 0,
            still: 0,
            quiet: 0,
            revive: 0,
            regen_hp: 0.0,
            regen_sp: 0.0,
            sent: None,
            gained: 0,
            progress_dirty: false,
            progress_wait: 0,
        }
    }

    pub fn is_up(&self) -> bool {
        self.state == Vital::Up
    }

    fn vitals(&self, entity_id: u32) -> ServerMsg {
        ServerMsg::PlayerVitals {
            entity_id,
            hp: self.hp.max(0) as u32,
            max_hp: self.max_hp as u32,
            sp: self.sp.max(0) as u32,
            max_sp: self.max_sp as u32,
            state: self.state.code(),
        }
    }

    fn progress(&self) -> ServerMsg {
        ServerMsg::Progress { level: self.level, exp: self.exp, exp_to_next: exp_to_next(self.level), gained: self.gained }
    }

    /// Adds experience; levels up as many times as it pays for (full heal
    /// on a level-up, as in RO).
    fn gain(&mut self, exp: u64) {
        if exp == 0 {
            return;
        }
        self.exp += exp;
        self.gained += exp;
        self.progress_dirty = true;
        while self.level < combat::MAX_LEVEL && self.exp >= exp_to_next(self.level) {
            self.exp -= exp_to_next(self.level);
            self.level += 1;
            self.max_hp = self.class.max_hp(self.level);
            self.max_sp = self.class.max_sp(self.level);
            self.ac = self.class.base_ac(self.level);
            if self.is_up() {
                self.hp = self.max_hp;
                self.sp = self.max_sp;
            }
            self.progress_wait = 0;
            self.skills_dirty = true;
        }
        if self.level >= combat::MAX_LEVEL {
            self.exp = 0;
        }
    }
}

fn damage_type_bits(kind: DamageType) -> u8 {
    (kind as u8) << 4
}

impl HubInstance {
    /// One combat step on the horde's map, after movement and the horde.
    pub(super) fn fight(&mut self, out: &mut Outgoing) {
        if self.horde.is_none() {
            return;
        }
        let mut swings: Vec<Swing> = Vec::new();
        let mut hits: Vec<HitEvent> = Vec::new();
        let mut kills: Vec<u16> = Vec::new();
        // 0. Skills: presses, wind-ups and due hits.
        self.cast_step(out, &mut hits, &mut kills);
        let Some(horde) = self.horde.as_mut() else { return };
        let map = horde.map();
        let hz = self.config.tick_hz as f32;
        let rng = &mut self.dice;

        // 1. Players: the basic attack while standing.
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let Some(f) = p.fighter.as_mut() else { continue };
            f.attack_cd = f.attack_cd.saturating_sub(1);
            if !f.is_up() || p.dir != (0.0, 0.0) {
                f.still = 0;
                continue;
            }
            if f.casting.is_some() || f.recovery > 0 {
                continue;
            }
            f.still += 1;
            if f.still < STILL_TICKS || f.attack_cd > 0 {
                continue;
            }
            let weapon = f.weapon;
            let (dice, kind) = weapon.damage(f.class, f.level);
            let damage = Damage::new(dice, kind);
            let (range, arc_cos, splash) = match weapon.reach {
                Reach::Melee { range, arc_cos } => (range, Some(arc_cos), 0.0),
                Reach::Ranged { range, splash } => (range, None, splash),
            };
            let store = horde.store();
            let catalog = horde.catalog();
            let mut best: Option<(u16, f32)> = None;
            for &s in store.alive() {
                let i = s as usize;
                if store.hp[i] <= 0 {
                    continue;
                }
                let r = catalog.get(store.kind[i]).radius;
                let (dx, dy) = (store.x[i] - p.x, store.y[i] - p.y);
                let reach = range + r;
                if dx.abs() > reach || dy.abs() > reach {
                    continue;
                }
                let d = (dx * dx + dy * dy).sqrt() - r;
                if d <= range && best.is_none_or(|(_, b)| d < b) {
                    best = Some((s, d));
                }
            }
            let Some((target, _)) = best else { continue };
            let (tx, ty) = (store.x[target as usize], store.y[target as usize]);
            let (ax, ay) = (tx - p.x, ty - p.y);
            let alen = (ax * ax + ay * ay).sqrt().max(1e-3);
            let mut targets: Vec<u16> = Vec::new();
            for &s in store.alive() {
                let i = s as usize;
                if store.hp[i] <= 0 {
                    continue;
                }
                let r = catalog.get(store.kind[i]).radius;
                let inside = match arc_cos {
                    Some(cos) => {
                        let (dx, dy) = (store.x[i] - p.x, store.y[i] - p.y);
                        let d = (dx * dx + dy * dy).sqrt();
                        s == target || (d - r <= range && (dx * ax + dy * ay) >= cos * d * alen)
                    }
                    None => {
                        let (dx, dy) = (store.x[i] - tx, store.y[i] - ty);
                        let reach = splash + r;
                        dx * dx + dy * dy <= reach * reach
                    }
                };
                if inside {
                    targets.push(s);
                }
            }
            let bonus = weapon.to_hit(f.level);
            let attacks = f.class.attacks_per_action(f.level);
            let entity = p.profile.character_id;
            for s in targets {
                let block = horde.block(s);
                let element = horde.catalog().get(horde.store().kind[s as usize]).element;
                for _ in 0..attacks {
                    let id = MONSTER_ENTITY_BASE + s as u32;
                    match attack_roll(rng, bonus, block.ac, 0, 20) {
                        AttackOutcome::Miss => hits.push(HitEvent { target: id, amount: 0, flags: hit::MISS }),
                        outcome => {
                            let crit = outcome == AttackOutcome::Critical;
                            let amount = damage.resolve(rng, crit, false, element, 0);
                            let hp = &mut horde.store_mut().hp[s as usize];
                            *hp -= amount;
                            let mut flags = damage_type_bits(damage.kind);
                            if crit {
                                flags |= hit::CRITICAL;
                            }
                            if *hp <= 0 {
                                flags |= hit::KILL;
                                kills.push(s);
                            }
                            hits.push(HitEvent { target: id, amount: amount as u32, flags });
                            if *hp <= 0 {
                                break;
                            }
                        }
                    }
                }
            }
            f.attack_cd = (weapon.interval_ms as f32 * hz / 1000.0).round() as u32;
            f.quiet = 0;
            p.facing = facing_of(ax, ay);
            swings.push(Swing { entity_id: entity, facing: p.facing });
        }

        // 2. Kills: experience for everyone in the party on this map, then
        //    the slot goes back to the pool (the client sees a REMOVE).
        if !kills.is_empty() {
            let levels: Vec<u32> =
                self.players.iter().filter(|p| p.map == map).filter_map(|p| p.fighter.as_ref().map(|f| f.level)).collect();
            for &s in &kills {
                let store = horde.store();
                if !store.is_alive(s) {
                    continue;
                }
                let def = horde.catalog().get(store.kind[s as usize]);
                let level = store.level[s as usize] as u32;
                let shares = share_kill(kill_value(level, def.archetype), level, &levels);
                let mut it = shares.into_iter();
                for p in self.players.iter_mut().filter(|p| p.map == map) {
                    if let Some(f) = p.fighter.as_mut() {
                        f.gain(it.next().unwrap_or(0));
                    }
                }
                horde.despawn(s);
            }
        }

        // 3. Monsters in contact strike the nearest standing player.
        let ups: Vec<(usize, f32, f32)> = self
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.map == map && p.fighter.as_ref().is_some_and(Fighter::is_up))
            .map(|(i, p)| (i, p.x, p.y))
            .collect();
        let alive = horde.store().alive().len();
        for k in 0..alive {
            let s = horde.store().alive()[k];
            let i = s as usize;
            let store = horde.store_mut();
            if store.cooldown[i] > 0 {
                store.cooldown[i] -= 1;
                continue;
            }
            if store.flags[i] & FLAG_FULL == 0 || ups.is_empty() {
                continue;
            }
            let (mx, my) = (store.x[i], store.y[i]);
            let reach = horde.catalog().get(horde.store().kind[i]).radius + PLAYER_RADIUS + MONSTER_REACH;
            let mut victim: Option<(usize, f32, f32, f32)> = None;
            for &(pi, px, py) in &ups {
                let (dx, dy) = (px - mx, py - my);
                let d2 = dx * dx + dy * dy;
                if d2 <= reach * reach && victim.is_none_or(|(_, _, _, b)| d2 < b) {
                    victim = Some((pi, dx, dy, d2));
                }
            }
            let Some((pi, dx, dy, _)) = victim else { continue };
            let block = horde.block(s);
            let p = &mut self.players[pi];
            let Some(f) = p.fighter.as_mut() else { continue };
            if !f.is_up() {
                continue;
            }
            let id = p.profile.character_id;
            match attack_roll(rng, block.attack, f.ac, 0, 20) {
                AttackOutcome::Miss => hits.push(HitEvent { target: id, amount: 0, flags: hit::MISS }),
                outcome => {
                    let crit = outcome == AttackOutcome::Critical;
                    let amount = block.damage.roll(rng, crit);
                    f.hp -= amount;
                    let flags = damage_type_bits(DamageType::Physical) | if crit { hit::CRITICAL } else { 0 };
                    hits.push(HitEvent { target: id, amount: amount as u32, flags });
                    if f.hp <= 0 {
                        f.hp = 0;
                        f.state = Vital::Downed { ticks: (DOWNED_SECS * hz) as u32 };
                        f.revive = 0;
                    }
                }
            }
            f.quiet = 0;
            horde.store_mut().cooldown[i] = (block.attack_interval_ms as f32 * hz / 1000.0).round() as u16;
            swings.push(Swing { entity_id: MONSTER_ENTITY_BASE + s as u32, facing: facing_of(dx, dy) });
        }

        // 4. Downed, revive, regeneration.
        let wave = horde.waves();
        let new_wave = wave != self.last_wave;
        self.last_wave = wave;
        let positions: Vec<(u32, f32, f32, bool)> = self
            .players
            .iter()
            .filter(|p| p.map == map)
            .map(|p| (p.profile.user_id, p.x, p.y, p.fighter.as_ref().is_some_and(Fighter::is_up)))
            .collect();
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let (uid, x, y) = (p.profile.user_id, p.x, p.y);
            let Some(f) = p.fighter.as_mut() else { continue };
            match f.state {
                Vital::Up => {
                    f.quiet = f.quiet.saturating_add(1);
                    if f.quiet as f32 >= QUIET_SECS * hz && f.hp < f.max_hp {
                        f.regen_hp += f.max_hp as f32 * 0.005 / hz;
                    }
                    let sp_rate = if p.dir == (0.0, 0.0) { 0.02 } else { 0.005 } + f.sp_regen;
                    if f.sp < f.max_sp {
                        f.regen_sp += f.max_sp as f32 * sp_rate / hz;
                    }
                    let (h, s) = (f.regen_hp.floor(), f.regen_sp.floor());
                    f.hp = (f.hp + h as i32).min(f.max_hp);
                    f.sp = (f.sp + s as i32).min(f.max_sp);
                    f.regen_hp -= h;
                    f.regen_sp -= s;
                }
                Vital::Downed { ticks } => {
                    let helped = positions.iter().any(|&(other, ox, oy, up)| {
                        up && other != uid && (ox - x).powi(2) + (oy - y).powi(2) <= REVIVE_RANGE * REVIVE_RANGE
                    });
                    f.revive = if helped { f.revive + 1 } else { 0 };
                    if f.revive as f32 >= REVIVE_SECS * hz {
                        f.state = Vital::Up;
                        f.hp = (f.max_hp / 4).max(1);
                        f.revive = 0;
                    } else if ticks <= 1 {
                        f.state = Vital::Dead;
                    } else {
                        f.state = Vital::Downed { ticks: ticks - 1 };
                    }
                }
                Vital::Dead => {
                    if new_wave {
                        f.state = Vital::Up;
                        f.hp = (f.max_hp / 2).max(1);
                    }
                }
            }
        }

        // 5. Messages: combat to the map, vitals on change, progress to self.
        let recipients: Vec<u32> = self.players.iter().filter(|p| p.map == map).map(|p| p.profile.user_id).collect();
        let mut skill_updates: Vec<u32> = Vec::new();
        if !swings.is_empty() || !hits.is_empty() {
            let msg = ServerMsg::Combat { swings, hits };
            for &uid in &recipients {
                out.push((uid, HubEvent::Control(msg.clone())));
            }
        }
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let id = p.profile.character_id;
            let uid = p.profile.user_id;
            let Some(f) = p.fighter.as_mut() else { continue };
            let now = (f.hp.max(0), f.max_hp, f.sp, f.max_sp, f.state.code());
            if f.sent != Some(now) {
                f.sent = Some(now);
                let msg = f.vitals(id);
                for &r in &recipients {
                    out.push((r, HubEvent::Control(msg.clone())));
                }
            }
            if std::mem::take(&mut f.skills_dirty) {
                skill_updates.push(uid);
            }
            f.progress_wait = f.progress_wait.saturating_sub(1);
            if f.progress_dirty && f.progress_wait == 0 {
                out.push((uid, HubEvent::Control(f.progress())));
                out.push((uid, HubEvent::Progress { level: f.level, exp: f.exp }));
                f.gained = 0;
                f.progress_dirty = false;
                f.progress_wait = PROGRESS_EVERY;
            }
        }

        for uid in skill_updates {
            if let Some(i) = self.find(uid) {
                out.push((uid, HubEvent::Control(self.skills_msg(i))));
            }
        }

        // 6. Defeat: nobody on the map is standing.
        let fighters = self.players.iter().filter(|p| p.map == map && p.fighter.is_some()).count();
        let standing = self.players.iter().filter(|p| p.map == map && p.fighter.as_ref().is_some_and(Fighter::is_up)).count();
        if fighters > 0 && standing == 0 {
            self.defeat(map, out);
        }
    }

    /// The whole party is down: each member loses 1% of their level's
    /// requirement (never a level) and everyone leaves through the stage exit.
    fn defeat(&mut self, map: super::MapIdx, out: &mut Outgoing) {
        let exit = (0..self.world.map(map).manifest.portals.len())
            .find(|&i| matches!(self.world.link(map, i), PortalLink::External { .. }));
        let mut leaving: Vec<usize> = Vec::new();
        for (idx, p) in self.players.iter_mut().enumerate().filter(|(_, p)| p.map == map) {
            let uid = p.profile.user_id;
            if let Some(f) = p.fighter.as_mut() {
                let penalty = exp_to_next(f.level) / 100;
                f.exp = f.exp.saturating_sub(penalty);
                f.gained = 0;
                out.push((uid, HubEvent::Control(f.progress())));
                out.push((uid, HubEvent::Progress { level: f.level, exp: f.exp }));
            }
            out.push((uid, HubEvent::Control(ServerMsg::Notification { message: "Your party has fallen.".into() })));
            leaving.push(idx);
        }
        if let Some(portal) = exit {
            for idx in leaving.into_iter().rev() {
                self.use_portal(idx, portal, out);
            }
        }
    }

    /// Vitals of everyone on the newcomer's map to them, theirs to everyone,
    /// and their progress to them.
    pub(super) fn announce_fighter(&self, idx: usize, out: &mut Outgoing) {
        let p = &self.players[idx];
        let Some(f) = p.fighter.as_ref() else { return };
        let mine = f.vitals(p.profile.character_id);
        for o in self.players.iter().filter(|o| o.map == p.map) {
            out.push((o.profile.user_id, HubEvent::Control(mine.clone())));
            if o.profile.user_id != p.profile.user_id
                && let Some(of) = o.fighter.as_ref()
            {
                out.push((p.profile.user_id, HubEvent::Control(of.vitals(o.profile.character_id))));
            }
        }
        out.push((p.profile.user_id, HubEvent::Control(f.progress())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_levels_up_and_heals() {
        let mut f = Fighter::new(Class::Mage, 1, 0, "");
        f.hp = 1;
        f.gain(exp_to_next(1) + exp_to_next(2) + 7);
        assert_eq!((f.level, f.exp), (3, 7));
        assert_eq!((f.hp, f.max_hp), (Class::Mage.max_hp(3), Class::Mage.max_hp(3)));
        assert!(f.progress_dirty);
    }

    #[test]
    fn fighters_hold_a_weapon_their_class_allows() {
        assert_eq!(Fighter::new(Class::Swordsman, 1, 0, "katana").weapon.id, "katana");
        assert_eq!(Fighter::new(Class::Archer, 1, 0, "katana").weapon.id, "bow");
    }
}
