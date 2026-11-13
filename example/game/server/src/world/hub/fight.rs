//! Stage combat: basic attacks, monster melee, kills and experience,
//! downed / revive / defeat (COMBAT.md §4). Rules come from
//! [`crate::world::combat`]; this file only applies them to the instance's
//! players and horde once per tick.

use super::area::Area;
use super::cast::{wound, Casting};
use super::{facing_of, HubEvent, HubInstance, Outgoing};
use crate::world::combat::skills::SLOTS;
use crate::world::combat::rules::{attack_roll, AttackOutcome};
use crate::world::combat::{
    self, cantrip_k, exp_to_next, kill_value, share_kill, Class, Condition, Damage, DamageType, Dice, Mods, Reach, Save, Status,
    Weapon, CONDITIONS,
};
use crate::world::horde::storage::{FLAG_BOSS, FLAG_FULL};
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

/// A buff running on a player.
#[derive(Clone, Copy, Debug)]
struct Buff {
    /// The skill that gave it (a recast refreshes it).
    skill: u16,
    until: u32,
    mods: Mods,
    /// It granted temporary HP (gone with the last such buff).
    temp: bool,
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
    /// Ground that lasts while a slot is held.
    pub(super) channel: Option<super::cast::Channel>,
    /// Rooted after a cast; the basic attack waits.
    pub(super) recovery: u32,
    /// Modifiers of the learned passives.
    passive: Mods,
    /// Running buffs.
    buffs: Vec<Buff>,
    /// Passives plus buffs, summed once per tick.
    pub(super) mods: Mods,
    /// Temporary HP, spent before HP (Endure, Kyrie Eleison).
    pub temp_hp: i32,
    /// Ticks left per condition (monster skills), and the active set.
    cond: [u16; CONDITIONS],
    pub(super) status: Status,
    /// Kills of the party and experience gained since the stage began
    /// (the result screen).
    pub(super) run_exp: u64,
    /// Signature of the buff set last sent (`BUFFS`).
    pub(super) buffs_sent: u64,
    /// Level changed: skill points to resend.
    pub(super) skills_dirty: bool,
    attack_cd: u32,
    still: u32,
    pub(super) quiet: u32,
    revive: u32,
    regen_hp: f32,
    regen_sp: f32,
    /// Last vitals sent (hp, max_hp, sp, max_sp, state, status, temp HP,
    /// light).
    sent: Option<[i32; 8]>,
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
            channel: None,
            recovery: 0,
            passive: Mods::default(),
            buffs: Vec::new(),
            mods: Mods::default(),
            temp_hp: 0,
            cond: [0; CONDITIONS],
            status: Status(0),
            run_exp: 0,
            buffs_sent: 0,
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

    /// Standing and not playing dead: monsters come for it.
    pub fn hunted(&self) -> bool {
        self.is_up() && !self.mods.hidden
    }

    pub(super) fn set_passive(&mut self, passive: Mods) {
        self.passive = passive;
        self.sum_mods();
    }

    fn sum_mods(&mut self) {
        let mut m = self.passive;
        for b in &self.buffs {
            m.add(&b.mods);
        }
        self.mods = m;
    }

    /// Drops buffs that ran out (at tick `now`) and sums the rest.
    pub(super) fn refresh(&mut self, now: u32) {
        let n = self.buffs.len();
        self.buffs.retain(|b| b.until > now);
        if self.buffs.len() != n || self.buffs.is_empty() && self.mods != self.passive {
            self.sum_mods();
        }
        if self.temp_hp > 0 && !self.buffs.iter().any(|b| b.temp) {
            self.temp_hp = 0;
        }
    }

    /// Starts (or refreshes) the buff of `skill` until tick `until`, with
    /// `temp_hp` temporary HP (the larger pool is kept).
    pub(super) fn buff(&mut self, skill: u16, until: u32, mods: Mods, temp_hp: i32) {
        let temp = temp_hp > 0;
        match self.buffs.iter_mut().find(|b| b.skill == skill) {
            Some(b) => {
                b.until = b.until.max(until);
                b.mods = mods;
                b.temp |= temp;
            }
            None => self.buffs.push(Buff { skill, until, mods, temp }),
        }
        self.temp_hp = self.temp_hp.max(temp_hp);
        self.sum_mods();
    }

