//! Scripted elite and boss fights (COMBAT.md "Boss encounters").
//!
//! A monster whose kind has a script ([`scripts::SCRIPTS`]) fights by it
//! instead of by its monster skills: once a standing player comes within
//! its aggro range it yells, shows its bar to the map and runs the
//! rotation of its current phase. Each ability is telegraphed as danger
//! zones that resolve in beats (`BOSS_ZONES`, each with its own delay so
//! the client fills them over the time left), and the server resolves the
//! damage when a beat lands: rings with safe gaps, crosses, sweeps,
//! charges, leaps, pools, a nova with sanctuaries. Channels can be broken
//! with burst damage, shields soaked up, adds killed before they are
//! swallowed. Phases follow HP; a soft enrage follows time.
//!
//! Everything random goes through the stage's dice, so a seeded stage
//! replays the same fight. The state is a handful of small vectors that
//! only grow when an ability starts: a quiet tick allocates nothing.

mod scripts;

use super::aim::{norm, BODY};
use super::area::Area;
use super::cast::{moved, ticks_of, Live};
use super::fight::damage_type_bits;
use super::{HubEvent, HubInstance, HubPlayer, MapIdx, Outgoing};
use crate::world::combat::rules::saving_throw;
use crate::world::combat::skills::MoveMode;
use crate::world::combat::{Condition, Dice, Rng};
use crate::world::horde::storage::{FLAG_FULL, FLAG_REFLECT};
use crate::world::horde::Horde;
use crate::world::map::WalkGrid;
use crate::world::protocol::{cast_flags, zone_flags, BossBarInfo, BossZone, HitEvent, ServerMsg, Telegraph, MONSTER_ENTITY_BASE};
use scripts::{Ability, Action, Anchor, Hurt, Pattern, Rank, Script, Traits};
use std::f32::consts::TAU;

/// Adds a fight remembers (to swallow them, or to count them).
const MAX_ADDS: usize = 24;
/// Sanctuaries a beat can spare players in.
const MAX_SAFE: usize = 4;
/// Look for newly spawned scripted monsters this often (ticks).
const SCAN_TICKS: u32 = 10;
/// The bar is resent at most this often (ticks) while HP moves, and at
/// least this often (seconds) so late joiners see it.
const BAR_EVERY: u32 = 4;
const BAR_REFRESH_SECS: f32 = 3.0;
/// A broken channel or shield stuns the monster this long.
const BREAK_STUN_SECS: f32 = 2.5;
/// Wind-ups shrink to this share once enraged; beats hit this much harder.
const ENRAGE_HASTE: f32 = 0.85;
const ENRAGE_DAMAGE: f32 = 1.5;
/// An elite with nobody within its leash this long leaves the fight.
const DISENGAGE_SECS: f32 = 10.0;
/// First ability after the fight starts.
const OPENING_SECS: f32 = 1.0;
/// Yelled when the last mirror image of a monster shatters.
const MIRRORS_BROKEN: &str = "My reflections! You'll pay for that!";
/// A burrowing monster stops tunnelling this long before it bursts up, so
/// the ripple holds still and a player can step out of it.
const TUNNEL_FUSE_SECS: f32 = 0.7;
/// Yelled when the party breaks a channel or a shield.
const BROKEN: &str = "Argh! You dare interrupt me?!";

/// Whether a monster id fights by a script (its monster skills sleep).
pub(super) fn scripted(monster: &str) -> bool {
    scripts::find(monster).is_some()
}

/// A danger zone's shape: the hub's areas, plus a ring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Zone {
    Area(Area),
    /// Hurts between `inner` and `outer` (the inside is safe).
    Ring { x: f32, y: f32, inner: f32, outer: f32 },
}

impl Zone {
    /// Whether a body of radius `body` at `(x, y)` touches the zone.
    pub fn contains(&self, x: f32, y: f32, body: f32) -> bool {
        match *self {
            Zone::Area(a) => a.contains(x, y, body),
            Zone::Ring { x: cx, y: cy, inner, outer } => {
                let d2 = (x - cx).powi(2) + (y - cy).powi(2);
                let out = outer + body;
                let inn = (inner - body).max(0.0);
                d2 <= out * out && d2 >= inn * inn
            }
        }
    }

    /// The shape for clients (shape 3 = ring: `a` outer, `b` inner).
    pub fn telegraph(&self, id: u32, look: u8, span_ms: u32) -> Telegraph {
        match *self {
            Zone::Area(a) => a.telegraph(id, look, span_ms, 0),
            Zone::Ring { x, y, inner, outer } => Telegraph { id, shape: 3, look, x, y, dir: 0, a: outer, b: inner, span_ms, follow: 0 },
        }
    }
}

/// One zone of an expanded pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Shape {
    pub zone: Zone,
    /// Seconds after the wind-up ends.
    pub delay: f32,
    /// A sanctuary: it spares the beat's victims standing inside.
    pub safe: bool,
    /// The monster lands here as it resolves (charges, leaps).
    pub place: Option<(f32, f32)>,
}

fn turn(d: (f32, f32), deg: f32) -> (f32, f32) {
    let (s, c) = deg.to_radians().sin_cos();
    (d.0 * c - d.1 * s, d.0 * s + d.1 * c)
}

fn circle(x: f32, y: f32, r: f32) -> Zone {
    Zone::Area(Area::Circle { x, y, r })
}

