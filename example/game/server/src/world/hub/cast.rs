//! Skills on a stage (COMBAT.md §4.1–4.3): slot presses, the rooted
//! wind-up, target archetypes and effects. What a skill does is data
//! ([`SkillBook`]); this file knows the generic effect kinds.
//!
//! Damage from fixed dice is a saving throw for half (`d20 + save` against
//! the caster's DC), so bolts and storms never whiff; skills that swing the
//! weapon roll to hit like the basic attack.

use super::fight::Fighter;
use super::{facing_of, HubEvent, HubInstance, HubPlayer, MapIdx, Outgoing};
use crate::world::combat::rules::{attack_roll, saving_throw, AttackOutcome};
use crate::world::combat::skills::{DamageFx, Effect, EffectDice, HealFx, MoveMode, SaveFx, Shape, SkillBook, SkillDef, Target};
use crate::world::combat::{cantrip_k, save_dc, Condition, Damage, DamageType, Dice, Rng};
use crate::world::horde::Horde;
use crate::world::map::WalkGrid;
use crate::world::protocol::{hit, HitEvent, ServerMsg, MONSTER_ENTITY_BASE};

/// Rooted after a cast lands, before the basic attack resumes.
const RECOVERY_SECS: f32 = 0.15;
/// Monsters this close decide where an escape goes (cells; 5 m).
const ESCAPE_SCAN: f32 = 10.0;
/// `densest` tries at most this many candidate centres.
const DENSEST_CANDIDATES: usize = 32;
/// One aim unit on the wire is 1/8 m = 0.25 cells.
const AIM_UNIT: f32 = 0.25;
/// Step of a slide or blink when checking the ground (cells).
const MOVE_STEP: f32 = 0.25;
/// Player body radius (cells), as the basic attack uses it.
const BODY: f32 = 0.4;

/// Where a skill lands, decided when it starts (the telegraph is honest).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Aimed {
    pub x: f32,
    pub y: f32,
    /// Unit direction from the caster (lines, arcs, moves).
    pub dir: (f32, f32),
    /// Ally target (user id) for heals.
    pub ally: Option<u32>,
}

/// A wind-up in progress.
#[derive(Clone, Copy, Debug)]
pub(super) struct Casting {
    pub skill: u16,
    pub level: u8,
    pub ticks: u32,
    pub aimed: Aimed,
}

/// One hit of a cast skill, due at `at` (bolts fall one by one, storms tick).
#[derive(Clone, Debug)]
pub(super) struct PendingHit {
    pub at: u32,
    pub map: MapIdx,
    pub kind: PendingKind,
}

