//! What one player's client knows about the horde, and the delta that
//! brings it up to date each tick.
//!
//! The client renders every monster by dead reckoning from its last record
//! `(p, v, stamp)`: `p + v · clamp(t − stamp, −BACK, HORIZON)` (see
//! [`predict`]). The server keeps the same records per player
//! ([`MonsterView`]) and each tick sends only the monsters whose real
//! position drifted from that prediction by more than a distance-dependent
//! threshold, plus monsters entering and leaving the light. A monster
//! standing still or walking a straight line costs nothing; a typical
//! correction costs 5 bytes.
//!
//! Each frame has a byte budget. When more monsters are due than fit,
//! removals go first (they are one byte), then newcomers nearest first (a
//! missing monster is worse than a drifting one), then corrections by
//! drift over threshold. Whatever is
//! left keeps drifting and only gains priority, so the budget spreads a
//! burst over the next ticks instead of sending one huge frame.
//!
//! The transport is a reliable, ordered WebSocket, so each frame is a delta
//! against the previous frame sent to that player. If a frame is lost
//! anyway (the instance drops snapshots when a player's outbox is full) or
//! the client reports a gap, [`MonsterView::reset`] makes the next frame a
//! keyframe: the client drops everything and the view refills nearest
//! first within the budget.
//!
//! Wire layout of the monster section (after the players of a
//! `STAGE_SNAPSHOT`):
//!
//! ```text
//! var  baseline   tick − tick of the frame this delta applies to; 0 = keyframe
//! var  count
//! count × entry, in increasing slot order:
//!   var  head     (slot − previous slot − 1) << 2 | op   (previous = −1 at start)
//!   op 0 NUDGE    i8 dx, i8 dy (1/64 cell, from the known p), i8 vx, i8 vy
//!   op 1 MOVE     u16 x, u16 y, i8 vx, i8 vy
//!   op 2 SPAWN    u8 generation, u8 kind, u16 x, u16 y, i8 vx, i8 vy
//!   op 3 REMOVE   —
//! ```
//!
//! Positions are cells in 1/64 fixed point, velocities cells per tick in
//! 1/256 (±0.5 cells/tick, above the fastest monster). Every record in a
//! frame gets the frame's tick as its stamp.

use super::storage::FLAG_FULL;
use super::{in_radius, Horde, PlayerView};
use crate::world::wire::{PutWire, Reader};
use bytes::{BufMut, BytesMut};

/// Position fixed point: units per cell.
pub const POS_ONE: f32 = 64.0;
/// Velocity fixed point: units per cell per tick.
pub const VEL_ONE: f32 = 256.0;
/// Dead reckoning runs at most this many ticks past a record's stamp, then
/// holds. The server refreshes a moving monster before that.
pub const HORIZON: u32 = 16;
/// ... and at most this many ticks before it (the client renders slightly
/// in the past, so a fresh record is evaluated before its stamp).
pub const BACK: u32 = 8;
/// Known monsters stay in the view until this far beyond the send radius,
/// so one walking along the edge does not flicker in and out.
const KEEP_MARGIN: f32 = 2.0;
/// Conservative bytes per entry head (a slot gap above 31 takes two).
const HEAD_COST: usize = 2;
/// Above any drift ratio a correction reaches in practice, and small
/// enough that f32 still resolves the nearness fraction added to it.
const SPAWN_PRIORITY: f32 = 1.0e4;

pub const OP_NUDGE: u8 = 0;
pub const OP_MOVE: u8 = 1;
pub const OP_SPAWN: u8 = 2;
pub const OP_REMOVE: u8 = 3;

/// Tuning of the monster section.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewConfig {
    /// Monster bytes per frame and player: a bandwidth cap (32 KB/s at
    /// 20 Hz). Up to ~700 monsters in one light radius never reach it
    /// (`examples/snapshot_bench.rs`); beyond that the drift grows instead.
    pub budget: usize,
    /// Allowed drift (cells) near the player ...
    pub near_error: f32,
    /// ... and at the edge of the light (where monsters dissolve anyway).
    pub edge_error: f32,
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self { budget: 1600, near_error: 0.05, edge_error: 0.3 }
    }
}

/// A dead-reckoning record, as both sides store it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub x: u16,
    pub y: u16,
    pub vx: i8,
    pub vy: i8,
    pub stamp: u32,
}

