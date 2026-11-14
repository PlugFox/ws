//! Skills on a stage (COMBAT.md §4.1–4.3): slot presses, the rooted
//! wind-up, and the effects. What a skill does is data ([`SkillBook`]):
//! every cast effect is an *area* (circle, box, line, cone; sizes per
//! level) with a *timing* (at once, repeated, a homing projectile, ground
//! that lasts, a trap, an aura on the caster) and a *payload* (damage,
//! heal, condition, push, buff, ward, barrier, revive). This file knows
//! those generic pieces, not skills.
//!
//! Each effect becomes [`Live`] entries; one list holds what is in flight
//! and what lies on the ground, and every due entry queries the horde's
//! spatial index for its area.
//!
//! Damage from fixed dice is a saving throw for half (`d20 + save` against
//! the caster's DC), so bolts and storms never whiff; skills that swing the
//! weapon roll to hit like the basic attack.

use super::aim::{aim_skill, norm, Aimed, Field, BODY};
use super::area::Area;
use super::fight::{Fighter, Vital};
use super::{facing_of, HubEvent, HubInstance, HubPlayer, MapIdx, Outgoing};
use crate::world::combat::rules::{attack_roll, saving_throw, AttackOutcome};
use crate::world::combat::skills::{
    Act, DamageFx, Effect, EffectDice, Follow as FollowFx, HealFx, MoveMode, Payload, Shape, SkillBook, SkillDef, Target,
};
use crate::world::combat::{cantrip_k, save_dc, Condition, Damage, DamageType, Dice, Mods, Rng};
use crate::world::horde::storage::FLAG_BOSS;
use crate::world::horde::Horde;
use crate::world::map::WalkGrid;
use crate::world::protocol::{hit, look, HitEvent, ServerMsg, Telegraph, MONSTER_ENTITY_BASE};

/// Rooted after a cast lands, before the basic attack resumes.
const RECOVERY_SECS: f32 = 0.15;
/// A channel ends this many ticks after its slot is let go (input jitter).
const CHANNEL_GRACE: u8 = 4;
/// Step of a slide, a blink or a push when checking the ground (cells).
const MOVE_STEP: f32 = 0.25;
/// A trap checks for monsters this often (ticks).
const TRAP_CHECK: u32 = 2;
/// A condition without a save and outside lasting ground holds this long.
const BRIEF_SECS: f32 = 1.0;

/// A wind-up in progress.
#[derive(Clone, Copy, Debug)]
pub(super) struct Casting {
    pub skill: u16,
    pub level: u8,
    pub ticks: u32,
    pub aimed: Aimed,
    /// Id of its first lasting area (the telegraphs sent at the press).
    pub zone: u32,
    /// Hotbar slot it was pressed on (channels last while it is held).
    pub slot: u8,
}

/// A channel in progress: its ground lasts while `slot` is held.
#[derive(Clone, Copy, Debug)]
pub(super) struct Channel {
    pub skill: u16,
    pub slot: u8,
    /// Ticks since the slot was last seen held.
    pub idle: u8,
}

/// What a live area moves with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Follow {
    None,
    /// A player (user id): the caster's aura, a heal on its way to an ally.
    Player(u32),
    /// A monster `(slot, generation)`: a homing hit.
    Monster(u16, u8),
}

/// A trap waiting for a monster.
#[derive(Clone, Copy, Debug)]
pub(super) struct Trap {
    radius: f32,
    fuse_at: Option<u32>,
}

/// An effect in flight or on the ground: acts at `next`, then every
/// `pulse` ticks while `next <= until`.
#[derive(Clone, Debug)]
pub(super) struct Live {
    /// Telegraph id of lasting ground (0 = a one-off strike).
    pub id: u32,
    pub map: MapIdx,
    pub skill: u16,
    /// User id of the caster.
    pub caster: u32,
    pub next: u32,
    pub until: u32,
    pub pulse: u32,
    pub area: Area,
    pub follow: Follow,
    /// No area: only the picked monster or ally.
    pub point: bool,
    pub trap: Option<Trap>,
    /// At most this many monsters (0 = all), nearest the origin first.
    pub limit: u32,
    /// Hops to the nearest monster not hit yet, within `.1` cells.
    pub chain: (u32, f32),
    /// The picked ally, always reached.
    pub ally: Option<u32>,
    pub load: Load,
}

/// A payload resolved at the caster's levels.
#[derive(Clone, Debug)]
pub(super) enum Load {
    Damage { blow: Blow, knock: f32 },
    Heal(Dice),
    /// `dc` `None`: no save.
    Condition { dc: Option<i32>, condition: Condition, ticks: u16, mired: bool },
    Push(f32),
    Buff { mods: Mods, ticks: u32, temp: Option<Dice>, shield: f32 },
    Ward { hits: u32, ranged: bool },
    Barrier,
    Revive(f32),
    Cure,
    Dispel,
}

impl Live {
    /// A ward still holding over a player at `(x, y)`.
    pub fn shields(&self, map: MapIdx, x: f32, y: f32) -> bool {
        matches!(self.load, Load::Ward { hits, ranged: false } if hits > 0) && self.map == map && self.area.contains(x, y, BODY)
    }

    /// A ranged ward (Pneuma) over a player at `(x, y)`.
    pub fn deflects(&self, map: MapIdx, x: f32, y: f32) -> bool {
        matches!(self.load, Load::Ward { ranged: true, .. }) && self.map == map && self.area.contains(x, y, BODY)
    }

