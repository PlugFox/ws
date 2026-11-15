//! Companions of the Merchant line: the homunculus, Summon Flora's plants
//! and Sphere Mine's Marine Spheres (design: "Ally entity architecture").
//!
//! A separate pool, outside `HordeStorage`: nothing that queries the horde
//! (player AoE, traps, kills, the director, `recycle_far`) can ever see a
//! companion, so "player skills never hit allies" holds by construction.
//! Monsters reach companions through two doors only: `Condition::Taunted`
//! with `store.taunter = LURE_BASE + id` (Amistr's Provoke and Castling, the
//! Marine Sphere; never a boss), and the opportunistic bite (a monster ready
//! to strike with no player in reach bites a companion in reach; `fight.rs`
//! step 4). Monster skills and boss patterns land on companions through the
//! hazards (`brain.rs strike`); the overtime fog and the map's own hazards
//! never do.
//!
//! Plain structs with an enum kind, not struct-of-arrays: the pool holds at
//! most [`CAPACITY`] units (4 players × 1 homunculus + 5 plants + 3
//! spheres), reserved once in [`Companions::default`] together with every
//! scratch buffer; nothing allocates in the tick after that.
//!
//! # The homunculus
//!
//! One per Merchant-line character, its type chosen by the player (never
//! random), its level, experience, Bond, stance, name and evolution kept in
//! `characters.homunculus` ([`HomSave`]; the socket task stores what
//! [`HubEvent::Homunculus`] reports). On a stage it is out whenever its
//! owner is on the horde's map, unless resting or dead: called once (the
//! first Call Homunculus picks the type), it follows its owner from mission
//! to mission. Its run HP stays with the instance (a relog resumes it),
//! never in the database. In town it follows its owner too, whole again,
//! and does nothing else ([`HubInstance::companions_town`]): that is where
//! its type is changed, it evolves, is renamed or sent to rest.
//!
//! The AI thinks at 5 Hz, staggered by id; movement, cooldowns and blows run
//! every tick. Thinking scores the threats around the owner (touching the
//! owner, the focus or the owner's own target, close to the owner, a role
//! term, close to itself), keeps its target unless another scores 30 %
//! better, and picks a state: follow, engage, kite (Vanilmirth), retreat
//! (Filir when hurt) or a skill's wind-up. Each type plays a role (healer,
//! tank, skirmisher, caster) through four skills unlocked at its levels 1,
//! 15 and 30 and on evolution. It walks a straight line with the player's
//! axis-separated wall slide, never chases more than [`LEASH`] cells from
//! its owner, and is put back beside the owner when it falls behind, gets
//! stuck or loses its line to the owner.
//!
//! Wire: entity ids are `COMPANION_ENTITY_BASE + id`; a homunculus rides in
//! the snapshot's player rows, rooted ones never; records go out as
//! `COMPANIONS` (0xA3), HP and death as `PLAYER_VITALS`, blows as `COMBAT`,
//! skills as `CAST` (the companion's entity id as caster), the owner's panel
//! as `COMPANION_STATE` (0xA4); commands come in as `COMPANION_CMD` (0xA8;
//! [`HubInstance::companion_cmd`]).

use super::area::Area;
use super::cast::{wound, Kill};
use super::{facing_of, hub_error, HubEvent, HubInstance, InstanceKind, MapIdx, Outgoing};
use crate::world::combat::rules::{attack_roll, saving_throw, AttackOutcome};
use crate::world::combat::skills::TownUse;
use crate::world::combat::{self, cantrip_k, exp_to_next, proficiency, save_dc, Class, Condition, Damage, DamageType, Dice, Mods, Rng};
use crate::world::horde::storage::FLAG_BOSS;
use crate::world::horde::{line_clear, Horde, Lure, LURE_BASE};
use crate::world::map::WalkGrid;
use crate::world::protocol::{
    companion_class, companion_flags, companion_state, hit, hom_type, look, stance, CompanionCmd, CompanionInfo, CompanionState, HitEvent,
    ServerMsg, Swing, Telegraph, COMPANION_ENTITY_BASE, MONSTER_ENTITY_BASE,
};
use serde::{Deserialize, Serialize};

/// The four homunculus types (chosen by the player, never random).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HomType {
    /// Healer: Healing Hands, Urgent Escape, Mental Charge.
    Lif,
    /// Tank: Provoke pulse, Bulwark, Castling.
    Amistr,
    /// Skirmisher: Moonlight, Flitting, Accelerated Flight.
    Filir,
    /// Caster: Caprice, Chaotic Blessings, Instruction Change.
    Vanilmirth,
}

impl HomType {
    pub const ALL: [HomType; 4] = [HomType::Lif, HomType::Amistr, HomType::Filir, HomType::Vanilmirth];

    /// Wire value (`protocol::hom_type`).
    pub const fn wire(self) -> u8 {
        match self {
            HomType::Lif => hom_type::LIF,
            HomType::Amistr => hom_type::AMISTR,
            HomType::Filir => hom_type::FILIR,
            HomType::Vanilmirth => hom_type::VANILMIRTH,
        }
    }

    /// From the wire value (`NONE` and unknown values: `None`).
    pub const fn from_wire(v: u8) -> Option<Self> {
        Some(match v {
            hom_type::LIF => HomType::Lif,
            hom_type::AMISTR => HomType::Amistr,
            hom_type::FILIR => HomType::Filir,
            hom_type::VANILMIRTH => HomType::Vanilmirth,
            _ => return None,
        })
    }

    /// Persistent id (`characters.homunculus` JSONB `type`).
    pub const fn id(self) -> &'static str {
        match self {
            HomType::Lif => "lif",
            HomType::Amistr => "amistr",
            HomType::Filir => "filir",
            HomType::Vanilmirth => "vanilmirth",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    pub const fn name(self) -> &'static str {
        match self {
            HomType::Lif => "Lif",
            HomType::Amistr => "Amistr",
            HomType::Filir => "Filir",
            HomType::Vanilmirth => "Vanilmirth",
        }
    }

    /// Sprite id in the companions pack (`_h` once evolved).
    pub const fn sprite(self, evolved: bool) -> &'static str {
        match (self, evolved) {
            (HomType::Lif, false) => "lif",
            (HomType::Lif, true) => "lif_h",
            (HomType::Amistr, false) => "amistr",
            (HomType::Amistr, true) => "amistr_h",
            (HomType::Filir, false) => "filir",
            (HomType::Filir, true) => "filir_h",
            (HomType::Vanilmirth, false) => "vanilmirth",
            (HomType::Vanilmirth, true) => "vanilmirth_h",
        }
    }
}

/// Summon Flora's plants, in unlock order (slv 1..=5; each cast plants the
/// next unlocked kind, cycling).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlantKind {
    /// Melee 1.5 m, slows on a failed save.
    Mandragora,
    /// Ranged 5 m.
    Hydra,
    /// Melee 2 m, the big die.
    Flora,
    /// Ranged 5 m poison.
    Parasite,
    /// Ranged 4 m; heals the most wounded ally within 4 m every 3 s.
    Geographer,
}

impl PlantKind {
    pub const ALL: [PlantKind; 5] = [PlantKind::Mandragora, PlantKind::Hydra, PlantKind::Flora, PlantKind::Parasite, PlantKind::Geographer];

    /// Sprite id in the companions pack.
    pub const fn sprite(self) -> &'static str {
        match self {
            PlantKind::Mandragora => "mandragora",
            PlantKind::Hydra => "hydra",
            PlantKind::Flora => "flora",
            PlantKind::Parasite => "parasite",
            PlantKind::Geographer => "geographer",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            PlantKind::Mandragora => "Mandragora",
            PlantKind::Hydra => "Hydra",
            PlantKind::Flora => "Flora",
            PlantKind::Parasite => "Parasite",
            PlantKind::Geographer => "Geographer",
        }
    }
}

/// What a companion is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Walks with its owner (snapshot rows).
    Homunculus(HomType),
    /// Rooted, `expires`.
    Plant(PlantKind),
    /// Sphere Mine's Marine Sphere: rooted, taunts, explodes on its fuse or
    /// when killed.
    Sphere,
}

impl Kind {
    /// Wire class (`protocol::companion_class`).
    pub const fn class(self) -> u8 {
        match self {
            Kind::Homunculus(_) => companion_class::HOMUNCULUS,
            Kind::Plant(_) => companion_class::PLANT,
            Kind::Sphere => companion_class::SPHERE,
        }
    }

    /// Rooted kinds never move and are not in snapshots.
    pub const fn rooted(self) -> bool {
        !matches!(self, Kind::Homunculus(_))
    }
}

/// Homunculus stance (`protocol::stance`): how far from the owner it
/// engages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stance {
    /// Threats within 9 cells of the owner.
    Aggressive,
    /// Within 5 cells.
    #[default]
    Defensive,
    /// Never engages; follows and supports.
    Passive,
}

impl Stance {
    pub const fn wire(self) -> u8 {
        match self {
            Stance::Aggressive => stance::AGGRESSIVE,
            Stance::Defensive => stance::DEFENSIVE,
            Stance::Passive => stance::PASSIVE,
        }
    }

    pub const fn from_wire(v: u8) -> Option<Self> {
        Some(match v {
            stance::AGGRESSIVE => Stance::Aggressive,
            stance::DEFENSIVE => Stance::Defensive,
            stance::PASSIVE => Stance::Passive,
            _ => return None,
        })
    }

    /// Threat radius around the owner, cells.
    pub const fn radius(self) -> f32 {
        match self {
            Stance::Aggressive => 9.0,
            Stance::Defensive => 5.0,
            Stance::Passive => 0.0,
        }
    }
}

/// What a companion is doing (the AI's state; `Dead` is also the wire's).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    /// Seek the slot 1.2 cells behind the owner's facing.
    #[default]
    Follow,
    /// Close in on the target and attack on cooldown.
    Engage,
    /// Hold a 3.5–5.5 cell band from the target (Vanilmirth).
    Kite,
    /// Back to the owner, no attacks until HP > 50 %.
    Retreat,
    /// Standing through a skill's wind-up (index into `cds`).
    Cast(u8),
    /// `hp <= 0`: the death animation, removed 0.75 s later (rooted) or
    /// back on the next wave (homunculus).
    Dead,
}


/// Units alive at once on one instance (companions of every player).
pub const CAPACITY: usize = 40;

/// The Call Homunculus skill (`skills.toml`): a character who learned it
/// and chose a type has the homunculus out on every stage.
pub const CALL_SKILL: &str = "call_homunculus";
/// The skills the result screen credits plants' and spheres' damage to
/// (when the book has them; else the basic-attack row).
pub const FLORA_SKILL: &str = "summon_flora";
pub const SPHERE_SKILL: &str = "sphere_mine";
/// The item an evolution consumes (Pharmacy's recipe).
pub const STONE_OF_SAGE: &str = "stone_of_sage";

/// Thinking rate (Hz): targets, states and skills; staggered by id.
const THINK_HZ: u32 = 5;
/// It never chases a target farther than this from its owner (cells).
pub const LEASH: f32 = 10.0;
/// Farther than this from its owner it is put back beside them (cells):
/// the owner blinked, rode a portal, or outran it.
const TELEPORT_FAR: f32 = 14.0;
/// Walking without progress this long, or without a straight line to its
/// owner this long, puts it back beside the owner (seconds).
const STUCK_SECS: f32 = 1.5;
const NO_LINE_SECS: f32 = 3.0;
/// A link-dead owner keeps it out this long (seconds).
const LINK_DEAD_SECS: f32 = 5.0;
/// Its follow slot: this far behind the owner's facing and this far to
/// the owner's right (cells). Off to the side, so it never stands on its
/// owner's sprite nor on the Pushcart trailing right behind them.
const FOLLOW_BEHIND: f32 = 1.4;
const FOLLOW_SIDE: f32 = 1.4;
/// Close enough to its follow slot (cells): it stops instead of shuffling.
const FOLLOW_SLACK: f32 = 0.5;
/// Its pace: the owner's walking speed times this.
const SPEED_FACTOR: f32 = 1.15;
/// The command key: an aim within [`FOCUS_PICK`] of a monster at most
/// [`FOCUS_RANGE`] from the owner focuses it this long; any other aim
/// recalls it for [`RECALL_SECS`] (no engaging while it comes back).
const FOCUS_SECS: f32 = 6.0;
const FOCUS_RANGE: f32 = 12.0;
const FOCUS_PICK: f32 = 2.5;
const RECALL_SECS: f32 = 2.0;
/// A new target must score this much better than the current one.
const HYSTERESIS: f32 = 1.3;
/// Threats looked at per think (nearest buckets first; plenty for a screen).
const MAX_THREATS: usize = 32;
/// Owner downed: it holds within this of the body (cells).
const DOWNED_HOLD: f32 = 3.0;
/// Vanilmirth's band around its target (cells): closer than `KITE_FLEE` it
/// steps away, blended toward the owner; farther than `KITE_FAR` it closes
/// in; its bolts reach [`BOLT_RANGE`] past the target's body.
const KITE_NEAR: f32 = 3.5;
const KITE_FAR: f32 = 5.5;
const KITE_FLEE: f32 = 2.5;
const BOLT_RANGE: f32 = 6.0;
/// Melee reach past both bodies (cells); "touching the owner" is a gap of
/// at most `TOUCH_GAP`.
const MELEE_GAP: f32 = 0.7;
const TOUCH_GAP: f32 = 0.6;
/// The death animation before a row goes (the monsters' delay).
const DEAD_SECS: f32 = 0.75;
/// Body radii (cells): blocking, bites, hazards.
const HOM_BODY: f32 = 0.4;
const PLANT_BODY: f32 = 0.45;
const SPHERE_BODY: f32 = 0.4;

/// Born this many levels below the owner; ×3 experience while this far
/// behind; a bonus copy of this share of the owner's kill experience.
const BORN_BEHIND: u32 = 5;
const CATCH_UP_GAP: u32 = 10;
const EXP_SHARE_PCT: u64 = 15;
/// Bond: per mission cleared together, per boss, per death (lost), what
/// S.B.R.44 spends, and the evolution's thresholds.
pub const BOND_MISSION: u8 = 5;
pub const BOND_BOSS: u8 = 2;
pub const BOND_DEATH: u8 = 3;
const BOND_SBR: u8 = 5;
pub const EVOLVE_OWNER_LEVEL: u32 = 60;
pub const EVOLVE_BOND: u8 = 75;
/// Changing the type of a homunculus (in town): its price; it keeps its
/// level, experience and evolution, the Bond starts again from 0.
pub const TYPE_CHANGE_ZENY: u64 = 5_000;
const NAME_MAX: usize = 16;
/// The owner's panel at most this often; the save at most this often.
const STATE_EVERY_SECS: f32 = 0.5;
const SAVE_EVERY_SECS: f32 = 2.0;

/// Summon Flora: lifetime, alive at once per skill level, HP share of the
/// owner's, blows every `PLANT_SWING_SECS`; the Geographer heals every
/// `GEO_SECS` within `GEO_RANGE`.
const PLANT_SECS: f32 = 40.0;
const PLANT_CAP: [usize; 5] = [2, 3, 3, 4, 4];
const PLANT_HP_PCT: i32 = 20;
const PLANT_SWING_SECS: f32 = 1.5;
const GEO_SECS: f32 = 3.0;
const GEO_RANGE: f32 = 4.0;
/// Sphere Mine: alive per owner, fuse, taunt pulse (every, how long, how
/// far, how many), blast radius.
const SPHERE_CAP: usize = 2;
const SPHERE_FUSE_SECS: f32 = 6.0;
const SPHERE_PULSE_SECS: f32 = 0.5;
const SPHERE_TAUNT_SECS: f32 = 3.0;
const SPHERE_TAUNT_R: f32 = 4.0;
const SPHERE_TAUNT_MAX: usize = 6;
const SPHERE_BLAST_R: f32 = 2.5;
/// Conditions the plants leave on a failed save (seconds).
const PLANT_COND_SECS: f32 = 3.0;

/// Buff ids of what a homunculus puts on its owner (never book indices,
/// below the pickup bonuses' `0xFF00`): Urgent Escape, Amistr Bulwark.
const BUFF_ESCAPE: u16 = 0xFE00;
const BUFF_BULWARK: u16 = 0xFE01;

/// The buff icon id of a homunculus buff on its owner (`BUFFS`:
/// `hom:<skill>`, the client picks the icon), or `None`.
pub(super) const fn buff_id(skill: u16) -> Option<&'static str> {
    match skill {
        BUFF_ESCAPE => Some("hom:urgent_escape"),
        BUFF_BULWARK => Some("hom:bulwark"),
        _ => None,
    }
}

