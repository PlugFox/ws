//! Items (COMBAT.md §2.2, §2.6): what a character carries, wears and buys.
//!
//! The catalogue is two const tables: [`ITEMS`] here (potions, armour,
//! shields, headgear, garments, shoes, accessories) and
//! [`super::weapon::WEAPONS`]; [`catalogue`] merges them into one list of
//! [`Entry`] rows with the description shops and tooltips show. Item names
//! are RO's; the numbers are 5e's (armour tables, potions of healing).
//!
//! A character's belongings are a [`Bag`]: stacks, the ten equipment slots
//! and the zeny. Every rule (buying, selling, equipping, what the gear
//! adds up to) is a pure function of the bag, so the socket task applies
//! it to the session's character and the stage reads the result.

use super::class::{Class, Save};
use super::dice::Dice;
use super::weapon::{Weapon, WEAPONS};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// Equipment slots (RO's ten), in wire order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Slot {
    UpperHead,
    MidHead,
    LowerHead,
    Armour,
    Weapon,
    Shield,
    Garment,
    Shoes,
    Accessory1,
    Accessory2,
}

pub const EQUIP_SLOTS: usize = 10;

impl Slot {
    pub const ALL: [Slot; EQUIP_SLOTS] = [
        Slot::UpperHead,
        Slot::MidHead,
        Slot::LowerHead,
        Slot::Armour,
        Slot::Weapon,
        Slot::Shield,
        Slot::Garment,
        Slot::Shoes,
        Slot::Accessory1,
        Slot::Accessory2,
    ];

    pub fn from_index(i: u8) -> Option<Slot> {
        Self::ALL.get(i as usize).copied()
    }

    pub const fn label(self) -> &'static str {
        match self {
            Slot::UpperHead => "Upper headgear",
            Slot::MidHead => "Middle headgear",
            Slot::LowerHead => "Lower headgear",
            Slot::Armour => "Armor",
            Slot::Weapon => "Weapon",
            Slot::Shield => "Shield",
            Slot::Garment => "Garment",
            Slot::Shoes => "Shoes",
            Slot::Accessory1 | Slot::Accessory2 => "Accessory",
        }
    }
}

/// Body armour weight (5e): how much of the class defence still counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    /// Base + the full class defence.
    Light,
    /// Base + class defence up to +2.
    Medium,
    /// Base only.
    Heavy,
}

/// What drinking a potion does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Restores HP (5e potions of healing).
    Heal(Dice),
    /// Restores SP.
    Spirit(Dice),
    /// Ends poison and blindness.
    Cure,
    /// Ends poison only (a Green Herb).
    Antidote,
}

/// Item families; the wire code is the discriminant (0 = etc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Etc,
    Consumable(Effect),
    Weapon,
    Armour(Weight, i8),
    Shield,
    Headgear(Slot),
    Garment,
    Shoes,
    Accessory,
}

impl Kind {
    pub const fn code(self) -> u8 {
        match self {
            Kind::Etc => 0,
            Kind::Consumable(_) => 1,
            Kind::Weapon => 2,
            Kind::Armour(..) => 3,
            Kind::Shield => 4,
            Kind::Headgear(_) => 5,
            Kind::Garment => 6,
            Kind::Shoes => 7,
            Kind::Accessory => 8,
        }
    }

    /// The slot it is worn in (accessories: the first of the two).
    pub const fn slot(self) -> Option<Slot> {
        match self {
            Kind::Etc | Kind::Consumable(_) => None,
            Kind::Weapon => Some(Slot::Weapon),
            Kind::Armour(..) => Some(Slot::Armour),
            Kind::Shield => Some(Slot::Shield),
            Kind::Headgear(s) => Some(s),
            Kind::Garment => Some(Slot::Garment),
            Kind::Shoes => Some(Slot::Shoes),
            Kind::Accessory => Some(Slot::Accessory1),
        }
    }
}

/// Class mask bits (`Class as u8`).
pub const fn class_bit(c: Class) -> u8 {
    1 << c as u8
}
const SW: u8 = 1 << Class::Swordsman as u8;
const AR: u8 = 1 << Class::Archer as u8;
const MG: u8 = 1 << Class::Mage as u8;
const AC: u8 = 1 << Class::Acolyte as u8;
pub const ALL_CLASSES: u8 = SW | AR | MG | AC;

/// What a worn item adds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bonus {
    pub ac: i8,
    pub hp: i16,
    pub sp: i16,
    /// Bonus to one saving throw.
    pub save: Option<(Save, i8)>,
}

impl Bonus {
    const NONE: Bonus = Bonus { ac: 0, hp: 0, sp: 0, save: None };
}

/// One non-weapon item of the catalogue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    pub price: u32,
    /// Classes that may wear it (`class_bit` mask).
    pub classes: u8,
    pub bonus: Bonus,
    /// Flavour line for the tooltip.
    pub flavour: &'static str,
}

const fn item(id: &'static str, name: &'static str, kind: Kind, price: u32, classes: u8, bonus: Bonus, flavour: &'static str) -> Item {
    Item { id, name, kind, price, classes, bonus, flavour }
}
const fn potion(id: &'static str, name: &'static str, effect: Effect, price: u32, flavour: &'static str) -> Item {
    item(id, name, Kind::Consumable(effect), price, ALL_CLASSES, Bonus::NONE, flavour)
}
const fn ac(ac: i8) -> Bonus {
    Bonus { ac, ..Bonus::NONE }
}
const fn hp(hp: i16) -> Bonus {
    Bonus { hp, ..Bonus::NONE }
}
const fn sp(sp: i16) -> Bonus {
    Bonus { sp, ..Bonus::NONE }
}
const fn save(s: Save, n: i8) -> Bonus {
    Bonus { save: Some((s, n)), ..Bonus::NONE }
}

