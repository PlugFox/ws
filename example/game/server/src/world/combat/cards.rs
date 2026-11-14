//! Monster cards (COMBAT.md §2.6.1c): one per monster, dropped only by
//! it, socketed into equipment of one kind. A card adds a prefix or a
//! suffix to the item's name and an effect while the item is worn.
//!
//! Slots are a property of the catalogue item ([`slots`]): plain gear has
//! more of them than magic gear, low weapon levels more than high ones,
//! so a fully carded common build is a real alternative to drops.
//!
//! What a card does is data: plain [`Mods`] (summed like a magic item's)
//! and [`Procs`], a fixed-size `Copy` struct of small numbers that the
//! stage reads at the few places it already branches (a landed basic
//! attack, a kill, a blow taken, the per-player tick, a cast). Nothing
//! iterates cards or allocates during a tick: [`Bag::gear`] sums the worn
//! cards once, when the fighter puts the gear on.
//!
//! [`Bag::gear`]: super::items::Bag::gear

use super::buffs::Mods;
use super::conditions::Condition;
use super::items::{Entry, Kind, Slot};
use super::magic::Rarity;
use super::rules::DamageType;
use super::Size;

/// The equipment a card fits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardSlot {
    Head,
    Body,
    Weapon,
    Shield,
    Garment,
    Shoes,
    Accessory,
}

impl CardSlot {
    /// Wire code (1..=7; 0 = not a card).
    pub const fn code(self) -> u8 {
        self as u8 + 1
    }

    pub const fn label(self) -> &'static str {
        match self {
            CardSlot::Head => "headgear",
            CardSlot::Body => "armor",
            CardSlot::Weapon => "weapon",
            CardSlot::Shield => "shield",
            CardSlot::Garment => "garment",
            CardSlot::Shoes => "shoes",
            CardSlot::Accessory => "accessory",
        }
    }

    /// The card slot of a piece of equipment.
    pub fn of(kind: Kind) -> Option<CardSlot> {
        Some(match kind {
            Kind::Weapon => CardSlot::Weapon,
            Kind::Armour(..) => CardSlot::Body,
            Kind::Shield => CardSlot::Shield,
            Kind::Headgear(_) => CardSlot::Head,
            Kind::Garment => CardSlot::Garment,
            Kind::Shoes => CardSlot::Shoes,
            Kind::Accessory => CardSlot::Accessory,
            _ => return None,
        })
    }
}

/// Where a card's word goes in the item's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Affix {
    /// Before the base name: "Stinging Sword".
    Prefix(&'static str),
    /// After it: "Sword of Kindling".
    Suffix(&'static str),
}

/// Card families: sets count the worn cards of one.
pub mod family {
    pub const NONE: u8 = 0;
    pub const SLIME: u8 = 1;
    pub const THIEF_BUG: u8 = 2;
    pub const DESERT_PACK: u8 = 3;
    pub const RESTLESS_DEAD: u8 = 4;
}

/// Effects beyond [`Mods`], read by the stage at its existing branches.
/// Every field is a small number or a flag, 0 = none; a fighter without
/// cards has `Procs::NONE` and rolls no extra dice.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Procs {
    // --- A landed basic attack ---------------------------------------
    /// Percent chance a basic attack sets the monster Burning (3 s).
    pub ignite: u8,
    /// Percent chance a basic attack Slows the monster (3 s).
    pub chill: u8,
    /// Poison from your attacks lasts twice as long.
    pub venom_long: bool,
    /// Flat damage against small / medium / large monsters.
    pub size_bane: [i8; 3],
    /// Flat damage against a monster under any condition.
    pub afflicted_bane: i8,
    /// Flat damage against a Stunned, Frozen, Rooted or Slowed monster.
    pub held_bane: i8,
    /// Flat damage against elites, bosses and starred monsters.
    pub elite_bane: i8,
    /// Extra die (sides) against a monster at full HP.
    pub fresh: u8,
    /// Extra die (sides) on a ranged critical hit.
    pub crit_die: u8,
    /// Percent chance a basic attack heals you 1d4.
    pub leech: u8,
    /// Basic attacks on a monster under 20% HP are critical hits.
    pub cull: bool,
    /// A critical hit leaves the monster Poisoned and Slowed (4 s).
    pub crit_sting: bool,
    /// A critical hit knocks loose zeny (the monster's level).
    pub crit_zeny: bool,
    /// Basic attacks deal this damage type instead (a weapon element).
    pub endow: Option<DamageType>,
    /// Ranged basic attacks reach this many cells farther.
    pub reach: u8,
    /// After a block or a dodge, the next basic attack deals +Nd6.
    pub counter: u8,
    /// +1 damage per 25 kills this expedition, up to +N (downed: reset).
    pub growth: u8,
    /// +N to hit and damage while an ally stands within 5 cells.
    pub pack: i8,
    // --- A kill ------------------------------------------------------
    /// The kill combo window lasts this many seconds longer.
    pub combo_secs: u8,
    /// Percent more zeny from your kills.
    pub zeny_pct: u8,
    /// Percent more zeny from elites, bosses and starred monsters.
    pub elite_zeny: u8,
    /// Every Nth kill restores 25% HP and SP.
    pub feast: u8,
    // --- A blow taken ------------------------------------------------
    /// Percent chance to dodge a melee blow outright.
    pub dodge: u8,
    /// A dodge gives +20% walking speed for 2 s.
    pub dodge_rush: bool,
    /// Percent chance a monster striking you in melee is Slowed (3 s).
    pub slow_back: u8,
    /// Percent chance a monster striking you in melee is Blinded (4 s).
    pub blind_back: u8,
    /// Melee attackers take 1dN (never fatal).
    pub thorns: u8,
    /// A blocked blow deals 1dN back (never fatal).
    pub riposte: u8,
    /// Conditions on you end this percent sooner.
    pub cond_short: u8,
    /// Conditions that cannot take hold (a [`Condition::bit`] mask).
    pub immune: u16,
    /// After standing still 2 s: +N AC, not stunned or frozen.
    pub still_ac: i8,
    /// Under 30% HP: +N AC, not feared.
    pub desperate_ac: i8,
    /// No single blow takes more than this percent of max HP.
    pub blow_cap: u8,
    /// Negates one melee blow every N seconds.
    pub veil: u8,
    /// Percent less non-physical damage; each point prevented is a zeny.
    pub gilded: u8,
    /// Once per expedition a fatal blow leaves 1 HP and 30% temp HP.
    pub last_stand: bool,
    /// Bleed out half as fast; revived at half HP.
    pub deathless: bool,
    // --- The tick ----------------------------------------------------
    /// Pickup bubbles are collected from this percent farther.
    pub magnet: u8,
    /// Percent more walking speed under half HP.
    pub flee_speed: u8,
    /// Percent more walking speed per style rank (D = 1 … SSS = 7).
    pub rank_speed: u8,
    /// Percent more walking speed while mounted.
    pub ride_speed: u8,
    /// After standing still 3 s: regenerate N tenths of a percent HP a second.
    pub still_regen: u8,
    /// A wisp casts Fire Bolt at the nearest monster every N seconds.
    pub wisp: u8,
    // --- Casts and potions -------------------------------------------
    /// Percent chance a skill does not go on cooldown.
    pub free_cd: u8,
    /// Every Nth skill cast costs no SP.
    pub free_every: u8,
    /// Every N seconds your next skill is free (no SP, no cooldown).
    pub moon: u8,
    /// Potion cooldown this percent shorter.
    pub potion_cd: u8,
    /// Potions also end Poisoned.
    pub spore_cure: bool,
    // --- Summed at wear time -----------------------------------------
    /// Thief Bug family cards worn give +2% attack and walking speed each.
    pub brood: bool,
    /// −1 physical damage per hit, −1 more at +5 and at +9 of its item.
    pub colossus: bool,
}