#[derive(Clone, Debug)]
pub(super) enum PendingKind {
    Damage {
        dice: Dice,
        extra: Option<Dice>,
        kind: DamageType,
        /// To-hit bonus when the skill swings the weapon; `None` = save for half.
        to_hit: Option<i32>,
        dc: i32,
        area: Area,
        /// A `spread` hit follows its monster (slot) instead of the point.
        follow: Option<u16>,
        /// A failed save also leaves this condition for so many ticks.
        save: Option<(Condition, u16)>,
        /// The caster's cantrip multiplier (shattering ice).
        k: u32,
    },
    /// Save or the condition, no damage.
    Condition { area: Area, dc: i32, condition: Condition, ticks: u16 },
    Heal {
        dice: Dice,
        x: f32,
        y: f32,
        radius: f32,
        /// Always healed (the chosen ally), even outside the radius.
        ally: Option<u32>,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Area {
    Circle { x: f32, y: f32, radius: f32 },
    Line { x: f32, y: f32, dir: (f32, f32), length: f32, half_width: f32 },
    Arc { x: f32, y: f32, dir: (f32, f32), radius: f32, arc_cos: f32 },
}

impl Area {
    fn contains(&self, mx: f32, my: f32, r: f32) -> bool {
        match *self {
            Area::Circle { x, y, radius } => {
                let (dx, dy) = (mx - x, my - y);
                let reach = radius + r;
                dx * dx + dy * dy <= reach * reach
            }
            Area::Line { x, y, dir, length, half_width } => {
                let (dx, dy) = (mx - x, my - y);
                let t = dx * dir.0 + dy * dir.1;
                let side = (dx * dir.1 - dy * dir.0).abs();
                t >= -r && t <= length + r && side <= half_width + r
            }
            Area::Arc { x, y, dir, radius, arc_cos } => {
                let (dx, dy) = (mx - x, my - y);
                let d = (dx * dx + dy * dy).sqrt();
                d <= radius + r && (d <= r + BODY || dx * dir.0 + dy * dir.1 >= arc_cos * d)
            }
        }
    }

    fn moved_to(self, nx: f32, ny: f32) -> Self {
        match self {
            Area::Circle { radius, .. } => Area::Circle { x: nx, y: ny, radius },
            other => other,
        }
    }
}

/// Ticks of a condition at skill level `slv`.
fn condition_ticks(save: &SaveFx, slv: u8, hz: f32) -> u16 {
    (save.secs.at(slv).max(0.0) * hz).round().min(u16::MAX as f32) as u16
}

fn norm(x: f32, y: f32) -> Option<(f32, f32)> {
    let l = (x * x + y * y).sqrt();
    (l > 1e-4).then(|| (x / l, y / l))
}

fn facing_dir(facing: u8) -> (f32, f32) {
    // Facing 0 = south (−y), counter-clockwise in eighths, as `facing_of`.
    let a = facing as f32 * std::f32::consts::FRAC_PI_4;
    (a.sin(), -a.cos())
}

/// The radius a skill's first area effect covers (for picking centres).
fn reach_of(def: &SkillDef) -> f32 {
    def.effects
        .iter()
        .find_map(|e| match e {
            Effect::Damage(d) => Some(d.radius.max(d.width / 2.0) + d.scatter * 0.5),
            Effect::Heal(h) => Some(h.radius),
            _ => None,
        })
        .unwrap_or(2.0)
        .max(1.0)
}

/// Read-only view of the stage used by target selection.
struct Field<'a> {
    horde: &'a Horde,
    players: &'a [HubPlayer],
    map: MapIdx,
}

impl Field<'_> {
    /// Live monsters within `range` (+ body) of a point: `(slot, x, y, d²)`.
    fn monsters_near(&self, x: f32, y: f32, range: f32) -> Vec<(u16, f32, f32, f32)> {
        let store = self.horde.store();
        let catalog = self.horde.catalog();
        let mut out = Vec::new();
        for &s in store.alive() {
            let i = s as usize;
            if store.hp[i] <= 0 {
                continue;
            }
            let reach = range + catalog.get(store.kind[i]).radius;
            let (dx, dy) = (store.x[i] - x, store.y[i] - y);
            if dx.abs() > reach || dy.abs() > reach {
                continue;
            }
            let d2 = dx * dx + dy * dy;
            if d2 <= reach * reach {
                out.push((s, store.x[i], store.y[i], d2));
            }
        }
        out
    }

    fn nearest(&self, x: f32, y: f32, range: f32) -> Option<(u16, f32, f32)> {
        self.monsters_near(x, y, range)
            .into_iter()
            .min_by(|a, b| a.3.total_cmp(&b.3))
            .map(|(s, mx, my, _)| (s, mx, my))
    }

