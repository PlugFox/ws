//! Monster skills (COMBAT.md §4.4): the same area × timing × payload
//! effects as players' skills, landing on players.
//!
//! A monster near a player checks its skills about once a second (staggered
//! by slot); a ready skill whose range holds a player and whose chance
//! rolls starts a wind-up: the monster stands still, the party sees the
//! telegraph (`CAST` from the monster's entity id), and the effect lands at
//! the end as [`Hazard`]s. Damage is a saving throw for half against
//! `10 + the monster's save bonus`; ranged skills miss players under a
//! ranged ward (Pneuma).
//!
//! Behaviours (`horde/tactics.rs`) change how a kind uses its kit:
//! * a **kiter**'s ranged skill is its attack: it needs line of sight, its
//!   global cooldown is [`KITER_GCD_SECS`], its cooldowns are jittered so a
//!   pack does not loose in step, and at most [`MAX_VOLLEYS`] (+1 per
//!   extra player) kiter wind-ups are in the air at once;
//! * a **support** rallies when a player is within [`SUPPORT_TRIGGER`]
//!   cells and at least [`RALLY_MIN_PACK`] other monsters stand in its
//!   area (a spur over nobody is wasted);
//! * a **healer**'s heal aimed at `nearest` goes to the most wounded
//!   monster in range (under [`HEAL_BELOW`] of its HP, never a boss, one
//!   healer per patient): a beam from the healer to it, telegraphed in the
//!   heal look.

use super::aim::{norm, Aimed, BODY};
use super::area::Area;
use super::cast::{area_of, count_of, look_of, moved, scaled, ticks16, ticks_of, Kill, Live};
use super::fight::damage_type_bits;
use super::{HubEvent, HubInstance, HubPlayer, MapIdx, Outgoing};
use crate::world::combat::rules::saving_throw;
use crate::world::combat::skills::{Act, DamageFx, Effect, EffectDice, MobTarget, MonsterSkill, Payload, RallyFx, SkillBook};
use crate::world::combat::{Condition, DamageType, Dice, Rng};
use crate::world::horde::storage::{Rally, FLAG_BOSS, FLAG_FULL, FLAG_LOS};
use crate::world::horde::tactics::Behaviour;
use crate::world::horde::{Horde, LURE_BASE};
use super::companion::Companions;
use crate::world::map::WalkGrid;
use crate::world::protocol::{hit, look, BossZone, HitEvent, ServerMsg, Telegraph, MONSTER_ENTITY_BASE};

/// Skills a monster kind may carry.
pub(super) const MAX_MOB_SKILLS: usize = 4;
/// A monster looks at its skills this often (seconds).
const CHECK_SECS: f32 = 1.0;
/// After a skill lands a monster casts none of its skills for this long
/// (seconds): a kind with several skills chains them no faster than this.
const SKILL_GCD_SECS: f32 = 4.0;
/// A kiter's global skill cooldown: its shot is its attack.
const KITER_GCD_SECS: f32 = 1.5;
/// Kiter wind-ups in the air at once (+1 per extra player): the rest hold
/// and look again a second later, so a pack of archers never paints the
/// screen with lines.
const MAX_VOLLEYS: usize = 3;
/// A support rallies when a player is this close (cells) ...
const SUPPORT_TRIGGER: f32 = 20.0;
/// ... and this many other monsters stand in its rally.
const RALLY_MIN_PACK: usize = 2;
/// A healer's beam goes to monsters under this share of their HP.
const HEAL_BELOW: f32 = 0.8;

/// What one monster slot remembers about its skills.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Mind {
    /// The generation it belongs to (a reused slot starts fresh).
    generation: u8,
    fresh: bool,
    next_check: u32,
    /// In a wind-up, then the global skill cooldown, until this tick.
    busy_until: u32,
    /// Tick each skill is ready again.
    ready: [u32; MAX_MOB_SKILLS],
}

/// A monster's wind-up.
#[derive(Clone, Copy, Debug)]
pub(super) struct MobCast {
    pub(super) slot: u16,
    generation: u8,
    /// Index in [`SkillBook::monster`].
    skill: u16,
    land: u32,
    x: f32,
    y: f32,
    dir: (f32, f32),
    zone: u32,
    /// A heal beam's patient (slot, generation): one healer each.
    ally: Option<(u16, u8)>,
}

