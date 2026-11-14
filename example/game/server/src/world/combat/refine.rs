//! Refining (COMBAT.md §2.6.2): a smith hammers a weapon or a piece of
//! armour one level up, +1 to +10 (RO's refine).
//!
//! Each level costs one ore and a fee. The ore follows the level: Phracon
//! for +1…+4, Emveretarcon for +5…+7, then Oridecon (weapons) or Elunium
//! (armour) for +8…+10. Up to an item's safe limit every attempt works;
//! past it the odds fall step by step, and a failed attempt costs the ore,
//! the fee and a roll of levels: the higher the item already is, the bigger
//! the die (d2 up to +5, d4 at +6 and +7, d6 at +8, d8 at +9), so a failed
//! +9 → +10 can throw it back to +1. Nothing ever breaks: drop-only magic
//! items are too rare to lose to a hammer.
//!
//! What a level adds (5e-sized, RO-shaped): a weapon +1 damage per two
//! levels (+5 at +10); body armour +1 AC at +4 and at +8; every refinable
//! piece of armour +3 max HP per level.

use super::items::{Entry, Kind, Slot, MAX_REFINE};

/// The refine ores, by catalogue id.
pub const PHRACON: &str = "phracon";
pub const EMVERETARCON: &str = "emveretarcon";
pub const ORIDECON: &str = "oridecon";
pub const ELUNIUM: &str = "elunium";

/// Odds of an attempt past the safe limit, by how far past (1, 2, …).
const PAST_SAFE: [u32; 6] = [70, 55, 40, 30, 20, 10];
/// Fee of the attempt that reaches +1 … +10.
const FEES: [u32; MAX_REFINE as usize] = [50, 100, 200, 400, 1000, 2000, 4000, 10_000, 20_000, 40_000];

/// One attempt: what it takes and how likely it works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// The level it reaches on success.
    pub to: u8,
    pub ore: &'static str,
    pub fee: u32,
    /// Percent, 1..=100.
    pub chance: u32,
    /// A failure loses 1d`slip` levels (never below +0).
    pub slip: u32,
}

/// Sides of the die a failure from level `from` loses levels by.
pub const fn slip_die(from: u8) -> u32 {
    match from {
        0..=5 => 2,
        6 | 7 => 4,
        8 => 6,
        _ => 8,
    }
}

/// The smith's slip roll comes as 1..=[`SLIP_ROLL`]: every die above
/// divides it, so `(slip - 1) % sides + 1` is a fair roll of each.
pub const SLIP_ROLL: u32 = 24;

/// Levels a failure from `from` loses with the slip roll `slip`
/// (1..=[`SLIP_ROLL`]).
pub fn slip_levels(from: u8, slip: u32) -> u8 {
    let sides = slip_die(from);
    ((slip.clamp(1, SLIP_ROLL) - 1) % sides + 1) as u8
}

/// What a refined item adds while worn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefineBonus {
    /// Flat damage on every weapon hit.
    pub damage: i32,
    pub ac: i32,
    pub hp: i32,
}

/// Whether a smith can work it: weapons, body armour, shields, upper
/// headgear, garments and shoes (RO's refinable slots).
pub fn refinable(e: &Entry) -> bool {
    matches!(e.kind, Kind::Weapon | Kind::Armour(..) | Kind::Shield | Kind::Headgear(Slot::UpperHead) | Kind::Garment | Kind::Shoes)
}

/// Highest level reached without risk: armour +4, weapons by their level
/// (+7 for a level-1 weapon down to +4 for a level-4 one).
pub fn safe_limit(e: &Entry) -> u8 {
    match e.weapon {
        Some(w) => 8u8.saturating_sub(w.level.clamp(1, 4)),
        None => 4,
    }
}

/// The attempt from level `from`, or `None` (not refinable, already +10).
pub fn step(e: &Entry, from: u8) -> Option<Step> {
    if !refinable(e) || from >= MAX_REFINE {
        return None;
    }
    let to = from + 1;
    let ore = match to {
        1..=4 => PHRACON,
        5..=7 => EMVERETARCON,
        _ if e.weapon.is_some() => ORIDECON,
        _ => ELUNIUM,
    };
    let safe = safe_limit(e);
    let chance = if to <= safe { 100 } else { PAST_SAFE[(to - safe - 1) as usize] };
    Some(Step { to, ore, fee: FEES[from as usize], chance, slip: slip_die(from) })
}