    /// Centre covering the most monsters within `radius`, among monsters in
    /// range of the caster (COMBAT.md §4.3 `densest`).
    fn densest(&self, x: f32, y: f32, range: f32, radius: f32) -> Option<(f32, f32)> {
        let near = self.monsters_near(x, y, range);
        if near.is_empty() {
            return None;
        }
        let stride = near.len().div_ceil(DENSEST_CANDIDATES);
        let pool = self.monsters_near(x, y, range + radius);
        let (mut best, mut best_n) = ((near[0].1, near[0].2), 0usize);
        for &(_, cx, cy, _) in near.iter().step_by(stride.max(1)) {
            let n = pool.iter().filter(|m| (m.1 - cx).powi(2) + (m.2 - cy).powi(2) <= radius * radius).count();
            if n > best_n {
                (best, best_n) = ((cx, cy), n);
            }
        }
        // Centre on what the best candidate covers, kept inside the range.
        let covered: Vec<_> =
            pool.iter().filter(|m| (m.1 - best.0).powi(2) + (m.2 - best.1).powi(2) <= radius * radius).collect();
        let n = covered.len().max(1) as f32;
        let (mut cx, mut cy) = (covered.iter().map(|m| m.1).sum::<f32>() / n, covered.iter().map(|m| m.2).sum::<f32>() / n);
        if covered.is_empty() {
            (cx, cy) = best;
        }
        let (dx, dy) = (cx - x, cy - y);
        let d = (dx * dx + dy * dy).sqrt();
        if d > range && d > 0.0 {
            (cx, cy) = (x + dx / d * range, y + dy / d * range);
        }
        Some((cx, cy))
    }

    fn strongest(&self, x: f32, y: f32, range: f32) -> Option<(f32, f32)> {
        let store = self.horde.store();
        self.monsters_near(x, y, range)
            .into_iter()
            .max_by(|a, b| store.hp[a.0 as usize].cmp(&store.hp[b.0 as usize]).then(b.3.total_cmp(&a.3)))
            .map(|(_, mx, my, _)| (mx, my))
    }

    fn allies(&self, x: f32, y: f32, range: f32) -> impl Iterator<Item = (&HubPlayer, &Fighter)> + '_ {
        let map = self.map;
        self.players.iter().filter_map(move |p| {
            let f = p.fighter.as_ref()?;
            (p.map == map && f.is_up() && (p.x - x).powi(2) + (p.y - y).powi(2) <= (range + BODY).powi(2)).then_some((p, f))
        })
    }
}

/// Picks where a skill goes; `None` = no valid target, nothing is spent.
fn aim_skill(field: &Field<'_>, caster: &HubPlayer, def: &SkillDef) -> Option<Aimed> {
    let (px, py) = (caster.x, caster.y);
    let aim = (caster.aim[0] != 0 || caster.aim[1] != 0)
        .then(|| (caster.aim[0] as f32 * AIM_UNIT, caster.aim[1] as f32 * AIM_UNIT));
    let range = def.range;
    let at = |x: f32, y: f32| {
        let dir = norm(x - px, y - py).unwrap_or_else(|| facing_dir(caster.facing));
        Aimed { x, y, dir, ally: None }
    };
    match def.target {
        Target::Caster => Some(at(px, py)),
        Target::Nearest => field.nearest(px, py, range).map(|(_, x, y)| at(x, y)),
        Target::NearestGroup => {
            let (_, nx, ny) = field.nearest(px, py, range)?;
            let r = reach_of(def);
            let group = field.monsters_near(nx, ny, r);
            let n = group.len().max(1) as f32;
            Some(at(group.iter().map(|m| m.1).sum::<f32>() / n, group.iter().map(|m| m.2).sum::<f32>() / n))
        }
        Target::Densest | Target::Spread => {
            field.densest(px, py, range, reach_of(def)).map(|(x, y)| at(x, y))
        }
        Target::Strongest => field.strongest(px, py, range).map(|(x, y)| at(x, y)),
        Target::Line | Target::Front | Target::Aim => {
            let dir = match aim.and_then(|(ax, ay)| norm(ax, ay)) {
                Some(d) => d,
                None => {
                    let (_, nx, ny) = field.nearest(px, py, range.max(reach_of(def)))?;
                    norm(nx - px, ny - py).unwrap_or_else(|| facing_dir(caster.facing))
                }
            };
            let (x, y) = match def.target {
                Target::Line => (px + dir.0 * range, py + dir.1 * range),
                // The point only tells the client the direction.
                Target::Front => (px + dir.0 * range.max(1.0), py + dir.1 * range.max(1.0)),
                _ => {
                    let d = aim.map_or(range * 0.5, |(ax, ay)| (ax * ax + ay * ay).sqrt().min(range));
                    (px + dir.0 * d, py + dir.1 * d)
                }
            };
            Some(Aimed { x, y, dir, ally: None })
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
                (Some(a), _) => if slide { (-a.0, -a.1) } else { a },
                (None, Some(away)) => away,
                (None, None) => {
                    let f = facing_dir(caster.facing);
                    if slide { (-f.0, -f.1) } else { f }
                }
            };
            Some(Aimed { x: px, y: py, dir, ally: None })
        }
        Target::MostWoundedAlly => {
            let (p, _) = field
                .allies(px, py, range)
                .filter(|(_, f)| f.hp < f.max_hp)
                .min_by(|a, b| (a.1.hp as f32 / a.1.max_hp as f32).total_cmp(&(b.1.hp as f32 / b.1.max_hp as f32)))?;
            Some(Aimed { x: p.x, y: p.y, dir: norm(p.x - px, p.y - py).unwrap_or((0.0, 1.0)), ally: Some(p.profile.user_id) })
        }
        Target::DensestAllies => {
            let r = reach_of(def);
            let allies: Vec<(f32, f32)> = field.allies(px, py, range).map(|(p, _)| (p.x, p.y)).collect();
            let best = allies.iter().max_by_key(|a| {
                allies.iter().filter(|b| (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) <= r * r).count()
            })?;
            Some(at(best.0, best.1))
        }
        // Not cast yet (buffs, wards, revives).
        Target::Allies | Target::MostThreatenedAlly | Target::DownedAlly => None,
    }
}