/// Expands one anchor of a pattern into zones. `origin`: where the monster
/// stands (lines and hops start there); `anchor`: where it lands; `aim`:
/// unit direction of patterns centred on the monster; `spin`: turns of the
/// sanctuaries (the stage's dice); `pace`: multiplier of the beat spacing.
#[allow(clippy::too_many_arguments)]
pub(super) fn expand(
    pattern: &Pattern,
    action: &Action,
    walk: &WalkGrid,
    origin: (f32, f32),
    anchor: (f32, f32),
    aim: (f32, f32),
    spin: f32,
    pace: f32,
    out: &mut Vec<Shape>,
) {
    let (ax, ay) = anchor;
    let lands = matches!(action, Action::Leap);
    let mut push = |zone: Zone, delay: f32, safe: bool, place: Option<(f32, f32)>| out.push(Shape { zone, delay: delay * pace, safe, place });
    match *pattern {
        Pattern::None => {}
        Pattern::Circle { r } | Pattern::Pool { r, .. } => {
            let place = (lands && walk.is_walkable(ax, ay)).then_some(anchor);
            push(circle(ax, ay, r), 0.0, false, place);
        }
        Pattern::Donut { inner, outer } => push(Zone::Ring { x: ax, y: ay, inner, outer }, 0.0, false, None),
        Pattern::Rings { count, start, width, step, every } => {
            for i in 0..count {
                let inner = start + i as f32 * step;
                let zone = if inner <= 0.0 { circle(ax, ay, inner + width) } else { Zone::Ring { x: ax, y: ay, inner, outer: inner + width } };
                push(zone, i as f32 * every, false, None);
            }
        }
        Pattern::Cross { arms, len, width, count, step, every } => {
            let arms = arms.max(1);
            for c in 0..count {
                for a in 0..arms {
                    let dir = turn(aim, c as f32 * step + a as f32 * 180.0 / arms as f32);
                    let rect = Area::Rect { x: ax, y: ay, dir, half_len: len / 2.0, half_wid: width / 2.0 };
                    push(Zone::Area(rect), c as f32 * every, false, None);
                }
            }
        }
        Pattern::Sweep { count, r, arc, step, every } => {
            let arc_cos = (arc / 2.0).to_radians().cos();
            for i in 0..count {
                let dir = turn(aim, i as f32 * step);
                push(Zone::Area(Area::Arc { x: ax, y: ay, dir, r, arc_cos }), i as f32 * every, false, None);
            }
        }
        Pattern::Line { len, width } => {
            let dir = norm(ax - origin.0, ay - origin.1).unwrap_or(aim);
            let charge = matches!(action, Action::Charge);
            // A charge stops at walls: the line shows where it really goes.
            let length = if charge {
                let end = moved(walk, origin.0, origin.1, dir, len, MoveMode::Slide);
                ((end.0 - origin.0).powi(2) + (end.1 - origin.1).powi(2)).sqrt().max(1.0)
            } else {
                len
            };
            let place = charge.then(|| moved(walk, origin.0, origin.1, dir, length, MoveMode::Slide));
            push(Zone::Area(Area::line(origin.0, origin.1, dir, length, width)), 0.0, false, place);
        }
        Pattern::Hops { count, first, grow, r, every } => {
            let dir = norm(ax - origin.0, ay - origin.1).unwrap_or(aim);
            let (mut x, mut y) = origin;
            for i in 0..count {
                (x, y) = moved(walk, x, y, dir, first + i as f32 * grow, MoveMode::Slide);
                push(circle(x, y, r), i as f32 * every, false, lands.then_some((x, y)));
            }
        }
        Pattern::Spiral { arms, steps, first, step, turn: deg, r, every } => {
            let arms = arms.max(1);
            for arm in 0..arms {
                let base = turn(aim, arm as f32 * 360.0 / arms as f32);
                for i in 0..steps {
                    let (dx, dy) = turn(base, i as f32 * deg);
                    let d = first + i as f32 * step;
                    push(circle(ax + dx * d, ay + dy * d, r), i as f32 * every, false, None);
                }
            }
        }
        Pattern::Bars { count, len, width, gap, first, advance, every } => {
            // Each rank stands across the aim; its gap slides by the dice.
            let across = (-aim.1, aim.0);
            let room = (len - gap).max(0.0);
            for i in 0..count {
                let along = first + i as f32 * advance;
                let (cx, cy) = (ax + aim.0 * along, ay + aim.1 * along);
                let at = ((spin / TAU + i as f32 * 0.618_034).fract() - 0.5) * room;
                // The two halves either side of the gap.
                for (lo, hi) in [(-len / 2.0, at - gap / 2.0), (at + gap / 2.0, len / 2.0)] {
                    if hi - lo < 0.5 {
                        continue;
                    }
                    let mid = (lo + hi) / 2.0;
                    let rect = Area::Rect { x: cx + across.0 * mid, y: cy + across.1 * mid, dir: across, half_len: (hi - lo) / 2.0, half_wid: width / 2.0 };
                    push(Zone::Area(rect), i as f32 * every, false, None);
                }
            }
        }
        Pattern::Chase { count, r, every } => {
            for i in 0..count {
                push(circle(ax, ay, r), i as f32 * every, false, None);
            }
        }
        // Drawn between the monster and its images, round the map's lit
        // lights (`Bosses::start`).
        Pattern::Crossfire { .. } | Pattern::LitSanctuary { .. } => {}
        // Dropped behind the player as the ability runs (`Deed::Puddle`).
        Pattern::Trail { .. } => {}
        Pattern::Sanctuary { r, safe, safe_r, dist } => {
            push(circle(ax, ay, r), 0.0, false, None);
            for i in 0..safe.min(MAX_SAFE as u8) {
                let a = spin + i as f32 * TAU / safe as f32;
                let (sx, sy) = moved(walk, ax, ay, (a.cos(), a.sin()), dist, MoveMode::Blink);
                if (sx - ax).powi(2) + (sy - ay).powi(2) >= (safe_r * 2.0).powi(2) {
                    push(circle(sx, sy, safe_r), 0.0, true, None);
                }
            }
        }
    }
}

/// A beat waiting to land, or lasting ground biting.
#[derive(Clone, Copy, Debug)]
struct Beat {
    fight: u32,
    cast: u32,
    map: MapIdx,
    next: u32,
    until: u32,
    pulse: u32,
    zone: Zone,
    /// Telegraph id of lasting ground (0 = a one-off).
    zone_id: u32,
    safe: [(f32, f32, f32); MAX_SAFE],
    n_safe: u8,
    hurt: Hurt,
    level: u32,
    dc: i32,
    boost: f32,
    evil: bool,
    /// Where the monster stood when it cast (a fear flees from there).
    from: (f32, f32),
    /// A chasing zone: the player (character id) it follows; it lands
    /// wherever they are (0 = it stays put).
    follow: u32,
    /// A zone riding on a monster (a burrowing boss's ripple): it lands
    /// wherever that monster (slot, generation) is by then.
    follow_mob: Option<(u16, u8)>,
}

/// Something the monster does later in an ability.
#[derive(Clone, Copy, Debug)]
enum Deed {
    /// It lands here.
    Place { x: f32, y: f32 },
    /// Its adds arrive.
    Summon { kind: u8, count: u8 },
    /// The ground shakes on everyone's screen (`ENVIRONMENT` shake).
    Shake { intensity: u8, ms: u32 },
    /// A burrowing monster bursts up where it is; unless `last`, it dives
    /// again and tunnels after `next` (a character id).
    Surface { last: bool, next: u32 },
    /// It lays `count` objects of `kind` round itself, `ring` cells out;
    /// each hatches into (kind, count) after `secs` unless broken.
    Lay { kind: u8, count: u8, ring: f32, hatch: Option<(u8, u8, f32)> },
    /// A puddle of ability `ability` dropped under `follow` (a character id).
    Puddle { ability: u8, follow: u32 },
}

#[derive(Clone, Copy, Debug)]
struct Later {
    fight: u32,
    cast: u32,
    at: u32,
    deed: Deed,
}

#[derive(Clone, Copy, Debug)]
struct Channel {
    ability: u8,
    until: u32,
    /// HP when it began; losing `breaks` of it breaks the channel.
    mark: i32,
    breaks: i32,
    cast: u32,
}

/// One scripted monster's fight.
#[derive(Clone, Copy, Debug)]
pub(super) struct Fight {
    serial: u32,
    slot: u16,
    generation: u8,
    kind: u8,
    script: &'static Script,
    engaged: bool,
    since: u32,
    phase: u8,
    step: usize,
    opener: Option<u8>,
    next_at: u32,
    busy_until: u32,
    channel: Option<Channel>,
    /// Absorb left, the cast it protects, when it lapses.
    shield: i32,
    shield_cast: u32,
    shield_until: u32,
    hp_seen: i32,
    enraged: bool,
    /// Ticks with nobody within its leash (an elite disengages).
    away: u32,
    adds: [(u16, u8); MAX_ADDS],
    n_adds: u8,
    /// Kind of its mirror images while any stands (it takes no damage).
    mirror: Option<u8>,
    /// Its body's traits (spell reflection, a cracked shell, spilled coins).
    traits: Option<&'static Traits>,
    /// Next HP % at which it spills zeny (0 = not yet counting).
    burst_next: u8,
    /// Burrowed and tunnelling after a player: (character id, cells/s,
    /// until tick: it stops under the ripple to let them step out).
    tunnel: Option<(u32, f32, u32)>,
    /// (hp, shield, phase, enraged) last sent, and the bar's timers.
    bar_sent: (i32, i32, u8, bool),
    bar_wait: u32,
    bar_refresh: u32,
}

#[cfg(test)]
impl Fight {
    pub fn phase(&self) -> u8 {
        self.phase
    }

    pub fn engaged(&self) -> bool {
        self.engaged
    }

    pub fn enraged(&self) -> bool {
        self.enraged
    }

    pub fn shield(&self) -> i32 {
        self.shield
    }
}