/// The bits of `conds`, for [`Procs::immune`].
const fn bits(conds: &[Condition]) -> u16 {
    let mut m = 0;
    let mut i = 0;
    while i < conds.len() {
        m |= conds[i].bit();
        i += 1;
    }
    m
}

impl Procs {
    pub const NONE: Procs = Procs {
        ignite: 0,
        chill: 0,
        venom_long: false,
        size_bane: [0; 3],
        afflicted_bane: 0,
        held_bane: 0,
        elite_bane: 0,
        fresh: 0,
        crit_die: 0,
        leech: 0,
        cull: false,
        crit_sting: false,
        crit_zeny: false,
        endow: None,
        reach: 0,
        counter: 0,
        growth: 0,
        pack: 0,
        combo_secs: 0,
        zeny_pct: 0,
        elite_zeny: 0,
        feast: 0,
        dodge: 0,
        dodge_rush: false,
        slow_back: 0,
        blind_back: 0,
        thorns: 0,
        riposte: 0,
        cond_short: 0,
        immune: 0,
        still_ac: 0,
        desperate_ac: 0,
        blow_cap: 0,
        veil: 0,
        gilded: 0,
        last_stand: false,
        deathless: false,
        magnet: 0,
        flee_speed: 0,
        rank_speed: 0,
        ride_speed: 0,
        still_regen: 0,
        wisp: 0,
        free_cd: 0,
        free_every: 0,
        moon: 0,
        potion_cd: 0,
        spore_cure: false,
        brood: false,
        colossus: false,
    };

    /// Adds another card's procs: chances and bonuses add, flags or,
    /// "every N" keeps the shorter, caps are applied by [`Procs::clamp`].
    pub fn add(&mut self, o: &Procs) {
        fn every(a: u8, b: u8) -> u8 {
            match (a, b) {
                (0, n) | (n, 0) => n,
                (a, b) => a.min(b),
            }
        }
        self.ignite = self.ignite.saturating_add(o.ignite);
        self.chill = self.chill.saturating_add(o.chill);
        self.venom_long |= o.venom_long;
        for (a, b) in self.size_bane.iter_mut().zip(o.size_bane) {
            *a = a.saturating_add(b);
        }
        self.afflicted_bane = self.afflicted_bane.saturating_add(o.afflicted_bane);
        self.held_bane = self.held_bane.saturating_add(o.held_bane);
        self.elite_bane = self.elite_bane.saturating_add(o.elite_bane);
        self.fresh = self.fresh.max(o.fresh);
        self.crit_die = self.crit_die.max(o.crit_die);
        self.leech = self.leech.saturating_add(o.leech);
        self.cull |= o.cull;
        self.crit_sting |= o.crit_sting;
        self.crit_zeny |= o.crit_zeny;
        self.endow = self.endow.or(o.endow);
        self.reach = self.reach.saturating_add(o.reach);
        self.counter = self.counter.saturating_add(o.counter);
        self.growth = self.growth.max(o.growth);
        self.pack = self.pack.saturating_add(o.pack);
        self.combo_secs = self.combo_secs.saturating_add(o.combo_secs);
        self.zeny_pct = self.zeny_pct.saturating_add(o.zeny_pct);
        self.elite_zeny = self.elite_zeny.saturating_add(o.elite_zeny);
        self.feast = every(self.feast, o.feast);
        self.dodge = self.dodge.saturating_add(o.dodge);
        self.dodge_rush |= o.dodge_rush;
        self.slow_back = self.slow_back.saturating_add(o.slow_back);
        self.blind_back = self.blind_back.saturating_add(o.blind_back);
        self.thorns = self.thorns.max(o.thorns);
        self.riposte = self.riposte.max(o.riposte);
        self.cond_short = self.cond_short.saturating_add(o.cond_short);
        self.immune |= o.immune;
        self.still_ac = self.still_ac.saturating_add(o.still_ac);
        self.desperate_ac = self.desperate_ac.saturating_add(o.desperate_ac);
        self.blow_cap = every(self.blow_cap, o.blow_cap);
        self.veil = every(self.veil, o.veil);
        self.gilded = self.gilded.saturating_add(o.gilded);
        self.last_stand |= o.last_stand;
        self.deathless |= o.deathless;
        self.magnet = self.magnet.saturating_add(o.magnet);
        self.flee_speed = self.flee_speed.saturating_add(o.flee_speed);
        self.rank_speed = self.rank_speed.saturating_add(o.rank_speed);
        self.ride_speed = self.ride_speed.saturating_add(o.ride_speed);
        self.still_regen = self.still_regen.saturating_add(o.still_regen);
        self.wisp = every(self.wisp, o.wisp);
        self.free_cd = self.free_cd.saturating_add(o.free_cd);
        self.free_every = every(self.free_every, o.free_every);
        self.moon = every(self.moon, o.moon);
        self.potion_cd = self.potion_cd.saturating_add(o.potion_cd);
        self.spore_cure |= o.spore_cure;
        self.brood |= o.brood;
        self.colossus |= o.colossus;
    }

