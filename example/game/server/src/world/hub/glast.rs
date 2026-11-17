//! Glast Heim Churchyard (mission `glast_heim`, docs/locations/glast_heim.md §3):
//! the location's own mechanics on top of the mission platform (`env.rs`
//! runs the night, the map events, the graves' broods and plants the
//! objects):
//!
//! * **Restless graves** (`missions.rs` `GLAST_OBJECTS`): spawners that let
//!   Ghouls out every 25 s until broken. Breaking one ([`GRAVE_HOOK`])
//!   consecrates it: a gold flash, and the spot is remembered.
//! * **The Witching Hour** (4:00, 8:00): as it begins every consecrated
//!   grave rises again where it stood (its first brood [`RISE_SECS`] later),
//!   the plots are cursed (`during` hazards of the mission table) and the
//!   light falls. The first rising lets Ghouls out again, the later ones
//!   Skeleton Prisoners.
//! * **Holy fonts** ([`FONT_HOOK`], three blows): a hallowed pool of
//!   [`POOL_R`] cells for [`POOL_SECS`] (drawn as calm gold ground) heals the
//!   living [`HEAL_PCT`] % a second and sets the dead and the demons in it
//!   alight; then a dry font stands there for [`DRY_SECS`].
//! * **Raise** (the Zombie Master): every [`RAISE_EVERY`] s with a player
//!   near, it plants two fresh graves beside itself that open into a Ghoul
//!   each [`RAISE_SECS`] later unless broken.
//!
//! The state is a few small vectors; a quiet tick allocates nothing.

use super::area::Area;
use super::cast::ticks_of;
use super::{HubInstance, MapIdx, Outgoing};
use crate::world::combat::Condition;
use crate::world::environment::{Hatch, ObjectEffect};
use crate::world::protocol::{hit, look, zone_flags, BossZone, HitEvent, ServerMsg, Telegraph};
use tracing::info;

/// The mission this module runs on.
pub(super) const MISSION: &str = "glast_heim";
/// Object hook of a broken (consecrated) grave (`missions.rs`).
pub(super) const GRAVE_HOOK: &str = "grave";
/// Object hook of a struck holy font.
pub(super) const FONT_HOOK: &str = "font";
/// The grave and font monsters (recipe `[monsters.<id>]`).
pub(super) const GRAVE: &str = "grave";
pub(super) const FRESH_GRAVE: &str = "fresh_grave";
pub(super) const FONT: &str = "holy_font";
pub(super) const DRY_FONT: &str = "holy_font_dry";
/// The map event that raises the graves.
pub(super) const WITCHING_HOUR: &str = "witching_hour";
/// A risen grave's first brood comes this long after it rises, then every
/// [`GRAVE_EVERY`].
pub(super) const RISE_SECS: f32 = 6.0;
pub(super) const GRAVE_EVERY: f32 = 25.0;
/// Effects keys (effects.toml `[skills.<id>]`).
pub(super) const CONSECRATE_ABILITY: &str = "glast_consecrate";
pub(super) const RISE_ABILITY: &str = "glast_rise";
pub(super) const FONT_ABILITY: &str = "glast_font";
pub(super) const RAISE_ABILITY: &str = "glast_raise";
/// The hallowed pool (cells; 4 m), its time, the heal a second (% of max
/// HP) and the wait before the font can be filled again.
pub(super) const POOL_R: f32 = 8.0;
pub(super) const POOL_SECS: f32 = 20.0;
pub(super) const HEAL_PCT: i32 = 2;
pub(super) const DRY_SECS: f32 = 90.0;
/// The dead in a pool burn this long (a fire of this k), renewed each second.
const SCORCH_SECS: f32 = 2.0;
const SCORCH_K: u8 = 3;
/// The Zombie Master's Raise: how often, how near a player must be, how far
/// out the graves land and how long until they open.
const ZOMBIE_MASTER: &str = "zombie_master";
pub(super) const RAISE_EVERY: f32 = 16.0;
/// A Zombie Master new on the field raises its first graves this soon.
const RAISE_FIRST: f32 = 4.0;
const RAISE_REACH: f32 = 14.0;
const RAISE_RING: f32 = 1.8;
pub(super) const RAISE_SECS: f32 = 6.0;

