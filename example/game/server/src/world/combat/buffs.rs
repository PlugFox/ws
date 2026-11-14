//! What buffs and passives change on a player (COMBAT.md §3): one flat set
//! of modifiers, summed over the passives and the running buffs.
//!
//! The skill table names the numbers (`ac`, `haste`, ...); the instance
//! sums them once per tick and the rules read the sum.

use super::dice::Dice;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mods {
    /// Armour class.
    pub ac: i32,
    /// Flat bonus to attack rolls.
    pub to_hit: i32,
    /// Extra die on attack rolls (Bless: 1d4); 0 = none.
    pub to_hit_die: u32,
    /// Dice added to every weapon hit (Impositio Manus).
    pub damage: Option<Dice>,
    /// Flat damage per weapon hit (Sword Mastery).
    pub damage_flat: i32,
    /// Basic attack interval shortened by this fraction.
    pub haste: f32,
    /// Walking speed raised by this fraction.
    pub speed: f32,
    /// Damage taken lowered by this fraction (Assumptio: 0.5).
    pub resist: f32,
    /// Physical damage taken lowered by this much per hit.
    pub reduce: i32,
    /// Percent of melee hits blocked outright (Auto Guard).
    pub block: u32,
    /// Light radius, cells.
    pub light: f32,
    /// Regeneration, fraction of the maximum per second.
    pub sp_regen: f32,
    pub hp_regen: f32,
    /// Critical range widened by this much (19–20 at 1).
    pub crit: u32,
    /// Advantage on attack rolls (True Sight).
    pub advantage: bool,
    /// Monsters ignore the player (Play Dead); no actions, movement ends it.
    pub hidden: bool,
    /// Not pushed, stunned or flinched (Endure).
    pub steady: bool,
    /// Damage comes off SP first, 1 SP for 2 HP (Energy Coat).
    pub mana_shield: bool,
    /// Damage from undead and demons lowered by this fraction.
    pub evil_resist: f32,
    /// Flat damage per hit against undead and demons.
    pub evil_damage: i32,
    /// SP back per kill by a skill (Soul Drain).
    pub soul_drain: i32,
    /// Percent chance a basic attack swings once more (Double Attack).
    pub double: u32,
    /// Percent chance a weapon hit leaves the monster Poisoned (Enchant
    /// Poison).
    pub venom: u32,
    /// Drop chances of the player's own loot rolls raised by this fraction
    /// (Steal).
    pub loot: f32,
    /// Hidden while moving too: walking does not end it (Cloaking).
    pub cloak: bool,
    /// Can strike out of hiding: the basic attack and skill presses work
    /// and end it, the first blow with advantage (Hiding, Cloaking).
    pub ambush: bool,
    /// The next damaging spell (fixed dice) deals as if this fraction of
    /// its dice rolled their maximum; casting it ends the buff (Mystical
    /// Amplification).
    pub amplify: f32,
}

impl Mods {
    /// Both sets together: numbers add, the largest extra die wins, flags or.
    pub fn add(&mut self, o: &Mods) {
        self.ac += o.ac;
        self.to_hit += o.to_hit;
        self.to_hit_die = self.to_hit_die.max(o.to_hit_die);
        self.damage = match (self.damage, o.damage) {
            (Some(a), Some(b)) if a.sides == b.sides => Some(Dice { count: a.count + b.count, ..a }),
            (Some(a), Some(b)) => Some(if b.count * b.sides > a.count * a.sides { b } else { a }),
            (a, b) => a.or(b),
        };
        self.damage_flat += o.damage_flat;
        self.haste += o.haste;
        self.speed += o.speed;
        self.resist += o.resist;
        self.reduce += o.reduce;
        self.block += o.block;
        self.light += o.light;
        self.sp_regen += o.sp_regen;
        self.hp_regen += o.hp_regen;
        self.crit += o.crit;
        self.advantage |= o.advantage;
        self.hidden |= o.hidden;
        self.steady |= o.steady;
        self.mana_shield |= o.mana_shield;
        self.evil_resist += o.evil_resist;
        self.evil_damage += o.evil_damage;
        self.soul_drain += o.soul_drain;
        self.double += o.double;
        self.venom += o.venom;
        self.loot += o.loot;
        self.cloak |= o.cloak;
        self.ambush |= o.ambush;
        self.amplify += o.amplify;
    }

    /// Multiplier of the basic attack interval (never below a third).
    pub fn interval(&self) -> f32 {
        (1.0 - self.haste).max(0.33)
    }

    /// Multiplier of the walking speed (never below a quarter).
    pub fn pace(&self) -> f32 {
        (1.0 + self.speed).max(0.25)
    }

    /// Damage after resistance and reduction (`physical`: armour applies).
    pub fn taken(&self, amount: i32, physical: bool) -> i32 {
        let mut a = amount;
        if physical {
            a -= self.reduce;
        }
        let keep = (1.0 - self.resist).clamp(0.0, 1.0);
        ((a.max(0) as f32) * keep).round() as i32
    }

    /// Hidden and still able to strike from it (an ambush comes next).
    pub fn lurking(&self) -> bool {
        self.hidden && self.ambush
    }

    /// The critical threshold of a d20 (20 = only natural 20s).
    pub fn crit_from(&self) -> u32 {
        20u32.saturating_sub(self.crit).max(15)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mods_sum_and_clamp() {
        let mut m = Mods { ac: 1, haste: 0.2, damage: Some(Dice::new(1, 4, 0)), ..Mods::default() };
        m.add(&Mods { ac: 2, haste: 0.6, damage: Some(Dice::new(2, 4, 0)), hidden: true, ..Mods::default() });
        assert_eq!((m.ac, m.damage, m.hidden), (3, Some(Dice::new(3, 4, 0)), true));
        assert!((m.interval() - 0.33).abs() < 1e-6, "haste cannot stop the clock");
        let r = Mods { resist: 0.5, reduce: 2, ..Mods::default() };
        assert_eq!((r.taken(10, true), r.taken(10, false), r.taken(1, true)), (4, 5, 0));
        assert_eq!(Mods { crit: 2, ..Mods::default() }.crit_from(), 18);
    }
}