/// Elements of Vanilmirth's bolts (Caprice's chaos).
const ELEMENTS: [DamageType; 4] = [DamageType::Fire, DamageType::Cold, DamageType::Lightning, DamageType::Earth];

/// One homunculus skill (unlocked at its level, `unlock` 0 = on
/// evolution). Cooldowns and SP are its own; the owner pays nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HomSkill {
    /// CAST skill id (the clients' effect key).
    pub id: &'static str,
    pub name: &'static str,
    pub unlock: u8,
    pub sp: i32,
    /// Seconds.
    pub cooldown: f32,
    pub cast: f32,
    /// Always on (no cooldown, never cast).
    pub passive: bool,
}

const fn sk(id: &'static str, name: &'static str, unlock: u8, sp: i32, cooldown: f32, cast: f32) -> HomSkill {
    HomSkill { id, name, unlock, sp, cooldown, cast, passive: false }
}

const LIF_SKILLS: [HomSkill; 4] = [
    sk("hom_healing_hands", "Healing Hands", 1, 8, 4.0, 0.3),
    sk("hom_urgent_escape", "Urgent Escape", 15, 12, 20.0, 0.0),
    sk("hom_mental_charge", "Mental Charge", 30, 16, 30.0, 0.0),
    HomSkill { passive: true, ..sk("hom_sanctified_touch", "Sanctified Touch", 0, 0, 0.0, 0.0) },
];
const AMISTR_SKILLS: [HomSkill; 4] = [
    sk("hom_provoke", "Provoke", 1, 6, 6.0, 0.0),
    sk("hom_bulwark", "Amistr Bulwark", 15, 14, 25.0, 0.0),
    sk("hom_castling", "Castling", 30, 20, 30.0, 0.0),
    sk("hom_blood_lust", "Blood Lust", 0, 18, 40.0, 0.0),
];
const FILIR_SKILLS: [HomSkill; 4] = [
    sk("hom_moonlight", "Moonlight", 1, 8, 5.0, 0.2),
    sk("hom_flitting", "Flitting", 15, 12, 25.0, 0.0),
    sk("hom_accelerated_flight", "Accelerated Flight", 30, 12, 25.0, 0.0),
    sk("hom_sbr44", "S.B.R.44", 0, 20, 60.0, 0.6),
];
const VANILMIRTH_SKILLS: [HomSkill; 4] = [
    sk("hom_caprice", "Caprice", 1, 8, 4.0, 0.4),
    sk("hom_chaotic_blessings", "Chaotic Blessings", 15, 12, 8.0, 0.3),
    sk("hom_instruction_change", "Instruction Change", 30, 14, 30.0, 0.0),
    sk("hom_self_destruction", "Self-Destruction", 0, 0, 60.0, 1.0),
];

/// Per type: hit die, CON, base armour class, basic attack die and
/// interval, ranged (Vanilmirth's bolt, save for half).
#[derive(Clone, Copy, Debug)]
struct TypeStats {
    hit_die: i32,
    con: i32,
    ac: i32,
    sides: u32,
    interval_ms: u32,
    ranged: bool,
}

impl HomType {
    /// Its four skills, in unlock order (`COMPANION_STATE` cooldowns).
    pub const fn skills(self) -> &'static [HomSkill; 4] {
        match self {
            HomType::Lif => &LIF_SKILLS,
            HomType::Amistr => &AMISTR_SKILLS,
            HomType::Filir => &FILIR_SKILLS,
            HomType::Vanilmirth => &VANILMIRTH_SKILLS,
        }
    }

    /// The numbers of the type (design §3 "Stats"). The basic attack's die
    /// is a step below the table's and its count follows the owner's Extra
    /// Attack (one die, two from level 30, [`hom_dice`]): the table's
    /// `k·dX` with `k = 1 + L/15` reaches the owner's whole basic-attack
    /// DPS by level 30, far above the 20–45 % band (`balance_tests.rs`).
    const fn stats(self) -> TypeStats {
        match self {
            HomType::Lif => TypeStats { hit_die: 8, con: 1, ac: 12, sides: 4, interval_ms: 1100, ranged: false },
            HomType::Amistr => TypeStats { hit_die: 12, con: 2, ac: 15, sides: 6, interval_ms: 1300, ranged: false },
            HomType::Filir => TypeStats { hit_die: 8, con: 1, ac: 13, sides: 4, interval_ms: 800, ranged: false },
            HomType::Vanilmirth => TypeStats { hit_die: 6, con: 0, ac: 11, sides: 8, interval_ms: 1400, ranged: true },
        }
    }

    /// Bolts at range instead of melee blows.
    pub const fn ranged(self) -> bool {
        self.stats().ranged
    }
}

/// Max HP at `level` (the `Class::max_hp` formula with the type's die and
/// CON), ×1.25 once evolved.
pub fn hom_max_hp(t: HomType, level: u8, evolved: bool) -> i32 {
    let s = t.stats();
    let mut hp = s.hit_die + s.con;
    for _ in 2..=level.max(1) {
        hp += s.hit_die / 2 + 1 + s.con;
    }
    if evolved { hp * 5 / 4 } else { hp }
}

pub const fn hom_max_sp(level: u8) -> i32 {
    20 + 3 * level as i32
}

/// Armour class `base + P/2`.
pub const fn hom_ac(t: HomType, level: u8) -> i32 {
    t.stats().ac + proficiency(level as u32) / 2
}

/// Attack bonus `P + 3` (`+4` evolved).
pub const fn hom_to_hit(level: u8, evolved: bool) -> i32 {
    proficiency(level as u32) + 3 + evolved as i32
}

/// One basic blow: one die of the type, two from level 30.
pub const fn hom_dice(t: HomType, level: u8) -> Dice {
    Dice::new(if level < 30 { 1 } else { 2 }, t.stats().sides, 0)
}

/// Basic attack interval (ms).
pub const fn hom_interval_ms(t: HomType) -> u32 {
    t.stats().interval_ms
}

impl Stance {
    /// Persistent id (`characters.homunculus` JSONB `stance`).
    pub const fn id(self) -> &'static str {
        match self {
            Stance::Aggressive => "aggressive",
            Stance::Defensive => "defensive",
            Stance::Passive => "passive",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [Stance::Aggressive, Stance::Defensive, Stance::Passive].into_iter().find(|s| s.id() == id)
    }
}

impl PlantKind {
    /// Reach past both bodies (cells), the die (`k` of them), the damage
    /// type, and the condition a failed save leaves.
    const fn attack(self) -> (f32, u32, DamageType, Option<Condition>) {
        match self {
            PlantKind::Mandragora => (1.5, 6, DamageType::Physical, Some(Condition::Slowed)),
            PlantKind::Hydra => (5.0, 6, DamageType::Physical, None),
            PlantKind::Flora => (2.0, 10, DamageType::Physical, None),
            PlantKind::Parasite => (5.0, 4, DamageType::Poison, Some(Condition::Poisoned)),
            PlantKind::Geographer => (4.0, 6, DamageType::Physical, None),
        }
    }
}

/// A homunculus as `characters.homunculus` keeps it (JSONB `{ "type",
/// "name", "level", "exp", "bond", "evolved", "stance" }`). Run HP stays
/// with the instance, never here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomSave {
    /// [`HomType::id`]; empty = not chosen yet.
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub level: u8,
    /// Experience into the level.
    #[serde(default)]
    pub exp: u64,
    /// 0..=100.
    #[serde(default)]
    pub bond: u8,
    #[serde(default)]
    pub evolved: bool,
    /// [`Stance::id`]; empty = defensive.
    #[serde(default)]
    pub stance: String,
}

/// Timed effects of its own skills (ticks they end at).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HomBuffs {
    /// Mental Charge (heals ×1.5, quicker blows), Instruction Change
    /// (damage ×1.5).
    pub boost: u32,
    /// Flitting: blows 30 % quicker.
    pub haste: u32,
    /// Amistr Bulwark: +4 AC.
    pub guard: u32,
    /// Accelerated Flight: +5 AC.
    pub dodge: u32,
    /// Urgent Escape: +30 % pace.
    pub speed: u32,
    /// Blood Lust: +2 damage, 30 % of it back as HP.
    pub lust: u32,
}

/// One companion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Companion {
    /// Pool id: entity `COMPANION_ENTITY_BASE + id`, lure `LURE_BASE + id`.
    pub id: u16,
    /// Owner's user id.
    pub owner: u32,
    /// Owner's player entity id (the `COMPANIONS` record).
    pub owner_entity: u32,
    pub kind: Kind,
    pub x: f32,
    pub y: f32,
    pub facing: u8,
    pub hp: i32,
    pub max_hp: i32,
    pub sp: i32,
    pub max_sp: i32,
    pub level: u8,
    /// Armour class and save bonus (one bonus for every save).
    pub ac: i32,
    pub save: i32,
    /// Attack bonus of its blows, DC of its blasts, `k` of its dice (plants
    /// and spheres: the owner's at the cast).
    pub to_hit: i32,
    pub dc: i32,
    pub k: u8,
    /// Skill level of the cast (plants, spheres).
    pub slv: u8,
    pub state: State,
    /// Monster slot + generation.
    pub target: Option<(u16, u8)>,
    /// Ticks until the next basic attack.
    pub attack_cd: u16,
    /// Tick of the next think (5 Hz, staggered by id).
    pub think_at: u32,
    /// Tick it goes (plants, spheres); 0 = never.
    pub expires: u32,
    /// Ticks without progress toward its goal (teleport after 1.5 s).
    pub stuck: u16,
    /// Ready tick of each of its four skills.
    pub cds: [u32; 4],
    pub stance: Stance,
    /// An evolved homunculus.
    pub evolved: bool,
    pub buffs: HomBuffs,
    /// Stunned, frozen or petrified (a monster skill) until this tick.
    pub held_until: u32,
    /// Where the skill being cast lands, its target, the ally it helps
    /// (user id, 0 = itself), and the tick it lands.
    aim: (f32, f32),
    cast_target: Option<(u16, u8)>,
    cast_ally: u32,
    cast_at: u32,
    /// The state it goes back to when the skill has landed.
    after: State,
    /// Amistr's guard point between the owner and the densest threat.
    goal: Option<(f32, f32)>,
    /// Tick of its death (the row goes [`DEAD_SECS`] later).
    died_at: u32,
    /// Next taunt pulse (sphere) or heal (Geographer).
    pulse_at: u32,
    /// SP regeneration carried between ticks.
    regen: f32,
    /// A sphere bitten to death: it blows up on the next step.
    boom: bool,
    /// Its first step put it on a walkable cell beside its owner.
    placed: bool,
    /// Self-Destruction: a death that costs no Bond.
    sacrificed: bool,
    /// Its record went out (cleared to resend: a level, a rename).
    shown: bool,
    /// Vitals last sent (hp, max hp, sp, max sp, state).
    sent: Option<[i32; 5]>,
    /// The map it stands on (a stage: the horde's; a town: its owner's).
    pub map: MapIdx,
}

impl Companion {
    /// Entity id on the wire.
    pub const fn entity(&self) -> u32 {
        COMPANION_ENTITY_BASE + self.id as u32
    }

    /// Lure / taunter id the horde sees.
    pub const fn lure_id(&self) -> u32 {
        crate::world::horde::LURE_BASE + self.id as u32
    }

    pub const fn alive(&self) -> bool {
        !matches!(self.state, State::Dead)
    }

    /// Body radius (cells).
    pub const fn body(&self) -> f32 {
        match self.kind {
            Kind::Homunculus(_) => HOM_BODY,
            Kind::Plant(_) => PLANT_BODY,
            Kind::Sphere => SPHERE_BODY,
        }
    }

    /// Armour class with its buffs, at tick `now`.
    pub const fn armour(&self, now: u32) -> i32 {
        self.ac + if self.buffs.guard > now { 4 } else { 0 } + if self.buffs.dodge > now { 5 } else { 0 }
    }

    fn blank(id: u16, owner: u32, owner_entity: u32, kind: Kind, x: f32, y: f32) -> Self {
        Companion {
            id,
            owner,
            owner_entity,
            kind,
            x,
            y,
            facing: 0,
            hp: 1,
            max_hp: 1,
            sp: 0,
            max_sp: 0,
            level: 1,
            ac: 10,
            save: 0,
            to_hit: 0,
            dc: 10,
            k: 1,
            slv: 1,
            state: State::Follow,
            target: None,
            attack_cd: 0,
            think_at: 0,
            expires: 0,
            stuck: 0,
            cds: [0; 4],
            stance: Stance::Defensive,
            evolved: false,
            buffs: HomBuffs::default(),
            held_until: 0,
            aim: (x, y),
            cast_target: None,
            cast_ally: 0,
            cast_at: 0,
            after: State::Follow,
            goal: None,
            died_at: 0,
            pulse_at: 0,
            regen: 0.0,
            boom: false,
            placed: false,
            sacrificed: false,
            shown: false,
            sent: None,
            map: 0,
        }
    }
}

/// What a summoning skill asks for (`Payload::Summon { what, level }`;
/// `skills.toml` `type = "summon", what = "<id>"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Summon {
    /// Call Homunculus: call it out (or, while out, the command key).
    Homunculus,
    /// Resurrect Homunculus: back at once at 20 %·slv HP.
    Resurrect,
    /// Summon Flora (Bio Cannibalize): the next unlocked plant.
    Flora,
    /// Sphere Mine: a Marine Sphere.
    SphereMine,
    /// Plant Cultivation: a real field plant (`horde/plants.rs`), not a
    /// companion: `summon` answers [`Summoned::FieldPlant`] and the caster
    /// side spawns it in the horde.
    Cultivation,
}

impl Summon {
    pub const ALL: [Summon; 5] = [Summon::Homunculus, Summon::Resurrect, Summon::Flora, Summon::SphereMine, Summon::Cultivation];

    /// The `what` id in `skills.toml`.
    pub const fn id(self) -> &'static str {
        match self {
            Summon::Homunculus => "homunculus",
            Summon::Resurrect => "resurrect_homunculus",
            Summon::Flora => "flora",
            Summon::SphereMine => "sphere_mine",
            Summon::Cultivation => "cultivation",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }
}

/// The summoner, as `summon` needs it (copied out of the fighter so the
/// pool needs no borrow of the players).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Owner {
    /// User id.
    pub uid: u32,
    /// Player entity id (the `COMPANIONS` owner field).
    pub entity: u32,
    pub level: u32,
    pub max_hp: i32,
    /// The owner's attack bonus (plants roll with it).
    pub to_hit: i32,
    /// The instance tick of the cast.
    pub tick: u32,
}

/// What a `summon` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Summoned {
    /// A companion with this entity id is out (new, or replacing the oldest
    /// plant / sphere over the cap), or the homunculus took the command.
    Unit(u32),
    /// Plant Cultivation: spawn a field plant at the point (slv 1 a
    /// mushroom, 2 a weighted herb plant); the caster side does it.
    FieldPlant,
    /// Nothing happened (no type chosen yet: the owner's panel opens the
    /// chooser; dead without Resurrect; the pool is full).
    Refused,
}

/// A player's homunculus on this instance: what is saved, and the run's
/// state while it is not out (HP, SP, cooldowns kept for its return).
#[derive(Clone, Debug)]
struct Hom {
    owner: u32,
    owner_entity: u32,
    owner_level: u32,
    save: HomSave,
    kind: Option<HomType>,
    stance: Stance,
    /// Out on stages: Call Homunculus learned (or cast) and a type chosen.
    called: bool,
    resting: bool,
    /// Waiting for the next wave (or Resurrect).
    dead: bool,
    /// The owner left the instance (a relog resumes the run state).
    away: bool,
    /// Pool id while out.
    unit: Option<u16>,
    /// HP while not out (`None` = full), SP, cooldowns.
    hp: Option<i32>,
    sp: i32,
    cds: [u32; 4],
    /// Focused monster (slot, generation, until tick); recall until.
    focus: Option<(u16, u8, u32)>,
    recall_until: u32,
    /// The command key's aim, for the next step.
    command: Option<(f32, f32)>,
    /// What the owner's last basic attack struck (it helps there).
    owner_target: Option<(u16, u8)>,
    /// Ticks the owner has been link-dead; ticks without a line to them.
    link_dead: u32,
    no_line: u32,
    /// Summon Flora's next kind (index into `PlantKind::ALL`).
    plant_next: u8,
    /// The panel last sent and when the next may go.
    sent: Option<CompanionState>,
    state_at: u32,
    /// The save changed since it was last reported, and when it may go.
    save_dirty: bool,
    save_at: u32,
}