/// Potions, armour and accessories (weapons are [`WEAPONS`]).
pub const ITEMS: &[Item] = &[
    // Potions: 5e's potions of healing (common, greater, superior, supreme).
    potion("red_potion", "Red Potion", Effect::Heal(Dice::new(2, 4, 2)), 50, "A potion made from grinded Red Herbs."),
    potion("orange_potion", "Orange Potion", Effect::Heal(Dice::new(4, 4, 4)), 200, "Red and Yellow Herbs, ground together."),
    potion("yellow_potion", "Yellow Potion", Effect::Heal(Dice::new(8, 4, 8)), 550, "A potion made from grinded Yellow Herbs."),
    potion("white_potion", "White Potion", Effect::Heal(Dice::new(10, 4, 20)), 1200, "A potion made from grinded White Herbs."),
    potion("blue_potion", "Blue Potion", Effect::Spirit(Dice::new(4, 4, 4)), 800, "A potion made from grinded Blue Herbs."),
    potion("green_potion", "Green Potion", Effect::Cure, 40, "A potion made from grinded Green Herbs."),
    // Body armour: light 11–12 + class defence, medium 12–15 + defence up
    // to +2, heavy 16–18 flat (5e).
    item("cotton_shirt", "Cotton Shirt", Kind::Armour(Weight::Light, 11), 10, ALL_CLASSES, Bonus::NONE, "A plain shirt of cotton."),
    item("jacket", "Leather Jacket", Kind::Armour(Weight::Light, 11), 200, ALL_CLASSES, hp(6), "A sturdy jacket of tanned leather."),
    item("adventurers_suit", "Adventurer's Suit", Kind::Armour(Weight::Light, 12), 1000, ALL_CLASSES, Bonus::NONE, "Comfortable clothes for long journeys."),
    item("silk_robe", "Silk Robe", Kind::Armour(Weight::Light, 11), 1500, MG | AC, sp(10), "A robe of fine silk that helps a caster focus."),
    item("tights", "Tights", Kind::Armour(Weight::Light, 12), 4000, AR, save(Save::Reflex, 1), "Close-fitting clothes that never catch on a branch."),
    item("silver_robe", "Silver Robe", Kind::Armour(Weight::Light, 12), 6000, MG, sp(20), "A robe woven with silver thread."),
    item("wooden_mail", "Wooden Mail", Kind::Armour(Weight::Medium, 12), 1200, SW | AR | AC, Bonus::NONE, "Armour of hardened wooden plates."),
    item("coat", "Coat", Kind::Armour(Weight::Medium, 13), 2500, SW | AR | AC, Bonus::NONE, "A long coat with leather padding."),
    item("saints_robe", "Saint's Robe", Kind::Armour(Weight::Medium, 13), 5000, AC, save(Save::Mind, 1), "A blessed robe for servants of God."),
    item("padded_armor", "Padded Armor", Kind::Armour(Weight::Medium, 14), 6000, SW | AR | AC, Bonus::NONE, "Thick layers of quilted cloth."),
    item("holy_robe", "Holy Robe", Kind::Armour(Weight::Medium, 15), 14000, AC, save(Save::Mind, 1), "Vestments that turn evil aside."),
    item("chain_mail", "Chain Mail", Kind::Armour(Weight::Heavy, 16), 9000, SW, Bonus::NONE, "Interlocking rings of steel."),
    item("full_plate", "Full Plate", Kind::Armour(Weight::Heavy, 18), 30000, SW, Bonus::NONE, "Shaped steel plates over the whole body."),
    // Shields: +2 AC (5e), not with a two-handed weapon.
    item("guard", "Guard", Kind::Shield, 50, SW | MG | AC, ac(2), "A small round shield."),
    item("buckler", "Buckler", Kind::Shield, 1500, SW | AC, Bonus { ac: 2, save: Some((Save::Reflex, 1)), ..Bonus::NONE }, "A light shield that turns blows aside."),
    item("shield", "Shield", Kind::Shield, 9000, SW, Bonus { ac: 2, hp: 15, ..Bonus::NONE }, "A heavy shield of iron."),
    item("mirror_shield", "Mirror Shield", Kind::Shield, 16000, SW | AC, Bonus { ac: 2, save: Some((Save::Mind, 1)), ..Bonus::NONE }, "A polished shield that throws spells back."),
    // Headgear.
    item("bandana", "Bandana", Kind::Headgear(Slot::UpperHead), 300, ALL_CLASSES, hp(4), "A cloth tied around the head."),
    item("ribbon", "Ribbon", Kind::Headgear(Slot::UpperHead), 800, ALL_CLASSES, sp(6), "A pretty ribbon for the hair."),
    item("hat", "Hat", Kind::Headgear(Slot::UpperHead), 1000, ALL_CLASSES, hp(8), "A hat with a broad brim."),
    item("biretta", "Biretta", Kind::Headgear(Slot::UpperHead), 3000, AC, save(Save::Mind, 1), "A square cap of the clergy."),
    item("circlet", "Circlet", Kind::Headgear(Slot::UpperHead), 3000, MG | AC, sp(12), "A thin band of metal around the brow."),
    item("cap", "Cap", Kind::Headgear(Slot::UpperHead), 4000, SW | AR | AC, ac(1), "A leather cap with metal studs."),
    item("helm", "Helm", Kind::Headgear(Slot::UpperHead), 12000, SW, Bonus { ac: 1, hp: 10, ..Bonus::NONE }, "A steel helmet."),
    item("sunglasses", "Sunglasses", Kind::Headgear(Slot::MidHead), 2000, ALL_CLASSES, save(Save::Reflex, 1), "Keeps the glare out of your eyes."),
    item("geek_glasses", "Geek Glasses", Kind::Headgear(Slot::MidHead), 2000, ALL_CLASSES, save(Save::Mind, 1), "Thick lenses for thick books."),
    item("flu_mask", "Flu Mask", Kind::Headgear(Slot::LowerHead), 1500, ALL_CLASSES, save(Save::Body, 1), "Keeps germs and poison out."),
    // Garments, shoes, accessories.
    item("hood", "Hood", Kind::Garment, 300, ALL_CLASSES, hp(5), "A hooded cape."),
    item("muffler", "Muffler", Kind::Garment, 2500, ALL_CLASSES, save(Save::Body, 1), "A warm scarf."),
    item("manteau", "Manteau", Kind::Garment, 8000, SW | AR | AC, ac(1), "A thick cloak that takes a blow."),
    item("sandals", "Sandals", Kind::Shoes, 300, ALL_CLASSES, hp(4), "Simple sandals."),
    item("shoes", "Shoes", Kind::Shoes, 2000, ALL_CLASSES, hp(10), "Leather shoes."),
    item("boots", "Boots", Kind::Shoes, 7000, SW | AR | AC, hp(18), "Sturdy boots for long marches."),
    item("clip", "Clip", Kind::Accessory, 1500, ALL_CLASSES, sp(10), "A hair clip that clears the mind."),
    item("rosary", "Rosary", Kind::Accessory, 2500, ALL_CLASSES, save(Save::Mind, 1), "Prayer beads."),
    item("ring", "Ring", Kind::Accessory, 2500, ALL_CLASSES, save(Save::Body, 1), "A plain iron ring."),
    item("earring", "Earring", Kind::Accessory, 2500, ALL_CLASSES, save(Save::Reflex, 1), "A small silver earring."),
    item("brooch", "Brooch", Kind::Accessory, 3000, ALL_CLASSES, hp(12), "A pin with a red stone."),
    // Herbs and food from drops: the same regeneration as potions, weaker
    // (a running potion is never replaced by one).
    potion("apple", "Apple", Effect::Heal(Dice::new(1, 4, 0)), 15, "A juicy red apple."),
    potion("carrot", "Carrot", Effect::Heal(Dice::new(1, 4, 0)), 15, "A crunchy carrot, a Lunatic's favourite."),
    potion("red_herb", "Red Herb", Effect::Heal(Dice::new(1, 6, 1)), 18, "A common herb that soothes small wounds."),
    potion("yellow_herb", "Yellow Herb", Effect::Heal(Dice::new(2, 4, 2)), 40, "A bitter herb that closes cuts."),
    potion("white_herb", "White Herb", Effect::Heal(Dice::new(4, 4, 4)), 120, "A rare herb with a strong healing scent."),
    potion("green_herb", "Green Herb", Effect::Antidote, 20, "A herb that draws poison out of the blood."),
    // Field plants' harvest (COMBAT.md §4.10): the Blue Herb is the only
    // drop that restores SP; honey and the berry come from Shining Plants.
    potion("blue_herb", "Blue Herb", Effect::Spirit(Dice::new(1, 6, 2)), 60, "A cool herb that clears a tired mind."),
    potion("honey", "Honey", Effect::Heal(Dice::new(2, 6, 2)), 100, "Thick golden honey hidden in a Shining Plant."),
    potion("yggdrasil_berry", "Yggdrasil Berry", Effect::Heal(Dice::new(10, 4, 20)), 2500, "A berry of the World Tree; one bite mends nearly anything."),
    // Loot (Etc): sold at any merchant for half the price.
    item("jellopy", "Jellopy", Kind::Etc, 6, 0, Bonus::NONE, "A small crystal left behind by Porings."),
    item("flower", "Flower", Kind::Etc, 4, 0, Bonus::NONE, "A small field flower. It smells of spring."),
    item("stem", "Stem", Kind::Etc, 6, 0, Bonus::NONE, "A tough green stalk cut from a plant."),
    item("mushroom_spore", "Mushroom Spore", Kind::Etc, 10, 0, Bonus::NONE, "A puff of spores shaken from a mushroom."),
    item("poison_spore", "Poison Spore", Kind::Etc, 14, 0, Bonus::NONE, "Spores of a Black Mushroom. Do not breathe in."),
    item("fluff", "Fluff", Kind::Etc, 8, 0, Bonus::NONE, "A soft tuft from a Fabre."),
    item("clover", "Clover", Kind::Etc, 12, 0, Bonus::NONE, "A three-leaf clover. Not the lucky kind."),
    item("tree_root", "Tree Root", Kind::Etc, 12, 0, Bonus::NONE, "A gnarled root torn from a Willow."),
    item("worm_peeling", "Worm Peeling", Kind::Etc, 10, 0, Bonus::NONE, "A shed skin of a crawling insect."),
    item("sticky_mucus", "Sticky Mucus", Kind::Etc, 14, 0, Bonus::NONE, "Slime that will not come off your fingers."),
    item("sticky_webfoot", "Sticky Webfoot", Kind::Etc, 18, 0, Bonus::NONE, "The webbed foot of a frog, still tacky."),
    item("chrysalis", "Chrysalis", Kind::Etc, 24, 0, Bonus::NONE, "The hard shell a Pupa grew in."),
    item("bee_sting", "Bee Sting", Kind::Etc, 24, 0, Bonus::NONE, "A Hornet's stinger, sharp as a needle."),
    item("garlet", "Garlet", Kind::Etc, 24, 0, Bonus::NONE, "A hard green crystal from a Poporing."),
    item("shell", "Shell", Kind::Etc, 28, 0, Bonus::NONE, "The rigid shell of a Chonchon."),
    item("feather", "Feather", Kind::Etc, 30, 0, Bonus::NONE, "A light, soft feather."),
    item("resin", "Resin", Kind::Etc, 32, 0, Bonus::NONE, "Sticky sap that smells of pine."),
    item("grasshoppers_leg", "Grasshopper's Leg", Kind::Etc, 36, 0, Bonus::NONE, "A springy leg of a Rocker."),
    item("powder_of_butterfly", "Powder of Butterfly", Kind::Etc, 160, 0, Bonus::NONE, "Glittering scales from a Creamy's wings."),
    item("light_granule", "Light Granule", Kind::Etc, 1200, 0, Bonus::NONE, "A grain of holy light that never dims."),
    item("azure_jewel", "Aquamarine", Kind::Etc, 3000, 0, Bonus::NONE, "A sea-blue gem."),
    item("white_jewel", "Opal", Kind::Etc, 3000, 0, Bonus::NONE, "A milky gem that shimmers in every colour."),
    item("scarlet_jewel", "Garnet", Kind::Etc, 3000, 0, Bonus::NONE, "A deep red gem."),
    item("emperium", "Emperium", Kind::Etc, 8000, 0, Bonus::NONE, "A golden crystal the angels guard."),
];