/// Dice of a fixed-dice effect at skill level `slv` and character level `cl`.
fn scaled(dice: Dice, scale_k: bool, mult: f32, bonus: f32, cl: u32) -> Dice {
    let mut d = if scale_k { dice.times(cantrip_k(cl)) } else { dice };
    d.count = ((d.count as f32) * mult).round().max(1.0) as u32;
    d.bonus += bonus.round() as i32;
    d
}

/// Turns one cast into hits (due now or later), heals and moves.
#[allow(clippy::too_many_arguments)]
fn land(
    def: &SkillDef,
    slv: u8,
    caster: &mut HubPlayer,
    aimed: Aimed,
    field_targets: &[(u16, f32, f32)],
    walk: &WalkGrid,
    rng: &mut Rng,
    now: u32,
    hz: f32,
    pending: &mut Vec<PendingHit>,
) {
    let Some(f) = caster.fighter.as_ref() else { return };
    let (cl, class, weapon) = (f.level, f.class, f.weapon);
    let dc = save_dc(cl);
    let map = caster.map;
    for effect in &def.effects {
        match effect {
            Effect::Damage(fx) => {
                let DamageFx { dice, extra, scale_k, mult, bonus, kind, count, every, shape, radius, width, arc_cos, scatter, spread, save } =
                    fx;
                let save = save.as_ref().map(|sv| (sv.condition, condition_ticks(sv, slv, hz)));
                let (base, kind, to_hit) = match dice {
                    EffectDice::Fixed(d) => (scaled(*d, *scale_k, mult.at(slv), bonus.at(slv), cl), *kind, None),
                    EffectDice::Weapon => {
                        let (d, k) = weapon.damage(class, cl);
                        (Dice { bonus: d.bonus + bonus.at(slv).round() as i32, ..d }, k, Some(weapon.to_hit(cl)))
                    }
                };
                let extra = extra.and_then(|e| {
                    let n = (e.count as f32 * mult.at(slv)).round() as u32;
                    (n > 0).then_some(Dice { count: n, ..e })
                });
                let n = count.at(slv).round().max(1.0) as u32;
                for i in 0..n {
                    let at = now + (i as f32 * every * hz).round() as u32;
                    let (mut x, mut y) = (aimed.x, aimed.y);
                    if *scatter > 0.0 {
                        let a = rng.die(360) as f32 * std::f32::consts::TAU / 360.0;
                        let r = scatter * (rng.die(1000) as f32 / 1000.0).sqrt();
                        (x, y) = (x + a.cos() * r, y + a.sin() * r);
                    }
                    let follow = (*spread && !field_targets.is_empty()).then(|| {
                        let t = field_targets[i as usize % field_targets.len()];
                        (x, y) = (t.1, t.2);
                        t.0
                    });
                    let area = match shape {
                        Shape::Circle => Area::Circle { x, y, radius: *radius },
                        Shape::Line => Area::Line {
                            x: caster.x,
                            y: caster.y,
                            dir: aimed.dir,
                            length: def.range,
                            half_width: width / 2.0,
                        },
                        Shape::Arc => Area::Arc { x: caster.x, y: caster.y, dir: aimed.dir, radius: *radius, arc_cos: *arc_cos },
                    };
                    pending.push(PendingHit {
                        at,
                        map,
                        kind: PendingKind::Damage { dice: base, extra, kind, to_hit, dc, area, follow, save, k: cantrip_k(cl) },
                    });
                }
            }
            Effect::Heal(HealFx { dice, scale_k, mult, radius, count, every }) => {
                let d = scaled(*dice, *scale_k, mult.at(slv), 0.0, cl);
                let n = count.at(slv).round().max(1.0) as u32;
                let ally = aimed.ally.or_else(|| (def.target == Target::Caster).then_some(caster.profile.user_id));
                for i in 0..n {
                    pending.push(PendingHit {
                        at: now + (i as f32 * every * hz).round() as u32,
                        map,
                        kind: PendingKind::Heal { dice: d, x: aimed.x, y: aimed.y, radius: *radius, ally },
                    });
                }
            }
            Effect::Move { distance, .. } => {
                let total = distance.at(slv).max(0.0);
                let (mut x, mut y) = (caster.x, caster.y);
                let mut moved = 0.0;
                while moved + MOVE_STEP <= total {
                    let (nx, ny) = (x + aimed.dir.0 * MOVE_STEP, y + aimed.dir.1 * MOVE_STEP);
                    if !walk.is_walkable(nx, ny) {
                        break;
                    }
                    (x, y, moved) = (nx, ny, moved + MOVE_STEP);
                }
                (caster.x, caster.y) = (x, y);
            }
            Effect::Condition { save, radius } => pending.push(PendingHit {
                at: now,
                map,
                kind: PendingKind::Condition {
                    area: Area::Circle { x: aimed.x, y: aimed.y, radius: *radius },
                    dc,
                    condition: save.condition,
                    ticks: condition_ticks(save, slv, hz),
                },
            }),
            Effect::SpRegen(_) | Effect::Pending(_) => {}
        }
    }
}

