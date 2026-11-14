//! Mission environment (COMBAT.md "Mission platform"): the generic
//! extension points a location's mission builds its mechanics on, without
//! touching the stage loop. Definitions only; the stage runs them in
//! `hub/env.rs`.
//!
//! * [`Hazard`]: a patch of ground that hurts, slows, afflicts or pulls
//!   whoever stands in it. Declared in the mission table
//!   ([`Environment::hazards`]) or baked with the location
//!   (`<dist>/server/locations/<map>/hazards.json`, a JSON array of the same
//!   shape, [`load_hazards`]). Always on, or only while a [`MapEvent`] runs
//!   ([`Hazard::during`]).
//! * [`Environment::light_scale`] and [`MapEvent::light_scale`]: the light
//!   radius of the run (darkness in a cave, a sandstorm closing in), at
//!   launch and as events come and go. Spawns, snapshots and the client's
//!   darkness all follow the scaled radius.
//! * [`MapEvent`]: a timed beat of the map (a tide, a sandstorm) every N
//!   seconds with a warning first; it shows on the mission timeline
//!   (`MISSION_INFO`), is announced (`ENVIRONMENT`) and switches its
//!   hazards and light on for its duration.
//! * [`ObjectDef`]: interactive things on the field (braziers to light,
//!   eggs to break). They are plants (`archetype = "plant"` monsters:
//!   rooted, harmless, felled in a fixed number of hits, COMBAT.md §4.10)
//!   planted at fixed spots, and felling one fires its [`ObjectEffect`].
//!
//! Everything here is plain data with `'static` defaults, so a mission
//! table stays a `const`, and hazards also deserialize from the bake.

use crate::world::combat::{Condition, DamageType};
use crate::world::map::MapError;
use crate::world::protocol::look;
use serde::Deserialize;
use std::path::Path;

/// Where a hazard lies (cells).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HazardShape {
    Circle { x: f32, y: f32, r: f32 },
    /// Axis-aligned box centred on `(x, y)`, `w` × `h` cells.
    Rect { x: f32, y: f32, w: f32, h: f32 },
}

impl HazardShape {
    /// Whether a body of radius `body` at `(px, py)` touches it.
    pub fn contains(&self, px: f32, py: f32, body: f32) -> bool {
        match *self {
            HazardShape::Circle { x, y, r } => crate::world::horde::in_radius(px - x, py - y, r + body),
            HazardShape::Rect { x, y, w, h } => (px - x).abs() <= w / 2.0 + body && (py - y).abs() <= h / 2.0 + body,
        }
    }

    pub fn centre(&self) -> (f32, f32) {
        match *self {
            HazardShape::Circle { x, y, .. } | HazardShape::Rect { x, y, .. } => (x, y),
        }
    }
}

/// A patch of hostile ground. Every effect is optional; a hazard with none
/// is pure decoration. Plain `'static` data so a mission table stays a
/// `const` (`Hazard { damage: [1, 4], ..Hazard::circle("acid", 10.0, 10.0, 3.0) }`);
/// the bake's rows ([`RawHazard`]) are leaked into the same shape once at
/// boot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hazard {
    /// Unique on the map (ZONE ids, [`ObjectEffect::EndHazard`]).
    pub id: &'static str,
    pub shape: HazardShape,
    /// Ground look for clients: a `protocol::look` name (`poison`, `swamp`,
    /// `fire`, `cold`, `dark`, ...).
    pub look: &'static str,
    /// Dice rolled per pulse on each standing player inside, `[n, sides]`
    /// (`n = 0`: no damage), plus a quarter of the mission band's minimum
    /// level (as boss beats add `level / 4`).
    pub damage: [u8; 2],
    /// `combat::DamageType` id of the damage (armour and resistances apply).
    pub damage_type: &'static str,
    /// Damage and the condition land this often (seconds).
    pub every_secs: f32,
    /// Condition left on whoever is inside (`combat::Condition` id:
    /// `slowed`, `poisoned`, `burning`, `blinded`, `mired`, ...) for
    /// `condition_secs` (at least until the next pulse, so it holds while
    /// inside and wears off soon after leaving).
    pub condition: Option<&'static str>,
    pub condition_secs: f32,
    /// Cells per second toward the centre (negative pushes out). Walls stop
    /// it.
    pub pull: f32,
    /// Saving throw DC (0 = no save): a save against the condition (Body
    /// without one) halves the damage and shakes off the condition for that
    /// pulse.
    pub dc: i32,
    /// Only while this [`MapEvent`] runs; `None` = the whole run.
    pub during: Option<&'static str>,
}