impl Hom {
    fn new(owner: u32, owner_entity: u32, owner_level: u32, save: Option<HomSave>, call_learned: bool) -> Self {
        let save = save.unwrap_or_default();
        let kind = HomType::from_id(&save.kind);
        Hom {
            owner,
            owner_entity,
            owner_level,
            stance: Stance::from_id(&save.stance).unwrap_or_default(),
            called: kind.is_some() && call_learned,
            kind,
            save,
            resting: false,
            dead: false,
            away: false,
            unit: None,
            hp: None,
            sp: i32::MAX,
            cds: [0; 4],
            focus: None,
            recall_until: 0,
            command: None,
            owner_target: None,
            link_dead: 0,
            no_line: 0,
            plant_next: 0,
            sent: None,
            state_at: 0,
            save_dirty: false,
            save_at: 0,
        }
    }

    fn name(&self) -> String {
        match (self.kind, self.save.name.is_empty()) {
            (Some(t), true) => t.name().into(),
            _ => self.save.name.clone(),
        }
    }

    fn touch(&mut self) {
        self.save_dirty = true;
        self.sent = None;
    }

    /// The bonus experience of a kill; levels up to the owner's level.
    /// Returns whether it levelled.
    fn gain(&mut self, exp: u64) -> bool {
        if self.kind.is_none() || exp == 0 {
            return false;
        }
        let cap = self.owner_level.clamp(1, combat::MAX_LEVEL) as u8;
        let behind = self.owner_level.saturating_sub(self.save.level as u32) >= CATCH_UP_GAP;
        self.save.exp += if behind { exp * 3 } else { exp };
        let mut levelled = false;
        while self.save.level < cap && self.save.exp >= exp_to_next(self.save.level as u32) {
            self.save.exp -= exp_to_next(self.save.level as u32);
            self.save.level += 1;
            levelled = true;
        }
        if self.save.level >= cap {
            self.save.exp = self.save.exp.min(exp_to_next(self.save.level as u32).saturating_sub(1));
        }
        self.touch();
        levelled
    }

    fn bond(&mut self, delta: i32) {
        if self.kind.is_some() {
            self.save.bond = (self.save.bond as i32 + delta).clamp(0, 100) as u8;
            self.touch();
        }
    }
}

/// A player on the horde's map, as the companions see them (copied out of
/// the players, so the step borrows no `HubPlayer`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Ally {
    pub uid: u32,
    pub entity: u32,
    pub x: f32,
    pub y: f32,
    pub facing: u8,
    pub hp: i32,
    pub max_hp: i32,
    /// Standing (not downed, not dead).
    pub up: bool,
    pub level: u32,
    pub link_dead: bool,
    /// Its walking step (cells per tick, its pace included).
    pub step: f32,
}

/// What companions do to players this tick (`fight.rs` applies it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum AllyFx {
    /// Heal (and with Sanctified Touch cure the ailments).
    Heal { uid: u32, amount: i32, cure: bool },
    /// A buff for `ticks` (Urgent Escape, Bulwark).
    Buff { uid: u32, skill: u16, ticks: u32, mods: Mods },
    /// Castling: the owner stands here now.
    Move { uid: u32, x: f32, y: f32 },
}

/// Where companions' damage is credited on the result screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Homunculus,
    Plant,
    Sphere,
}

/// The split borrows of one companion step (the pattern of `brain::strike`).
pub(super) struct StepCx<'a> {
    pub horde: &'a mut Horde,
    pub walk: &'a WalkGrid,
    pub rng: &'a mut Rng,
    /// Players on the horde's map (with a fighter).
    pub allies: &'a [Ally],
    pub tick: u32,
    pub hz: f32,
    pub hits: &'a mut Vec<HitEvent>,
    pub kills: &'a mut Vec<Kill>,
    pub swings: &'a mut Vec<Swing>,
    pub fx: &'a mut Vec<AllyFx>,
}

/// A monster as the homunculus weighs it.
#[derive(Clone, Copy, Debug)]
struct Threat {
    slot: u16,
    generation: u8,
    x: f32,
    y: f32,
    radius: f32,
    /// Distance from the owner and from the homunculus (centres).
    d_owner: f32,
    d_self: f32,
    /// Its body touches the owner's.
    touch: bool,
    /// HP left (Filir only; 1 for the others).
    hp_frac: f32,
    /// An elite, a boss or starred: worth S.B.R.44.
    big: bool,
}

/// What the step produced besides the hit list.
#[derive(Debug, Default)]
struct Sink {
    /// Skill casts (`CAST`), for everyone on the map.
    casts: Vec<ServerMsg>,
    /// Monster kills by companions (slot, owner).
    killed: Vec<(u16, u32)>,
    /// Damage on monsters, per owner and source (the result screen).
    credits: Vec<(u32, Source, HitEvent)>,
}

/// The pool of one instance.
#[derive(Debug)]
pub struct Companions {
    units: Vec<Companion>,
    /// Monster kills by companions this tick (slot, owner uid): the owner
    /// gets exp, zeny and loot, no combo and no card kill.
    killed: Vec<(u16, u32)>,
    /// What the horde sees (`horde.set_lures`), rebuilt every tick.
    lures: Vec<Lure>,
    next_id: u16,
    homs: Vec<Hom>,
    sink: Sink,
    /// Records of units that left (`GONE`), for the next broadcast to the
    /// players of the map they stood on.
    gone: Vec<(MapIdx, CompanionInfo)>,
    /// The map units summoned now stand on (set before every step).
    map: MapIdx,
    /// Heals landed on companions between steps (`heal_in`, potions).
    pending: Vec<HitEvent>,
    // Scratch, reserved once.
    threats: Vec<Threat>,
    buf: Vec<u16>,
}

impl Default for Companions {
    fn default() -> Self {
        Self {
            units: Vec::with_capacity(CAPACITY),
            killed: Vec::with_capacity(CAPACITY),
            lures: Vec::with_capacity(CAPACITY),
            next_id: 0,
            homs: Vec::with_capacity(8),
            sink: Sink { casts: Vec::with_capacity(16), killed: Vec::with_capacity(CAPACITY), credits: Vec::with_capacity(64) },
            gone: Vec::with_capacity(CAPACITY),
            map: 0,
            pending: Vec::with_capacity(16),
            threats: Vec::with_capacity(MAX_THREATS),
            buf: Vec::with_capacity(256),
        }
    }
}

/// `secs` in whole ticks (at least one).
fn ticks(secs: f32, hz: f32) -> u32 {
    ((secs * hz).round() as u32).max(1)
}

fn ms(ticks: u32, hz: f32) -> u32 {
    (ticks as f32 * 1000.0 / hz.max(1.0)).round() as u32
}

fn dist(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    ((ax - bx) * (ax - bx) + (ay - by) * (ay - by)).sqrt()
}

fn unit_vec(dx: f32, dy: f32) -> Option<(f32, f32)> {
    let d = (dx * dx + dy * dy).sqrt();
    (d > 1e-4).then(|| (dx / d, dy / d))
}

/// A live monster still of that generation, with HP left.
fn live(horde: &Horde, slot: u16, generation: u8) -> bool {
    let s = horde.store();
    s.is_alive(slot) && s.generation[slot as usize] == generation && s.hp[slot as usize] > 0 && !s.burrowed(slot)
}

/// Non-plant monsters with HP left whose body touches the circle (into
/// `buf`, cleared first).
fn monsters_in(horde: &Horde, x: f32, y: f32, r: f32, buf: &mut Vec<u16>) {
    buf.clear();
    Area::Circle { x, y, r }.monsters(horde, buf);
    let s = horde.store();
    buf.retain(|&m| !s.is_plant(m) && s.hp[m as usize] > 0);
}

/// The slot behind the owner's facing (or the owner's own spot when that
/// is walled off).
fn follow_slot(owner: &Ally, walk: &WalkGrid) -> (f32, f32) {
    let a = owner.facing as f32 * std::f32::consts::FRAC_PI_4;
    // Facing (sin, cos); its right hand (cos, -sin).
    let (sx, sy) = (owner.x - a.sin() * FOLLOW_BEHIND + a.cos() * FOLLOW_SIDE, owner.y - a.cos() * FOLLOW_BEHIND - a.sin() * FOLLOW_SIDE);
    if walk.is_walkable(sx, sy) && line_clear(walk, owner.x, owner.y, sx, sy) { (sx, sy) } else { (owner.x, owner.y) }
}

/// A walkable spot beside the owner: the follow slot, else a ring cell,
/// else the owner's own spot.
fn beside(owner: &Ally, walk: &WalkGrid) -> (f32, f32) {
    let slot = follow_slot(owner, walk);
    if slot != (owner.x, owner.y) {
        return slot;
    }
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let (x, y) = (owner.x + a.sin(), owner.y + a.cos());
        if walk.is_walkable(x, y) && line_clear(walk, owner.x, owner.y, x, y) {
            return (x, y);
        }
    }
    (owner.x, owner.y)
}

/// `goal` pulled back to within `leash` of the owner.
fn leashed(goal: (f32, f32), owner: &Ally, leash: f32) -> (f32, f32) {
    let d = dist(goal.0, goal.1, owner.x, owner.y);
    if d <= leash {
        return goal;
    }
    let f = leash / d;
    (owner.x + (goal.0 - owner.x) * f, owner.y + (goal.1 - owner.y) * f)
}

/// One straight step toward `goal` with the player's axis-separated wall
/// slide; counts the ticks without progress (`stuck`).
fn walk_to(c: &mut Companion, goal: (f32, f32), speed: f32, slack: f32, walk: &WalkGrid) {
    let (dx, dy) = (goal.0 - c.x, goal.1 - c.y);
    let d = (dx * dx + dy * dy).sqrt();
    if d <= slack {
        c.stuck = 0;
        return;
    }
    let step = speed.min(d);
    let (ux, uy) = (dx / d, dy / d);
    let (x0, y0) = (c.x, c.y);
    let nx = c.x + ux * step;
    if walk.is_walkable(nx, c.y) {
        c.x = nx;
    }
    let ny = c.y + uy * step;
    if walk.is_walkable(c.x, ny) {
        c.y = ny;
    }
    c.facing = facing_of(ux, uy);
    let moved = dist(c.x, c.y, x0, y0);
    c.stuck = if moved < step * 0.25 { c.stuck.saturating_add(1) } else { 0 };
}

/// Monsters (not plants, not bosses when `skip_boss`) touching the circle.
fn count_near(horde: &Horde, x: f32, y: f32, r: f32, skip_boss: bool, buf: &mut Vec<u16>) -> usize {
    monsters_in(horde, x, y, r, buf);
    let s = horde.store();
    buf.iter().filter(|&&m| !(skip_boss && s.flags[m as usize] & FLAG_BOSS != 0)).count()
}

/// Taunts up to `max` non-boss monsters touching the circle (nearest
/// first) toward `lure` for `ticks`. Returns how many.
fn taunt(horde: &mut Horde, x: f32, y: f32, r: f32, max: usize, lure: u32, ticks: u16, buf: &mut Vec<u16>) -> usize {
    monsters_in(horde, x, y, r, buf);
    let s = horde.store();
    buf.retain(|&m| s.flags[m as usize] & FLAG_BOSS == 0);
    let d2 = |m: u16| (s.x[m as usize] - x).powi(2) + (s.y[m as usize] - y).powi(2);
    buf.sort_by(|&a, &b| d2(a).total_cmp(&d2(b)).then(a.cmp(&b)));
    buf.truncate(max);
    for &m in buf.iter() {
        horde.store_mut().afflict(m, Condition::Taunted, ticks);
        horde.store_mut().taunter[m as usize] = lure;
    }
    buf.len()
}

/// A blow (or bolt) of a companion on monster `s`, with its credit, kill
/// and number. `dice`: what lands; `ranged`: a save for half instead of an
/// attack roll; `sure`: no roll at all. Returns the damage dealt.
fn blow(
    c: &Companion,
    s: u16,
    dice: Dice,
    kind: DamageType,
    ranged: bool,
    sure: bool,
    source: Source,
    cx: &mut StepCx<'_>,
    sink: &mut Sink,
) -> i32 {
    let horde = &mut *cx.horde;
    let block = horde.block(s);
    let status = horde.store().status_of(s);
    let target = MONSTER_ENTITY_BASE + s as u32;
    let k = c.k.max(1) as u32;
    let (crit, saved) = if ranged {
        (false, !status.saves_fail() && saving_throw(cx.rng, block.save, c.dc, 0))
    } else if sure {
        (false, false)
    } else {
        match attack_roll(cx.rng, c.to_hit, block.ac + status.ac(), status.advantage_against(), 20) {
            AttackOutcome::Miss => {
                cx.hits.push(HitEvent { target, amount: 0, flags: hit::MISS });
                return 0;
            }
            o => (o == AttackOutcome::Critical && !horde.is_plant(s), false),
        }
    };
    let mut amount = wound(horde, cx.rng, s, Damage::new(dice, kind), None, crit, saved, k);
    // Instruction Change and Blood Lust add on top of what landed.
    let mut extra = 0;
    if matches!(c.kind, Kind::Homunculus(HomType::Vanilmirth)) && c.buffs.boost > cx.tick {
        extra += amount / 2;
    }
    if c.buffs.lust > cx.tick && amount > 0 {
        extra += 2;
    }
    if extra > 0 && !horde.is_plant(s) {
        horde.store_mut().hp[s as usize] -= extra;
        amount += extra;
    }
    let dead = horde.store().hp[s as usize] <= 0;
    let mut flags = super::fight::damage_type_bits(kind);
    if crit {
        flags |= hit::CRITICAL;
    }
    if dead {
        flags |= hit::KILL;
        cx.kills.push((s, c.owner));
        sink.killed.push((s, c.owner));
    }
    let h = HitEvent { target, amount: amount.max(0) as u32, flags };
    cx.hits.push(h);
    sink.credits.push((c.owner, source, h));
    amount
}

/// A swing on the wire, facing its target.
fn swing(c: &mut Companion, tx: f32, ty: f32, s: u16, swings: &mut Vec<Swing>) {
    c.facing = facing_of(tx - c.x, ty - c.y);
    swings.push(Swing { entity_id: c.entity(), facing: c.facing, target: MONSTER_ENTITY_BASE + s as u32 });
}

impl Companions {
    /// Every unit, alive or dying.
    pub fn units(&self) -> &[Companion] {
        &self.units
    }

    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// The units of one owner.
    pub fn of(&self, owner: u32) -> impl Iterator<Item = &Companion> {
        self.units.iter().filter(move |c| c.owner == owner)
    }

    /// The unit with this entity id.
    #[cfg_attr(not(test), expect(dead_code, reason = "API for the skills side (cast.rs); only the tests call it yet"))]
    pub fn by_entity(&self, entity: u32) -> Option<&Companion> {
        self.units.iter().find(|c| c.entity() == entity)
    }

    /// The owner's homunculus, when out.
    #[cfg_attr(not(test), expect(dead_code, reason = "API for the skills side (cast.rs); only the tests call it yet"))]
    pub fn homunculus_of(&self, owner: u32) -> Option<&Companion> {
        self.units.iter().find(|c| c.owner == owner && matches!(c.kind, Kind::Homunculus(_)))
    }

    /// The owner's saved homunculus (type, level, Bond...), if any.
    #[cfg_attr(not(test), expect(dead_code, reason = "API for the skills side (cast.rs); only the tests call it yet"))]
    pub fn save_of(&self, owner: u32) -> Option<&HomSave> {
        self.homs.iter().find(|h| h.owner == owner).map(|h| &h.save)
    }

    /// The horde's view of the pool.
    pub fn lures(&self) -> &[Lure] {
        &self.lures
    }

