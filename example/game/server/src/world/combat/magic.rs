//! Magic items (COMBAT.md §2.6): drop-only equipment with a rarity and an
//! enchantment that holds while it is worn. Their numbers follow 5e's
//! uncommon and a few rare items (character levels 1–5 there): a +1, a
//! die of extra damage, a step of speed, a point of AC or of every save,
//! never a game changer.
//!
//! The base rows live with the plain ones ([`super::items::ITEMS`],
//! [`super::weapon::WEAPONS`]: AC, dice, class rules, price); this table
//! adds what makes them magic: the rarity, the [`Mods`] they grant while
//! worn (summed into the fighter's passives) and bonuses the plain rows
//! cannot say (every save, HP on a weapon). Each enchantment also writes
//! its own tooltip lines, so what an item says is what it does.

use super::buffs::Mods;
use super::dice::Dice;

/// How rare an item is: the name colour players read (white, green, blue).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rarity {
    #[default]
    Common,
    Uncommon,
    Rare,
}

impl Rarity {
    pub const fn code(self) -> u8 {
        self as u8
    }

    pub const fn label(self) -> &'static str {
        match self {
            Rarity::Common => "Common",
            Rarity::Uncommon => "Uncommon",
            Rarity::Rare => "Rare",
        }
    }
}

/// What makes an item magic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Magic {
    pub id: &'static str,
    pub rarity: Rarity,
    /// Granted while worn.
    pub mods: Mods,
    /// Bonus to every saving throw.
    pub saves: i8,
    /// Extra max HP / SP (weapons have no bonus row of their own).
    pub hp: i16,
    pub sp: i16,
}

const fn m(id: &'static str, rarity: Rarity, mods: Mods) -> Magic {
    Magic { id, rarity, mods, saves: 0, hp: 0, sp: 0 }
}
const N: Mods = Mods::NONE;
use Rarity::{Rare, Uncommon};

/// Every magic item's enchantment, by catalogue id.
pub const MAGIC: &[Magic] = &[
    // Weapons (the base row's level is the +1).
    m("viper_fang", Uncommon, Mods { venom: 15, ..N }),
    m("ember_blade", Uncommon, Mods { damage: Some(Dice::new(1, 4, 0)), ..N }),
    m("wyvern_spear", Uncommon, Mods { crit: 1, ..N }),
    Magic { hp: 15, ..m("berserker_axe", Rare, Mods { haste: 0.1, ..N }) },
    m("wind_bow", Uncommon, Mods { haste: 0.15, ..N }),
    m("sunshot_bow", Rare, Mods { crit: 1, evil_damage: 2, light: 2.0, ..N }),
    m("dawn_mace", Uncommon, Mods { evil_damage: 3, light: 2.0, ..N }),
    m("war_mage_wand", Uncommon, Mods { spell_dc: 1, ..N }),
    Magic { sp: 10, ..m("ember_staff", Rare, Mods { spell_power: 2, ..N }) },
    m("tome_of_mercy", Uncommon, Mods { heal_power: 3, ..N }),
    m("shadowfang", Rare, Mods { crit: 1, double: 10, ..N }),
    // Armour and shields.
    m("mithril_shirt", Uncommon, N),
    m("elven_mail", Rare, Mods { speed: 0.05, ..N }),
    m("apprentice_robe", Uncommon, Mods { sp_regen: 0.002, ..N }),
    m("dwarven_plate", Rare, Mods { reduce: 1, ..N }),
    m("sentinel_shield", Uncommon, Mods { light: 2.0, ..N }),
    m("spellward_buckler", Uncommon, Mods { resist: 0.05, ..N }),
    // Headgear, garments, shoes.
    m("focus_circlet", Uncommon, Mods { sp_regen: 0.003, ..N }),
    m("night_goggles", Uncommon, Mods { light: 3.0, ..N }),
    m("bandit_cap", Uncommon, Mods { loot: 0.05, ..N }),
    Magic { saves: 1, ..m("protection_cloak", Uncommon, N) },
    m("elven_cloak", Uncommon, Mods { speed: 0.05, ..N }),
    m("striding_boots", Uncommon, Mods { speed: 0.1, ..N }),
    m("pilgrim_boots", Uncommon, Mods { hp_regen: 0.002, ..N }),
    // Accessories.
    Magic { saves: 1, ..m("protection_ring", Rare, Mods { ac: 1, ..N }) },
    m("wound_periapt", Uncommon, Mods { potion: 0.5, ..N }),
    m("power_pearl", Uncommon, Mods { soul_drain: 2, ..N }),
    m("archery_bracers", Uncommon, Mods { damage_flat: 2, ..N }),
    Magic { saves: 1, ..m("luckstone", Uncommon, Mods { loot: 0.03, ..N }) },
    m("cutpurse_gloves", Uncommon, Mods { double: 10, ..N }),
    Magic { hp: 20, ..m("bear_amulet", Uncommon, N) },
    m("sun_pendant", Uncommon, Mods { heal_power: 2, evil_resist: 0.1, ..N }),
];