    /// Spends one hit of a ward.
    pub fn absorb(&mut self) {
        if let Load::Ward { hits, .. } = &mut self.load {
            *hits = hits.saturating_sub(1);
        }
    }

    fn spent(&self) -> bool {
        matches!(self.load, Load::Ward { hits: 0, .. })
    }
}

pub(super) fn ticks_of(secs: f32, hz: f32) -> u32 {
    (secs.max(0.0) * hz).round() as u32
}

pub(super) fn ticks16(secs: f32, hz: f32) -> u16 {
    ticks_of(secs, hz).min(u16::MAX as u32) as u16
}

/// Dice of a fixed-dice effect at skill level `slv` and character level `cl`.
pub(super) fn scaled(dice: Dice, scale_k: bool, mult: f32, bonus: f32, cl: u32) -> Dice {
    let mut d = if scale_k { dice.times(cantrip_k(cl)) } else { dice };
    d.count = ((d.count as f32) * mult).round().max(1.0) as u32;
    d.bonus += bonus.round() as i32;
    d
}

/// How an effect looks on the ground.
pub(super) fn look_of(act: &Act) -> u8 {
    if act.timing.trigger > 0.0 {
        return look::TRAP;
    }
    match &act.payload {
        Payload::Damage(d) => match d.kind {
            DamageType::Fire => look::FIRE,
            DamageType::Cold => look::COLD,
            DamageType::Lightning => look::LIGHTNING,
            DamageType::Radiant => look::HOLY,
            DamageType::Earth => look::EARTH,
            DamageType::Poison => look::POISON,
            DamageType::Necrotic => look::DARK,
            _ => look::DAMAGE,
        },
        Payload::Heal(_) | Payload::Revive { .. } => look::HEAL,
        Payload::Condition { condition: Condition::Taunted, .. } => look::TAUNT,
        Payload::Condition { save: false, .. } => look::SWAMP,
        Payload::Condition { .. } => look::CURSE,
        Payload::Push(_) => look::DAMAGE,
        Payload::Buff(_) => look::BUFF,
        Payload::Ward { .. } => look::WARD,
        Payload::Barrier | Payload::Dispel => look::BARRIER,
        Payload::Cure => look::HEAL,
    }
}

/// The area of an act for a cast aimed at `aimed`, the caster at `(cx, cy)`.
pub(super) fn area_of(act: &Act, slv: u8, range: f32, aimed: &Aimed, cx: f32, cy: f32) -> (Area, bool) {
    let a = &act.area;
    let (x, y) = if a.at_caster { (cx, cy) } else { (aimed.x, aimed.y) };
    let point = a.is_point(slv);
    let area = match a.shape {
        Shape::Circle => Area::Circle { x, y, r: a.radius.at(slv).max(0.0) },
        Shape::Rect => {
            let w = a.width.at(slv).max(0.0);
            let l = a.length.at(slv);
            Area::Rect { x, y, dir: aimed.dir, half_len: if l > 0.0 { l / 2.0 } else { w / 2.0 }, half_wid: w / 2.0 }
        }
        Shape::Line => {
            let l = a.length.at(slv);
            Area::line(aimed.ox, aimed.oy, aimed.dir, if l > 0.0 { l } else { range }, a.width.at(slv))
        }
        Shape::Arc => Area::Arc { x: cx, y: cy, dir: aimed.dir, r: a.radius.at(slv).max(0.0), arc_cos: a.arc_cos },
    };
    (area, point)
}

/// Repeated hits of an act.
pub(super) fn count_of(act: &Act, slv: u8) -> u32 {
    act.timing.count.at(slv).round().max(1.0) as u32
}

/// Seconds a projectile flies from the caster to `(x, y)`.
fn flight(act: &Act, aimed: &Aimed, x: f32, y: f32) -> f32 {
    if act.timing.speed <= 0.0 {
        return 0.0;
    }
    ((x - aimed.ox).powi(2) + (y - aimed.oy).powi(2)).sqrt() / act.timing.speed
}

/// Whether an act leaves ground with a telegraph id.
fn keeps_id(act: &Act, slv: u8) -> bool {
    act.timing.lasting(slv)
}

/// The telegraphs of a cast (sent at the press), ids from `zone` up.
pub(super) fn telegraphs(def: &SkillDef, slv: u8, caster: &HubPlayer, aimed: &Aimed, zone: u32, players: &[HubPlayer]) -> Vec<Telegraph> {
    let mut out = Vec::new();
    let mut id = zone;
    let entity_of = |uid: u32| players.iter().find(|p| p.profile.user_id == uid).map_or(0, |p| p.profile.character_id);
    for act in def.effects.iter().filter_map(Effect::act) {
        let (mut area, point) = area_of(act, slv, def.range, aimed, caster.x, caster.y);
        if act.area.scatter > 0.0
            && let Area::Circle { x, y, r } = area
        {
            area = Area::Circle { x, y, r: r + act.area.scatter };
        }
        if point {
            area = Area::Circle { x: aimed.x, y: aimed.y, r: 1.0 };
        }
        let n = count_of(act, slv);
        let t = &act.timing;
        let fly = flight(act, aimed, aimed.x, aimed.y);
        let span = if t.lasting(slv) { t.secs.at(slv) } else { (n - 1) as f32 * t.every + fly };
        let follow = match act.area.follow {
            FollowFx::Caster => caster.profile.character_id,
            FollowFx::Target => match (aimed.monster, aimed.ally) {
                (Some((s, _)), _) => MONSTER_ENTITY_BASE + s as u32,
                (None, Some(uid)) => entity_of(uid),
                _ => 0,
            },
            FollowFx::None => 0,
        };
        let tid = if keeps_id(act, slv) {
            id += 1;
            id - 1
        } else {
            0
        };
        out.push(area.telegraph(tid, look_of(act), (span * 1000.0).round() as u32, follow));
    }
    out
}

