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

use super::aim::{aim_press, norm, Aimed, Field, Press, BODY, REFRESH_SECS};
use super::area::Area;
use super::companion::{Companions, Owner, Summon, Summoned};
use super::fight::{Fighter, Vital};
use super::{facing_of, HubEvent, HubInstance, HubPlayer, MapIdx, Outgoing};
use crate::world::combat::rules::{attack_roll, saving_throw, AttackOutcome};
use crate::world::combat::items::{self, Kind as ItemKind};
use crate::world::combat::skills::{
    Act, DamageFx, Effect, EffectDice, Follow as FollowFx, HealFx, MoveMode, Payload, Shape, SkillBook, SkillDef, Target, TOUGH_CAP,
};
use crate::world::combat::{cantrip_k, save_dc, Condition, Damage, DamageType, Dice, Mods, Rng};
use crate::world::horde::storage::{FLAG_BOSS, FLAG_REFLECT};
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
/// Field plants one caster may have grown at once (Plant Cultivation); a
/// new one replaces the oldest.
pub(super) const MAX_GROWN: usize = 3;
/// Without its reagents a skill lands with this share of its dice.
pub(super) const WEAK: f32 = 0.5;

/// What casts took from the bags and asked of the instance, reported once
/// the step is done: the hub's copy of each bag already changed, the
/// socket task's follows (`HubEvent::ItemUsed`, `HubEvent::ZenySpent`).
#[derive(Debug, Default)]
pub(super) struct Spent {
    /// (user id, zeny) a skill paid (Mammonite).
    pub zeny: Vec<(u32, u64)>,
    /// (user id, item) used up: reagents, a thrown potion.
    pub items: Vec<(u32, &'static str)>,
    /// (user id, x, y, skill level) field plants to grow (Plant
    /// Cultivation: the horde is borrowed while casts land).
    pub plants: Vec<(u32, f32, f32, u8)>,
}

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
    /// A companion (entity id): a heal or a potion on its way to a
    /// homunculus.
    Companion(u32),
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
    /// One of the caster's potions, restoring `mult` × its roll over
    /// `ticks` to whom the area reaches.
    Pitch { mult: f32, ticks: u32 },
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

/// Bonus damage of an amplified spell: `fraction` of its dice as if they
/// rolled their maximum (each such die adds `(sides - 1) / 2` over its
/// average).
pub(super) fn amplified(d: Dice, fraction: f32) -> i32 {
    (fraction.clamp(0.0, 1.0) * d.count as f32 * (d.sides as f32 - 1.0) / 2.0).round() as i32
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
        Payload::Cure | Payload::Pitch { .. } => look::HEAL,
        Payload::Summon { .. } | Payload::Rally(_) => look::BUFF,
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

/// A piercing projectile: a line with a flight speed, once (no ground, no
/// target limit or chain). It strikes every monster on the line as the
/// arrow reaches it (Piercing Arrow).
fn pierces(act: &Act, slv: u8) -> bool {
    act.area.shape == Shape::Line
        && act.timing.speed > 0.0
        && !act.timing.lasting(slv)
        && act.timing.trigger <= 0.0
        && act.area.limit.at(slv) <= 0.0
        && act.area.chain.at(slv) <= 0.0
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
        let fly = match area {
            // A piercing arrow flies the whole line.
            Area::Rect { half_len, .. } if pierces(act, slv) => 2.0 * half_len / t.speed,
            _ => flight(act, aimed, aimed.x, aimed.y),
        };
        let span = if t.lasting(slv) { t.secs.at(slv) } else { (n - 1) as f32 * t.every + fly };
        let follow = match act.area.follow {
            FollowFx::Caster => caster.profile.character_id,
            FollowFx::Target => match (aimed.monster, aimed.ally) {
                (Some((s, _)), _) => MONSTER_ENTITY_BASE + s as u32,
                (None, Some(uid)) => entity_of(uid),
                _ => aimed.pet.unwrap_or(0),
            },
            // A single target is struck wherever it goes (`land`): its
            // marker goes with it.
            FollowFx::None if point => match (aimed.monster, aimed.ally) {
                (Some((s, _)), _) => MONSTER_ENTITY_BASE + s as u32,
                (None, Some(uid)) => entity_of(uid),
                _ => aimed.pet.unwrap_or(0),
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

/// Turns one cast into live areas and moves the caster; summons call out
/// their companions at once. What it costs from the bag (zeny, reagents)
/// is paid from the hub's copy and listed in `spent`.
pub(super) fn land(
    cx: &Landing<'_>,
    caster: &mut HubPlayer,
    mut aimed: Aimed,
    horde: &Horde,
    walk: &WalkGrid,
    rng: &mut Rng,
    lives: &mut Vec<Live>,
    companions: &mut Companions,
    spent: &mut Spent,
) {
    let Some(f) = caster.fighter.as_ref() else { return };
    let (cl, class, weapon, mods) = (cx.power.unwrap_or(f.level), f.class, f.weapon, f.mods);
    let (own_level, max_hp) = (f.level, f.max_hp);
    // A pickup bonus casts for the player without their gear.
    let own = cx.power.is_none();
    // The arrow nocked (Arrow Change) for arrow damage.
    let nocked = if own { f.nocked() } else { None };
    let dc = save_dc(cl) + if own { mods.spell_dc } else { 0 };
    let (slv, hz, now, def) = (cx.slv, cx.hz, cx.now, cx.def);
    let uid = caster.profile.user_id;
    let mut zone = cx.zone;
    // Paid for from the bag (a pickup bonus casts for free, but plainly):
    // Mammonite's zeny, else a plain swing; Acid Demonstration's bottles,
    // else half the dice.
    let cost = def.zeny_cost(slv);
    let paid = cost == 0 || (own && pay(caster, cost, spent));
    let stocked = def.reagents.is_empty() || (own && use_reagents(caster, &def.reagents, spent));
    let strength = if stocked { 1.0 } else { WEAK };
    // The cart's weight: what the bag holds (stacks).
    let loaded = caster.profile.bag.items.len() as f32;
    // An offensive cast out of hiding ends it; weapon blows of that cast
    // roll with advantage (an ambush). A buff or a slide keeps the thief
    // hidden; a pickup bonus casting for the player does neither.
    let ambush = mods.lurking() && cx.power.is_none() && def.offensive();
    if ambush && let Some(f) = caster.fighter.as_mut() {
        f.unhide();
    }
    for effect in &def.effects {
        let act = match effect {
            Effect::Move { mode, distance } => {
                // Behind: through the picked target, `distance` past it.
                let total = match mode {
                    MoveMode::Behind => ((aimed.x - caster.x).powi(2) + (aimed.y - caster.y).powi(2)).sqrt() + distance.at(slv).max(0.0),
                    _ => distance.at(slv).max(0.0),
                };
                let to = moved(walk, caster.x, caster.y, aimed.dir, total, *mode);
                (caster.x, caster.y) = to;
                // What follows starts from where the caster ended up.
                (aimed.ox, aimed.oy) = to;
                continue;
            }
            Effect::Act(act) => act,
            Effect::Quiver(arrows) => {
                if let Some(f) = caster.fighter.as_mut() {
                    f.nock_next(arrows, slv);
                }
                continue;
            }
            Effect::Passive(_) | Effect::Mount | Effect::Pending(_) => continue,
        };
        let t = &act.timing;
        let load = match &act.payload {
            Payload::Damage(DamageFx { dice, extra, scale_k, mult, bonus, kind, save, knockback, arrow, flank, sure, tough, cart }) => {
                let (base, kind, to_hit) = match dice {
                    EffectDice::Fixed(d) => {
                        let mut d = scaled(*d, *scale_k, mult.at(slv) * strength, bonus.at(slv), cl);
                        if own {
                            d.bonus += mods.spell_power;
                        }
                        // Mystical Amplification: this cast is the next
                        // spell; the buff ends with it.
                        if mods.amplify > 0.0 && cx.power.is_none() {
                            d.bonus += amplified(d, mods.amplify);
                            if let Some(f) = caster.fighter.as_mut() {
                                f.spend_amplify();
                            }
                        }
                        (d, *kind, None)
                    }
                    EffectDice::Weapon => {
                        let (d, k) = weapon.damage(class, cl);
                        let d = if own { mods.weapon_dice(d) } else { d };
                        let to_hit = weapon.to_hit(cl) + mods.to_hit;
                        (Dice { bonus: d.bonus + bonus.at(slv).round() as i32 + mods.damage_flat, ..d }, k, Some(to_hit))
                    }
                };
                // Unpaid: a plain swing, no extra dice (Mammonite).
                let extra = extra.filter(|_| paid).and_then(|e| {
                    let n = (e.count as f32 * mult.at(slv) * strength).round() as u32;
                    (n > 0).then_some(Dice { count: n, ..e })
                });
                // A loaded cart hits harder: the dice count more.
                let heavy = if paid { (cart.at(slv) * loaded).clamp(0.0, 1.0) } else { 0.0 };
                let load_up = |d: Dice| Dice { count: ((d.count as f32) * (1.0 + heavy)).round().max(1.0) as u32, ..d };
                let (base, extra) = if heavy > 0.0 { (load_up(base), extra.map(load_up)) } else { (base, extra) };
                let save = save.as_ref().map(|sv| (sv.condition, ticks16(sv.secs.at(slv), hz)));
                // An arrow takes the element of the one nocked.
                let kind = if *arrow { nocked.unwrap_or(kind) } else { kind };
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
                    // Riders of the weapon: Enchant Poison, the ambush.
                    venom: (to_hit.is_some() && mods.venom > 0).then(|| (mods.venom, ticks16(VENOM_SECS, hz))),
                    advantage: ambush && to_hit.is_some(),
                    flank: Flank { more: flank.at(slv).max(0.0), from: (caster.x, caster.y), ambush },
                    sure: *sure,
                    tough: tough.at(slv).max(0.0) * strength,
                };
                Load::Damage { blow, knock: knockback.at(slv) }
            }
            Payload::Heal(HealFx { dice, scale_k, mult }) => {
                let mut d = scaled(*dice, *scale_k, mult.at(slv), 0.0, cl);
                if own {
                    d.bonus += mods.heal_power;
                }
                Load::Heal(d)
            }
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
            // Monster skills only (the skill table refuses it on players').
            Payload::Rally(_) => continue,
            Payload::Dispel => Load::Dispel,
            // Out at once, wherever it was aimed (a pickup bonus never
            // summons).
            Payload::Summon { what } => {
                if own && let Some(what) = Summon::from_id(what) {
                    let owner =
                        Owner { uid, entity: caster.profile.character_id, level: own_level, max_hp, to_hit: weapon.to_hit(own_level) + mods.to_hit, tick: now };
                    if companions.summon(owner, what, slv, aimed.x, aimed.y) == Summoned::FieldPlant {
                        spent.plants.push((uid, aimed.x, aimed.y, slv));
                    }
                }
                continue;
            }
            // Learning Potion makes the throws stronger too.
            Payload::Pitch { .. } if !own => continue,
            Payload::Pitch { mult, secs } => Load::Pitch { mult: mult.at(slv).max(0.0) * (1.0 + mods.potion.max(0.0)), ticks: ticks_of(*secs, hz).max(1) },
        };
        let (area, point) = area_of(act, slv, def.range, &aimed, caster.x, caster.y);
        let lasting = t.lasting(slv);
        let id = if lasting {
            zone += 1;
            zone - 1
        } else {
            0
        };
        // A self skill's friendly payload lands on the caster (an escape's
        // too: Tunnel Drive hides where it ends).
        let ally = aimed.ally.or_else(|| (matches!(def.target, Target::Caster | Target::Escape) && !act.payload.hostile()).then_some(uid));
        let follow = match act.area.follow {
            FollowFx::Caster => Follow::Player(uid),
            FollowFx::Target => match (aimed.monster, aimed.ally, aimed.pet) {
                (Some((s, g)), ..) => Follow::Monster(s, g),
                (None, Some(a), _) => Follow::Player(a),
                (None, None, Some(e)) => Follow::Companion(e),
                _ => Follow::None,
            },
            // A point skill still lands on the monster it picked (or the
            // homunculus a heal or a potion picked).
            FollowFx::None => match (aimed.monster, aimed.pet) {
                (Some((s, g)), _) if point => Follow::Monster(s, g),
                (None, Some(e)) if point => Follow::Companion(e),
                _ => Follow::None,
            },
        };
        // A piercing arrow: one homing hit per monster on the line, due
        // when the arrow gets there (none twice, none missed for walking
        // on), on a small box facing the flight (the knockback's way).
        if pierces(act, slv)
            && let Area::Rect { dir, .. } = area
        {
            let mut on = Vec::new();
            area.monsters(horde, &mut on);
            let store = horde.store();
            for s in on {
                let (mx, my) = (store.x[s as usize], store.y[s as usize]);
                let along = ((mx - aimed.ox) * dir.0 + (my - aimed.oy) * dir.1).max(0.0);
                let at = now + ticks_of(along / t.speed, hz);
                lives.push(Live {
                    id: 0,
                    map: caster.map,
                    skill: cx.skill,
                    caster: uid,
                    next: at,
                    until: at,
                    pulse: 0,
                    area: Area::Rect { x: mx, y: my, dir, half_len: 0.5, half_wid: 0.5 },
                    follow: Follow::Monster(s, store.generation[s as usize]),
                    point: true,
                    trap: None,
                    limit: 0,
                    chain: (0, 0.0),
                    ally: None,
                    load: load.clone(),
                });
            }
            continue;
        }
        let n = count_of(act, slv);
        let spread = if act.area.spread { spread_targets(horde, &aimed, def.range, n) } else { Vec::new() };
        let knots = if act.area.seek {
            let (x, y) = area.centre();
            let r = match area {
                Area::Circle { r, .. } => r,
                _ => 1.0,
            };
            seek_knots(horde, x, y, act.area.scatter, r, n)
        } else {
            Vec::new()
        };
        for i in 0..n {
            let mut area = area;
            let mut follow = follow;
            if let Some(&Some((kx, ky))) = knots.get(i as usize) {
                area = area.moved_to(kx, ky);
            } else if act.area.scatter > 0.0 {
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
            let at = now + ticks_of(i as f32 * t.every + flight(act, &aimed, x, y) + t.fall, hz);
            // A seeking hit's spot is shown before it lands.
            if act.area.seek
                && let Some(f) = caster.fighter.as_mut()
            {
                let r = if let Area::Circle { r, .. } = area { r } else { 0.0 };
                f.strikes.push((cx.skill, r, x, y, ((at - now) as f32 * 1000.0 / hz).round() as u32));
            }
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

/// Pays `cost` zeny from the caster's bag (the hub's copy): false when it
/// holds less.
fn pay(caster: &mut HubPlayer, cost: u64, spent: &mut Spent) -> bool {
    let bag = &mut caster.profile.bag;
    if bag.zeny < cost {
        return false;
    }
    bag.zeny -= cost;
    spent.zeny.push((caster.profile.user_id, cost));
    true
}

/// Uses up one cast's reagents from the caster's bag (the hub's copy): all
/// of them or none.
fn use_reagents(caster: &mut HubPlayer, reagents: &[(String, u32)], spent: &mut Spent) -> bool {
    let bag = &mut caster.profile.bag;
    if reagents.iter().any(|(id, n)| bag.count(id) < *n) {
        return false;
    }
    for (id, n) in reagents {
        let Some(e) = items::find(id) else { return false };
        if bag.take(id, *n).is_err() {
            return false;
        }
        for _ in 0..*n {
            spent.items.push((caster.profile.user_id, e.id));
        }
    }
    true
}

/// The potion Potion Pitcher throws from `potions` (id, count) for a
/// wound of `want` HP, each worth `mult` × its average: the weakest that
/// covers it, else the strongest (`best_potion`'s rule). Healing ones only.
pub(super) fn pitch_pick(potions: &[(String, u32)], want: i32, mult: f32) -> Option<(usize, &'static str, Dice)> {
    let mut best: Option<(usize, &'static str, Dice, i32)> = None;
    for (i, (id, n)) in potions.iter().enumerate() {
        let Some(e) = items::find(id).filter(|_| *n > 0) else { continue };
        let ItemKind::Consumable(items::Effect::Heal(d)) = e.kind else { continue };
        let value = ((d.average_x2() as f32 / 2.0) * mult).round() as i32;
        let better = match best {
            None => true,
            Some((.., bv)) => match (value >= want, bv >= want) {
                (true, false) => true,
                (false, true) => false,
                (covers, _) => value != bv && (value < bv) == covers,
            },
        };
        if better {
            best = Some((i, e.id, d, value));
        }
    }
    best.map(|(i, id, d, _)| (i, id, d))
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
        MoveMode::Blink | MoveMode::Behind => {
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

/// Where each of `n` seeking hits lands (`seek`): the densest knot of
/// monsters (most of them within `r` of one of them) inside `reach` of
/// `(x, y)`, at the knot's centre (kept inside the reach); a knot taken by
/// one hit is left to the next while another one is left, then they start
/// over from the densest. `None`: no monster in reach (the hit scatters).
pub(super) fn seek_knots(horde: &Horde, x: f32, y: f32, reach: f32, r: f32, n: u32) -> Vec<Option<(f32, f32)>> {
    /// Candidates looked at (the nearest), bounding the O(m²) count.
    const MAX: usize = 64;
    let mut slots = Vec::new();
    Area::Circle { x, y, r: reach }.monsters(horde, &mut slots);
    let store = horde.store();
    slots.retain(|&s| !horde.is_plant(s) && store.hp[s as usize] > 0);
    let pos = |s: u16| (store.x[s as usize], store.y[s as usize]);
    let d2 = |s: u16| (pos(s).0 - x).powi(2) + (pos(s).1 - y).powi(2);
    slots.sort_by(|&a, &b| d2(a).total_cmp(&d2(b)).then(a.cmp(&b)));
    slots.truncate(MAX);
    let pts: Vec<(f32, f32)> = slots.iter().map(|&s| pos(s)).collect();
    let r2 = r * r;
    let mut taken = vec![false; pts.len()];
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        if pts.is_empty() {
            out.push(None);
            continue;
        }
        if taken.iter().all(|&t| t) {
            taken.iter_mut().for_each(|t| *t = false);
        }
        let near = |c: usize, taken: &[bool]| {
            pts.iter().enumerate().filter(|&(j, p)| !taken[j] && (p.0 - pts[c].0).powi(2) + (p.1 - pts[c].1).powi(2) <= r2).count()
        };
        // The densest free knot (ties: nearest the point, as sorted).
        let best = (0..pts.len()).filter(|&c| !taken[c]).max_by(|&a, &b| near(a, &taken).cmp(&near(b, &taken)).then(b.cmp(&a)));
        let Some(c) = best else {
            out.push(None);
            continue;
        };
        let (mut sx, mut sy, mut k) = (0.0, 0.0, 0.0);
        for (j, p) in pts.iter().enumerate() {
            if !taken[j] && (p.0 - pts[c].0).powi(2) + (p.1 - pts[c].1).powi(2) <= r2 {
                (sx, sy, k) = (sx + p.0, sy + p.1, k + 1.0);
                taken[j] = true;
            }
        }
        let (mut kx, mut ky) = (sx / k, sy / k);
        let off = ((kx - x).powi(2) + (ky - y).powi(2)).sqrt();
        if off > reach && off > 0.0 {
            (kx, ky) = (x + (kx - x) * reach / off, y + (ky - y) * reach / off);
        }
        out.push(Some((kx, ky)));
    }
    out
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
        let mut spent = Spent::default();
        let second = (hz.round() as u32).max(1);

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
            // A draining toggle (Maximize Power) takes its SP once a
            // second and goes off when the SP runs out.
            if f.mods.sp_drain > 0.0 && (now + uid).is_multiple_of(second) {
                let cost = f.mods.sp_drain.round().max(1.0) as i32;
                if f.sp >= cost {
                    f.sp -= cost;
                } else {
                    let draining: Vec<u16> = f.buff_list(now).map(|(s, _)| s).filter(|&s| drains(book.get(s))).collect();
                    for s in draining {
                        f.toggle(s, Mods::NONE);
                    }
                }
            }
            for cd in f.cooldowns.iter_mut().filter(|c| **c > 0) {
                *cd -= 1;
            }
            f.recovery = f.recovery.saturating_sub(1);
            // A channel holds while its slot is held and the caster stands.
            if let Some(mut ch) = f.channel {
                ch.idle = if pressed & (1 << ch.slot) != 0 { 0 } else { ch.idle.saturating_add(1) };
                let lasting = self.lives.iter().any(|l| l.caster == uid && l.skill == ch.skill);
                if moving || ch.idle > CHANNEL_GRACE || !f.is_up() || f.status.held() || f.status.silenced() || !lasting {
                    f.channel = None;
                    broken.push((uid, ch.skill));
                } else {
                    f.channel = Some(ch);
                    continue;
                }
            }
            // Down, stunned, frozen, turned to stone, terrified or
            // silenced: no casting (basic attacks go on while silenced).
            if !f.is_up() || f.status.mute() {
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
                    if f.sp >= cost || f.free_cast().0 {
                        let rng = &mut self.dice;
                        let (free_sp, free_cd) = f.card_cast(hz, || rng.die(100));
                        if !free_sp {
                            f.sp -= cost;
                        }
                        f.cooldowns[c.skill as usize] = if free_cd { 0 } else { (def.cooldown_secs(c.level) * hz).round() as u32 };
                        f.recovery = (RECOVERY_SECS * hz).round() as u32;
                        f.quiet = 0;
                        let walk = &self.world.map(map).walk;
                        let cx = Landing { def, skill: c.skill, slv: c.level, now, hz, zone: c.zone, power: None };
                        land(&cx, &mut self.players[idx], c.aimed, horde, walk, &mut self.dice, &mut self.lives, &mut self.companions, &mut spent);
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
            // dead allows nothing; a thief's hiding allows a strike out of it
            // (`land` ends the hiding).
            let Some(f) = self.players[idx].fighter.as_ref() else { continue };
            if f.mods.hidden && !f.mods.ambush {
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
                    // Mounting is MOUNT_REQ's; riding and spear skills
                    // need the mount and the spear.
                    || def.mounts()
                    || !def.usable_with(f.riding, f.weapon.kind)
                    || busy
                    || !rooted_ok
                    || f.cooldowns[skill as usize] > 0
                    || (f.sp < def.sp_cost(level) && !f.free_cast().0)
                {
                    continue;
                }
                let field = Field { horde, players: &self.players, companions: &self.companions, map };
                // An auto-cast slot fires only at a target that needs it and
                // aims itself; a press by hand follows the pointer when the
                // player aims manually.
                let prefs = self.players[idx].prefs;
                let auto = prefs.auto & (1 << slot) != 0;
                // Hidden, nothing fires by itself (it would give the thief
                // away); nor does Arrow Change, which only cycles.
                if auto && (f.mods.hidden || def.never_auto()) {
                    continue;
                }
                let press = Press {
                    skill: Some(skill),
                    now,
                    refresh: ticks_of(REFRESH_SECS, hz),
                    auto,
                    manual: prefs.manual_aim && !auto,
                };
                let me = &self.players[idx];
                // A summon pressed by hand with nothing to aim at comes out
                // beside the caster (the homunculus at the stage's start).
                let beside = |x: f32, y: f32| Aimed { x, y, ox: me.x, oy: me.y, dir: (0.0, 1.0), ally: None, monster: None, pet: None };
                // A potion throw with no wounded player or homunculus in
                // reach goes to the caster's most wounded companion (a
                // plant; auto-cast: below the potion threshold too).
                let pet = || {
                    let share = |c: &&super::companion::Companion| c.hp as f32 / c.max_hp.max(1) as f32;
                    self.companions
                        .of(uid)
                        .filter(|c| c.alive() && c.hp < c.max_hp && (c.x - me.x).powi(2) + (c.y - me.y).powi(2) <= def.range * def.range)
                        .filter(|c| !auto || share(c) * 100.0 < me.uses.hp as f32)
                        .min_by(|a, b| share(a).total_cmp(&share(b)))
                        .map(|c| beside(c.x, c.y))
                };
                let fallback = || match () {
                    _ if def.pitches() => pet(),
                    _ if !auto && def.summons().is_some() => Some(beside(me.x, me.y)),
                    _ => None,
                };
                let Some(aimed) = aim_press(&field, me, def, level, &press).or_else(fallback) else { continue };
                // Auto-cast throws potions only below the caster's own
                // potion threshold (`USE_PREFS`), as it drinks them.
                if auto && def.pitches() && (aimed.ally.is_some() || aimed.pet.is_some()) && !pitch_wanted(&self.players, &self.companions, me, &aimed) {
                    continue;
                }
                let cast_ticks = (def.cast_secs(level) * hz).round() as u32;
                let cooldown_ms = (def.cooldown_secs(level) * 1000.0).round() as u32;
                let zone = self.next_zone;
                self.next_zone = self.next_zone.wrapping_add(zone_ids(def, level)).max(1);
                let areas = telegraphs(def, level, &self.players[idx], &aimed, zone, &self.players);
                let p = &mut self.players[idx];
                if def.travels() {
                    // A travel skill faces where it goes.
                    p.facing = facing_of(aimed.dir.0, aimed.dir.1);
                } else if def.target != Target::Escape && (aimed.x, aimed.y) != (p.x, p.y) {
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
                    let rng = &mut self.dice;
                    let (free_sp, free_cd) = f.card_cast(hz, || rng.die(100));
                    if !free_sp {
                        f.sp -= def.sp_cost(level);
                    }
                    f.cooldowns[skill as usize] = if free_cd { 0 } else { (def.cooldown_secs(level) * hz).round() as u32 };
                    if !def.mobile {
                        f.recovery = (RECOVERY_SECS * hz).round() as u32;
                    }
                    f.quiet = 0;
                    let walk = &self.world.map(map).walk;
                    let cx = Landing { def, skill, slv: level, now, hz, zone, power: None };
                    land(&cx, &mut self.players[idx], aimed, horde, walk, &mut self.dice, &mut self.lives, &mut self.companions, &mut spent);
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
            ended.extend(self.lives.extract_if(.., |l| broken.contains(&(l.caster, l.skill))).filter(|l| l.id != 0).map(|l| l.id));
        }
        let mut lives = std::mem::take(&mut self.lives);
        let mut dispels: Vec<Area> = Vec::new();
        // Spells meeting the map's hazard ground (`env.rs` GROUND_MEETS):
        // fire fizzles out on sewage, cold freezes it over or quenches lava.
        let mut sewage: Vec<(DamageType, Area)> = Vec::new();
        let env = &self.env;
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
            if let Load::Damage { blow, .. } = &l.load
                && super::env::GROUND_MEETS.iter().any(|m| m.kind == blow.damage.kind)
                && l.caster != 0
            {
                let (x, y) = l.area.centre();
                if let Some(meet) = env.ground_meet(x, y, blow.damage.kind) {
                    sewage.push((blow.damage.kind, l.area));
                    // Burning ground cannot burn on sewage: it goes out.
                    if meet.douse && l.pulse > 0 {
                        ended.push(l.id);
                        return false;
                    }
                }
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
                Follow::Companion(e) => match self.companions.by_entity(e) {
                    Some(c) if c.alive() => l.area = l.area.moved_to(c.x, c.y),
                    // It fell on the way: the heal lands on nobody.
                    _ => l.follow = Follow::None,
                },
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
                act(l, horde, &mut self.players, &mut self.companions, &mut spent, &mut self.dice, walk, now, hits, kills);
                credit(&mut self.run, l, &hits[h0..], now, secs, horde);
                ended.push(l.id);
                return false;
            }
            let h0 = hits.len();
            act(l, horde, &mut self.players, &mut self.companions, &mut spent, &mut self.dice, walk, now, hits, kills);
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
        self.ground_meets(&sewage, &mut casts);
        for area in dispels {
            ended.extend(self.dispel_hazards(map, &area));
        }

        self.report_spent(spent, out);

        // Seeking hits: where each one falls, told as the cast lands.
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let entity = p.profile.character_id;
            let Some(f) = p.fighter.as_mut().filter(|f| !f.strikes.is_empty()) else { continue };
            let mut list = std::mem::take(&mut f.strikes);
            while let Some(&(skill, radius, ..)) = list.first() {
                let strikes = list.iter().filter(|s| s.0 == skill).map(|s| (s.2, s.3, s.4)).collect();
                list.retain(|s| s.0 != skill);
                casts.push(ServerMsg::SkillStrikes { caster: entity, skill: book.get(skill).id.clone(), radius, strikes });
            }
        }

        let recipients: Vec<u32> = self.players.iter().filter(|p| p.map == map).map(|p| p.profile.user_id).collect();
        for msg in casts.into_iter().chain(ended.into_iter().filter(|&id| id != 0).map(|id| ServerMsg::ZoneEnd { id })) {
            for &uid in &recipients {
                out.push((uid, HubEvent::Control(msg.clone())));
            }
        }
    }
}

/// A toggle that drains SP (Maximize Power).
fn drains(def: &SkillDef) -> bool {
    def.effects.iter().filter_map(Effect::act).any(|a| matches!(&a.payload, Payload::Buff(b) if b.toggle && b.sp_drain.at(def.max_level) > 0.0))
}

/// An auto-cast potion throw is wanted: the picked ally (or homunculus)
/// is below the caster's HP potion threshold (none set: never by itself).
fn pitch_wanted(players: &[HubPlayer], companions: &Companions, me: &HubPlayer, aimed: &Aimed) -> bool {
    let pct = me.uses.hp;
    let vitals = match aimed.pet {
        Some(e) => companions.by_entity(e).map(|c| (c.hp, c.max_hp)),
        None => aimed.ally.and_then(|a| players.iter().find(|p| p.profile.user_id == a)).and_then(|p| p.fighter.as_ref()).map(|f| (f.hp, f.max_hp)),
    };
    let Some((hp, max_hp)) = vitals else { return false };
    pct > 0 && i64::from(hp) * 100 < i64::from(max_hp) * i64::from(pct)
}

impl HubInstance {
    /// Tells the socket tasks what casts took from the bags (the hub's
    /// copies already paid), and grows the field plants Plant Cultivation
    /// asked for (at most [`MAX_GROWN`] per caster: the oldest goes).
    pub(super) fn report_spent(&mut self, spent: Spent, out: &mut Outgoing) {
        for (uid, amount) in spent.zeny {
            out.push((uid, HubEvent::ZenySpent { amount }));
        }
        for (uid, item) in spent.items {
            out.push((uid, HubEvent::ItemUsed { item: item.into() }));
        }
        let Some(horde) = self.horde.as_mut() else { return };
        for (uid, x, y, slv) in spent.plants {
            let Some(f) = self.players.iter_mut().find(|p| p.profile.user_id == uid).and_then(|p| p.fighter.as_mut()) else { continue };
            let Some(kind) = cultivated(horde, slv, self.dice.die(100)) else { continue };
            let st = horde.store();
            f.grown.retain(|&(s, g)| st.is_alive(s) && st.generation[s as usize] == g && st.is_plant(s));
            if f.grown.len() >= MAX_GROWN {
                let (s, _) = f.grown.remove(0);
                horde.despawn(s);
            }
            if let Some(s) = horde.place_object(kind, x, y) {
                f.grown.push((s, horde.store().generation[s as usize]));
            }
        }
    }
}

/// What Plant Cultivation grows at skill level `slv` (`roll`: a d100): a
/// mushroom first (spores for the alchemist's bottles), then herb plants,
/// the common ones likelier. `None` when the stage knows no such plant.
pub(super) fn cultivated(horde: &Horde, slv: u8, roll: u32) -> Option<u8> {
    const HERBS: [(&str, u32); 5] = [("green_plant", 35), ("red_plant", 30), ("yellow_plant", 20), ("white_plant", 10), ("blue_plant", 5)];
    let id = if slv <= 1 {
        "black_mushroom"
    } else {
        let mut left = roll.clamp(1, 100);
        HERBS.iter().find(|(_, w)| {
            if left <= *w {
                return true;
            }
            left -= w;
            false
        })?.0
    };
    horde.catalog().kind_of(id).filter(|&k| horde.catalog().get(k).is_plant())
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
fn act(
    l: &mut Live,
    horde: &mut Horde,
    players: &mut [HubPlayer],
    companions: &mut Companions,
    spent: &mut Spent,
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
            let reflected = strike(horde, rng, &targets, blow, hits, kills);
            if reflected > 0
                && let Some(p) = players.iter_mut().find(|p| p.profile.user_id == blow.caster)
                && let Some(f) = p.fighter.as_mut()
                && f.is_up()
            {
                // A shell that turns spells back: half of it, on the caster
                // (it stings, but never downs them by itself).
                let taken = f.take_from(reflected, false, false);
                f.hp = f.hp.max(1);
                hits.push(HitEvent { target: p.profile.character_id, amount: taken as u32, flags: (blow.damage.kind as u8) << 4 });
            }
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
            // A heal over an area mends the companions in it too; one on a
            // picked homunculus mends it.
            if let (true, Follow::Companion(e)) = (l.point, l.follow) {
                companions.heal_entity(e, dice.roll(rng, false).max(0));
            } else if !l.point && !companions.is_empty() {
                companions.heal_in(&l.area, dice.roll(rng, false).max(0));
            }
        }
        Load::Pitch { mult, ticks } => pitch(l, players, companions, spent, rng, now, mult, ticks),
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

/// Potion Pitcher: one potion out of the caster's carried ones, picked for
/// the worst wound among those it reaches, restores `mult` × its roll over
/// `ticks` to each of them (a stronger potion already working is kept, the
/// throw then wasted on that one) and mends the companions inside.
fn pitch(l: &Live, players: &mut [HubPlayer], companions: &mut Companions, spent: &mut Spent, rng: &mut Rng, now: u32, mult: f32, ticks: u32) {
    let targets = friends(l, players);
    // A single throw at a companion lands on it: at least a body around.
    let reach = match l.area {
        Area::Circle { x, y, r } => Area::Circle { x, y, r: r.max(1.0) },
        a => a,
    };
    let pet_want = companions.units().iter().filter(|c| c.alive() && reach.contains(c.x, c.y, BODY)).map(|c| c.max_hp - c.hp).max().unwrap_or(0);
    let hurt_pet = pet_want > 0;
    let want = targets.iter().filter_map(|&i| players[i].fighter.as_ref()).map(|f| f.max_hp - f.hp).max().unwrap_or(0).max(pet_want);
    if want <= 0 {
        return;
    }
    let Some(ci) = players.iter().position(|p| p.profile.user_id == l.caster) else { return };
    let Some(f) = players[ci].fighter.as_mut() else { return };
    let Some((slot, item, dice)) = pitch_pick(&f.potions, want.max(1), mult) else { return };
    f.potions[slot].1 -= 1;
    spent.items.push((l.caster, item));
    let total = ((dice.roll(rng, false).max(1) as f32) * mult).round() as i32;
    for i in targets {
        let Some(f) = players[i].fighter.as_mut() else { continue };
        if f.regen[0].as_ref().is_some_and(|r| !r.beaten_by(total, ticks)) {
            continue;
        }
        f.regen[0] = Some(super::shop::Regen { item, left: total, total, ticks, span: ticks, shown: 0, started: now });
    }
    if hurt_pet {
        companions.heal_in(&reach, total);
    }
}

/// Seconds an envenomed weapon's poison lasts (Enchant Poison).
pub(super) const VENOM_SECS: f32 = 4.0;

/// An envenomed weapon hit: Poisoned for `ticks` (the longer timer is
/// kept, never stacked; plants shrug it off).
pub(super) fn envenom(horde: &mut Horde, s: u16, ticks: u16) {
    horde.store_mut().afflict(s, Condition::Poisoned, ticks);
}

/// Puts a condition on a monster; fire remembers the strongest `k` lit.
pub(super) fn inflict(horde: &mut Horde, s: u16, c: Condition, ticks: u16, k: u32) {
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
        // Burrowed: out of reach of everything, ailments included.
        if !status.ailing() || !(now + s as u32).is_multiple_of(second) || horde.store().burrowed(s) {
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
    // Affixes: the shield soaks, Armoured cuts, Thorns tallies (affix.rs).
    // A rally's spur (Shelter): less damage while it lasts.
    let amount = horde.store().rally_of(s).soak(amount);
    let amount = super::affix::absorb(horde, s, amount, damage.kind);
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
    /// Percent chance a weapon hit poisons (Enchant Poison), and for how
    /// many ticks.
    venom: Option<(u32, u16)>,
    /// The weapon roll has advantage (struck out of hiding).
    advantage: bool,
    /// More dice from behind or out of hiding (Back Stab).
    flank: Flank,
    /// No attack roll and never halved; the save only decides the rider.
    sure: bool,
    /// Also this fraction of the target's maximum HP (capped, `TOUGH_CAP`).
    tough: f32,
}

/// Back Stab's bonus: `more` × the dice count when struck out of hiding
/// (`ambush`) or from behind a monster that moves away from `from` (where
/// the caster stood when it landed).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Flank {
    pub more: f32,
    pub from: (f32, f32),
    pub ambush: bool,
}

/// A monster moving at least this fast (cells per tick, its damped
/// velocity) has a back to strike.
const BACK_SPEED: f32 = 0.002;

impl Flank {
    /// The blow on monster `s` lands from behind or out of hiding.
    pub(super) fn holds(&self, horde: &Horde, s: u16) -> bool {
        if self.more <= 0.0 {
            return false;
        }
        if self.ambush {
            return true;
        }
        let st = horde.store();
        let (vx, vy) = (st.vel_x[s as usize], st.vel_y[s as usize]);
        let (ax, ay) = (self.from.0 - st.x[s as usize], self.from.1 - st.y[s as usize]);
        vx * vx + vy * vy >= BACK_SPEED * BACK_SPEED && vx * ax + vy * ay < 0.0
    }

    /// `d` with its count raised by the bonus.
    fn raise(&self, d: Dice) -> Dice {
        Dice { count: ((d.count as f32) * (1.0 + self.more)).round().max(1.0) as u32, ..d }
    }
}

/// One hit on each of `targets`. Returns the damage turned back on the
/// caster by reflecting monsters ([`FLAG_REFLECT`]: a spell, i.e. a blow
/// without a weapon roll, does nothing to them and half of it comes back).
fn strike(horde: &mut Horde, rng: &mut Rng, targets: &[u16], blow: Blow, hits: &mut Vec<HitEvent>, kills: &mut Vec<Kill>) -> i32 {
    let mut reflected = 0;
    for &s in targets {
        // Dead, or dived since it was aimed at.
        if horde.store().hp[s as usize] <= 0 || horde.store().burrowed(s) {
            continue;
        }
        let id = MONSTER_ENTITY_BASE + s as u32;
        let (crit, saved) = match blow.to_hit {
            // Acid: it lands whatever the armour; the save is for the rider.
            _ if blow.sure => (false, resist(horde, rng, s, blow.dc, blow.save.is_some())),
            Some(bonus) => {
                let ac = horde.block(s).ac + horde.store().status_of(s).ac();
                let advantage = (horde.store().status_of(s).advantage_against() + blow.advantage as i8).clamp(-1, 1);
                match attack_roll(rng, bonus, ac, advantage, 20) {
                    AttackOutcome::Miss => {
                        hits.push(HitEvent { target: id, amount: 0, flags: hit::MISS });
                        continue;
                    }
                    o => (o == AttackOutcome::Critical && !horde.is_plant(s), false),
                }
            }
            None => (false, resist(horde, rng, s, blow.dc, blow.save.is_some())),
        };
        if blow.to_hit.is_none() && horde.store().flags[s as usize] & FLAG_REFLECT != 0 {
            let element = horde.catalog().get(horde.store().kind[s as usize]).element;
            let mut amount = blow.damage.resolve(rng, false, saved, element, 0);
            if let Some(e) = blow.extra {
                amount += Damage::new(e, blow.damage.kind).resolve(rng, false, saved, element, 0);
            }
            reflected += amount / 2;
            hits.push(HitEvent { target: id, amount: 0, flags: hit::GUARD });
            continue;
        }
        // From behind or out of hiding: more dice (Back Stab).
        let (damage, extra) = if blow.flank.holds(horde, s) {
            (Damage { dice: blow.flank.raise(blow.damage.dice), ..blow.damage }, blow.extra.map(|e| blow.flank.raise(e)))
        } else {
            (blow.damage, blow.extra)
        };
        let mut amount = wound(horde, rng, s, damage, extra, crit, saved && !blow.sure, blow.k);
        // What is tough melts faster: a share of its maximum HP, at most a
        // few times what the dice did (once on a boss); a plant takes its
        // one HP and no more.
        if blow.tough > 0.0 && amount > 0 && !horde.is_plant(s) && horde.store().hp[s as usize] > 0 {
            let cap = if horde.store().flags[s as usize] & FLAG_BOSS != 0 { 1.0 } else { TOUGH_CAP };
            let more = ((horde.block(s).hp as f32 * blow.tough).round() as i32).min((amount as f32 * cap) as i32).max(0);
            horde.store_mut().hp[s as usize] -= more;
            amount += more;
        }
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
        } else {
            if let Some((condition, ticks)) = blow.save {
                // A weapon swing that hit still allows a save against its rider.
                let failed = if blow.to_hit.is_some() && !blow.sure { !resist(horde, rng, s, blow.dc, true) } else { !saved };
                if failed {
                    inflict(horde, s, condition, ticks, blow.k);
                    if condition == Condition::Taunted {
                        horde.store_mut().taunter[s as usize] = blow.caster;
                    }
                }
            }
            if let Some((chance, ticks)) = blow.venom
                && amount > 0
                && rng.die(100) <= chance
            {
                envenom(horde, s, ticks);
            }
        }
        hits.push(HitEvent { target: id, amount: amount as u32, flags });
    }
    reflected
}

/// Learned levels, hotbar and passives of a fighter from a loadout.
pub(super) fn equip(f: &mut Fighter, book: &SkillBook, loadout: &crate::world::combat::Loadout) {
    f.skills = vec![0; book.len()];
    f.cooldowns.resize(book.len(), 0);
    for (i, def) in book.iter() {
        let lvl = loadout.level(&def.id).min(def.max_level);
        if lvl > 0 && def.usable_by(f.class) {
            f.skills[i as usize] = lvl;
        }
    }
    // Riding passives count while mounted, weapon ones with that weapon.
    f.riding = book.rides(loadout);
    f.set_passive(book.passives(f.class, loadout, f.weapon.kind));
    for (slot, h) in f.hotbar.iter_mut().enumerate() {
        *h = book.find(loadout.slot(slot)).map(|(i, _)| i).filter(|&i| f.skills[i as usize] > 0);
    }
    // The travel key: the learned movement skill, the higher tier first
    // (later in the book), then the higher level.
    f.travel = book
        .iter()
        .filter(|(i, def)| f.skills[*i as usize] > 0 && !def.passive && def.travels())
        .max_by_key(|(i, _)| (*i, f.skills[*i as usize]))
        .map(|(i, _)| i);
}

/// Bit of `Input::slots` for the travel key (past the hotbar's slots).
pub const TRAVEL_SLOT: usize = 15;
