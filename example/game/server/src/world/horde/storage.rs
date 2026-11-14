//! Struct-of-arrays monster pool.
//!
//! Every column is allocated once at the pool capacity; spawning and
//! despawning only move slot indices between the FIFO free queue and the
//! dense `alive` list, so the steady state never touches the allocator.
//!
//! The free queue is FIFO on purpose: a freed slot rests behind every other
//! free slot before reuse, and its generation is bumped. Clients key monsters
//! on `(slot, generation)`, so a reused slot is a new entity to them, never the old
//! one sliding across the map.

use crate::world::combat::{Condition, Status, CONDITIONS};
use std::collections::VecDeque;

/// Simulated at full rate (inside some player's view, or a boss).
pub const FLAG_FULL: u8 = 1;
pub const FLAG_BOSS: u8 = 2;
/// Last line-of-sight probe to the nearest player succeeded.
pub const FLAG_LOS: u8 = 4;
/// A plant (`plants.rs`): rooted, harmless, immune to every condition.
pub const FLAG_PLANT: u8 = 8;
/// Called by a Summoner or split off a Mirror (`hub/affix.rs`): it pays no
/// experience, zeny or loot.
pub const FLAG_MINION: u8 = 16;
/// Burrowed (dived underground, `hub/boss.rs` Burrow, a burrowing
/// monster's skill): it still moves, but nothing can target, hit, push or
/// block it, and it neither strikes nor casts. Clients draw a moving mound
/// ([`STATUS_BURROWED`] on the wire). See [`HordeStorage::set_burrowed`].
pub const FLAG_BURROWED: u8 = 32;
/// Turns spells back (a scripted boss's shell, `hub/boss.rs` traits):
/// save-based blows on it do nothing to it and hit their caster for half
/// (`hub/cast.rs` strike).
pub const FLAG_REFLECT: u8 = 64;
/// The wire's status bit of a burrowed monster (`status`, past the
/// conditions' bits; kept by [`HordeStorage::tick_conditions`]).
pub const STATUS_BURROWED: u16 = 1 << 15;

/// [`HordeStorage::affix_state`] bits (the low ones are the wire's
/// `protocol::affix_state`).
pub const AFFIX_FRENZIED: u8 = 1;
pub const AFFIX_SHIELD_BROKEN: u8 = 2;
/// A Mirror that split already (it splits once).
pub const AFFIX_SPLIT: u8 = 8;

pub struct HordeStorage {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    /// Last tick's displacement (cells/tick), damped.
    pub vel_x: Vec<f32>,
    pub vel_y: Vec<f32>,
    pub hp: Vec<i32>,
    /// Level it spawned at (its combat numbers derive from it).
    pub level: Vec<u8>,
    /// Ticks until its next melee attack.
    pub cooldown: Vec<u16>,
    pub kind: Vec<u8>,
    pub flags: Vec<u8>,
    pub generation: Vec<u8>,
    /// Ticks spent outside every player's recycle radius (far mobs are
    /// teleported back just outside the light after a while).
    pub far_ticks: Vec<u16>,
    /// Ticks left per condition (index `Condition as usize`).
    pub cond: Vec<[u16; CONDITIONS]>,
    /// Active conditions (`Condition::bit`), what clients see.
    pub status: Vec<u16>,
    /// Cantrip multiplier of the fire it burns with.
    pub burn_k: Vec<u8>,
    /// Failed saves it may still turn into successes (bosses).
    pub legendary: Vec<u8>,
    /// User id of whoever taunted it (meaningful while Taunted).
    pub taunter: Vec<u32>,
    /// Ticks left of a flinch (hit stun): no steps, no attacks.
    pub stagger: Vec<u8>,
    /// Ticks before it can flinch again (no endless stun-lock).
    pub stagger_guard: Vec<u8>,
    /// Affixes (`combat::affixes::AffixFlags` bits; 0 for nearly all).
    pub affix: Vec<u16>,
    /// What is left of a Shielded monster's absorb shield.
    pub shield: Vec<i32>,
    /// Seconds an affixed monster has lived (its tricks' clock).
    pub affix_beat: Vec<u16>,
    /// Seconds since it last took damage (saturating).
    pub affix_quiet: Vec<u8>,
    /// `AFFIX_*` bits.
    pub affix_state: Vec<u8>,
    /// Damage taken since the last affix beat (Thorns reflects it).
    pub affix_taken: Vec<i32>,
    /// Dense list of live slots (iteration order).
    alive: Vec<u16>,
    /// Position of a slot in `alive` (`u16::MAX` when free).
    alive_pos: Vec<u16>,
    free: VecDeque<u16>,
}

