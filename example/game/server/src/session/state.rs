use crate::world::combat::{Bag, Loadout};
use crate::world::protocol::Look;
use crate::world::record::Record;
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
    /// Belongings: bag stacks, equipment, zeny (`inventory`,
    /// `equipment`, `zeny`; the worn weapon also goes to `weapon`).
    pub bag: Bag,
    pub last_location_id: String,
    pub look: Look,
    /// Held weapon id; empty = the class's starting weapon.
    pub weapon: String,
    /// Learned skills and the hotbar.
    pub loadout: Loadout,
    /// Lifetime counters (the leaderboard; `world::record`).
    pub record: Record,

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
        bag: Bag,
        last_location_id: String,
        look: Look,
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
            weapon: bag.weapon().to_string(),
            bag,
            last_location_id,
            look,
            loadout: Loadout::default(),
            record: Record::default(),
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

    /// Level and experience into it (combat), marking state dirty when changed.
    pub fn set_progress(&mut self, level: u32, exp: u64) {
        if (self.level, self.base_exp) != (level, exp) {
            self.level = level;
            self.base_exp = exp;
            self.is_dirty = true;
            self.last_modified = Instant::now();
        }
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

    /// Learned skills or hotbar changed, marking state dirty.
    pub fn set_loadout(&mut self, loadout: Loadout) {
        if self.loadout != loadout {
            self.loadout = loadout;
            self.is_dirty = true;
            self.last_modified = Instant::now();
        }
    }

    /// Belongings changed (trade, equipment, potions, zeny), marking state
    /// dirty.
    pub fn set_bag(&mut self, bag: Bag) {
        if self.bag != bag {
            self.weapon = bag.weapon().to_string();
            self.bag = bag;
            self.is_dirty = true;
            self.last_modified = Instant::now();
        }
    }

    /// Counters gained in an instance (kills, MVPs, ...), marking state
    /// dirty.
    pub fn add_record(&mut self, delta: &Record) {
        if !delta.is_empty() {
            self.record.add(delta);
            self.is_dirty = true;
            self.last_modified = Instant::now();
        }
    }

    /// Updates location ID, marking state dirty.
    pub fn set_location(&mut self, location_id: String) {
        self.last_location_id = location_id;
        self.is_dirty = true;
        self.last_modified = Instant::now();
    }

    /// Restyled at a stylist, marking state dirty.
    pub fn set_look(&mut self, look: Look) {
        self.look = look;
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
    /// Server announcement (admin broadcast, restart countdown): a banner
    /// and a chat log line.
    Announce { text: String },
}
