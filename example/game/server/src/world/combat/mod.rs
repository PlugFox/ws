//! Combat rules: tabletop dice over a survivors horde.
//!
//! The design (and the tables these functions must reproduce) lives in
//! `example/docs/COMBAT.md`. Everything here is pure: no world state, no
//! clocks; randomness comes in through [`Dice`], seeded per instance so a
//! stage replays the same rolls in tests.

pub mod class;
pub mod dice;
pub mod exp;
pub mod monster;
pub mod rules;
pub mod weapon;

pub use class::{Class, Save};
pub use dice::{Dice, Rng};
pub use exp::{exp_to_next, kill_value, level_diff_percent, share_kill};
pub use monster::{Archetype, MonsterBlock, Size};
pub use rules::{AttackOutcome, Damage, DamageType, Element, Affinity};
pub use weapon::{Reach, Weapon, WeaponKind};

/// Highest character level.
pub const MAX_LEVEL: u32 = 99;

/// Skill-tree tier of a character level: 1 below 30, 2 below 60, else 3.
pub const fn tier(level: u32) -> u32 {
    if level < 30 {
        1
    } else if level < 60 {
        2
    } else {
        3
    }
}

/// Proficiency bonus `2 + floor((L − 1) / 12)`: bounded accuracy over 99 levels.
pub const fn proficiency(level: u32) -> i32 {
    2 + (level.saturating_sub(1) / 12) as i32
}

/// Class modifier `3 + (tier − 1)`, standing in for an ability modifier.
pub const fn class_mod(level: u32) -> i32 {
    3 + tier(level) as i32 - 1
}

/// Cantrip multiplier `k = 1 + floor(L / 15)`: how many times each die a
/// skill lists is rolled.
pub const fn cantrip_k(level: u32) -> u32 {
    1 + level / 15
}

/// Spell save DC `8 + P + A`.
pub const fn save_dc(level: u32) -> i32 {
    8 + proficiency(level) + class_mod(level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_numbers() {
        assert_eq!((proficiency(1), proficiency(12), proficiency(13), proficiency(97), proficiency(99)), (2, 2, 3, 10, 10));
        assert_eq!((tier(29), tier(30), tier(59), tier(60)), (1, 2, 2, 3));
        assert_eq!((class_mod(1), class_mod(30), class_mod(99)), (3, 4, 5));
        assert_eq!((cantrip_k(1), cantrip_k(30), cantrip_k(60), cantrip_k(90)), (1, 3, 5, 7));
        assert_eq!(save_dc(1), 13);
    }
}
