//! Attack rolls, saving throws, damage types and the element wheel
//! (COMBAT.md §2.1).

use super::dice::{Dice, Rng};

/// Mechanical damage type; the RO element is the costume (see [`DamageType::ro_element`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DamageType {
    Physical,
    Fire,
    Cold,
    Lightning,
    Earth,
    Poison,
    Radiant,
    Necrotic,
    Force,
}

impl DamageType {
    pub const fn ro_element(self) -> &'static str {
        match self {
            DamageType::Physical => "neutral",
            DamageType::Fire => "fire",
            DamageType::Cold => "water",
            DamageType::Lightning => "wind",
            DamageType::Earth => "earth",
            DamageType::Poison => "poison",
            DamageType::Radiant => "holy",
            DamageType::Necrotic => "shadow",
            DamageType::Force => "ghost",
        }
    }
}

/// A monster's RO element, which decides its affinities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Element {
    Neutral,
    Water,
    Earth,
    Fire,
    Wind,
    Poison,
    Holy,
    Shadow,
    Ghost,
    Undead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Affinity {
    Normal,
    Resistant,
    Vulnerable,
    Immune,
}

impl Affinity {
    pub const fn apply(self, amount: i32) -> i32 {
        match self {
            Affinity::Normal => amount,
            Affinity::Resistant => amount / 2,
            Affinity::Vulnerable => amount * 2,
            Affinity::Immune => 0,
        }
    }
}

impl Element {
    pub fn from_id(id: &str) -> Self {
        match id {
            "water" => Element::Water,
            "earth" => Element::Earth,
            "fire" => Element::Fire,
            "wind" => Element::Wind,
            "poison" => Element::Poison,
            "holy" => Element::Holy,
            "shadow" | "dark" => Element::Shadow,
            "ghost" => Element::Ghost,
            "undead" => Element::Undead,
            _ => Element::Neutral,
        }
    }