    /// Caps on what stacked cards add up to (four of one card in four
    /// items must not break the bounded numbers).
    pub fn clamp(&mut self) {
        self.ignite = self.ignite.min(25);
        self.chill = self.chill.min(25);
        for b in &mut self.size_bane {
            *b = (*b).min(6);
        }
        self.afflicted_bane = self.afflicted_bane.min(6);
        self.held_bane = self.held_bane.min(6);
        self.elite_bane = self.elite_bane.min(6);
        self.leech = self.leech.min(20);
        self.reach = self.reach.min(2);
        self.counter = self.counter.min(4);
        self.pack = self.pack.min(2);
        self.combo_secs = self.combo_secs.min(3);
        self.zeny_pct = self.zeny_pct.min(60);
        self.elite_zeny = self.elite_zeny.min(90);
        self.dodge = self.dodge.min(20);
        self.slow_back = self.slow_back.min(50);
        self.blind_back = self.blind_back.min(50);
        self.cond_short = self.cond_short.min(60);
        self.still_ac = self.still_ac.min(4);
        self.desperate_ac = self.desperate_ac.min(5);
        self.gilded = self.gilded.min(40);
        self.magnet = self.magnet.min(150);
        self.flee_speed = self.flee_speed.min(25);
        self.rank_speed = self.rank_speed.min(4);
        self.ride_speed = self.ride_speed.min(25);
        self.still_regen = self.still_regen.min(30);
        self.free_cd = self.free_cd.min(25);
        self.potion_cd = self.potion_cd.min(50);
    }

    /// No card effect at all.
    pub fn is_none(&self) -> bool {
        *self == Procs::NONE
    }

    /// Flat damage against a monster of `size`.
    pub fn size_bonus(&self, size: Size) -> i32 {
        match size {
            Size::Small => self.size_bane[0] as i32,
            Size::Medium => self.size_bane[1] as i32,
            Size::Large | Size::Huge => self.size_bane[2] as i32,
        }
    }
}

/// One card.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Card {
    /// Catalogue id (`poring_card`).
    pub id: &'static str,
    /// The monster that drops it (monster id).
    pub monster: &'static str,
    /// The card's own name ("Poring Card").
    pub name: &'static str,
    pub fits: CardSlot,
    pub rarity: Rarity,
    pub affix: Affix,
    /// Sets count cards of one family ([`family`]).
    pub family: u8,
    /// Cards of one exclusive group (non-zero) cannot share an item.
    pub exclusive: u8,
    pub mods: Mods,
    pub hp: i16,
    pub sp: i16,
    pub procs: Procs,
    /// Where the monster lives, for the card face ("a sunny meadow").
    pub habitat: &'static str,
}

use Rarity::{Common, Mythic, Rare, Uncommon};

const P: Procs = Procs::NONE;
const N: Mods = Mods::NONE;

#[allow(clippy::too_many_arguments)]
const fn c(
    id: &'static str,
    monster: &'static str,
    name: &'static str,
    fits: CardSlot,
    rarity: Rarity,
    affix: Affix,
    habitat: &'static str,
    procs: Procs,
) -> Card {
    Card { id, monster, name, fits, rarity, affix, family: 0, exclusive: 0, mods: N, hp: 0, sp: 0, procs, habitat }
}

use Affix::{Prefix, Suffix};
use CardSlot as S;

/// The weapon elements (only one per item).
const ELEMENT: u8 = 1;