/// A monster effect in flight or on the ground.
#[derive(Clone, Debug)]
pub(super) struct Hazard {
    /// Telegraph id of lasting ground (0 = a one-off strike).
    pub id: u32,
    pub map: MapIdx,
    pub next: u32,
    pub until: u32,
    pub pulse: u32,
    pub area: Area,
    dc: i32,
    ranged: bool,
    /// Cast by an undead or a demon.
    evil: bool,
    load: HazardLoad,
}

impl Hazard {
    /// Damage landing on `area` at tick `at`, then lasting until `until`
    /// with a strike every `pulse` ticks (0: once); a saving throw against
    /// `dc` halves it (and spares the rider). For effects that are no
    /// monster skill (affixes, `affix.rs`).
    pub(super) fn damage(
        id: u32,
        map: MapIdx,
        area: Area,
        at: u32,
        until: u32,
        pulse: u32,
        dc: i32,
        dice: Dice,
        kind: DamageType,
        rider: Option<(Condition, u16)>,
    ) -> Hazard {
        Hazard { id, map, next: at, until: until.max(at), pulse, area, dc, ranged: false, evil: false, load: HazardLoad::Damage { dice, kind, rider } }
    }

    /// A condition on whoever stands in `area` (a save against `dc` resists).
    pub(super) fn condition(id: u32, map: MapIdx, area: Area, at: u32, until: u32, pulse: u32, dc: i32, condition: Condition, ticks: u16) -> Hazard {
        Hazard { id, map, next: at, until: until.max(at), pulse, area, dc, ranged: false, evil: false, load: HazardLoad::Condition { condition, ticks, save: true } }
    }
}

#[derive(Clone, Copy, Debug)]
enum HazardLoad {
    Damage { dice: Dice, kind: DamageType, rider: Option<(Condition, u16)> },
    Condition { condition: Condition, ticks: u16, save: bool },
}

/// Monster skill indices per kind (catalog order), resolved once.
pub(super) fn kind_skills(horde: &Horde, book: &SkillBook) -> Vec<Vec<u16>> {
    horde
        .catalog()
        .defs()
        .iter()
        .map(|d| {
            // Scripted elites and bosses fight by their script (boss.rs).
            if super::boss::scripted(&d.id) {
                return Vec::new();
            }
            let mut v: Vec<u16> = d.skills.iter().filter_map(|id| book.find_monster(id)).collect();
            if v.len() < d.skills.len() {
                tracing::warn!(monster = %d.id, "unknown monster skill ids skipped");
            }
            v.truncate(MAX_MOB_SKILLS);
            v
        })
        .collect()
}

/// Where the monster at `(mx, my)` ends up after the skill's moves, and the
/// areas it will strike (what the telegraphs show).
fn plan<'a>(sk: &'a MonsterSkill, walk: &WalkGrid, mx: f32, my: f32, aim: (f32, f32), dir: (f32, f32)) -> Vec<(Area, &'a Act)> {
    let (mut x, mut y) = (mx, my);
    let mut out = Vec::new();
    for e in &sk.effects {
        match e {
            Effect::Move { mode, distance } => (x, y) = moved(walk, x, y, dir, distance.at(1).max(0.0), *mode),
            Effect::Act(act) => {
                let aimed = Aimed { x: aim.0, y: aim.1, ox: x, oy: y, dir, ally: None, monster: None, pet: None };
                out.push((area_of(act, 1, sk.range, &aimed, x, y).0, &**act));
            }
            Effect::Passive(_) | Effect::Mount | Effect::Quiver(_) | Effect::Pending(_) => {}
        }
    }
    out
}

fn telegraphs(planned: &[(Area, &Act)], zone: u32) -> Vec<Telegraph> {
    let mut id = zone;
    planned
        .iter()
        .map(|(area, act)| {
            let t = &act.timing;
            let lasting = t.lasting(1);
            let n = count_of(act, 1);
            let span = if lasting { t.secs.at(1) } else { (n - 1) as f32 * t.every };
            let mut area = *area;
            if act.area.scatter > 0.0
                && let Area::Circle { x, y, r } = area
            {
                area = Area::Circle { x, y, r: r + act.area.scatter };
            }
            let tid = if lasting {
                id += 1;
                id - 1
            } else {
                0
            };
            area.telegraph(tid, telegraph_look(act), (span * 1000.0).round() as u32, 0)
        })
        .collect()
}