    /// The element wheel: RO's logic in 5e terms.
    pub const fn affinity(self, kind: DamageType) -> Affinity {
        use Affinity::*;
        use DamageType as D;
        match (self, kind) {
            (Element::Water, D::Lightning) => Vulnerable,
            (Element::Water, D::Cold | D::Fire) => Resistant,
            (Element::Earth, D::Fire) => Vulnerable,
            (Element::Earth, D::Earth | D::Lightning) => Resistant,
            (Element::Fire, D::Cold) => Vulnerable,
            (Element::Fire, D::Fire | D::Earth) => Resistant,
            (Element::Wind, D::Earth) => Vulnerable,
            (Element::Wind, D::Lightning | D::Cold) => Resistant,
            (Element::Poison, D::Radiant) => Vulnerable,
            (Element::Poison, D::Poison) => Resistant,
            (Element::Holy, D::Necrotic) => Vulnerable,
            (Element::Holy, D::Radiant) => Resistant,
            (Element::Shadow, D::Radiant) => Vulnerable,
            (Element::Shadow, D::Necrotic | D::Poison) => Resistant,
            (Element::Ghost, D::Force | D::Radiant) => Vulnerable,
            (Element::Ghost, D::Physical) => Resistant,
            (Element::Undead, D::Radiant | D::Fire) => Vulnerable,
            (Element::Undead, D::Necrotic | D::Cold) => Resistant,
            (Element::Undead, D::Poison) => Immune,
            _ => Normal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackOutcome {
    Miss,
    Hit,
    /// Natural 20: damage dice are rolled twice.
    Critical,
}

/// `d20 + bonus` against `ac`. A natural 1 always misses; a natural roll of
/// `crit_from` or higher (20 normally, 18 with Sharp Shooting) is a critical.
pub fn attack_roll(rng: &mut Rng, bonus: i32, ac: i32, advantage: i8, crit_from: u32) -> AttackOutcome {
    let d = rng.d20(advantage);
    if d == 1 {
        AttackOutcome::Miss
    } else if d >= crit_from.min(20) {
        AttackOutcome::Critical
    } else if d as i32 + bonus >= ac {
        AttackOutcome::Hit
    } else {
        AttackOutcome::Miss
    }
}

/// `d20 + bonus` against `dc`; `true` = the save succeeds.
pub fn saving_throw(rng: &mut Rng, bonus: i32, dc: i32, advantage: i8) -> bool {
    rng.d20(advantage) as i32 + bonus >= dc
}

/// A hit's damage: dice of one type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Damage {
    pub dice: Dice,
    pub kind: DamageType,
}

impl Damage {
    pub const fn new(dice: Dice, kind: DamageType) -> Self {
        Self { dice, kind }
    }

    /// Rolls and applies, in order: critical dice, half on a successful
    /// save, the target's affinity, then flat reduction (Armor Mastery),
    /// which only applies to physical damage and never below zero.
    pub fn resolve(self, rng: &mut Rng, critical: bool, saved: bool, element: Element, reduction: i32) -> i32 {
        let mut amount = self.dice.roll(rng, critical);
        if saved {
            amount /= 2;
        }
        amount = element.affinity(self.kind).apply(amount);
        if self.kind == DamageType::Physical {
            amount -= reduction;
        }
        amount.max(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_wheel() {
        assert_eq!(Element::Water.affinity(DamageType::Lightning), Affinity::Vulnerable);
        assert_eq!(Element::Earth.affinity(DamageType::Fire), Affinity::Vulnerable);
        assert_eq!(Element::Undead.affinity(DamageType::Poison), Affinity::Immune);
        assert_eq!(Element::Ghost.affinity(DamageType::Physical), Affinity::Resistant);
        assert_eq!(Element::Neutral.affinity(DamageType::Fire), Affinity::Normal);
        assert_eq!(Element::from_id("dark"), Element::Shadow);
        assert_eq!((Affinity::Resistant.apply(7), Affinity::Vulnerable.apply(7)), (3, 14));
    }

    #[test]
    fn natural_one_and_twenty() {
        let mut rng = Rng::new(11);
        let (mut miss, mut crit) = (0, 0);
        for _ in 0..20_000 {
            match attack_roll(&mut rng, 100, 0, 0, 20) {
                AttackOutcome::Miss => miss += 1,
                AttackOutcome::Critical => crit += 1,
                AttackOutcome::Hit => {}
            }
        }
        // Each is 1 in 20 even with an absurd bonus.
        assert!((850..1150).contains(&miss) && (850..1150).contains(&crit), "{miss} {crit}");
        // +0 against AC 21 only crits.
        assert!((0..2000).all(|_| attack_roll(&mut rng, 0, 21, 0, 20) != AttackOutcome::Hit));
    }

    #[test]
    fn hit_chance_is_tabletop() {
        let mut rng = Rng::new(5);
        // +5 vs AC 15: hits on 10+ = 55%.
        let hits = (0..40_000).filter(|_| attack_roll(&mut rng, 5, 15, 0, 20) != AttackOutcome::Miss).count();
        assert!((21_400..22_600).contains(&hits), "{hits}");
        // DC 13 save with +2: succeeds on 11+ = 50%.
        let saves = (0..40_000).filter(|_| saving_throw(&mut rng, 2, 13, 0)).count();
        assert!((19_400..20_600).contains(&saves), "{saves}");
    }

    #[test]
    fn damage_pipeline() {
        let mut rng = Rng::new(9);
        let fixed = |kind| Damage::new(Dice::new(0, 6, 10), kind);
        assert_eq!(fixed(DamageType::Lightning).resolve(&mut rng, false, false, Element::Water, 0), 20);
        assert_eq!(fixed(DamageType::Cold).resolve(&mut rng, false, true, Element::Water, 0), 2);
        assert_eq!(fixed(DamageType::Physical).resolve(&mut rng, false, false, Element::Neutral, 3), 7);
        assert_eq!(fixed(DamageType::Fire).resolve(&mut rng, false, false, Element::Neutral, 3), 10);
        assert_eq!(fixed(DamageType::Poison).resolve(&mut rng, true, false, Element::Undead, 0), 0);
    }
}
