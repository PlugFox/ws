//! Payon Caves' own stage mechanics (example/docs/locations/payon.md §4), on
//! top of the mission platform (`env.rs` runs the darkness, the map events
//! and plants the cold braziers):
//!
//! * **Braziers**: the 23 braziers of the map are cold-brazier objects (a
//!   plant monster, four blows: `missions.rs` `env.objects`). Kindling one
//!   (felling it, [`ObjectEffect::Hook`] `"brazier"`) lights it for
//!   [`LIT_SECS`]: a warm pool of [`POOL_R`] cells shows on the ground
//!   (a sanctuary-looking zone and the flame, `BOSS_ZONES` caster 0,
//!   ability [`BRAZIER_ABILITY`]), the run's light grows by [`LIGHT`] (for
//!   at most [`MAX_BOOSTS`] braziers at once), and the dead and the demons
//!   that walk into the pool catch fire. When it burns out a cold brazier
//!   is planted again on the spot, to be kindled anew.
//!
//!   Familiars flitting over a lit brazier snuff it a little (each one
//!   [`SNUFF_SECS`] off its time); a Horong (a burning skull) drifting by a
//!   cold one sets it alight.
//! * **Shrine bells**: the map's two bells are bell objects (three blows,
//!   `"bell"` hook). Ringing one sends a gold shockwave out [`BELL_R`]
//!   cells that stuns every undead and demon in it (not elites or bosses)
//!   for [`BELL_STUN_SECS`]; then a quiet bell hangs there for
//!   [`BELL_COOLDOWN`] before it can be rung again.
//!
//! The horde is not drawn to the light (the owner's call): the pools are
//! where the party chooses to fight.
//!
//! The state is a few small vectors; a quiet tick allocates nothing.

use super::area::Area;
use super::cast::ticks_of;
use super::{HubInstance, MapIdx, Outgoing};
use crate::world::combat::{Archetype, Condition};
use crate::world::environment::ObjectEffect;
use crate::world::protocol::{look, zone_flags, BossZone, ServerMsg, Telegraph};
use tracing::info;

/// The mission this module runs on.
pub(super) const MISSION: &str = "payon_caves";
/// Object hook id of a kindled brazier (`missions.rs`).
pub(super) const BRAZIER_HOOK: &str = "brazier";
/// The cold brazier monster (recipe `[monsters.brazier]`).
pub(super) const BRAZIER_MONSTER: &str = "brazier";
/// Effects key of a lit brazier's ground (effects.toml `[skills.payon_brazier]`).
pub(super) const BRAZIER_ABILITY: &str = "payon_brazier";
/// A kindled brazier burns this long (seconds).
pub(super) const LIT_SECS: f32 = 60.0;
/// Its pool of light (cells; 3.5 m).
pub(super) const POOL_R: f32 = 7.0;
/// Light radius multiplier per lit brazier ...
pub(super) const LIGHT: f32 = 1.12;
/// ... for at most this many at once (×1.57 over the cave's darkness).
pub(super) const MAX_BOOSTS: usize = 4;
/// The dead and demons in a pool catch fire this long, checked this often.
const SCORCH_SECS: f32 = 2.0;
const SCORCH_EVERY: f32 = 1.0;
/// Their burning bites as a fire of this k (`SCORCH_K`d4 a second).
const SCORCH_K: u8 = 3;
/// A Familiar within this many cells of a lit brazier takes this much off
/// its time, at most once a second each.
pub(super) const SNUFF_R: f32 = 2.0;
pub(super) const SNUFF_SECS: f32 = 5.0;
/// A Horong within this many cells of a cold brazier lights it.
pub(super) const IGNITE_R: f32 = 1.5;
/// The monsters that snuff and light braziers.
const FAMILIAR: &str = "familiar";
const HORONG: &str = "horong";
/// Object hook id of a rung shrine bell (`missions.rs`).
pub(super) const BELL_HOOK: &str = "bell";
/// The ready bell and the quiet one hanging while it recovers.
pub(super) const BELL_MONSTER: &str = "shrine_bell";
pub(super) const QUIET_BELL: &str = "shrine_bell_quiet";
/// Effects key of the shockwave (effects.toml `[skills.payon_bell]`).
pub(super) const BELL_ABILITY: &str = "payon_bell";
/// Its reach (cells; 8 m), the stun and the wait before it rings again.
pub(super) const BELL_R: f32 = 16.0;
pub(super) const BELL_STUN_SECS: f32 = 2.0;
pub(super) const BELL_COOLDOWN: f32 = 90.0;
/// Width of each ring of its shockwave (cells).
const BELL_RING_W: f32 = 1.4;