/// Merchants and what they sell (`shop` of a `"shop"` NPC).
pub const SHOPS: &[(&str, &[&str])] = &[
    ("tool", &["red_potion", "orange_potion", "yellow_potion", "white_potion", "blue_potion", "green_potion"]),
    (
        "weapon",
        &[
            "knife", "cutter", "main_gauche", "sword", "falchion", "katana", "javelin", "pike", "axe", "battle_axe", "club", "mace",
            "bow", "composite_bow", "rod", "wand", "bible",
        ],
    ),
    (
        "armor",
        &[
            "cotton_shirt", "jacket", "adventurers_suit", "silk_robe", "tights", "wooden_mail", "coat", "saints_robe", "padded_armor",
            "chain_mail", "full_plate", "guard", "buckler", "shield", "bandana", "ribbon", "hat", "biretta", "circlet", "cap", "helm",
            "sunglasses", "geek_glasses", "flu_mask", "hood", "muffler", "manteau", "sandals", "shoes", "boots", "clip", "rosary",
            "ring", "earring", "brooch",
        ],
    ),
];

pub fn shop(id: &str) -> Option<&'static [&'static str]> {
    SHOPS.iter().find(|(s, _)| *s == id).map(|(_, items)| *items)
}

/// Sale price: half the shop price (RO's NPC rate).
pub const fn sell_price(price: u32) -> u32 {
    price / 2
}

/// Shared delay after drinking anything (ticks are the stage's business).
pub const POTION_COOLDOWN_MS: u32 = 1000;
/// A healing or SP potion works over this long (no instant heals).
pub const POTION_REGEN_MS: u32 = 6000;

/// One row of the merged catalogue.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    pub price: u32,
    pub classes: u8,
    pub bonus: Bonus,
    /// Weapon stats when it is one.
    pub weapon: Option<&'static Weapon>,
    /// Tooltip text (`markup()`: `**bold**`, `[ok]…[/]`).
    pub desc: String,
}

impl Entry {
    pub fn slot(&self) -> Option<Slot> {
        self.kind.slot()
    }

    pub fn usable_by(&self, class: Class) -> bool {
        self.classes & class_bit(class) != 0
    }

    pub fn two_handed(&self) -> bool {
        self.weapon.is_some_and(|w| w.kind.two_handed())
    }
}