/// Position of `r` at fractional tick `t` (cells).
pub fn predict(r: &Record, t: f64) -> (f32, f32) {
    let age = (t - r.stamp as f64).clamp(-(BACK as f64), HORIZON as f64) as f32;
    (
        r.x as f32 / POS_ONE + r.vx as f32 / VEL_ONE * age,
        r.y as f32 / POS_ONE + r.vy as f32 / VEL_ONE * age,
    )
}

fn quant_vel(v: f32) -> i8 {
    (v * VEL_ONE).round().clamp(-127.0, 127.0) as i8
}

fn quant_pos(v: f32) -> u16 {
    (v * POS_ONE).round().clamp(0.0, u16::MAX as f32) as u16
}

#[derive(Clone, Copy, Default)]
struct Known {
    present: bool,
    generation: u8,
    rec: Record,
    /// Last tick the monster was within keep range.
    seen: u32,
}

#[derive(Clone, Copy)]
struct Candidate {
    slot: u16,
    spawn: bool,
    /// The known position fits a NUDGE.
    nudge: bool,
    priority: f32,
}

#[derive(Clone, Copy)]
struct Entry {
    slot: u16,
    op: u8,
    generation: u8,
    kind: u8,
    rec: Record,
    /// Previous position, for NUDGE.
    from: (u16, u16),
}

/// Per-player mirror of the client's monster records.
pub struct MonsterView {
    known: Vec<Known>,
    list: Vec<u16>,
    last_frame: Option<u32>,
    candidates: Vec<Candidate>,
    entries: Vec<Entry>,
}

/// What one [`Horde::write_delta`] call put in the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeltaStats {
    pub bytes: usize,
    pub spawns: u32,
    pub updates: u32,
    pub removes: u32,
    /// Due but left for a later frame (over budget).
    pub deferred: u32,
    /// Monsters the client knows after this frame.
    pub known: u32,
}

impl std::fmt::Debug for MonsterView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonsterView").field("known", &self.list.len()).field("last_frame", &self.last_frame).finish()
    }
}

impl MonsterView {
    pub fn new(pool: usize) -> Self {
        Self {
            known: vec![Known::default(); pool],
            list: Vec::with_capacity(pool.min(2048)),
            last_frame: None,
            candidates: Vec::with_capacity(1024),
            entries: Vec::with_capacity(1024),
        }
    }

    /// The next frame is a keyframe (after a lost frame or a client resync).
    pub fn reset(&mut self) {
        for &s in &self.list {
            self.known[s as usize].present = false;
        }
        self.list.clear();
        self.last_frame = None;
    }

    pub fn known(&self) -> usize {
        self.list.len()
    }
}