impl HubInstance {
    /// Slot presses, wind-ups and due hits on the horde map. Runs inside
    /// [`HubInstance::fight`], before the basic attacks; kills land in
    /// `kills` like the basic attack's.
    pub(super) fn cast_step(&mut self, out: &mut Outgoing, hits: &mut Vec<HitEvent>, kills: &mut Vec<u16>) {
        let Some(horde) = self.horde.as_mut() else { return };
        let map = horde.map();
        let hz = self.config.tick_hz as f32;
        let now = self.tick;
        let book = std::sync::Arc::clone(&self.skills);
        let mut casts: Vec<ServerMsg> = Vec::new();

        for idx in 0..self.players.len() {
            if self.players[idx].map != map {
                continue;
            }
            let pressed = std::mem::take(&mut self.players[idx].pressed);
            let moving = self.players[idx].dir != (0.0, 0.0);
            let entity = self.players[idx].profile.character_id;
            let Some(f) = self.players[idx].fighter.as_mut() else { continue };
            for cd in f.cooldowns.iter_mut().filter(|c| **c > 0) {
                *cd -= 1;
            }
            f.recovery = f.recovery.saturating_sub(1);
            if !f.is_up() {
                if f.casting.take().is_some() {
                    casts.push(cancel(entity));
                }
                continue;
            }

            // A wind-up: movement cancels it (nothing spent), else it counts down.
            if let Some(mut c) = f.casting {
                if moving {
                    f.casting = None;
                    casts.push(cancel(entity));
                } else if c.ticks > 1 {
                    c.ticks -= 1;
                    f.casting = Some(c);
                } else {
                    f.casting = None;
                    let def = book.get(c.skill);
                    let cost = def.sp_cost(c.level);
                    if f.sp >= cost {
                        f.sp -= cost;
                        f.cooldowns[c.skill as usize] = (def.cooldown_secs(c.level) * hz).round() as u32;
                        f.recovery = (RECOVERY_SECS * hz).round() as u32;
                        f.quiet = 0;
                        let spread = spread_targets(horde, def, c.level, c.aimed);
                        let walk = &self.world.map(map).walk;
                        land(def, c.level, &mut self.players[idx], c.aimed, &spread, walk, &mut self.dice, now, hz, &mut self.pending);
                    } else {
                        casts.push(cancel(entity));
                    }
                }
            }

            // New presses, lowest slot first; one skill per tick.
            let Some(f) = self.players[idx].fighter.as_ref() else { continue };
            for slot in (0..16).filter(|s| pressed & (1 << s) != 0) {
                let Some(skill) = f.hotbar.get(slot).copied().flatten() else { continue };
                let def = book.get(skill);
                let level = f.skills.get(skill as usize).copied().unwrap_or(0);
                let busy = f.casting.is_some() && !def.mobile;
                let rooted_ok = def.mobile || !moving;
                if level == 0
                    || !def.ready()
                    || def.passive
                    || busy
                    || !rooted_ok
                    || f.cooldowns[skill as usize] > 0
                    || f.sp < def.sp_cost(level)
                {
                    continue;
                }
                let field = Field { horde, players: &self.players, map };
                let Some(aimed) = aim_skill(&field, &self.players[idx], def) else { continue };
                let cast_ticks = (def.cast_secs(level) * hz).round() as u32;
                let cooldown_ms = (def.cooldown_secs(level) * 1000.0).round() as u32;
                let p = &mut self.players[idx];
                if def.target != Target::Escape {
                    let d = norm(aimed.x - p.x, aimed.y - p.y).unwrap_or(aimed.dir);
                    p.facing = facing_of(d.0, d.1);
                }
                casts.push(ServerMsg::Cast {
                    caster: entity,
                    skill: def.id.clone(),
                    x: aimed.x,
                    y: aimed.y,
                    cast_ms: (cast_ticks as f32 * 1000.0 / hz) as u32,
                    cooldown_ms,
                });
                let f = p.fighter.as_mut().expect("checked above");
                if cast_ticks == 0 {
                    // Instant (and mobile skills cancel a wind-up).
                    if f.casting.take().is_some() {
                        casts.push(cancel(entity));
                    }
                    f.sp -= def.sp_cost(level);
                    f.cooldowns[skill as usize] = (def.cooldown_secs(level) * hz).round() as u32;
                    if !def.mobile {
                        f.recovery = (RECOVERY_SECS * hz).round() as u32;
                    }
                    f.quiet = 0;
                    let spread = spread_targets(horde, def, level, aimed);
                    let walk = &self.world.map(map).walk;
                    land(def, level, &mut self.players[idx], aimed, &spread, walk, &mut self.dice, now, hz, &mut self.pending);
                } else {
                    f.casting = Some(Casting { skill, level, ticks: cast_ticks, aimed });
                }
                break;
            }
        }

        // Due hits.
        let mut due = Vec::new();
        self.pending.retain(|h| {
            if h.at <= now {
                due.push(h.clone());
                false
            } else {
                true
            }
        });
        for h in due.into_iter().filter(|h| h.map == map) {
            match h.kind {
                PendingKind::Condition { area, dc, condition, ticks } => {
                    for s in inside(horde, area) {
                        if !resist(horde, &mut self.dice, s, dc, true) {
                            horde.store_mut().afflict(s, condition, ticks);
                        }
                    }
                }
                PendingKind::Damage { dice, extra, kind, to_hit, dc, area, follow, save, k } => {
                    let area = match follow {
                        Some(s) if horde.store().is_alive(s) && horde.store().hp[s as usize] > 0 => {
                            area.moved_to(horde.store().x[s as usize], horde.store().y[s as usize])
                        }
                        Some(_) => continue,
                        None => area,
                    };
                    let blow = Blow { damage: Damage::new(dice, kind), extra, to_hit, dc, save, k };
                    strike(horde, &mut self.dice, area, blow, hits, kills);
                }
                PendingKind::Heal { dice, x, y, radius, ally } => {
                    for p in self.players.iter_mut().filter(|p| p.map == map) {
                        let uid = p.profile.user_id;
                        let id = p.profile.character_id;
                        let inside = (p.x - x).powi(2) + (p.y - y).powi(2) <= (radius + BODY).powi(2);
                        let Some(f) = p.fighter.as_mut() else { continue };
                        if !f.is_up() || !(inside && radius > 0.0 || ally == Some(uid)) {
                            continue;
                        }
                        let amount = dice.roll(&mut self.dice, false).max(0).min(f.max_hp - f.hp);
                        f.hp += amount;
                        hits.push(HitEvent { target: id, amount: amount as u32, flags: hit::HEAL | (DamageType::Radiant as u8) << 4 });
                    }
                }
            }
        }

        let recipients: Vec<u32> = self.players.iter().filter(|p| p.map == map).map(|p| p.profile.user_id).collect();
        for msg in casts {
            for &uid in &recipients {
                out.push((uid, HubEvent::Control(msg.clone())));
            }
        }
    }
}