/// A hallowed pool springing from a font.
#[derive(Clone, Copy, Debug)]
struct Pool {
    x: f32,
    y: f32,
    until: f32,
    next_pulse: f32,
    zone: u32,
}

/// A font run dry: the dry twin standing in its place.
#[derive(Clone, Copy, Debug)]
struct Dry {
    x: f32,
    y: f32,
    until: f32,
    slot: u16,
    generation: u8,
}

/// The churchyard's state on a Glast Heim stage.
#[derive(Debug, Default)]
pub(super) struct Churchyard {
    /// Where graves were consecrated since the last Witching Hour.
    felled: Vec<(f32, f32)>,
    /// Start of the last Witching Hour that raised them.
    risen_at: Option<f32>,
    /// How many Witching Hours have raised graves.
    risings: u32,
    pools: Vec<Pool>,
    dry: Vec<Dry>,
    /// Zombie Masters' next Raise: (slot, generation, run second).
    raise: Vec<(u16, u8, f32)>,
    next_check: f32,
    near: Vec<u16>,
}

impl Churchyard {
    /// Consecrated graves waiting for the Witching Hour.
    #[cfg(test)]
    pub fn felled(&self) -> usize {
        self.felled.len()
    }

    /// Hallowed pools now.
    #[cfg(test)]
    pub fn pools(&self) -> usize {
        self.pools.len()
    }
}

impl HubInstance {
    fn glast_zone(&mut self) -> u32 {
        let id = self.next_zone;
        self.next_zone = self.next_zone.wrapping_add(1).max(1);
        id
    }

    /// A grave was broken at `(x, y)`: it is consecrated (a gold flash)
    /// until the next Witching Hour raises it.
    pub(super) fn glast_consecrate(&mut self, map: MapIdx, x: f32, y: f32, out: &mut Outgoing) {
        self.env.glast.felled.push((x, y));
        let id = self.glast_zone();
        let area = Telegraph { id, shape: 0, look: look::GOLD, x, y, dir: 0, a: 2.4, b: 0.0, span_ms: 1200, follow: 0 };
        let msg = ServerMsg::BossZones { caster: 0, ability: CONSECRATE_ABILITY.into(), zones: vec![BossZone { area, delay_ms: 0, flags: zone_flags::SAFE }] };
        self.broadcast(map, &msg, out);
        info!("{}: a grave is consecrated at ({x:.0}, {y:.0}) ({} closed)", self.name, self.env.glast.felled.len());
    }

    /// A holy font was struck at `(x, y)`: a hallowed pool springs for
    /// [`POOL_SECS`], and a dry font stands there until it fills again.
    pub(super) fn glast_font(&mut self, map: MapIdx, x: f32, y: f32, out: &mut Outgoing) {
        let Some(now) = self.horde.as_ref().map(|h| h.director_elapsed()) else { return };
        let zone = self.glast_zone();
        let area = Telegraph { id: zone, shape: 0, look: look::GOLD, x, y, dir: 0, a: POOL_R, b: 0.0, span_ms: (POOL_SECS * 1000.0) as u32, follow: 0 };
        let msg = ServerMsg::BossZones { caster: 0, ability: FONT_ABILITY.into(), zones: vec![BossZone { area, delay_ms: 0, flags: zone_flags::SAFE }] };
        self.broadcast(map, &msg, out);
        self.env.glast.pools.push(Pool { x, y, until: now + POOL_SECS, next_pulse: now, zone });
        match self.env_plant(DRY_FONT, x, y, ObjectEffect::None) {
            Some((slot, generation)) => self.env.glast.dry.push(Dry { x, y, until: now + DRY_SECS, slot, generation }),
            None => {
                let _ = self.env_plant(FONT, x, y, ObjectEffect::Hook(FONT_HOOK));
            }
        }
        info!("{}: a holy font springs at ({x:.0}, {y:.0})", self.name);
    }

