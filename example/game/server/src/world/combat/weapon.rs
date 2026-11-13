//! Weapons: the basic attack is whatever the character holds (COMBAT.md
//! §2.3). The weapon gives the die, the damage type, the reach and shape of
//! the swing and its speed; the class decides what it may hold (RO's
//! equip rules) and adds proficiency and its modifier to the roll.
//!
//! Item names are RO's; the numbers are 5e's simple and martial weapons.

use super::class::Class;
use super::dice::Dice;
use super::rules::DamageType;
use super::cantrip_k;

/// Weapon families: what a class may equip and how the basic attack moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WeaponKind {
    Dagger,
    Sword,
    TwoHandSword,
    Spear,
    Axe,
    TwoHandAxe,
    Mace,
    Bow,
    /// Staves and rods: a spell focus; the basic attack is a bolt.
    Staff,
    /// Priest books: a holy focus; the basic attack is a bolt.
    Book,
}

/// Physical damage subtype (5e), for monster traits later (skeletons and
/// bludgeoning, plants and slashing); all count as physical for the wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Physical {
    Slashing,
    Piercing,
    Bludgeoning,
}

/// How the basic attack reaches its targets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reach {
    /// Every monster within `range` cells inside a cone of `arc_cos`
    /// (cosine of the half angle) around the nearest one.
    Melee { range: f32, arc_cos: f32 },
    /// The nearest monster within `range` cells, plus everything within
    /// `splash` cells of it.
    Ranged { range: f32, splash: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weapon {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: WeaponKind,
    /// RO weapon level 1–4: a +0…+3 magic bonus to hit and damage (5e's
    /// +1/+2/+3 weapons); upgrades are how weapons scale.
    pub level: u8,
    pub die: Dice,
    pub physical: Option<Physical>,
    pub reach: Reach,
    /// Milliseconds between attack actions.
    pub interval_ms: u32,
}

impl WeaponKind {
    /// Basic attack shape and speed of the family.
    const fn profile(self) -> (Reach, u32, Option<Physical>) {
        use Physical::*;
        match self {
            WeaponKind::Dagger => (Reach::Melee { range: 3.0, arc_cos: 0.7 }, 700, Some(Piercing)),
            WeaponKind::Sword => (Reach::Melee { range: 4.0, arc_cos: 0.5 }, 900, Some(Slashing)),
            // Two-handers sweep wide and slow.
            WeaponKind::TwoHandSword => (Reach::Melee { range: 4.5, arc_cos: 0.0 }, 1200, Some(Slashing)),
            // Reach weapon: long and narrow.
            WeaponKind::Spear => (Reach::Melee { range: 6.0, arc_cos: 0.85 }, 1000, Some(Piercing)),
            WeaponKind::Axe => (Reach::Melee { range: 4.0, arc_cos: 0.5 }, 1000, Some(Slashing)),
            WeaponKind::TwoHandAxe => (Reach::Melee { range: 4.5, arc_cos: 0.0 }, 1300, Some(Slashing)),
            WeaponKind::Mace => (Reach::Melee { range: 4.0, arc_cos: 0.5 }, 1000, Some(Bludgeoning)),
            WeaponKind::Bow => (Reach::Ranged { range: 32.0, splash: 1.2 }, 1000, Some(Piercing)),
            WeaponKind::Staff => (Reach::Ranged { range: 20.0, splash: 1.2 }, 1200, None),
            WeaponKind::Book => (Reach::Ranged { range: 20.0, splash: 1.2 }, 1100, None),
        }
    }

    /// Foci cast their basic attack: the die is rolled `k` times and the
    /// damage type is the class's magic.
    pub const fn is_focus(self) -> bool {
        matches!(self, WeaponKind::Staff | WeaponKind::Book)
    }
}

impl Class {
    /// Weapon families the class may hold (RO equip rules, trimmed).
    pub const fn can_hold(self, kind: WeaponKind) -> bool {
        use WeaponKind::*;
        match self {
            Class::Swordsman => matches!(kind, Dagger | Sword | TwoHandSword | Spear | Axe | TwoHandAxe | Mace),
            Class::Archer => matches!(kind, Dagger | Bow),
            Class::Mage => matches!(kind, Dagger | Staff),
            Class::Acolyte => matches!(kind, Mace | Staff | Book),
        }
    }

    /// What a new character holds.
    pub const fn starting_weapon(self) -> &'static str {
        match self {
            Class::Swordsman => "sword",
            Class::Archer => "bow",
            Class::Mage => "rod",
            Class::Acolyte => "rod",
        }
    }

    /// The class's magic for focus bolts.
    pub const fn magic(self) -> DamageType {
        match self {
            Class::Acolyte => DamageType::Radiant,
            _ => DamageType::Force,
        }
    }
}

const fn w(id: &'static str, name: &'static str, kind: WeaponKind, level: u8, count: u32, sides: u32) -> Weapon {
    let (reach, interval_ms, physical) = kind.profile();
    Weapon { id, name, kind, level, die: Dice::new(count, sides, 0), physical, reach, interval_ms }
}