/// Ids the lasting areas of a cast take.
pub(super) fn zone_ids(def: &SkillDef, slv: u8) -> u32 {
    def.effects.iter().filter_map(Effect::act).filter(|a| keeps_id(a, slv)).count() as u32
}

/// The context of one landing.
pub(super) struct Landing<'a> {
    pub def: &'a SkillDef,
    pub skill: u16,
    pub slv: u8,
    pub now: u32,
    pub hz: f32,
    pub zone: u32,
    /// Character level the dice scale with instead of the caster's (a
    /// pickup bonus casts at the location's level).
    pub power: Option<u32>,
}

/// Turns one cast into live areas and moves the caster.
pub(super) fn land(cx: &Landing<'_>, caster: &mut HubPlayer, mut aimed: Aimed, horde: &Horde, walk: &WalkGrid, rng: &mut Rng, lives: &mut Vec<Live>) {
    let Some(f) = caster.fighter.as_ref() else { return };
    let (cl, class, weapon, mods) = (cx.power.unwrap_or(f.level), f.class, f.weapon, f.mods);
    let dc = save_dc(cl);
    let (slv, hz, now, def) = (cx.slv, cx.hz, cx.now, cx.def);
    let uid = caster.profile.user_id;
    let mut zone = cx.zone;
    for effect in &def.effects {
        let act = match effect {
            Effect::Move { mode, distance } => {
                let to = moved(walk, caster.x, caster.y, aimed.dir, distance.at(slv).max(0.0), *mode);
                (caster.x, caster.y) = to;
                // What follows starts from where the caster ended up.
                (aimed.ox, aimed.oy) = to;
                continue;
            }
            Effect::Act(act) => act,
            Effect::Passive(_) | Effect::Pending(_) => continue,
        };
        let t = &act.timing;
        let load = match &act.payload {
            Payload::Damage(DamageFx { dice, extra, scale_k, mult, bonus, kind, save, knockback }) => {
                let (base, kind, to_hit) = match dice {
                    EffectDice::Fixed(d) => (scaled(*d, *scale_k, mult.at(slv), bonus.at(slv), cl), *kind, None),
                    EffectDice::Weapon => {
                        let (d, k) = weapon.damage(class, cl);
                        let to_hit = weapon.to_hit(cl) + mods.to_hit;
                        (Dice { bonus: d.bonus + bonus.at(slv).round() as i32 + mods.damage_flat, ..d }, k, Some(to_hit))
                    }
                };
                let extra = extra.and_then(|e| {
                    let n = (e.count as f32 * mult.at(slv)).round() as u32;
                    (n > 0).then_some(Dice { count: n, ..e })
                });
                let save = save.as_ref().map(|sv| (sv.condition, ticks16(sv.secs.at(slv), hz)));
                let blow = Blow {
                    damage: Damage::new(base, kind),
                    extra,
                    to_hit,
                    dc,
                    save,
                    k: cantrip_k(cl),
                    caster: uid,
                    bane: mods.evil_damage,
                    drain: mods.soul_drain,
                };
                Load::Damage { blow, knock: knockback.at(slv) }
            }
            Payload::Heal(HealFx { dice, scale_k, mult }) => Load::Heal(scaled(*dice, *scale_k, mult.at(slv), 0.0, cl)),
            Payload::Condition { condition, save, secs, mired } => {
                let ticks = match (*save, t.lasting(slv)) {
                    (true, _) => ticks16(secs.at(slv), hz),
                    // Kept while inside: two pulses.
                    (false, true) => ticks16(t.pulse * 2.0, hz).max(2),
                    (false, false) => ticks16(BRIEF_SECS, hz),
                };
                Load::Condition { dc: save.then_some(dc), condition: *condition, ticks, mired: *mired }
            }
            Payload::Push(d) => Load::Push(d.at(slv)),
            Payload::Buff(b) if b.toggle => {
                if let Some(f) = caster.fighter.as_mut() {
                    f.toggle(cx.skill, b.mods(slv));
                }
                continue;
            }
            Payload::Buff(b) => {
                let ticks = if t.lasting(slv) { ticks_of(t.pulse * 2.0, hz).max(2) } else { ticks_of(b.secs.at(slv), hz) };
                let temp = b.temp_hp.map(|d| scaled(d, b.scale_k, b.mult.at(slv), 0.0, cl));
                Load::Buff { mods: b.mods(slv), ticks, temp, shield: b.shield.at(slv) }
            }
            Payload::Ward { hits, ranged } => Load::Ward { hits: hits.at(slv).round().max(1.0) as u32, ranged: *ranged },
            Payload::Barrier => Load::Barrier,
            Payload::Revive { hp } => Load::Revive(hp.at(slv).clamp(0.01, 1.0)),
            Payload::Cure => Load::Cure,
            Payload::Dispel => Load::Dispel,
        };
        let (area, point) = area_of(act, slv, def.range, &aimed, caster.x, caster.y);
        let lasting = t.lasting(slv);
        let id = if lasting {
            zone += 1;
            zone - 1
        } else {
            0
        };
        let ally = aimed.ally.or_else(|| (def.target == Target::Caster && !act.payload.hostile()).then_some(uid));
        let follow = match act.area.follow {
            FollowFx::Caster => Follow::Player(uid),
            FollowFx::Target => match (aimed.monster, aimed.ally) {
                (Some((s, g)), _) => Follow::Monster(s, g),
                (None, Some(a)) => Follow::Player(a),
                _ => Follow::None,
            },
            // A point skill still lands on the monster it picked.
            FollowFx::None => match aimed.monster {
                Some((s, g)) if point => Follow::Monster(s, g),
                _ => Follow::None,
            },
        };
        let n = count_of(act, slv);
        let spread = if act.area.spread { spread_targets(horde, &aimed, def.range, n) } else { Vec::new() };
        for i in 0..n {
            let mut area = area;
            let mut follow = follow;
            if act.area.scatter > 0.0 {
                let a = rng.die(360) as f32 * std::f32::consts::TAU / 360.0;
                let r = act.area.scatter * (rng.die(1000) as f32 / 1000.0).sqrt();
                let (x, y) = area.centre();
                area = area.moved_to(x + a.cos() * r, y + a.sin() * r);
            }
            if !spread.is_empty() {
                let (s, g, x, y) = spread[i as usize % spread.len()];
                area = area.moved_to(x, y);
                follow = Follow::Monster(s, g);
            }
            let (x, y) = area.centre();
            let at = now + ticks_of(i as f32 * t.every + flight(act, &aimed, x, y), hz);
            let pulse = if lasting { ticks_of(t.pulse, hz).max(1) } else { 0 };
            let until = if lasting { at + ticks_of(t.secs.at(slv), hz) } else { at };
            let trap = (t.trigger > 0.0).then(|| Trap { radius: t.trigger, fuse_at: (t.fuse > 0.0).then(|| at + ticks_of(t.fuse, hz)) });
            lives.push(Live {
                id,
                map: caster.map,
                skill: cx.skill,
                caster: uid,
                next: at,
                until,
                pulse,
                area,
                follow,
                point: point && !act.area.spread,
                trap,
                limit: act.area.limit.at(slv).round().max(0.0) as u32,
                chain: (act.area.chain.at(slv).round().max(0.0) as u32, act.area.jump),
                ally,
                load: load.clone(),
            });
        }
    }
}