fn cancel(entity: u32) -> ServerMsg {
    ServerMsg::Cast { caster: entity, skill: String::new(), x: 0.0, y: 0.0, cast_ms: 0, cooldown_ms: 0 }
}

/// Distinct monsters for a `spread` effect: nearest to the aimed point first.
fn spread_targets(horde: &Horde, def: &SkillDef, slv: u8, aimed: Aimed) -> Vec<(u16, f32, f32)> {
    let Some(n) = def.effects.iter().find_map(|e| match e {
        Effect::Damage(d) if d.spread => Some(d.count.at(slv).round().max(1.0)),
        _ => None,
    }) else {
        return Vec::new();
    };
    let store = horde.store();
    let mut near: Vec<(u16, f32, f32, f32)> = store
        .alive()
        .iter()
        .filter(|&&s| store.hp[s as usize] > 0)
        .map(|&s| {
            let i = s as usize;
            (s, store.x[i], store.y[i], (store.x[i] - aimed.x).powi(2) + (store.y[i] - aimed.y).powi(2))
        })
        .filter(|m| m.3 <= (def.range * 1.5).powi(2))
        .collect();
    near.sort_by(|a, b| a.3.total_cmp(&b.3));
    near.truncate(n as usize);
    near.into_iter().map(|(s, x, y, _)| (s, x, y)).collect()
}