/// A row of `hazards.json` (the fields of [`Hazard`], with defaults).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RawHazard {
    pub id: String,
    pub shape: HazardShape,
    /// Ground look for clients: a `protocol::look` name (`poison`, `swamp`,
    /// `fire`, `cold`, `dark`, ...).
    #[serde(default = "default_look")]
    pub look: String,
    /// Dice rolled per pulse on each standing player inside, `[n, sides]`
    /// (`n = 0`: no damage), plus `level / 4` of the mission band's
    /// minimum (the same bonus boss beats use).
    #[serde(default)]
    pub damage: [u8; 2],
    /// `combat::DamageType` id of the damage (armour and resistances apply).
    #[serde(default = "default_damage_type")]
    pub damage_type: String,
    /// Damage and the condition land this often (seconds).
    #[serde(default = "one_sec")]
    pub every_secs: f32,
    /// Condition left on whoever is inside (`combat::Condition` id:
    /// `slowed`, `poisoned`, `burning`, `blinded`, `mired`, ...) for
    /// `condition_secs`; renewed each pulse, so it wears off soon after
    /// leaving.
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default = "one_sec")]
    pub condition_secs: f32,
    /// Cells per second toward the centre (negative pushes out). Walls stop
    /// it; it never moves a player off walkable ground.
    #[serde(default)]
    pub pull: f32,
    /// Saving throw DC (0 = no save): a Body save halves the damage and
    /// shakes off the condition for that pulse.
    #[serde(default)]
    pub dc: i32,
    /// Only while this [`MapEvent`] runs; `None` = the whole run.
    #[serde(default)]
    pub during: Option<String>,
}

fn default_look() -> String {
    "poison".into()
}
fn default_damage_type() -> String {
    "poison".into()
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

impl RawHazard {
    /// The row as a [`Hazard`]: its strings live for the process (a map's
    /// handful of rows, loaded once at boot).
    pub fn leak(self) -> Hazard {
        Hazard {
            id: leak(self.id),
            shape: self.shape,
            look: leak(self.look),
            damage: self.damage,
            damage_type: leak(self.damage_type),
            every_secs: self.every_secs,
            condition: self.condition.map(leak),
            condition_secs: self.condition_secs,
            pull: self.pull,
            dc: self.dc,
            during: self.during.map(leak),
        }
    }
}
fn one_sec() -> f32 {
    1.0
}

impl Hazard {
    /// A plain always-on circle with no effect yet; set the fields after
    /// (`Hazard { damage: [1, 4], ..Hazard::circle("acid", 10.0, 10.0, 3.0) }`).
    pub const fn circle(id: &'static str, x: f32, y: f32, r: f32) -> Hazard {
        Hazard {
            id,
            shape: HazardShape::Circle { x, y, r },
            look: "poison",
            damage: [0, 0],
            damage_type: "poison",
            every_secs: 1.0,
            condition: None,
            condition_secs: 1.0,
            pull: 0.0,
            dc: 0,
            during: None,
        }
    }

    /// The condition it leaves, if it names a known one.
    pub fn condition(&self) -> Option<Condition> {
        self.condition.and_then(Condition::from_id)
    }

    pub fn damage_type(&self) -> DamageType {
        DamageType::from_id(self.damage_type).unwrap_or(DamageType::Poison)
    }

    /// Its `protocol::look` id.
    pub fn look_id(&self) -> u8 {
        look_of(self.look)
    }

    /// What is wrong with it, if anything (loading warns and skips it).
    pub fn problem(&self) -> Option<String> {
        if self.id.is_empty() {
            return Some("a hazard needs an id".into());
        }
        if !(self.every_secs.is_finite() && self.every_secs > 0.0) {
            return Some(format!("{}: every_secs must be > 0", self.id));
        }
        if self.condition.is_some() && self.condition().is_none() {
            return Some(format!("{}: unknown condition {:?}", self.id, self.condition));
        }
        if DamageType::from_id(self.damage_type).is_none() {
            return Some(format!("{}: unknown damage_type {}", self.id, self.damage_type));
        }
        let bad = match self.shape {
            HazardShape::Circle { r, .. } => !(r.is_finite() && r > 0.0),
            HazardShape::Rect { w, h, .. } => !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0),
        };
        bad.then(|| format!("{}: the shape needs a positive size", self.id))
    }
}