/// Every card, grouped by mission.
pub const CARDS: &[Card] = &[
    // --- Poring Island -----------------------------------------------
    Card { family: family::SLIME, ..c("poring_card", "poring", "Poring Card", S::Shoes, Common, Prefix("Bouncy"), "a sunny meadow of clover", Procs { magnet: 60, ..P }) },
    Card { family: family::SLIME, ..c("drops_card", "drops", "Drops Card", S::Weapon, Common, Suffix("Kindling"), "a warm sandy beach at sunset", Procs { ignite: 10, ..P }) },
    Card { mods: Mods { speed: 0.05, ..N }, ..c("lunatic_card", "lunatic", "Lunatic Card", S::Shoes, Common, Prefix("Springheel"), "tall grass under a full moon", Procs { flee_speed: 10, ..P }) },
    Card { hp: 10, ..c("fabre_card", "fabre", "Fabre Card", S::Body, Common, Prefix("Silken"), "a leafy branch in morning dew", Procs { cond_short: 30, ..P }) },
    Card { mods: Mods { hp_regen: 0.001, ..N }, ..c("roda_frog_card", "roda_frog", "Roda Frog Card", S::Garment, Common, Suffix("the Pond"), "a lily pond with reeds", Procs { immune: bits(&[Condition::Slowed, Condition::Mired]), ..P }) },
    c("hornet_card", "hornet", "Hornet Card", S::Weapon, Common, Prefix("Stinging"), "a flowering hedge buzzing with bees", Procs { size_bane: [3, 0, 0], ..P }),
    c("willow_card", "willow", "Willow Card", S::Body, Uncommon, Prefix("Rooted"), "a misty riverbank", Procs { still_ac: 2, ..P }),
    c("rocker_card", "rocker", "Rocker Card", S::Accessory, Common, Suffix("the Fiddler"), "a grassy hill at dusk with fireflies", Procs { combo_secs: 1, ..P }),
    c("creamy_card", "creamy", "Creamy Card", S::Garment, Common, Prefix("Fluttering"), "a garden of pastel flowers", Procs { dodge: 8, dodge_rush: true, ..P }),
    Card { family: family::SLIME, mods: Mods { venom: 12, ..N }, ..c("poporing_card", "poporing", "Poporing Card", S::Weapon, Common, Prefix("Venomous"), "a damp mossy forest floor", Procs { venom_long: true, ..P }) },
    c("eclipse_card", "eclipse", "Eclipse Card", S::Shoes, Rare, Suffix("the Moon Hare"), "a moonlit clearing", Procs { fresh: 6, ..P }),
    c("vocal_card", "vocal", "Vocal Card", S::Head, Rare, Suffix("the Encore"), "a stage of mushrooms under starlight", Procs { free_cd: 10, ..P }),
    Card { family: family::SLIME, ..c("mastering_card", "mastering", "Mastering Card", S::Accessory, Rare, Prefix("Regal"), "a jelly throne on a sunny hill", Procs { feast: 50, ..P }) },
    Card { family: family::SLIME, ..c("angeling_card", "angeling", "Angeling Card", S::Body, Mythic, Prefix("Seraphic"), "golden clouds and light beams", Procs { last_stand: true, ..P }) },
    // --- Prontera Culvert --------------------------------------------
    Card { family: family::THIEF_BUG, ..c("thief_bug_larva_card", "thief_bug_larva", "Thief Bug Larva Card", S::Head, Common, Prefix("Grubby"), "a mossy sewer pipe", Procs { zeny_pct: 15, ..P }) },
    Card { family: family::THIEF_BUG, ..c("thief_bug_card", "thief_bug", "Thief Bug Card", S::Shoes, Common, Prefix("Skittering"), "a sewer ledge by a lantern", Procs { rank_speed: 2, ..P }) },
    c("tarou_card", "tarou", "Tarou Card", S::Weapon, Common, Prefix("Plague"), "a pile of old crates in a cellar", Procs { afflicted_bane: 2, ..P }),
    c("plankton_card", "plankton", "Plankton Card", S::Shield, Uncommon, Suffix("the Sludge"), "glowing green sewer water", Procs { slow_back: 20, ..P }),
    Card { family: family::THIEF_BUG, mods: Mods { loot: 0.04, ..N }, ..c("thief_bug_female_card", "thief_bug_female", "Female Thief Bug Card", S::Accessory, Common, Prefix("Pilfering"), "a stash of shiny coins in a drain", Procs { crit_zeny: true, ..P }) },
    Card { family: family::THIEF_BUG, mods: Mods { block: 6, ..N }, ..c("thief_bug_male_card", "thief_bug_male", "Male Thief Bug Card", S::Shield, Uncommon, Prefix("Carapace"), "a brick tunnel with dripping water", Procs { riposte: 6, ..P }) },
    c("drainliar_card", "drainliar", "Drainliar Card", S::Weapon, Common, Prefix("Sanguine"), "a dark vaulted ceiling", Procs { leech: 8, ..P }),
    Card { family: family::THIEF_BUG, ..c("thief_bug_queen_card", "thief_bug_queen", "Thief Bug Queen Card", S::Garment, Rare, Suffix("the Brood"), "a nest of eggs in a warm sewer chamber", Procs { brood: true, ..P }) },
    c("rat_king_card", "rat_king", "Rat King Card", S::Head, Rare, Prefix("Crowned"), "a throne of junk in the sewers", Procs { elite_bane: 2, elite_zeny: 30, ..P }),
    c("golden_bug_card", "golden_bug", "Golden Thief Bug Card", S::Shield, Mythic, Prefix("Gilded"), "a hoard of gold in a hidden vault", Procs { gilded: 20, ..P }),
    Card { family: family::THIEF_BUG, hp: 15, ..c("thief_bug_egg_card", "thief_bug_egg", "Thief Bug Egg Card", S::Accessory, Common, Prefix("Brood-egg"), "a cosy nest of straw", P) },
    // --- Sograt Desert -----------------------------------------------
    c("picky_card", "picky", "Picky Card", S::Head, Common, Prefix("Hatchling"), "warm desert sand with a cracked eggshell", Procs { potion_cd: 25, ..P }),
    c("condor_card", "condor", "Condor Card", S::Head, Common, Prefix("Hawkeyed"), "a red mesa under a blue sky", Procs { reach: 1, ..P }),
    Card { family: family::DESERT_PACK, ..c("desert_wolf_b_card", "desert_wolf_b", "Baby Desert Wolf Card", S::Accessory, Common, Prefix("Pack"), "a sandy den among rocks", Procs { pack: 1, ..P }) },
    c("scorpion_card", "scorpion", "Scorpion Card", S::Weapon, Common, Suffix("the Stinger"), "sun-baked dunes", Procs { crit_sting: true, ..P }),
    Card { mods: Mods { speed: 0.05, ..N }, ..c("pecopeco_card", "pecopeco", "Peco Peco Card", S::Shoes, Common, Prefix("Galloping"), "a desert oasis with palm trees", Procs { ride_speed: 10, ..P }) },
    c("hode_card", "hode", "Hode Card", S::Body, Common, Prefix("Burrowing"), "a sand dune with burrow holes", Procs { still_regen: 10, ..P }),
    c("frilldora_card", "frilldora", "Frilldora Card", S::Garment, Common, Prefix("Frilled"), "a rocky desert canyon", Procs { blind_back: 15, ..P }),
    Card { family: family::DESERT_PACK, ..c("desert_wolf_card", "desert_wolf", "Desert Wolf Card", S::Weapon, Common, Prefix("Ravenous"), "a desert ridge at twilight", Procs { cull: true, ..P }) },
    c("sand_man_card", "sand_man", "Sandman Card", S::Body, Uncommon, Suffix("Shifting Sands"), "a swirling sandstorm", Procs { veil: 8, ..P }),
    Card { family: family::DESERT_PACK, exclusive: ELEMENT, ..c("sirocco_card", "sirocco", "Sirocco Card", S::Weapon, Rare, Suffix("the Simoom"), "a scorching heat haze over dunes", Procs { endow: Some(DamageType::Fire), ..P }) },
    c("sand_colossus_card", "sand_colossus", "Sand Colossus Card", S::Shield, Rare, Prefix("Monolithic"), "ancient desert ruins", Procs { colossus: true, ..P }),
    c("phreeoni_card", "phreeoni", "Phreeoni Card", S::Weapon, Mythic, Prefix("Devouring"), "a vast desert under a red sun", Procs { growth: 6, ..P }),
    // --- Payon Caves -------------------------------------------------
    Card { family: family::RESTLESS_DEAD, ..c("zombie_card", "zombie", "Zombie Card", S::Body, Common, Prefix("Deathless"), "a lantern-lit cave tunnel", Procs { deathless: true, ..P }) },
    Card { mods: Mods { light: 2.0, ..N }, ..c("familiar_card", "familiar", "Familiar Card", S::Garment, Common, Prefix("Nightwing"), "a cave ceiling with stalactites", Procs { immune: bits(&[Condition::Blinded]), ..P }) },
    Card { family: family::RESTLESS_DEAD, ..c("skeleton_card", "skeleton", "Skeleton Card", S::Weapon, Common, Prefix("Marrow"), "an old crypt with candles", Procs { held_bane: 3, ..P }) },
    Card { family: family::RESTLESS_DEAD, ..c("archer_skeleton_card", "archer_skeleton", "Archer Skeleton Card", S::Accessory, Common, Suffix("the Deadeye"), "a cave ledge with broken arrows", Procs { crit_die: 8, ..P }) },
    c("horong_card", "horong", "Horong Card", S::Head, Uncommon, Prefix("Wisp-lit"), "a dark cave lit by blue flames", Procs { wisp: 5, ..P }),
    Card { family: family::RESTLESS_DEAD, hp: 10, ..c("munak_card", "munak", "Munak Card", S::Shoes, Uncommon, Suffix("the Hopping Dead"), "a paper-lantern shrine in a cave", Procs { immune: bits(&[Condition::Rooted, Condition::Feared]), ..P }) },
    Card { family: family::RESTLESS_DEAD, ..c("bongun_card", "bongun", "Bongun Card", S::Shield, Uncommon, Prefix("Stiff-armed"), "a cave temple with hanging talismans", Procs { counter: 2, ..P }) },
    Card { exclusive: ELEMENT, ..c("sohee_card", "sohee", "Sohee Card", S::Weapon, Uncommon, Suffix("Sorrow"), "a cold underground lake", Procs { endow: Some(DamageType::Cold), chill: 8, ..P }) },
    c("nine_tail_card", "nine_tail", "Nine Tail Card", S::Accessory, Common, Prefix("Foxfire"), "an autumn forest at the cave mouth", Procs { free_every: 9, ..P }),
    c("weeping_bride_card", "weeping_bride", "Weeping Bride Card", S::Head, Rare, Suffix("the Veil"), "a ruined wedding hall with petals", Procs { desperate_ac: 3, ..P }),
    Card { family: family::RESTLESS_DEAD, ..c("jiangshi_general_card", "jiangshi_general", "Jiangshi General Card", S::Body, Rare, Prefix("Warlord's"), "a war camp of banners in a cavern", Procs { blow_cap: 35, ..P }) },
    c("moonlight_flower_card", "moonlight_flower", "Moonlight Flower Card", S::Garment, Mythic, Suffix("the Ninth Moon"), "a cherry-blossom grove under the moon", Procs { moon: 20, ..P }),
    // --- Flora -------------------------------------------------------
    Card { exclusive: ELEMENT, mods: Mods { light: 1.0, ..N }, ..c("shining_plant_card", "shining_plant", "Shining Plant Card", S::Weapon, Rare, Suffix("Radiance"), "a glowing grove with golden pollen", Procs { endow: Some(DamageType::Radiant), ..P }) },
    Card { mods: Mods { potion: 0.2, ..N }, ..c("black_mushroom_card", "black_mushroom", "Black Mushroom Card", S::Accessory, Common, Prefix("Sporecap"), "a shady forest floor", Procs { spore_cure: true, ..P }) },
    c("muka_card", "muka", "Muka Card", S::Shield, Uncommon, Prefix("Thorned"), "a desert garden of cacti", Procs { thorns: 4, ..P }),
];