/// A monster act's telegraph look: a heal (a rally that heals) in the heal
/// colour, so a healer's beam reads as one; else the act's own.
fn telegraph_look(act: &Act) -> u8 {
    match &act.payload {
        Payload::Rally(r) if r.heal > 0.0 => look::HEAL,
        _ => look_of(act),
    }
}

/// A monster skill's rally, when it has one.
fn rally_fx(sk: &MonsterSkill) -> Option<&RallyFx> {
    sk.effects.iter().filter_map(Effect::act).find_map(|a| if let Payload::Rally(r) = &a.payload { Some(r) } else { None })
}

/// The most wounded monster a healer at `(x, y)` can beam within `range`
/// (cells): under [`HEAL_BELOW`] of its HP, alive, above ground, no
/// plant, no boss, not itself, and no other healer's patient already.
/// `(slot, x, y)`.
fn wounded_ally(horde: &Horde, healer: u16, x: f32, y: f32, range: f32, taken: &[MobCast]) -> Option<(u16, f32, f32)> {
    let mut near = Vec::new();
    horde.near_box(x - range, y - range, x + range, y + range, &mut near);
    let st = horde.store();
    let mut best: Option<(f32, u16)> = None;
    for &m in &near {
        let i = m as usize;
        if m == healer || st.is_plant(m) || st.flags[i] & FLAG_BOSS != 0 || st.hp[i] <= 0 {
            continue;
        }
        if (st.x[i] - x).powi(2) + (st.y[i] - y).powi(2) > range * range {
            continue;
        }
        let frac = st.hp[i] as f32 / horde.block(m).hp.max(1) as f32;
        if frac >= HEAL_BELOW || taken.iter().any(|c| c.ally == Some((m, st.generation[i]))) {
            continue;
        }
        if best.is_none_or(|(f, b)| frac < f || (frac == f && m < b)) {
            best = Some((frac, m));
        }
    }
    best.map(|(_, m)| (m, st.x[m as usize], st.y[m as usize]))
}

/// What a monster skill needs of its caster once it lands (a corpse's
/// too: read before the slot is freed).
#[derive(Clone, Copy)]
struct Caster {
    map: MapIdx,
    dc: i32,
    level: u32,
    evil: bool,
}

fn caster_of(horde: &Horde, slot: u16) -> Caster {
    let block = horde.block(slot);
    Caster { map: horde.map(), dc: 10 + block.save, level: block.level, evil: horde.catalog().get(horde.store().kind[slot as usize]).evil }
}

/// The hazards of one act landing on `area` from `at` on (its repeats
/// `every` apart); lasting ground takes the next telegraph id of `zone`.
fn drop_hazards(act: &Act, ranged: bool, area: Area, c: Caster, at: u32, zone: &mut u32, rng: &mut Rng, hz: f32, out: &mut Vec<Hazard>) {
    let t = &act.timing;
    let lasting = t.lasting(1);
    let load = match &act.payload {
        Payload::Damage(DamageFx { dice: EffectDice::Fixed(d), scale_k, mult, bonus, kind, save, .. }) => HazardLoad::Damage {
            dice: scaled(*d, *scale_k, mult.at(1), bonus.at(1), c.level),
            kind: *kind,
            rider: save.map(|sv| (sv.condition, ticks16(sv.secs.at(1), hz))),
        },
        Payload::Condition { condition, save, secs, .. } => {
            let ticks = match (*save, lasting) {
                (true, _) => ticks16(secs.at(1), hz),
                (false, true) => ticks16(t.pulse * 2.0, hz).max(2),
                (false, false) => ticks16(1.0, hz),
            };
            HazardLoad::Condition { condition: *condition, ticks, save: *save }
        }
        _ => return,
    };
    let id = if lasting {
        *zone += 1;
        *zone - 1
    } else {
        0
    };
    for i in 0..count_of(act, 1) {
        let mut area = area;
        if act.area.scatter > 0.0 {
            let a = rng.die(360) as f32 * std::f32::consts::TAU / 360.0;
            let r = act.area.scatter * (rng.die(1000) as f32 / 1000.0).sqrt();
            let (x, y) = area.centre();
            area = area.moved_to(x + a.cos() * r, y + a.sin() * r);
        }
        let at = at + ticks_of(i as f32 * t.every, hz);
        out.push(Hazard {
            id,
            map: c.map,
            next: at,
            until: if lasting { at + ticks_of(t.secs.at(1), hz) } else { at },
            pulse: if lasting { ticks_of(t.pulse, hz).max(1) } else { 0 },
            area,
            dc: c.dc,
            ranged,
            evil: c.evil,
            load,
        });
    }
}