fn save_name(s: Save) -> &'static str {
    match s {
        Save::Body => "Body",
        Save::Reflex => "Reflex",
        Save::Mind => "Mind",
    }
}

fn bonus_lines(b: &Bonus, out: &mut Vec<String>) {
    if b.ac != 0 {
        out.push(format!("[ok]{:+} AC[/]", b.ac));
    }
    if b.hp != 0 {
        out.push(format!("[ok]{:+} max HP[/]", b.hp));
    }
    if b.sp != 0 {
        out.push(format!("[ok]{:+} max SP[/]", b.sp));
    }
    if let Some((s, n)) = b.save {
        out.push(format!("[ok]{n:+} to {} saves[/]", save_name(s)));
    }
}

fn dice_text(d: Dice) -> String {
    if d.bonus == 0 { format!("{}d{}", d.count, d.sides) } else { format!("{}d{}{:+}", d.count, d.sides, d.bonus) }
}

fn describe_item(i: &Item) -> String {
    let mut lines = Vec::new();
    match i.kind {
        Kind::Consumable(Effect::Heal(d)) => lines.push(format!("Heals **{}** HP over {} s", dice_text(d), POTION_REGEN_MS / 1000)),
        Kind::Consumable(Effect::Spirit(d)) => lines.push(format!("Restores **{}** SP over {} s", dice_text(d), POTION_REGEN_MS / 1000)),
        Kind::Consumable(Effect::Cure) => lines.push("Cures **poison** and **blindness**".into()),
        Kind::Consumable(Effect::Antidote) => lines.push("Cures **poison**".into()),
        Kind::Armour(w, base) => lines.push(match w {
            Weight::Light => format!("**AC {base}** + class defence (light)"),
            Weight::Medium => format!("**AC {base}** + class defence, max +2 (medium)"),
            Weight::Heavy => format!("**AC {base}** (heavy)"),
        }),
        Kind::Shield => lines.push("Shield · [dim]not with a two-handed weapon[/]".into()),
        _ => {}
    }
    bonus_lines(&i.bonus, &mut lines);
    if matches!(i.kind, Kind::Consumable(_)) {
        lines.push("[dim]Drink on a mission: R / T, the HP / SP orb, or double-click. A stronger potion replaces a running one; a weaker one is kept.[/]".into());
    }
    lines.push(format!("[dim]{}[/]", i.flavour));
    lines.join("\n")
}

fn describe_weapon(w: &Weapon) -> String {
    let kind = match w.physical {
        Some(p) => p.label(),
        None => "magic",
    };
    let die = dice_text(w.die);
    let reach = match w.reach {
        super::weapon::Reach::Melee { range, .. } => format!("melee, {:.1} m", range * 0.5),
        super::weapon::Reach::Ranged { range, .. } => format!("ranged, {:.0} m", range * 0.5),
    };
    let mut lines = vec![
        format!("**{die} {kind}**"),
        format!("{} · {reach} · {:.1} attacks/s", w.kind.label(), 1000.0 / w.interval_ms as f32),
    ];
    if w.enchantment() > 0 {
        lines.push(format!("[ok]+{} to hit and damage[/] (weapon level {})", w.enchantment(), w.level));
    }
    if w.kind.two_handed() {
        lines.push("[dim]Two-handed: no shield[/]".into());
    }
    lines.join("\n")
}

/// Every item, weapons included, in shop order.
pub fn catalogue() -> &'static [Entry] {
    static ALL: LazyLock<Vec<Entry>> = LazyLock::new(|| {
        let mut v: Vec<Entry> = ITEMS
            .iter()
            .map(|i| Entry {
                id: i.id,
                name: i.name,
                kind: i.kind,
                price: i.price,
                classes: i.classes,
                bonus: i.bonus,
                weapon: None,
                desc: describe_item(i),
            })
            .collect();
        for w in WEAPONS {
            let classes = Class::ALL.iter().filter(|c| c.can_hold(w.kind)).fold(0, |m, &c| m | class_bit(c));
            v.push(Entry {
                id: w.id,
                name: w.name,
                kind: Kind::Weapon,
                price: w.price,
                classes,
                bonus: Bonus::default(),
                weapon: Some(w),
                desc: describe_weapon(w),
            });
        }
        v
    });
    &ALL
}

pub fn find(id: &str) -> Option<&'static Entry> {
    catalogue().iter().find(|e| e.id == id)
}

/// Zeny a kill drops for everyone in the party.
pub const fn kill_zeny(ml: u32, archetype: super::Archetype) -> u64 {
    let factor = match archetype {
        super::Archetype::Swarm | super::Archetype::Chaser => 1,
        super::Archetype::Tank => 3,
        super::Archetype::Elite => 10,
        super::Archetype::Boss => 200,
        super::Archetype::Plant => 0,
    };
    (ml as u64 + 1) * factor
}

/// Zeny for clearing a mission at `level`.
pub const fn victory_zeny(level: u32) -> u64 {
    250 + 25 * level as u64
}

/// Most stacks a bag holds and items per stack.
pub const MAX_STACKS: usize = 100;
pub const MAX_STACK: u32 = 30_000;
pub const MAX_ZENY: u64 = 1_000_000_000;

/// Highest refine level an item can carry (RO's +10).
pub const MAX_REFINE: u8 = 10;
/// Card slots an item can carry.
pub const MAX_CARDS: usize = 4;

/// What makes one copy of an item differ from another: refine level and
/// compounded cards (card item ids, kept sorted: the order they went in
/// does not change what the item does). Plain catalogue items have none;
/// only equipment carries them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Attrs {
    #[serde(default, rename = "r", skip_serializing_if = "is_zero")]
    pub refine: u8,
    #[serde(default, rename = "c", skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<String>,
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

impl Attrs {
    pub fn is_plain(&self) -> bool {
        self.refine == 0 && self.cards.is_empty()
    }

    /// The canonical form: refine capped, cards sorted, at most
    /// [`MAX_CARDS`]; `None` when a card id is not a plain id.
    fn canonical(mut self) -> Option<Attrs> {
        if self.cards.len() > MAX_CARDS || self.cards.iter().any(|c| !plain_id(c)) {
            return None;
        }
        self.refine = self.refine.min(MAX_REFINE);
        self.cards.sort_unstable();
        Some(self)
    }
}

/// `[a-z0-9_]+`: what catalogue ids (and card ids) look like.
fn plain_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The instance key of an item: its id, then `+<refine>` when refined,
/// then `[<card>,<card>]` when it has cards (`sword`, `sword+7`,
/// `sword+7[poring_card]`). Two copies stack exactly when their keys are
/// equal; the wire and every bag operation name a stack by it, so a plain
/// item's key is just its id (what the protocol always carried).
pub fn key(id: &str, attrs: &Attrs) -> String {
    let mut k = id.to_string();
    if attrs.refine > 0 {
        k.push('+');
        k.push_str(&attrs.refine.to_string());
    }
    if !attrs.cards.is_empty() {
        k.push('[');
        k.push_str(&attrs.cards.join(","));
        k.push(']');
    }
    k
}