/// A set: cards of one family worn together.
pub struct Set {
    pub family: u8,
    pub name: &'static str,
    /// (cards worn, what it adds, its tooltip line).
    pub tiers: &'static [(u8, Mods, &'static str)],
}

pub const SETS: &[Set] = &[
    Set {
        family: family::SLIME,
        name: "Slime Court",
        tiers: &[
            (2, Mods { hp_regen: 0.001, ..N }, "Regenerates 0.1% HP per second"),
            (3, Mods { potion: 0.3, ..N }, "Potions and food restore 30% more"),
        ],
    },
    Set {
        family: family::DESERT_PACK,
        name: "Desert Pack",
        tiers: &[(2, Mods { speed: 0.05, ..N }, "+5% walking speed"), (3, Mods { crit: 1, ..N }, "Critical hits on 19–20")],
    },
    Set {
        family: family::RESTLESS_DEAD,
        name: "Restless Dead",
        tiers: &[
            (2, Mods { evil_resist: 0.05, ..N }, "−5% damage from undead and demons"),
            (4, Mods { evil_damage: 2, ..N }, "+2 damage to undead and demons"),
        ],
    },
];

pub fn card(id: &str) -> Option<&'static Card> {
    CARDS.iter().find(|c| c.id == id)
}

/// The card a monster drops, if it has one.
pub fn of_monster(monster: &str) -> Option<&'static Card> {
    CARDS.iter().find(|c| c.monster == monster)
}

/// Card sockets of a catalogue item: by kind, fewer on magic items and
/// on high weapon levels (0..=4).
pub fn slots(e: &Entry) -> u8 {
    let base: i32 = match e.kind {
        Kind::Weapon => {
            let w = e.weapon.map_or(1, |w| w.level as i32);
            4 - w + i32::from(e.two_handed())
        }
        Kind::Armour(..) => 2,
        Kind::Headgear(Slot::UpperHead) => 1,
        Kind::Headgear(_) => 0,
        Kind::Shield | Kind::Garment | Kind::Shoes | Kind::Accessory => 1,
        _ => 0,
    };
    let less = match e.rarity {
        Rarity::Common => 0,
        Rarity::Uncommon => 1,
        _ => 2,
    };
    (base - less).clamp(0, super::items::MAX_CARDS as i32) as u8
}