    /// Once per tick on a Glast Heim stage.
    pub(super) fn glast_step(&mut self, map: MapIdx, now: f32, out: &mut Outgoing, hits: &mut Vec<HitEvent>) {
        if let Some(start) = self.env_event_start(WITCHING_HOUR)
            && self.env.glast.risen_at != Some(start)
        {
            self.env.glast.risen_at = Some(start);
            self.glast_rise(map, out);
        }
        if now >= self.env.glast.next_check {
            self.env.glast.next_check = now + 1.0;
            self.glast_fonts(now);
            self.glast_raise(map, now, out);
        }
        self.glast_pools(map, now, out, hits);
    }

    /// The Witching Hour: every consecrated grave rises again.
    fn glast_rise(&mut self, map: MapIdx, out: &mut Outgoing) {
        self.env.glast.risings += 1;
        let brood = if self.env.glast.risings <= 1 { "ghoul" } else { "skel_prisoner" };
        let spots = std::mem::take(&mut self.env.glast.felled);
        let mut zones = Vec::with_capacity(spots.len());
        let mut risen = 0;
        for &(x, y) in &spots {
            let hatch = Hatch::every(RISE_SECS, GRAVE_EVERY, brood, 2);
            if self.env_plant_hatching(GRAVE, x, y, ObjectEffect::Hook(GRAVE_HOOK), Some(hatch)).is_some() {
                risen += 1;
                let area = Telegraph { id: 0, shape: 0, look: look::CURSE, x, y, dir: 0, a: 2.2, b: 0.0, span_ms: 0, follow: 0 };
                zones.push(BossZone { area, delay_ms: 600, flags: 0 });
            }
        }
        if !zones.is_empty() {
            self.broadcast(map, &ServerMsg::BossZones { caster: 0, ability: RISE_ABILITY.into(), zones }, out);
        }
        info!("{}: the Witching Hour raises {risen} graves ({brood})", self.name);
    }

    /// Dry fonts that have filled again.
    fn glast_fonts(&mut self, now: f32) {
        let mut k = 0;
        while k < self.env.glast.dry.len() {
            let d = self.env.glast.dry[k];
            if now < d.until {
                k += 1;
                continue;
            }
            self.env.glast.dry.swap_remove(k);
            if let Some(horde) = self.horde.as_mut() {
                let store = horde.store();
                if store.is_alive(d.slot) && store.generation[d.slot as usize] == d.generation {
                    horde.despawn(d.slot);
                }
            }
            let _ = self.env_plant(FONT, d.x, d.y, ObjectEffect::Hook(FONT_HOOK));
        }
    }

