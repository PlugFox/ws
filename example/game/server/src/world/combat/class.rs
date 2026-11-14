//! Playable classes: hit dice, spell points, armour, attack cadence.
//!
//! A character is *class + level + learned skills*; nothing is distributed
//! by hand. Numbers are the tables in COMBAT.md §1.3 and §2.2–2.3.

use super::{proficiency, tier};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Swordsman,
    Archer,
    Mage,
    Acolyte,
}

/// The three saving throws (5e's six folded into what monsters target).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Save {
    Body,
    Reflex,
    Mind,
}

// Class lines shown on the creation screen but not selectable yet are the
// locked entries of `profile::PROFILES`.

impl Class {
    pub const ALL: [Class; 4] = [Class::Swordsman, Class::Archer, Class::Mage, Class::Acolyte];

    /// Database / wire id.
    pub const fn id(self) -> &'static str {
        match self {
            Class::Swordsman => "swordsman",
            Class::Archer => "archer",
            Class::Mage => "mage",
            Class::Acolyte => "acolyte",
        }
    }

    /// Parses an archetype id; the pre-combat ids of old characters map to
    /// the nearest new line (`rogue` had no counterpart and becomes an archer).
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id.to_ascii_lowercase().as_str() {
            "swordsman" | "warrior" => Class::Swordsman,
            "archer" | "ranger" | "rogue" => Class::Archer,
            "mage" => Class::Mage,
            "acolyte" => Class::Acolyte,
            _ => return None,
        })
    }

    /// Title at a level (the body sprite follows it).
    pub const fn title(self, level: u32) -> &'static str {
        match (self, tier(level)) {
            (Class::Swordsman, 1) => "Swordsman",
            (Class::Swordsman, 2) => "Crusader",
            (Class::Swordsman, _) => "Paladin",
            (Class::Archer, 1) => "Archer",
            (Class::Archer, 2) => "Hunter",
            (Class::Archer, _) => "Sniper",
            (Class::Mage, 1) => "Mage",
            (Class::Mage, 2) => "Wizard",
            (Class::Mage, _) => "High Wizard",
            (Class::Acolyte, 1) => "Acolyte",
            (Class::Acolyte, 2) => "Priest",
            (Class::Acolyte, _) => "High Priest",
        }
    }

    /// Hit die sides per tier.
    const fn hit_die(self, tier: u32) -> u32 {
        let dice = match self {
            Class::Mage => [6, 6, 8],
            Class::Acolyte => [8, 8, 10],
            Class::Archer => [8, 10, 10],
            Class::Swordsman => [10, 12, 12],
        };
        dice[(tier - 1) as usize]
    }

    /// Hit die sides of the tier `level` is in.
    pub const fn hit_die_at(self, level: u32) -> u32 {
        self.hit_die(tier(level))
    }

    /// Flat HP per level (the CON modifier nobody rolls for).
    const fn con(self, tier: u32) -> i32 {
        match (self, tier) {
            (Class::Mage, _) => 0,
            (Class::Swordsman, 3) => 3,
            (Class::Swordsman, _) => 2,
            _ => 1,
        }
    }

    const fn sp_per_level(self, tier: u32) -> i32 {
        let sp = match self {
            Class::Mage => [4, 5, 6],
            Class::Acolyte => [3, 4, 5],
            Class::Archer => [2, 2, 3],
            Class::Swordsman => [1, 1, 2],
        };
        sp[(tier - 1) as usize]
    }

    /// Maximum HP: die maximum + CON at level 1, then the die's average
    /// rounded up + CON per level, with the die of the tier each level was
    /// gained in. No rolling, so equal characters are equal.
    pub fn max_hp(self, level: u32) -> i32 {
        let level = level.clamp(1, super::MAX_LEVEL);
        let mut hp = self.hit_die(1) as i32 + self.con(1);
        for l in 2..=level {
            let t = tier(l);
            hp += (self.hit_die(t) / 2 + 1) as i32 + self.con(t);
        }
        hp
    }

    /// Maximum SP: 10 + per-level gains of each tier.
    pub fn max_sp(self, level: u32) -> i32 {
        let level = level.clamp(1, super::MAX_LEVEL);
        10 + (1..=level).map(|l| self.sp_per_level(tier(l))).sum::<i32>()
    }

    /// Armour class in the starting kit, without buffs. Armour class is
    /// worn armour + class defence + 1 per tier above the first
    /// ([`super::items::Gear`]); the kit keeps the numbers the classes had
    /// before items: 16 / 14 / 13 / 11 at tier 1.
    pub fn base_ac(self, level: u32) -> i32 {
        super::items::Bag::starting(self, "").gear(self).ac_at(level)
    }

    pub const fn proficient_save(self) -> Save {
        match self {
            Class::Swordsman => Save::Body,
            Class::Archer => Save::Reflex,
            Class::Mage | Class::Acolyte => Save::Mind,
        }
    }

    /// Save bonus: full proficiency on the class save, half otherwise.
    pub const fn save_bonus(self, level: u32, save: Save) -> i32 {
        let p = proficiency(level);
        if self.proficient_save() as u8 == save as u8 { p } else { p / 2 }
    }

    /// Swings or arrows per attack action: martial classes get Extra Attack
    /// at tier 2 and 3; casters stay at one and scale through `k`.
    pub const fn attacks_per_action(self, level: u32) -> u32 {
        match self {
            Class::Swordsman | Class::Archer => tier(level),
            Class::Mage | Class::Acolyte => 1,
        }
    }

    /// Skill points available at a level: one per level from 2, plus one
    /// bonus at each tier unlock (100 at 99).
    pub const fn skill_points(level: u32) -> u32 {
        let level = if level > super::MAX_LEVEL { super::MAX_LEVEL } else { level };
        level.saturating_sub(1) + (level >= 30) as u32 + (level >= 60) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The HP / SP table of COMBAT.md §1.3, levels 1 / 10 / 30 / 60 / 99.
    #[test]
    fn pools_match_the_design_table() {
        let levels = [1, 10, 30, 60, 99];
        let table = [
            (Class::Mage, [6, 42, 122, 243, 438], [14, 50, 131, 282, 516]),
            (Class::Acolyte, [9, 63, 183, 364, 637], [13, 40, 101, 222, 417]),
            (Class::Archer, [9, 63, 184, 394, 667], [12, 30, 70, 131, 248]),
            (Class::Swordsman, [12, 84, 245, 516, 906], [11, 20, 40, 71, 149]),
        ];
        for (class, hp, sp) in table {
            for (i, &l) in levels.iter().enumerate() {
                assert_eq!(class.max_hp(l), hp[i], "{class:?} hp at {l}");
                assert_eq!(class.max_sp(l), sp[i], "{class:?} sp at {l}");
            }
        }
    }

    #[test]
    fn ids_titles_and_points() {
        for c in Class::ALL {
            assert_eq!(Class::from_id(c.id()), Some(c));
        }
        assert_eq!(Class::from_id("Warrior"), Some(Class::Swordsman));
        assert_eq!(Class::from_id("thief"), None);
        assert_eq!(Class::Mage.title(29), "Mage");
        assert_eq!(Class::Mage.title(30), "Wizard");
        assert_eq!(Class::Acolyte.title(60), "High Priest");
        assert_eq!((Class::skill_points(1), Class::skill_points(29), Class::skill_points(30), Class::skill_points(99)), (0, 28, 30, 100));
        assert_eq!(Class::Swordsman.attacks_per_action(60), 3);
        assert_eq!(Class::Mage.save_bonus(1, Save::Mind), 2);
        assert_eq!(Class::Mage.save_bonus(1, Save::Body), 1);
        assert_eq!(Class::Swordsman.base_ac(99), 18);
    }
}