/// Why a set of cards cannot sit in `e`, or `None` when it can.
pub fn check(e: &Entry, cards: &[String]) -> Option<CardError> {
    if CardSlot::of(e.kind).is_none() {
        return Some(CardError::WrongSlot);
    }
    if cards.len() > slots(e) as usize {
        return Some(CardError::NoSocket);
    }
    for (i, id) in cards.iter().enumerate() {
        let Some(c) = card(id) else { return Some(CardError::NotACard) };
        if Some(c.fits) != CardSlot::of(e.kind) {
            return Some(CardError::WrongSlot);
        }
        if cards[..i].contains(id) {
            return Some(CardError::Twice);
        }
        if c.exclusive != 0 && cards[..i].iter().filter_map(|o| card(o)).any(|o| o.exclusive == c.exclusive) {
            return Some(CardError::Conflict);
        }
    }
    None
}

/// What can go wrong socketing a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardError {
    NotACard,
    WrongSlot,
    NoSocket,
    Twice,
    Conflict,
}

/// The item's name with its cards: `+N Prefix Base of Suffix`, one prefix
/// and one suffix (the rarest card of each, ties by id; keys keep cards
/// sorted, so the order they went in never matters). A base already
/// holding " of " joins the suffix with "and".
pub fn item_name(base: &str, refine: u8, cards: &[String]) -> String {
    let pick = |want_prefix: bool| {
        cards
            .iter()
            .filter_map(|id| card(id))
            .filter(|c| matches!(c.affix, Affix::Prefix(_)) == want_prefix)
            .max_by(|a, b| a.rarity.cmp(&b.rarity).then(b.id.cmp(a.id)))
            .map(|c| match c.affix {
                Affix::Prefix(w) | Affix::Suffix(w) => w,
            })
    };
    let mut name = String::new();
    if refine > 0 {
        name.push_str(&format!("+{refine} "));
    }
    if let Some(p) = pick(true) {
        name.push_str(p);
        name.push(' ');
    }
    name.push_str(base);
    if let Some(s) = pick(false) {
        name.push_str(if base.contains(" of ") { " and " } else { " of " });
        name.push_str(s);
    }
    name
}

fn damage_label(t: DamageType) -> &'static str {
    match t {
        DamageType::Physical => "physical",
        DamageType::Fire => "fire",
        DamageType::Cold => "cold",
        DamageType::Lightning => "lightning",
        DamageType::Earth => "earth",
        DamageType::Poison => "poison",
        DamageType::Radiant => "radiant",
        DamageType::Necrotic => "necrotic",
        DamageType::Force => "force",
    }
}

fn cond_label(c: Condition) -> &'static str {
    match c {
        Condition::Slowed => "Slowed",
        Condition::Frozen => "Frozen",
        Condition::Stunned => "Stunned",
        Condition::Blinded => "Blinded",
        Condition::Rooted => "Rooted",
        Condition::Petrified => "Petrified",
        Condition::Burning => "Burning",
        Condition::Poisoned => "Poisoned",
        Condition::Mired => "Mired",
        Condition::Taunted => "Taunted",
        Condition::Feared => "Feared",
    }
}