/// The weapon catalogue (ids are what characters store).
pub const WEAPONS: &[Weapon] = &[
    w("knife", "Knife", WeaponKind::Dagger, 1, 1, 4),
    w("cutter", "Cutter", WeaponKind::Dagger, 1, 1, 4),
    w("main_gauche", "Main Gauche", WeaponKind::Dagger, 2, 1, 4),
    w("stiletto", "Stiletto", WeaponKind::Dagger, 3, 1, 4),
    w("sword", "Sword", WeaponKind::Sword, 1, 1, 8),
    w("falchion", "Falchion", WeaponKind::Sword, 2, 1, 8),
    w("blade", "Blade", WeaponKind::Sword, 3, 1, 8),
    w("katana", "Katana", WeaponKind::TwoHandSword, 1, 2, 6),
    w("claymore", "Claymore", WeaponKind::TwoHandSword, 3, 2, 6),
    w("javelin", "Javelin", WeaponKind::Spear, 1, 1, 8),
    w("pike", "Pike", WeaponKind::Spear, 2, 1, 10),
    w("axe", "Axe", WeaponKind::Axe, 1, 1, 8),
    w("battle_axe", "Battle Axe", WeaponKind::TwoHandAxe, 1, 1, 12),
    w("club", "Club", WeaponKind::Mace, 1, 1, 6),
    w("mace", "Mace", WeaponKind::Mace, 2, 1, 8),
    w("bow", "Bow", WeaponKind::Bow, 1, 1, 8),
    w("composite_bow", "Composite Bow", WeaponKind::Bow, 2, 1, 8),
    w("great_bow", "Great Bow", WeaponKind::Bow, 3, 1, 10),
    w("rod", "Rod", WeaponKind::Staff, 1, 1, 6),
    w("wand", "Wand", WeaponKind::Staff, 2, 1, 6),
    w("staff", "Staff", WeaponKind::Staff, 3, 1, 8),
    w("bible", "Bible", WeaponKind::Book, 2, 1, 6),
];

impl Weapon {
    pub fn find(id: &str) -> Option<&'static Weapon> {
        WEAPONS.iter().find(|w| w.id == id)
    }

    /// The weapon a character of `class` actually swings: the stored id if it
    /// exists and the class may hold it, else the class's starting weapon.
    pub fn held(class: Class, id: &str) -> &'static Weapon {
        Self::find(id)
            .filter(|w| class.can_hold(w.kind))
            .unwrap_or_else(|| Self::find(class.starting_weapon()).expect("starting weapons are in the catalogue"))
    }

    /// Magic bonus of the weapon level (+0…+3).
    pub const fn enchantment(&self) -> i32 {
        self.level.saturating_sub(1) as i32
    }

    /// Damage of one hit for a character of `class` at `level`: the weapon
    /// die plus class modifier and enchantment for weapons; foci roll the
    /// die `k` times in the class's magic, plus enchantment.
    pub fn damage(&self, class: Class, level: u32) -> (Dice, DamageType) {
        if self.kind.is_focus() {
            (Dice { bonus: self.enchantment(), ..self.die.times(cantrip_k(level)) }, class.magic())
        } else {
            (Dice { bonus: super::class_mod(level) + self.enchantment(), ..self.die }, DamageType::Physical)
        }
    }

    /// Attack bonus: proficiency + class modifier + enchantment.
    pub const fn to_hit(&self, level: u32) -> i32 {
        super::proficiency(level) + super::class_mod(level) + self.enchantment()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_sane() {
        for (i, w) in WEAPONS.iter().enumerate() {
            assert!(WEAPONS[..i].iter().all(|o| o.id != w.id), "duplicate id {}", w.id);
            assert!((1..=4).contains(&w.level));
            assert!(w.interval_ms >= 450);
            assert_eq!(w.physical.is_none(), w.kind.is_focus());
        }
        for c in Class::ALL {
            let start = Weapon::find(c.starting_weapon()).unwrap();
            assert!(c.can_hold(start.kind), "{c:?} can hold its starting weapon");
        }
    }

    #[test]
    fn held_falls_back_to_the_starting_weapon() {
        assert_eq!(Weapon::held(Class::Swordsman, "katana").id, "katana");
        assert_eq!(Weapon::held(Class::Mage, "katana").id, "rod", "mages cannot swing a katana");
        assert_eq!(Weapon::held(Class::Archer, "").id, "bow");
    }

    #[test]
    fn damage_follows_the_weapon() {
        let sword = Weapon::find("sword").unwrap();
        assert_eq!(sword.damage(Class::Swordsman, 1), (Dice::new(1, 8, 3), DamageType::Physical));
        let katana = Weapon::find("katana").unwrap();
        assert_eq!(katana.damage(Class::Swordsman, 30).0, Dice::new(2, 6, 4));
        let staff = Weapon::find("staff").unwrap();
        // Focus: d8 rolled k = 5 times at 60, +2 for a level-3 staff.
        assert_eq!(staff.damage(Class::Mage, 60), (Dice::new(5, 8, 2), DamageType::Force));
        assert_eq!(Weapon::find("rod").unwrap().damage(Class::Acolyte, 1).1, DamageType::Radiant);
        assert_eq!(staff.to_hit(1), 2 + 3 + 2);
    }
}