/// Splits an instance key into the catalogue entry and its attributes;
/// `None` for an unknown item, a malformed key, or attributes on an item
/// that cannot carry them (only equipment does).
pub fn parse_key(key: &str) -> Option<(&'static Entry, Attrs)> {
    let end = key.find(['+', '[']).unwrap_or(key.len());
    let e = find(&key[..end])?;
    let mut rest = &key[end..];
    let mut attrs = Attrs::default();
    if let Some(r) = rest.strip_prefix('+') {
        let digits = r.find('[').unwrap_or(r.len());
        attrs.refine = r[..digits].parse().ok().filter(|n| (1..=MAX_REFINE).contains(n))?;
        rest = &r[digits..];
    }
    if !rest.is_empty() {
        let cards = rest.strip_prefix('[')?.strip_suffix(']')?;
        attrs.cards = cards.split(',').map(String::from).collect();
    }
    let (e, attrs) = instance(e.id, attrs)?;
    // Canonical spelling only: one key per instance.
    (attrs.is_plain() || key == self::key(e.id, &attrs)).then_some((e, attrs))
}

/// A catalogue item with canonical attributes, or `None` (unknown item,
/// bad cards, attributes on something that is not equipment).
fn instance(id: &str, attrs: Attrs) -> Option<(&'static Entry, Attrs)> {
    let e = find(id)?;
    let attrs = attrs.canonical()?;
    (attrs.is_plain() || e.slot().is_some()).then_some((e, attrs))
}

/// The catalogue id of an instance key (`sword+7` → `sword`); the key
/// itself when malformed.
pub fn base_id(key: &str) -> &str {
    &key[..key.find(['+', '[']).unwrap_or(key.len())]
}

/// Copies of one item instance: a catalogue id, its per-instance
/// attributes and how many. Stored as `{"id","n","a"?}` in
/// the `inventory` column (`a` only when the item has attributes).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    pub id: String,
    #[serde(rename = "n")]
    pub count: u32,
    #[serde(default, rename = "a", skip_serializing_if = "Attrs::is_plain")]
    pub attrs: Attrs,
}

impl Stack {
    /// The instance key (`key`): what the wire calls this stack.
    pub fn key(&self) -> String {
        key(&self.id, &self.attrs)
    }

    fn is(&self, id: &str, attrs: &Attrs) -> bool {
        self.id == id && self.attrs == *attrs
    }
}

/// What a character owns: stacks in the bag, the ten equipment slots
/// (item ids, empty = nothing) and zeny.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bag {
    pub items: Vec<Stack>,
    pub equip: [String; EQUIP_SLOTS],
    pub zeny: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemError {
    Unknown,
    NotSoldHere,
    NoZeny,
    NotInBag,
    BagFull,
    WrongClass,
    TwoHanded,
    NotEquipment,
    CannotUnequip,
    NotUsable,
}

impl ItemError {
    pub const fn code(self) -> &'static str {
        match self {
            ItemError::Unknown => "unknown_item",
            ItemError::NotSoldHere => "not_sold_here",
            ItemError::NoZeny => "not_enough_zeny",
            ItemError::NotInBag => "not_in_bag",
            ItemError::BagFull => "bag_full",
            ItemError::WrongClass => "wrong_class",
            ItemError::TwoHanded => "two_handed",
            ItemError::NotEquipment => "not_equipment",
            ItemError::CannotUnequip => "cannot_unequip",
            ItemError::NotUsable => "not_usable",
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            ItemError::Unknown => "No such item.",
            ItemError::NotSoldHere => "This merchant does not sell that.",
            ItemError::NoZeny => "Not enough zeny.",
            ItemError::NotInBag => "You do not have that.",
            ItemError::BagFull => "Your bag is full.",
            ItemError::WrongClass => "Your class cannot use that.",
            ItemError::TwoHanded => "Not with a two-handed weapon.",
            ItemError::NotEquipment => "That cannot be worn.",
            ItemError::CannotUnequip => "You need a weapon to fight with.",
            ItemError::NotUsable => "That cannot be used.",
        }
    }
}

/// What the worn items add up to (the level-independent part of the
/// fighter's numbers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gear {
    /// Armour class before the tier bonus.
    pub ac: i32,
    pub hp: i32,
    pub sp: i32,
    /// Bonus per [`Save`] (Body, Reflex, Mind).
    pub saves: [i32; 3],
}

impl Class {
    /// Class defence: the Dexterity modifier nobody rolls for, added to
    /// light armour in full and to medium up to +2.
    pub const fn defence(self) -> i32 {
        match self {
            Class::Swordsman | Class::Archer => 3,
            Class::Acolyte => 2,
            Class::Mage => 0,
        }
    }

    /// Armour the class starts in.
    pub const fn starting_kit(self) -> &'static [&'static str] {
        match self {
            Class::Swordsman => &["cotton_shirt", "guard"],
            _ => &["cotton_shirt"],
        }
    }
}

impl Bag {
    /// A new character's belongings: the class's weapon (or `weapon`, the
    /// old per-character column, when its class may hold it) and armour,
    /// five Red Potions, a little zeny.
    pub fn starting(class: Class, weapon: &str) -> Bag {
        let mut bag = Bag { zeny: 100, ..Bag::default() };
        bag.equip[Slot::Weapon as usize] = Weapon::held(class, weapon).id.into();
        let two_handed = Weapon::held(class, weapon).kind.two_handed();
        for id in class.starting_kit() {
            if let Some(slot) = find(id).and_then(Entry::slot).filter(|&s| !(two_handed && s == Slot::Shield)) {
                bag.equip[slot as usize] = (*id).into();
            }
        }
        bag.items.push(Stack { id: "red_potion".into(), count: 5, ..Stack::default() });
        bag
    }