/// The stage's scripted fights.
#[derive(Default)]
pub(super) struct Bosses {
    /// Script per monster kind (catalog order), resolved once.
    kinds: Vec<Option<&'static Script>>,
    any: bool,
    fights: Vec<Fight>,
    beats: Vec<Beat>,
    laters: Vec<Later>,
    serial: u32,
    /// Scratch: standing players `(x, y)`, expanded shapes, monster queries.
    ups: Vec<(f32, f32)>,
    /// Character ids of `ups`, in the same order (chasing zones).
    up_ids: Vec<u32>,
    /// Lit lights of the map (cells; Payon's burning braziers): the safe
    /// spots of a [`Pattern::LitSanctuary`].
    lights: Vec<(f32, f32)>,
    shapes: Vec<Shape>,
    near: Vec<u16>,
    /// What the hub does for the fights after their step (it owns the
    /// environment and the purses): objects laid (slot, generation, hatch
    /// at run second, kind, count), "hatch everything near" requests
    /// (fight serial, x, y, radius, heal %), zeny spills (monster entity,
    /// x, y, min, max).
    laid: Vec<(u16, u8, Option<(f32, u8, u8)>)>,
    hatch_all: Vec<(u32, f32, f32, f32, u8)>,
    bursts: Vec<(u32, f32, f32, u32, u32)>,
}

/// What one boss step works with.
struct Ctx<'a> {
    horde: &'a mut Horde,
    walk: &'a WalkGrid,
    dice: &'a mut Rng,
    next_zone: &'a mut u32,
    msgs: &'a mut Vec<ServerMsg>,
    map: MapIdx,
    now: u32,
    hz: f32,
    /// Fighters on the map (scales breaks, shields, summons).
    party: u32,
}

impl Ctx<'_> {
    fn ticks(&self, secs: f32) -> u32 {
        ticks_of(secs.max(0.0), self.hz)
    }

    fn party_scale(&self, base: i32) -> i32 {
        // +50 % per extra player.
        base + base * (self.party.saturating_sub(1) as i32) / 2
    }
}

impl Bosses {
    #[cfg(test)]
    pub fn fights(&self) -> &[Fight] {
        &self.fights
    }

    /// Enrages the engaged fight of the monster in `slot` now, ahead of its
    /// timer (a location's own trigger: Sograt's alpha losing its pack).
    /// Returns whether it was not enraged yet; the bar shows it next tick.
    pub fn enrage(&mut self, slot: u16) -> bool {
        match self.fights.iter_mut().find(|f| f.slot == slot && f.engaged && !f.enraged) {
            Some(f) => {
                f.enraged = true;
                true
            }
            None => false,
        }
    }

    /// Whether monster kind `kind` fights by a script.
    pub fn is_scripted(&self, kind: u8) -> bool {
        self.kinds.get(kind as usize).is_some_and(Option::is_some)
    }

    fn next_serial(&mut self) -> u32 {
        self.serial = self.serial.wrapping_add(1).max(1);
        self.serial
    }

    fn shout(cx: &mut Ctx<'_>, f: &Fight, text: &str) {
        if text.is_empty() {
            return;
        }
        let name = cx.horde.catalog().get(f.kind).name.clone();
        let style = u8::from(f.script.rank == Rank::Boss);
        cx.msgs.push(ServerMsg::BossShout { entity_id: MONSTER_ENTITY_BASE + f.slot as u32, name, text: text.into(), style });
    }

    /// New scripted monsters on the field.
    fn discover(&mut self, cx: &mut Ctx<'_>) {
        if !cx.now.is_multiple_of(SCAN_TICKS) {
            return;
        }
        let store = cx.horde.store();
        for &slot in store.alive() {
            let s = slot as usize;
            let Some(script) = self.kinds.get(store.kind[s] as usize).copied().flatten() else { continue };
            if self.fights.iter().any(|f| f.slot == slot && f.generation == store.generation[s]) {
                continue;
            }
            self.serial = self.serial.wrapping_add(1).max(1);
            self.fights.push(Fight {
                serial: self.serial,
                slot,
                generation: store.generation[s],
                kind: store.kind[s],
                script,
                engaged: false,
                since: 0,
                phase: 0,
                step: 0,
                opener: script.phases[0].opener,
                next_at: 0,
                busy_until: 0,
                channel: None,
                shield: 0,
                shield_cast: 0,
                shield_until: 0,
                hp_seen: store.hp[s],
                enraged: false,
                away: 0,
                adds: [(0, 0); MAX_ADDS],
                n_adds: 0,
                bar_sent: (i32::MIN, 0, 0, false),
                bar_wait: 0,
                bar_refresh: 0,
                mirror: None,
                traits: scripts::traits(script.monster),
                burst_next: 0,
                tunnel: None,
            });
        }
    }

    /// A fight is over (its monster fell or left): its bar goes, its
    /// lasting ground ends, nothing it planned happens.
    fn end(&mut self, cx: &mut Ctx<'_>, k: usize, ended: &mut Vec<u32>) {
        let f = self.fights.swap_remove(k);
        // Killed (not cleared away with the field): its last words.
        let s = f.slot as usize;
        let store = cx.horde.store();
        let fell = !store.is_alive(f.slot) && store.generation[s] == f.generation.wrapping_add(1) && store.hp[s] <= 0;
        if f.engaged && fell {
            cx.msgs.push(ServerMsg::BossShout {
                entity_id: MONSTER_ENTITY_BASE + f.slot as u32,
                name: cx.horde.catalog().get(f.kind).name.clone(),
                text: f.script.death.into(),
                style: u8::from(f.script.rank == Rank::Boss),
            });
        }
        if f.engaged {
            Self::gone(cx, &f);
        }
        self.beats.retain(|b| {
            let keep = b.fight != f.serial;
            if !keep && b.zone_id != 0 {
                ended.push(b.zone_id);
            }
            keep
        });
        self.laters.retain(|l| l.fight != f.serial);
    }

    /// The monster's bar leaves the party's screens.
    fn gone(cx: &mut Ctx<'_>, f: &Fight) {
        cx.msgs.push(ServerMsg::BossBar(BossBarInfo {
            entity_id: MONSTER_ENTITY_BASE + f.slot as u32,
            name: String::new(),
            rank: 0,
            hp: 0,
            max_hp: 0,
            shield: 0,
            phase: 0,
            pips: Vec::new(),
            flags: 0,
        }));
    }

    /// Calls off what is left of cast `cast` (a broken channel or shield).
    fn cancel(&mut self, cast: u32, ended: &mut Vec<u32>) {
        self.beats.retain(|b| {
            let keep = b.cast != cast;
            if !keep && b.zone_id != 0 {
                ended.push(b.zone_id);
            }
            keep
        });
        self.laters.retain(|l| l.cast != cast);
    }

    fn bar(cx: &mut Ctx<'_>, f: &mut Fight, force: bool) {
        let s = f.slot as usize;
        let store = cx.horde.store();
        let hp = store.hp[s].max(0);
        let now = (hp, f.shield, f.phase, f.enraged);
        f.bar_wait = f.bar_wait.saturating_sub(1);
        let due = force || cx.now >= f.bar_refresh || (now != f.bar_sent && f.bar_wait == 0);
        if !due {
            return;
        }
        f.bar_sent = now;
        f.bar_wait = BAR_EVERY;
        f.bar_refresh = cx.now + cx.ticks(BAR_REFRESH_SECS);
        let def = cx.horde.catalog().get(f.kind);
        let max = cx.horde.block(f.slot).hp.max(hp);
        cx.msgs.push(ServerMsg::BossBar(BossBarInfo {
            entity_id: MONSTER_ENTITY_BASE + f.slot as u32,
            name: def.name.clone(),
            rank: if f.script.rank == Rank::Boss { 2 } else { 1 },
            hp: hp as u32,
            max_hp: max as u32,
            shield: f.shield.max(0) as u32,
            phase: f.phase,
            pips: f.script.phases[1..].iter().map(|p| p.below).collect(),
            flags: u8::from(f.enraged),
        }));
    }