/// `protocol::look` id of a look name (unknown: plain danger).
pub fn look_of(name: &str) -> u8 {
    match name {
        "heal" => look::HEAL,
        "fire" => look::FIRE,
        "swamp" => look::SWAMP,
        "ward" => look::WARD,
        "cold" => look::COLD,
        "lightning" => look::LIGHTNING,
        "holy" => look::HOLY,
        "trap" => look::TRAP,
        "earth" => look::EARTH,
        "curse" => look::CURSE,
        "poison" => look::POISON,
        "dark" => look::DARK,
        "ripple" => look::RIPPLE,
        "sand" => look::SAND,
        "sludge" => look::SLUDGE,
        "petals" => look::PETALS,
        "glyph" => look::GLYPH,
        "gold" => look::GOLD,
        _ => look::DAMAGE,
    }
}

/// `<server_dir>/hazards.json` when the location has one (a JSON array of
/// [`Hazard`]); invalid rows are skipped with a warning.
pub fn load_hazards(server_dir: &Path) -> Result<Vec<Hazard>, MapError> {
    let path = server_dir.join("hazards.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(&path).map_err(|source| MapError::Io { path: path.display().to_string(), source })?;
    parse_hazards(&bytes)
}

pub fn parse_hazards(bytes: &[u8]) -> Result<Vec<Hazard>, MapError> {
    let raw: Vec<RawHazard> = serde_json::from_slice(bytes).map_err(|e| MapError::Walkability(format!("hazards.json: {e}")))?;
    Ok(raw
        .into_iter()
        .map(RawHazard::leak)
        .filter(|h| match h.problem() {
            Some(p) => {
                tracing::warn!("hazards.json: {p}; skipped");
                false
            }
            None => true,
        })
        .collect())
}

/// A timed beat of the map: first at `first_secs`, then every
/// `every_secs` (0 = once), announced `warn_secs` before, lasting
/// `duration_secs`. While it runs its hazards ([`Hazard::during`]) bite and
/// the light is scaled by `light_scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapEvent {
    pub id: &'static str,
    /// Shown on the timeline and in the warning ("Sandstorm").
    pub name: &'static str,
    /// The warning's text ("A sandstorm is rising!"); empty: `name` only.
    pub warning: &'static str,
    pub first_secs: f32,
    pub every_secs: f32,
    pub warn_secs: f32,
    pub duration_secs: f32,
    /// Light radius multiplier while it runs (1 = unchanged).
    pub light_scale: f32,
    /// Camera shake as it begins (0 = none, 255 = strongest; clients with
    /// reduced motion skip it), over a second.
    pub shake: u8,
}

impl MapEvent {
    /// When its `n`-th occurrence (0-based) begins, if it has one.
    pub fn start(&self, n: u32) -> Option<f32> {
        if n > 0 && self.every_secs <= 0.0 {
            return None;
        }
        Some(self.first_secs + self.every_secs.max(0.0) * n as f32)
    }

    /// Occurrence running at `secs`, as its start.
    pub fn running_at(&self, secs: f32) -> Option<f32> {
        let n = self.occurrence(secs)?;
        let s = self.start(n)?;
        (secs < s + self.duration_secs).then_some(s)
    }

    /// The latest occurrence begun by `secs`.
    fn occurrence(&self, secs: f32) -> Option<u32> {
        if secs < self.first_secs {
            return None;
        }
        Some(if self.every_secs > 0.0 { ((secs - self.first_secs) / self.every_secs) as u32 } else { 0 })
    }

    /// The next occurrence beginning after `secs` (at `secs` counts).
    pub fn next_start(&self, secs: f32) -> Option<f32> {
        match self.occurrence(secs) {
            None => self.start(0),
            Some(n) => {
                let s = self.start(n)?;
                if s >= secs { Some(s) } else { self.start(n + 1) }
            }
        }
    }
}