#[derive(Clone, Copy, Debug)]
struct Lit {
    x: f32,
    y: f32,
    /// Run second it burns out.
    until: f32,
    next_scorch: f32,
    /// It counts toward the light (one of the first [`MAX_BOOSTS`]): the
    /// boost's end as pushed (taken back if it is snuffed early).
    boosted: Option<f32>,
    /// Its ground zones on clients (the pool, the flame).
    zones: [u32; 2],
}

/// A rung bell recovering: the quiet bell hanging in its place.
#[derive(Clone, Copy, Debug)]
struct Quiet {
    x: f32,
    y: f32,
    until: f32,
    slot: u16,
    generation: u8,
}

/// The braziers burning and the bells recovering on a Payon stage.
#[derive(Debug, Default)]
pub(super) struct Braziers {
    lit: Vec<Lit>,
    quiet: Vec<Quiet>,
    /// Next run second Familiars and Horongs are looked for.
    next_check: f32,
    /// Scratch for monster queries.
    near: Vec<u16>,
}

impl Braziers {
    /// Where braziers burn now (cells).
    pub fn light_spots(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.lit.iter().map(|l| (l.x, l.y))
    }

    /// How many burn now.
    #[cfg(test)]
    pub fn lit(&self) -> usize {
        self.lit.len()
    }
}

impl HubInstance {
    /// A cold brazier was kindled at `(x, y)`: light it.
    pub(super) fn payon_kindle(&mut self, map: MapIdx, x: f32, y: f32, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_ref() else { return };
        let now = horde.director_elapsed();
        let boosted = (self.env.payon.lit.iter().filter(|l| l.boosted.is_some()).count() < MAX_BOOSTS).then_some(now + LIT_SECS);
        if let Some(until) = boosted {
            self.env.boost_light(LIGHT, until);
        }
        let span = (LIT_SECS * 1000.0) as u32;
        let mut zones = [0u32; 2];
        for z in &mut zones {
            *z = self.next_zone;
            self.next_zone = self.next_zone.wrapping_add(1).max(1);
        }
        let pool = Telegraph { id: zones[0], shape: 0, look: look::GOLD, x, y, dir: 0, a: POOL_R, b: 0.0, span_ms: span, follow: 0 };
        let flame = Telegraph { id: zones[1], shape: 0, look: look::FIRE, x, y, dir: 0, a: 0.6, b: 0.0, span_ms: span, follow: 0 };
        let msg = ServerMsg::BossZones {
            caster: 0,
            ability: BRAZIER_ABILITY.into(),
            zones: vec![BossZone { area: pool, delay_ms: 0, flags: zone_flags::SAFE }, BossZone { area: flame, delay_ms: 0, flags: 0 }],
        };
        self.broadcast(map, &msg, out);
        self.env.payon.lit.push(Lit { x, y, until: now + LIT_SECS, next_scorch: now, boosted, zones });
        info!("{}: a brazier is lit at ({x:.0}, {y:.0}) ({} burning)", self.name, self.env.payon.lit.len());
    }

    /// A shrine bell was rung at `(x, y)`: a gold shockwave stuns the dead
    /// around it, and a quiet bell hangs there while it recovers.
    pub(super) fn payon_ring(&mut self, map: MapIdx, x: f32, y: f32, out: &mut Outgoing) {
        let hz = self.config.tick_hz.max(1) as f32;
        let Some(horde) = self.horde.as_mut() else { return };
        let now = horde.director_elapsed();
        let near = &mut self.env.payon.near;
        near.clear();
        Area::Circle { x, y, r: BELL_R }.monsters(horde, near);
        let ticks = ticks_of(BELL_STUN_SECS, hz).min(u16::MAX as u32) as u16;
        let mut stunned = 0;
        for &m in near.iter() {
            let def = horde.catalog().get(horde.store().kind[m as usize]);
            if horde.is_plant(m) || !def.evil || def.boss || def.archetype == Archetype::Elite {
                continue;
            }
            horde.store_mut().afflict(m, Condition::Stunned, ticks);
            stunned += 1;
        }
        // Three thin gold rings roll out to its reach (harmless: drawn as
        // calm ground).
        let zones = (0..3)
            .map(|i| {
                let outer = (i + 1) as f32 * BELL_R / 3.0;
                let area = Telegraph { id: 0, shape: 3, look: look::GOLD, x, y, dir: 0, a: outer, b: outer - BELL_RING_W, span_ms: 0, follow: 0 };
                BossZone { area, delay_ms: 150 + i * 220, flags: zone_flags::SAFE }
            })
            .collect();
        self.broadcast(map, &ServerMsg::BossZones { caster: 0, ability: BELL_ABILITY.into(), zones }, out);
        self.env_shake(map, 70, 500, out);
        match self.env_plant(QUIET_BELL, x, y, ObjectEffect::None) {
            Some((slot, generation)) => self.env.payon.quiet.push(Quiet { x, y, until: now + BELL_COOLDOWN, slot, generation }),
            // No room for the quiet one: the bell is ready again at once.
            None => {
                let _ = self.env_plant(BELL_MONSTER, x, y, ObjectEffect::Hook(BELL_HOOK));
            }
        }
        info!("{}: a shrine bell rings at ({x:.0}, {y:.0}): {stunned} stunned", self.name);
    }