    /// Every fight's tick: shields, channels, phases, enrage, the next
    /// ability, the bar.
    fn run(&mut self, cx: &mut Ctx<'_>, ended: &mut Vec<u32>) {
        let mut k = 0;
        while k < self.fights.len() {
            let (slot, generation) = (self.fights[k].slot, self.fights[k].generation);
            let s = slot as usize;
            let store = cx.horde.store();
            if !store.is_alive(slot) || store.generation[s] != generation || store.hp[s] <= 0 {
                self.end(cx, k, ended);
                continue;
            }
            let max = cx.horde.block(slot).hp.max(1);
            let mut f = self.fights[k];

            // Mirror images: while one stands, nothing reaches the real one.
            if let Some(kind) = f.mirror {
                let store = cx.horde.store();
                let standing = f.adds[..f.n_adds as usize].iter().any(|&(m, g)| {
                    let mi = m as usize;
                    store.is_alive(m) && store.generation[mi] == g && store.kind[mi] == kind
                });
                // (Before its images arrive the cast is still winding up.)
                let arrived = !self.laters.iter().any(|l| l.fight == f.serial && matches!(l.deed, Deed::Summon { kind: k, .. } if k == kind));
                if standing || !arrived {
                    let hp = cx.horde.store().hp[s];
                    if hp < f.hp_seen {
                        cx.horde.store_mut().hp[s] = f.hp_seen;
                    }
                } else {
                    f.mirror = None;
                    Self::shout(cx, &f, MIRRORS_BROKEN);
                }
            }
            // Shield: what got through since the last tick is soaked up.
            let hp = cx.horde.store().hp[s];
            if f.shield > 0 && hp < f.hp_seen {
                let soak = f.shield.min(f.hp_seen - hp);
                cx.horde.store_mut().hp[s] += soak;
                f.shield -= soak;
                if f.shield == 0 && f.shield_cast != 0 {
                    // Broken in time: the big one never comes.
                    self.cancel(f.shield_cast, ended);
                    f.shield_cast = 0;
                    Self::broken(cx, &mut f);
                }
            }
            if f.shield > 0 && cx.now >= f.shield_until {
                f.shield = 0;
                f.shield_cast = 0;
            }
            let mut hp = cx.horde.store().hp[s];

            // Its body: a shell that turns spells back, cracked it takes
            // more, and it spills coins as it bleeds.
            if let Some(t) = f.traits {
                let reflect = (f.phase as u8) < t.reflect_until;
                let store = cx.horde.store_mut();
                if reflect {
                    store.flags[s] |= FLAG_REFLECT;
                } else {
                    store.flags[s] &= !FLAG_REFLECT;
                }
                if t.vulnerable_from > 0 && f.phase >= t.vulnerable_from && hp < f.hp_seen && hp > 0 {
                    let extra = (f.hp_seen - hp) * t.vulnerable_pct as i32 / 100;
                    hp = (hp - extra).max(1);
                    store.hp[s] = hp;
                }
                if t.burst_from > 0 && f.phase >= t.burst_from && t.burst_every > 0 {
                    if f.burst_next == 0 {
                        f.burst_next = f.script.phases[t.burst_from as usize].below.saturating_sub(t.burst_every).max(1);
                    }
                    while f.burst_next > 0 && hp as i64 * 100 <= f.burst_next as i64 * max as i64 {
                        self.bursts.push((MONSTER_ENTITY_BASE + slot as u32, store.x[s], store.y[s], t.burst_zeny.0, t.burst_zeny.1));
                        f.burst_next = f.burst_next.saturating_sub(t.burst_every);
                    }
                }
            }

            // Burrowed: it tunnels after its quarry until just before it bursts up.
            if let Some((who, speed, stop)) = f.tunnel
                && cx.now < stop
                && let Some(k) = self.up_ids.iter().position(|&id| id == who)
            {
                let (tx, ty) = self.ups[k];
                let (mx, my) = (cx.horde.store().x[s], cx.horde.store().y[s]);
                let d = ((tx - mx).powi(2) + (ty - my).powi(2)).sqrt();
                if let Some(dir) = norm(tx - mx, ty - my) {
                    let to = moved(cx.walk, mx, my, dir, (speed / cx.hz).min(d), MoveMode::Slide);
                    cx.horde.place(slot, to.0, to.1);
                }
            }

            if !f.engaged {
                let full = cx.horde.store().flags[s] & FLAG_FULL != 0;
                let (mx, my) = (cx.horde.store().x[s], cx.horde.store().y[s]);
                let r2 = f.script.aggro * f.script.aggro;
                if full && self.ups.iter().any(|&(x, y)| (x - mx).powi(2) + (y - my).powi(2) <= r2) {
                    f.engaged = true;
                    f.since = cx.now;
                    f.next_at = cx.now + cx.ticks(OPENING_SECS);
                    Self::shout(cx, &f, f.script.intro);
                    Self::bar(cx, &mut f, true);
                }
                f.hp_seen = hp;
                self.fights[k] = f;
                k += 1;
                continue;
            }

            // A channel: broken by burst damage, or done.
            if let Some(c) = f.channel {
                if c.mark - hp >= c.breaks {
                    f.channel = None;
                    self.cancel(c.cast, ended);
                    Self::broken(cx, &mut f);
                } else if cx.now >= c.until {
                    f.channel = None;
                    self.finish(cx, &mut f, c.ability, max);
                }
            }

            // Phases by HP; an opener goes first.
            let phases = f.script.phases;
            while (f.phase as usize + 1) < phases.len() && hp as i64 * 100 <= phases[f.phase as usize + 1].below as i64 * max as i64 {
                f.phase += 1;
                let p = &phases[f.phase as usize];
                f.step = 0;
                f.opener = p.opener;
                Self::shout(cx, &f, p.shout);
            }

            // Soft enrage.
            if !f.enraged && cx.now.saturating_sub(f.since) >= cx.ticks(f.script.enrage_secs) {
                f.enraged = true;
                Self::shout(cx, &f, f.script.enrage);
            }

            // The next ability.
            // Only while someone is still around (no volleys at the far end of the map).
            let held = cx.horde.store().status_of(slot).held();
            let (mx, my) = (cx.horde.store().x[s], cx.horde.store().y[s]);
            let leash = f.script.aggro * 1.5;
            let near = self.ups.iter().any(|&(x, y)| (x - mx).powi(2) + (y - my).powi(2) <= leash * leash);
            if cx.now >= f.next_at && cx.now >= f.busy_until && f.channel.is_none() && !held && near {
                let p = &phases[f.phase as usize];
                let ability = match f.opener.take() {
                    Some(a) => a,
                    None => {
                        let a = p.rotation[f.step % p.rotation.len()];
                        f.step += 1;
                        a
                    }
                };
                self.start(cx, &mut f, ability, max);
            }

            // An elite the party left behind drops out of the fight (its
            // bar goes); it picks up where it was when they come back.
            f.away = if near || f.script.rank == Rank::Boss { 0 } else { f.away + 1 };
            if f.away > cx.ticks(DISENGAGE_SECS) && f.channel.is_none() {
                f.engaged = false;
                f.away = 0;
                f.shield = 0;
                Self::gone(cx, &f);
            }

            f.hp_seen = cx.horde.store().hp[s];
            Self::bar(cx, &mut f, false);
            self.fights[k] = f;
            k += 1;
        }
    }