impl Card {
    /// The word it adds to an item's name.
    pub fn word(&self) -> &'static str {
        match self.affix {
            Affix::Prefix(w) | Affix::Suffix(w) => w,
        }
    }

    /// Tooltip lines of the effect (`markup()`), plain [`Mods`] lines
    /// first (shared with magic items), then the procs.
    pub fn lines(&self) -> Vec<String> {
        let m = super::magic::Magic { id: self.id, rarity: self.rarity, mods: self.mods, saves: 0, hp: self.hp, sp: self.sp };
        let mut v = m.lines();
        let o = &self.procs;
        let mut ok = |s: String| v.push(format!("[ok]{s}[/]"));
        if o.ignite > 0 {
            ok(format!("{}% chance a basic attack sets the monster **Burning**", o.ignite));
        }
        if o.chill > 0 {
            ok(format!("{}% chance a basic attack **Slows** the monster", o.chill));
        }
        if o.venom_long {
            ok("Your poison lasts twice as long".into());
        }
        for (i, label) in ["small", "medium", "large"].iter().enumerate() {
            if o.size_bane[i] != 0 {
                ok(format!("{:+} damage to {label} monsters", o.size_bane[i]));
            }
        }
        if o.afflicted_bane != 0 {
            ok(format!("{:+} damage to monsters under any condition", o.afflicted_bane));
        }
        if o.held_bane != 0 {
            ok(format!("{:+} damage to Stunned, Frozen, Rooted or Slowed monsters", o.held_bane));
        }
        if o.elite_bane != 0 {
            ok(format!("{:+} damage to elites, bosses and starred monsters", o.elite_bane));
        }
        if o.fresh > 0 {
            ok(format!("+1d{} damage to a monster at full HP", o.fresh));
        }
        if o.crit_die > 0 {
            ok(format!("Ranged critical hits deal +1d{}", o.crit_die));
        }
        if o.leech > 0 {
            ok(format!("{}% chance a basic attack heals you 1d4", o.leech));
        }
        if o.cull {
            ok("Basic attacks on monsters under 20% HP are critical hits".into());
        }
        if o.crit_sting {
            ok("Critical hits leave the monster **Poisoned** and **Slowed**".into());
        }
        if o.crit_zeny {
            ok("Critical hits knock loose zeny".into());
        }
        if let Some(t) = o.endow {
            ok(format!("Basic attacks deal **{}** damage", damage_label(t)));
        }
        if o.reach > 0 {
            ok(format!("Ranged basic attacks reach {} m farther", o.reach as f32 * 0.5));
        }
        if o.counter > 0 {
            ok(format!("After a block or a dodge, your next basic attack deals +{}d6", o.counter));
        }
        if o.growth > 0 {
            ok(format!("+1 damage per 25 kills this expedition (max +{}); going down resets it", o.growth));
        }
        if o.pack != 0 {
            ok(format!("{:+} to hit and damage while an ally is near", o.pack));
        }
        if o.combo_secs > 0 {
            ok(format!("Kill combo lasts {} s longer", o.combo_secs));
        }
        if o.zeny_pct > 0 {
            ok(format!("+{}% zeny from kills", o.zeny_pct));
        }
        if o.elite_zeny > 0 {
            ok(format!("+{}% zeny from elites, bosses and starred monsters", o.elite_zeny));
        }
        if o.feast > 0 {
            ok(format!("Every {}th kill restores 25% HP and SP", o.feast));
        }
        if o.dodge > 0 {
            ok(format!("{}% chance to dodge a melee blow", o.dodge));
        }
        if o.dodge_rush {
            ok("A dodge gives +20% walking speed for 2 s".into());
        }
        if o.slow_back > 0 {
            ok(format!("{}% chance a monster that hits you in melee is **Slowed**", o.slow_back));
        }
        if o.blind_back > 0 {
            ok(format!("{}% chance a monster that hits you in melee is **Blinded**", o.blind_back));
        }
        if o.thorns > 0 {
            ok(format!("Melee attackers take 1d{}", o.thorns));
        }
        if o.riposte > 0 {
            ok(format!("A blocked blow deals 1d{} back", o.riposte));
        }
        if o.cond_short > 0 {
            ok(format!("Conditions on you end {}% sooner", o.cond_short));
        }
        if o.immune != 0 {
            let names: Vec<&str> = Condition::ALL.iter().filter(|c| o.immune & c.bit() != 0).map(|&c| cond_label(c)).collect();
            ok(format!("Cannot be {}", names.join(" or ")));
        }
        if o.still_ac != 0 {
            ok(format!("After standing still 2 s: {:+} AC, cannot be stunned or frozen", o.still_ac));
        }
        if o.desperate_ac != 0 {
            ok(format!("Under 30% HP: {:+} AC, cannot be feared", o.desperate_ac));
        }
        if o.blow_cap > 0 {
            ok(format!("No single blow takes more than {}% of your max HP", o.blow_cap));
        }
        if o.veil > 0 {
            ok(format!("Negates one melee blow every {} s", o.veil));
        }
        if o.gilded > 0 {
            ok(format!("−{}% non-physical damage taken; each point prevented becomes a zeny", o.gilded));
        }
        if o.last_stand {
            ok("Once per expedition, a blow that would down you leaves 1 HP and a shield".into());
        }
        if o.deathless {
            ok("Bleed out half as fast; revived at half HP".into());
        }
        if o.magnet > 0 {
            ok(format!("Pick up bonuses from {}% farther", o.magnet));
        }
        if o.flee_speed > 0 {
            ok(format!("+{}% walking speed under half HP", o.flee_speed));
        }
        if o.rank_speed > 0 {
            ok(format!("+{}% walking speed per style rank", o.rank_speed));
        }
        if o.ride_speed > 0 {
            ok(format!("+{}% walking speed while riding", o.ride_speed));
        }
        if o.still_regen > 0 {
            ok(format!("After standing still 3 s: regenerate {:.1}% HP per second", o.still_regen as f32 / 10.0));
        }
        if o.wisp > 0 {
            ok(format!("A wisp casts **Fire Bolt** at the nearest monster every {} s", o.wisp));
        }
        if o.free_cd > 0 {
            ok(format!("{}% chance a skill does not go on cooldown", o.free_cd));
        }
        if o.free_every > 0 {
            ok(format!("Every {}th skill costs no SP", o.free_every));
        }
        if o.moon > 0 {
            ok(format!("Every {} s your next skill is free: no SP, no cooldown", o.moon));
        }
        if o.potion_cd > 0 {
            ok(format!("Potion cooldown {}% shorter", o.potion_cd));
        }
        if o.spore_cure {
            ok("Potions also cure **poison**".into());
        }
        if o.brood {
            ok("+2% attack and walking speed per Thief Bug card worn (this one too)".into());
        }
        if o.colossus {
            ok("−1 physical damage per hit, −1 more at +5 and at +9 of its shield".into());
        }
        v
    }

    /// The catalogue description: what it fits, its word, its effect.
    pub fn describe(&self) -> String {
        let mut lines = vec![format!("Card · fits **{}** · adds \"{}\"", self.fits.label(), self.word())];
        lines.extend(self.lines());
        if let Some(s) = SETS.iter().find(|s| s.family == self.family) {
            lines.push(format!("[dim]Set: {}[/]", s.name));
        }
        lines.push(format!("[dim]{} card · drops only from its monster · socket it in town[/]", self.rarity.label()));
        lines.join("\n")
    }

    /// Price at a merchant (half when sold).
    pub const fn price(&self) -> u32 {
        match self.rarity {
            Rarity::Common => 400,
            Rarity::Uncommon => 1500,
            Rarity::Rare => 8000,
            Rarity::Mythic => 40000,
        }
    }
}

/// What the cards worn add up to (summed into [`super::items::Gear`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CardSum {
    pub mods: Mods,
    pub hp: i32,
    pub sp: i32,
    pub procs: Procs,
}

impl CardSum {
    /// Adds the cards of one worn item at `refine`.
    pub fn wear(&mut self, cards: &[String], refine: u8) {
        for c in cards.iter().filter_map(|id| card(id)) {
            self.mods.add(&c.mods);
            self.hp += c.hp as i32;
            self.sp += c.sp as i32;
            self.procs.add(&c.procs);
            if c.procs.colossus {
                self.mods.reduce += 1 + i32::from(refine >= 5) + i32::from(refine >= 9);
            }
        }
    }

    /// Sets and the brood, once every worn card is in (`worn`: card ids).
    pub fn finish<'a>(&mut self, worn: impl Iterator<Item = &'a str> + Clone) {
        let count = |f: u8| worn.clone().filter_map(card).filter(|c| c.family == f).count() as u8;
        for s in SETS {
            let n = count(s.family);
            for (need, m, _) in s.tiers {
                if n >= *need {
                    self.mods.add(m);
                }
            }
        }
        if self.procs.brood {
            let n = count(family::THIEF_BUG).min(6) as f32;
            self.mods.haste += 0.02 * n;
            self.mods.speed += 0.02 * n;
        }
        self.procs.clamp();
    }
}

#[cfg(test)]
mod tests {
    use super::super::items::{find, parse_key, Bag, ItemError};
    use super::super::Class;
    use super::*;

    #[test]
    fn every_card_is_unique_in_the_catalogue_and_says_what_it_does() {
        for (i, c) in CARDS.iter().enumerate() {
            assert!(CARDS[..i].iter().all(|o| o.id != c.id && o.monster != c.monster && o.word() != c.word()), "duplicate {}", c.id);
            assert!(c.id.ends_with("_card") && c.name.ends_with(" Card"), "{}", c.id);
            let e = find(c.id).unwrap_or_else(|| panic!("{} is not in the catalogue", c.id));
            assert_eq!(e.kind, Kind::Card(c.fits));
            assert!(!c.lines().is_empty(), "{} does nothing", c.id);
            assert!(e.desc.contains(c.word()));
        }
    }

