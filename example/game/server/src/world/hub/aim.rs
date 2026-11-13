//! Target archetypes (COMBAT.md §4.3): where a skill goes when it is
//! pressed. Monster lookups go through the horde's spatial index, so their
//! cost follows the skill's range, not the horde size; allies are at most a
//! party.

use super::area::Area;
use super::fight::Fighter;
use super::{HubPlayer, MapIdx};
use crate::world::combat::skills::{Effect, MoveMode, Payload, Shape, SkillDef, Target};
use crate::world::horde::storage::FLAG_BOSS;
use crate::world::horde::Horde;

/// Monsters this close decide where an escape goes (cells; 5 m).
const ESCAPE_SCAN: f32 = 10.0;
/// `densest` tries at most this many candidate centres.
const DENSEST_CANDIDATES: usize = 32;
/// `line` sorts monsters into this many angle buckets.
const LINE_BUCKETS: usize = 16;
/// One aim unit on the wire is 1/8 m = 0.25 cells.
const AIM_UNIT: f32 = 0.25;
/// "Threatened": monsters within this of an ally (cells; 2 m).
const THREAT_RADIUS: f32 = 4.0;
/// Player body radius (cells).
pub(super) const BODY: f32 = 0.4;

/// Where a skill lands, decided when it is pressed (the telegraph is honest).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Aimed {
    /// The target point.
    pub x: f32,
    pub y: f32,
    /// The caster's position at the press (lines and cones start here).
    pub ox: f32,
    pub oy: f32,
    /// Unit direction from the caster.
    pub dir: (f32, f32),
    /// The picked ally (user id): heals, buffs and revives always reach it.
    pub ally: Option<u32>,
    /// The picked monster `(slot, generation)`: homing hits follow it.
    pub monster: Option<(u16, u8)>,
}

pub(super) fn norm(x: f32, y: f32) -> Option<(f32, f32)> {
    let l = (x * x + y * y).sqrt();
    (l > 1e-4).then(|| (x / l, y / l))
}

pub(super) fn facing_dir(facing: u8) -> (f32, f32) {
    // Facing 0 = north (+y), clockwise in eighths, as `facing_of`.
    let a = facing as f32 * std::f32::consts::FRAC_PI_4;
    (a.sin(), a.cos())
}

/// The radius a skill's first area effect covers (for picking centres).
pub(super) fn reach_of(def: &SkillDef, slv: u8) -> f32 {
    def.effects
        .iter()
        .filter_map(Effect::act)
        .find_map(|a| {
            let r = match a.area.shape {
                Shape::Circle | Shape::Arc => a.area.radius.at(slv),
                Shape::Rect => a.area.length.at(slv).max(a.area.width.at(slv)) / 2.0,
                Shape::Line => a.area.width.at(slv) / 2.0,
            };
            (r > 0.0).then_some(r + a.area.scatter * 0.5)
        })
        .unwrap_or(2.0)
        .max(1.0)
}

/// Read-only view of the stage used by target selection.
pub(super) struct Field<'a> {
    pub horde: &'a Horde,
    pub players: &'a [HubPlayer],
    pub map: MapIdx,
}

