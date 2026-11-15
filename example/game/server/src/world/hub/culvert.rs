//! Prontera Culvert's ground (example/docs/locations/culvert.md §3): the
//! sewage of its banks and of the Flush (baked hazards, look `sludge`)
//! meets the party's spells.
//!
//! * **Fire fizzles**: burning ground (a Fire Wall, a Fire Pillar) laid on
//!   sewage goes out at its first bite, with a puff of steam (`cast.rs`
//!   ends it; here the steam).
//! * **Cold freezes it over**: a cold spell landing on sewage calms the
//!   hazards in a disc for 10 s (`env.rs` calm discs and `GROUND_MEETS`:
//!   whoever stands there is spared), shown as a safe pale ice patch.
//!
//! Everything else of the culvert is data: the mission's light, events and
//! egg clusters (`missions.rs`), the hazards (`hazards.json`), the boss
//! scripts (`boss/scripts/culvert.rs`).

use super::area::Area;
use super::{HubInstance, MapIdx};
use crate::world::combat::DamageType;
use crate::world::protocol::{look, ServerMsg};

/// Hazard looks that are sewage.
pub(super) const SEWAGE: &[u8] = &[look::SLUDGE];

impl HubInstance {
    /// Spells that met sewage this step: the ice and the steam (the pairing
    /// lives in `env.rs` [`super::env::GROUND_MEETS`], shared with Thor's
    /// obsidian).
    #[cfg_attr(not(test), expect(dead_code, reason = "the culvert tests drive it; the step calls `ground_meets`"))]
    pub(super) fn sewage_meets(&mut self, _map: MapIdx, met: &[(DamageType, Area)], msgs: &mut Vec<ServerMsg>) {
        self.ground_meets(met, msgs);
    }
}

#[cfg(test)]
#[path = "culvert_tests.rs"]
mod tests;