    /// Once per tick on a Payon stage: lit braziers scorch the dead in
    /// their pools, burn out and are planted cold again; Familiars snuff
    /// them and Horongs light cold ones; rung bells recover.
    pub(super) fn payon_step(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        if now >= self.env.payon.next_check {
            self.env.payon.next_check = now + 1.0;
            self.payon_bells(now);
            self.payon_snuff_and_ignite(now);
        }
        if self.env.payon.lit.is_empty() {
            return;
        }
        let hz = self.config.tick_hz.max(1) as f32;
        let mut k = 0;
        while k < self.env.payon.lit.len() {
            let l = self.env.payon.lit[k];
            if now >= l.until {
                self.env.payon.lit.swap_remove(k);
                if let Some(until) = l.boosted {
                    // Snuffed early: its share of the light goes with it.
                    self.env.end_boost(LIGHT, until);
                }
                for id in l.zones {
                    self.broadcast(map, &ServerMsg::ZoneEnd { id }, out);
                }
                if self.env_plant(BRAZIER_MONSTER, l.x, l.y, ObjectEffect::Hook(BRAZIER_HOOK)).is_none() {
                    info!("{}: no room to plant a cold brazier at ({:.0}, {:.0})", self.name, l.x, l.y);
                }
                continue;
            }
            if now >= l.next_scorch {
                self.env.payon.lit[k].next_scorch = now + SCORCH_EVERY;
                let Some(horde) = self.horde.as_mut() else { return };
                let near = &mut self.env.payon.near;
                near.clear();
                Area::Circle { x: l.x, y: l.y, r: POOL_R }.monsters(horde, near);
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
            k += 1;
        }
    }

    /// Recovered bells hang ready again (the quiet one goes).
    fn payon_bells(&mut self, now: f32) {
        let mut k = 0;
        while k < self.env.payon.quiet.len() {
            let q = self.env.payon.quiet[k];
            if now < q.until {
                k += 1;
                continue;
            }
            self.env.payon.quiet.swap_remove(k);
            if let Some(horde) = self.horde.as_mut() {
                let store = horde.store();
                if store.is_alive(q.slot) && store.generation[q.slot as usize] == q.generation {
                    horde.despawn(q.slot);
                }
            }
            let _ = self.env_plant(BELL_MONSTER, q.x, q.y, ObjectEffect::Hook(BELL_HOOK));
        }
    }

    /// Familiars over a lit brazier take time off it; a Horong by a cold
    /// one lights it (felled, so its hook fires on the next object scan).
    fn payon_snuff_and_ignite(&mut self, now: f32) {
        let Some(horde) = self.horde.as_mut() else { return };
        let catalog = horde.catalog();
        let (familiar, horong, brazier) = (catalog.kind_of(FAMILIAR), catalog.kind_of(HORONG), catalog.kind_of(BRAZIER_MONSTER));
        let near = &mut self.env.payon.near;
        if let Some(familiar) = familiar {
            for l in &mut self.env.payon.lit {
                near.clear();
                Area::Circle { x: l.x, y: l.y, r: SNUFF_R }.monsters(horde, near);
                let bats = near.iter().filter(|&&m| horde.store().kind[m as usize] == familiar).count();
                // Never out from under the players in one go: a second's grace.
                l.until = (l.until - bats as f32 * SNUFF_SECS).max(now + 1.0).min(l.until);
            }
        }
        let (Some(horong), Some(brazier)) = (horong, brazier) else { return };
        let skulls: Vec<(f32, f32)> = horde.monsters().filter(|m| m.2 == horong).map(|m| (m.3, m.4)).collect();
        if skulls.is_empty() {
            return;
        }
        let cold: Vec<u16> = horde
            .monsters()
            .filter(|m| m.2 == brazier && skulls.iter().any(|s| (s.0 - m.3).powi(2) + (s.1 - m.4).powi(2) <= IGNITE_R * IGNITE_R))
            .map(|m| m.0)
            .collect();
        for slot in cold {
            horde.despawn(slot);
        }
    }
}

#[cfg(test)]
#[path = "payon_tests.rs"]
mod tests;
