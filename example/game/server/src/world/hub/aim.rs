//! Target archetypes (COMBAT.md §4.3): where a skill goes when it is
//! pressed. Monster lookups go through the horde's spatial index, so their
//! cost follows the skill's range, not the horde size; allies are at most a
//! party.

use super::area::Area;
use super::fight::Fighter;
use super::{HubPlayer, MapIdx};
use crate::world::combat::skills::{Effect, MoveMode, Payload, Shape, SkillDef, Target, TargetKind};
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
/// A plant counts as pointed at within this of the aim point (cells).
const PLANT_PICK: f32 = 1.0;

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

/// The plant a player means to cut (COMBAT.md §4.10), for a basic attack
/// with no monster in reach: the one under the cursor (`aim`, 1/8 m from
/// the player), or with no cursor (touch, a resting gamepad) the nearest.
/// Automatic targeting never picks plants otherwise.
pub(super) fn aimed_plant(horde: &Horde, px: f32, py: f32, aim: [i8; 2], range: f32) -> Option<u16> {
    let not_plant = |s: u16| !horde.is_plant(s);
    if aim == [0, 0] {
        return horde.nearest_to(px, py, range, not_plant).map(|(s, _)| s);
    }
    let (ax, ay) = (px + aim[0] as f32 * AIM_UNIT, py + aim[1] as f32 * AIM_UNIT);
    let (s, _) = horde.nearest_to(ax, ay, PLANT_PICK, not_plant)?;
    let i = s as usize;
    let body = horde.catalog().get(horde.store().kind[i]).radius;
    let gap = (horde.store().x[i] - px).hypot(horde.store().y[i] - py) - body;
    (gap <= range).then_some(s)
}

impl Field<'_> {
    /// Live monsters whose body is within `range` of a point: `(slot, x, y)`.
    /// Plants are left out: skills aim at monsters (an area still cuts the
    /// plants it covers).
    pub fn monsters_near(&self, x: f32, y: f32, range: f32) -> Vec<(u16, f32, f32)> {
        let mut slots = Vec::new();
        Area::Circle { x, y, r: range }.monsters(self.horde, &mut slots);
        let store = self.horde.store();
        slots.into_iter().filter(|&s| !store.is_plant(s)).map(|s| (s, store.x[s as usize], store.y[s as usize])).collect()
    }

    fn tag(&self, slot: u16) -> (u16, u8) {
        (slot, self.horde.store().generation[slot as usize])
    }

    pub fn nearest(&self, x: f32, y: f32, range: f32) -> Option<(u16, f32, f32)> {
        let (s, _) = self.horde.nearest_foe(x, y, range)?;
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
            let n = buf.iter().filter(|&&s| !self.horde.is_plant(s)).count();
            if n > best.2 {
                best = (cx, cy, n);
            }
        }
        // Centre on what the best candidate covers, kept inside the range.
        buf.clear();
        Area::Circle { x: best.0, y: best.1, r: radius }.monsters(self.horde, &mut buf);
        let store = self.horde.store();
        buf.retain(|&s| !store.is_plant(s));
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
        buf.iter().filter(|&&s| !self.horde.is_plant(s)).count()
    }
}

/// A group skill on monster `m`: centred on what crowds around it.
fn group_on(field: &Field<'_>, def: &SkillDef, slv: u8, m: (u16, f32, f32), at: impl Fn(f32, f32) -> Aimed) -> Aimed {
    let group = field.monsters_near(m.1, m.2, reach_of(def, slv));
    let n = group.len().max(1) as f32;
    let (cx, cy) = (group.iter().map(|g| g.1).sum::<f32>() / n, group.iter().map(|g| g.2).sum::<f32>() / n);
    Aimed { monster: Some(field.tag(m.0)), ..at(cx, cy) }
}