    /// The party broke a channel or a shield: the cast bar shows it, the
    /// monster reels.
    fn broken(cx: &mut Ctx<'_>, f: &mut Fight) {
        let stun = cx.ticks(BREAK_STUN_SECS);
        cx.horde.store_mut().afflict(f.slot, Condition::Stunned, stun.min(u16::MAX as u32) as u16);
        cx.msgs.push(ServerMsg::BossCast {
            caster: MONSTER_ENTITY_BASE + f.slot as u32,
            ability: String::new(),
            name: String::new(),
            cast_ms: 0,
            flags: cast_flags::INTERRUPTED,
        });
        Self::shout(cx, f, BROKEN);
        let gap = cx.ticks(f.script.phases[f.phase as usize].gap);
        f.busy_until = cx.now + stun;
        f.next_at = f.busy_until + gap;
    }

    /// A channel ran its course: heal, rally, swallow adds.
    fn finish(&mut self, cx: &mut Ctx<'_>, f: &mut Fight, ability: u8, max: i32) {
        let a = &f.script.abilities[ability as usize];
        let s = f.slot as usize;
        match a.action {
            Action::Channel { heal, rally, rally_r, .. } => {
                let store = cx.horde.store_mut();
                store.hp[s] = (store.hp[s] + max * heal as i32 / 100).min(max);
                if rally > 0 {
                    let (x, y) = (store.x[s], store.y[s]);
                    self.near.clear();
                    Area::Circle { x, y, r: rally_r }.monsters(cx.horde, &mut self.near);
                    for &m in &self.near {
                        // Its own body, and the field's plants, are no allies.
                        if m == f.slot || cx.horde.is_plant(m) {
                            continue;
                        }
                        let top = cx.horde.block(m).hp;
                        let store = cx.horde.store_mut();
                        store.hp[m as usize] = (store.hp[m as usize] + top * rally as i32 / 100).min(top);
                        for c in Condition::ALL {
                            store.cure(m, c);
                        }
                    }
                }
            }
            Action::Absorb { heal, radius, .. } => {
                let (x, y) = (cx.horde.store().x[s], cx.horde.store().y[s]);
                let mut eaten = 0;
                for i in 0..f.n_adds as usize {
                    let (m, g) = f.adds[i];
                    let store = cx.horde.store();
                    let mi = m as usize;
                    if store.is_alive(m) && store.generation[mi] == g && (store.x[mi] - x).powi(2) + (store.y[mi] - y).powi(2) <= radius * radius {
                        cx.horde.despawn(m);
                        eaten += 1;
                    }
                }
                f.n_adds = 0;
                let store = cx.horde.store_mut();
                store.hp[s] = (store.hp[s] + max * heal as i32 / 100 * eaten).min(max);
            }
            Action::HatchAll { radius, heal, .. } => {
                let (x, y) = (cx.horde.store().x[s], cx.horde.store().y[s]);
                self.hatch_all.push((f.serial, x, y, radius, heal));
            }
            _ => {}
        }
        let gap = cx.ticks(f.script.phases[f.phase as usize].gap);
        f.next_at = cx.now + gap;
    }

