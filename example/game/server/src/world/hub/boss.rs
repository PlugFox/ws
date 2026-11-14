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
use crate::world::horde::storage::FLAG_FULL;
use crate::world::horde::Horde;
use crate::world::map::WalkGrid;
use crate::world::protocol::{cast_flags, zone_flags, BossBarInfo, BossZone, HitEvent, ServerMsg, Telegraph, MONSTER_ENTITY_BASE};
use scripts::{Ability, Action, Anchor, Hurt, Pattern, Rank, Script};
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
}

/// Something the monster does later in an ability.
#[derive(Clone, Copy, Debug)]
enum Deed {
    /// It lands here.
    Place { x: f32, y: f32 },
    /// Its adds arrive.
    Summon { kind: u8, count: u8 },
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
    shapes: Vec<Shape>,
    near: Vec<u16>,
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
            let hp = cx.horde.store().hp[s];

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
            if matches!(a.action, Action::Leap) && a.stagger > 0.0 {
                origin = self.shapes[before..].iter().rev().find_map(|s| s.place).unwrap_or(origin);
            }
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
                zones.push(BossZone {
                    area: sh.zone.telegraph(zone_id, if sh.safe { crate::world::protocol::look::WARD } else { a.look }, span_ms),
                    delay_ms: (at - cx.now) * 1000 / cx.hz.max(1.0) as u32,
                    flags: if sh.safe {
                        zone_flags::SAFE
                    } else if matches!(a.pattern, Pattern::Cross { .. }) {
                        zone_flags::CENTRE
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
                });
            }
            j += count;
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
            Action::None | Action::Retreat { .. } => {}
        }
        f.busy_until = busy + 1;
        f.next_at = f.busy_until + gap;
        // It stands its ground through the wind-up and holds its melee.
        let hold = (busy.saturating_sub(cx.now) + 1).min(u16::MAX as u32) as u16;
        let store = cx.horde.store_mut();
        store.afflict(f.slot, Condition::Rooted, hold);
        store.cooldown[s] = store.cooldown[s].max(hold.saturating_add(1));

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
    for p in players.iter_mut().filter(|p| p.map == b.map) {
        let (x, y, id) = (p.x, p.y, p.profile.character_id);
        let Some(f) = p.fighter.as_mut() else { continue };
        if !f.is_up() || !b.zone.contains(x, y, BODY) {
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
                f.afflict(c, ticks_of(secs, hz).min(u16::MAX as u32) as u16);
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
        let mut party = 0;
        for p in self.players.iter().filter(|p| p.map == map) {
            let Some(f) = p.fighter.as_ref() else { continue };
            party += 1;
            if f.hunted() {
                b.ups.push((p.x, p.y));
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
                let beat = *beat;
                strike(&beat, &mut self.players, &self.lives, &mut self.dice, hz, hits);
            }
            k += 1;
        }

        msgs.extend(ended.into_iter().map(|id| ServerMsg::ZoneEnd { id }));
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

#[cfg(test)]
mod tests;