/// A rally (S5): the monsters inside `area` (the caster always, when it
/// passes the filter) are healed a share of their HP and spurred for a
/// while ([`Rally`]). Plants, objects and bosses other than the caster
/// take nothing.
pub(super) fn rally(horde: &mut Horde, caster: u16, r: &RallyFx, area: &Area, hz: f32, hits: &mut Vec<HitEvent>) {
    let mut near = Vec::new();
    area.monsters(horde, &mut near);
    if horde.store().is_alive(caster) && !near.contains(&caster) {
        near.push(caster);
    }
    let kinds: Vec<u8> = r.kinds.iter().filter_map(|k| horde.catalog().kind_of(k)).collect();
    let spur = Rally {
        to_hit: r.to_hit.clamp(-20, 20) as i8,
        speed: (r.speed * 100.0).round().clamp(0.0, 255.0) as u8,
        taken: (r.resist * 100.0).round().clamp(-100.0, 90.0) as i8,
        ticks: ticks16(r.secs, hz),
    };
    let spurs = spur.ticks > 0 && (spur.to_hit != 0 || spur.speed != 0 || spur.taken != 0);
    for s in near {
        let i = s as usize;
        let st = horde.store();
        if !st.is_alive(s) || st.hp[i] <= 0 || st.is_plant(s) || (s != caster && st.flags[i] & FLAG_BOSS != 0) {
            continue;
        }
        let def = horde.catalog().get(st.kind[i]);
        if (!r.kinds.is_empty() && !kinds.contains(&st.kind[i])) || (r.evil && !def.evil) {
            continue;
        }
        if r.heal > 0.0 {
            let max = horde.block(s).hp;
            let hp = &mut horde.store_mut().hp[i];
            let healed = ((max as f32 * r.heal).round() as i32).max(1).min(max - *hp).max(0);
            *hp += healed;
            if healed > 0 {
                hits.push(HitEvent { target: MONSTER_ENTITY_BASE + s as u32, amount: healed as u32, flags: hit::HEAL });
            }
        }
        if spurs {
            horde.store_mut().rally(s, spur);
        }
    }
}

/// Turns a finished wind-up into hazards and rallies (and moves the
/// monster; a sacrifice dies: its kill goes to `doomed`, paid next tick);
/// false when it was called off (the monster died, or is stunned, frozen,
/// terrified or silenced).
fn land(
    c: &MobCast,
    sk: &MonsterSkill,
    horde: &mut Horde,
    walk: &WalkGrid,
    rng: &mut Rng,
    now: u32,
    hz: f32,
    out: &mut Vec<Hazard>,
    hits: &mut Vec<HitEvent>,
    doomed: &mut Vec<Kill>,
) -> bool {
    let store = horde.store();
    let s = c.slot as usize;
    if !store.is_alive(c.slot) || store.generation[s] != c.generation || store.status_of(c.slot).mute() {
        return false;
    }
    let (mut mx, mut my) = (store.x[s], store.y[s]);
    let who = caster_of(horde, c.slot);
    let mut zone = c.zone;
    for e in &sk.effects {
        let act = match e {
            Effect::Move { mode, distance } => {
                (mx, my) = moved(walk, mx, my, c.dir, distance.at(1).max(0.0), *mode);
                horde.place(c.slot, mx, my);
                continue;
            }
            Effect::Act(act) => act,
            Effect::Passive(_) | Effect::Mount | Effect::Quiver(_) | Effect::Pending(_) => continue,
        };
        let aimed = Aimed { x: c.x, y: c.y, ox: mx, oy: my, dir: c.dir, ally: None, monster: None, pet: None };
        let (area, _) = area_of(act, 1, sk.range, &aimed, mx, my);
        if let Payload::Rally(r) = &act.payload {
            rally(horde, c.slot, r, &area, hz, hits);
            continue;
        }
        drop_hazards(act, sk.ranged, area, who, now, &mut zone, rng, hz, out);
    }
    if sk.sacrifice {
        sacrifice(horde, c.slot, hits, doomed);
    }
    true
}

