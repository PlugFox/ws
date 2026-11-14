//! Prontera Culvert's ground (example/docs/locations/culvert.md §3): the
//! sewage of its banks and of the Flush (baked hazards, look `sludge`)
//! meets the party's spells.
//!
//! * **Fire fizzles**: burning ground (a Fire Wall, a Fire Pillar) laid on
//!   sewage goes out at its first bite, with a puff of steam (`cast.rs`
//!   ends it; here the steam).
//! * **Cold freezes it over**: a cold spell landing on sewage calms the
//!   hazards in a disc for [`ICE_SECS`] (`env.rs` calm discs: whoever stands
//!   there is spared), shown as a safe pale ice patch.
//!
//! Everything else of the culvert is data: the mission's light, events and
//! egg clusters (`missions.rs`), the hazards (`hazards.json`), the boss
//! scripts (`boss/scripts/culvert.rs`).

use super::area::Area;
use super::{HubInstance, MapIdx};
use crate::world::combat::DamageType;
use crate::world::protocol::{look, zone_flags, BossZone, ServerMsg};

/// Hazard looks that are sewage.
pub(super) const SEWAGE: &[u8] = &[look::SLUDGE];
/// How long frozen sewage holds (s), and its smallest radius (cells).
const ICE_SECS: f32 = 10.0;
const ICE_MIN_R: f32 = 2.5;
const ICE_MAX_R: f32 = 6.0;

impl HubInstance {
    /// Spells that met sewage this step (`cast.rs`): the ice and the steam
    /// for clients, the calm for the hazards.
    pub(super) fn sewage_meets(&mut self, _map: MapIdx, met: &[(DamageType, Area)], msgs: &mut Vec<ServerMsg>) {
        for &(kind, area) in met {
            let (x, y) = area.centre();
            let (x0, y0, x1, y1) = area.bounds();
            let r = ((x1 - x0).max(y1 - y0) / 2.0).clamp(ICE_MIN_R, ICE_MAX_R);
            let (ability, look, r, span) = match kind {
                DamageType::Cold => {
                    self.env_calm(x, y, r, ICE_SECS);
                    ("culvert_ice", look::COLD, r, (ICE_SECS * 1000.0) as u32)
                }
                _ => ("culvert_steam", look::SWAMP, 1.5, 0),
            };
            let id = self.next_zone;
            self.next_zone = self.next_zone.wrapping_add(1).max(1);
            let area = Area::Circle { x, y, r }.telegraph(if span > 0 { id } else { 0 }, look, span, 0);
            msgs.push(ServerMsg::BossZones { caster: 0, ability: ability.into(), zones: vec![BossZone { area, delay_ms: 0, flags: zone_flags::SAFE }] });
        }
    }
}

#[cfg(test)]
#[path = "culvert_tests.rs"]
mod tests;