impl Field<'_> {
    /// Live monsters whose body is within `range` of a point: `(slot, x, y)`.
    pub fn monsters_near(&self, x: f32, y: f32, range: f32) -> Vec<(u16, f32, f32)> {
        let mut slots = Vec::new();
        Area::Circle { x, y, r: range }.monsters(self.horde, &mut slots);
        let store = self.horde.store();
        slots.into_iter().map(|s| (s, store.x[s as usize], store.y[s as usize])).collect()
    }

    fn tag(&self, slot: u16) -> (u16, u8) {
        (slot, self.horde.store().generation[slot as usize])
    }

    pub fn nearest(&self, x: f32, y: f32, range: f32) -> Option<(u16, f32, f32)> {
        let (s, _) = self.horde.nearest_to(x, y, range, |_| false)?;
        let store = self.horde.store();
        Some((s, store.x[s as usize], store.y[s as usize]))
    }

    /// Centre covering the most monsters within `radius`, among monsters in
    /// range of the caster: up to 32 candidate centres, each counted
    /// through the index.
    fn densest(&self, x: f32, y: f32, range: f32, radius: f32) -> Option<(f32, f32)> {
        let near = self.monsters_near(x, y, range);
        if near.is_empty() {
            return None;
        }
        let stride = near.len().div_ceil(DENSEST_CANDIDATES).max(1);
        let mut buf = Vec::new();
        let mut best = (near[0].1, near[0].2, 0usize);
        for &(_, cx, cy) in near.iter().step_by(stride) {
            buf.clear();
            Area::Circle { x: cx, y: cy, r: radius }.monsters(self.horde, &mut buf);
            if buf.len() > best.2 {
                best = (cx, cy, buf.len());
            }
        }
        // Centre on what the best candidate covers, kept inside the range.
        buf.clear();
        Area::Circle { x: best.0, y: best.1, r: radius }.monsters(self.horde, &mut buf);
        let store = self.horde.store();
        let (mut cx, mut cy) = (best.0, best.1);
        if !buf.is_empty() {
            let n = buf.len() as f32;
            cx = buf.iter().map(|&s| store.x[s as usize]).sum::<f32>() / n;
            cy = buf.iter().map(|&s| store.y[s as usize]).sum::<f32>() / n;
        }
        let (dx, dy) = (cx - x, cy - y);
        let d = (dx * dx + dy * dy).sqrt();
        if d > range && d > 0.0 {
            (cx, cy) = (x + dx / d * range, y + dy / d * range);
        }
        Some((cx, cy))
    }

    /// Bosses first, then the highest HP, ties to the nearest.
    fn strongest(&self, x: f32, y: f32, range: f32) -> Option<(u16, f32, f32)> {
        let store = self.horde.store();
        self.monsters_near(x, y, range).into_iter().max_by(|a, b| {
            let key = |m: &(u16, f32, f32)| {
                let i = m.0 as usize;
                (store.flags[i] & FLAG_BOSS != 0, store.hp[i])
            };
            let d = |m: &(u16, f32, f32)| (m.1 - x).powi(2) + (m.2 - y).powi(2);
            key(a).cmp(&key(b)).then(d(b).total_cmp(&d(a)))
        })
    }

    /// Direction through the most monsters in range: 16 angle buckets,
    /// the fullest one's mean direction.
    fn best_line(&self, x: f32, y: f32, range: f32) -> Option<(f32, f32)> {
        let near = self.monsters_near(x, y, range);
        let mut sum = [(0.0f32, 0.0f32, 0u32); LINE_BUCKETS];
        for &(_, mx, my) in &near {
            let Some((ux, uy)) = norm(mx - x, my - y) else { continue };
            let a = uy.atan2(ux).rem_euclid(std::f32::consts::TAU);
            let b = ((a / std::f32::consts::TAU * LINE_BUCKETS as f32) as usize).min(LINE_BUCKETS - 1);
            sum[b] = (sum[b].0 + ux, sum[b].1 + uy, sum[b].2 + 1);
        }
        // A bucket and its neighbours: a line is wider than one bucket up close.
        let score = |b: usize| (0..3).map(|o| sum[(b + LINE_BUCKETS + o - 1) % LINE_BUCKETS].2).sum::<u32>();
        let best = (0..LINE_BUCKETS).filter(|&b| sum[b].2 > 0).max_by_key(|&b| (score(b), sum[b].2))?;
        norm(sum[best].0, sum[best].1)
    }

    pub fn allies(&self, x: f32, y: f32, range: f32) -> impl Iterator<Item = (&HubPlayer, &Fighter)> + '_ {
        let map = self.map;
        self.players.iter().filter_map(move |p| {
            let f = p.fighter.as_ref()?;
            (p.map == map && f.is_up() && (p.x - x).powi(2) + (p.y - y).powi(2) <= (range + BODY).powi(2)).then_some((p, f))
        })
    }

    /// Monsters crowding an ally.
    fn threat(&self, p: &HubPlayer) -> usize {
        let mut buf = Vec::new();
        Area::Circle { x: p.x, y: p.y, r: THREAT_RADIUS }.monsters(self.horde, &mut buf);
        buf.len()
    }
}

/// It strikes monsters at once (a nova, a taunt), rather than laying
/// ground that waits for them (an aura, a sanctum).
fn strikes_now(def: &SkillDef, slv: u8) -> bool {
    def.effects
        .iter()
        .filter_map(Effect::act)
        .any(|a| a.payload.hostile() && !matches!(a.payload, Payload::Barrier) && !a.timing.lasting(slv) && a.timing.trigger <= 0.0)
}