    /// Kills of this tick, drained by the combat step.
    pub fn killed(&mut self) -> std::vec::Drain<'_, (u16, u32)> {
        self.killed.drain(..)
    }

    fn hom_index(&self, owner: u32) -> Option<usize> {
        self.homs.iter().position(|h| h.owner == owner)
    }

    fn hom_entry(&mut self, owner: u32, entity: u32, level: u32) -> usize {
        match self.hom_index(owner) {
            Some(i) => i,
            None => {
                self.homs.push(Hom::new(owner, entity, level, None, false));
                self.homs.len() - 1
            }
        }
    }

    /// A free pool id, or `None` when the pool is full.
    fn alloc(&mut self) -> Option<u16> {
        if self.units.len() >= CAPACITY {
            return None;
        }
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);
            if !self.units.iter().any(|c| c.id == id) {
                return Some(id);
            }
        }
    }

    /// Cast a summoning skill of level `slv` at (`x`, `y`) for `owner`
    /// (the aim point; units are put on a walkable cell by their first
    /// step). Call Homunculus while it is out is the command key: an aim
    /// on a monster focuses it, any other aim recalls it.
    pub fn summon(&mut self, owner: Owner, what: Summon, slv: u8, x: f32, y: f32) -> Summoned {
        let slv = slv.max(1);
        match what {
            Summon::Cultivation => Summoned::FieldPlant,
            Summon::Homunculus => self.call(owner, x, y),
            Summon::Resurrect => {
                let h = self.hom_entry(owner.uid, owner.entity, owner.level);
                let hom = &mut self.homs[h];
                let Some(t) = hom.kind else { return Summoned::Refused };
                if !hom.dead || hom.unit.is_some() {
                    return Summoned::Refused;
                }
                let max = hom_max_hp(t, hom.save.level, hom.save.evolved);
                hom.dead = false;
                hom.resting = false;
                hom.called = true;
                hom.hp = Some((max * 20 * slv.min(5) as i32 / 100).max(1));
                hom.sent = None;
                self.spawn_hom(h, x, y, owner.tick).map_or(Summoned::Refused, Summoned::Unit)
            }
            Summon::Flora => {
                let h = self.hom_entry(owner.uid, owner.entity, owner.level);
                let open = (slv as usize).min(PlantKind::ALL.len());
                let next = self.homs[h].plant_next as usize % open;
                self.homs[h].plant_next = ((next + 1) % open) as u8;
                let kind = PlantKind::ALL[next];
                self.make_room(owner.uid, |k| matches!(k, Kind::Plant(_)), PLANT_CAP[open - 1], owner.tick);
                let Some(id) = self.alloc() else { return Summoned::Refused };
                let k = cantrip_k(owner.level).min(u8::MAX as u32) as u8;
                let hp = (owner.max_hp * PLANT_HP_PCT / 100).max(5);
                let mut c = Companion::blank(id, owner.uid, owner.entity, Kind::Plant(kind), x, y);
                c.map = self.map;
                (c.hp, c.max_hp, c.level, c.slv, c.k) = (hp, hp, owner.level.min(255) as u8, slv, k);
                (c.ac, c.to_hit, c.dc, c.save) = (10 + proficiency(owner.level) / 2, owner.to_hit, save_dc(owner.level), proficiency(owner.level) / 2);
                // Lifetime and pulses start with its first step (it knows
                // the tick rate).
                self.units.push(c);
                Summoned::Unit(c.entity())
            }
            Summon::SphereMine => {
                self.make_room(owner.uid, |k| k == Kind::Sphere, SPHERE_CAP, owner.tick);
                let Some(id) = self.alloc() else { return Summoned::Refused };
                let mut c = Companion::blank(id, owner.uid, owner.entity, Kind::Sphere, x, y);
                c.map = self.map;
                let hits = 3 + slv as i32;
                (c.hp, c.max_hp, c.level, c.slv) = (hits, hits, owner.level.min(255) as u8, slv);
                c.k = cantrip_k(owner.level).min(u8::MAX as u32) as u8;
                (c.ac, c.dc) = (8, save_dc(owner.level));
                self.units.push(c);
                Summoned::Unit(c.entity())
            }
        }
    }

    /// Call Homunculus: the chooser (no type yet), wake it, or the
    /// command key while out.
    fn call(&mut self, owner: Owner, x: f32, y: f32) -> Summoned {
        let h = self.hom_entry(owner.uid, owner.entity, owner.level);
        let hom = &mut self.homs[h];
        if hom.kind.is_none() {
            // The panel says "no type": the client opens the chooser.
            hom.sent = None;
            return Summoned::Refused;
        }
        if hom.dead {
            return Summoned::Refused;
        }
        if let Some(id) = hom.unit {
            hom.command = Some((x, y));
            return Summoned::Unit(COMPANION_ENTITY_BASE + id as u32);
        }
        hom.called = true;
        hom.resting = false;
        hom.sent = None;
        self.spawn_hom(h, x, y, owner.tick).map_or(Summoned::Refused, Summoned::Unit)
    }

    /// Removes the oldest of the owner's units of a kind until a new one
    /// fits under `cap`.
    fn make_room(&mut self, owner: u32, kind: impl Fn(Kind) -> bool, cap: usize, tick: u32) {
        loop {
            let n = self.units.iter().filter(|c| c.owner == owner && kind(c.kind)).count();
            if n < cap {
                return;
            }
            // Units are pushed in order: the first is the oldest.
            let Some(i) = self.units.iter().position(|c| c.owner == owner && kind(c.kind)) else { return };
            self.remove_at(i, tick);
        }
    }

    /// Puts the homunculus of entry `h` out at (`x`, `y`) (its first step
    /// moves it beside the owner). Returns its entity id.
    fn spawn_hom(&mut self, h: usize, x: f32, y: f32, tick: u32) -> Option<u32> {
        let t = self.homs[h].kind?;
        if let Some(id) = self.homs[h].unit {
            return Some(COMPANION_ENTITY_BASE + id as u32);
        }
        let id = self.alloc()?;
        let hom = &mut self.homs[h];
        let mut c = Companion::blank(id, hom.owner, hom.owner_entity, Kind::Homunculus(t), x, y);
        c.map = self.map;
        let (level, evolved) = (hom.save.level.max(1), hom.save.evolved);
        c.level = level;
        c.evolved = evolved;
        c.max_hp = hom_max_hp(t, level, evolved);
        c.hp = hom.hp.unwrap_or(c.max_hp).clamp(1, c.max_hp);
        c.max_sp = hom_max_sp(level);
        c.sp = hom.sp.clamp(0, c.max_sp);
        c.ac = hom_ac(t, level);
        c.save = proficiency(level as u32);
        c.to_hit = hom_to_hit(level, evolved);
        c.dc = save_dc(level as u32);
        c.k = cantrip_k(level as u32) as u8;
        c.cds = hom.cds;
        c.stance = hom.stance;
        c.think_at = tick;
        hom.unit = Some(id);
        hom.sent = None;
        self.units.push(c);
        Some(c.entity())
    }

    /// Removes unit `i`: its record goes out as `GONE`; a homunculus
    /// leaves its HP, SP and cooldowns with its owner's entry.
    fn remove_at(&mut self, i: usize, tick: u32) {
        let c = self.units.remove(i);
        if let Some(h) = self.homs.iter_mut().find(|h| h.unit == Some(c.id)) {
            h.unit = None;
            if c.alive() {
                h.hp = Some(c.hp);
                h.sp = c.sp;
            }
            h.cds = c.cds;
            h.sent = None;
        }
        let _ = tick;
        self.gone.push((c.map, CompanionInfo {
            entity_id: c.entity(),
            owner_entity_id: c.owner_entity,
            class: c.kind.class(),
            sprite: String::new(),
            name: String::new(),
            level: c.level,
            flags: companion_flags::GONE,
            x: c.x,
            y: c.y,
            expires_ms: 0,
        }));
    }

    /// Heal every companion inside `area` by `amount` (the Heal arm of
    /// `cast.rs act`; heals only, no buffs). Returns how many were healed;
    /// their numbers go out with the next combat step.
    pub fn heal_in(&mut self, area: &super::area::Area, amount: i32) -> usize {
        let mut n = 0;
        for c in self.units.iter_mut().filter(|c| c.alive() && c.kind != Kind::Sphere) {
            if area.contains(c.x, c.y, c.body()) {
                let h = amount.min(c.max_hp - c.hp).max(0);
                c.hp += h;
                n += 1;
                self.pending.push(HitEvent { target: c.entity(), amount: h as u32, flags: hit::HEAL });
            }
        }
        n
    }

    /// Potion Pitcher's homunculus target: the owner's homunculus, alive,
    /// within `r` of (`x`, `y`) (its entity id; then [`Self::heal_entity`]).
    #[cfg_attr(not(test), expect(dead_code, reason = "API for the skills side (cast.rs); only the tests call it yet"))]
    pub fn pitch_target(&self, owner: u32, x: f32, y: f32, r: f32) -> Option<u32> {
        self.units
            .iter()
            .find(|c| c.owner == owner && c.alive() && matches!(c.kind, Kind::Homunculus(_)) && dist(c.x, c.y, x, y) <= r + c.body())
            .map(Companion::entity)
    }

    /// Heals one companion (a pitched potion); returns what it took.
    #[cfg_attr(not(test), expect(dead_code, reason = "API for the skills side (cast.rs); only the tests call it yet"))]
    pub fn heal_entity(&mut self, entity: u32, amount: i32) -> i32 {
        let Some(c) = self.units.iter_mut().find(|c| c.entity() == entity && c.alive() && c.kind != Kind::Sphere) else { return 0 };
        let h = amount.min(c.max_hp - c.hp).max(0);
        c.hp += h;
        self.pending.push(HitEvent { target: entity, amount: h as u32, flags: hit::HEAL });
        h
    }

    /// Everything of `owner` leaves (the owner left the map).
    pub fn dismiss(&mut self, owner: u32) {
        while let Some(i) = self.units.iter().position(|c| c.owner == owner) {
            self.remove_at(i, 0);
        }
    }

    /// The owner arrives on the instance (`HubInstance::join`): their
    /// homunculus entry, from the save (a relog keeps the run's state).
    pub(super) fn arrive(&mut self, owner: u32, entity: u32, level: u32, save: Option<HomSave>, call_learned: bool) {
        match self.hom_index(owner) {
            Some(h) => {
                let hom = &mut self.homs[h];
                hom.away = false;
                hom.owner_entity = entity;
                hom.owner_level = level;
                hom.called |= hom.kind.is_some() && call_learned;
                hom.sent = None;
                hom.link_dead = 0;
            }
            None => self.homs.push(Hom::new(owner, entity, level, save, call_learned)),
        }
    }

    /// The owner left the instance: their units go; the save to report
    /// (if they have a homunculus).
    pub(super) fn depart(&mut self, owner: u32) -> Option<HomSave> {
        self.dismiss(owner);
        let h = self.hom_index(owner)?;
        let hom = &mut self.homs[h];
        hom.away = true;
        hom.focus = None;
        hom.command = None;
        hom.save_dirty = false;
        hom.kind.is_some().then(|| hom.save.clone())
    }

    /// The bonus experience of the owner's kill share (15 %, ×3 while far
    /// behind) while the homunculus is out and alive.
    pub(super) fn grant_exp(&mut self, owner: u32, share: u64) {
        let Some(h) = self.hom_index(owner) else { return };
        let Some(id) = self.homs[h].unit else { return };
        if !self.units.iter().any(|c| c.id == id && c.alive()) {
            return;
        }
        if self.homs[h].gain(share * EXP_SHARE_PCT / 100) {
            self.refit(h);
        }
    }

    /// The numbers of an out homunculus after a level or an evolution
    /// (full HP and SP, as for players).
    fn refit(&mut self, h: usize) {
        let hom = &self.homs[h];
        let (Some(t), Some(id)) = (hom.kind, hom.unit) else { return };
        let (level, evolved) = (hom.save.level.max(1), hom.save.evolved);
        let Some(c) = self.units.iter_mut().find(|c| c.id == id) else { return };
        c.level = level;
        c.evolved = evolved;
        c.max_hp = hom_max_hp(t, level, evolved);
        c.max_sp = hom_max_sp(level);
        if c.alive() {
            c.hp = c.max_hp;
            c.sp = c.max_sp;
        }
        c.ac = hom_ac(t, level);
        c.save = proficiency(level as u32);
        c.to_hit = hom_to_hit(level, evolved);
        c.dc = save_dc(level as u32);
        c.k = cantrip_k(level as u32) as u8;
        c.shown = false;
    }

    /// Bond for the homunculi of these owners that are out and alive
    /// (a boss, a cleared mission).
    pub(super) fn bond_out(&mut self, owners: &[u32], amount: u8) {
        for h in 0..self.homs.len() {
            let hom = &self.homs[h];
            let out = hom.unit.is_some_and(|id| self.units.iter().any(|c| c.id == id && c.alive()));
            if out && owners.contains(&hom.owner) {
                self.homs[h].bond(amount as i32);
            }
        }
    }

    /// A new wave: the dead homunculi come back at half HP (the players'
    /// rule, `fight.rs`).
    pub(super) fn new_wave(&mut self) {
        for hom in self.homs.iter_mut().filter(|h| h.dead && h.unit.is_none()) {
            let Some(t) = hom.kind else { continue };
            hom.dead = false;
            hom.hp = Some((hom_max_hp(t, hom.save.level, hom.save.evolved) / 2).max(1));
            hom.sent = None;
        }
    }

    /// What the owner's basic attack struck this tick (the homunculus
    /// helps there).
    pub(super) fn owner_swung(&mut self, owner: u32, slot: u16, generation: u8) {
        if let Some(h) = self.hom_index(owner) {
            self.homs[h].owner_target = Some((slot, generation));
        }
    }

    /// The lure a monster taunted by `taunter` strikes, when its body is in
    /// reach (`reach` past touching: the monster's radius plus its reach).
    pub(super) fn lure_in_reach(&self, taunter: u32, mx: f32, my: f32, reach: f32) -> Option<usize> {
        if taunter < LURE_BASE {
            return None;
        }
        let i = self.units.iter().position(|c| c.lure_id() == taunter && c.alive())?;
        let c = &self.units[i];
        let r = reach + c.body();
        (dist(c.x, c.y, mx, my) <= r).then_some(i)
    }

    /// The nearest live companion in reach (the opportunistic bite).
    pub(super) fn nearest_in_reach(&self, mx: f32, my: f32, reach: f32) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, c) in self.units.iter().enumerate() {
            if !c.alive() {
                continue;
            }
            let (dx, dy) = (c.x - mx, c.y - my);
            if dx.abs() > reach + c.body() || dy.abs() > reach + c.body() {
                continue;
            }
            let d = (dx * dx + dy * dy).sqrt();
            if d <= reach + c.body() && best.is_none_or(|(_, b)| d < b) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Unit `i`'s position, entity and armour (a monster's blow).
    pub(super) fn target_of(&self, i: usize, now: u32) -> (f32, f32, u32, i32) {
        let c = &self.units[i];
        (c.x, c.y, c.entity(), c.armour(now))
    }

    /// Unit `i` takes `amount` (a sphere counts hits, not HP). It dies at 0.
    pub(super) fn take(&mut self, i: usize, amount: i32, tick: u32) -> i32 {
        let c = &mut self.units[i];
        if !c.alive() {
            return 0;
        }
        let taken = if c.kind == Kind::Sphere { i32::from(amount > 0) } else { amount.max(0) };
        c.hp -= taken;
        if c.hp <= 0 {
            self.kill_unit(i, tick);
        }
        taken
    }

    fn kill_unit(&mut self, i: usize, tick: u32) {
        let c = &mut self.units[i];
        c.hp = 0;
        c.state = State::Dead;
        c.died_at = tick;
        c.target = None;
        if c.kind == Kind::Sphere {
            c.boom = true;
        }
        let (id, sacrificed) = (c.id, c.sacrificed);
        if let Some(h) = self.homs.iter_mut().find(|h| h.unit == Some(id)) {
            h.dead = true;
            h.focus = None;
            h.hp = Some(0);
            if !sacrificed {
                h.bond(-(BOND_DEATH as i32));
            }
            h.sent = None;
        }
    }

    /// A monster hazard (skill, affix, boss pattern) acting on every live
    /// companion inside `area`: a save against `dc` halves the damage.
    pub(super) fn hazard_damage(&mut self, area: &Area, dice: Dice, kind: DamageType, dc: i32, rng: &mut Rng, tick: u32, hits: &mut Vec<HitEvent>) {
        self.zone_damage(|x, y, r| area.contains(x, y, r), dice, 1.0, true, kind, dc, rng, tick, hits);
    }

    /// Damage on every live companion `inside` a zone (a boss beat, a
    /// hazard): `dice` × `boost`, a save against `dc` halves it when
    /// `save`.
    pub(super) fn zone_damage(
        &mut self,
        inside: impl Fn(f32, f32, f32) -> bool,
        dice: Dice,
        boost: f32,
        save: bool,
        kind: DamageType,
        dc: i32,
        rng: &mut Rng,
        tick: u32,
        hits: &mut Vec<HitEvent>,
    ) {
        for i in 0..self.units.len() {
            let c = &self.units[i];
            if !c.alive() || !inside(c.x, c.y, c.body()) {
                continue;
            }
            let saved = save && saving_throw(rng, c.save, dc, 0);
            let mut amount = (dice.roll(rng, false) as f32 * boost) as i32;
            if saved {
                amount /= 2;
            }
            let entity = c.entity();
            let taken = self.take(i, amount, tick);
            hits.push(HitEvent { target: entity, amount: taken as u32, flags: super::fight::damage_type_bits(kind) });
        }
    }

    /// A monster hazard's condition: the holding ones (stunned, frozen,
    /// petrified) stop a companion; the others do not stick to it.
    pub(super) fn hazard_condition(&mut self, area: &Area, condition: Condition, ticks: u16, save: bool, dc: i32, rng: &mut Rng, tick: u32) {
        if !matches!(condition, Condition::Stunned | Condition::Frozen | Condition::Petrified) {
            return;
        }
        for c in self.units.iter_mut().filter(|c| c.alive() && matches!(c.kind, Kind::Homunculus(_))) {
            if area.contains(c.x, c.y, c.body()) && (!save || !saving_throw(rng, c.save, dc, 0)) {
                c.held_until = c.held_until.max(tick + ticks as u32);
            }
        }
    }

    /// The map units summoned from now on stand on (the horde's).
    pub(super) fn set_map(&mut self, map: MapIdx) {
        self.map = map;
    }

    /// Rebuilds what the horde sees: every live companion.
    pub(super) fn refresh_lures(&mut self) {
        self.lures.clear();
        self.lures.extend(self.units.iter().filter(|c| c.alive()).map(|c| Lure { x: c.x, y: c.y, id: c.lure_id(), radius: c.body() }));
    }

    /// The `COMPANIONS` record of a unit.
    fn info(&self, c: &Companion, tick: u32, hz: f32) -> CompanionInfo {
        let (sprite, name) = match c.kind {
            Kind::Homunculus(t) => {
                let name = self.homs.iter().find(|h| h.unit == Some(c.id)).map_or_else(|| t.name().to_string(), Hom::name);
                (t.sprite(c.evolved), name)
            }
            Kind::Plant(p) => (p.sprite(), p.name().to_string()),
            Kind::Sphere => ("marine_sphere", "Marine Sphere".to_string()),
        };
        CompanionInfo {
            entity_id: c.entity(),
            owner_entity_id: c.owner_entity,
            class: c.kind.class(),
            sprite: sprite.into(),
            name,
            level: c.level,
            flags: if c.evolved { companion_flags::EVOLVED } else { 0 },
            x: c.x,
            y: c.y,
            expires_ms: if c.expires > tick { ms(c.expires - tick, hz) } else { 0 },
        }
    }

    fn vitals(c: &Companion) -> ServerMsg {
        ServerMsg::PlayerVitals {
            entity_id: c.entity(),
            hp: c.hp.max(0) as u32,
            max_hp: c.max_hp.max(1) as u32,
            sp: c.sp.max(0) as u32,
            max_sp: c.max_sp.max(0) as u32,
            state: if c.alive() { 0 } else { 2 },
            status: 0,
            temp_hp: 0,
            light: 0,
            level: c.level as u32,
        }
    }

    /// The owner's panel.
    fn panel(&self, h: &Hom, tick: u32, hz: f32) -> CompanionState {
        let unit = h.unit.and_then(|id| self.units.iter().find(|c| c.id == id));
        let Some(t) = h.kind else { return CompanionState { stance: h.stance.wire(), ..CompanionState::default() } };
        let level = h.save.level.max(1);
        let cds = unit.map_or(h.cds, |c| c.cds);
        let (sp, max_sp) = unit.map_or((h.sp.clamp(0, hom_max_sp(level)), hom_max_sp(level)), |c| (c.sp, c.max_sp));
        let state = if h.dead || unit.is_some_and(|c| !c.alive()) {
            companion_state::DEAD
        } else if h.resting {
            companion_state::RESTING
        } else {
            companion_state::OUT
        };
        let _ = t;
        CompanionState {
            hom_type: t.wire(),
            level,
            exp: h.save.exp.min(u32::MAX as u64) as u32,
            exp_to_next: exp_to_next(level as u32).min(u32::MAX as u64) as u32,
            bond: h.save.bond,
            stance: h.stance.wire(),
            state,
            back_ms: 0,
            sp: sp.max(0) as u32,
            max_sp: max_sp.max(0) as u32,
            cooldowns_ms: cds.map(|r| if r > tick { ms(r - tick, hz) } else { 0 }),
        }
    }

    /// Places, spawns and recalls homunculi for the players on the map:
    /// out while the owner is here (link-dead at most 5 s), it is called,
    /// not resting, not dead.
    fn place_homunculi(&mut self, cx: &StepCx<'_>) {
        let limit = ticks(LINK_DEAD_SECS, cx.hz);
        for h in 0..self.homs.len() {
            let ally = cx.allies.iter().find(|a| a.uid == self.homs[h].owner).copied();
            let hom = &mut self.homs[h];
            if let Some(a) = ally {
                hom.owner_level = a.level;
                hom.owner_entity = a.entity;
                hom.link_dead = if a.link_dead { hom.link_dead + 1 } else { 0 };
            }
            let want = hom.kind.is_some() && hom.called && !hom.resting && !hom.dead && !hom.away && ally.is_some() && hom.link_dead <= limit;
            match (hom.unit, want) {
                (Some(id), false) => {
                    if let Some(i) = self.units.iter().position(|c| c.id == id && c.alive()) {
                        self.remove_at(i, cx.tick);
                    }
                }
                (None, true) => {
                    let a = ally.expect("wanted out with its owner here");
                    self.spawn_hom(h, a.x, a.y, cx.tick);
                }
                _ => {}
            }
        }
    }
}