/// The caster of a `sacrifice` skill dies as it resolves: no HP left, the
/// death shown now, the kill paid by the kill funnel next tick (nobody's
/// blow: the nearest party member loots it).
fn sacrifice(horde: &mut Horde, slot: u16, hits: &mut Vec<HitEvent>, doomed: &mut Vec<Kill>) {
    let hp = &mut horde.store_mut().hp[slot as usize];
    if *hp <= 0 {
        return;
    }
    hits.push(HitEvent { target: MONSTER_ENTITY_BASE + slot as u32, amount: *hp as u32, flags: hit::KILL });
    *hp = 0;
    doomed.push((slot, 0));
}

/// A dying monster's `on_death` skills (S4), from the kill funnel
/// (`fight.rs`) before its slot is freed: each one whose chance rolls
/// resolves at the corpse `cast` seconds later (the fuse; telegraphed
/// now as `BOSS_ZONES` from the dead monster's entity, ability = the
/// skill id). `nearest` / `densest` aim at the nearest standing player in
/// range, else the corpse; a rally on a corpse spurs at once.
pub(super) fn on_death(
    horde: &mut Horde,
    s: u16,
    skills: &[u16],
    book: &SkillBook,
    ups: &[(f32, f32)],
    now: u32,
    hz: f32,
    rng: &mut Rng,
    next_zone: &mut u32,
    out: &mut Vec<Hazard>,
    hits: &mut Vec<HitEvent>,
    msgs: &mut Vec<ServerMsg>,
) {
    if !horde.store().is_alive(s) {
        return;
    }
    let (x, y) = (horde.store().x[s as usize], horde.store().y[s as usize]);
    let who = caster_of(horde, s);
    for &sk in skills {
        let def = book.monster(sk);
        if !def.on_death || (def.chance < 100 && rng.die(100) > def.chance) {
            continue;
        }
        let nearest = ups
            .iter()
            .map(|&(px, py)| ((px - x).powi(2) + (py - y).powi(2), px, py))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .filter(|n| n.0.sqrt() <= def.range + BODY);
        let dir = nearest.and_then(|n| norm(n.1 - x, n.2 - y)).unwrap_or((1.0, 0.0));
        let aim = match (def.target, nearest) {
            (MobTarget::Nearest | MobTarget::Densest, Some(n)) => (n.1, n.2),
            _ => (x, y),
        };
        let at = now + ticks_of(def.cast, hz).max(1);
        let mut zone = *next_zone;
        let mut zones = Vec::new();
        for act in def.effects.iter().filter_map(Effect::act) {
            let aimed = Aimed { x: aim.0, y: aim.1, ox: x, oy: y, dir, ally: None, monster: None, pet: None };
            let (area, _) = area_of(act, 1, def.range, &aimed, x, y);
            if let Payload::Rally(r) = &act.payload {
                rally(horde, s, r, &area, hz, hits);
                continue;
            }
            let first = zone;
            drop_hazards(act, def.ranged, area, who, at, &mut zone, rng, hz, out);
            let t = &act.timing;
            let span = if t.lasting(1) { t.secs.at(1) } else { (count_of(act, 1) - 1) as f32 * t.every };
            let mut shown = area;
            if act.area.scatter > 0.0
                && let Area::Circle { x, y, r } = area
            {
                shown = Area::Circle { x, y, r: r + act.area.scatter };
            }
            let tid = if zone != first { first } else { 0 };
            zones.push(BossZone { area: shown.telegraph(tid, look_of(act), (span * 1000.0).round() as u32, 0), delay_ms: (def.cast * 1000.0).round() as u32, flags: 0 });
        }
        *next_zone = zone.max(1);
        if !zones.is_empty() {
            msgs.push(ServerMsg::BossZones { caster: MONSTER_ENTITY_BASE + s as u32, ability: def.id.clone(), zones });
        }
    }
}