impl Horde {
    /// Appends `viewer`'s monster section for `tick` to `out` and advances
    /// `view` to what the client will know after it.
    pub fn write_delta(
        &self,
        view: &mut MonsterView,
        viewer: &PlayerView,
        tick: u32,
        out: &mut BytesMut,
    ) -> DeltaStats {
        let config = &self.config.view;
        let start = out.len();
        let reach = viewer.light + self.config.aoi_margin;
        let keep = reach + KEEP_MARGIN;
        let s = &self.store;
        let MonsterView { known, list, last_frame, candidates, entries } = view;
        let keyframe = last_frame.is_none();
        let t = tick as f64;
        candidates.clear();
        entries.clear();

        for &slot in s.alive() {
            let i = slot as usize;
            if s.flags[i] & FLAG_FULL == 0 {
                continue;
            }
            let (dx, dy) = (s.x[i] - viewer.x, s.y[i] - viewer.y);
            if !in_radius(dx, dy, keep) {
                continue;
            }
            let k = &mut known[i];
            let d = (dx * dx + dy * dy).sqrt();
            if k.present && k.generation == s.generation[i] {
                k.seen = tick;
                let (px, py) = predict(&k.rec, t);
                let (ex, ey) = (s.x[i] - px, s.y[i] - py);
                let near = viewer.light * 0.5;
                let u = ((d - near) / (viewer.light - near)).clamp(0.0, 1.0);
                let th = config.near_error + (config.edge_error - config.near_error) * u;
                let err2 = ex * ex + ey * ey;
                let moving = k.rec.vx != 0 || k.rec.vy != 0;
                let stale = moving && tick.wrapping_sub(k.rec.stamp) + 2 >= HORIZON;
                if err2 >= th * th || stale {
                    let small = |a: f32, b: u16| (quant_pos(a) as i32 - b as i32).abs() <= 127;
                    let nudge = small(s.x[i], k.rec.x) && small(s.y[i], k.rec.y);
                    candidates.push(Candidate { slot, spawn: false, nudge, priority: (err2 / (th * th)).sqrt().max(1.0) });
                }
            } else if in_radius(dx, dy, reach) {
                // Newcomers before any correction (a missing monster is worse
                // than a drifting one), nearest first.
                candidates.push(Candidate { slot, spawn: true, nudge: false, priority: SPAWN_PRIORITY + 1.0 - d / reach });
            }
        }

        // Removals: out of range, dead, or the slot now holds another
        // monster. Always sent; a replacing SPAWN supersedes the REMOVE.
        let mut used = 0usize;
        for &slot in list.iter() {
            let k = &known[slot as usize];
            let alive_same = s.is_alive(slot) && s.generation[slot as usize] == k.generation;
            if k.seen != tick || !alive_same {
                entries.push(Entry { slot, op: OP_REMOVE, generation: 0, kind: 0, rec: Record::default(), from: (0, 0) });
                used += HEAD_COST;
            }
        }

        let cost = |c: &Candidate| HEAD_COST + if c.spawn { 8 } else if c.nudge { 4 } else { 6 };
        let total: usize = candidates.iter().map(cost).sum();
        if used + total > config.budget {
            candidates.sort_unstable_by(|a, b| b.priority.total_cmp(&a.priority));
        }
        let mut deferred = 0u32;
        let (mut spawns, mut updates) = (0u32, 0u32);
        for c in candidates.iter() {
            let need = cost(c);
            if used + need > config.budget {
                deferred += 1;
                continue;
            }
            used += need;
            let i = c.slot as usize;
            let rec = Record {
                x: quant_pos(s.x[i]),
                y: quant_pos(s.y[i]),
                vx: quant_vel(s.vel_x[i]),
                vy: quant_vel(s.vel_y[i]),
                stamp: tick,
            };
            let k = &known[i];
            if c.spawn {
                spawns += 1;
                entries.push(Entry { slot: c.slot, op: OP_SPAWN, generation: s.generation[i], kind: s.kind[i], rec, from: (0, 0) });
            } else {
                updates += 1;
                let op = if c.nudge { OP_NUDGE } else { OP_MOVE };
                entries.push(Entry { slot: c.slot, op, generation: 0, kind: 0, rec, from: (k.rec.x, k.rec.y) });
            }
        }

        // One entry per slot: a SPAWN replacing a stale record drops its REMOVE.
        entries.sort_unstable_by_key(|e| (e.slot, e.op != OP_REMOVE));
        entries.dedup_by(|later, first| {
            // `first` is kept; it is the REMOVE when a SPAWN shares the slot.
            if later.slot == first.slot {
                *first = *later;
                true
            } else {
                false
            }
        });

        // Apply to the mirror.
        let mut removes = 0u32;
        for e in entries.iter() {
            let k = &mut known[e.slot as usize];
            match e.op {
                OP_REMOVE => {
                    removes += 1;
                    k.present = false;
                }
                OP_SPAWN => {
                    // A replaced slot is already listed.
                    if !k.present {
                        list.push(e.slot);
                    }
                    *k = Known { present: true, generation: e.generation, rec: e.rec, seen: tick };
                }
                _ => k.rec = e.rec,
            }
        }
        if removes > 0 {
            list.retain(|&s| known[s as usize].present);
        }

        // Encode.
        out.put_var(match *last_frame {
            Some(prev) if !keyframe => u64::from(tick.wrapping_sub(prev)),
            _ => 0,
        });
        out.put_var(entries.len() as u64);
        let mut prev: i32 = -1;
        for e in entries.iter() {
            let gap = (e.slot as i32 - prev - 1) as u64;
            prev = e.slot as i32;
            out.put_var(gap << 2 | u64::from(e.op));
            let r = &e.rec;
            match e.op {
                OP_NUDGE => {
                    out.put_i8((r.x as i32 - e.from.0 as i32) as i8);
                    out.put_i8((r.y as i32 - e.from.1 as i32) as i8);
                }
                OP_MOVE => {
                    out.put_u16_le(r.x);
                    out.put_u16_le(r.y);
                }
                OP_SPAWN => {
                    out.put_u8(e.generation);
                    out.put_u8(e.kind);
                    out.put_u16_le(r.x);
                    out.put_u16_le(r.y);
                }
                _ => continue,
            }
            out.put_i8(r.vx);
            out.put_i8(r.vy);
        }
        *last_frame = Some(tick);
        DeltaStats {
            bytes: out.len() - start,
            spawns,
            updates,
            removes,
            deferred,
            known: list.len() as u32,
        }
    }
}