    /// From the stored form (the `inventory` and `equipment` JSONB
    /// columns); no `equip` (a character never given items) gets the
    /// starting kit.
    pub fn load(class: Class, items: Vec<Stack>, equip: Option<Vec<String>>, zeny: i64, weapon: &str) -> Bag {
        let Some(equip) = equip.filter(|e| e.len() == EQUIP_SLOTS) else {
            return Bag::starting(class, weapon);
        };
        let mut bag = Bag { zeny: zeny.clamp(0, MAX_ZENY as i64) as u64, ..Bag::default() };
        // Rows from before instance keys (or hand-edited ones) may hold the
        // same instance more than once: they merge into one stack, capped
        // at MAX_STACK.
        for s in items {
            let Some((e, attrs)) = instance(&s.id, s.attrs) else { continue };
            if s.count == 0 {
                continue;
            }
            match bag.items.iter().position(|o| o.is(e.id, &attrs)) {
                Some(i) => bag.items[i].count = (bag.items[i].count.saturating_add(s.count)).min(MAX_STACK),
                None if bag.items.len() < MAX_STACKS => bag.items.push(Stack { id: e.id.into(), count: s.count.min(MAX_STACK), attrs }),
                None => {}
            }
        }
        for (i, k) in equip.into_iter().enumerate() {
            let fits = parse_key(&k)
                .and_then(|(e, _)| e.slot())
                .is_some_and(|s| s == Slot::ALL[i] || (s == Slot::Accessory1 && i == Slot::Accessory2 as usize));
            if fits {
                bag.equip[i] = k;
            }
        }
        if bag.equip[Slot::Weapon as usize].is_empty() {
            bag.equip[Slot::Weapon as usize] = Weapon::held(class, weapon).id.into();
        }
        bag
    }

    /// How many of the instance `key` the bag holds (worn items are not
    /// in it).
    pub fn count(&self, key: &str) -> u32 {
        let Some((e, attrs)) = parse_key(key) else { return 0 };
        self.items.iter().find(|s| s.is(e.id, &attrs)).map_or(0, |s| s.count)
    }

    /// Adds `n` of the instance `key`: onto its stack when there is one,
    /// else as a new stack. Bags have no weight, only [`MAX_STACKS`]
    /// stacks of at most [`MAX_STACK`].
    pub fn add(&mut self, key: &str, n: u32) -> Result<(), ItemError> {
        let (e, attrs) = parse_key(key).ok_or(ItemError::Unknown)?;
        match self.items.iter().position(|s| s.is(e.id, &attrs)) {
            Some(i) if self.items[i].count + n > MAX_STACK => Err(ItemError::BagFull),
            Some(i) => {
                self.items[i].count += n;
                Ok(())
            }
            None if self.items.len() >= MAX_STACKS => Err(ItemError::BagFull),
            None => {
                self.items.push(Stack { id: e.id.into(), count: n, attrs });
                Ok(())
            }
        }
    }

    /// Takes `n` of the instance `key` off its stack (the stack goes when
    /// it empties).
    pub fn take(&mut self, key: &str, n: u32) -> Result<(), ItemError> {
        let (e, attrs) = parse_key(key).ok_or(ItemError::NotInBag)?;
        let i = self.items.iter().position(|s| s.is(e.id, &attrs)).ok_or(ItemError::NotInBag)?;
        if self.items[i].count < n || n == 0 {
            return Err(ItemError::NotInBag);
        }
        self.items[i].count -= n;
        if self.items[i].count == 0 {
            self.items.remove(i);
        }
        Ok(())
    }

    /// Buys `n` of `id` from a merchant selling `stock`.
    pub fn buy(&mut self, id: &str, n: u32, stock: &[&str]) -> Result<u64, ItemError> {
        let e = find(id).ok_or(ItemError::Unknown)?;
        if !stock.contains(&id) {
            return Err(ItemError::NotSoldHere);
        }
        let n = n.clamp(1, 999);
        let cost = e.price as u64 * n as u64;
        if cost > self.zeny {
            return Err(ItemError::NoZeny);
        }
        self.add(id, n)?;
        self.zeny -= cost;
        Ok(cost)
    }

    /// Sells `n` of the instance `id` from the bag (not what is worn) at
    /// half the catalogue price.
    pub fn sell(&mut self, id: &str, n: u32) -> Result<u64, ItemError> {
        let (e, _) = parse_key(id).ok_or(ItemError::Unknown)?;
        let n = n.max(1);
        self.take(id, n)?;
        let gain = sell_price(e.price) as u64 * n as u64;
        self.zeny = (self.zeny + gain).min(MAX_ZENY);
        Ok(gain)
    }

    /// Adds zeny (kills, mission bonus).
    pub fn earn(&mut self, zeny: u64) {
        self.zeny = (self.zeny + zeny).min(MAX_ZENY);
    }

    /// Wears one of the instance `id` from the bag (one off its stack);
    /// what was in its slot goes back to the bag, onto its own stack.
    /// Accessories take the free one of the two slots (`hint` picks one);
    /// a two-handed weapon puts the shield away. Slots hold instance keys.
    pub fn equip(&mut self, class: Class, id: &str, hint: Option<Slot>) -> Result<Slot, ItemError> {
        let (e, _) = parse_key(id).ok_or(ItemError::Unknown)?;
        let mut slot = e.slot().ok_or(ItemError::NotEquipment)?;
        if !e.usable_by(class) {
            return Err(ItemError::WrongClass);
        }
        if self.count(id) == 0 {
            return Err(ItemError::NotInBag);
        }
        if slot == Slot::Accessory1 {
            slot = match hint {
                Some(h @ (Slot::Accessory1 | Slot::Accessory2)) => h,
                _ if self.equip[Slot::Accessory1 as usize].is_empty() => Slot::Accessory1,
                _ if self.equip[Slot::Accessory2 as usize].is_empty() => Slot::Accessory2,
                _ => Slot::Accessory1,
            };
        }
        if slot == Slot::Shield && find(self.weapon()).is_some_and(Entry::two_handed) {
            return Err(ItemError::TwoHanded);
        }
        // On a copy: what comes off may not fit in the bag.
        let mut next = self.clone();
        next.take(id, 1)?;
        if e.two_handed() && !next.equip[Slot::Shield as usize].is_empty() {
            let s = std::mem::take(&mut next.equip[Slot::Shield as usize]);
            next.add(&s, 1)?;
        }
        let old = std::mem::replace(&mut next.equip[slot as usize], id.into());
        if !old.is_empty() {
            next.add(&old, 1)?;
        }
        *self = next;
        Ok(slot)
    }

    /// Takes off what is in `slot` (the weapon stays: nobody fights bare-handed).
    pub fn unequip(&mut self, slot: Slot) -> Result<(), ItemError> {
        if slot == Slot::Weapon {
            return Err(ItemError::CannotUnequip);
        }
        let id = self.equip[slot as usize].clone();
        if id.is_empty() {
            return Err(ItemError::NotInBag);
        }
        self.add(&id, 1)?;
        self.equip[slot as usize].clear();
        Ok(())
    }

    /// Worn weapon's catalogue id (the slot holds its instance key).
    pub fn weapon(&self) -> &str {
        base_id(&self.equip[Slot::Weapon as usize])
    }