    /// Starts ability `index`: telegraphs, beats, deeds, the cast bar.
    fn start(&mut self, cx: &mut Ctx<'_>, f: &mut Fight, index: u8, max: i32) {
        let a: &'static Ability = &f.script.abilities[index as usize];
        let s = f.slot as usize;
        let caster = MONSTER_ENTITY_BASE + f.slot as u32;
        let (mx, my) = (cx.horde.store().x[s], cx.horde.store().y[s]);
        let phase = &f.script.phases[f.phase as usize];
        let pace = phase.haste * if f.enraged { ENRAGE_HASTE } else { 1.0 };
        let wind = cx.ticks(a.wind * pace);
        let gap = cx.ticks(phase.gap);
        let cast = self.next_serial();

        // Who it goes for.
        let nearest = self
            .ups
            .iter()
            .copied()
            .min_by(|p, q| ((p.0 - mx).powi(2) + (p.1 - my).powi(2)).total_cmp(&((q.0 - mx).powi(2) + (q.1 - my).powi(2))))
            .unwrap_or((mx, my));
        let aim = norm(nearest.0 - mx, nearest.1 - my).unwrap_or((1.0, 0.0));
        let mut anchors = [(0.0f32, 0.0f32); 8];
        let n = match a.anchor {
            Anchor::Boss => {
                anchors[0] = (mx, my);
                1
            }
            Anchor::Nearest => {
                anchors[0] = nearest;
                1
            }
            Anchor::Farthest => {
                anchors[0] = self
                    .ups
                    .iter()
                    .copied()
                    .max_by(|p, q| ((p.0 - mx).powi(2) + (p.1 - my).powi(2)).total_cmp(&((q.0 - mx).powi(2) + (q.1 - my).powi(2))))
                    .unwrap_or((mx, my));
                1
            }
            Anchor::Marked => {
                anchors[0] = self.ups[cx.dice.die(self.ups.len() as u32) as usize - 1];
                1
            }
            Anchor::Each(k) => {
                let n = self.ups.len().min(k as usize).min(anchors.len());
                anchors[..n].copy_from_slice(&self.ups[..n]);
                n
            }
        };
        // A retreat: it flies from the nearest player; the zone is where it lands.
        if let Action::Retreat { dist } = a.action {
            let away = (-aim.0, -aim.1);
            let to = moved(cx.walk, mx, my, away, dist, MoveMode::Blink);
            anchors[0] = to;
            self.laters.push(Later { fight: f.serial, cast, at: cx.now + wind, deed: Deed::Place { x: to.0, y: to.1 } });
        }

        // Shapes, anchor by anchor (a chain of leaps starts each from the last).
        let spin = cx.dice.die(360) as f32 * TAU / 360.0;
        self.shapes.clear();
        let mut origin = (mx, my);
        let mut stagger = [0u32; 8];
        let mut counts = [0usize; 8];
        for (i, &anchor) in anchors[..n].iter().enumerate() {
            let before = self.shapes.len();
            expand(&a.pattern, &a.action, cx.walk, origin, anchor, aim, spin, pace, &mut self.shapes);
            counts[i] = self.shapes.len() - before;
            stagger[i] = cx.ticks(a.stagger * i as f32 * pace);
            // A chain of leaps or charges: each starts where the last ended.
            if matches!(a.action, Action::Leap | Action::Charge) && a.stagger > 0.0 {
                origin = self.shapes[before..].iter().rev().find_map(|s| s.place).unwrap_or(origin);
            }
        }

        // Crossfire: bars between the monster and its standing images, and
        // from each of them toward the nearest player.
        let n = if let Pattern::Crossfire { width, len } = a.pattern {
            self.shapes.clear();
            let mut points = [(mx, my); 1 + MAX_ADDS];
            let mut k = 1;
            if let Some(kind) = f.mirror {
                let store = cx.horde.store();
                for &(m, g) in &f.adds[..f.n_adds as usize] {
                    let mi = m as usize;
                    if store.is_alive(m) && store.generation[mi] == g && store.kind[mi] == kind {
                        points[k] = (store.x[mi], store.y[mi]);
                        k += 1;
                    }
                }
            }
            let mut bar = |from: (f32, f32), dir: (f32, f32), length: f32| {
                self.shapes.push(Shape { zone: Zone::Area(Area::line(from.0, from.1, dir, length, width)), delay: 0.0, safe: false, place: None });
            };
            if k == 1 {
                for t in [0.0, 120.0, 240.0] {
                    bar((mx, my), turn(aim, t), len);
                }
            } else {
                for i in 0..k {
                    for q in &points[i + 1..k] {
                        let (dx, dy) = (q.0 - points[i].0, q.1 - points[i].1);
                        if let Some(dir) = norm(dx, dy) {
                            bar(points[i], dir, (dx * dx + dy * dy).sqrt());
                        }
                    }
                    if let Some(dir) = norm(nearest.0 - points[i].0, nearest.1 - points[i].1) {
                        bar(points[i], dir, len);
                    }
                }
            }
            counts[0] = self.shapes.len();
            stagger[0] = 0;
            1
        } else {
            n
        };
        // A nova the lit lights shelter from (moonlit circles make up the
        // rest when too few burn).
        let n = if let Pattern::LitSanctuary { r, lit_r, min, safe_r, dist } = a.pattern {
            self.shapes.clear();
            self.shapes.push(Shape { zone: circle(mx, my, r), delay: 0.0, safe: false, place: None });
            let mut lit: Vec<(f32, f32)> = self.lights.iter().copied().filter(|&(x, y)| (x - mx).powi(2) + (y - my).powi(2) <= r * r).collect();
            lit.sort_by(|p, q| ((p.0 - mx).powi(2) + (p.1 - my).powi(2)).total_cmp(&((q.0 - mx).powi(2) + (q.1 - my).powi(2))));
            lit.truncate(MAX_SAFE);
            for &(x, y) in &lit {
                self.shapes.push(Shape { zone: circle(x, y, lit_r), delay: 0.0, safe: true, place: None });
            }
            let need = (min as usize).saturating_sub(lit.len()).min(MAX_SAFE - lit.len());
            for i in 0..need {
                let t = spin + i as f32 * TAU / need as f32;
                let (sx, sy) = moved(cx.walk, mx, my, (t.cos(), t.sin()), dist, MoveMode::Blink);
                self.shapes.push(Shape { zone: circle(sx, sy, safe_r), delay: 0.0, safe: true, place: None });
            }
            counts[0] = self.shapes.len();
            stagger[0] = 0;
            1
        } else {
            n
        };
        // Who each anchor is (a chasing zone follows them).
        let mut ids = [0u32; 8];
        for (i, &anchor) in anchors[..n].iter().enumerate() {
            ids[i] = self.ups.iter().position(|&u| u == anchor).map_or(0, |k| self.up_ids[k]);
        }

        // Beats, deeds and what the party sees.
        let block = cx.horde.block(f.slot);
        let def = cx.horde.catalog().get(cx.horde.store().kind[s]);
        let evil = def.evil;
        let mut safe = [(0.0f32, 0.0f32, 0.0f32); MAX_SAFE];
        let mut n_safe = 0u8;
        for sh in self.shapes.iter().filter(|sh| sh.safe) {
            if let Zone::Area(Area::Circle { x, y, r }) = sh.zone
                && (n_safe as usize) < MAX_SAFE
            {
                safe[n_safe as usize] = (x, y, r);
                n_safe += 1;
            }
        }
        let mut zones = Vec::with_capacity(self.shapes.len());
        let mut last = cx.now + wind;
        let mut j = 0;
        for (i, &count) in counts[..n].iter().enumerate() {
            for sh in &self.shapes[j..j + count] {
                let at = cx.now + wind + stagger[i] + cx.ticks(sh.delay);
                last = last.max(at);
                let lasting = match a.pattern {
                    Pattern::Pool { secs, pulse, .. } => Some((secs, pulse)),
                    _ => None,
                };
                let zone_id = if lasting.is_some() {
                    let id = *cx.next_zone;
                    *cx.next_zone = cx.next_zone.wrapping_add(1).max(1);
                    id
                } else {
                    0
                };
                let span_ms = lasting.map_or(0, |(secs, _)| (secs * 1000.0) as u32);
                let burrow = matches!(a.action, Action::Burrow { .. });
                // A burrowing boss's ripple rides on its mound, not on the player.
                let follow = if burrow {
                    caster
                } else if matches!(a.pattern, Pattern::Chase { .. }) {
                    ids[i]
                } else {
                    0
                };
                let mut area = sh.zone.telegraph(zone_id, if sh.safe { crate::world::protocol::look::WARD } else { a.look }, span_ms);
                area.follow = follow;
                zones.push(BossZone {
                    area,
                    delay_ms: (at - cx.now) * 1000 / cx.hz.max(1.0) as u32,
                    flags: if sh.safe {
                        zone_flags::SAFE
                    } else if matches!(a.pattern, Pattern::Cross { .. }) {
                        zone_flags::CENTRE
                    } else if sh.place.is_some() && matches!(a.action, Action::Leap) {
                        zone_flags::LANDS
                    } else {
                        0
                    },
                });
                if let Some((x, y)) = sh.place {
                    self.laters.push(Later { fight: f.serial, cast, at, deed: Deed::Place { x, y } });
                }
                if sh.safe || (a.hurt.dice.0 == 0 && a.hurt.rider.is_none()) {
                    continue;
                }
                let (until, pulse) = match lasting {
                    Some((secs, pulse)) => (at + cx.ticks(secs), cx.ticks(pulse).max(1)),
                    None => (at, 0),
                };
                self.beats.push(Beat {
                    fight: f.serial,
                    cast,
                    map: cx.map,
                    next: at,
                    until,
                    pulse,
                    zone: sh.zone,
                    zone_id,
                    safe,
                    n_safe,
                    hurt: a.hurt,
                    level: block.level,
                    dc: 10 + block.save,
                    boost: if f.enraged { ENRAGE_DAMAGE } else { 1.0 },
                    evil,
                    from: (mx, my),
                    follow: if burrow { 0 } else { follow },
                    follow_mob: burrow.then(|| (f.slot, f.generation)),
                });
                if burrow {
                    // It bursts up under this beat, then tunnels on to the next.
                    let next = ids.get(i + 1).copied().filter(|_| i + 1 < n).unwrap_or(0);
                    self.laters.push(Later { fight: f.serial, cast, at, deed: Deed::Surface { last: i + 1 == n, next } });
                }
            }
            j += count;
        }

        // A trail: puddles dropped under each anchored player as it runs.
        if let Pattern::Trail { count, every, fuse, .. } = a.pattern {
            for (i, &id) in ids[..n].iter().enumerate() {
                for t in 0..count {
                    let at = cx.now + wind + stagger[i] + cx.ticks(t as f32 * every * pace);
                    last = last.max(at + cx.ticks(fuse));
                    self.laters.push(Later { fight: f.serial, cast, at, deed: Deed::Puddle { ability: index, follow: id } });
                }
            }
        }

        // What the monster itself does.
        let mut flags = 0;
        let mut cast_ms = (wind as f32 * 1000.0 / cx.hz) as u32;
        let mut busy = cx.now + wind;
        match a.action {
            Action::Charge | Action::Leap => busy = last,
            Action::Summon { kind, count, per_player } => {
                if let Some(kind) = cx.horde.catalog().kind_of(kind) {
                    let n = count as u32 + per_player as u32 * cx.party.saturating_sub(1);
                    self.laters.push(Later { fight: f.serial, cast, at: cx.now + wind, deed: Deed::Summon { kind, count: n.min(MAX_ADDS as u32) as u8 } });
                }
            }
            Action::Channel { secs, break_pct, .. } => {
                let until = cx.now + wind + cx.ticks(secs);
                let breaks = cx.party_scale(max * break_pct as i32 / 100).max(1);
                f.channel = Some(Channel { ability: index, until, mark: cx.horde.store().hp[s], breaks, cast });
                flags |= cast_flags::CHANNEL;
                cast_ms = ((until - cx.now) as f32 * 1000.0 / cx.hz) as u32;
                busy = until;
            }
            Action::Absorb { secs, .. } => {
                let until = cx.now + wind + cx.ticks(secs);
                f.channel = Some(Channel { ability: index, until, mark: cx.horde.store().hp[s], breaks: i32::MAX, cast });
                cast_ms = ((until - cx.now) as f32 * 1000.0 / cx.hz) as u32;
                busy = until;
            }
            Action::Shield { pct } => {
                f.shield = cx.party_scale(max * pct as i32 / 100).max(1);
                f.shield_cast = cast;
                f.shield_until = last + 1;
                flags |= cast_flags::SHIELDED;
            }
            Action::Mirror { kind, count, per_player } => {
                if let Some(kind) = cx.horde.catalog().kind_of(kind) {
                    let n = count as u32 + per_player as u32 * cx.party.saturating_sub(1);
                    f.mirror = Some(kind);
                    self.laters.push(Later { fight: f.serial, cast, at: cx.now + wind, deed: Deed::Summon { kind, count: n.min(MAX_ADDS as u32) as u8 } });
                }
            }
            Action::Burrow { speed } => {
                // Under at once; it tunnels through the wind-up and stops
                // under the ripple a moment before it bursts up.
                cx.horde.set_burrowed(f.slot, true);
                let stop = (cx.now + wind + stagger[0]).saturating_sub(cx.ticks(TUNNEL_FUSE_SECS));
                f.tunnel = Some((ids[0], speed, stop));
                busy = last;
            }
            Action::Lay { kind, count, per_player, ring, hatch, hatch_count, hatch_secs } => {
                if let Some(k) = cx.horde.catalog().kind_of(kind) {
                    let n = (count as u32 + per_player as u32 * cx.party.saturating_sub(1)).min(MAX_ADDS as u32) as u8;
                    let hatch = cx.horde.catalog().kind_of(hatch).map(|h| (h, hatch_count, hatch_secs));
                    self.laters.push(Later { fight: f.serial, cast, at: cx.now + wind, deed: Deed::Lay { kind: k, count: n, ring, hatch } });
                }
            }
            Action::HatchAll { secs, break_pct, .. } => {
                let until = cx.now + wind + cx.ticks(secs);
                let breaks = cx.party_scale(max * break_pct as i32 / 100).max(1);
                f.channel = Some(Channel { ability: index, until, mark: cx.horde.store().hp[s], breaks, cast });
                flags |= cast_flags::CHANNEL;
                cast_ms = ((until - cx.now) as f32 * 1000.0 / cx.hz) as u32;
                busy = until;
            }
            Action::None | Action::Retreat { .. } => {}
        }
        f.busy_until = busy + 1;
        // The pause counts from the last beat, not the wind-up's end: an
        // ability's rings and sweeps play out before the next one starts.
        f.next_at = f.busy_until.max(last + 1) + gap;
        // It stands its ground through the wind-up and holds its melee.
        let hold = (busy.saturating_sub(cx.now) + 1).min(u16::MAX as u32) as u16;
        let store = cx.horde.store_mut();
        store.afflict(f.slot, Condition::Rooted, hold);
        store.cooldown[s] = store.cooldown[s].max(hold.saturating_add(1));

        if a.shake > 0 {
            // As the first beat lands (a slam, an eruption).
            self.laters.push(Later { fight: f.serial, cast, at: cx.now + wind, deed: Deed::Shake { intensity: a.shake, ms: 600 } });
        }
        Self::shout(cx, f, a.shout);
        cx.msgs.push(ServerMsg::BossCast { caster, ability: a.id.into(), name: a.name.into(), cast_ms, flags });
        if !zones.is_empty() {
            cx.msgs.push(ServerMsg::BossZones { caster, ability: a.id.into(), zones });
        }
    }

