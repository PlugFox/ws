//! A character's lifetime record, the leaderboard's counters: monsters
//! slain, MVPs (bosses), falls, missions cleared and the best kill combo.
//!
//! Who gets credit (`hub/fight.rs`, `hub/mission.rs`):
//!
//! - **Kill**: the player who landed the killing blow. A kill nobody
//!   landed (burning, poison) goes to whoever picks up its drops: the
//!   nearest fighter on the map (`fight::looter`).
//! - **MVP**: a boss dies and every party member on the stage map gets
//!   one — the same players who share its experience, standing, downed
//!   or fallen. Being in the fight is what counts, not the last hit.
//! - **Fall** (`deaths`): knocked down to 0 HP (standing → downed), or
//!   killed outright by the overtime fog while standing.
//! - **Mission**: a victory, for every player on the stage map at the end.
//! - **Best combo**: the highest kill combo count reached (a maximum, not
//!   a sum).
//!
//! The instance keeps what changed in the player's `Fighter` and reports it
//! with the progress (`HubEvent::Record`, a few times a second at most), at
//! the end of a mission and when the player leaves the instance. The socket
//! task adds it to the session's character, and the flusher stores it with
//! the rest of the character (`db::leaderboard_repo`): never a write per
//! kill.

use serde::Serialize;

/// Counters of one character (or what changed since the last report).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Record {
    /// Monsters slain (killing blows).
    pub kills: u64,
    /// Bosses killed with the party.
    pub mvps: u64,
    /// Times knocked down.
    pub deaths: u64,
    /// Missions won.
    pub missions: u64,
    /// The highest kill combo count.
    pub best_combo: u32,
}

impl Record {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Adds `delta` (sums; the best combo is the larger).
    pub fn add(&mut self, delta: &Record) {
        self.kills += delta.kills;
        self.mvps += delta.mvps;
        self.deaths += delta.deaths;
        self.missions += delta.missions;
        self.best_combo = self.best_combo.max(delta.best_combo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_sums_counters_and_keeps_the_best_combo() {
        let mut r = Record { kills: 10, mvps: 1, deaths: 2, missions: 3, best_combo: 40 };
        r.add(&Record { kills: 5, mvps: 1, deaths: 0, missions: 1, best_combo: 12 });
        assert_eq!(r, Record { kills: 15, mvps: 2, deaths: 2, missions: 4, best_combo: 40 });
        r.add(&Record { best_combo: 55, ..Record::default() });
        assert_eq!(r.best_combo, 55);
        assert!(Record::default().is_empty());
        assert!(!r.is_empty());
    }
}