    /// What the worn items add up to for `class` (items the class may not
    /// wear, from hand-edited rows, count for nothing).
    pub fn gear(&self, class: Class) -> Gear {
        let d = class.defence();
        // Unarmoured: 10 + defence.
        let mut body = 10 + d;
        let mut g = Gear { ac: 0, hp: 0, sp: 0, saves: [0; 3] };
        for k in &self.equip {
            let Some(e) = find(base_id(k)).filter(|e| e.usable_by(class)) else { continue };
            if let Kind::Armour(w, base) = e.kind {
                body = base as i32
                    + match w {
                        Weight::Light => d,
                        Weight::Medium => d.min(2),
                        Weight::Heavy => 0,
                    };
            }
            g.ac += e.bonus.ac as i32;
            g.hp += e.bonus.hp as i32;
            g.sp += e.bonus.sp as i32;
            if let Some((s, n)) = e.bonus.save {
                g.saves[s as usize] += n as i32;
            }
        }
        g.ac += body;
        g
    }
}

impl Gear {
    /// Armour class at `level`: the gear plus one per tier above the first.
    pub const fn ac_at(&self, level: u32) -> i32 {
        self.ac + super::tier(level) as i32 - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`Bag::load`] from the columns' JSON text; an unparsable `equip`
    /// stands for a NULL column.
    fn load_json(class: Class, items: &str, equip: &str, zeny: i64, weapon: &str) -> Bag {
        Bag::load(class, serde_json::from_str(items).unwrap(), serde_json::from_str(equip).ok(), zeny, weapon)
    }

    #[test]
    fn catalogue_is_sane() {
        let all = catalogue();
        for (i, e) in all.iter().enumerate() {
            assert!(all[..i].iter().all(|o| o.id != e.id), "duplicate id {}", e.id);
            assert!(e.price > 0, "{} has a price", e.id);
            assert!(!e.desc.is_empty());
            if e.slot().is_some() {
                assert!(e.classes != 0, "{} is wearable by someone", e.id);
            }
        }
        for (shop, stock) in SHOPS {
            for id in *stock {
                assert!(find(id).is_some(), "{shop} sells unknown {id}");
            }
        }
        assert!(find("sword").unwrap().desc.contains("1d8 slashing"));
        assert!(find("red_potion").unwrap().desc.contains("2d4+2"));
    }

    /// The starting kit gives the AC each class had before items
    /// (COMBAT.md §2.2): 16 / 14 / 13 / 11, +1 per tier.
    #[test]
    fn starting_kit_keeps_the_class_armour() {
        for (c, want) in [(Class::Swordsman, 16), (Class::Archer, 14), (Class::Acolyte, 13), (Class::Mage, 11)] {
            let bag = Bag::starting(c, "");
            assert_eq!(bag.gear(c).ac_at(1), want, "{c:?}");
            assert_eq!(bag.gear(c).ac_at(99), want + 2, "{c:?}");
            assert_eq!(bag.weapon(), c.starting_weapon());
            assert_eq!(bag.count("red_potion"), 5);
        }
        assert_eq!(Bag::starting(Class::Swordsman, "katana").weapon(), "katana");
    }

    #[test]
    fn armour_class_follows_weight_and_stays_in_budget() {
        let wear = |c: Class, ids: &[&str]| {
            let mut b = Bag::starting(c, "");
            b.zeny = MAX_ZENY;
            for id in ids {
                b.add(id, 1).unwrap();
                b.equip(c, id, None).unwrap();
            }
            b.gear(c)
        };
        // Medium caps the class defence at +2; heavy ignores it.
        assert_eq!(wear(Class::Archer, &["padded_armor"]).ac, 14 + 2);
        assert_eq!(wear(Class::Archer, &["tights"]).ac, 12 + 3);
        assert_eq!(wear(Class::Swordsman, &["full_plate"]).ac, 18 + 2);
        // The best a swordsman can wear, at 99: within COMBAT.md's 10–24.
        let best = wear(Class::Swordsman, &["full_plate", "shield", "helm", "manteau"]);
        assert_eq!(best.ac_at(99), 24);
        assert_eq!(best.hp, 25);
        // The barest mage: no armour at all.
        let mut bare = Bag::starting(Class::Mage, "");
        bare.unequip(Slot::Armour).unwrap();
        assert_eq!(bare.gear(Class::Mage).ac, 10);
        assert_eq!(bare.count("cotton_shirt"), 1);
    }

    #[test]
    fn buying_and_selling() {
        let mut b = Bag::starting(Class::Mage, "");
        let tool = shop("tool").unwrap();
        assert_eq!(b.buy("red_potion", 2, tool), Ok(100));
        assert_eq!((b.zeny, b.count("red_potion")), (0, 7));
        assert_eq!(b.buy("red_potion", 1, tool), Err(ItemError::NoZeny));
        assert_eq!(b.buy("sword", 1, tool), Err(ItemError::NotSoldHere));
        assert_eq!(b.buy("nope", 1, tool), Err(ItemError::Unknown));
        assert_eq!(b.sell("red_potion", 7), Ok(175));
        assert_eq!((b.zeny, b.count("red_potion")), (175, 0));
        assert_eq!(b.sell("red_potion", 1), Err(ItemError::NotInBag));
        // What is worn is not in the bag.
        assert_eq!(b.sell("rod", 1), Err(ItemError::NotInBag));
    }

    #[test]
    fn equipping_swaps_checks_class_and_hands() {
        let mut b = Bag::starting(Class::Swordsman, "");
        b.zeny = MAX_ZENY;
        let weapons = shop("weapon").unwrap();
        b.buy("katana", 1, weapons).unwrap();
        b.buy("rod", 1, weapons).unwrap();
        assert_eq!(b.equip(Class::Swordsman, "rod", None), Err(ItemError::WrongClass));
        // A two-hander puts the shield away, and the shield then won't go on.
        assert_eq!(b.equip(Class::Swordsman, "katana", None), Ok(Slot::Weapon));
        assert_eq!((b.count("sword"), b.count("guard"), b.weapon()), (1, 1, "katana"));
        assert!(b.equip[Slot::Shield as usize].is_empty());
        assert_eq!(b.equip(Class::Swordsman, "guard", None), Err(ItemError::TwoHanded));
        assert_eq!(b.gear(Class::Swordsman).ac, 14);
        assert_eq!(b.unequip(Slot::Weapon), Err(ItemError::CannotUnequip));
        // Accessories fill both slots.
        let armor = shop("armor").unwrap();
        b.buy("clip", 2, armor).unwrap();
        assert_eq!(b.equip(Class::Swordsman, "clip", None), Ok(Slot::Accessory1));
        assert_eq!(b.equip(Class::Swordsman, "clip", None), Ok(Slot::Accessory2));
        assert_eq!(b.gear(Class::Swordsman).sp, 20);
        assert_eq!(b.equip(Class::Swordsman, "clip", None), Err(ItemError::NotInBag));
        assert_eq!(b.equip(Class::Swordsman, "red_potion", None), Err(ItemError::NotEquipment));
    }

