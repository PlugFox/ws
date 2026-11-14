//! Monster stat blocks derived from level + archetype (COMBAT.md §4.5), so
//! a new monster needs identity (element, race, size), not hand balancing.

use super::dice::Dice;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Archetype {
    Swarm,
    Chaser,
    Tank,
    Elite,
    Boss,
    /// Planted on the field at stage creation (`horde/plants.rs`), never
    /// bought by the director: rooted, harmless, every hit takes exactly
    /// one HP (COMBAT.md §4.10).
    Plant,
}

impl Archetype {
    /// Director archetype ids of the recipe; unknown ids fall back to swarm.
    pub fn from_id(id: &str) -> Self {
        match id {
            "chaser" => Archetype::Chaser,
            "tank" => Archetype::Tank,
            "elite" => Archetype::Elite,
            "boss" => Archetype::Boss,
            "plant" => Archetype::Plant,
            _ => Archetype::Swarm,
        }
    }

    /// The recipe id (the inverse of [`Archetype::from_id`]).
    pub const fn id(self) -> &'static str {
        match self {
            Archetype::Swarm => "swarm",
            Archetype::Chaser => "chaser",
            Archetype::Tank => "tank",
            Archetype::Elite => "elite",
            Archetype::Boss => "boss",
            Archetype::Plant => "plant",
        }
    }
}

/// RO size; Large and Huge ignore knockback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Small,
    Medium,
    Large,
    Huge,
}

impl Size {
    pub fn from_id(id: &str) -> Self {
        match id {
            "medium" => Size::Medium,
            "large" => Size::Large,
            "huge" => Size::Huge,
            _ => Size::Small,
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Medium => "medium",
            Size::Large => "large",
            Size::Huge => "huge",
        }
    }

    pub const fn ignores_knockback(self) -> bool {
        matches!(self, Size::Large | Size::Huge)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonsterBlock {
    pub level: u32,
    pub hp: i32,
    pub ac: i32,
    /// Added to its d20 attack roll.
    pub attack: i32,
    pub damage: Dice,
    pub save: i32,
    pub attack_interval_ms: u32,
    /// Failed saves it may turn into successes per fight.
    pub legendary_resistance: u8,
}

impl MonsterBlock {
    /// `players` scales boss HP by `1 + 0.75·(players − 1)`.
    pub fn new(level: u32, archetype: Archetype, players: u32) -> Self {
        let ml = level.max(1);
        let (hp_pct, ac_mod, die, interval) = match archetype {
            Archetype::Swarm => (100, -1, Dice::new(1, 4, 0), 1600),
            Archetype::Chaser => (70, 0, Dice::new(1, 6, 0), 1000),
            Archetype::Tank => (400, 2, Dice::new(1, 10, 0), 2000),
            Archetype::Elite => (600, 2, Dice::new(2, 6, 0), 1400),
            Archetype::Boss => (6000, 4, Dice::new(2, 10, 0), 1200),
            // HP is its rolled hit count (`Horde::plant`), it never strikes,
            // and AC 1 leaves only a natural 1 to miss it.
            Archetype::Plant => (0, -(9 + (ml / 8) as i32) + 1, Dice::new(0, 1, 0), u32::MAX),
        };
        // (4 + 1.7·ml) × multiplier, in integers: (40 + 17·ml) / 10 × pct / 100.
        let mut hp = ((40 + 17 * ml as i64) * hp_pct + 500) / 1000;
        let boss = archetype == Archetype::Boss;
        if boss {
            let n = players.max(1) as i64;
            hp = (hp * (4 + 3 * (n - 1)) + 2) / 4;
        }
        Self {
            level: ml,
            hp: hp.max(1) as i32,
            // 9 + ⌊ml/8⌋: one under the 5e baseline, so fewer whiffs (§2.2).
            ac: 9 + (ml / 8) as i32 + ac_mod,
            attack: 2 + (ml / 6) as i32,
            damage: Dice { bonus: (ml / 5) as i32, ..die },
            save: 1 + (ml / 8) as i32 + if boss { 3 } else { 0 },
            attack_interval_ms: interval,
            legendary_resistance: if boss { 3 } else { 0 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Poring Island rows of COMBAT.md §4.5.
    #[test]
    fn blocks_match_the_design_table() {
        let b = |l, a| MonsterBlock::new(l, a, 1);
        assert_eq!((b(1, Archetype::Swarm).hp, b(12, Archetype::Swarm).hp), (6, 24));
        assert_eq!((b(1, Archetype::Swarm).ac, b(12, Archetype::Swarm).ac), (8, 9));
        assert_eq!((b(3, Archetype::Chaser).hp, b(14, Archetype::Chaser).hp), (6, 19));
        assert_eq!((b(8, Archetype::Tank).hp, b(18, Archetype::Tank).hp), (70, 138));
        assert_eq!((b(8, Archetype::Tank).ac, b(18, Archetype::Tank).ac), (12, 13));
        let baph = b(25, Archetype::Boss);
        assert_eq!((baph.hp, baph.ac, baph.attack, baph.damage), (2790, 16, 6, Dice::new(2, 10, 5)));
        assert_eq!(baph.legendary_resistance, 3);
        assert_eq!(MonsterBlock::new(25, Archetype::Boss, 4).hp, 2790 * 13 / 4 + 1);
    }
}