/// One action of a hazard on every standing player inside, then on every
/// companion inside (`companion.rs`: a save halves the damage; only the
/// holding conditions stick to them).
fn strike(h: &Hazard, players: &mut [HubPlayer], companions: &mut Companions, lives: &[Live], rng: &mut Rng, now: u32, hz: f32, hits: &mut Vec<HitEvent>) {
    match h.load {
        HazardLoad::Damage { dice, kind, .. } => companions.hazard_damage(&h.area, dice, kind, h.dc, rng, now, hits),
        HazardLoad::Condition { condition, ticks, save } => companions.hazard_condition(&h.area, condition, ticks, save, h.dc, rng, now),
    }
    for p in players.iter_mut().filter(|p| p.map == h.map) {
        let (x, y, id) = (p.x, p.y, p.profile.character_id);
        let Some(f) = p.fighter.as_mut() else { continue };
        if !f.is_up() || !h.area.contains(x, y, BODY) {
            continue;
        }
        if h.ranged && lives.iter().any(|l| l.deflects(h.map, x, y)) {
            hits.push(HitEvent { target: id, amount: 0, flags: hit::GUARD });
            continue;
        }
        f.quiet = 0;
        match h.load {
            HazardLoad::Damage { dice, kind, rider } => {
                let bonus = f.save_against(rider.map(|r| r.0));
                let saved = !f.status.saves_fail() && saving_throw(rng, bonus, h.dc, 0);
                let mut amount = dice.roll(rng, false);
                if saved {
                    amount /= 2;
                }
                let taken = f.take_from(amount, kind == DamageType::Physical, h.evil);
                hits.push(HitEvent { target: id, amount: taken as u32, flags: damage_type_bits(kind) });
                if let Some((c, ticks)) = rider
                    && !saved
                {
                    f.afflict(c, ticks);
                }
            }
            HazardLoad::Condition { condition, ticks, save } => {
                let bonus = f.save_against(Some(condition));
                if !save || f.status.saves_fail() || !saving_throw(rng, bonus, h.dc, 0) {
                    f.afflict(condition, ticks);
                }
            }
        }
        f.fall_if_spent(hz);
    }
}