/// Where a slide (stops at the first wall) or a blink (the farthest open
/// spot along the way) ends.
pub(super) fn moved(walk: &WalkGrid, x: f32, y: f32, dir: (f32, f32), total: f32, mode: MoveMode) -> (f32, f32) {
    match mode {
        MoveMode::Slide => {
            let (mut x, mut y, mut gone) = (x, y, 0.0);
            while gone + MOVE_STEP <= total {
                let (nx, ny) = (x + dir.0 * MOVE_STEP, y + dir.1 * MOVE_STEP);
                if !walk.is_walkable(nx, ny) {
                    break;
                }
                (x, y, gone) = (nx, ny, gone + MOVE_STEP);
            }
            (x, y)
        }
        MoveMode::Blink => {
            let mut d = total;
            while d > 0.0 {
                let (nx, ny) = (x + dir.0 * d, y + dir.1 * d);
                if walk.is_walkable(nx, ny) {
                    return (nx, ny);
                }
                d -= MOVE_STEP;
            }
            (x, y)
        }
    }
}

/// Distinct monsters for a `spread` effect: nearest to the aimed point
/// first, within the skill's range of it.
fn spread_targets(horde: &Horde, aimed: &Aimed, range: f32, n: u32) -> Vec<(u16, u8, f32, f32)> {
    let mut slots = Vec::new();
    Area::Circle { x: aimed.x, y: aimed.y, r: range * 0.75 }.monsters(horde, &mut slots);
    let store = horde.store();
    let d2 = |s: u16| (store.x[s as usize] - aimed.x).powi(2) + (store.y[s as usize] - aimed.y).powi(2);
    slots.sort_by(|&a, &b| d2(a).total_cmp(&d2(b)));
    slots.truncate(n as usize);
    slots.into_iter().map(|s| (s, store.generation[s as usize], store.x[s as usize], store.y[s as usize])).collect()
}