    /// Deeds that are due: landings and arrivals.
    fn act(&mut self, cx: &mut Ctx<'_>) {
        let mut i = 0;
        while i < self.laters.len() {
            if self.laters[i].at > cx.now {
                i += 1;
                continue;
            }
            let l = self.laters.swap_remove(i);
            let Some(k) = self.fights.iter().position(|f| f.serial == l.fight) else { continue };
            let f = &mut self.fights[k];
            let store = cx.horde.store();
            if !store.is_alive(f.slot) || store.generation[f.slot as usize] != f.generation {
                continue;
            }
            match l.deed {
                Deed::Place { x, y } => {
                    if cx.walk.is_walkable(x, y) {
                        cx.horde.place(f.slot, x, y);
                    }
                }
                Deed::Shake { intensity, ms } => cx.msgs.push(ServerMsg::EnvShake { intensity, duration_ms: ms }),
                Deed::Surface { last, next } => {
                    if last {
                        f.tunnel = None;
                        cx.horde.set_burrowed(f.slot, false);
                    } else {
                        // Up for the blow, then down again after the next one.
                        cx.horde.set_burrowed(f.slot, false);
                        cx.horde.set_burrowed(f.slot, true);
                        let speed = f.tunnel.map_or(7.0, |t| t.1);
                        let a = self.laters.iter().filter(|o| o.fight == f.serial && matches!(o.deed, Deed::Surface { .. })).map(|o| o.at).min().unwrap_or(cx.now);
                        f.tunnel = Some((next, speed, a.saturating_sub(cx.ticks(TUNNEL_FUSE_SECS))));
                    }
                }
                Deed::Lay { kind, count, ring, hatch } => {
                    let (x, y) = (store.x[f.slot as usize], store.y[f.slot as usize]);
                    let base = cx.dice.die(360) as f32;
                    let run_now = cx.horde.director_elapsed();
                    for j in 0..count {
                        let a = (base + j as f32 * 360.0 / count.max(1) as f32).to_radians();
                        let Some(m) = cx.horde.place_object(kind, x + a.cos() * ring, y + a.sin() * ring) else { continue };
                        let g = cx.horde.store().generation[m as usize];
                        self.laid.push((m, g, hatch.map(|(k, c, secs)| (run_now + secs, k, c))));
                    }
                }
                Deed::Puddle { ability, follow } => {
                    let a = &f.script.abilities[ability as usize];
                    let Pattern::Trail { r, fuse, secs, pulse, .. } = a.pattern else { continue };
                    // Where the player stands now: the puddle stays there.
                    let Some(k) = self.up_ids.iter().position(|&id| id == follow) else { continue };
                    let (px, py) = self.ups[k];
                    let id = *cx.next_zone;
                    *cx.next_zone = cx.next_zone.wrapping_add(1).max(1);
                    let zone = circle(px, py, r);
                    let at = cx.now + cx.ticks(fuse);
                    let block = cx.horde.block(f.slot);
                    let caster = MONSTER_ENTITY_BASE + f.slot as u32;
                    cx.msgs.push(ServerMsg::BossZones {
                        caster,
                        ability: a.id.into(),
                        zones: vec![BossZone { area: zone.telegraph(id, a.look, (secs * 1000.0) as u32), delay_ms: (fuse * 1000.0) as u32, flags: 0 }],
                    });
                    self.beats.push(Beat {
                        fight: f.serial,
                        cast: l.cast,
                        map: cx.map,
                        next: at,
                        until: at + cx.ticks(secs),
                        pulse: cx.ticks(pulse).max(1),
                        zone,
                        zone_id: id,
                        safe: [(0.0, 0.0, 0.0); MAX_SAFE],
                        n_safe: 0,
                        hurt: a.hurt,
                        level: block.level,
                        dc: 10 + block.save,
                        boost: if f.enraged { ENRAGE_DAMAGE } else { 1.0 },
                        evil: cx.horde.catalog().get(f.kind).evil,
                        from: (store.x[f.slot as usize], store.y[f.slot as usize]),
                        follow: 0,
                        follow_mob: None,
                    });
                }
                Deed::Summon { kind, count } => {
                    // Forget the adds that already fell.
                    let mut keep = 0;
                    for j in 0..f.n_adds as usize {
                        let (m, g) = f.adds[j];
                        if store.is_alive(m) && store.generation[m as usize] == g {
                            f.adds[keep] = f.adds[j];
                            keep += 1;
                        }
                    }
                    f.n_adds = keep as u8;
                    let (x, y) = (store.x[f.slot as usize], store.y[f.slot as usize]);
                    let base = cx.dice.die(360) as f32;
                    for j in 0..count {
                        let a = (base + j as f32 * 360.0 / count.max(1) as f32).to_radians();
                        let r = 2.5 + (j % 3) as f32;
                        let Some(m) = cx.horde.spawn(kind, x + a.cos() * r, y + a.sin() * r) else { continue };
                        // Adds join the fight at once.
                        cx.horde.store_mut().flags[m as usize] |= FLAG_FULL;
                        if (f.n_adds as usize) < MAX_ADDS {
                            f.adds[f.n_adds as usize] = (m, cx.horde.store().generation[m as usize]);
                            f.n_adds += 1;
                        }
                    }
                }
            }
        }
    }
}