pub fn magic(id: &str) -> Option<&'static Magic> {
    MAGIC.iter().find(|m| m.id == id)
}

fn pct(f: f32) -> i32 {
    (f * 100.0).round() as i32
}

impl Magic {
    /// Tooltip lines of the enchantment (`markup()`), in a fixed order.
    pub fn lines(&self) -> Vec<String> {
        let o = &self.mods;
        let mut v = Vec::new();
        let mut ok = |s: String| v.push(format!("[ok]{s}[/]"));
        if self.hp != 0 {
            ok(format!("{:+} max HP", self.hp));
        }
        if self.sp != 0 {
            ok(format!("{:+} max SP", self.sp));
        }
        if self.saves != 0 {
            ok(format!("{:+} to every save", self.saves));
        }
        if o.ac != 0 {
            ok(format!("{:+} AC", o.ac));
        }
        if o.to_hit != 0 {
            ok(format!("{:+} to hit", o.to_hit));
        }
        if let Some(d) = o.damage {
            ok(format!("+{}d{} damage on every weapon hit", d.count, d.sides));
        }
        if o.damage_flat != 0 {
            ok(format!("{:+} damage on every weapon hit", o.damage_flat));
        }
        if o.crit != 0 {
            ok(format!("Critical hits on {}–20", 20 - o.crit as i32));
        }
        if o.double != 0 {
            ok(format!("{}% chance to strike twice", o.double));
        }
        if o.venom != 0 {
            ok(format!("{}% chance a hit **poisons**", o.venom));
        }
        if o.evil_damage != 0 {
            ok(format!("{:+} damage to undead and demons", o.evil_damage));
        }
        if o.evil_resist != 0.0 {
            ok(format!("−{}% damage from undead and demons", pct(o.evil_resist)));
        }
        if o.haste != 0.0 {
            ok(format!("+{}% attack speed", pct(o.haste)));
        }
        if o.speed != 0.0 {
            ok(format!("+{}% walking speed", pct(o.speed)));
        }
        if o.resist != 0.0 {
            ok(format!("−{}% damage taken", pct(o.resist)));
        }
        if o.reduce != 0 {
            ok(format!("−{} physical damage per hit", o.reduce));
        }
        if o.light != 0.0 {
            ok(format!("+{} light radius", o.light));
        }
        if o.hp_regen != 0.0 {
            ok(format!("Regenerates {:.1}% HP per second", o.hp_regen * 100.0));
        }
        if o.sp_regen != 0.0 {
            ok(format!("Regenerates {:.1}% SP per second", o.sp_regen * 100.0));
        }
        if o.spell_dc != 0 {
            ok(format!("{:+} to your spell save DC", o.spell_dc));
        }
        if o.spell_power != 0 {
            ok(format!("{:+} damage on every damaging spell", o.spell_power));
        }
        if o.heal_power != 0 {
            ok(format!("{:+} HP on every heal you cast", o.heal_power));
        }
        if o.soul_drain != 0 {
            ok(format!("{:+} SP for every kill by a skill", o.soul_drain));
        }
        if o.potion != 0.0 {
            ok(format!("Potions and food restore {}% more", pct(o.potion)));
        }
        if o.loot != 0.0 {
            ok(format!("+{}% drop chance", pct(o.loot)));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::super::items::{find, Kind};
    use super::*;

    #[test]
    fn every_magic_item_is_in_the_catalogue_and_says_what_it_does() {
        for (i, g) in MAGIC.iter().enumerate() {
            assert!(MAGIC[..i].iter().all(|o| o.id != g.id), "duplicate {}", g.id);
            let e = find(g.id).unwrap_or_else(|| panic!("{} is not in the catalogue", g.id));
            assert!(e.slot().is_some(), "{} is equipment", g.id);
            assert_ne!(g.rarity, Rarity::Common);
            assert_eq!(e.rarity, g.rarity);
            // Something beyond the base row: a line of its own, or (armour)
            // a better row than its plain kin.
            assert!(!g.lines().is_empty() || matches!(e.kind, Kind::Armour(..)), "{} does nothing", g.id);
        }
    }

    /// 5e levels 1–5: no enchantment stacks into a game changer.
    #[test]
    fn enchantments_stay_modest() {
        for g in MAGIC {
            let o = &g.mods;
            assert!(o.ac <= 1 && g.saves <= 1 && o.to_hit <= 1, "{}", g.id);
            assert!(o.crit <= 1 && o.double <= 10 && o.venom <= 15, "{}", g.id);
            assert!(o.haste <= 0.15 && o.speed <= 0.1 && o.resist <= 0.05, "{}", g.id);
            assert!(o.damage.is_none_or(|d| d.count == 1 && d.sides <= 4), "{}", g.id);
            assert!(o.spell_dc <= 1 && o.spell_power <= 2 && o.heal_power <= 3 && o.damage_flat <= 2, "{}", g.id);
            assert!(o.hp_regen <= 0.003 && o.sp_regen <= 0.003 && o.potion <= 0.5 && o.loot <= 0.05, "{}", g.id);
            assert!(!(o.hidden || o.advantage || o.mana_shield || o.cloak || o.ambush), "{}: no special states", g.id);
        }
        let rare = MAGIC.iter().filter(|g| g.rarity == Rarity::Rare).count();
        assert!(rare * 3 < MAGIC.len(), "mostly uncommon: {rare} of {}", MAGIC.len());
    }

    /// Every magic item drops somewhere in the baked pack, from an elite or
    /// a boss (shared with the party) or rarely from a regular monster, and
    /// never more often than an elite's 3% or a boss's 5%.
    #[test]
    fn every_magic_item_drops_somewhere_and_stays_rare() {
        use crate::world::combat::Archetype;
        use crate::world::horde::catalog::MonsterCatalog;
        let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
        let Ok(c) = MonsterCatalog::load(&dist) else { return };
        for g in MAGIC {
            let rows: Vec<_> = c.defs().iter().flat_map(|d| d.drops.iter().map(move |r| (d, r))).filter(|(_, r)| r.item == g.id).collect();
            assert!(!rows.is_empty(), "{} drops nowhere", g.id);
            for (d, r) in rows {
                let cap = match d.archetype {
                    Archetype::Boss => 500,
                    Archetype::Elite => 300,
                    _ => 20,
                };
                assert!(r.rate <= cap, "{} from {}: {} / 10000", g.id, d.id, r.rate);
                if g.rarity == Rarity::Rare {
                    assert!(d.archetype == Archetype::Boss || d.archetype == Archetype::Elite, "{} is a boss or elite drop", g.id);
                    assert!(r.rate <= 200, "{} from {}: rare items stay at 2%", g.id, d.id);
                }
            }
        }
    }
}