impl HubInstance {
    /// Slot presses, wind-ups and live areas on the horde map. Runs inside
    /// [`HubInstance::fight`], before the basic attacks; kills land in
    /// `kills` like the basic attack's (slot, the caster's user id).
    pub(super) fn cast_step(&mut self, out: &mut Outgoing, hits: &mut Vec<HitEvent>, kills: &mut Vec<Kill>) {
        let Some(horde) = self.horde.as_mut() else { return };
        let map = horde.map();
        let hz = self.config.tick_hz as f32;
        let now = self.tick;
        let book = std::sync::Arc::clone(&self.skills);
        let mut casts: Vec<ServerMsg> = Vec::new();
        let mut broken: Vec<(u32, u16)> = Vec::new();

        for idx in 0..self.players.len() {
            if self.players[idx].map != map {
                continue;
            }
            let pressed = std::mem::take(&mut self.players[idx].pressed);
            let moving = self.players[idx].dir != (0.0, 0.0);
            let entity = self.players[idx].profile.character_id;
            let uid = self.players[idx].profile.user_id;
            let Some(f) = self.players[idx].fighter.as_mut() else { continue };
            f.refresh(now);
            for cd in f.cooldowns.iter_mut().filter(|c| **c > 0) {
                *cd -= 1;
            }
            f.recovery = f.recovery.saturating_sub(1);
            // A channel holds while its slot is held and the caster stands.
            if let Some(mut ch) = f.channel {
                ch.idle = if pressed & (1 << ch.slot) != 0 { 0 } else { ch.idle.saturating_add(1) };
                let lasting = self.lives.iter().any(|l| l.caster == uid && l.skill == ch.skill);
                if moving || ch.idle > CHANNEL_GRACE || !f.is_up() || f.status.held() || !lasting {
                    f.channel = None;
                    broken.push((uid, ch.skill));
                } else {
                    f.channel = Some(ch);
                    continue;
                }
            }
            // Down, stunned, frozen or turned to stone: no casting.
            if !f.is_up() || f.status.held() {
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
                        let walk = &self.world.map(map).walk;
                        let cx = Landing { def, skill: c.skill, slv: c.level, now, hz, zone: c.zone, power: None };
                        land(&cx, &mut self.players[idx], c.aimed, horde, walk, &mut self.dice, &mut self.lives);
                        if def.channel
                            && let Some(f) = self.players[idx].fighter.as_mut()
                        {
                            f.channel = Some(Channel { skill: c.skill, slot: c.slot, idle: 0 });
                        }
                    } else {
                        casts.push(cancel(entity));
                    }
                }
            }

            // New presses, lowest slot first; one skill per tick. Playing
            // dead allows nothing.
            let Some(f) = self.players[idx].fighter.as_ref() else { continue };
            if f.mods.hidden {
                continue;
            }
            for slot in (0..16).filter(|s| pressed & (1 << s) != 0) {
                let pick = if slot == TRAVEL_SLOT { f.travel } else { f.hotbar.get(slot).copied().flatten() };
                let Some(skill) = pick else { continue };
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
                let Some(aimed) = aim_skill(&field, &self.players[idx], def, level) else { continue };
                let cast_ticks = (def.cast_secs(level) * hz).round() as u32;
                let cooldown_ms = (def.cooldown_secs(level) * 1000.0).round() as u32;
                let zone = self.next_zone;
                self.next_zone = self.next_zone.wrapping_add(zone_ids(def, level)).max(1);
                let areas = telegraphs(def, level, &self.players[idx], &aimed, zone, &self.players);
                let p = &mut self.players[idx];
                if def.target != Target::Escape && (aimed.x, aimed.y) != (p.x, p.y) {
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
                    areas,
                    level,
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
                    let walk = &self.world.map(map).walk;
                    let cx = Landing { def, skill, slv: level, now, hz, zone, power: None };
                    land(&cx, &mut self.players[idx], aimed, horde, walk, &mut self.dice, &mut self.lives);
                    if def.channel
                        && let Some(f) = self.players[idx].fighter.as_mut()
                    {
                        f.channel = Some(Channel { skill, slot: slot as u8, idle: 0 });
                    }
                } else {
                    f.casting = Some(Casting { skill, level, ticks: cast_ticks, aimed, zone, slot: slot as u8 });
                }
                break;
            }
        }

        // Live areas: due hits, ground pulses, traps, wards.
        let mut ended: Vec<u32> = Vec::new();
        if !broken.is_empty() {
            self.lives.retain(|l| {
                let gone = broken.contains(&(l.caster, l.skill));
                if gone && l.id != 0 {
                    ended.push(l.id);
                }
                !gone
            });
        }
        let mut lives = std::mem::take(&mut self.lives);
        let mut dispels: Vec<Area> = Vec::new();
        let walk = &self.world.map(map).walk;
        let secs = horde.director_elapsed();
        lives.retain_mut(|l| {
            if l.map != map {
                return now <= l.until;
            }
            if l.spent() {
                ended.push(l.id);
                return false;
            }
            if now > l.until || (l.next > l.until && l.pulse == 0 && !matches!(l.load, Load::Ward { .. })) {
                return false;
            }
            if now < l.next || matches!(l.load, Load::Ward { .. }) {
                return true;
            }
            if matches!(l.load, Load::Dispel) {
                dispels.push(l.area);
                return false;
            }
            // Moves with its player or monster.
            match l.follow {
                Follow::Player(uid) => match self.players.iter().find(|p| p.profile.user_id == uid && p.map == map) {
                    Some(p) if p.fighter.as_ref().is_none_or(|f| f.state != Vital::Dead) => l.area = l.area.moved_to(p.x, p.y),
                    // The caster of an aura left or died: it ends.
                    _ if l.pulse > 0 => {
                        ended.push(l.id);
                        return false;
                    }
                    _ => {}
                },
                Follow::Monster(s, g) => {
                    let st = horde.store();
                    if st.is_alive(s) && st.generation[s as usize] == g && st.hp[s as usize] > 0 {
                        l.area = l.area.moved_to(st.x[s as usize], st.y[s as usize]);
                    } else {
                        // Its monster is gone: it lands where it was.
                        l.follow = Follow::None;
                    }
                }
                Follow::None => {}
            }
            if let Some(trap) = l.trap {
                let (x, y) = l.area.centre();
                let mut near = Vec::new();
                Area::Circle { x, y, r: trap.radius }.monsters(horde, &mut near);
                // A plant does not spring a trap.
                near.retain(|&s| !horde.is_plant(s));
                if near.is_empty() && trap.fuse_at.is_none_or(|t| now < t) {
                    l.next = now + TRAP_CHECK;
                    return now + TRAP_CHECK <= l.until;
                }
                let h0 = hits.len();
                act(l, horde, &mut self.players, &mut self.dice, walk, now, hits, kills);
                credit(&mut self.run, l, &hits[h0..], now, secs, horde);
                ended.push(l.id);
                return false;
            }
            let h0 = hits.len();
            act(l, horde, &mut self.players, &mut self.dice, walk, now, hits, kills);
            credit(&mut self.run, l, &hits[h0..], now, secs, horde);
            if l.pulse > 0 && now + l.pulse <= l.until {
                l.next = now + l.pulse;
                true
            } else {
                false
            }
        });
        // Anything a pulse added (none today) goes after the old entries.
        lives.append(&mut self.lives);
        self.lives = lives;
        ailments(horde, &mut self.dice, now, hz, hits, kills);
        for area in dispels {
            ended.extend(self.dispel_hazards(map, &area));
        }

        let recipients: Vec<u32> = self.players.iter().filter(|p| p.map == map).map(|p| p.profile.user_id).collect();
        for msg in casts.into_iter().chain(ended.into_iter().filter(|&id| id != 0).map(|id| ServerMsg::ZoneEnd { id })) {
            for &uid in &recipients {
                out.push((uid, HubEvent::Control(msg.clone())));
            }
        }
    }
}