    /// Moving ends playing dead.
    pub(super) fn unhide(&mut self) {
        if self.mods.hidden {
            self.buffs.retain(|b| !b.mods.hidden);
            self.sum_mods();
        }
    }

    /// Leaves condition `c` for `ticks` (the longer of two is kept). Endure
    /// keeps the player on their feet.
    pub(super) fn afflict(&mut self, c: Condition, ticks: u16) {
        if self.mods.steady && matches!(c, Condition::Stunned | Condition::Frozen) {
            return;
        }
        let t = &mut self.cond[c as usize];
        *t = (*t).max(ticks);
        self.status.0 |= c.bit();
    }

    /// One tick of the condition timers.
    fn wear_off(&mut self) {
        let mut bits = 0;
        for (i, t) in self.cond.iter_mut().enumerate() {
            *t = t.saturating_sub(1);
            if *t > 0 {
                bits |= 1 << i;
            }
        }
        self.status = Status(bits);
    }

    pub(super) fn clear_conditions(&mut self) {
        self.cond = [0; CONDITIONS];
        self.status = Status(0);
    }

    /// Armour class with buffs and conditions.
    pub(super) fn armour(&self) -> i32 {
        self.ac + self.mods.ac + self.status.ac()
    }

    /// Saving throw bonus against `c` (or a plain Reflex save).
    pub(super) fn save_against(&self, c: Option<Condition>) -> i32 {
        self.class.save_bonus(self.level, c.map_or(Save::Reflex, Condition::save))
    }

    /// Takes a hit: resistance and armour, then temporary HP, then HP.
    /// Returns what came off HP and temporary HP together.
    pub(super) fn take(&mut self, amount: i32, physical: bool) -> i32 {
        let a = self.mods.taken(amount, physical);
        let soak = a.min(self.temp_hp);
        self.temp_hp -= soak;
        let mut rest = a - soak;
        if self.mods.mana_shield && rest > 0 {
            // Energy Coat: 1 SP stops 2 HP.
            let stopped = rest.min(self.sp.max(0) * 2);
            self.sp -= (stopped + 1) / 2;
            rest -= stopped;
        }
        self.hp -= rest;
        a
    }

    /// A hit from a monster: undead and demons hurt less with Divine
    /// Protection.
    pub(super) fn take_from(&mut self, amount: i32, physical: bool, evil: bool) -> i32 {
        let amount = if evil && self.mods.evil_resist > 0.0 {
            (amount as f32 * (1.0 - self.mods.evil_resist.min(0.9))).round() as i32
        } else {
            amount
        };
        self.take(amount, physical)
    }

    /// Turns a toggled buff of `skill` on (until cast again) or off.
    pub(super) fn toggle(&mut self, skill: u16, mods: Mods) {
        if let Some(i) = self.buffs.iter().position(|b| b.skill == skill) {
            self.buffs.swap_remove(i);
        } else {
            self.buffs.push(Buff { skill, until: u32::MAX, mods, temp: false });
        }
        self.sum_mods();
    }

    /// Changes when a buff starts, is refreshed or ends.
    pub(super) fn buff_signature(&self) -> u64 {
        self.buffs.iter().fold(self.buffs.len() as u64, |h, b| {
            h.rotate_left(13) ^ (u64::from(b.skill) << 32 | u64::from(b.until)).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        })
    }