impl HordeStorage {
    /// `capacity` ≤ 65535 slots.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0 && capacity < u16::MAX as usize, "pool capacity {capacity} out of range");
        Self {
            x: vec![0.0; capacity],
            y: vec![0.0; capacity],
            vel_x: vec![0.0; capacity],
            vel_y: vec![0.0; capacity],
            hp: vec![0; capacity],
            level: vec![1; capacity],
            cooldown: vec![0; capacity],
            kind: vec![0; capacity],
            flags: vec![0; capacity],
            generation: vec![0; capacity],
            far_ticks: vec![0; capacity],
            cond: vec![[0; CONDITIONS]; capacity],
            status: vec![0; capacity],
            burn_k: vec![0; capacity],
            legendary: vec![0; capacity],
            taunter: vec![0; capacity],
            stagger: vec![0; capacity],
            stagger_guard: vec![0; capacity],
            affix: vec![0; capacity],
            shield: vec![0; capacity],
            affix_beat: vec![0; capacity],
            affix_quiet: vec![0; capacity],
            affix_state: vec![0; capacity],
            affix_taken: vec![0; capacity],
            alive: Vec::with_capacity(capacity),
            alive_pos: vec![u16::MAX; capacity],
            free: (0..capacity as u16).collect(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.x.len()
    }

    pub fn len(&self) -> usize {
        self.alive.len()
    }

    pub fn is_empty(&self) -> bool {
        self.alive.is_empty()
    }

    pub fn alive(&self) -> &[u16] {
        &self.alive
    }

    pub fn is_alive(&self, slot: u16) -> bool {
        self.alive_pos.get(slot as usize).is_some_and(|&p| p != u16::MAX)
    }

    /// Takes the oldest free slot. Returns `None` when the pool is full.
    pub fn spawn(&mut self, kind: u8, x: f32, y: f32, hp: i32, flags: u8) -> Option<u16> {
        let slot = self.free.pop_front()?;
        let s = slot as usize;
        self.x[s] = x;
        self.y[s] = y;
        self.vel_x[s] = 0.0;
        self.vel_y[s] = 0.0;
        self.hp[s] = hp;
        self.level[s] = 1;
        self.cooldown[s] = 0;
        self.kind[s] = kind;
        self.flags[s] = flags;
        self.far_ticks[s] = 0;
        self.cond[s] = [0; CONDITIONS];
        self.status[s] = 0;
        self.burn_k[s] = 0;
        self.legendary[s] = 0;
        self.taunter[s] = 0;
        self.stagger[s] = 0;
        self.stagger_guard[s] = 0;
        self.affix[s] = 0;
        self.shield[s] = 0;
        self.affix_beat[s] = 0;
        self.affix_quiet[s] = 0;
        self.affix_state[s] = 0;
        self.affix_taken[s] = 0;
        self.alive_pos[s] = self.alive.len() as u16;
        self.alive.push(slot);
        Some(slot)
    }

    pub fn status_of(&self, slot: u16) -> Status {
        Status(self.status[slot as usize])
    }

    /// Puts `c` on a monster for `ticks` (a longer one already running wins).
    /// Plants shrug every condition off.
    pub fn afflict(&mut self, slot: u16, c: Condition, ticks: u16) {
        let s = slot as usize;
        if self.flags[s] & FLAG_PLANT != 0 {
            return;
        }
        let t = &mut self.cond[s][c as usize];
        *t = (*t).max(ticks);
        if *t > 0 {
            self.status[s] |= c.bit();
        }
    }

    /// A hit makes a non-boss monster flinch for `ticks` (RO's damage
    /// motion: it stops and cannot strike), unless it flinched within the
    /// last `guard` ticks after the previous one ended. Plants never do.
    pub fn flinch(&mut self, slot: u16, ticks: u8, guard: u8) {
        let s = slot as usize;
        if self.flags[s] & (FLAG_BOSS | FLAG_PLANT) != 0 || self.stagger[s] > 0 || self.stagger_guard[s] > 0 {
            return;
        }
        self.stagger[s] = ticks;
        self.stagger_guard[s] = guard;
    }

    /// Under the ground ([`FLAG_BURROWED`]).
    pub fn burrowed(&self, slot: u16) -> bool {
        self.flags[slot as usize] & FLAG_BURROWED != 0
    }

    /// Dives (`on`) or surfaces. Use [`super::Horde::set_burrowed`], which
    /// also keeps the query index right.
    pub fn set_burrowed(&mut self, slot: u16, on: bool) {
        let s = slot as usize;
        if on {
            self.flags[s] |= FLAG_BURROWED;
            self.status[s] |= STATUS_BURROWED;
        } else {
            self.flags[s] &= !FLAG_BURROWED;
            self.status[s] &= !STATUS_BURROWED;
        }
    }

    pub fn is_plant(&self, slot: u16) -> bool {
        self.flags[slot as usize] & FLAG_PLANT != 0
    }

    pub fn staggered(&self, slot: u16) -> bool {
        self.stagger[slot as usize] > 0
    }

    /// Ends `c` early (a physical hit shatters the ice).
    pub fn cure(&mut self, slot: u16, c: Condition) {
        let s = slot as usize;
        self.cond[s][c as usize] = 0;
        self.status[s] &= !c.bit();
    }

    /// One tick off every running condition.
    pub fn tick_conditions(&mut self) {
        for &slot in &self.alive {
            let s = slot as usize;
            if self.stagger[s] > 0 {
                self.stagger[s] -= 1;
            } else if self.stagger_guard[s] > 0 {
                self.stagger_guard[s] -= 1;
            }
            if self.status[s] == 0 {
                continue;
            }
            let mut bits = self.status[s] & STATUS_BURROWED;
            for (i, t) in self.cond[s].iter_mut().enumerate() {
                if *t > 0 {
                    *t -= 1;
                    if *t > 0 {
                        bits |= 1 << i;
                    }
                }
            }
            self.status[s] = bits;
        }
    }

    /// Frees a live slot; its generation is bumped. No-op for a free slot.
    pub fn despawn(&mut self, slot: u16) {
        let s = slot as usize;
        let pos = self.alive_pos[s];
        if pos == u16::MAX {
            return;
        }
        self.alive.swap_remove(pos as usize);
        if let Some(&moved) = self.alive.get(pos as usize) {
            self.alive_pos[moved as usize] = pos;
        }
        self.alive_pos[s] = u16::MAX;
        self.flags[s] = 0;
        self.generation[s] = self.generation[s].wrapping_add(1);
        self.free.push_back(slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flinch_holds_then_guards_and_skips_bosses() {
        let mut s = HordeStorage::new(4);
        let m = s.spawn(0, 1.0, 1.0, 10, 0).unwrap();
        let boss = s.spawn(0, 2.0, 2.0, 10, FLAG_BOSS).unwrap();
        s.flinch(m, 2, 3);
        s.flinch(boss, 2, 3);
        assert!(s.staggered(m));
        assert!(!s.staggered(boss), "bosses endure");
        s.tick_conditions();
        s.tick_conditions();
        assert!(!s.staggered(m));
        // Guarded right after: a new hit does not flinch it again.
        s.flinch(m, 2, 3);
        assert!(!s.staggered(m));
        for _ in 0..3 {
            s.tick_conditions();
        }
        s.flinch(m, 2, 3);
        assert!(s.staggered(m));
    }

    #[test]
    fn plants_take_no_condition_and_never_flinch() {
        let mut s = HordeStorage::new(2);
        let p = s.spawn(0, 1.0, 1.0, 5, FLAG_PLANT).unwrap();
        for c in Condition::ALL {
            s.afflict(p, c, 40);
        }
        s.flinch(p, 2, 3);
        assert_eq!((s.status_of(p), s.staggered(p), s.cond[p as usize]), (Status(0), false, [0; CONDITIONS]));
        assert!(s.is_plant(p));
    }

    #[test]
    fn spawn_despawn_keeps_dense_list_and_rests_slots() {
        let mut s = HordeStorage::new(4);
        let a = s.spawn(0, 1.0, 1.0, 10, 0).unwrap();
        let b = s.spawn(0, 2.0, 2.0, 10, 0).unwrap();
        let c = s.spawn(0, 3.0, 3.0, 10, 0).unwrap();
        assert_eq!((a, b, c), (0, 1, 2));
        s.despawn(a);
        assert_eq!(s.len(), 2);
        assert!(!s.is_alive(a));
        assert!(s.alive().contains(&b) && s.alive().contains(&c));
        assert_eq!(s.generation[a as usize], 1);
        // FIFO: the next spawn takes slot 3 (never used), not the freed 0.
        assert_eq!(s.spawn(0, 0.0, 0.0, 1, 0), Some(3));
        assert_eq!(s.spawn(0, 0.0, 0.0, 1, 0), Some(0));
        assert_eq!(s.spawn(0, 0.0, 0.0, 1, 0), None, "pool exhausted");
        s.despawn(a);
        s.despawn(a);
        assert_eq!(s.generation[a as usize], 2, "double despawn is a no-op");
        for &slot in s.alive() {
            assert_eq!(s.alive()[s.alive_pos[slot as usize] as usize], slot);
        }
    }

    #[test]
    fn conditions_run_out() {
        let mut s = HordeStorage::new(4);
        let a = s.spawn(0, 0.0, 0.0, 10, 0).unwrap();
        s.afflict(a, Condition::Frozen, 2);
        s.afflict(a, Condition::Frozen, 1);
        s.afflict(a, Condition::Slowed, 3);
        assert!(s.status_of(a).has(Condition::Frozen) && s.status_of(a).has(Condition::Slowed));
        s.tick_conditions();
        assert!(s.status_of(a).has(Condition::Frozen), "the longer freeze is kept");
        s.tick_conditions();
        assert_eq!(s.status_of(a), Status(Condition::Slowed.bit()));
        s.cure(a, Condition::Slowed);
        assert_eq!(s.status_of(a), Status(0));
        s.afflict(a, Condition::Stunned, 5);
        s.legendary[a as usize] = 2;
        let mut one = HordeStorage::new(1);
        let x = one.spawn(0, 0.0, 0.0, 10, 0).unwrap();
        one.afflict(x, Condition::Stunned, 5);
        one.legendary[x as usize] = 2;
        one.despawn(x);
        let y = one.spawn(0, 0.0, 0.0, 10, 0).unwrap();
        assert_eq!(x, y);
        assert_eq!((one.status_of(y), one.legendary[y as usize]), (Status(0), 0), "a reused slot starts clean");
    }

    #[test]
    fn steady_state_does_not_reallocate() {
        let mut s = HordeStorage::new(64);
        let cap = (s.alive.capacity(), s.free.capacity());
        for round in 0..1000 {
            while s.spawn(0, 0.0, 0.0, 1, 0).is_some() {}
            let victims: Vec<u16> = s.alive().iter().copied().filter(|v| (*v as usize + round).is_multiple_of(3)).collect();
            for v in victims {
                s.despawn(v);
            }
        }
        assert_eq!((s.alive.capacity(), s.free.capacity()), cap);
    }
}