/// The result screen's share of one action of a live area: its damage and
/// healing to the caster's skill (`stats.rs`), a revive to its caster.
fn credit(run: &mut super::stats::RunLog, l: &Live, hits: &[HitEvent], now: u32, secs: f32, horde: &Horde) {
    if matches!(l.load, Load::Revive(_)) {
        run.revive_cast(l.caster, now);
    }
    run.credit(l.caster, l.skill, hits, now, secs, horde.store());
}

pub(super) fn cancel(entity: u32) -> ServerMsg {
    ServerMsg::Cast { caster: entity, skill: String::new(), x: 0.0, y: 0.0, cast_ms: 0, cooldown_ms: 0, areas: Vec::new(), level: 0 }
}

/// Monsters a live area acts on this time.
fn foes(l: &Live, horde: &Horde) -> Vec<u16> {
    let mut out = Vec::new();
    if l.point {
        match l.follow {
            Follow::Monster(s, g) => {
                let st = horde.store();
                if st.is_alive(s) && st.generation[s as usize] == g && st.hp[s as usize] > 0 {
                    out.push(s);
                }
            }
            _ => {
                let (x, y) = l.area.centre();
                // A monster under the point before a plant.
                out.extend(horde.nearest_foe(x, y, 1.0).or_else(|| horde.nearest_to(x, y, 1.0, |_| false)).map(|(s, _)| s));
            }
        }
    } else {
        l.area.monsters(horde, &mut out);
        let store = horde.store();
        if l.limit > 0 || l.chain.0 > 0 {
            // Monsters first: plants never take a limited target's place.
            let key = |s: u16| l.area.order(store.x[s as usize], store.y[s as usize]);
            out.sort_by(|&a, &b| store.is_plant(a).cmp(&store.is_plant(b)).then(key(a).total_cmp(&key(b))).then(a.cmp(&b)));
        }
        if l.limit > 0 {
            out.truncate(l.limit as usize);
        }
    }
    // A chain starts at the first target and hops to the nearest new one.
    if l.chain.0 > 0
        && let Some(&first) = out.first()
    {
        out.truncate(1);
        let store = horde.store();
        let mut at = first;
        for _ in 0..l.chain.0 {
            let (x, y) = (store.x[at as usize], store.y[at as usize]);
            let Some((next, _)) = horde.nearest_to(x, y, l.chain.1, |s| out.contains(&s) || store.is_plant(s)) else { break };
            out.push(next);
            at = next;
        }
    }
    out
}

/// Players a friendly area acts on (indices): standing ones inside, the
/// downed for a revive, and always the picked ally.
fn friends(l: &Live, players: &[HubPlayer]) -> Vec<usize> {
    let revive = matches!(l.load, Load::Revive(_));
    players
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            let Some(f) = p.fighter.as_ref() else { return false };
            let state_ok = if revive { matches!(f.state, Vital::Downed { .. }) } else { f.is_up() };
            let picked = l.ally == Some(p.profile.user_id);
            p.map == l.map && state_ok && (picked || (!l.point && l.area.contains(p.x, p.y, BODY)))
        })
        .map(|(i, _)| i)
        .collect()
}

