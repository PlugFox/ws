use serde::{Deserialize, Serialize};
use std::time::Instant;

/// In-memory character simulation state with dirty-flag tracking.
#[derive(Clone, Debug)]
pub struct CharacterState {
    pub id: u32,
    pub user_id: u32,
    pub name: String,
    pub archetype: String,
    pub level: u32,
    pub base_exp: u64,
    pub stats: serde_json::Value,
    pub unlocked_skills: serde_json::Value,
    pub inventory: serde_json::Value,
    pub last_location_id: String,

    pub is_dirty: bool,
    pub last_modified: Instant,
}

impl CharacterState {
    pub fn new(
        id: u32,
        user_id: u32,
        name: String,
        archetype: String,
        level: u32,
        base_exp: u64,
        stats: serde_json::Value,
        unlocked_skills: serde_json::Value,
        inventory: serde_json::Value,
        last_location_id: String,
    ) -> Self {
        Self {
            id,
            user_id,
            name,
            archetype,
            level,
            base_exp,
            stats,
            unlocked_skills,
            inventory,
            last_location_id,
            is_dirty: false,
            last_modified: Instant::now(),
        }
    }

    /// Mutates experience and recalculates level, marking state dirty.
    pub fn add_exp(&mut self, exp_gain: u64) {
        self.base_exp += exp_gain;
        self.is_dirty = true;
        self.last_modified = Instant::now();
    }

    /// Mutates stats, marking state dirty.
    pub fn update_stats(&mut self, stats: serde_json::Value) {
        self.stats = stats;
        self.is_dirty = true;
        self.last_modified = Instant::now();
    }

    /// Unlocks or upgrades a skill, marking state dirty.
    pub fn unlock_skill(&mut self, skill_id: &str) {
        if let Some(arr) = self.unlocked_skills.as_array_mut() {
            let val = serde_json::Value::String(skill_id.to_string());
            if !arr.contains(&val) {
                arr.push(val);
                self.is_dirty = true;
                self.last_modified = Instant::now();
            }
        }
    }

    /// Updates location ID, marking state dirty.
    pub fn set_location(&mut self, location_id: String) {
        self.last_location_id = location_id;
        self.is_dirty = true;
        self.last_modified = Instant::now();
    }

    /// Marks the state as clean after successful database persistence.
    pub fn mark_clean(&mut self) {
        self.is_dirty = false;
    }
}

/// Signals dispatched across session control channels.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum SessionSignal {
    /// Another connection was established for this account; displaced from current session.
    Displaced { reason: String },
    /// Administrative or session expiration forced disconnect.
    ForceDisconnect { reason: String },
    /// Generic notification message for client.
    Notification { message: String },
}