/// It strikes monsters at once (a nova, a taunt), rather than laying
/// ground that waits for them (an aura, a sanctum).
fn strikes_now(def: &SkillDef, slv: u8) -> bool {
    def.effects
        .iter()
        .filter_map(Effect::act)
        .any(|a| a.payload.hostile() && !matches!(a.payload, Payload::Barrier) && !a.timing.lasting(slv) && a.timing.trigger <= 0.0)
}

/// A buff with at most this many seconds left counts as gone: auto-cast
/// renews it on whoever is about to lose it.
pub(super) const REFRESH_SECS: f32 = 3.0;
/// Auto-cast heals an ally only below this share of their HP (a scratch
/// is not worth the SP).
const AUTO_HEAL_BELOW: f32 = 0.9;

/// How a press asks for its target (`Press::default()`: a plain press
/// with automatic targeting, as pickup bonuses cast).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Press {
    /// The skill's index in the book: the buff it leaves is known by it.
    pub skill: Option<u16>,
    /// The tick of the press.
    pub now: u32,
    /// Ticks of [`REFRESH_SECS`].
    pub refresh: u32,
    /// An auto-cast slot: only a target that needs the skill (an ally
    /// without its buff, a real wound), else no cast and nothing spent.
    pub auto: bool,
    /// Manual aiming with a pointer: enemy skills pick the monster nearest
    /// the pointer, ground skills land on it.
    pub manual: bool,
}

/// Picks where a skill goes; `None` = no valid target, nothing is spent.
pub(super) fn aim_skill(field: &Field<'_>, caster: &HubPlayer, def: &SkillDef, slv: u8) -> Option<Aimed> {
    aim_press(field, caster, def, slv, &Press::default())
}