/// One action of a live area.
#[allow(clippy::too_many_arguments)]
fn act(
    l: &mut Live,
    horde: &mut Horde,
    players: &mut [HubPlayer],
    rng: &mut Rng,
    walk: &WalkGrid,
    now: u32,
    hits: &mut Vec<HitEvent>,
    kills: &mut Vec<Kill>,
) {
    match l.load.clone() {
        Load::Damage { blow, knock } => {
            let targets = foes(l, horde);
            let before = kills.len();
            strike(horde, rng, &targets, blow, hits, kills);
            let slain = (kills.len() - before) as i32;
            if slain > 0
                && blow.drain > 0
                && let Some(f) = players.iter_mut().find(|p| p.profile.user_id == blow.caster).and_then(|p| p.fighter.as_mut())
            {
                f.sp = (f.sp + slain * blow.drain).min(f.max_sp);
            }
            if knock != 0.0 {
                for s in targets {
                    push(horde, walk, l.area, s, knock);
                }
            }
        }
        Load::Condition { dc, condition, ticks, mired } => {
            for s in foes(l, horde) {
                if dc.is_some_and(|dc| resist(horde, rng, s, dc, true)) {
                    continue;
                }
                inflict(horde, s, condition, ticks, 0);
                if mired && condition != Condition::Mired {
                    inflict(horde, s, Condition::Mired, ticks, 0);
                }
                if condition == Condition::Taunted {
                    horde.store_mut().taunter[s as usize] = l.caster;
                }
            }
        }
        Load::Push(d) => {
            for s in foes(l, horde) {
                push(horde, walk, l.area, s, d);
            }
        }
        Load::Barrier => {
            for s in foes(l, horde) {
                let i = s as usize;
                let body = horde.catalog().get(horde.store().kind[i]).radius;
                let (x, y) = (horde.store().x[i], horde.store().y[i]);
                if let Some((nx, ny)) = l.area.eject(x, y, body)
                    && walk.is_walkable(nx, ny)
                {
                    horde.place(s, nx, ny);
                }
            }
        }
        Load::Heal(dice) => {
            for i in friends(l, players) {
                let p = &mut players[i];
                let id = p.profile.character_id;
                let Some(f) = p.fighter.as_mut() else { continue };
                let amount = dice.roll(rng, false).max(0).min(f.max_hp - f.hp);
                f.hp += amount;
                hits.push(HitEvent { target: id, amount: amount as u32, flags: hit::HEAL | (DamageType::Radiant as u8) << 4 });
            }
        }
        Load::Buff { mods, ticks, temp, shield } => {
            for i in friends(l, players) {
                let Some(f) = players[i].fighter.as_mut() else { continue };
                let extra = temp.map_or(0, |d| d.roll(rng, false).max(0)) + (f.max_hp as f32 * shield).round() as i32;
                f.buff(l.skill, now + ticks, mods, extra);
            }
        }
        Load::Revive(frac) => {
            for i in friends(l, players) {
                let Some(f) = players[i].fighter.as_mut() else { continue };
                f.state = Vital::Up;
                f.hp = ((f.max_hp as f32 * frac).round() as i32).clamp(1, f.max_hp);
            }
        }
        Load::Cure => {
            for i in friends(l, players) {
                if let Some(f) = players[i].fighter.as_mut() {
                    f.clear_conditions();
                }
            }
        }
        // Handled by the instance (it owns the monsters' ground).
        Load::Ward { .. } | Load::Dispel => {}
    }
}

/// Puts a condition on a monster; fire remembers the strongest `k` lit.
fn inflict(horde: &mut Horde, s: u16, c: Condition, ticks: u16, k: u32) {
    let store = horde.store_mut();
    store.afflict(s, c, ticks);
    if c == Condition::Burning {
        let b = &mut store.burn_k[s as usize];
        *b = (*b).max(k.clamp(1, u8::MAX as u32) as u8);
    }
}

/// Pushes a monster `distance` cells away from the area (negative pulls it
/// in, never past the centre) over open ground. Bosses stand firm.
pub(super) fn push(horde: &mut Horde, walk: &WalkGrid, area: Area, s: u16, distance: f32) {
    let i = s as usize;
    if horde.store().flags[i] & FLAG_BOSS != 0 || horde.store().hp[i] <= 0 || !horde.store().is_alive(s) {
        return;
    }
    let (x, y) = (horde.store().x[i], horde.store().y[i]);
    let mut dir = area.push_dir(x, y);
    let mut total = distance;
    if distance < 0.0 {
        let (cx, cy) = area.centre();
        let to_centre = ((cx - x).powi(2) + (cy - y).powi(2)).sqrt();
        dir = (-dir.0, -dir.1);
        total = (-distance).min(to_centre);
    }
    let to = moved(walk, x, y, dir, total, MoveMode::Slide);
    if to != (x, y) {
        horde.place(s, to.0, to.1);
    }
}