impl HubInstance {
    /// Monster wind-ups, landings and hazards; inside [`HubInstance::fight`]
    /// after the players' attacks, before monster melee.
    pub(super) fn monster_skills(&mut self, out: &mut Outgoing, hits: &mut Vec<HitEvent>) {
        let Some(horde) = self.horde.as_mut() else { return };
        let book = std::sync::Arc::clone(&self.skills);
        let map = horde.map();
        let hz = self.config.tick_hz as f32;
        let now = self.tick;
        if self.kind_skills.len() != horde.catalog().defs().len() {
            self.kind_skills = kind_skills(horde, &book);
            // Kiters keep their band inside their longest ranged shot.
            let reach = self.kind_skills.iter().map(|v| v.iter().map(|&k| book.monster(k)).filter(|m| m.ranged).map(|m| m.range).fold(0.0, f32::max)).collect();
            horde.set_reach(reach);
        }
        if self.minds.len() != horde.store().capacity() {
            self.minds = vec![Mind::default(); horde.store().capacity()];
        }
        let walk = &self.world.map(map).walk;

        // 1. Wind-ups that end now land. One called off tells the clients,
        // or they would still play its landing (unless the slot already
        // winds up a newer cast: a cancel names the entity, not the cast).
        let mut i = 0;
        let mut casts: Vec<ServerMsg> = Vec::new();
        while i < self.mob_casts.len() {
            let c = self.mob_casts[i];
            if c.land > now {
                // Silenced (or held) mid-wind-up, or flinched during an
                // interruptible one: called off at once.
                let st = horde.store();
                let mine = st.is_alive(c.slot) && st.generation[c.slot as usize] == c.generation;
                if mine && (st.status_of(c.slot).mute() || (book.monster(c.skill).interrupt && st.staggered(c.slot))) {
                    self.mob_casts.swap_remove(i);
                    casts.push(super::cast::cancel(MONSTER_ENTITY_BASE + c.slot as u32));
                } else {
                    i += 1;
                }
                continue;
            }
            let c = self.mob_casts.swap_remove(i);
            if !land(&c, book.monster(c.skill), horde, walk, &mut self.dice, now, hz, &mut self.hazards, hits, &mut self.doomed)
                && !self.mob_casts.iter().any(|o| o.slot == c.slot)
            {
                casts.push(super::cast::cancel(MONSTER_ENTITY_BASE + c.slot as u32));
            }
        }

        // 2. Hazards act.
        let mut k = 0;
        while k < self.hazards.len() {
            let h = &mut self.hazards[k];
            // Ran its time: the client fades the telegraph on its own.
            if now > h.until {
                self.hazards.swap_remove(k);
                continue;
            }
            if now >= h.next {
                h.next = if h.pulse > 0 { now + h.pulse } else { h.until + 1 };
                let h = h.clone();
                strike(&h, &mut self.players, &mut self.companions, &self.lives, &mut self.dice, now, hz, hits);
            }
            k += 1;
        }

        // 3. New wind-ups.
        let ups: Vec<(f32, f32)> = self
            .players
            .iter()
            .filter(|p| p.map == map && p.fighter.as_ref().is_some_and(|f| f.hunted()))
            .map(|p| (p.x, p.y))
            .collect();
        let check = ticks_of(CHECK_SECS, hz).max(1);
        // Kiter wind-ups in the air (capped: MAX_VOLLEYS).
        let volley_cap = MAX_VOLLEYS + ups.len().saturating_sub(1);
        let mut volleys = {
            let st = horde.store();
            self.mob_casts
                .iter()
                .filter(|c| st.is_alive(c.slot) && st.generation[c.slot as usize] == c.generation && st.behaviour[c.slot as usize] == Behaviour::Kiter as u8)
                .count()
        };
        // Nobody to hunt: no new wind-ups (the cancels above still go out).
        let alive = if ups.is_empty() { 0 } else { horde.store().alive().len() };
        for a in 0..alive {
            let slot = horde.store().alive()[a];
            let s = slot as usize;
            let store = horde.store();
            let kind = store.kind[s] as usize;
            let skills = &self.kind_skills[kind];
            if skills.is_empty() || store.flags[s] & FLAG_FULL == 0 || store.burrowed(slot) {
                continue;
            }
            let mind = &mut self.minds[s];
            if mind.generation != store.generation[s] || !mind.fresh {
                *mind = Mind { generation: store.generation[s], fresh: true, next_check: now + 1 + s as u32 % check, ..Mind::default() };
                // Bosses open with their skills ready; the rest wait a cooldown
                // (kiters a random share of one: a pack does not loose in step).
                let kiter = store.behaviour[s] == Behaviour::Kiter as u8;
                for (j, &sk) in skills.iter().enumerate() {
                    let cd = ticks_of(book.monster(sk).cooldown, hz);
                    mind.ready[j] = if store.flags[s] & FLAG_BOSS != 0 {
                        now
                    } else if kiter {
                        now + cd * self.dice.die(100) / 100
                    } else {
                        now + cd / 2
                    };
                }
                continue;
            }
            if now < mind.next_check || now < mind.busy_until || store.status_of(slot).mute() {
                continue;
            }
            mind.next_check = now + check;
            let (mx, my) = (store.x[s], store.y[s]);
            let block = horde.block(slot);
            let hp_frac = (store.hp[s] as f32 / block.hp.max(1) as f32).min(1.0);
            // Taunted by a companion's lure (Amistr, a Marine Sphere): it
            // aims at the lure.
            let taunter = if store.status_of(slot).has(Condition::Taunted) { store.taunter[s] } else { 0 };
            let lure = (taunter >= LURE_BASE).then(|| horde.lures().iter().find(|l| l.id == taunter)).flatten();
            let nearest = match lure {
                Some(l) => Some((((l.x - mx).powi(2) + (l.y - my).powi(2)).sqrt(), l.x, l.y)),
                None => ups
                    .iter()
                    .map(|&(x, y)| ((x - mx).powi(2) + (y - my).powi(2), x, y))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(d2, x, y)| (d2.sqrt(), x, y)),
            };
            let Some((dist, nx, ny)) = nearest else { continue };
            let behaviour = Behaviour::from_u8(store.behaviour[s]);
            let kiter = behaviour == Behaviour::Kiter;
            let backline = matches!(behaviour, Behaviour::Support | Behaviour::Healer) && lure.is_none();
            if kiter && volleys >= volley_cap {
                continue; // the sky is full: it looks again in a second
            }
            let los = store.flags[s] & FLAG_LOS != 0;
            let mut chosen = None;
            for (j, &sk) in skills.iter().enumerate() {
                let def = book.monster(sk);
                // Corpse skills wait for the death; an interruptible one
                // does not start while the monster still flinches.
                if def.on_death || (def.interrupt && store.staggered(slot)) {
                    continue;
                }
                let rally = rally_fx(def).filter(|_| backline);
                let reach = if rally.is_some() { SUPPORT_TRIGGER } else { def.range + BODY };
                if now < mind.ready[j] || hp_frac > def.hp_below || dist > reach {
                    continue;
                }
                // A kiter shoots only what it sees.
                if kiter && def.ranged && !los {
                    continue;
                }
                // Backline rallies: a heal beam needs a patient, a spur a pack.
                let mut patient = None;
                if let Some(r) = rally {
                    if r.heal > 0.0 && matches!(def.target, MobTarget::Nearest) {
                        match wounded_ally(horde, slot, mx, my, def.range, &self.mob_casts) {
                            Some(p) => patient = Some(p),
                            None => continue,
                        }
                    } else {
                        let planned = plan(def, walk, mx, my, (mx, my), (1.0, 0.0));
                        let mut near = Vec::new();
                        if let Some((area, _)) = planned.iter().find(|(_, a)| matches!(a.payload, Payload::Rally(_))) {
                            area.monsters(horde, &mut near);
                        }
                        let st = horde.store();
                        let pack = near.iter().filter(|&&m| m != slot && !st.is_plant(m) && st.flags[m as usize] & FLAG_BOSS == 0).count();
                        if pack < RALLY_MIN_PACK {
                            continue;
                        }
                    }
                }
                if def.chance < 100 && self.dice.die(100) > def.chance {
                    // A miss waits a cooldown's fraction so a crowd does
                    // not roll every second.
                    mind.ready[j] = now + ticks_of(def.cooldown, hz) / 4;
                    continue;
                }
                chosen = Some((j, sk, patient));
                break;
            }
            let Some((j, sk, patient)) = chosen else { continue };
            let def = book.monster(sk);
            let (nx, ny) = patient.map_or((nx, ny), |p| (p.1, p.2));
            let dir = norm(nx - mx, ny - my).unwrap_or((1.0, 0.0));
            let aim = match def.target {
                MobTarget::Caster | MobTarget::Front => (mx, my),
                MobTarget::Nearest => (nx, ny),
                MobTarget::Densest => {
                    let r = def.effects.iter().filter_map(Effect::act).map(|a| a.area.radius.at(1)).fold(1.0f32, f32::max);
                    ups.iter()
                        .filter(|&&(x, y)| (x - mx).powi(2) + (y - my).powi(2) <= (def.range + BODY).powi(2))
                        .max_by_key(|&&(x, y)| ups.iter().filter(|&&(ox, oy)| (ox - x).powi(2) + (oy - y).powi(2) <= r * r).count())
                        .copied()
                        .unwrap_or((nx, ny))
                }
            };
            let cast = ticks_of(def.cast, hz);
            let cooldown = ticks_of(def.cooldown, hz);
            mind.ready[j] = now + if kiter { cooldown * (85 + self.dice.die(31)) / 100 } else { cooldown };
            mind.busy_until = now + cast + 1 + ticks_of(if kiter { KITER_GCD_SECS } else { SKILL_GCD_SECS }, hz);
            if kiter {
                volleys += 1;
            }
            let zone = self.next_zone;
            let planned = plan(def, walk, mx, my, aim, dir);
            let lasting = planned.iter().filter(|(_, a)| a.timing.lasting(1)).count() as u32;
            self.next_zone = self.next_zone.wrapping_add(lasting).max(1);
            casts.push(ServerMsg::Cast {
                caster: MONSTER_ENTITY_BASE + slot as u32,
                skill: def.id.clone(),
                x: aim.0,
                y: aim.1,
                cast_ms: (def.cast * 1000.0).round() as u32,
                cooldown_ms: 0,
                areas: telegraphs(&planned, zone),
                level: 1,
            });
            let ally = patient.map(|p| (p.0, store.generation[p.0 as usize]));
            self.mob_casts.push(MobCast { slot, generation: store.generation[s], skill: sk, land: now + cast, x: aim.0, y: aim.1, dir, zone, ally });
            // It stands still and holds its melee through the wind-up.
            let store = horde.store_mut();
            store.afflict(slot, Condition::Rooted, (cast + 1).min(u16::MAX as u32) as u16);
            store.cooldown[s] = store.cooldown[s].max((cast + 2).min(u16::MAX as u32) as u16);
        }
        if !casts.is_empty() {
            for p in self.players.iter().filter(|p| p.map == map) {
                for m in &casts {
                    out.push((p.profile.user_id, HubEvent::Control(m.clone())));
                }
            }
        }
    }

    /// Ends monster ground inside `area` (Ganbantein); returns the
    /// telegraph ids to end on the clients.
    pub(super) fn dispel_hazards(&mut self, map: MapIdx, area: &Area) -> Vec<u32> {
        let mut ids = Vec::new();
        self.hazards.retain(|h| {
            let (x, y) = h.area.centre();
            let hit = h.map == map && h.pulse > 0 && area.contains(x, y, 0.0);
            if hit && h.id != 0 {
                ids.push(h.id);
            }
            !hit
        });
        ids
    }
}