/// [`aim_skill`] for a press: auto-cast and manual aiming (COMBAT.md §4.3).
pub(super) fn aim_press(field: &Field<'_>, caster: &HubPlayer, def: &SkillDef, slv: u8, press: &Press) -> Option<Aimed> {
    let (px, py) = (caster.x, caster.y);
    let aim = (caster.aim[0] != 0 || caster.aim[1] != 0)
        .then(|| (caster.aim[0] as f32 * AIM_UNIT, caster.aim[1] as f32 * AIM_UNIT));
    // Where the pointer is on the ground, when the player aims by hand.
    let pointer = aim.filter(|_| press.manual).map(|(ax, ay)| (px + ax, py + ay));
    let range = def.range;
    let base = Aimed { x: px, y: py, ox: px, oy: py, dir: facing_dir(caster.facing), ally: None, monster: None };
    let at = |x: f32, y: f32| Aimed { x, y, dir: norm(x - px, y - py).unwrap_or(base.dir), ..base };
    let on = |m: (u16, f32, f32)| Aimed { monster: Some(field.tag(m.0)), ..at(m.1, m.2) };
    let ally = |p: &HubPlayer| Aimed { ally: Some(p.profile.user_id), ..at(p.x, p.y) };
    // Who still wants the buff this skill leaves (everyone, if it leaves none).
    let timed = press.skill.filter(|_| def.timed_buff(slv));
    let needs = |f: &Fighter| timed.is_none_or(|s| f.buff_left(s, press.now) <= press.refresh);
    let d2 = |p: &HubPlayer| (p.x - px).powi(2) + (p.y - py).powi(2);

    // Travel goes where the player walks, standing still where they point;
    // with neither (a resting gamepad, touch) the archetype decides below.
    if def.travels()
        && let Some(dir) = norm(caster.dir.0, caster.dir.1).or_else(|| aim.and_then(|(ax, ay)| norm(ax, ay)))
    {
        return Some(match def.target {
            Target::Escape => Aimed { dir, ..base },
            _ => Aimed { x: px + dir.0 * range.max(1.0), y: py + dir.1 * range.max(1.0), dir, ..base },
        });
    }
    // Manual aiming: the monster nearest the pointer, or the pointer itself.
    if let Some((qx, qy)) = pointer {
        match def.target.kind() {
            TargetKind::Enemy => {
                let q2 = |m: &(u16, f32, f32)| (m.1 - qx).powi(2) + (m.2 - qy).powi(2);
                let m = field.monsters_near(px, py, range).into_iter().min_by(|a, b| q2(a).total_cmp(&q2(b)))?;
                return Some(match def.target {
                    Target::NearestGroup => group_on(field, def, slv, m, at),
                    _ => on(m),
                });
            }
            TargetKind::Ground => {
                let (dx, dy) = (qx - px, qy - py);
                let d = (dx * dx + dy * dy).sqrt();
                let k = if d > range && d > 0.0 { range / d } else { 1.0 };
                return Some(at(px + dx * k, py + dy * k));
            }
            _ => {}
        }
    }
    match def.target {
        // A nova or a taunt around the caster waits for something to hit.
        Target::Caster if strikes_now(def, slv) => field.nearest(px, py, reach_of(def, slv)).map(|_| base),
        // Auto-cast keeps a self buff up, never recasts it early.
        Target::Caster if press.auto && caster.fighter.as_ref().is_some_and(|f| !needs(f)) => None,
        Target::Caster => Some(base),
        Target::Nearest => field.nearest(px, py, range).map(on),
        Target::Strongest => field.strongest(px, py, range).map(on),
        Target::NearestGroup => {
            let first = field.nearest(px, py, range)?;
            Some(group_on(field, def, slv, first, at))
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
        // Standing still with no pointer (travel otherwise returned above).
        Target::Escape => {
            let slide = def.effects.iter().any(|e| matches!(e, Effect::Move { mode: MoveMode::Slide, .. }));
            let near = field.monsters_near(px, py, ESCAPE_SCAN);
            let away = (!near.is_empty()).then(|| {
                let n = near.len() as f32;
                let (cx, cy) = (near.iter().map(|m| m.1).sum::<f32>() / n, near.iter().map(|m| m.2).sum::<f32>() / n);
                norm(px - cx, py - cy)
            });
            let dir = away.flatten().unwrap_or_else(|| {
                let f = base.dir;
                if slide { (-f.0, -f.1) } else { f }
            });
            Some(Aimed { dir, ..base })
        }
        // The nearest member (the caster first) who lacks the buff or is
        // about to lose it. Nobody: auto-cast waits, a press renews it
        // around the caster.
        Target::Allies => {
            let pick = field.allies(px, py, range).filter(|(_, f)| needs(f)).min_by(|a, b| d2(a.0).total_cmp(&d2(b.0)));
            match pick {
                Some((p, _)) => Some(ally(p)),
                None if press.auto => None,
                None => Some(Aimed { ally: Some(caster.profile.user_id), ..base }),
            }
        }
        // Lowest HP share, ties to the nearest; auto-cast leaves scratches.
        Target::MostWoundedAlly => {
            let share = |f: &Fighter| f.hp as f32 / f.max_hp.max(1) as f32;
            let (p, _) = field
                .allies(px, py, range)
                .filter(|(_, f)| f.hp < f.max_hp && (!press.auto || share(f) < AUTO_HEAL_BELOW))
                .min_by(|a, b| share(a.1).total_cmp(&share(b.1)).then(d2(a.0).total_cmp(&d2(b.0))))?;
            Some(ally(p))
        }
        // The most crowded ally still without the buff; auto-cast only
        // when monsters crowd someone.
        Target::MostThreatenedAlly => {
            let crowded: Vec<(&HubPlayer, bool, usize)> =
                field.allies(px, py, range).map(|(p, f)| (p, needs(f), field.threat(p))).collect();
            let most = |need: bool| {
                crowded
                    .iter()
                    .filter(|c| c.1 || !need)
                    .max_by(|a, b| a.2.cmp(&b.2).then(d2(b.0).total_cmp(&d2(a.0))))
                    .map(|c| (c.0, c.2))
            };
            let (p, n) = match most(true) {
                Some(found) => found,
                None if press.auto => return None,
                None => most(false)?,
            };
            (!press.auto || n > 0).then(|| ally(p))
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