/// Burning (1d4 fire per k) and poison (1d4) bite once a second, staggered
/// by slot so a burning crowd does not land in one tick.
fn ailments(horde: &mut Horde, rng: &mut Rng, now: u32, hz: f32, hits: &mut Vec<HitEvent>, kills: &mut Vec<Kill>) {
    let second = (hz.round() as u32).max(1);
    for k in 0..horde.store().alive().len() {
        let s = horde.store().alive()[k];
        let i = s as usize;
        let status = horde.store().status_of(s);
        if !status.ailing() || !(now + s as u32).is_multiple_of(second) {
            continue;
        }
        let burn = horde.store().burn_k[i] as u32;
        for (c, dice, kind) in [
            (Condition::Burning, Dice::new(burn.max(1), 4, 0), DamageType::Fire),
            (Condition::Poisoned, Dice::new(1, 4, 0), DamageType::Poison),
        ] {
            if !status.has(c) || horde.store().hp[i] <= 0 {
                continue;
            }
            let amount = wound(horde, rng, s, Damage::new(dice, kind), None, false, false, 0);
            let mut flags = (kind as u8) << 4;
            if horde.store().hp[i] <= 0 {
                flags |= hit::KILL;
                // Nobody to credit: the nearest party member gets the loot.
                kills.push((s, 0));
            }
            hits.push(HitEvent { target: MONSTER_ENTITY_BASE + s as u32, amount: amount as u32, flags });
        }
    }
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
/// halves everything. A plant takes exactly one HP from any hit, whatever
/// the dice, element or crit (COMBAT.md §4.8). Returns the amount taken
/// off its HP.
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
    if horde.is_plant(s) {
        horde.store_mut().hp[s as usize] -= 1;
        return 1;
    }
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
    if amount > 0 {
        horde.flinch(s);
    }
    amount
}

/// A monster slot that died this tick and the user id of the player who
/// landed the blow (0 = nobody: burning, poison).
pub(super) type Kill = (u16, u32);

/// What one skill hit carries.
#[derive(Clone, Copy, Debug)]
pub(super) struct Blow {
    damage: Damage,
    extra: Option<Dice>,
    /// To-hit bonus when it swings the weapon; `None` = save for half.
    to_hit: Option<i32>,
    dc: i32,
    save: Option<(Condition, u16)>,
    k: u32,
    /// User id of the caster (a taunt rider hunts them).
    caster: u32,
    /// Extra damage against undead and demons (Demon Bane).
    bane: i32,
    /// SP back to the caster per kill (Soul Drain).
    drain: i32,
}

/// One hit on each of `targets`.
fn strike(horde: &mut Horde, rng: &mut Rng, targets: &[u16], blow: Blow, hits: &mut Vec<HitEvent>, kills: &mut Vec<Kill>) {
    for &s in targets {
        if horde.store().hp[s as usize] <= 0 {
            continue;
        }
        let id = MONSTER_ENTITY_BASE + s as u32;
        let (crit, saved) = match blow.to_hit {
            Some(bonus) => {
                let ac = horde.block(s).ac + horde.store().status_of(s).ac();
                match attack_roll(rng, bonus, ac, horde.store().status_of(s).advantage_against(), 20) {
                    AttackOutcome::Miss => {
                        hits.push(HitEvent { target: id, amount: 0, flags: hit::MISS });
                        continue;
                    }
                    o => (o == AttackOutcome::Critical && !horde.is_plant(s), false),
                }
            }
            None => (false, resist(horde, rng, s, blow.dc, blow.save.is_some())),
        };
        let mut amount = wound(horde, rng, s, blow.damage, blow.extra, crit, saved, blow.k);
        if blow.bane > 0 && horde.catalog().get(horde.store().kind[s as usize]).evil {
            amount += blow.bane;
            horde.store_mut().hp[s as usize] -= blow.bane;
        }
        let mut flags = (blow.damage.kind as u8) << 4;
        if crit {
            flags |= hit::CRITICAL;
        }
        if horde.store().hp[s as usize] <= 0 {
            flags |= hit::KILL;
            kills.push((s, blow.caster));
        } else if let Some((condition, ticks)) = blow.save {
            // A weapon swing that hit still allows a save against its rider.
            let failed = if blow.to_hit.is_some() { !resist(horde, rng, s, blow.dc, true) } else { !saved };
            if failed {
                inflict(horde, s, condition, ticks, blow.k);
                if condition == Condition::Taunted {
                    horde.store_mut().taunter[s as usize] = blow.caster;
                }
            }
        }
        hits.push(HitEvent { target: id, amount: amount as u32, flags });
    }
}

/// Learned levels, hotbar and passives of a fighter from a loadout.
pub(super) fn equip(f: &mut Fighter, book: &SkillBook, loadout: &crate::world::combat::Loadout) {
    f.skills = vec![0; book.len()];
    f.cooldowns.resize(book.len(), 0);
    let mut passive = Mods::default();
    for (i, def) in book.iter() {
        let lvl = loadout.level(&def.id).min(def.max_level);
        if lvl == 0 || !def.usable_by(f.class) {
            continue;
        }
        f.skills[i as usize] = lvl;
        for e in &def.effects {
            if let Effect::Passive(b) = e {
                passive.add(&b.mods(lvl));
            }
        }
    }
    f.set_passive(passive);
    for (slot, h) in f.hotbar.iter_mut().enumerate() {
        *h = book.find(loadout.slot(slot)).map(|(i, _)| i).filter(|&i| f.skills[i as usize] > 0);
    }
    // The travel key: the learned movement skill, the higher tier first
    // (later in the book), then the higher level.
    f.travel = book
        .iter()
        .filter(|(i, def)| f.skills[*i as usize] > 0 && !def.passive && def.effects.iter().any(|e| matches!(e, Effect::Move { .. })))
        .max_by_key(|(i, _)| (*i, f.skills[*i as usize]))
        .map(|(i, _)| i);
}

/// Bit of `Input::slots` for the travel key (past the hotbar's slots).
pub const TRAVEL_SLOT: usize = 15;