/// One combat tick of every companion: homunculi placed, think (5 Hz),
/// move, attack, cast; plants and spheres act; expired and dead rows go.
/// Runs inside `fight.rs` after the players' attacks (step 1b).
pub(super) fn step(cs: &mut Companions, cx: &mut StepCx<'_>) {
    cx.hits.append(&mut cs.pending);
    cs.place_homunculi(cx);
    let mut threats = std::mem::take(&mut cs.threats);
    let mut buf = std::mem::take(&mut cs.buf);
    let mut i = 0;
    while i < cs.units.len() {
        let keep = match cs.units[i].kind {
            Kind::Homunculus(t) => hom_tick(cs, i, t, cx, &mut threats, &mut buf),
            Kind::Plant(p) => plant_tick(cs, i, p, cx, &mut buf),
            Kind::Sphere => sphere_tick(cs, i, cx, &mut buf),
        };
        if keep {
            i += 1;
        } else {
            cs.remove_at(i, cx.tick);
        }
    }
    cs.killed.append(&mut cs.sink.killed);
    cs.refresh_lures();
    cs.threats = threats;
    cs.buf = buf;
}

/// SP back over time (full in ~40 s), the blow's cooldown.
fn timers(c: &mut Companion, hz: f32) {
    c.attack_cd = c.attack_cd.saturating_sub(1);
    if c.sp < c.max_sp {
        c.regen += c.max_sp.max(40) as f32 / 40.0 / hz;
        let n = c.regen.floor();
        c.sp = (c.sp + n as i32).min(c.max_sp);
        c.regen -= n;
    }
}

fn hom_tick(cs: &mut Companions, i: usize, t: HomType, cx: &mut StepCx<'_>, threats: &mut Vec<Threat>, buf: &mut Vec<u16>) -> bool {
    let mut c = cs.units[i];
    let tick = cx.tick;
    let Some(h) = cs.hom_index(c.owner) else { return false };
    if !c.alive() {
        // The death animation, then the row goes (back on the next wave).
        return tick < c.died_at + ticks(DEAD_SECS, cx.hz);
    }
    let Some(owner) = cx.allies.iter().find(|a| a.uid == c.owner).copied() else { return false };
    timers(&mut c, cx.hz);
    if !c.placed {
        (c.x, c.y) = beside(&owner, cx.walk);
        c.placed = true;
        c.shown = false;
    }
    if tick < c.held_until {
        cs.units[i] = c;
        return true;
    }
    // Back beside the owner: fell behind (a blink, a portal), stuck, or
    // out of line too long.
    let hom = &mut cs.homs[h];
    if dist(c.x, c.y, owner.x, owner.y) > TELEPORT_FAR || c.stuck as u32 >= ticks(STUCK_SECS, cx.hz) || hom.no_line >= ticks(NO_LINE_SECS, cx.hz) {
        (c.x, c.y) = beside(&owner, cx.walk);
        c.stuck = 0;
        hom.no_line = 0;
        c.target = None;
        if matches!(c.state, State::Cast(_)) {
            c.state = State::Follow;
        }
    }
    let period = (cx.hz.round() as u32 / THINK_HZ).max(1);
    let command = hom.command.is_some();
    if (command || (tick + c.id as u32).is_multiple_of(period)) && !matches!(c.state, State::Cast(_)) {
        think(&mut c, hom, t, &owner, cx, threats, buf, &mut cs.sink, period);
        c.think_at = tick + period;
    }
    act(&mut c, &mut cs.homs[h], t, &owner, cx, buf, &mut cs.sink);
    cs.units[i] = c;
    if !cs.units[i].alive() {
        // Self-Destruction: it went with the blast.
        let died = cs.units[i].died_at;
        cs.units[i].died_at = died.min(tick);
    }
    true
}

/// Thinking (5 Hz): the line to the owner, the command key, threats,
/// target with hysteresis, state, and the role's skills.
fn think(c: &mut Companion, hom: &mut Hom, t: HomType, owner: &Ally, cx: &mut StepCx<'_>, threats: &mut Vec<Threat>, buf: &mut Vec<u16>, sink: &mut Sink, period: u32) {
    let tick = cx.tick;
    if line_clear(cx.walk, c.x, c.y, owner.x, owner.y) {
        hom.no_line = 0;
    } else {
        hom.no_line += period;
    }
    if let Some((ax, ay)) = hom.command.take() {
        let pick = cx
            .horde
            .nearest_foe(ax, ay, FOCUS_PICK)
            .map(|(s, _)| s)
            .filter(|&s| {
                let st = cx.horde.store();
                dist(st.x[s as usize], st.y[s as usize], owner.x, owner.y) <= FOCUS_RANGE
            });
        match pick {
            Some(s) => {
                let g = cx.horde.store().generation[s as usize];
                hom.focus = Some((s, g, tick + ticks(FOCUS_SECS, cx.hz)));
                hom.recall_until = 0;
                c.target = Some((s, g));
            }
            None => {
                hom.focus = None;
                hom.recall_until = tick + ticks(RECALL_SECS, cx.hz);
                c.target = None;
                if c.state != State::Retreat {
                    c.state = State::Follow;
                }
            }
        }
    }
    hom.focus.take_if(|&mut (s, g, until)| tick >= until || !live(cx.horde, s, g));
    hom.owner_target.take_if(|&mut (s, g)| !live(cx.horde, s, g));
    gather(c, hom, t, owner, cx, threats, buf);
    pick_target(c, hom, t, threats);
    if tick < hom.recall_until {
        c.target = None;
    }
    // State.
    let hurt = c.hp * 10 < c.max_hp * 3;
    if c.state == State::Retreat && c.hp * 2 > c.max_hp {
        c.state = State::Follow;
    }
    if t == HomType::Filir && hurt {
        c.state = State::Retreat;
    }
    if c.state != State::Retreat {
        c.state = match (c.target, t) {
            (Some(_), HomType::Vanilmirth) => State::Kite,
            (Some(_), _) => State::Engage,
            (None, _) => State::Follow,
        };
    }
    c.goal = None;
    role(c, hom, t, owner, cx, threats, buf, sink);
}

/// Threats: monsters within the stance's radius of the owner with a clear
/// line from the owner (plus the focus, wherever it is within range).
fn gather(c: &Companion, hom: &Hom, t: HomType, owner: &Ally, cx: &StepCx<'_>, threats: &mut Vec<Threat>, buf: &mut Vec<u16>) {
    threats.clear();
    let stance = if owner.up { c.stance } else { Stance::Defensive };
    let r = if owner.up { stance.radius() } else { stance.radius().min(DOWNED_HOLD) };
    let span = if hom.focus.is_some() { r.max(FOCUS_RANGE) } else { r };
    if span <= 0.0 {
        return;
    }
    let horde = &*cx.horde;
    buf.clear();
    horde.near_box(owner.x - span, owner.y - span, owner.x + span, owner.y + span, buf);
    let store = horde.store();
    // Nearest first, so the cap keeps the ones that matter.
    let d2 = |s: u16| (store.x[s as usize] - owner.x).powi(2) + (store.y[s as usize] - owner.y).powi(2);
    buf.sort_by(|&a, &b| d2(a).total_cmp(&d2(b)).then(a.cmp(&b)));
    for &s in buf.iter() {
        let i = s as usize;
        if store.is_plant(s) || store.hp[i] <= 0 {
            continue;
        }
        let generation = store.generation[i];
        let focused = hom.focus.is_some_and(|(fs, fg, _)| fs == s && fg == generation);
        let (x, y) = (store.x[i], store.y[i]);
        let radius = horde.catalog().get(store.kind[i]).radius;
        let d_owner = dist(x, y, owner.x, owner.y);
        if !focused && d_owner > r + radius {
            continue;
        }
        if !line_clear(cx.walk, owner.x, owner.y, x, y) {
            continue;
        }
        let def = horde.catalog().get(store.kind[i]);
        let big = matches!(def.archetype, combat::Archetype::Elite | combat::Archetype::Boss) || store.affix[i] != 0;
        let hp_frac = if t == HomType::Filir { (store.hp[i] as f32 / horde.block(s).hp.max(1) as f32).clamp(0.0, 1.0) } else { 1.0 };
        threats.push(Threat {
            slot: s,
            generation,
            x,
            y,
            radius,
            d_owner,
            d_self: dist(x, y, c.x, c.y),
            touch: d_owner - radius - HOM_BODY <= TOUCH_GAP,
            hp_frac,
            big,
        });
        if threats.len() >= MAX_THREATS {
            break;
        }
    }
}

/// `score(m) = 3·touches owner + 2·(focus or the owner's target) +
/// 1.5/(1 + dist to owner) + role − 0.1·dist to itself`; the current target
/// stays unless the best beats it by [`HYSTERESIS`]. The focus always wins.
fn pick_target(c: &mut Companion, hom: &Hom, t: HomType, threats: &[Threat]) {
    let mut best: Option<(usize, f32)> = None;
    let mut current: Option<f32> = None;
    for (k, m) in threats.iter().enumerate() {
        let id = Some((m.slot, m.generation));
        let focused = hom.focus.is_some_and(|(s, g, _)| (s, g) == (m.slot, m.generation));
        // Lif guards its owner: only what comes within 2.5 of them.
        if t == HomType::Lif && !focused && m.d_owner - m.radius > 2.5 {
            continue;
        }
        let role = match t {
            HomType::Filir => 2.0 * (1.0 - m.hp_frac),
            HomType::Amistr => f32::from(u8::from(m.touch)),
            HomType::Lif | HomType::Vanilmirth => 0.0,
        };
        let mine = focused || hom.owner_target == id;
        let score = 3.0 * f32::from(u8::from(m.touch)) + 2.0 * f32::from(u8::from(mine)) + 1.5 / (1.0 + m.d_owner) + role - 0.1 * m.d_self
            + if focused { 100.0 } else { 0.0 };
        if c.target == id {
            current = Some(score);
        }
        if best.is_none_or(|(_, b)| score > b) {
            best = Some((k, score));
        }
    }
    c.target = match (best, current) {
        (None, _) => None,
        (Some((_, b)), Some(cur)) if b - cur <= (cur.abs() * (HYSTERESIS - 1.0)).max(0.05) => c.target,
        (Some((k, _)), _) => Some((threats[k].slot, threats[k].generation)),
    };
}

/// Whether skill `slot` can go now.
fn ready(c: &Companion, t: HomType, slot: usize, tick: u32) -> bool {
    let s = &t.skills()[slot];
    let open = if s.unlock == 0 { c.evolved } else { c.level >= s.unlock };
    open && !s.passive && tick >= c.cds[slot] && c.sp >= s.sp
}