/// One decoded entry of a monster section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeltaEntry {
    Nudge { slot: u16, dx: i8, dy: i8, vx: i8, vy: i8 },
    Move { slot: u16, x: u16, y: u16, vx: i8, vy: i8 },
    Spawn { slot: u16, generation: u8, kind: u8, x: u16, y: u16, vx: i8, vy: i8 },
    Remove { slot: u16 },
}

/// A decoded monster section.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonsterDelta {
    /// 0 = keyframe.
    pub baseline: u32,
    pub entries: Vec<DeltaEntry>,
}

impl MonsterDelta {
    pub fn decode(r: &mut Reader<'_>) -> Option<Self> {
        let baseline = r.var32()?;
        let count = r.var32()? as usize;
        // Every entry is at least one byte: bound the allocation by the frame.
        if count > r.remaining() {
            return None;
        }
        let mut entries = Vec::with_capacity(count);
        let mut prev: i64 = -1;
        for _ in 0..count {
            let head = r.var()?;
            let slot = prev + 1 + (head >> 2) as i64;
            let slot = u16::try_from(slot).ok()?;
            prev = slot as i64;
            entries.push(match (head & 3) as u8 {
                OP_NUDGE => DeltaEntry::Nudge { slot, dx: r.i8()?, dy: r.i8()?, vx: r.i8()?, vy: r.i8()? },
                OP_MOVE => DeltaEntry::Move { slot, x: r.u16()?, y: r.u16()?, vx: r.i8()?, vy: r.i8()? },
                OP_SPAWN => DeltaEntry::Spawn {
                    slot,
                    generation: r.u8()?,
                    kind: r.u8()?,
                    x: r.u16()?,
                    y: r.u16()?,
                    vx: r.i8()?,
                    vy: r.i8()?,
                },
                _ => DeltaEntry::Remove { slot },
            });
        }
        Some(Self { baseline, entries })
    }
}

/// A monster delta that does not apply to what the client holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutOfSync;

/// A monster as the client knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mirrored {
    pub generation: u8,
    pub kind: u8,
    pub rec: Record,
}

/// Client-side state rebuilt from monster sections (tests, bots, benches;
/// the Dart client does the same).
#[derive(Clone, Debug, Default)]
pub struct MonsterMirror {
    pub monsters: std::collections::BTreeMap<u16, Mirrored>,
    last_tick: Option<u32>,
}

impl MonsterMirror {
    /// Applies the section of the frame at `tick`. Fails when it is a delta
    /// against a frame this mirror has not applied (the client then asks
    /// for a resync and ignores deltas until a keyframe).
    pub fn apply(&mut self, tick: u32, delta: &MonsterDelta) -> Result<(), OutOfSync> {
        if delta.baseline == 0 {
            self.monsters.clear();
        } else if self.last_tick != Some(tick.wrapping_sub(delta.baseline)) {
            self.last_tick = None;
            return Err(OutOfSync);
        }
        for e in &delta.entries {
            match *e {
                DeltaEntry::Spawn { slot, generation, kind, x, y, vx, vy } => {
                    self.monsters.insert(slot, Mirrored { generation, kind, rec: Record { x, y, vx, vy, stamp: tick } });
                }
                DeltaEntry::Move { slot, x, y, vx, vy } => {
                    let m = self.monsters.get_mut(&slot).ok_or(OutOfSync)?;
                    m.rec = Record { x, y, vx, vy, stamp: tick };
                }
                DeltaEntry::Nudge { slot, dx, dy, vx, vy } => {
                    let m = self.monsters.get_mut(&slot).ok_or(OutOfSync)?;
                    let x = (m.rec.x as i32 + dx as i32).clamp(0, u16::MAX as i32) as u16;
                    let y = (m.rec.y as i32 + dy as i32).clamp(0, u16::MAX as i32) as u16;
                    m.rec = Record { x, y, vx, vy, stamp: tick };
                }
                DeltaEntry::Remove { slot } => {
                    self.monsters.remove(&slot);
                }
            }
        }
        self.last_tick = Some(tick);
        Ok(())
    }
}
