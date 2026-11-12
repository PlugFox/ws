//! Mission catalogue offered by mission-board NPCs.
//!
//! Static for now; Phase 4 turns a selection into a lobby and a stage
//! instance.

use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MissionInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Stage location the mission is played on.
    pub map: &'static str,
    pub max_players: u8,
}

pub const MISSIONS: &[MissionInfo] = &[MissionInfo {
    id: "poring_island_survival",
    name: "Poring Island Survival",
    description: "Hold out against growing waves on Poring Island until the boss shows up.",
    map: "prt_fild08",
    max_players: 4,
}];

pub fn find(id: &str) -> Option<&'static MissionInfo> {
    MISSIONS.iter().find(|m| m.id == id)
}