/// Starts skill `slot`: SP and cooldown spent, `CAST` to the map; it
/// lands when its wind-up is over (at once for instant ones).
fn begin(c: &mut Companion, t: HomType, slot: usize, aim: (f32, f32), target: Option<(u16, u8)>, ally: u32, areas: Vec<Telegraph>, cx: &StepCx<'_>, sink: &mut Sink) {
    let s = &t.skills()[slot];
    let tick = cx.tick;
    c.sp -= s.sp;
    c.cds[slot] = tick + ticks(s.cooldown, cx.hz);
    c.aim = aim;
    c.cast_target = target;
    c.cast_ally = ally;
    let wind = if s.cast > 0.0 { ticks(s.cast, cx.hz) } else { 0 };
    c.cast_at = tick + wind;
    if !matches!(c.state, State::Cast(_) | State::Dead) {
        c.after = c.state;
    }
    c.state = State::Cast(slot as u8);
    if aim != (c.x, c.y) {
        c.facing = facing_of(aim.0 - c.x, aim.1 - c.y);
    }
    sink.casts.push(ServerMsg::Cast {
        caster: c.entity(),
        skill: s.id.into(),
        x: aim.0,
        y: aim.1,
        cast_ms: ms(wind, cx.hz),
        cooldown_ms: ms(c.cds[slot] - tick, cx.hz),
        areas,
        level: c.level,
    });
}

/// The ally (user id) most in need of a heal under `frac` of their HP
/// within `r` of (`x`, `y`): the owner first.
fn wounded(allies: &[Ally], owner: &Ally, x: f32, y: f32, r: f32, frac: f32) -> Option<u32> {
    if owner.up && (owner.hp as f32) < owner.max_hp as f32 * frac {
        return Some(owner.uid);
    }
    allies
        .iter()
        .filter(|a| a.up && a.uid != owner.uid && dist(a.x, a.y, x, y) <= r && (a.hp as f32) < a.max_hp as f32 * frac)
        .min_by(|a, b| (a.hp as f32 / a.max_hp.max(1) as f32).total_cmp(&(b.hp as f32 / b.max_hp.max(1) as f32)))
        .map(|a| a.uid)
}

/// The role's skills (design §3 "AI"): at most one starts per think.
fn role(c: &mut Companion, hom: &Hom, t: HomType, owner: &Ally, cx: &mut StepCx<'_>, threats: &[Threat], buf: &mut Vec<u16>, sink: &mut Sink) {
    let tick = cx.tick;
    let me = (c.x, c.y);
    let target = c.target.and_then(|id| threats.iter().find(|m| (m.slot, m.generation) == id).copied());
    let passive = c.stance == Stance::Passive && owner.up;
    match t {
        HomType::Lif => {
            if ready(c, t, 0, tick) {
                let heal = wounded(cx.allies, owner, c.x, c.y, 6.0, 0.6);
                let me_hurt = c.hp * 2 < c.max_hp;
                if let Some(uid) = heal.or(me_hurt.then_some(0)) {
                    let at = cx.allies.iter().find(|a| a.uid == uid).map_or(me, |a| (a.x, a.y));
                    begin(c, t, 0, at, None, uid, Vec::new(), cx, sink);
                    return;
                }
            }
            if ready(c, t, 1, tick) && owner.up && (count_near(cx.horde, owner.x, owner.y, 3.0, false, buf) >= 6 || owner.hp * 100 < owner.max_hp * 35) {
                begin(c, t, 1, (owner.x, owner.y), None, owner.uid, Vec::new(), cx, sink);
                return;
            }
            if ready(c, t, 2, tick) && target.is_some() && threats.len() >= 3 {
                begin(c, t, 2, me, None, 0, Vec::new(), cx, sink);
            }
        }
        HomType::Amistr => {
            // Its guard point: between the owner and the densest threat
            // within 5 of them.
            let (mut sx, mut sy, mut n) = (0.0, 0.0, 0);
            for m in threats.iter().filter(|m| m.d_owner <= 5.0) {
                sx += m.x;
                sy += m.y;
                n += 1;
            }
            if n > 0 && let Some((ux, uy)) = unit_vec(sx / n as f32 - owner.x, sy / n as f32 - owner.y) {
                c.goal = Some((owner.x + ux * 1.5, owner.y + uy * 1.5));
            }
            let touching = threats.iter().filter(|m| m.touch).count();
            if ready(c, t, 2, tick) && owner.up && owner.hp * 10 < owner.max_hp * 3 && touching >= 3 && c.hp * 2 > c.max_hp {
                begin(c, t, 2, (owner.x, owner.y), None, owner.uid, Vec::new(), cx, sink);
                return;
            }
            if !passive && ready(c, t, 0, tick) && count_near(cx.horde, c.x, c.y, 3.0, true, buf) >= 1 {
                let area = Area::Circle { x: c.x, y: c.y, r: 3.0 }.telegraph(0, look::TAUNT, 300, 0);
                begin(c, t, 0, me, None, 0, vec![area], cx, sink);
                return;
            }
            if ready(c, t, 1, tick) && (count_near(cx.horde, c.x, c.y, 3.0, false, buf) >= 5 || c.hp * 2 < c.max_hp) {
                begin(c, t, 1, me, None, owner.uid, Vec::new(), cx, sink);
                return;
            }
            if ready(c, t, 3, tick) && target.is_some() && c.hp * 10 < c.max_hp * 7 {
                begin(c, t, 3, me, None, 0, Vec::new(), cx, sink);
            }
        }
        HomType::Filir => {
            let in_reach = target.filter(|m| m.d_self - m.radius - HOM_BODY <= MELEE_GAP);
            if c.state != State::Retreat
                && let Some(m) = in_reach
            {
                if ready(c, t, 3, tick) && hom.save.bond >= BOND_SBR && m.big {
                    begin(c, t, 3, (m.x, m.y), Some((m.slot, m.generation)), 0, Vec::new(), cx, sink);
                    return;
                }
                if ready(c, t, 0, tick) {
                    begin(c, t, 0, (m.x, m.y), Some((m.slot, m.generation)), 0, Vec::new(), cx, sink);
                    return;
                }
            }
            if ready(c, t, 2, tick) && c.hp * 10 < c.max_hp * 6 && count_near(cx.horde, c.x, c.y, 2.5, false, buf) >= 1 {
                begin(c, t, 2, me, None, 0, Vec::new(), cx, sink);
                return;
            }
            if ready(c, t, 1, tick) && target.is_some() && threats.len() >= 3 && c.state != State::Retreat {
                begin(c, t, 1, me, None, 0, Vec::new(), cx, sink);
            }
        }
        HomType::Vanilmirth => {
            if ready(c, t, 3, tick) && c.hp * 100 < c.max_hp * 15 && count_near(cx.horde, c.x, c.y, 3.0, false, buf) >= 6 {
                let area = Area::Circle { x: c.x, y: c.y, r: 3.0 }.telegraph(0, look::FIRE, 1000, 0);
                begin(c, t, 3, me, None, 0, vec![area], cx, sink);
                return;
            }
            if ready(c, t, 1, tick) && (owner.up && owner.hp * 10 < owner.max_hp * 7 || c.hp * 10 < c.max_hp * 7 || wounded(cx.allies, owner, c.x, c.y, 8.0, 0.7).is_some()) {
                begin(c, t, 1, me, None, 0, Vec::new(), cx, sink);
                return;
            }
            if ready(c, t, 2, tick) && threats.len() >= 4 {
                begin(c, t, 2, me, None, 0, Vec::new(), cx, sink);
                return;
            }
            if ready(c, t, 0, tick) && target.is_some() {
                // Caprice at the densest knot in range.
                let knot = threats
                    .iter()
                    .filter(|m| m.d_self - m.radius <= BOLT_RANGE && line_clear(cx.walk, c.x, c.y, m.x, m.y))
                    .max_by_key(|m| threats.iter().filter(|o| dist(o.x, o.y, m.x, m.y) <= 1.5).count());
                if let Some(m) = knot {
                    let area = Area::Circle { x: m.x, y: m.y, r: 1.5 }.telegraph(0, look::LIGHTNING, 400, 0);
                    begin(c, t, 0, (m.x, m.y), Some((m.slot, m.generation)), 0, vec![area], cx, sink);
                }
            }
        }
    }
}

/// Acting every tick: the skill landing, moving to the state's goal,
/// striking on cooldown.
fn act(c: &mut Companion, hom: &mut Hom, t: HomType, owner: &Ally, cx: &mut StepCx<'_>, buf: &mut Vec<u16>, sink: &mut Sink) {
    let tick = cx.tick;
    if let Some((s, g)) = c.target
        && !live(cx.horde, s, g)
    {
        c.target = None;
        if matches!(c.state, State::Engage | State::Kite) {
            c.state = State::Follow;
        }
    }
    let speed = owner.step * SPEED_FACTOR * if c.buffs.speed > tick { 1.3 } else { 1.0 };
    let leash = if owner.up { LEASH } else { DOWNED_HOLD };
    match c.state {
        State::Dead => {}
        State::Cast(slot) => {
            if tick >= c.cast_at {
                c.state = c.after;
                land(c, hom, t, slot as usize, owner, cx, buf, sink);
            }
        }
        State::Retreat => walk_to(c, follow_slot(owner, cx.walk), speed, FOLLOW_SLACK, cx.walk),
        State::Follow => {
            let goal = leashed(c.goal.unwrap_or_else(|| follow_slot(owner, cx.walk)), owner, leash);
            walk_to(c, goal, speed, FOLLOW_SLACK, cx.walk);
        }
        State::Engage => {
            let Some((s, _)) = c.target else { return };
            let st = cx.horde.store();
            let (tx, ty) = (st.x[s as usize], st.y[s as usize]);
            let radius = cx.horde.catalog().get(st.kind[s as usize]).radius;
            if dist(tx, ty, owner.x, owner.y) > leash + radius + MELEE_GAP + HOM_BODY && !hom.focus.is_some_and(|f| f.0 == s) {
                // Past the leash: let it go.
                c.target = None;
                c.state = State::Follow;
                return;
            }
            let gap = dist(tx, ty, c.x, c.y) - radius - HOM_BODY;
            if gap <= MELEE_GAP {
                if c.attack_cd == 0 {
                    strike(c, t, s, tx, ty, cx, sink);
                }
            } else {
                // Amistr closes in on the owner's side of it; the others
                // straight at it.
                let from = if t == HomType::Amistr { (owner.x, owner.y) } else { (c.x, c.y) };
                let (ux, uy) = unit_vec(from.0 - tx, from.1 - ty).unwrap_or((0.0, 0.0));
                let stand = radius + HOM_BODY + MELEE_GAP * 0.5;
                let goal = leashed((tx + ux * stand, ty + uy * stand), owner, leash + radius + MELEE_GAP + HOM_BODY);
                walk_to(c, goal, speed, 0.05, cx.walk);
            }
        }
        State::Kite => {
            let Some((s, _)) = c.target else { return };
            let st = cx.horde.store();
            let (tx, ty) = (st.x[s as usize], st.y[s as usize]);
            let radius = cx.horde.catalog().get(st.kind[s as usize]).radius;
            let d = dist(tx, ty, c.x, c.y);
            if d < KITE_FLEE {
                // Step away, blended toward the owner.
                let away = unit_vec(c.x - tx, c.y - ty).unwrap_or((1.0, 0.0));
                let home = unit_vec(owner.x - c.x, owner.y - c.y).unwrap_or((0.0, 0.0));
                let (dx, dy) = (away.0 * 0.6 + home.0 * 0.4, away.1 * 0.6 + home.1 * 0.4);
                let (ux, uy) = unit_vec(dx, dy).unwrap_or(away);
                let goal = leashed((c.x + ux * 2.0, c.y + uy * 2.0), owner, leash);
                walk_to(c, goal, speed, 0.05, cx.walk);
            } else if d > KITE_FAR {
                let (ux, uy) = unit_vec(c.x - tx, c.y - ty).unwrap_or((0.0, 0.0));
                let goal = leashed((tx + ux * (KITE_NEAR + KITE_FAR) / 2.0, ty + uy * (KITE_NEAR + KITE_FAR) / 2.0), owner, leash);
                walk_to(c, goal, speed, 0.05, cx.walk);
            }
            if c.attack_cd == 0 && d - radius <= BOLT_RANGE && line_clear(cx.walk, c.x, c.y, tx, ty) {
                strike(c, t, s, tx, ty, cx, sink);
            }
        }
    }
}

/// The basic attack on monster `s`.
fn strike(c: &mut Companion, t: HomType, s: u16, tx: f32, ty: f32, cx: &mut StepCx<'_>, sink: &mut Sink) {
    let tick = cx.tick;
    let kind = if t.ranged() { ELEMENTS[(cx.rng.die(ELEMENTS.len() as u32) - 1) as usize] } else { DamageType::Physical };
    let dealt = blow(c, s, hom_dice(t, c.level), kind, t.ranged(), false, Source::Homunculus, cx, sink);
    if c.buffs.lust > tick && dealt > 0 {
        let back = (dealt * 3 / 10).max(1).min(c.max_hp - c.hp);
        if back > 0 {
            c.hp += back;
            cx.hits.push(HitEvent { target: c.entity(), amount: back as u32, flags: hit::HEAL });
        }
    }
    swing(c, tx, ty, s, cx.swings);
    let quick = if c.buffs.haste > tick { 0.7 } else { 1.0 } * if t == HomType::Lif && c.buffs.boost > tick { 0.7 } else { 1.0 };
    c.attack_cd = ((hom_interval_ms(t) as f32 * quick * cx.hz / 1000.0).round() as u16).max(1);
}

/// A skill lands.
fn land(c: &mut Companion, hom: &mut Hom, t: HomType, slot: usize, owner: &Ally, cx: &mut StepCx<'_>, buf: &mut Vec<u16>, sink: &mut Sink) {
    let tick = cx.tick;
    let hz = cx.hz;
    let level = c.level as u32;
    let k = cantrip_k(level);
    let p = proficiency(level);
    let heal_to = |c: &mut Companion, uid: u32, amount: i32, cure: bool, cx: &mut StepCx<'_>| {
        if uid == 0 {
            let h = amount.min(c.max_hp - c.hp).max(0);
            c.hp += h;
            cx.hits.push(HitEvent { target: c.entity(), amount: h as u32, flags: hit::HEAL });
        } else {
            cx.fx.push(AllyFx::Heal { uid, amount, cure });
        }
    };
    match (t, slot) {
        (HomType::Lif, 0) => {
            let mut amount = Dice::new(k, 8, p).roll(cx.rng, false);
            if c.buffs.boost > tick {
                amount = amount * 3 / 2;
            }
            // Sanctified Touch (evolved): the heal also cures.
            heal_to(c, c.cast_ally, amount, c.evolved, cx);
        }
        (HomType::Lif, 1) => {
            let secs = ticks(6.0, hz);
            cx.fx.push(AllyFx::Buff { uid: owner.uid, skill: BUFF_ESCAPE, ticks: secs, mods: Mods { speed: 0.3, ..Mods::default() } });
            c.buffs.speed = tick + secs;
        }
        (HomType::Lif, 2) | (HomType::Vanilmirth, 2) => c.buffs.boost = tick + ticks(10.0, hz),
        (HomType::Amistr, 0) => {
            let n = 3 + level as usize / 20;
            taunt(cx.horde, c.x, c.y, 3.0, n, c.lure_id(), ticks(4.0, hz).min(u16::MAX as u32) as u16, buf);
        }
        (HomType::Amistr, 1) => {
            let secs = ticks(10.0, hz);
            c.buffs.guard = tick + secs;
            cx.fx.push(AllyFx::Buff { uid: owner.uid, skill: BUFF_BULWARK, ticks: secs, mods: Mods { ac: 4, ..Mods::default() } });
        }
        (HomType::Amistr, 2) => {
            // Castling: swap places, and what stood around the owner turns
            // on the Amistr.
            let (ox, oy) = (owner.x, owner.y);
            cx.fx.push(AllyFx::Move { uid: owner.uid, x: c.x, y: c.y });
            (c.x, c.y) = (ox, oy);
            taunt(cx.horde, ox, oy, 2.5, 6, c.lure_id(), ticks(4.0, hz).min(u16::MAX as u32) as u16, buf);
        }
        (HomType::Amistr, 3) => c.buffs.lust = tick + ticks(12.0, hz),
        (HomType::Filir, 0) => {
            if let Some((s, g)) = c.cast_target
                && live(cx.horde, s, g)
            {
                let (tx, ty) = (cx.horde.store().x[s as usize], cx.horde.store().y[s as usize]);
                for _ in 0..3 {
                    if !live(cx.horde, s, g) {
                        break;
                    }
                    blow(c, s, hom_dice(t, c.level), DamageType::Physical, false, false, Source::Homunculus, cx, sink);
                }
                swing(c, tx, ty, s, cx.swings);
            }
        }
        (HomType::Filir, 1) => c.buffs.haste = tick + ticks(8.0, hz),
        (HomType::Filir, 2) => c.buffs.dodge = tick + ticks(8.0, hz),
        (HomType::Filir, 3) => {
            if let Some((s, g)) = c.cast_target
                && live(cx.horde, s, g)
            {
                let base = hom_dice(t, c.level);
                let dice = Dice::new(base.count * 4, base.sides, hom.save.bond as i32 / 4);
                let (tx, ty) = (cx.horde.store().x[s as usize], cx.horde.store().y[s as usize]);
                blow(c, s, dice, DamageType::Physical, false, true, Source::Homunculus, cx, sink);
                swing(c, tx, ty, s, cx.swings);
                hom.bond(-(BOND_SBR as i32));
            }
        }
        (HomType::Vanilmirth, 0) => {
            let (ax, ay) = c.aim;
            monsters_in(cx.horde, ax, ay, 1.5, buf);
            let targets: [u16; 16] = {
                let mut a = [u16::MAX; 16];
                for (j, &m) in buf.iter().take(16).enumerate() {
                    a[j] = m;
                }
                a
            };
            for &m in targets.iter().filter(|&&m| m != u16::MAX) {
                let kind = ELEMENTS[(cx.rng.die(ELEMENTS.len() as u32) - 1) as usize];
                blow(c, m, Dice::new(k, 6, 0), kind, true, false, Source::Homunculus, cx, sink);
            }
        }
        (HomType::Vanilmirth, 1) => {
            // Chaotic Blessings: half the time the owner, else itself, else
            // the most wounded ally.
            let amount = Dice::new(k, 8, p).roll(cx.rng, false);
            let roll = cx.rng.die(100);
            let other = wounded(cx.allies, owner, c.x, c.y, 8.0, 1.0).filter(|&u| u != owner.uid);
            let uid = if roll <= 50 && owner.up {
                owner.uid
            } else if roll <= 80 || other.is_none() {
                0
            } else {
                other.unwrap_or(0)
            };
            heal_to(c, uid, amount, false, cx);
        }
        (HomType::Vanilmirth, 3) => {
            // Self-Destruction: its whole max HP as force on what is near;
            // it goes with the blast, at no Bond.
            monsters_in(cx.horde, c.x, c.y, 3.0, buf);
            let n = buf.len().min(32);
            let mut slots = [u16::MAX; 32];
            slots[..n].copy_from_slice(&buf[..n]);
            for &m in &slots[..n] {
                blow(c, m, Dice::new(0, 1, c.max_hp), DamageType::Force, true, false, Source::Homunculus, cx, sink);
            }
            c.sacrificed = true;
            c.hp = 0;
            c.state = State::Dead;
            c.died_at = tick;
            hom.dead = true;
            hom.hp = Some(0);
            hom.focus = None;
            hom.sent = None;
        }
        _ => {}
    }
}