    /// Hallowed pools: heal the living, burn the dead, dry up.
    fn glast_pools(&mut self, map: MapIdx, now: f32, out: &mut Outgoing, hits: &mut Vec<HitEvent>) {
        if self.env.glast.pools.is_empty() {
            return;
        }
        let hz = self.config.tick_hz.max(1) as f32;
        let mut k = 0;
        while k < self.env.glast.pools.len() {
            let p = self.env.glast.pools[k];
            if now >= p.until {
                self.env.glast.pools.swap_remove(k);
                self.broadcast(map, &ServerMsg::ZoneEnd { id: p.zone }, out);
                continue;
            }
            k += 1;
            if now < p.next_pulse {
                continue;
            }
            self.env.glast.pools[k - 1].next_pulse = now + 1.0;
            let r2 = POOL_R * POOL_R;
            for pl in self.players.iter_mut().filter(|pl| pl.map == map) {
                let (px, py, id) = (pl.x, pl.y, pl.profile.character_id);
                let Some(f) = pl.fighter.as_mut() else { continue };
                if !f.is_up() || (px - p.x).powi(2) + (py - p.y).powi(2) > r2 || f.hp >= f.max_hp {
                    continue;
                }
                let heal = (f.max_hp * HEAL_PCT / 100).max(1).min(f.max_hp - f.hp);
                f.hp += heal;
                hits.push(HitEvent { target: id, amount: heal as u32, flags: hit::HEAL });
            }
            let Some(horde) = self.horde.as_mut() else { continue };
            let near = &mut self.env.glast.near;
            near.clear();
            Area::Circle { x: p.x, y: p.y, r: POOL_R }.monsters(horde, near);
            let ticks = ticks_of(SCORCH_SECS, hz).min(u16::MAX as u32) as u16;
            for &m in near.iter() {
                let kind = horde.store().kind[m as usize];
                if horde.is_plant(m) || !horde.catalog().get(kind).evil {
                    continue;
                }
                let store = horde.store_mut();
                store.burn_k[m as usize] = store.burn_k[m as usize].max(SCORCH_K);
                store.afflict(m, Condition::Burning, ticks);
            }
        }
    }

    /// Zombie Masters with a player near plant fresh graves beside them.
    fn glast_raise(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_ref() else { return };
        // Once the fog closed the waves its graves would never open: no Raise.
        if horde.spawning_closed() {
            return;
        }
        let Some(master) = horde.catalog().kind_of(ZOMBIE_MASTER) else { return };
        let masters: Vec<(u16, u8, f32, f32)> = horde.monsters().filter(|m| m.2 == master).map(|m| (m.0, m.1, m.3, m.4)).collect();
        // Forget the dead.
        self.env.glast.raise.retain(|r| masters.iter().any(|m| m.0 == r.0 && m.1 == r.1));
        let ups: Vec<(f32, f32)> = self.players.iter().filter(|p| p.map == map && p.fighter.as_ref().is_some_and(|f| f.hunted())).map(|p| (p.x, p.y)).collect();
        for (slot, generation, x, y) in masters {
            let due = match self.env.glast.raise.iter().find(|r| r.0 == slot && r.1 == generation) {
                Some(r) => r.2,
                None => {
                    // A new one waits a moment before its first Raise.
                    self.env.glast.raise.push((slot, generation, now + RAISE_FIRST));
                    continue;
                }
            };
            let Some(&(px, py)) = ups.iter().find(|p| (p.0 - x).powi(2) + (p.1 - y).powi(2) <= RAISE_REACH * RAISE_REACH) else { continue };
            if now < due {
                continue;
            }
            if let Some(r) = self.env.glast.raise.iter_mut().find(|r| r.0 == slot && r.1 == generation) {
                r.2 = now + RAISE_EVERY;
            }
            // Two graves across its line to the player.
            let (dx, dy) = (px - x, py - y);
            let len = (dx * dx + dy * dy).sqrt().max(0.01);
            let (sx, sy) = (-dy / len * RAISE_RING, dx / len * RAISE_RING);
            let mut zones = Vec::new();
            for (gx, gy) in [(x + sx, y + sy), (x - sx, y - sy)] {
                if self.env_plant_hatching(FRESH_GRAVE, gx, gy, ObjectEffect::None, Some(Hatch::once(RAISE_SECS, "ghoul", 1))).is_some() {
                    let area = Telegraph { id: 0, shape: 0, look: look::EARTH, x: gx, y: gy, dir: 0, a: 1.2, b: 0.0, span_ms: 0, follow: 0 };
                    zones.push(BossZone { area, delay_ms: (RAISE_SECS * 1000.0) as u32, flags: 0 });
                }
            }
            if !zones.is_empty() {
                self.broadcast(map, &ServerMsg::BossZones { caster: 0, ability: RAISE_ABILITY.into(), zones }, out);
            }
        }
    }
}

#[cfg(test)]
#[path = "glast_tests.rs"]
mod tests;