    /// Running buffs as (skill, ticks left), for the HUD.
    pub(super) fn buff_list(&self, now: u32) -> impl Iterator<Item = (u16, u32)> + '_ {
        self.buffs.iter().map(move |b| (b.skill, b.until.saturating_sub(now)))
    }

    /// Downed at 0 HP (conditions end).
    pub(super) fn fall_if_spent(&mut self, hz: f32) {
        if self.hp <= 0 && self.is_up() {
            self.hp = 0;
            self.state = Vital::Downed { ticks: (DOWNED_SECS * hz) as u32 };
            self.revive = 0;
            self.clear_conditions();
        }
    }

    fn vitals(&self, entity_id: u32) -> ServerMsg {
        ServerMsg::PlayerVitals {
            entity_id,
            hp: self.hp.max(0) as u32,
            max_hp: self.max_hp as u32,
            sp: self.sp.max(0) as u32,
            max_sp: self.max_sp as u32,
            state: self.state.code(),
            status: self.status.0 as u32,
            temp_hp: self.temp_hp.max(0) as u32,
            light: (self.mods.light.max(0.0).round() as u32).min(255),
            level: self.level,
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
        self.run_exp += exp;
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

pub(super) fn damage_type_bits(kind: DamageType) -> u8 {
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

        // 1. Players: the basic attack while standing. It is an area too: a
        //    cone in front for melee, a splash at the impact for ranged; the
        //    target is the nearest monster in reach (index queries only).
        let mut targets: Vec<u16> = Vec::new();
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let Some(f) = p.fighter.as_mut() else { continue };
            f.attack_cd = f.attack_cd.saturating_sub(1);
            if !f.is_up() || p.dir != (0.0, 0.0) || f.mods.hidden || f.status.held() {
                f.still = 0;
                continue;
            }
            if f.casting.is_some() || f.channel.is_some() || f.recovery > 0 {
                continue;
            }
            f.still += 1;
            if f.still < STILL_TICKS || f.attack_cd > 0 {
                continue;
            }
            let weapon = f.weapon;
            let m = f.mods;
            let (dice, kind) = weapon.damage(f.class, f.level);
            let damage = Damage::new(Dice { bonus: dice.bonus + m.damage_flat, ..dice }, kind);
            let (range, arc_cos, splash) = match weapon.reach {
                Reach::Melee { range, arc_cos } => (range, Some(arc_cos), 0.0),
                Reach::Ranged { range, splash } => (range, None, splash),
            };
            let Some((target, _)) = horde.nearest_to(p.x, p.y, range, |_| false) else { continue };
            let store = horde.store();
            let (tx, ty) = (store.x[target as usize], store.y[target as usize]);
            let (ax, ay) = (tx - p.x, ty - p.y);
            let alen = (ax * ax + ay * ay).sqrt().max(1e-3);
            let area = match arc_cos {
                Some(arc_cos) => Area::Arc { x: p.x, y: p.y, dir: (ax / alen, ay / alen), r: range, arc_cos },
                None => Area::Circle { x: tx, y: ty, r: splash },
            };
            targets.clear();
            area.monsters(horde, &mut targets);
            if !targets.contains(&target) {
                targets.push(target);
            }
            let attacks = f.class.attacks_per_action(f.level);
            let entity = p.profile.character_id;
            let k = cantrip_k(f.level);
            for &s in &targets {
                let block = horde.block(s);
                for _ in 0..attacks {
                    let id = MONSTER_ENTITY_BASE + s as u32;
                    let status = horde.store().status_of(s);
                    let advantage = (status.advantage_against() + m.advantage as i8 + f.status.advantage()).clamp(-1, 1);
                    let ac = block.ac + status.ac();
                    let bless = if m.to_hit_die > 0 { rng.die(m.to_hit_die) as i32 } else { 0 };
                    match attack_roll(rng, weapon.to_hit(f.level) + m.to_hit + bless, ac, advantage, m.crit_from()) {
                        AttackOutcome::Miss => hits.push(HitEvent { target: id, amount: 0, flags: hit::MISS }),
                        outcome => {
                            let crit = outcome == AttackOutcome::Critical;
                            let bane = if horde.catalog().get(horde.store().kind[s as usize]).evil { m.evil_damage } else { 0 };
                            let amount = wound(horde, rng, s, damage, m.damage, crit, false, k) + bane;
                            horde.store_mut().hp[s as usize] -= bane;
                            let dead = horde.store().hp[s as usize] <= 0;
                            let mut flags = damage_type_bits(damage.kind);
                            if crit {
                                flags |= hit::CRITICAL;
                            }
                            if dead {
                                flags |= hit::KILL;
                                kills.push(s);
                            }
                            hits.push(HitEvent { target: id, amount: amount as u32, flags });
                            if dead {
                                break;
                            }
                        }
                    }
                }
            }
            f.attack_cd = (weapon.interval_ms as f32 * m.interval() * hz / 1000.0).round() as u32;
            f.quiet = 0;
            p.facing = facing_of(ax, ay);
            swings.push(Swing { entity_id: entity, facing: p.facing });
        }

        // 2. Kills: experience for everyone in the party on this map, then
        //    the slot goes back to the pool (the client sees a REMOVE).
        let mut boss_down = false;
        if !kills.is_empty() {
            let levels: Vec<u32> =
                self.players.iter().filter(|p| p.map == map).filter_map(|p| p.fighter.as_ref().map(|f| f.level)).collect();
            for &s in &kills {
                let store = horde.store();
                if !store.is_alive(s) {
                    continue;
                }
                boss_down |= store.flags[s as usize] & FLAG_BOSS != 0;
                self.kills += 1;
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

        // 3. Monster skills: wind-ups, landings, hazards on players.
        self.monster_skills(out, &mut hits);
        let Some(horde) = self.horde.as_mut() else { return };
        let rng = &mut self.dice;

        // 4. Monsters in contact strike the nearest standing player (a
        //    taunted one its taunter, when in reach).
        let ups: Vec<(usize, f32, f32, u32)> = self
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.map == map && p.fighter.as_ref().is_some_and(Fighter::hunted))
            .map(|(i, p)| (i, p.x, p.y, p.profile.user_id))
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
            let status = store.status_of(s);
            if store.flags[i] & FLAG_FULL == 0 || ups.is_empty() || status.held() {
                continue;
            }
            let (mx, my) = (store.x[i], store.y[i]);
            let reach = horde.catalog().get(horde.store().kind[i]).radius + PLAYER_RADIUS + MONSTER_REACH;
            let taunter = if status.has(Condition::Taunted) { horde.store().taunter[i] } else { 0 };
            let mut victim: Option<(usize, f32, f32, f32)> = None;
            for &(pi, px, py, uid) in &ups {
                let (dx, dy) = (px - mx, py - my);
                // The taunter sorts first.
                let d2 = dx * dx + dy * dy - if uid == taunter && taunter != 0 { 1e6 } else { 0.0 };
                if dx * dx + dy * dy <= reach * reach && victim.is_none_or(|(_, _, _, b)| d2 < b) {
                    victim = Some((pi, dx, dy, d2));
                }
            }
            let Some((pi, dx, dy, _)) = victim else { continue };
            // Provoked: it fights badly against anyone but its taunter.
            let off_target = taunter != 0 && ups.iter().find(|u| u.0 == pi).is_some_and(|u| u.3 != taunter);
            let block = horde.block(s);
            let p = &mut self.players[pi];
            let Some(f) = p.fighter.as_mut() else { continue };
            if !f.is_up() {
                continue;
            }
            let id = p.profile.character_id;
            if let Some(w) = self.lives.iter_mut().find(|z| z.shields(map, p.x, p.y)) {
                // Safety Wall: the blow lands on the ward.
                w.absorb();
                hits.push(HitEvent { target: id, amount: 0, flags: hit::GUARD });
            } else if f.mods.block > 0 && rng.chance(f.mods.block) {
                // Auto Guard: the shield takes it.
                hits.push(HitEvent { target: id, amount: 0, flags: hit::GUARD });
            } else {
                let advantage = (status.advantage() + f.status.advantage_against() - off_target as i8).clamp(-1, 1);
                match attack_roll(rng, block.attack, f.armour(), advantage, 20) {
                    AttackOutcome::Miss => hits.push(HitEvent { target: id, amount: 0, flags: hit::MISS }),
                    outcome => {
                        let crit = outcome == AttackOutcome::Critical;
                        let evil = horde.catalog().get(horde.store().kind[i]).evil;
                        let amount = f.take_from(block.damage.roll(rng, crit), true, evil);
                        let flags = damage_type_bits(DamageType::Physical) | if crit { hit::CRITICAL } else { 0 };
                        hits.push(HitEvent { target: id, amount: amount as u32, flags });
                        f.fall_if_spent(hz);
                    }
                }
            }
            f.quiet = 0;
            horde.store_mut().cooldown[i] = (block.attack_interval_ms as f32 * hz / 1000.0).round() as u16;
            swings.push(Swing { entity_id: MONSTER_ENTITY_BASE + s as u32, facing: facing_of(dx, dy) });
        }

        // 5. Downed, revive, regeneration.
        let wave = horde.waves();
        let new_wave = wave != self.last_wave;
        self.last_wave = wave;
        let positions: Vec<(u32, f32, f32, bool)> = self
            .players
            .iter()
            .filter(|p| p.map == map)
            .map(|p| (p.profile.user_id, p.x, p.y, p.fighter.as_ref().is_some_and(Fighter::is_up)))
            .collect();
        let tick = self.tick;
        let every = (hz as u32).max(1);
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let (uid, x, y) = (p.profile.user_id, p.x, p.y);
            let Some(f) = p.fighter.as_mut() else { continue };
            match f.state {
                Vital::Up => {
                    // Burning and poison bite once a second (staggered).
                    if f.status.ailing() && (tick + uid).is_multiple_of(every) {
                        for (c, kind) in [(Condition::Burning, DamageType::Fire), (Condition::Poisoned, DamageType::Poison)] {
                            if f.status.has(c) {
                                let amount = f.take(rng.die(4) as i32, false);
                                hits.push(HitEvent { target: p.profile.character_id, amount: amount as u32, flags: damage_type_bits(kind) });
                                f.quiet = 0;
                            }
                        }
                        f.fall_if_spent(hz);
                    }
                    f.wear_off();
                    if !f.is_up() {
                        continue;
                    }
                    f.quiet = f.quiet.saturating_add(1);
                    if f.quiet as f32 >= QUIET_SECS * hz && f.hp < f.max_hp {
                        f.regen_hp += f.max_hp as f32 * 0.005 / hz;
                    }
                    if f.mods.hp_regen > 0.0 && f.hp < f.max_hp {
                        f.regen_hp += f.max_hp as f32 * f.mods.hp_regen / hz;
                    }
                    let sp_rate = if p.dir == (0.0, 0.0) { 0.02 } else { 0.005 } + f.mods.sp_regen;
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

        // 6. Messages: combat to the map, vitals on change, progress to self.
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
            let now = [
                f.hp.max(0),
                f.max_hp,
                f.sp,
                f.max_sp,
                f.state.code() as i32,
                f.status.0 as i32,
                f.temp_hp,
                f.mods.light.round() as i32,
            ];
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

        // 7. Defeat: nobody on the map is standing.
        let fighters = self.players.iter().filter(|p| p.map == map && p.fighter.is_some()).count();
        let standing = self.players.iter().filter(|p| p.map == map && p.fighter.as_ref().is_some_and(Fighter::is_up)).count();
        if fighters > 0 && standing == 0 {
            self.defeat(map, out);
        }
        self.mission_step(map, boss_down, out);
    }

    /// The whole party is down: each member loses 1% of their level's
    /// requirement (never a level); the result screen, then the exit.
    fn defeat(&mut self, map: super::MapIdx, out: &mut Outgoing) {
        if self.outcome != super::mission::Outcome::Running {
            return;
        }
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let uid = p.profile.user_id;
            if let Some(f) = p.fighter.as_mut() {
                let penalty = exp_to_next(f.level) / 100;
                f.exp = f.exp.saturating_sub(penalty);
                f.gained = 0;
                out.push((uid, HubEvent::Control(f.progress())));
                out.push((uid, HubEvent::Progress { level: f.level, exp: f.exp }));
            }
        }
        self.end_stage(map, false, out);
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