/// What felling an interactive object does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ObjectEffect {
    /// Nothing beyond its drops (an egg that only holds loot).
    None,
    /// The run's light radius is multiplied by `scale` for `secs` (a lit
    /// brazier). Boosts multiply with each other and with events.
    Light { scale: f32, secs: f32 },
    /// Ends a hazard of the map for the rest of the run (a broken valve).
    EndHazard(&'static str),
    /// A location's own effect: `hub/env.rs` `object_hook` matches the id.
    Hook(&'static str),
}

/// A timed hatch: an object left standing this long breaks open into
/// `count` monsters of `monster` (a roster kind) around its spot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hatch {
    pub secs: f32,
    pub monster: &'static str,
    pub count: u8,
}

/// Interactive objects of one kind: a plant monster (`archetype =
/// "plant"`, `stats.hits` = blows it takes) planted at each spot when the
/// stage is made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectDef {
    pub monster: &'static str,
    /// Cells; spots on unwalkable ground are skipped with a warning.
    pub spots: &'static [(f32, f32)],
    pub on_felled: ObjectEffect,
    /// Hatches unless felled in time (an egg); `None`: it just stands.
    pub hatch: Option<Hatch>,
}

/// A mission's environment. [`Environment::NONE`] is Poring Island's:
/// plain light, no hazards, events or objects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Environment {
    /// Light radius multiplier of the whole run (a dark cave: 0.7).
    pub light_scale: f32,
    pub hazards: &'static [Hazard],
    pub events: &'static [MapEvent],
    pub objects: &'static [ObjectDef],
}

impl Environment {
    pub const NONE: Environment = Environment { light_scale: 1.0, hazards: &[], events: &[], objects: &[] };

    pub fn event(&self, id: &str) -> Option<&MapEvent> {
        self.events.iter().find(|e| e.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORM: MapEvent = MapEvent {
        id: "storm",
        name: "Sandstorm",
        warning: "",
        first_secs: 100.0,
        every_secs: 120.0,
        warn_secs: 10.0,
        duration_secs: 30.0,
        light_scale: 0.6,
        shake: 0,
    };

    #[test]
    fn events_repeat_on_schedule() {
        assert_eq!(STORM.next_start(0.0), Some(100.0));
        assert_eq!(STORM.next_start(100.0), Some(100.0));
        assert_eq!(STORM.next_start(101.0), Some(220.0));
        assert_eq!(STORM.running_at(99.0), None);
        assert_eq!(STORM.running_at(129.0), Some(100.0));
        assert_eq!(STORM.running_at(131.0), None);
        assert_eq!(STORM.running_at(225.0), Some(220.0));
        let once = MapEvent { every_secs: 0.0, ..STORM };
        assert_eq!(once.next_start(101.0), None);
        assert_eq!(once.running_at(110.0), Some(100.0));
    }

    #[test]
    fn hazards_parse_from_the_bake_and_skip_bad_rows() {
        let json = br#"[
          {"id":"tar","shape":{"circle":{"x":10,"y":12,"r":3}},"look":"swamp","condition":"slowed","condition_secs":0.5,"pull":1.5},
          {"id":"vent","shape":{"rect":{"x":4,"y":4,"w":6,"h":2}},"damage":[1,6],"damage_type":"fire","every_secs":2,"dc":12,"during":"eruption"},
          {"id":"bad","shape":{"circle":{"x":1,"y":1,"r":0}}},
          {"id":"worse","shape":{"circle":{"x":1,"y":1,"r":2}},"condition":"sneezing"}
        ]"#;
        let h = parse_hazards(json).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].condition(), h[0].look_id(), h[0].pull, h[0].damage), (Some(Condition::Slowed), look::SWAMP, 1.5, [0, 0]));
        assert_eq!((h[1].damage_type(), h[1].dc, h[1].during), (DamageType::Fire, 12, Some("eruption")));
        assert!(h[1].shape.contains(6.9, 4.0, 0.0) && !h[1].shape.contains(7.5, 4.0, 0.0));
        assert!(h[0].shape.contains(12.9, 12.0, 0.0) && !h[0].shape.contains(13.5, 12.0, 0.0));
        assert!(Hazard::circle("x", 0.0, 0.0, 1.0).problem().is_none());
    }
}