/// Live monsters inside an area.
fn inside(horde: &Horde, area: Area) -> Vec<u16> {
    let store = horde.store();
    let catalog = horde.catalog();
    store
        .alive()
        .iter()
        .copied()
        .filter(|&s| {
            let i = s as usize;
            store.hp[i] > 0 && area.contains(store.x[i], store.y[i], catalog.get(store.kind[i]).radius)
        })
        .collect()
}

/// A monster's saving throw against `dc`; stunned ones fail. When a
/// condition rides on it, a boss turns a failure into a success while its
/// legendary resistance lasts (it is not spent on plain damage).
fn resist(horde: &mut Horde, rng: &mut Rng, s: u16, dc: i32, condition: bool) -> bool {
    let block = horde.block(s);
    if !horde.store().status_of(s).saves_fail() && saving_throw(rng, block.save, dc, 0) {
        return true;
    }
    let lr = &mut horde.store_mut().legendary[s as usize];
    if condition && *lr > 0 {
        *lr -= 1;
        return true;
    }
    false
}

/// Damage to one monster with its conditions applied (COMBAT.md §2.5): ice
/// makes it water and a physical hit shatters it for +1d6 per `k`; stone
/// halves everything. Returns the amount taken off its HP.
#[allow(clippy::too_many_arguments)]
pub(super) fn wound(
    horde: &mut Horde,
    rng: &mut Rng,
    s: u16,
    damage: Damage,
    extra: Option<Dice>,
    crit: bool,
    saved: bool,
    k: u32,
) -> i32 {
    let status = horde.store().status_of(s);
    let element = status.element(horde.catalog().get(horde.store().kind[s as usize]).element);
    let mut amount = damage.resolve(rng, crit, saved, element, 0);
    if let Some(e) = extra {
        amount += Damage::new(e, damage.kind).resolve(rng, crit, saved, element, 0);
    }
    if status.has(Condition::Frozen) && damage.kind == DamageType::Physical {
        amount += Dice::new(k.max(1), 6, 0).roll(rng, crit);
        horde.store_mut().cure(s, Condition::Frozen);
    }
    if status.resists_all() {
        amount /= 2;
    }
    horde.store_mut().hp[s as usize] -= amount;
    amount
}