fn plant_tick(cs: &mut Companions, i: usize, p: PlantKind, cx: &mut StepCx<'_>, buf: &mut Vec<u16>) -> bool {
    let mut c = cs.units[i];
    let tick = cx.tick;
    if !c.alive() {
        return tick < c.died_at + ticks(DEAD_SECS, cx.hz);
    }
    if !c.placed {
        place_rooted(&mut c, cx);
        c.expires = tick + ticks(PLANT_SECS, cx.hz);
        c.pulse_at = tick + ticks(GEO_SECS, cx.hz);
    }
    if c.expires != 0 && tick >= c.expires {
        return false;
    }
    c.attack_cd = c.attack_cd.saturating_sub(1);
    if p == PlantKind::Geographer && tick >= c.pulse_at {
        c.pulse_at = tick + ticks(GEO_SECS, cx.hz);
        geographer(cs, &c, cx);
    }
    if c.attack_cd == 0 {
        let (reach, sides, kind, rider) = p.attack();
        match cx.horde.nearest_foe(c.x, c.y, reach + c.body()) {
            Some((s, _)) => {
                let (tx, ty) = (cx.horde.store().x[s as usize], cx.horde.store().y[s as usize]);
                let dealt = blow(&c, s, Dice::new(c.k.max(1) as u32, sides, 0), kind, false, false, Source::Plant, cx, &mut cs.sink);
                if let Some(cond) = rider
                    && dealt > 0
                    && live(cx.horde, s, cx.horde.store().generation[s as usize])
                {
                    let block = cx.horde.block(s);
                    if !saving_throw(cx.rng, block.save, c.dc, 0) {
                        super::cast::inflict(cx.horde, s, cond, ticks(PLANT_COND_SECS, cx.hz).min(u16::MAX as u32) as u16, c.k as u32);
                    }
                }
                swing(&mut c, tx, ty, s, cx.swings);
                c.attack_cd = ticks(PLANT_SWING_SECS, cx.hz).min(u16::MAX as u32) as u16;
            }
            // Nothing in reach: look again in a moment.
            None => c.attack_cd = 2,
        }
    }
    let _ = buf;
    cs.units[i] = c;
    true
}

/// A rooted companion's first step: on a walkable cell (else at its
/// owner's feet), its record goes out.
fn place_rooted(c: &mut Companion, cx: &StepCx<'_>) {
    if !cx.walk.is_walkable(c.x, c.y)
        && let Some(o) = cx.allies.iter().find(|a| a.uid == c.owner)
    {
        (c.x, c.y) = (o.x, o.y);
    }
    c.placed = true;
    c.shown = false;
}

/// The Geographer heals the most wounded ally within reach: a player (the
/// fight applies it) or a companion.
fn geographer(cs: &mut Companions, g: &Companion, cx: &mut StepCx<'_>) {
    let amount = Dice::new(g.k.max(1) as u32, 6, 0).roll(cx.rng, false);
    let frac = |hp: i32, max: i32| hp as f32 / max.max(1) as f32;
    let player = cx
        .allies
        .iter()
        .filter(|a| a.up && a.hp < a.max_hp && dist(a.x, a.y, g.x, g.y) <= GEO_RANGE)
        .min_by(|a, b| frac(a.hp, a.max_hp).total_cmp(&frac(b.hp, b.max_hp)))
        .map(|a| (a.uid, frac(a.hp, a.max_hp)));
    let pet = cs
        .units
        .iter()
        .enumerate()
        .filter(|(_, c)| c.alive() && c.kind != Kind::Sphere && c.hp < c.max_hp && dist(c.x, c.y, g.x, g.y) <= GEO_RANGE)
        .min_by(|a, b| frac(a.1.hp, a.1.max_hp).total_cmp(&frac(b.1.hp, b.1.max_hp)))
        .map(|(i, c)| (i, frac(c.hp, c.max_hp)));
    match (player, pet) {
        (Some((uid, pf)), Some((_, cf))) if pf <= cf => cx.fx.push(AllyFx::Heal { uid, amount, cure: false }),
        (Some((uid, _)), None) => cx.fx.push(AllyFx::Heal { uid, amount, cure: false }),
        (_, Some((i, _))) => {
            let c = &mut cs.units[i];
            let h = amount.min(c.max_hp - c.hp).max(0);
            c.hp += h;
            cx.hits.push(HitEvent { target: c.entity(), amount: h as u32, flags: hit::HEAL });
        }
        (None, None) => {}
    }
}

fn sphere_tick(cs: &mut Companions, i: usize, cx: &mut StepCx<'_>, buf: &mut Vec<u16>) -> bool {
    let mut c = cs.units[i];
    let tick = cx.tick;
    if !c.placed {
        place_rooted(&mut c, cx);
        c.expires = tick + ticks(SPHERE_FUSE_SECS, cx.hz);
        c.pulse_at = tick;
    }
    if c.boom || (c.alive() && c.expires != 0 && tick >= c.expires) {
        explode(&mut c, cx, buf, &mut cs.sink);
    }
    if !c.alive() {
        cs.units[i] = c;
        return tick < c.died_at + ticks(DEAD_SECS, cx.hz);
    }
    if tick >= c.pulse_at {
        c.pulse_at = tick + ticks(SPHERE_PULSE_SECS, cx.hz);
        taunt(cx.horde, c.x, c.y, SPHERE_TAUNT_R, SPHERE_TAUNT_MAX, c.lure_id(), ticks(SPHERE_TAUNT_SECS, cx.hz).min(u16::MAX as u32) as u16, buf);
    }
    cs.units[i] = c;
    true
}

/// The Marine Sphere's blast: `(2 + slv)d6·k` fire, a save for half,
/// credited to its owner; the sphere is gone.
fn explode(c: &mut Companion, cx: &mut StepCx<'_>, buf: &mut Vec<u16>, sink: &mut Sink) {
    c.boom = false;
    let tick = cx.tick;
    let dice = Dice::new((2 + c.slv as u32) * c.k.max(1) as u32, 6, 0);
    monsters_in(cx.horde, c.x, c.y, SPHERE_BLAST_R, buf);
    let n = buf.len().min(64);
    let mut slots = [u16::MAX; 64];
    slots[..n].copy_from_slice(&buf[..n]);
    for &m in &slots[..n] {
        blow(c, m, dice, DamageType::Fire, true, false, Source::Sphere, cx, sink);
    }
    let area = Area::Circle { x: c.x, y: c.y, r: SPHERE_BLAST_R }.telegraph(0, look::FIRE, 0, 0);
    sink.casts.push(ServerMsg::Cast {
        caster: c.entity(),
        skill: "sphere_mine_blast".into(),
        x: c.x,
        y: c.y,
        cast_ms: 0,
        cooldown_ms: 0,
        areas: vec![area],
        level: c.slv,
    });
    if c.alive() {
        c.hp = 0;
        c.state = State::Dead;
        c.died_at = tick;
    }
}

impl HubInstance {
    /// Called on every join (after the player is in `players`): the
    /// homunculus entry of a Merchant-line character (or of any character
    /// with a saved one).
    pub(super) fn companion_arrive(&mut self, idx: usize) {
        let p = &self.players[idx];
        let merchant = Class::from_id(&p.profile.archetype) == Some(Class::Merchant);
        if !merchant && p.profile.homunculus.is_none() {
            return;
        }
        let level = p.fighter.as_ref().map_or(p.profile.level, |f| f.level);
        let learned = p.profile.loadout.level(CALL_SKILL) > 0;
        let save = p.profile.homunculus.clone();
        let (uid, entity) = (p.profile.user_id, p.profile.character_id);
        self.companions.arrive(uid, entity, level, save, learned);
    }

    /// The owner leaves the instance: their companions go, the save is
    /// reported.
    pub(super) fn companion_depart(&mut self, user_id: u32, out: &mut Outgoing) {
        if let Some(save) = self.companions.depart(user_id) {
            out.push((user_id, HubEvent::Homunculus(save)));
        }
    }

    /// Everything about the companions a (re)arriving player needs: every
    /// record, their vitals, and their own homunculus panel.
    pub(super) fn announce_companions(&self, idx: usize, out: &mut Outgoing) {
        let p = &self.players[idx];
        let uid = p.profile.user_id;
        let hz = self.config.tick_hz as f32;
        let cs = &self.companions;
        let here = |c: &&Companion| c.map == p.map && c.placed;
        if cs.units.iter().any(|c| here(&c)) {
            let list = cs.units.iter().filter(here).map(|c| cs.info(c, self.tick, hz)).collect();
            out.push((uid, HubEvent::Control(ServerMsg::Companions { list })));
            for c in cs.units.iter().filter(here) {
                out.push((uid, HubEvent::Control(Companions::vitals(c))));
            }
        }
        if let Some(h) = cs.homs.iter().find(|h| h.owner == uid) {
            out.push((uid, HubEvent::Control(ServerMsg::CompanionState(cs.panel(h, self.tick, hz)))));
        }
    }

    /// One combat step of the companions (fight step 1b): the players on
    /// the map as allies, the step, then what it did to the players.
    pub(super) fn companions_step(&mut self, map: MapIdx, hits: &mut Vec<HitEvent>, kills: &mut Vec<Kill>, swings: &mut Vec<Swing>) {
        if self.companions.homs.is_empty() && self.companions.units.is_empty() {
            return;
        }
        let Some(horde) = self.horde.as_mut() else { return };
        let hz = self.config.tick_hz as f32;
        let walk_step = self.config.walk_speed / hz.max(1.0);
        let allies: Vec<Ally> = self
            .players
            .iter()
            .filter(|p| p.map == map)
            .filter_map(|p| {
                let f = p.fighter.as_ref()?;
                Some(Ally {
                    uid: p.profile.user_id,
                    entity: p.profile.character_id,
                    x: p.x,
                    y: p.y,
                    facing: p.facing,
                    hp: f.hp,
                    max_hp: f.max_hp,
                    up: f.is_up(),
                    level: f.level,
                    link_dead: p.link_dead,
                    step: walk_step * f.mods.pace() * f.card_pace(),
                })
            })
            .collect();
        let mut fx: Vec<AllyFx> = Vec::new();
        let walk = &self.world.map(map).walk;
        let mut cx = StepCx { horde, walk, rng: &mut self.dice, allies: &allies, tick: self.tick, hz, hits, kills, swings, fx: &mut fx };
        step(&mut self.companions, &mut cx);
        // The result screen: what companions dealt, as their skill's row.
        let secs = horde_secs(self.horde.as_ref());
        let rows = [CALL_SKILL, FLORA_SKILL, SPHERE_SKILL].map(|id| self.skills.find(id).map_or(super::stats::SKILL_ATTACK, |(i, _)| i));
        if let Some(horde) = self.horde.as_ref() {
            for (owner, source, h) in self.companions.sink.credits.drain(..) {
                let row = rows[source as usize];
                self.run.credit(owner, row, std::slice::from_ref(&h), self.tick, secs, horde.store());
            }
        }
        for e in fx {
            match e {
                AllyFx::Heal { uid, amount, cure } => {
                    let Some(p) = self.players.iter_mut().find(|p| p.profile.user_id == uid) else { continue };
                    let id = p.profile.character_id;
                    let Some(f) = p.fighter.as_mut().filter(|f| f.is_up()) else { continue };
                    let h = amount.min(f.max_hp - f.hp).max(0);
                    f.hp += h;
                    if cure {
                        for c in [Condition::Poisoned, Condition::Burning, Condition::Blinded, Condition::Slowed, Condition::Mired] {
                            f.cure(c);
                        }
                    }
                    hits.push(HitEvent { target: id, amount: h as u32, flags: hit::HEAL });
                }
                AllyFx::Buff { uid, skill, ticks, mods } => {
                    let now = self.tick;
                    if let Some(f) = self.players.iter_mut().find(|p| p.profile.user_id == uid).and_then(|p| p.fighter.as_mut()).filter(|f| f.is_up()) {
                        f.buff(skill, now + ticks, mods, 0);
                    }
                }
                AllyFx::Move { uid, x, y } => {
                    if let Some(p) = self.players.iter_mut().find(|p| p.profile.user_id == uid) {
                        // Server-side moves reconcile like Blink: the
                        // client's predictor snaps to the snapshot.
                        (p.x, p.y) = (x, y);
                    }
                }
            }
        }
    }

    /// The companions' news for the map (fight step 6): new and changed
    /// records, the gone ones, casts, vitals; each owner's panel (at most
    /// twice a second) and save (at most every 2 s).
    pub(super) fn companion_messages(&mut self, map: MapIdx, out: &mut Outgoing) {
        self.companion_map_messages(map, out);
        self.companion_panels(out);
    }

    /// The records (new, changed, gone), casts and vitals of the units on
    /// `map`, to the players there.
    fn companion_map_messages(&mut self, map: MapIdx, out: &mut Outgoing) {
        let hz = self.config.tick_hz as f32;
        let tick = self.tick;
        let recipients: Vec<u32> = self.players.iter().filter(|p| p.map == map).map(|p| p.profile.user_id).collect();
        let cs = &mut self.companions;
        let mut list: Vec<CompanionInfo> = Vec::new();
        list.extend(cs.gone.extract_if(.., |(m, _)| *m == map).map(|(_, info)| info));
        for i in 0..cs.units.len() {
            if cs.units[i].map == map && !cs.units[i].shown && cs.units[i].placed {
                cs.units[i].shown = true;
                let c = cs.units[i];
                list.push(cs.info(&c, tick, hz));
            }
        }
        let mut msgs: Vec<ServerMsg> = Vec::new();
        if !list.is_empty() {
            msgs.push(ServerMsg::Companions { list });
        }
        msgs.append(&mut cs.sink.casts);
        for c in cs.units.iter_mut().filter(|c| c.placed && c.map == map) {
            let now = [c.hp.max(0), c.max_hp, c.sp.max(0), c.max_sp, i32::from(c.alive())];
            if c.sent != Some(now) {
                c.sent = Some(now);
                msgs.push(Companions::vitals(c));
            }
        }
        for m in msgs {
            for &r in &recipients {
                out.push((r, HubEvent::Control(m.clone())));
            }
        }
    }

