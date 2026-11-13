//! Mission catalogue offered by mission-board NPCs.
//!
//! Static for now. A selection forms a lobby (`lobby.rs`); starting it
//! launches a stage instance of `map` (`stage.rs`) whose monsters come in
//! waves bought under the mission's [`WaveRules`].

use super::horde::director::WaveRules;
#[derive(Clone, Debug, PartialEq)]
pub struct MissionInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Stage location the mission is played on.
    pub map: &'static str,
    pub max_players: u8,
    /// Surviving this long wins even if the boss lives (seconds).
    pub time_limit_secs: f32,
    /// Difficulty: what the wave game master gets to spend and where packs
    /// land. Server-side only.
    pub waves: WaveRules,
}

pub const MISSIONS: &[MissionInfo] = &[MissionInfo {
    id: "poring_island_survival",
    name: "Poring Island Survival",
    description: "Hold out against growing waves on Poring Island until the boss shows up.",
    map: "prt_fild08",
    max_players: 4,
    time_limit_secs: 900.0,
    waves: WaveRules::NORMAL,
}];

pub fn find(id: &str) -> Option<&'static MissionInfo> {
    MISSIONS.iter().find(|m| m.id == id)
}