/// What one skill hit carries.
#[derive(Clone, Copy)]
struct Blow {
    damage: Damage,
    extra: Option<Dice>,
    /// To-hit bonus when it swings the weapon; `None` = save for half.
    to_hit: Option<i32>,
    dc: i32,
    save: Option<(Condition, u16)>,
    k: u32,
}

/// One hit over an area of the horde.
fn strike(horde: &mut Horde, rng: &mut Rng, area: Area, blow: Blow, hits: &mut Vec<HitEvent>, kills: &mut Vec<u16>) {
    for s in inside(horde, area) {
        let id = MONSTER_ENTITY_BASE + s as u32;
        let (crit, saved) = match blow.to_hit {
            Some(bonus) => {
                let ac = horde.block(s).ac;
                match attack_roll(rng, bonus, ac, horde.store().status_of(s).advantage_against(), 20) {
                    AttackOutcome::Miss => {
                        hits.push(HitEvent { target: id, amount: 0, flags: hit::MISS });
                        continue;
                    }
                    o => (o == AttackOutcome::Critical, false),
                }
            }
            None => (false, resist(horde, rng, s, blow.dc, blow.save.is_some())),
        };
        let amount = wound(horde, rng, s, blow.damage, blow.extra, crit, saved, blow.k);
        let mut flags = (blow.damage.kind as u8) << 4;
        if crit {
            flags |= hit::CRITICAL;
        }
        if horde.store().hp[s as usize] <= 0 {
            flags |= hit::KILL;
            kills.push(s);
        } else if let Some((condition, ticks)) = blow.save {
            // A weapon swing that hit still allows a save against its rider.
            let failed = if blow.to_hit.is_some() { !resist(horde, rng, s, blow.dc, true) } else { !saved };
            if failed {
                horde.store_mut().afflict(s, condition, ticks);
            }
        }
        hits.push(HitEvent { target: id, amount: amount as u32, flags });
    }
}

/// Learned levels, hotbar and passives of a fighter from a loadout.
pub(super) fn equip(f: &mut Fighter, book: &SkillBook, loadout: &crate::world::combat::Loadout) {
    f.skills = vec![0; book.len()];
    f.cooldowns.resize(book.len(), 0);
    f.sp_regen = 0.0;
    for (i, def) in book.iter() {
        let lvl = loadout.level(&def.id).min(def.max_level);
        if lvl == 0 || !def.usable_by(f.class) {
            continue;
        }
        f.skills[i as usize] = lvl;
        for e in &def.effects {
            if let Effect::SpRegen(v) = e {
                f.sp_regen += v.at(lvl) / 100.0;
            }
        }
    }
    for (slot, h) in f.hotbar.iter_mut().enumerate() {
        *h = book.find(loadout.slot(slot)).map(|(i, _)| i).filter(|&i| f.skills[i as usize] > 0);
    }
}