/// What `e` at `refine` adds while worn.
pub fn bonus(e: &Entry, refine: u8) -> RefineBonus {
    if refine == 0 || !refinable(e) {
        return RefineBonus::default();
    }
    let r = refine.min(MAX_REFINE) as i32;
    match e.kind {
        Kind::Weapon => RefineBonus { damage: (r + 1) / 2, ..RefineBonus::default() },
        Kind::Armour(..) => RefineBonus { ac: r / 4, hp: 3 * r, ..RefineBonus::default() },
        _ => RefineBonus { hp: 3 * r, ..RefineBonus::default() },
    }
}

/// How an attempt went (`ServerMsg::Refined`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refined {
    /// The instance key the item has now.
    pub key: String,
    pub ok: bool,
    /// Its level now.
    pub refine: u8,
    /// On a failure the face the slip die showed (1..=`Step::slip`; the
    /// item lost that many levels, fewer when it hit +0); 0 on a success.
    pub slip: u8,
}

#[cfg(test)]
mod tests {
    use super::super::items::find;
    use super::*;

    /// The whole table, one line per item kind: `+level:ore:fee:chance`.
    /// The client (`refine.dart`) mirrors it: change both together
    /// (`client_test.dart`, `refine table`).
    fn table(id: &str) -> String {
        let e = find(id).unwrap();
        (0..MAX_REFINE)
            .map(|r| step(e, r).unwrap())
            .map(|s| if s.chance < 100 { format!("{}:{}:{}:{}d{}", s.to, &s.ore[..2], s.fee, s.chance, s.slip) } else { format!("{}:{}:{}:{}", s.to, &s.ore[..2], s.fee, s.chance) })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn ores_fees_and_odds_follow_the_level() {
        assert_eq!(
            table("sword"),
            "1:ph:50:100 2:ph:100:100 3:ph:200:100 4:ph:400:100 5:em:1000:100 6:em:2000:100 7:em:4000:100 8:or:10000:70d4 9:or:20000:55d6 10:or:40000:40d8"
        );
        assert_eq!(
            table("blade"),
            "1:ph:50:100 2:ph:100:100 3:ph:200:100 4:ph:400:100 5:em:1000:100 6:em:2000:70d2 7:em:4000:55d4 8:or:10000:40d4 9:or:20000:30d6 10:or:40000:20d8"
        );
        assert_eq!(
            table("coat"),
            "1:ph:50:100 2:ph:100:100 3:ph:200:100 4:ph:400:100 5:em:1000:70d2 6:em:2000:55d2 7:em:4000:40d4 8:el:10000:30d4 9:el:20000:20d6 10:el:40000:10d8"
        );
        assert_eq!(step(find("sword").unwrap(), MAX_REFINE), None);
        assert_eq!(step(find("ring").unwrap(), 0), None, "accessories are not refined");
        assert_eq!(step(find("sunglasses").unwrap(), 0), None, "nor mid headgear");
        assert!(step(find("hat").unwrap(), 0).is_some());
        assert!(step(find("ember_blade").unwrap(), 0).is_some(), "magic items are refined too");
    }

    #[test]
    fn a_failure_slips_further_the_higher_the_item_is() {
        assert_eq!((slip_die(4), slip_die(5), slip_die(6), slip_die(7), slip_die(8), slip_die(9)), (2, 2, 4, 4, 6, 8));
        // Every die is a fair roll of the slip: each face as often.
        for from in [4u8, 6, 8, 9] {
            let sides = slip_die(from);
            let mut seen = vec![0; sides as usize];
            for s in 1..=SLIP_ROLL {
                seen[slip_levels(from, s) as usize - 1] += 1;
            }
            assert!(seen.iter().all(|&n| n == SLIP_ROLL / sides), "{from}: {seen:?}");
        }
        // A failed +9 → +10 can throw a sword back to +1.
        let mut b = bag();
        b.add(ORIDECON, 1).unwrap();
        b.add("sword+9", 1).unwrap();
        let r = b.refine("sword+9", None, 100, SLIP_ROLL).unwrap();
        assert_eq!(r, Refined { key: "sword+1".into(), ok: false, refine: 1, slip: 8 });
        let mut low = bag();
        low.add("coat+1", 1).unwrap();
        low.equip[Slot::Armour as usize] = "coat+4".into();
        let r = low.refine("coat+4", Some(Slot::Armour), 100, 2).unwrap();
        assert_eq!((r.refine, r.slip), (2, 2), "a d2 from +4");
    }

    #[test]
    fn levels_add_damage_armour_and_health() {
        let sword = find("sword").unwrap();
        assert_eq!((bonus(sword, 1).damage, bonus(sword, 4).damage, bonus(sword, 10).damage), (1, 2, 5));
        let coat = find("coat").unwrap();
        assert_eq!(bonus(coat, 3), RefineBonus { ac: 0, hp: 9, damage: 0 });
        assert_eq!(bonus(coat, 8), RefineBonus { ac: 2, hp: 24, damage: 0 });
        assert_eq!(bonus(find("boots").unwrap(), 5), RefineBonus { hp: 15, ..RefineBonus::default() });
        assert_eq!(bonus(find("ring").unwrap(), 5), RefineBonus::default());
    }

    use super::super::class::Class;
    use super::super::items::{Bag, ItemError};

    fn bag() -> Bag {
        let mut b = Bag::starting(Class::Swordsman, "sword");
        b.zeny = 100_000;
        b.add(PHRACON, 3).unwrap();
        b.add(EMVERETARCON, 1).unwrap();
        b
    }

    #[test]
    fn a_worn_weapon_is_refined_in_place_for_ore_and_zeny() {
        let mut b = bag();
        let r = b.refine("sword", Some(Slot::Weapon), 100, 1).unwrap();
        assert_eq!(r, Refined { key: "sword+1".into(), ok: true, refine: 1, slip: 0 });
        assert_eq!(b.equip[Slot::Weapon as usize], "sword+1");
        assert_eq!((b.count(PHRACON), b.zeny), (2, 100_000 - 50));
        // The worn copy is what the slot names.
        assert_eq!(b.refine("sword", Some(Slot::Weapon), 1, 1), Err(ItemError::NotInBag));
        b.refine("sword+1", Some(Slot::Weapon), 1, 1).unwrap();
        b.refine("sword+2", Some(Slot::Weapon), 1, 1).unwrap();
        assert_eq!(b.refine("sword+3", Some(Slot::Weapon), 1, 1), Err(ItemError::NoOre), "out of Phracon");
        let worn = b.gear(Class::Swordsman);
        assert_eq!(worn.mods.damage_flat, 2, "+3: two more damage a hit");
    }

    #[test]
    fn a_bag_copy_comes_off_its_stack_and_a_failure_costs_a_level() {
        let mut b = bag();
        b.add("coat+4", 2).unwrap();
        // +5 is past armour's safe +4: 70%.
        let r = b.refine("coat+4", None, 71, 1).unwrap();
        assert_eq!(r, Refined { key: "coat+3".into(), ok: false, refine: 3, slip: 1 });
        assert_eq!((b.count("coat+4"), b.count("coat+3"), b.count(EMVERETARCON)), (1, 1, 0));
        assert_eq!(b.zeny, 100_000 - 1000);
        assert_eq!(b.refine("coat+4", None, 1, 1), Err(ItemError::NoOre));
        b.zeny = 10;
        assert_eq!(b.refine("coat+3", None, 1, 1), Err(ItemError::NoZeny));
        assert_eq!(b.refine("red_potion", None, 1, 1), Err(ItemError::NotRefinable));
        let mut top = bag();
        top.add("sword+10", 1).unwrap();
        assert_eq!(top.refine("sword+10", None, 1, 1), Err(ItemError::MaxRefine));
    }

    #[test]
    fn refined_armour_adds_to_what_the_gear_adds_up_to() {
        let mut b = bag();
        let plain = b.gear(Class::Swordsman);
        b.equip[Slot::Armour as usize] = "cotton_shirt+8".into();
        b.equip[Slot::Shield as usize] = "guard+2".into();
        let g = b.gear(Class::Swordsman);
        assert_eq!((g.ac - plain.ac, g.hp - plain.hp), (2, 24 + 6));
    }
}