/// Picks where a skill goes; `None` = no valid target, nothing is spent.
pub(super) fn aim_skill(field: &Field<'_>, caster: &HubPlayer, def: &SkillDef, slv: u8) -> Option<Aimed> {
    let (px, py) = (caster.x, caster.y);
    let aim = (caster.aim[0] != 0 || caster.aim[1] != 0)
        .then(|| (caster.aim[0] as f32 * AIM_UNIT, caster.aim[1] as f32 * AIM_UNIT));
    let range = def.range;
    let base = Aimed { x: px, y: py, ox: px, oy: py, dir: facing_dir(caster.facing), ally: None, monster: None };
    let at = |x: f32, y: f32| Aimed { x, y, dir: norm(x - px, y - py).unwrap_or(base.dir), ..base };
    let on = |m: (u16, f32, f32)| Aimed { monster: Some(field.tag(m.0)), ..at(m.1, m.2) };
    let ally = |p: &HubPlayer| Aimed { ally: Some(p.profile.user_id), ..at(p.x, p.y) };
    match def.target {
        // A nova or a taunt around the caster waits for something to hit.
        Target::Caster if strikes_now(def, slv) => field.nearest(px, py, reach_of(def, slv)).map(|_| base),
        Target::Caster => Some(base),
        Target::Nearest => field.nearest(px, py, range).map(on),
        Target::Strongest => field.strongest(px, py, range).map(on),
        Target::NearestGroup => {
            let first = field.nearest(px, py, range)?;
            let group = field.monsters_near(first.1, first.2, reach_of(def, slv));
            let n = group.len().max(1) as f32;
            let (cx, cy) = (group.iter().map(|m| m.1).sum::<f32>() / n, group.iter().map(|m| m.2).sum::<f32>() / n);
            Some(Aimed { monster: Some(field.tag(first.0)), ..at(cx, cy) })
        }
        Target::Densest | Target::Spread => field.densest(px, py, range, reach_of(def, slv)).map(|(x, y)| at(x, y)),
        Target::Line | Target::Front | Target::Aim => {
            let dir = match aim.and_then(|(ax, ay)| norm(ax, ay)) {
                Some(d) => d,
                None if def.target == Target::Line => field.best_line(px, py, range.max(reach_of(def, slv)))?,
                None => {
                    let (_, nx, ny) = field.nearest(px, py, range.max(reach_of(def, slv)))?;
                    norm(nx - px, ny - py).unwrap_or(base.dir)
                }
            };
            let d = match def.target {
                Target::Line => range,
                // The point only tells clients the direction.
                Target::Front => range.max(1.0),
                _ => aim.map_or(range * 0.5, |(ax, ay)| (ax * ax + ay * ay).sqrt().min(range)),
            };
            Some(Aimed { x: px + dir.0 * d, y: py + dir.1 * d, dir, ..base })
        }
        Target::Escape => {
            let slide = def.effects.iter().any(|e| matches!(e, Effect::Move { mode: MoveMode::Slide, .. }));
            let near = field.monsters_near(px, py, ESCAPE_SCAN);
            let away = (!near.is_empty()).then(|| {
                let n = near.len() as f32;
                let (cx, cy) = (near.iter().map(|m| m.1).sum::<f32>() / n, near.iter().map(|m| m.2).sum::<f32>() / n);
                norm(px - cx, py - cy)
            });
            let dir = match (aim.and_then(|(ax, ay)| norm(ax, ay)), away.flatten()) {
                // A slide goes back from the aim, a blink along it.
                (Some(a), _) => {
                    if slide {
                        (-a.0, -a.1)
                    } else {
                        a
                    }
                }
                (None, Some(away)) => away,
                (None, None) => {
                    let f = base.dir;
                    if slide { (-f.0, -f.1) } else { f }
                }
            };
            Some(Aimed { dir, ..base })
        }
        Target::Allies => Some(Aimed { ally: Some(caster.profile.user_id), ..base }),
        Target::MostWoundedAlly => {
            let (p, _) = field
                .allies(px, py, range)
                .filter(|(_, f)| f.hp < f.max_hp)
                .min_by(|a, b| (a.1.hp as f32 / a.1.max_hp as f32).total_cmp(&(b.1.hp as f32 / b.1.max_hp as f32)))?;
            Some(ally(p))
        }
        Target::MostThreatenedAlly => {
            let (p, _) = field.allies(px, py, range).max_by_key(|(p, _)| field.threat(p))?;
            Some(ally(p))
        }
        Target::DensestAllies => {
            let r = reach_of(def, slv);
            let allies: Vec<(f32, f32)> = field.allies(px, py, range).map(|(p, _)| (p.x, p.y)).collect();
            let best = allies.iter().max_by_key(|a| {
                allies.iter().filter(|b| (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) <= r * r).count()
            })?;
            Some(at(best.0, best.1))
        }
        Target::DownedAlly => {
            let map = field.map;
            let p = field
                .players
                .iter()
                .filter(|p| {
                    p.map == map
                        && p.profile.user_id != caster.profile.user_id
                        && p.fighter.as_ref().is_some_and(|f| matches!(f.state, super::fight::Vital::Downed { .. }))
                        && (p.x - px).powi(2) + (p.y - py).powi(2) <= (range + BODY).powi(2)
                })
                .min_by(|a, b| ((a.x - px).powi(2) + (a.y - py).powi(2)).total_cmp(&((b.x - px).powi(2) + (b.y - py).powi(2))))?;
            Some(ally(p))
        }
    }
}