    #[test]
    fn stored_bags_round_trip_and_old_rows_get_the_kit() {
        let mut b = Bag::starting(Class::Archer, "");
        b.earn(1234);
        let back = load_json(Class::Archer, &serde_json::to_string(&b.items).unwrap(), &serde_json::to_string(&b.equip).unwrap(), b.zeny as i64, "");
        assert_eq!(back, b);
        // Never given items: '[]' and a NULL equipment column.
        let old = load_json(Class::Swordsman, "[]", "", 0, "katana");
        assert_eq!(old, Bag::starting(Class::Swordsman, "katana"));
        // Unknown ids and misplaced items are dropped.
        let odd = load_json(Class::Mage, r#"[{"id":"x","n":3},{"id":"red_potion","n":2}]"#, r#"["sword","","","","","","","","",""]"#, -5, "");
        assert_eq!((odd.items.len(), odd.equip[0].as_str(), odd.weapon(), odd.zeny), (1, "", "rod", 0));
    }

    #[test]
    fn instance_keys_are_canonical() {
        let refined = Attrs { refine: 7, cards: vec!["b_card".into(), "a_card".into()] };
        let k = key("sword", &refined.clone().canonical().unwrap());
        assert_eq!(k, "sword+7[a_card,b_card]");
        let (e, a) = parse_key(&k).unwrap();
        assert_eq!((e.id, a.refine, a.cards.len()), ("sword", 7, 2));
        assert_eq!(parse_key("sword").map(|(e, a)| (e.id, a.is_plain())), Some(("sword", true)));
        assert_eq!(base_id(&k), "sword");
        // One spelling per instance; attributes only on equipment.
        for bad in ["sword+0", "sword+07", "sword+11", "sword[b_card,a_card]", "sword+7[]", "sword+7[A]", "red_potion+1", "nope+1", "sword[a,b,c,d,e]"] {
            assert!(parse_key(bad).is_none(), "{bad}");
        }
        assert!(parse_key("sword[a,b,c,d]").is_some());
    }

    /// Same id and same attributes: one stack with a count; anything that
    /// differs (refine, cards): its own stack.
    #[test]
    fn identical_instances_stack() {
        let mut b = Bag::default();
        b.add("sword", 2).unwrap();
        b.add("sword", 1).unwrap();
        b.add("sword+3", 1).unwrap();
        b.add("sword+3", 1).unwrap();
        b.add("sword+3[x_card]", 1).unwrap();
        assert_eq!(b.items.len(), 3);
        assert_eq!((b.count("sword"), b.count("sword+3"), b.count("sword+3[x_card]"), b.count("sword+4")), (3, 2, 1, 0));
        assert_eq!(b.add("sword+3[X]", 1), Err(ItemError::Unknown));
        b.take("sword+3", 2).unwrap();
        assert_eq!((b.items.len(), b.count("sword")), (2, 3));
        assert_eq!(b.take("sword+3", 1), Err(ItemError::NotInBag));
        // No weight: only the stack count limits a bag.
        b.add("full_plate", 1000).unwrap();
        assert_eq!(b.count("full_plate"), 1000);
    }

    /// Wearing takes one off its stack; taking off puts it back on the
    /// same instance's stack, never onto a plain one.
    #[test]
    fn equipping_from_a_stack() {
        let mut b = Bag::starting(Class::Swordsman, "");
        b.add("helm", 3).unwrap();
        b.add("helm+5", 2).unwrap();
        assert_eq!(b.equip(Class::Swordsman, "helm+5", None), Ok(Slot::UpperHead));
        assert_eq!((b.count("helm"), b.count("helm+5")), (3, 1));
        assert_eq!(b.equip[Slot::UpperHead as usize], "helm+5");
        assert_eq!(b.gear(Class::Swordsman), {
            let mut plain = Bag::starting(Class::Swordsman, "");
            plain.add("helm", 1).unwrap();
            plain.equip(Class::Swordsman, "helm", None).unwrap();
            plain.gear(Class::Swordsman)
        });
        // Swapping to a plain helm returns the +5 to its stack.
        b.equip(Class::Swordsman, "helm", None).unwrap();
        assert_eq!((b.count("helm"), b.count("helm+5")), (2, 2));
        b.unequip(Slot::UpperHead).unwrap();
        assert_eq!((b.count("helm"), b.count("helm+5"), b.items.len()), (3, 2, 3));
        // A refined weapon is still the weapon the fighter holds.
        b.add("katana+2", 1).unwrap();
        b.equip(Class::Swordsman, "katana+2", None).unwrap();
        assert_eq!((b.weapon(), b.equip[Slot::Weapon as usize].as_str()), ("katana", "katana+2"));
    }

    /// The stored form keeps attributes; old rows with the same item in
    /// several entries merge into one stack on load.
    #[test]
    fn stored_instances_round_trip_and_duplicates_merge() {
        let mut b = Bag::starting(Class::Swordsman, "");
        b.add("helm+5[a_card]", 2).unwrap();
        b.add("helm", 1).unwrap();
        b.equip(Class::Swordsman, "helm+5[a_card]", None).unwrap();
        let json = serde_json::to_string(&b.items).unwrap();
        assert!(json.contains(r#"{"id":"helm","n":1}"#), "plain rows keep the old form: {json}");
        assert!(json.contains(r#""a":{"r":5,"c":["a_card"]}"#), "{json}");
        let back = load_json(Class::Swordsman, &json, &serde_json::to_string(&b.equip).unwrap(), b.zeny as i64, "");
        assert_eq!(back, b);
        let old = r#"[{"id":"red_potion","n":2},{"id":"helm","n":1},{"id":"red_potion","n":3},{"id":"helm","n":1,"a":{"r":2}},{"id":"helm","n":4,"a":{}},{"id":"helm","n":1,"a":{"r":2,"c":[]}},{"id":"red_potion","n":29999}]"#;
        let merged = load_json(Class::Swordsman, old, &serde_json::to_string(&b.equip).unwrap(), 0, "");
        assert_eq!(merged.items.len(), 3);
        assert_eq!((merged.count("red_potion"), merged.count("helm"), merged.count("helm+2")), (MAX_STACK, 5, 2));
        // Cards in another order are the same instance; attributes on a
        // potion are dropped as a bad row.
        let cards = r#"[{"id":"helm","n":1,"a":{"c":["b_card","a_card"]}},{"id":"helm","n":1,"a":{"c":["a_card","b_card"]}},{"id":"red_potion","n":1,"a":{"r":1}}]"#;
        let merged = load_json(Class::Swordsman, cards, &serde_json::to_string(&b.equip).unwrap(), 0, "");
        assert_eq!((merged.items.len(), merged.count("helm[a_card,b_card]")), (1, 2));
    }

    #[test]
    fn zeny_scales_with_the_monster() {
        use super::super::Archetype;
        assert_eq!(kill_zeny(1, Archetype::Swarm), 2);
        assert!(kill_zeny(25, Archetype::Boss) > 100 * kill_zeny(25, Archetype::Swarm));
        assert_eq!(victory_zeny(10), 500);
    }
}