    #[test]
    fn slots_follow_kind_rarity_and_weapon_level() {
        let s = |id: &str| slots(find(id).unwrap());
        assert_eq!((s("knife"), s("sword"), s("cotton_shirt"), s("guard"), s("red_potion")), (3, 3, 2, 1, 0));
        assert_eq!(s("katana"), 4, "two-handed level 1");
        for e in super::super::items::catalogue() {
            let n = slots(e);
            assert!(n as usize <= super::super::items::MAX_CARDS);
            if e.rarity == Rarity::Rare {
                assert!(n <= 2, "{}", e.id);
            }
        }
    }

    #[test]
    fn names_take_one_prefix_and_one_suffix_whatever_the_order() {
        let cards = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(item_name("Sword", 7, &cards(&["drops_card", "hornet_card"])), "+7 Stinging Sword of Kindling");
        assert_eq!(item_name("Sword", 0, &cards(&["hornet_card", "tarou_card"])), "Stinging Sword");
        assert_eq!(item_name("Sword", 0, &cards(&["tarou_card", "phreeoni_card"])), "Devouring Sword", "the rarer card names it");
        assert_eq!(item_name("Mace of Dawn", 0, &cards(&["drops_card"])), "Mace of Dawn and Kindling");
        assert_eq!(item_name("Sword", 0, &[]), "Sword");
    }

    #[test]
    fn a_card_fits_one_kind_once_and_elements_exclude_each_other() {
        let sword = find("sword").unwrap();
        let v = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(check(sword, &v(&["drops_card", "hornet_card"])), None);
        assert_eq!(check(sword, &v(&["poring_card"])), Some(CardError::WrongSlot));
        assert_eq!(check(sword, &v(&["drops_card", "drops_card"])), Some(CardError::Twice));
        assert_eq!(check(sword, &v(&["sirocco_card", "sohee_card"])), Some(CardError::Conflict));
        assert_eq!(check(sword, &v(&["drops_card", "hornet_card", "tarou_card", "skeleton_card"])), Some(CardError::NoSocket));
        assert_eq!(check(sword, &v(&["jellopy"])), Some(CardError::NotACard));
        assert!(parse_key("sword[poring_card]").is_none());
        assert!(parse_key("sword[drops_card,hornet_card]").is_some());
    }

    #[test]
    fn socketing_takes_one_off_the_stack_and_removing_puts_it_back() {
        let c = Class::Swordsman;
        let mut b = Bag::starting(c, "");
        b.add("sword", 2).unwrap();
        b.add("hornet_card", 1).unwrap();
        let key = b.socket("sword", None, "hornet_card").unwrap();
        assert_eq!(key, "sword[hornet_card]");
        assert_eq!((b.count("sword"), b.count("sword[hornet_card]"), b.count("hornet_card")), (1, 1, 0));
        // The worn one.
        b.add("drops_card", 1).unwrap();
        let worn = b.socket("sword", Some(Slot::Weapon), "drops_card").unwrap();
        assert_eq!(b.equip[Slot::Weapon as usize], worn);
        assert_eq!(b.socket(&worn, Some(Slot::Weapon), "drops_card"), Err(ItemError::NotInBag));
        let back = b.unsocket(&worn, Some(Slot::Weapon), "drops_card").unwrap();
        assert_eq!((back.as_str(), b.count("drops_card")), ("sword", 1));
        // Selling a carded item would lose the card.
        assert_eq!(b.sell("sword[hornet_card]", 1), Err(ItemError::HasCards));
        let plain = b.unsocket("sword[hornet_card]", None, "hornet_card").unwrap();
        assert_eq!((plain.as_str(), b.count("sword"), b.count("hornet_card")), ("sword", 2, 1));
        // Wrong kind.
        b.add("poring_card", 1).unwrap();
        assert_eq!(b.socket("sword", None, "poring_card"), Err(ItemError::Card(CardError::WrongSlot)));
    }

    #[test]
    fn worn_cards_add_procs_sets_and_the_colossus_scales_with_refine() {
        let c = Class::Swordsman;
        let mut b = Bag::starting(c, "");
        b.equip[Slot::Weapon as usize] = "sword[drops_card,hornet_card]".into();
        b.equip[Slot::Armour as usize] = "cotton_shirt[angeling_card]".into();
        b.equip[Slot::Shield as usize] = "guard+9[sand_colossus_card]".into();
        b.equip[Slot::Shoes as usize] = "sandals[poring_card]".into();
        let g = b.gear(c);
        assert_eq!((g.procs.ignite, g.procs.size_bane[0], g.procs.last_stand, g.procs.magnet), (10, 3, true, 60));
        assert_eq!(g.mods.reduce, 3);
        // Three slimes: Slime Court at 3.
        assert!(g.mods.hp_regen >= 0.001 && g.mods.potion >= 0.3);
        assert_eq!(Bag::starting(c, "").gear(c).procs, Procs::NONE);
    }

    #[test]
    fn stacked_cards_stay_capped() {
        let mut s = CardSum::default();
        for _ in 0..8 {
            s.wear(&["drops_card".into(), "creamy_card".into()], 0);
        }
        s.finish(std::iter::empty());
        assert_eq!((s.procs.ignite, s.procs.dodge), (25, 20));
    }

    /// Every card drops in the baked pack from its own monster only, at
    /// its tier's rate.
    #[test]
    fn every_card_drops_only_from_its_monster() {
        use crate::world::horde::catalog::MonsterCatalog;
        let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
        let Ok(cat) = MonsterCatalog::load(&dist) else { return };
        for d in cat.defs() {
            for r in d.drops.iter().filter(|r| card(r.item).is_some()) {
                let c = card(r.item).unwrap();
                assert_eq!(c.monster, d.id, "{} drops {}", d.id, c.id);
                assert!(r.rate <= 200, "{}: {} / 10000", c.id, r.rate);
            }
        }
        for c in CARDS {
            let Some(d) = cat.defs().iter().find(|d| d.id == c.monster) else { panic!("{} has no monster", c.id) };
            assert!(d.drops.iter().any(|r| r.item == c.id), "{} never drops", c.id);
        }
    }
}
