//! Recipes: a recipe giver in town (NPC kind `hatter`) makes headgear from
//! the Etc items monsters and field plants drop, plus a small fee; an
//! Alchemist's Pharmacy (a skill, opened anywhere in town) brews potions
//! and the alchemy bottles from herbs and drops the same way, its level
//! gating the recipes. Repeatable; all or nothing (`craft`), never a
//! failure roll. The hub checks where the player stands and the Pharmacy
//! level (`hub::quest`), the socket task applies the craft to the bag.

use super::items::{self, Bag, ItemError, Kind};

/// One recipe: `needs` and `zeny` in, one `item` out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recipe {
    pub id: &'static str,
    pub item: &'static str,
    pub zeny: u32,
    pub needs: &'static [(&'static str, u32)],
    /// Pharmacy level it needs (0: a Hatter's recipe).
    pub level: u8,
}

const fn r(item: &'static str, zeny: u32, needs: &'static [(&'static str, u32)]) -> Recipe {
    Recipe { id: item, item, zeny, needs, level: 0 }
}

const fn brew(item: &'static str, level: u8, zeny: u32, needs: &'static [(&'static str, u32)]) -> Recipe {
    Recipe { id: item, item, zeny, needs, level }
}

/// The Pharmacy's recipes (an Alchemist's skill), by the level they need.
/// Every input is a drop, never shop stock, so no brew turns zeny into
/// more zeny even at the top Discount and Overcharge (a test checks it).
pub const PHARMACY: &[Recipe] = &[
    brew("red_potion", 1, 10, &[("red_herb", 2)]),
    brew("green_potion", 1, 5, &[("green_herb", 2)]),
    brew("yellow_potion", 2, 40, &[("yellow_herb", 2)]),
    // Acid Demonstration's pair: the meadow's mushrooms and slimes, the
    // forest's resin and Fabres' fluff.
    brew("acid_bottle", 2, 50, &[("poison_spore", 2), ("sticky_mucus", 1)]),
    brew("bottle_grenade", 2, 50, &[("resin", 1), ("fluff", 2)]),
    brew("white_potion", 3, 100, &[("white_herb", 2)]),
    brew("blue_potion", 3, 150, &[("blue_herb", 2)]),
    // The homunculus's evolution.
    brew("stone_of_sage", 5, 5000, &[("emperium", 1), ("light_granule", 10)]),
];

/// Pharmacy level from which a brew makes two now and then.
pub const DOUBLE_LEVEL: u8 = 5;
/// Percent chance of two from one brew at [`DOUBLE_LEVEL`].
pub const DOUBLE_CHANCE: u32 = 20;

/// The Hatter's recipes, cheapest first. Plain hats cost less in coin than
/// at the Armor Dealer (the materials are the price); the two magic ones
/// (`magic.rs`) are a long farm instead of a lucky drop.
pub const HATTER: &[Recipe] = &[
    // Poring Meadow: Porings and Drops.
    r("ribbon", 200, &[("jellopy", 10), ("sticky_mucus", 3)]),
    // Fabres and Lunatics (the desert's wolves and birds too).
    r("hat", 300, &[("fluff", 10), ("feather", 5)]),
    // The meadow's mushrooms.
    r("flu_mask", 300, &[("mushroom_spore", 6), ("poison_spore", 3)]),
    // Sograt Desert: Frilldoras and Sand Men.
    r("sunglasses", 600, &[("shell", 6), ("garlet", 6)]),
    // The Culvert's thief bugs, Hornets and Scorpions.
    r("bandit_cap", 2000, &[("worm_peeling", 20), ("bee_sting", 8), ("jellopy", 10)]),
    // Black Mushrooms, Roda Frogs and Fabres' herbs.
    r("plague_mask", 2500, &[("poison_spore", 12), ("sticky_webfoot", 10), ("green_herb", 5)]),
];

/// A recipe of either book by id (Hatter first; the ids do not overlap).
pub fn find(id: &str) -> Option<&'static Recipe> {
    HATTER.iter().chain(PHARMACY).find(|r| r.id == id)
}

/// Whether some recipe makes `item` (its tooltip says so).
pub fn makes(item: &str) -> bool {
    HATTER.iter().chain(PHARMACY).any(|r| r.item == item)
}

/// Makes `recipe` from the bag: every material (plain copies) and the fee
/// must be there, and the hat must fit; else nothing changes.
pub fn craft(bag: &mut Bag, recipe: &Recipe) -> Result<(), ItemError> {
    craft_n(bag, recipe, 1)
}

/// [`craft`] that hands out `count` of the item (a Pharmacy's double brew).
pub fn craft_n(bag: &mut Bag, recipe: &Recipe, count: u32) -> Result<(), ItemError> {
    let mut next = bag.clone();
    if next.zeny < recipe.zeny as u64 {
        return Err(ItemError::NoZeny);
    }
    for &(item, n) in recipe.needs {
        next.take(item, n)?;
    }
    next.zeny -= recipe.zeny as u64;
    next.add(recipe.item, count.max(1))?;
    *bag = next;
    Ok(())
}

/// Whether every recipe names real items: headgear out, Etc or
/// consumables in (a test, and a guard for the catalogue).
pub fn valid(recipe: &Recipe) -> bool {
    let out: fn(Kind) -> bool = match recipe.level {
        0 => |k: Kind| matches!(k, Kind::Headgear(_)),
        _ => |k: Kind| matches!(k, Kind::Consumable(_) | Kind::Etc),
    };
    items::find(recipe.item).is_some_and(|e| out(e.kind))
        && !recipe.needs.is_empty()
        && recipe.needs.iter().all(|(id, n)| *n > 0 && items::find(id).is_some_and(|e| matches!(e.kind, Kind::Etc | Kind::Consumable(_))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::combat::Class;

    #[test]
    fn every_recipe_turns_drops_into_a_hat() {
        assert!((4..=6).contains(&HATTER.len()));
        for r in HATTER {
            assert!(valid(r), "{}", r.id);
            assert_eq!(find(r.id), Some(r));
        }
        assert!(makes("bandit_cap") && !makes("helm"));
        for r in PHARMACY {
            assert!(valid(r) && (1..=5).contains(&r.level), "{}", r.id);
            assert_eq!(find(r.id), Some(r));
        }
        assert!(makes("acid_bottle") && makes("stone_of_sage"));
    }

    /// Nothing a Pharmacy brews from is shop stock, and no shop item sold
    /// back makes money: with Discount and Overcharge at their top a loop
    /// from zeny to zeny always loses (a double brew counted every time).
    #[test]
    fn no_brew_or_shop_loop_is_a_money_pump() {
        use crate::world::combat::items::{Haggle, SHOPS};
        use crate::world::combat::skills::MAX_HAGGLE;
        let top = Haggle { buy: MAX_HAGGLE, sell: MAX_HAGGLE };
        let price = |id: &str| items::find(id).unwrap().price;
        let stocked = |id: &str| SHOPS.iter().any(|(_, s)| s.contains(&id));
        for (_, stock) in SHOPS {
            for id in *stock {
                assert!(top.buy_cost(price(id), 1) > top.sell_gain(price(id), 1), "{id}: bought and sold back at a profit");
            }
        }
        for r in PHARMACY {
            let bought = r.needs.iter().all(|(id, _)| stocked(id));
            let cost: u64 = r.needs.iter().map(|(id, n)| top.buy_cost(price(id), *n)).sum::<u64>() + r.zeny as u64;
            let gain = top.sell_gain(price(r.item), 2);
            assert!(!bought || gain < cost, "{}: a brew from shop stock sells for more than it cost", r.id);
            assert!(r.needs.iter().all(|(id, _)| !stocked(id)), "{}: brews from shop stock", r.id);
        }
    }

    #[test]
    fn a_craft_is_all_or_nothing_and_repeatable() {
        let ribbon = find("ribbon").unwrap();
        let mut bag = Bag::starting(Class::Mage, "");
        bag.zeny = 450;
        bag.add("jellopy", 25).unwrap();
        bag.add("sticky_mucus", 2).unwrap();
        let before = bag.clone();
        assert_eq!(craft(&mut bag, ribbon), Err(ItemError::NotInBag), "one mucus short");
        assert_eq!(bag, before, "nothing taken");

        bag.add("sticky_mucus", 4).unwrap();
        craft(&mut bag, ribbon).unwrap();
        assert_eq!((bag.count("ribbon"), bag.count("jellopy"), bag.count("sticky_mucus"), bag.zeny), (1, 15, 3, 250));
        craft(&mut bag, ribbon).unwrap();
        assert_eq!((bag.count("ribbon"), bag.count("jellopy"), bag.count("sticky_mucus"), bag.zeny), (2, 5, 0, 50));
        assert_eq!(craft(&mut bag, ribbon), Err(ItemError::NoZeny));
    }

    #[test]
    fn refined_or_carded_materials_do_not_count() {
        // Etc items never carry refines or cards, so a plain key is the
        // only stack a recipe looks at; a magic hat comes out plain.
        let cap = find("bandit_cap").unwrap();
        let mut bag = Bag { zeny: 5000, ..Bag::default() };
        for &(item, n) in cap.needs {
            bag.add(item, n).unwrap();
        }
        craft(&mut bag, cap).unwrap();
        assert_eq!(bag.count("bandit_cap"), 1);
        assert!(bag.items.iter().all(|s| s.id == "bandit_cap"), "{:?}", bag.items);
    }
}