/// One beat on every standing player inside it (and out of the sanctuaries).
fn strike(b: &Beat, players: &mut [HubPlayer], lives: &[Live], rng: &mut Rng, hz: f32, hits: &mut Vec<HitEvent>) {
    // A chasing zone lands wherever its player is now.
    let zone = match b.zone {
        Zone::Area(Area::Circle { r, .. }) if b.follow != 0 => players
            .iter()
            .find(|p| p.map == b.map && p.profile.character_id == b.follow)
            .map_or(b.zone, |p| circle(p.x, p.y, r)),
        z => z,
    };
    for p in players.iter_mut().filter(|p| p.map == b.map) {
        let (x, y, id) = (p.x, p.y, p.profile.character_id);
        let Some(f) = p.fighter.as_mut() else { continue };
        if !f.is_up() || !zone.contains(x, y, BODY) {
            continue;
        }
        if b.safe[..b.n_safe as usize].iter().any(|&(sx, sy, r)| (x - sx).powi(2) + (y - sy).powi(2) <= r * r) {
            continue;
        }
        if b.hurt.ranged && lives.iter().any(|l| l.deflects(b.map, x, y)) {
            hits.push(HitEvent { target: id, amount: 0, flags: crate::world::protocol::hit::GUARD });
            continue;
        }
        f.quiet = 0;
        let h = b.hurt;
        let saved = h.save && !f.status.saves_fail() && saving_throw(rng, f.save_against(h.rider.map(|r| r.0)), b.dc, 0);
        if h.dice.0 > 0 {
            let dice = Dice::new(h.dice.0 as u32, h.dice.1 as u32, (b.level / 4) as i32);
            let mut amount = (dice.roll(rng, false) as f32 * b.boost) as i32;
            if saved {
                amount /= 2;
            }
            let taken = f.take_from(amount, h.kind == crate::world::combat::DamageType::Physical, b.evil);
            hits.push(HitEvent { target: id, amount: taken as u32, flags: damage_type_bits(h.kind) });
        }
        if let Some((c, secs)) = h.rider {
            // A save resists the rider; mechanics without a save always land it.
            let resisted = if h.save { saved } else { false };
            if !resisted {
                let ticks = ticks_of(secs, hz).min(u16::MAX as u32) as u16;
                if c == Condition::Feared {
                    // It runs from the monster that cast it.
                    f.frighten(b.from, ticks);
                } else {
                    f.afflict(c, ticks);
                }
            }
        }
        f.fall_if_spent(hz);
    }
}

impl HubInstance {
    /// Scripted fights: inside [`HubInstance::fight`] after the monster
    /// skills, before monster melee.
    pub(super) fn boss_step(&mut self, out: &mut Outgoing, hits: &mut Vec<HitEvent>) {
        let Some(horde) = self.horde.as_mut() else { return };
        let b = &mut self.bosses;
        if b.kinds.len() != horde.catalog().defs().len() {
            b.kinds = horde.catalog().defs().iter().map(|d| scripts::find(&d.id)).collect();
            b.any = b.kinds.iter().any(Option::is_some);
        }
        if !b.any {
            return;
        }
        let map = horde.map();
        let hz = self.config.tick_hz as f32;
        let now = self.tick;
        b.ups.clear();
        b.up_ids.clear();
        b.lights.clear();
        b.lights.extend(self.env.payon.light_spots());
        let mut party = 0;
        for p in self.players.iter().filter(|p| p.map == map) {
            let Some(f) = p.fighter.as_ref() else { continue };
            party += 1;
            if f.hunted() {
                b.ups.push((p.x, p.y));
                b.up_ids.push(p.profile.character_id);
            }
        }
        let mut msgs = Vec::new();
        let mut ended = Vec::new();
        let mut cx = Ctx {
            horde,
            walk: &self.world.map(map).walk,
            dice: &mut self.dice,
            next_zone: &mut self.next_zone,
            msgs: &mut msgs,
            map,
            now,
            hz,
            party,
        };
        b.discover(&mut cx);
        b.run(&mut cx, &mut ended);
        b.act(&mut cx);

        // Beats land; lasting ground bites.
        let mut k = 0;
        while k < b.beats.len() {
            let beat = &mut b.beats[k];
            if now > beat.until {
                b.beats.swap_remove(k);
                continue;
            }
            if now >= beat.next {
                beat.next = if beat.pulse > 0 { now + beat.pulse } else { beat.until + 1 };
                let mut beat = *beat;
                // A ripple riding on a burrowed monster lands where it is.
                if let (Some((m, g)), Zone::Area(Area::Circle { r, .. })) = (beat.follow_mob, beat.zone)
                    && let Some(horde) = self.horde.as_ref()
                    && horde.store().is_alive(m)
                    && horde.store().generation[m as usize] == g
                {
                    beat.zone = circle(horde.store().x[m as usize], horde.store().y[m as usize], r);
                }
                strike(&beat, &mut self.players, &self.lives, &mut self.dice, hz, hits);
            }
            k += 1;
        }

        msgs.extend(ended.into_iter().map(|id| ServerMsg::ZoneEnd { id }));
        self.boss_requests(map, out);
        if msgs.is_empty() {
            return;
        }
        for p in self.players.iter().filter(|p| p.map == map) {
            for m in &msgs {
                out.push((p.profile.user_id, HubEvent::Control(m.clone())));
            }
        }
    }
}

impl HubInstance {
    /// What the fights asked of the stage this step: eggs laid join the
    /// environment's objects, brood calls hatch the objects round them (and
    /// heal the caller per one), cracked shells spill zeny to everyone on
    /// the map.
    fn boss_requests(&mut self, map: MapIdx, out: &mut Outgoing) {
        let laid = std::mem::take(&mut self.bosses.laid);
        for (slot, generation, hatch) in laid {
            self.env_adopt(slot, generation, hatch);
        }
        let calls = std::mem::take(&mut self.bosses.hatch_all);
        for (serial, x, y, r, heal) in calls {
            let n = self.env_hatch_near(x, y, r);
            let Some(f) = self.bosses.fights.iter().find(|f| f.serial == serial) else { continue };
            let Some(horde) = self.horde.as_mut() else { continue };
            let (slot, s) = (f.slot, f.slot as usize);
            if n > 0 && horde.store().is_alive(slot) && horde.store().generation[s] == f.generation {
                let max = horde.block(slot).hp;
                let store = horde.store_mut();
                store.hp[s] = (store.hp[s] + max * heal as i32 / 100 * n as i32).min(max);
            }
        }
        let bursts = std::mem::take(&mut self.bosses.bursts);
        for (entity_id, x, y, lo, hi) in bursts {
            for p in self.players.iter_mut().filter(|p| p.map == map) {
                let Some(f) = p.fighter.as_mut() else { continue };
                let n = lo + self.dice.die(hi.saturating_sub(lo) + 1) - 1;
                f.add_zeny(n as u64);
                f.run_zeny += n as u64;
                let msg = ServerMsg::Loot { entity_id, x, y, item: "zeny".into(), count: n, rare: false };
                out.push((p.profile.user_id, HubEvent::Control(msg)));
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "boss/culvert_tests.rs"]
mod culvert_tests;
#[cfg(test)]
mod payon_tests;