    /// Town (an instance without a horde): each homunculus that is out
    /// follows its owner around the owner's map, and nothing else: no
    /// targets, no taunts, no skills, never hurt (design: it lives on
    /// stages; in town it keeps its owner company). A map change takes it
    /// along (`GONE` on the old map, a record on the new one); it is whole
    /// again in town. Cheap: a loop over the homunculi, no allocation once
    /// warm.
    pub(super) fn companions_town(&mut self, out: &mut Outgoing) {
        if self.companions.homs.is_empty() && self.companions.gone.is_empty() {
            return;
        }
        let hz = self.config.tick_hz as f32;
        let tick = self.tick;
        let walk_step = self.config.walk_speed / hz.max(1.0);
        for h in 0..self.companions.homs.len() {
            let hom = &self.companions.homs[h];
            let Some(p) = self.players.iter().find(|p| p.profile.user_id == hom.owner) else { continue };
            let owner = Ally {
                uid: p.profile.user_id,
                entity: p.profile.character_id,
                x: p.x,
                y: p.y,
                facing: p.facing,
                hp: 1,
                max_hp: 1,
                up: true,
                level: p.profile.level,
                link_dead: p.link_dead,
                step: walk_step * (1.0 + p.ride),
            };
            let map = p.map;
            let want = hom.kind.is_some() && hom.called && !hom.resting && !hom.away;
            let unit = hom.unit.and_then(|id| self.companions.units.iter().position(|c| c.id == id));
            let walk = &self.world.map(map).walk;
            let cs = &mut self.companions;
            match unit {
                // Gone to rest, or its owner took another map: it goes (and
                // comes back beside them on the next tick).
                Some(i) if !want || cs.units[i].map != map => cs.remove_at(i, tick),
                Some(i) => {
                    let c = &mut cs.units[i];
                    if dist(c.x, c.y, owner.x, owner.y) > TELEPORT_FAR || c.stuck as u32 >= ticks(STUCK_SECS, hz) {
                        (c.x, c.y) = beside(&owner, walk);
                        c.stuck = 0;
                    }
                    let slot = follow_slot(&owner, walk);
                    walk_to(c, slot, owner.step * SPEED_FACTOR, FOLLOW_SLACK, walk);
                    if dist(c.x, c.y, slot.0, slot.1) <= FOLLOW_SLACK {
                        c.stuck = 0;
                    }
                }
                None if want => {
                    // Whole again in town (a stage's wounds and death stay there).
                    let hom = &mut cs.homs[h];
                    hom.hp = None;
                    hom.dead = false;
                    cs.map = map;
                    let at = beside(&owner, walk);
                    if cs.spawn_hom(h, at.0, at.1, tick).is_some() {
                        let c = cs.units.last_mut().expect("just spawned");
                        c.placed = true;
                        c.state = State::Follow;
                        c.hp = c.max_hp;
                        c.sp = c.max_sp;
                    }
                }
                None => {}
            }
        }
        // News per map: the maps that have units or gone records.
        let mut maps: [MapIdx; 8] = [0; 8];
        let mut n = 0;
        for m in self.companions.units.iter().map(|c| c.map).chain(self.companions.gone.iter().map(|(m, _)| *m)) {
            if n < maps.len() && !maps[..n].contains(&m) {
                maps[n] = m;
                n += 1;
            }
        }
        for &m in &maps[..n] {
            self.companion_map_messages(m, out);
        }
        // Records of maps with nobody on them are dropped, not hoarded.
        self.companions.gone.clear();
        self.companion_panels(out);
    }

    /// Each owner's panel when it changed (at most twice a second) and
    /// their save when it changed (at most every 2 s).
    fn companion_panels(&mut self, out: &mut Outgoing) {
        let hz = self.config.tick_hz as f32;
        let tick = self.tick;
        let cs = &mut self.companions;
        for h in 0..cs.homs.len() {
            if cs.homs[h].away {
                continue;
            }
            let uid = cs.homs[h].owner;
            if tick >= cs.homs[h].state_at {
                let panel = cs.panel(&cs.homs[h], tick, hz);
                let hom = &mut cs.homs[h];
                // Cooldowns tick down on their own on the client: only a
                // change of anything else (or a new cooldown) is news.
                let same = hom.sent.as_ref().is_some_and(|s| CompanionState { cooldowns_ms: [0; 4], ..s.clone() } == CompanionState { cooldowns_ms: [0; 4], ..panel.clone() } && cooldowns_follow(&s.cooldowns_ms, &panel.cooldowns_ms));
                if !same {
                    hom.sent = Some(panel.clone());
                    hom.state_at = tick + ticks(STATE_EVERY_SECS, hz);
                    out.push((uid, HubEvent::Control(ServerMsg::CompanionState(panel))));
                }
            }
            let hom = &mut cs.homs[h];
            if hom.save_dirty && tick >= hom.save_at && hom.kind.is_some() {
                hom.save_dirty = false;
                hom.save_at = tick + ticks(SAVE_EVERY_SECS, hz);
                out.push((uid, HubEvent::Homunculus(hom.save.clone())));
            }
        }
    }

    /// The town skill bar (`TOWN_SKILL`, checked by the socket task): the
    /// Pharmacy opens; Call Homunculus sends it to rest or wakes it, and
    /// with no type yet answers the panel (the client opens the chooser).
    pub fn town_skill(&mut self, user_id: u32, town: TownUse, out: &mut Outgoing) {
        if self.horde.is_some() {
            out.push((user_id, HubEvent::Control(hub_error("not_in_town", "On a mission skills are cast from the hotbar."))));
            return;
        }
        match town {
            TownUse::Pharmacy => self.interact(user_id, super::quest::PHARMACY, out),
            TownUse::Homunculus => {
                let Some(h) = self.companions.hom_index(user_id) else { return };
                let hom = &mut self.companions.homs[h];
                if hom.kind.is_some() {
                    // Out: to rest; resting or never called: out it comes.
                    let out_now = hom.unit.is_some();
                    hom.resting = out_now;
                    hom.called |= !out_now;
                    hom.sent = None;
                }
                let hz = self.config.tick_hz as f32;
                let panel = self.companions.panel(&self.companions.homs[h], self.tick, hz);
                self.companions.homs[h].sent = Some(panel.clone());
                out.push((user_id, HubEvent::Control(ServerMsg::CompanionState(panel))));
            }
            // The socket task mounts (riding is the loadout's).
            TownUse::Mount => {}
        }
    }

    /// `COMPANION_CMD` from `user_id`: choose the type, stance, rename,
    /// evolve (in town), rest / wake.
    pub fn companion_cmd(&mut self, user_id: u32, cmd: CompanionCmd, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let town = matches!(self.kind, InstanceKind::Hub { .. });
        let p = &self.players[i];
        let merchant = Class::from_id(&p.profile.archetype) == Some(Class::Merchant);
        if !merchant && self.companions.hom_index(user_id).is_none() {
            out.push((user_id, HubEvent::Control(hub_error("no_homunculus", "Only the Merchant line keeps a homunculus."))));
            return;
        }
        let level = p.fighter.as_ref().map_or(p.profile.level, |f| f.level);
        let entity = p.profile.character_id;
        let h = self.companions.hom_entry(user_id, entity, level);
        let evolving = matches!(cmd, CompanionCmd::Evolve);
        // A new type: the unit following in town goes and comes back as it.
        let mut retype = false;
        let hom = &mut self.companions.homs[h];
        let error = match cmd {
            CompanionCmd::Choose(v) => match (HomType::from_wire(v), hom.kind) {
                (None, _) => Some(("bad_type", "No such homunculus.")),
                (Some(t), None) => {
                    hom.kind = Some(t);
                    hom.save = HomSave {
                        kind: t.id().into(),
                        name: String::new(),
                        level: level.saturating_sub(BORN_BEHIND).clamp(1, combat::MAX_LEVEL) as u8,
                        exp: 0,
                        bond: 0,
                        evolved: false,
                        stance: hom.stance.id().into(),
                    };
                    // Chosen on a stage: it comes out at once.
                    hom.called = true;
                    hom.hp = None;
                    hom.touch();
                    None
                }
                (Some(t), Some(old)) if t == old => None,
                // Town only: the new type keeps the level and the
                // evolution, the Bond starts again.
                (Some(t), Some(_)) => {
                    let bag = &mut self.players[i].profile.bag;
                    if !town {
                        Some(("type_change_in_town", "Change your homunculus in town."))
                    } else if bag.zeny < TYPE_CHANGE_ZENY {
                        Some(("type_change_zeny", "Changing the homunculus costs 5,000 zeny."))
                    } else {
                        bag.zeny -= TYPE_CHANGE_ZENY;
                        out.push((user_id, HubEvent::ZenySpent { amount: TYPE_CHANGE_ZENY }));
                        hom.kind = Some(t);
                        hom.save.kind = t.id().into();
                        hom.save.bond = 0;
                        hom.hp = None;
                        hom.cds = [0; 4];
                        hom.touch();
                        retype = true;
                        None
                    }
                }
            },
            CompanionCmd::Stance(v) => match Stance::from_wire(v) {
                Some(s) => {
                    hom.stance = s;
                    hom.save.stance = s.id().into();
                    hom.touch();
                    if let Some(id) = hom.unit
                        && let Some(c) = self.companions.units.iter_mut().find(|c| c.id == id)
                    {
                        c.stance = s;
                    }
                    None
                }
                None => Some(("bad_stance", "No such stance.")),
            },
            CompanionCmd::Rename(name) => {
                let name = name.trim();
                if hom.kind.is_none() {
                    Some(("no_homunculus", "Call your homunculus first."))
                } else if name.is_empty() || name.chars().count() > NAME_MAX || name.chars().any(char::is_control) {
                    Some(("bad_name", "A name of 1 to 16 characters."))
                } else {
                    hom.save.name = name.into();
                    hom.touch();
                    if let Some(id) = hom.unit
                        && let Some(c) = self.companions.units.iter_mut().find(|c| c.id == id)
                    {
                        c.shown = false;
                    }
                    None
                }
            }
            CompanionCmd::Evolve => {
                let p = &mut self.players[i];
                if hom.kind.is_none() {
                    Some(("no_homunculus", "Call your homunculus first."))
                } else if hom.save.evolved {
                    Some(("evolved", "It has already evolved."))
                } else if !town && !self.config.missions_anywhere {
                    Some(("evolve_in_town", "Evolve your homunculus in town."))
                } else if level < EVOLVE_OWNER_LEVEL {
                    Some(("evolve_level", "Evolution needs a level 60 master."))
                } else if hom.save.bond < EVOLVE_BOND {
                    Some(("evolve_bond", "Evolution needs a Bond of 75."))
                } else if p.profile.bag.count(STONE_OF_SAGE) == 0 {
                    Some(("evolve_stone", "Evolution needs a Stone of Sage."))
                } else {
                    let _ = p.profile.bag.take(STONE_OF_SAGE, 1);
                    out.push((user_id, HubEvent::ItemUsed { item: STONE_OF_SAGE.into() }));
                    hom.save.evolved = true;
                    hom.hp = None;
                    hom.touch();
                    None
                }
            }
            CompanionCmd::Rest => {
                if hom.kind.is_none() {
                    Some(("no_homunculus", "Call your homunculus first."))
                } else {
                    hom.resting.toggle();
                    if !hom.resting {
                        hom.called = true;
                    }
                    hom.sent = None;
                    None
                }
            }
        };
        if retype && let Some(i) = self.companions.homs[h].unit.and_then(|id| self.companions.units.iter().position(|c| c.id == id)) {
            self.companions.remove_at(i, self.tick);
        }
        if evolving && error.is_none() {
            // The `_h` body, HP ×1.25, the fourth skill.
            self.companions.refit(h);
        }
        if let Some((code, message)) = error {
            out.push((user_id, HubEvent::Control(hub_error(code, message))));
        }
        let hz = self.config.tick_hz as f32;
        let panel = self.companions.panel(&self.companions.homs[h], self.tick, hz);
        let hom = &mut self.companions.homs[h];
        hom.sent = Some(panel.clone());
        out.push((user_id, HubEvent::Control(ServerMsg::CompanionState(panel))));
        if hom.save_dirty && hom.kind.is_some() {
            hom.save_dirty = false;
            out.push((user_id, HubEvent::Homunculus(hom.save.clone())));
        }
    }
}

/// The run clock of the horde (the result screen's buckets).
fn horde_secs(horde: Option<&Horde>) -> f32 {
    horde.map_or(0.0, Horde::director_elapsed)
}

/// Whether `now` is `before` run down by the clock (no new cooldown).
fn cooldowns_follow(before: &[u32; 4], now: &[u32; 4]) -> bool {
    before.iter().zip(now).all(|(b, n)| n <= b)
}

/// AI scenarios and the integration (a stage with a Merchant, a homunculus
/// and monsters, stepped tick by tick).
#[cfg(test)]
#[path = "companion_tests.rs"]
mod scenario_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_the_pool_is_reserved_once() {
        for t in HomType::ALL {
            assert_eq!(HomType::from_wire(t.wire()), Some(t));
            assert_eq!(HomType::from_id(t.id()), Some(t));
            assert!(t.sprite(true).ends_with("_h") && t.sprite(true).starts_with(t.sprite(false)));
        }
        assert_eq!(HomType::from_wire(hom_type::NONE), None);
        for s in [Stance::Aggressive, Stance::Defensive, Stance::Passive] {
            assert_eq!(Stance::from_wire(s.wire()), Some(s));
            assert_eq!(Stance::from_id(s.id()), Some(s));
        }
        assert_eq!(stance::DEFENSIVE, Stance::default().wire());
        for s in Summon::ALL {
            assert_eq!(Summon::from_id(s.id()), Some(s));
        }
        assert!(Kind::Plant(PlantKind::Hydra).rooted() && Kind::Sphere.rooted() && !Kind::Homunculus(HomType::Lif).rooted());
        let pool = Companions::default();
        assert!(pool.units.capacity() >= CAPACITY && pool.is_empty());
    }

    /// The save's JSON shape is the column's (design §3 "Persistence").
    #[test]
    fn the_save_is_the_columns_json() {
        let s = HomSave { kind: "lif".into(), name: "Pip".into(), level: 12, exp: 40, bond: 7, evolved: false, stance: "defensive".into() };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["type"], "lif");
        assert_eq!(serde_json::from_value::<HomSave>(serde_json::json!({"type": "amistr"})).unwrap().kind, "amistr");
        assert_eq!(serde_json::from_value::<HomSave>(v).unwrap(), s);
    }

    #[test]
    fn the_numbers_of_each_type() {
        // HP by the class formula: d8 + 1 at 1, then 4 + 1 + 1 a level.
        assert_eq!(hom_max_hp(HomType::Lif, 1, false), 9);
        assert_eq!(hom_max_hp(HomType::Lif, 10, false), 9 + 9 * 6);
        assert_eq!(hom_max_hp(HomType::Amistr, 10, true), (14 + 9 * 9) * 5 / 4);
        assert_eq!((hom_ac(HomType::Amistr, 13), hom_to_hit(13, false), hom_to_hit(13, true)), (16, 6, 7));
        assert_eq!(hom_dice(HomType::Filir, 29).count, 1);
        assert_eq!(hom_dice(HomType::Filir, 30).count, 2);
        for t in HomType::ALL {
            let unlocks: Vec<u8> = t.skills().iter().map(|s| s.unlock).collect();
            assert_eq!(unlocks, [1, 15, 30, 0], "{t:?}: levels 1 / 15 / 30 and the evolution");
        }
    }

    #[test]
    fn exp_levels_up_to_the_owner_and_catches_up() {
        let mut h = Hom::new(1, 101, 30, Some(HomSave { kind: "filir".into(), level: 10, ..HomSave::default() }), true);
        // 20 levels behind: ×3.
        h.gain(10);
        assert_eq!(h.save.exp, 30);
        let mut h = Hom::new(1, 101, 12, Some(HomSave { kind: "filir".into(), level: 11, ..HomSave::default() }), true);
        h.gain(1_000_000);
        assert_eq!(h.save.level, 12, "capped at the owner's level");
        assert!(h.save.exp < exp_to_next(12));
        h.bond(200);
        assert_eq!(h.save.bond, 100);
        h.bond(-300);
        assert_eq!(h.save.bond, 0);
    }
}
