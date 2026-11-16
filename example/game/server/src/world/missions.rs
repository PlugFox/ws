//! Mission catalogue (COMBAT.md "Mission platform"): the ladder of missions
//! a Kafra offers, each one a location with its level band, its monster
//! roster, its boss, its time limit, its fog and its environment.
//!
//! A selection forms a lobby (`lobby.rs`); starting it launches a stage
//! instance of `map` (`stage.rs`) whose monsters come in waves bought under
//! the mission's [`WaveRules`] from the mission's own roster catalogue
//! ([`roster`]): the baked monsters scoped to it, with their levels raised
//! into its band.
//!
//! The table is Rust, not data: a mission is mostly server rules (waves,
//! fog, environment) that live next to the code that runs them. What is
//! data stays in the bake: which monsters exist and which missions they
//! belong to (recipe `[monsters.<id>.director] missions = [...]`), the map,
//! its hazards (`hazards.json`) and the loading art.
//!
//! A mission whose map is not in the baked pack, or whose roster has no
//! monster yet, is listed as *coming soon* (`MISSION_LIST`) and cannot be
//! started; nothing breaks when a pack lacks a location.

use super::combat::{items, MonsterBlock};
use super::environment::{Environment, Hatch, MapEvent, ObjectDef, ObjectEffect};
use super::horde::catalog::{MonsterCatalog, MonsterDef};
use super::horde::director::WaveRules;
use super::modifiers::MonsterScale;

/// Player levels a mission is made for. The Kafra shows them; a party with
/// nobody at `min` yet gets a warning, never a lock (see [`LevelBand::fit`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelBand {
    pub min: u32,
    /// Where a newcomer to the mission should be.
    pub recommended: u32,
    pub max: u32,
}

/// How a party's levels sit in a mission's band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Somebody is in the band (or above): no warning.
    Ok,
    /// Everybody is under `min`: warn, still allowed.
    Under,
    /// Everybody is above `max`: the run will be easy and pay little exp
    /// (the level-difference modifier); a hint, not a warning.
    Over,
}

impl LevelBand {
    pub const fn new(min: u32, recommended: u32, max: u32) -> Self {
        Self { min, recommended, max }
    }

    /// Soft gate: one member at or above `min` carries the party (the
    /// others are carried, and experience sharing already weights shares by
    /// level). Only a party entirely under the band is warned.
    pub fn fit(&self, levels: &[u32]) -> Fit {
        if levels.is_empty() || levels.iter().any(|&l| l >= self.min && l <= self.max) {
            Fit::Ok
        } else if levels.iter().all(|&l| l < self.min) {
            Fit::Under
        } else if levels.iter().all(|&l| l > self.max) {
            Fit::Over
        } else {
            // Some under, some over, nobody inside: the strong ones carry.
            Fit::Ok
        }
    }

    /// The level a won run's clear bonus is paid at: the player's, at least
    /// the band's floor (carried low characters are paid for the risk).
    pub fn clear_level(&self, level: u32) -> u32 {
        level.max(self.min)
    }
}

/// A roster tweak of one catalogue monster for this mission: its weight or
/// unlock time differ from the baked director block. A listed monster is in
/// the roster even if its `missions` do not name this mission (a monster
/// shared by two locations).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RosterEntry {
    pub monster: &'static str,
    /// Pick weight; 0 keeps the baked one.
    pub weight: u32,
    /// Seconds before the game master may buy it; negative keeps the baked
    /// one (bosses: when the boss shows up).
    pub unlock_secs: f32,
    /// Seconds after which the game master buys no more of it (a run in
    /// acts: each act's roster retires as the next begins; those on the
    /// field stay until killed). Infinite: never.
    pub retire_secs: f32,
}

impl RosterEntry {
    /// The monster as baked.
    pub const fn of(monster: &'static str) -> Self {
        Self { monster, weight: 0, unlock_secs: -1.0, retire_secs: f32::INFINITY }
    }
    /// No more are bought from `retire_secs` into the run.
    pub const fn retire(self, retire_secs: f32) -> Self {
        Self { retire_secs, ..self }
    }
    pub const fn weight(self, weight: u32) -> Self {
        Self { weight, ..self }
    }
    pub const fn unlock(self, unlock_secs: f32) -> Self {
        Self { unlock_secs, ..self }
    }
}

/// Open field or dungeon (the ladder alternates them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terrain {
    Field,
    Dungeon,
}

impl Terrain {
    pub const fn id(self) -> &'static str {
        match self {
            Terrain::Field => "field",
            Terrain::Dungeon => "dungeon",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MissionInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// One line for lists and dialogs.
    pub description: &'static str,
    /// A paragraph for the mission card (the expedition window).
    pub story: &'static str,
    /// Stage location the mission is played on.
    pub map: &'static str,
    /// The location's display name ("Poring Island").
    pub place: &'static str,
    pub terrain: Terrain,
    /// Card/loading art id (`ui/missions/<art>-{320,1024}.webp`,
    /// `ui/backdrops/<art>-*.webp`); empty = the map id.
    pub art: &'static str,
    pub max_players: u8,
    /// Player levels it is made for.
    pub levels: LevelBand,
    /// Surviving this long wins even if the boss lives (seconds).
    pub time_limit_secs: f32,
    /// Difficulty: what the wave game master gets to spend, where packs
    /// land and the monster level band (`levels`). Server-side only.
    pub waves: WaveRules,
    /// Overtime poison fog (`hub/fog.rs`); `None`: the run never fogs.
    pub fog: Option<FogRules>,
    /// Monsters scoped to this mission come from the bake (`missions`);
    /// these rows add or tune some ([`roster`]).
    pub roster: &'static [RosterEntry],
    /// Boss monster ids (recipe ids) and display names, for the card
    /// before the roster is baked; the live roster's bosses come first.
    pub bosses: &'static [(&'static str, &'static str)],
    /// Short threat/mechanic tags for the card ("poison fog", "darkness");
    /// the fog, the light scale and the map events add their own.
    pub tags: &'static [&'static str],
    /// Hazards, light, timed events, interactive objects
    /// (`environment.rs`, run by `hub/env.rs`).
    pub env: Environment,
    /// Loading-screen tips of this mission (`<b>` allowed).
    pub tips: &'static [&'static str],
}

/// When a dragged-out run starts to close in (COMBAT.md §4.7): a poison fog
/// rises at the edge of the field and its safe circle shrinks to a small
/// walkable spot. Server-side only; the wire carries the circle (`FOG`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogRules {
    /// The fog rises once a boss has been on the field this long (s) ...
    pub after_boss_secs: f32,
    /// ... and at this point of the run at the latest, boss or not (s).
    pub latest_secs: f32,
    /// The safe circle closes from the edge of the reachable field to
    /// `final_radius` over this long (s), then holds.
    pub shrink_secs: f32,
    /// Radius of the last safe circle (cells).
    pub final_radius: f32,
}

impl FogRules {
    /// Poring Island: Angeling unlocks at 10:00 and the run is won at
    /// 15:00. The fog rises two minutes after the boss shows up (12:00 at
    /// the latest), closes by 14:30 and holds a 5 m circle to the end.
    pub const NORMAL: FogRules = FogRules { after_boss_secs: 120.0, latest_secs: 720.0, shrink_secs: 150.0, final_radius: 10.0 };
}

/// Most accounts in a party (`lobby.rs`), and what a mission takes.
pub const PARTY_MAX: u8 = 8;

/// The mission whose roster is every monster without a `missions` list
/// (everything baked before the ladder existed).
pub const LEGACY_ROSTER: &str = "poring_island_survival";

impl MissionInfo {
    /// Defaults for a new entry (`MissionInfo { id, name, map, ..MissionInfo::TEMPLATE }`).
    pub const TEMPLATE: MissionInfo = MissionInfo {
        id: "",
        name: "",
        description: "",
        story: "",
        map: "",
        place: "",
        terrain: Terrain::Field,
        art: "",
        max_players: PARTY_MAX,
        levels: LevelBand::new(1, 1, 15),
        time_limit_secs: 900.0,
        waves: WaveRules::NORMAL,
        fog: Some(FogRules::NORMAL),
        roster: &[],
        bosses: &[],
        tags: &[],
        env: Environment::NONE,
        tips: &[],
    };

    /// The card/loading art id.
    pub fn art(&self) -> &'static str {
        if self.art.is_empty() { self.map } else { self.art }
    }
}

/// Sograt Desert's sandstorm: every 2½ minutes from 2:30 for 30 s, the
/// light down to 60 %; the sand-blast outside shelter is `hub/sograt.rs`.
const SANDSTORM: MapEvent = MapEvent {
    id: "sandstorm",
    name: "Sandstorm",
    warning: "A sandstorm is rising! Get behind cover!",
    first_secs: 150.0,
    every_secs: 150.0,
    warn_secs: 12.0,
    duration_secs: 30.0,
    light_scale: 0.6,
    shake: 30,
    calls_waves: false,
};

/// The great sinkhole churns from Phreeoni's unlock to the end of the run
/// (its hazard is `during` this event).
const CHURNING: MapEvent = MapEvent {
    id: "churning",
    name: "The dunes churn",
    warning: "The dunes are churning... something huge stirs below!",
    first_secs: 600.0,
    every_secs: 0.0,
    warn_secs: 15.0,
    duration_secs: 300.0,
    light_scale: 1.0,
    shake: 160,
    calls_waves: false,
};

/// The ladder, in order. Location agents fill in their entry (roster
/// tweaks, bosses, environment, tips) and tag their monsters with the
/// mission id in the recipe; see COMBAT.md "Mission platform".
pub const MISSIONS: &[MissionInfo] = &[
    MissionInfo {
        id: "poring_island_survival",
        name: "Poring Island Survival",
        description: "Hold out against growing waves on Poring Island until the boss shows up.",
        // The expedition window's paragraph (EXPEDITION-UI): keep it true to the roster.
        story: "A sunny island off the coast, overrun by slimes that grow bolder by the minute. Hold the meadows \
                while frogs, hornets and the island's rabbits swell the waves, watch for Eclipse and the other \
                champions, and bring Angeling down when it descends on the ruins.",
        map: "prt_fild08",
        place: "Poring Island",
        terrain: Terrain::Field,
        levels: LevelBand::new(1, 1, 15),
        waves: WaveRules::NORMAL,
        fog: Some(FogRules::NORMAL),
        bosses: &[("angeling", "Angeling")],
        ..MissionInfo::TEMPLATE
    },
    // Placeholders: coming soon until their location and monsters are baked.
    MissionInfo {
        id: "prontera_culvert",
        name: "Prontera Culvert",
        description: "Clear the flooded sewers under Prontera before the Golden Thief Bug comes up.",
        story: "Under the capital's streets the old culverts crawl with thief bugs, sewer rats and bats. Wide stone \
                halls are cut apart by channels of sewage and joined only by narrow iron bridges, where the horde \
                piles up. Thief Bug eggs hatch in the side halls, the Thief Bug Queen and the Rat King rule the dark, \
                and the Golden Thief Bug has come up from the deep to guard its hoard.",
        map: "prt_sewb1",
        place: "Prontera Culvert",
        terrain: Terrain::Dungeon,
        levels: LevelBand::new(15, 18, 30),
        waves: WaveRules { levels: (15, 35), ..WaveRules::NORMAL },
        bosses: &[("golden_bug", "Golden Thief Bug")],
        tags: &["sewage", "eggs", "magic reflect"],
        env: CULVERT_ENV,
        tips: &[
            "The bridges are where the horde <b>bunches up</b>: a Fire Wall or Magnum Break there hits a whole crowd.",
            "<b>Thief Bug Eggs</b> hatch if you leave them: break a cluster while it is still just eggs.",
            "The <b>Golden Thief Bug</b>'s shell <b>reflects magic</b> until it cracks: swords, arrows and daggers first.",
            "When the sluice gates groan, <b>step off the banks</b>: the flush brings the sewage up over them.",
            "<b>Fire</b> fizzles in sewage; <b>cold</b> freezes it into safe ice for a while.",
            "The <b>Rat King</b>'s stampedes run in lanes: stand in the gap.",
        ],
        ..MissionInfo::TEMPLATE
    },
    MissionInfo {
        id: "sograt_desert",
        name: "Sograt Desert",
        description: "Survive the dunes of Sograt and the beast that hunts under the sand.",
        story: "A sun-scorched sea of dunes south of Morroc. There is no cover out here: the desert closes in from \
                every side, and the sand itself is restless. When the dunes start to churn, Phreeoni is coming up to feed.",
        map: "moc_fild17",
        place: "Sograt Desert",
        terrain: Terrain::Field,
        levels: LevelBand::new(30, 33, 45),
        // The plateau is cut into 5-8 m lanes by cacti, ridges and ruins
        // (example/docs/locations/sograt.md §1.1): a little less budget
        // than the island's open meadow, packs a little more often from
        // all sides (they come through the lanes anyway).
        waves: WaveRules { levels: (30, 50), budget: 11.0, growth: 1.11, surround_chance: 0.2, ..WaveRules::NORMAL },
        bosses: &[("phreeoni", "Phreeoni")],
        tags: &["quicksand", "burrowers"],
        env: Environment {
            light_scale: 1.0,
            // The antlion pits and the sinkhole come from the bake
            // (`hazards.json`); the sand-blast, buried Hodes and the boss's
            // rise are `hub/sograt.rs`.
            hazards: &[],
            events: &[SANDSTORM, CHURNING],
            objects: &[],
            strikes: &[],
        },
        tips: &[
            "Sandstorms come every <b>2½ minutes</b>: fight from the <b>lee of the ruins, bones and cacti</b>, the open sand gets blasted.",
            "Each sandstorm blows from its own side: the sheltered spot is <b>downwind</b> of the cover.",
            "The <b>antlion pits</b> drag you to the rim. Walk out sideways, never straight back.",
            "A <b>moving sand mound</b> is a Hode. It surfaces right next to you, so keep moving.",
            "Don't stand in a line with a <b>Peco Peco</b> or <b>Phreeoni</b>: they charge.",
            "When the dunes start to churn, <b>Phreeoni</b> is coming up out of the great sinkhole.",
        ],
        ..MissionInfo::TEMPLATE
    },
    MissionInfo {
        id: "payon_caves",
        name: "Payon Caves",
        description: "Descend into the haunted caves under Payon and face Moonlight Flower.",
        story: "Beneath the forested hills of Payon the dead do not rest. Lantern-lit shrines give way to dark tunnels \
                where zombies, skeletons and hopping jiangshi shamble out of the gloom, and deep below Moonlight \
                Flower rings her bells among the nine-tailed foxes.",
        map: "pay_dun04",
        place: "Payon Caves",
        terrain: Terrain::Dungeon,
        levels: LevelBand::new(45, 48, 60),
        waves: WaveRules { levels: (45, 65), ..WaveRules::NORMAL },
        bosses: &[("moonlight_flower", "Moonlight Flower")],
        tags: &["undead", "braziers"],
        env: PAYON_ENV,
        tips: &[
            "Undead hate the light: <b>holy</b> skills hit Zombies, Skeletons, Munak and Bongun twice as hard.",
            "Strike a cold <b>brazier</b> to kindle it: the dark draws back and the dead burn in its light.",
            "The rings are loops: keep moving round instead of backing into a corner.",
            "Horong and Nine Tails are fire. Bring water, not more fire.",
            "The <b>Weeping Bride</b>'s wail terrifies: you run from her and cannot fight until it passes.",
            "When the bells ring on their own, Moonlight Flower is coming. Light the braziers round the shrine first.",
        ],
        ..MissionInfo::TEMPLATE
    },
    // The ladder from 60 to 99 (docs/locations/ladder_60_99.md): coming
    // soon until each location and its monsters are baked.
    // >>> orc_village (agent ORC) >>>
    MissionInfo {
        id: "orc_village",
        name: "Orc Village",
        description: "Break the orc warband's camp before the Orc Lord rides out.",
        story: "A daylight war camp of huts, palisades and banners on two plateaus joined by a rope bridge. The \
                orcs fight as a warband: packs come in formation, archers stand behind shields, banners rally whoever \
                fights under them and a war horn sends a whole column in from one side. The Orc Lord comes at the end.",
        map: "gef_fild10",
        place: "Orc Village",
        terrain: Terrain::Field,
        levels: LevelBand::new(60, 62, 68),
        // Starting points of docs/locations/ladder_60_99.md §2.1.
        waves: WaveRules { levels: (60, 73), budget: 13.0, cluster_chance: 0.45, surround_chance: 0.2, ..WaveRules::NORMAL },
        bosses: &[("orc_lord", "Orc Lord")],
        tags: &["war banners", "war horn"],
        env: ORC_ENV,
        tips: &[
            "Orcs fight harder <b>under a war banner</b>. Cut the banner down, then the orcs.",
            "The <b>war horn</b> names a side. A whole column comes from there: be somewhere else, or be ready.",
            "Arrows fall during a horn: <b>keep moving</b>, and <b>Pneuma</b> turns them aside.",
            "Orcs are creatures of earth: <b>fire</b> burns them. High Orcs are fire themselves: use <b>cold</b>.",
            "Never stand in a line with a <b>Savage</b>, the <b>Orc Hero</b> or the <b>Orc Lord</b>: they charge.",
            "When the Orc Lord's <b>drums</b> start, break them: each one left calls a High Orc.",
        ],
        ..MissionInfo::TEMPLATE
    },
    // <<< orc_village <<<
    // >>> toy_factory (agent TOY) >>>
    MissionInfo {
        id: "toy_factory",
        name: "Toy Factory",
        description: "Survive the frozen toy factory, where the floor itself moves.",
        story: "A bright, frozen factory whose floor moves: conveyor belts carry players and monsters alike, a \
                whistle turns them round while the cranes drop crates, and wind-up toys and clockwork alarms pour out of \
                the packing halls. The Stormy Knight rides in last and tries to blow the party onto the belts.",
        map: "xmas_dun02",
        place: "Toy Factory",
        terrain: Terrain::Dungeon,
        levels: LevelBand::new(66, 68, 75),
        // Starting points of docs/locations/ladder_60_99.md §2.1: the
        // corridors are Payon's width, packs land a little closer.
        waves: WaveRules { levels: (66, 80), surround_chance: 0.1, spawn_margin: (4.0, 10.0), ..WaveRules::NORMAL },
        bosses: &[("knight_of_windstorm", "Stormy Knight")],
        tags: &["conveyor belts", "switches"],
        env: TOY_ENV,
        tips: &[
            "The belts carry you <b>and the monsters</b>. Fight at a belt's end and it delivers them to you.",
            "When the whistle blows the belts <b>reverse and speed up</b>, and the cranes drop crates.",
            "Hit a <b>switch</b> to stop the belts near it for half a minute.",
            "Most toys are <b>neutral</b>: no element helps. Hatii hates <b>lightning</b>, the Stormy Knight hates <b>earth</b>.",
            "A <b>Marionette</b> pulls you toward it. Look at what is behind it first.",
            "The Stormy Knight's <b>Gale</b> throws you: keep a wall at your back, never a belt.",
        ],
        ..MissionInfo::TEMPLATE
    },
    // <<< toy_factory <<<
    // >>> glast_heim (agent GLAST) >>>
    MissionInfo {
        id: "glast_heim",
        name: "Glast Heim Churchyard",
        description: "Consecrate the graves of the fallen capital before the Dark Lord rises.",
        story: "The churchyard of a fallen capital at night: hundreds of graves round a broken chapel. The dead here \
                wear armour, shoot, heal each other and climb back out of their graves until someone consecrates them, \
                and twice a run the bell tolls thirteen and every grave opens again. The Dark Lord waits at the chapel.",
        map: "gl_chyard",
        place: "Glast Heim",
        terrain: Terrain::Field,
        levels: LevelBand::new(73, 75, 83),
        // Starting points of docs/locations/ladder_60_99.md §2.1.
        waves: WaveRules { levels: (73, 88), budget: 13.0, growth: 1.125, surround_chance: 0.2, elite_weight: 1.2, ..WaveRules::NORMAL },
        bosses: &[("dark_lord", "Dark Lord")],
        tags: &["undead", "graves", "holy fonts"],
        env: GLAST_ENV,
        tips: &[
            "<b>Radiant</b> hurts almost everything here; <b>poison</b> does nothing to the dead.",
            "A <b>restless grave</b> keeps sending Ghouls until someone breaks it. Break them early.",
            "When the bell tolls <b>thirteen</b>, every grave opens again and the plots are cursed: get to a court.",
            "Strike a <b>holy font</b>: its pool heals you and burns the dead.",
            "<b>Evil Druids</b> silence with their rot bolts. Casters: kill them first, or stand behind someone.",
            "<b>Dark Priests</b> mend the dead from behind the armour. Reach them, or hit them as they chant.",
            "The Dark Lord's <b>Hell's Gate</b> is shared: stand together on the marked one.",
        ],
        ..MissionInfo::TEMPLATE
    },
    // <<< glast_heim <<<
    // >>> thor_volcano (agent THOR) >>>
    MissionInfo {
        id: "thor_volcano",
        name: "Thor Volcano",
        description: "Hold out in the heart of a volcano while the lava floods the chambers.",
        story: "A volcano's heart of round magma chambers joined by narrow cracks in the rock. The lava surges into \
                the chambers in turn, bombs fall out of the smoke and the party is driven into the cracks where the horde \
                waits. Almost everything here burns and almost nothing fears fire. Ifrit rises at the end.",
        map: "thor_v03",
        place: "Thor Volcano",
        terrain: Terrain::Dungeon,
        levels: LevelBand::new(81, 83, 91),
        // Starting points of docs/locations/ladder_60_99.md §2.1.
        waves: WaveRules { levels: (81, 96), growth: 1.13, cluster_chance: 0.45, surround_chance: 0.1, spawn_margin: (4.0, 10.0), ..WaveRules::NORMAL },
        bosses: &[("ifrit", "Ifrit")],
        tags: &["fire", "lava surges"],
        env: THOR_ENV,
        tips: &[
            "Nearly everything here is fire: <b>cold</b> hits twice as hard, fire barely scratches.",
            "When the mountain groans, lava floods the <b>chambers</b> of one side. Fight in the cracks until it sinks.",
            "A <b>cold</b> spell on a vent or on rising lava leaves <b>obsidian</b>: safe ground for a few seconds.",
            "An <b>Explosion</b> swells before it bursts. Kill it then, or step back.",
            "A shimmer in the air is a <b>Byorgue</b>. Keep moving: it strikes whoever stands still.",
            "Ifrit's <b>Meteor</b> is shared: stand together under it. His <b>Eruption</b> is not: find the obsidian.",
        ],
        ..MissionInfo::TEMPLATE
    },
    // <<< thor_volcano <<<
    // >>> thanatos_tower (agent TOWER) >>>
    MissionInfo {
        id: "thanatos_tower",
        name: "Thanatos Tower",
        description: "Climb the tower in three acts and face Thanatos at the summit.",
        story: "The last rung of the ladder is a climb: three floors played as three acts on one map, each with its \
                own monsters, its own rule and a stair that opens on the clock. Books and mimics give way to plasma of \
                five colours, then to the angels of the sanctum, and everything the ladder taught comes back once. \
                Thanatos waits at the summit.",
        map: "tha_t02",
        place: "Thanatos Tower",
        terrain: Terrain::Dungeon,
        levels: LevelBand::new(89, 91, 99),
        // Starting points of docs/locations/ladder_60_99.md §2.1.
        waves: WaveRules { levels: (89, 104), budget: 15.0, growth: 1.13, surround_chance: 0.25, elite_weight: 1.3, opening_boost: 2.0, ..WaveRules::NORMAL },
        // Three acts on one floor (docs/locations/thanatos_tower.md §3.1):
        // each act's kinds retire as the next stair opens (those on the
        // field stay); the elites guard their own act.
        roster: &[
            RosterEntry::of("deathword").retire(360.0),
            RosterEntry::of("ancient_mimic").retire(360.0),
            RosterEntry::of("curator").retire(180.0),
            RosterEntry::of("owl_duke").retire(600.0),
            RosterEntry::of("tha_odium").retire(360.0),
            RosterEntry::of("plasma_r").retire(600.0),
            RosterEntry::of("plasma_b").retire(600.0),
            RosterEntry::of("plasma_g").retire(600.0),
            RosterEntry::of("plasma_p").retire(600.0),
            RosterEntry::of("plasma_y").retire(600.0),
            RosterEntry::of("tha_despero").retire(600.0),
        ],
        bosses: &[("thanatos", "Thanatos")],
        tags: &["three acts", "colour wheel", "specimen tubes", "judgment", "ghosts"],
        env: TOWER_ENV,
        tips: &[
            "The tower is three floors: the monsters change when a <b>stair opens</b> at 3:00 and 6:00.",
            "Break the <b>specimen tubes</b> in the galleries before they crack: each one left hatches plasma.",
            "Every <b>plasma</b> fears one thing: red cold, blue lightning, green fire, purple holy, yellow force.",
            "The angels of the Sanctum are <b>holy</b>: radiant skills are weak against Solace and Shelter. Kill <b>Shelter</b> first.",
            "When the statues open their eyes, beams fall. They never strike <b>beside a statue</b>.",
            "Thanatos and his fragments are <b>ghosts</b>: swords and arrows do half. Force and holy do double.",
            "<b>Burden</b> is shared: stand together under the mask.",
        ],
        ..MissionInfo::TEMPLATE
    },
    // <<< thanatos_tower <<<
];

/// Prontera Culvert (docs/locations/culvert.md §3): a cellar (the light
/// ×0.85), the banks of the channels (baked `hazards.json`: always on, and
/// the Flush's `during` boxes), the sluices opening on the west and the east
/// halls in turn, and Thief Bug egg clusters in the side halls that hatch
/// one after another unless broken (`hub/culvert.rs`: sewage vs fire and
/// cold).
const CULVERT_ENV: Environment = Environment {
    light_scale: 0.85,
    hazards: &[],
    events: &[
        MapEvent {
            id: "flush_west",
            name: "Flush (west)",
            warning: "The sluice gates groan open... sewage is coming down the west halls!",
            first_secs: 150.0,
            every_secs: 360.0,
            warn_secs: 12.0,
            duration_secs: 20.0,
            light_scale: 1.0,
            shake: 60,
            calls_waves: false,
        },
        MapEvent {
            id: "flush_east",
            name: "Flush (east)",
            warning: "The sluice gates groan open... sewage is coming down the east halls!",
            first_secs: 330.0,
            every_secs: 360.0,
            warn_secs: 12.0,
            duration_secs: 20.0,
            light_scale: 1.0,
            shake: 60,
            calls_waves: false,
        },
    ],
    objects: &CULVERT_EGGS,
    strikes: &[],
};

/// An egg cluster of the culvert: six eggs round a spot of a side hall,
/// hatching `secs` into the run into two larvae each.
const fn egg_cluster(spots: &'static [(f32, f32)], secs: f32) -> ObjectDef {
    ObjectDef {
        monster: "thief_bug_egg",
        spots,
        on_felled: ObjectEffect::None,
        hatch: Some(Hatch::once(secs, "thief_bug_larva", 2)),
    }
}

/// Ring of five round a centre, 1.8 cells out (spots checked walkable and
/// off the banks by the bake's hazard boxes).
macro_rules! cluster {
    ($x:expr, $y:expr) => {
        &[
            ($x, $y),
            ($x + 1.8, $y),
            ($x + 0.6, $y + 1.7),
            ($x - 1.5, $y + 1.1),
            ($x - 1.5, $y - 1.1),
            ($x + 0.6, $y - 1.7),
        ]
    };
}

/// One cluster per side hall, hatching a little after one another from
/// 2:00 (the party cannot be everywhere: the far ones become larvae).
const CULVERT_EGGS: [ObjectDef; 10] = [
    egg_cluster(cluster!(56.5, 49.0), 120.0),
    egg_cluster(cluster!(263.5, 49.0), 150.0),
    egg_cluster(cluster!(56.5, 83.0), 180.0),
    egg_cluster(cluster!(263.5, 83.0), 210.0),
    egg_cluster(cluster!(56.5, 117.0), 240.0),
    egg_cluster(cluster!(263.5, 117.0), 270.0),
    egg_cluster(cluster!(56.5, 151.0), 300.0),
    egg_cluster(cluster!(263.5, 151.0), 330.0),
    egg_cluster(cluster!(130.0, 192.0), 360.0),
    egg_cluster(cluster!(190.0, 192.0), 390.0),
];

/// Payon Caves (docs/locations/payon.md §4): a dark cave (the light ×0.55),
/// cold draughts that dim it, the coffin and the moonrise as warnings, the
/// map's 23 braziers to kindle and its two shrine bells to ring
/// (`hub/payon.rs`).
const PAYON_ENV: Environment = Environment {
    light_scale: 0.55,
    hazards: &[],
    events: &[
        MapEvent {
            id: "draught",
            name: "Cold Draught",
            warning: "A cold draught howls through the tunnels...",
            first_secs: 180.0,
            every_secs: 180.0,
            warn_secs: 6.0,
            duration_secs: 20.0,
            light_scale: 0.7,
            shake: 0,
            calls_waves: false,
        },
        MapEvent {
            id: "coffin",
            name: "The Coffin",
            warning: "Something is pounding on a coffin lid in the south cave...",
            // Just before the Jiangshi General may rise (5:00).
            first_secs: 290.0,
            every_secs: 0.0,
            warn_secs: 10.0,
            duration_secs: 4.0,
            light_scale: 1.0,
            shake: 110,
            calls_waves: false,
        },
        MapEvent {
            id: "moonrise",
            name: "Moonrise",
            warning: "Bells ring in the dark... Moonlight Flower is coming!",
            first_secs: 600.0,
            every_secs: 0.0,
            warn_secs: 10.0,
            duration_secs: 30.0,
            light_scale: 1.15,
            shake: 70,
            calls_waves: false,
        },
    ],
    // On the walkable cell nearest each brazier of the map (RSW fire effect 47).
    objects: &[ObjectDef {
        monster: "brazier",
        spots: &[
            (101.5, 128.5),
            (113.5, 139.5),
            (132.5, 132.5),
            (137.5, 116.5),
            (124.5, 106.5),
            (103.5, 115.5),
            (115.5, 104.5),
            (111.5, 85.5),
            (129.5, 86.5),
            (83.5, 111.5),
            (84.5, 129.5),
            (187.5, 118.5),
            (186.5, 129.5),
            (44.5, 116.5),
            (44.5, 127.5),
            (113.5, 47.5),
            (125.5, 47.5),
            (109.5, 197.5),
            (122.5, 197.5),
            (191.5, 47.5),
            (39.5, 45.5),
            (47.5, 37.5),
            (184.5, 40.5),
        ],
        on_felled: ObjectEffect::Hook("brazier"),
        hatch: None,
    },
    // The two shrine bells, out in the open on the north and south outer
    // ring (the map's own bell pavilions stand walled in).
    ObjectDef { monster: "shrine_bell", spots: &[(149.5, 42.5), (97.5, 201.5)], on_felled: ObjectEffect::Hook("bell"), hatch: None }],
    strikes: &[],
};

// >>> orc_village: env (agent ORC) >>>
/// Orc Village's war horn: every 2½ minutes from 2:00 (2:00, 4:30, 7:00,
/// 9:30; the one at 12:00 falls after the fog and spawns nothing). The
/// column it calls from one side, the replanted banners and the side
/// itself are `hub/orc.rs`; the Orc Lord's Call the Clans forces one.
const WAR_HORN: MapEvent = MapEvent {
    id: "war_horn",
    name: "War Horn",
    warning: "A war horn sounds across the valley... an orc column is marching in!",
    first_secs: 120.0,
    every_secs: 150.0,
    warn_secs: 10.0,
    duration_secs: 25.0,
    light_scale: 1.0,
    shake: 40,
    calls_waves: true,
};

/// The war banners planted at the start (cells, by the map's banner
/// models): four in the east village round the chief's hall, two in the
/// north-west village, two on the approaches (`hub/orc.rs` replants them).
pub(crate) const ORC_BANNERS: [(f32, f32); 8] =
    [(221.5, 165.5), (204.5, 161.5), (232.5, 170.5), (188.5, 187.5), (63.5, 324.5), (76.5, 344.5), (162.5, 76.5), (213.5, 296.5)];

/// Orc Village (docs/locations/orc_village.md §3): plain daylight, the war
/// horn with its arrow volleys, and eight war banners planted round the
/// villages and on the approaches (`hub/orc.rs`: they rally the orcs round
/// them; the horn replants the fallen).
const ORC_ENV: Environment = Environment {
    light_scale: 1.0,
    hazards: &[],
    events: &[WAR_HORN],
    objects: &[ObjectDef {
        monster: "orc_banner",
        spots: &ORC_BANNERS,
        on_felled: ObjectEffect::Hook("orc_banner"),
        hatch: None,
    }],
    // Arrow volleys while the horn sounds: a circle on or near each player
    // every 2.5 s, falling for 0.9 s (Pneuma turns them aside).
    strikes: &[crate::world::environment::Strikes {
        fall_secs: 0.9,
        scatter: 2.0,
        extra: 1,
        extra_spread: 5.0,
        look: "damage",
        damage: [2, 6],
        damage_type: "physical",
        ranged: true,
        dc: 13,
        ..crate::world::environment::Strikes::new("orc_arrows", "war_horn", 2.5, 1.8)
    }],
};
// <<< orc_village: env <<<

// >>> toy_factory: env (agent TOY) >>>
/// Toy Factory (docs/locations/toy_factory.md §3): a bright hall (no
/// darkness); the belts are baked (`hazards.json` `belt_<n>`, a drift
/// each) and run by `hub/toy.rs` (reversed in a shift, stopped by a
/// switch); every two minutes from 2:00 a shift change reverses them while
/// the cranes drop crates; ten switches stop the belts near them.
const TOY_ENV: Environment = Environment {
    light_scale: 1.0,
    hazards: &[],
    events: &[SHIFT_CHANGE],
    objects: &[ObjectDef {
        monster: "factory_switch",
        // The walkable cell beside each of the map's ten switch models.
        spots: &[
            (129.5, 117.5),
            (27.5, 105.5),
            (17.5, 105.5),
            (235.5, 41.5),
            (191.5, 67.5),
            (173.5, 48.5),
            (183.5, 109.5),
            (215.5, 27.5),
            (51.5, 105.5),
            (61.5, 105.5),
        ],
        on_felled: ObjectEffect::Hook("switch"),
        hatch: None,
    }],
    strikes: &[CRATES],
};

/// The factory whistle: every two minutes from 2:00 the belts run
/// reversed and faster for 30 s (`hub/toy.rs`).
const SHIFT_CHANGE: MapEvent = MapEvent {
    id: "shift_change",
    name: "Shift Change",
    warning: "The whistle blows! Shift change: the belts are turning!",
    first_secs: 120.0,
    every_secs: 120.0,
    warn_secs: 10.0,
    duration_secs: 30.0,
    light_scale: 1.0,
    shake: 30,
    calls_waves: false,
};

/// Crates from the cranes during a shift: one near each player every 3 s;
/// some leave a Myst Case (`hub/toy.rs` crate hook).
const CRATES: super::environment::Strikes = super::environment::Strikes {
    fall_secs: 1.0,
    scatter: 3.0,
    look: "damage",
    damage: [2, 8],
    damage_type: "physical",
    dc: 14,
    push: 2.0,
    hook: Some("crate"),
    ..super::environment::Strikes::new("toy_crate", "shift_change", 3.0, 2.0)
};
// <<< toy_factory: env <<<

// >>> glast_heim: env (agent GLAST) >>>
/// Glast Heim Churchyard (docs/locations/glast_heim.md §3): a night field
/// (the light ×0.7), ten restless graves that keep letting Ghouls out
/// until they are broken, the Witching Hour at 4:00 and 8:00 (the bell
/// tolls thirteen: every broken grave rises again and the plots are
/// cursed), four holy fonts in the corner courts and the eclipse before
/// the Dark Lord (`hub/glast.rs`).
const GLAST_ENV: Environment = Environment {
    light_scale: 0.7,
    hazards: &GLAST_PLOTS,
    events: &[
        MapEvent {
            id: "witching_hour",
            name: "The Witching Hour",
            warning: "The bell is tolling thirteen... the graves are opening!",
            first_secs: 240.0,
            every_secs: 240.0,
            warn_secs: 13.0,
            duration_secs: 30.0,
            light_scale: 0.7,
            shake: 50,
            calls_waves: true,
        },
        MapEvent {
            id: "eclipse",
            name: "Eclipse",
            warning: "The moon goes black over the chapel... the Dark Lord is rising!",
            first_secs: 585.0,
            every_secs: 0.0,
            warn_secs: 15.0,
            duration_secs: 35.0,
            light_scale: 0.55,
            shake: 120,
            calls_waves: false,
        },
    ],
    objects: &GLAST_OBJECTS,
    strikes: &[],
};

/// A restless grave on a big plot (the walkable cell nearest the plot's
/// centre with 3 cells of room): from 1:30, one after another 20 s apart,
/// it lets two Ghouls out every 25 s until it is broken (`grave` hook).
const fn grave(spot: &'static [(f32, f32)], first: f32) -> ObjectDef {
    ObjectDef { monster: "grave", spots: spot, on_felled: ObjectEffect::Hook("grave"), hatch: Some(Hatch::every(first, 25.0, "ghoul", 2)) }
}

/// The ten graves and the four holy fonts of the corner courts.
const GLAST_OBJECTS: [ObjectDef; 11] = [
    grave(&[(54.5, 86.5)], 90.0),
    grave(&[(251.5, 89.5)], 110.0),
    grave(&[(46.5, 210.5)], 130.0),
    grave(&[(251.5, 210.5)], 150.0),
    grave(&[(116.5, 185.5)], 170.0),
    grave(&[(182.5, 222.5)], 190.0),
    grave(&[(110.5, 222.5)], 210.0),
    grave(&[(171.5, 184.5)], 230.0),
    grave(&[(178.5, 74.5)], 250.0),
    grave(&[(107.5, 124.5)], 270.0),
    ObjectDef {
        monster: "holy_font",
        spots: &[(84.5, 50.5), (212.5, 51.5), (86.5, 197.5), (219.5, 202.5)],
        on_felled: ObjectEffect::Hook("font"),
        hatch: None,
    },
];

/// A grave plot cursed while the Witching Hour runs (the box round its
/// gravestones): 1d6 necrotic a second, slowed, DC 14.
const fn plot(id: &'static str, x0: f32, x1: f32, y0: f32, y1: f32) -> super::environment::Hazard {
    super::environment::Hazard {
        look: "curse",
        damage: [1, 6],
        damage_type: "necrotic",
        condition: Some("slowed"),
        condition_secs: 1.2,
        dc: 14,
        during: Some("witching_hour"),
        shape: super::environment::HazardShape::Rect { x: (x0 + x1) / 2.0, y: (y0 + y1) / 2.0, w: x1 - x0 + 3.0, h: y1 - y0 + 3.0 },
        ..super::environment::Hazard::circle(id, 0.0, 0.0, 1.0)
    }
}

/// The ten big plots (their gravestones' extent, docs §1.2).
const GLAST_PLOTS: [super::environment::Hazard; 10] = [
    plot("cursed_plot_0", 43.0, 55.0, 71.0, 95.0),
    plot("cursed_plot_1", 251.0, 257.0, 77.0, 98.0),
    plot("cursed_plot_2", 43.0, 49.0, 198.0, 217.0),
    plot("cursed_plot_3", 250.0, 257.0, 197.0, 218.0),
    plot("cursed_plot_4", 107.0, 129.0, 177.0, 193.0),
    plot("cursed_plot_5", 173.0, 194.0, 211.0, 225.0),
    plot("cursed_plot_6", 101.0, 122.0, 211.0, 225.0),
    plot("cursed_plot_7", 166.0, 189.0, 179.0, 193.0),
    plot("cursed_plot_8", 175.0, 185.0, 66.0, 85.0),
    plot("cursed_plot_9", 97.0, 118.0, 111.0, 135.0),
];
// <<< glast_heim: env <<<

// >>> thor_volcano: env (agent THOR) >>>
/// Thor Volcano (docs/locations/thor_volcano.md §3): a hot dark (the light
/// ×0.9), the magma vents (baked `hazards.json`, always on), the Lava Surge
/// flooding the chambers of one half and then the other every five minutes
/// (the `surge_*` hazards, `during` their event), volcanic bombs falling on
/// everyone while a surge runs, and the tremor that warns of Ifrit. Cold
/// spells on the lava leave obsidian (`hub/env.rs` `ground_meets`); Ifrit's
/// chamber and the hidden Byorgues are `hub/thor.rs`.
const THOR_ENV: Environment = Environment { light_scale: 0.9, hazards: &[], events: &THOR_EVENTS, objects: &[], strikes: &THOR_BOMBS };

/// The Lava Surge of one half (`surge_west` / `surge_east`).
const fn surge(id: &'static str, name: &'static str, warning: &'static str, first_secs: f32) -> MapEvent {
    MapEvent { id, name, warning, first_secs, every_secs: 300.0, warn_secs: 12.0, duration_secs: 25.0, light_scale: 1.1, shake: 90, calls_waves: false }
}

const THOR_EVENTS: [MapEvent; 3] = [
    surge("surge_west", "Lava Surge (west)", "The mountain groans... lava is rising in the west chambers!", 150.0),
    surge("surge_east", "Lava Surge (east)", "The mountain groans... lava is rising in the east chambers!", 300.0),
    // Ifrit's arrival (10:00): warned at 9:45, the whole mountain shakes.
    MapEvent {
        id: "tremor",
        name: "The mountain shakes",
        warning: "The whole mountain is shaking... something vast stirs in the magma!",
        first_secs: 600.0,
        every_secs: 0.0,
        warn_secs: 15.0,
        duration_secs: 8.0,
        light_scale: 1.15,
        shake: 200,
        calls_waves: false,
    },
];

/// Volcanic bombs out of the smoke while a surge runs: one on every
/// standing player every 3 s (and one more nearby), 1.2 s of shadow, and a
/// patch of embers left for 3 s.
const fn bombs(id: &'static str, during: &'static str) -> super::environment::Strikes {
    super::environment::Strikes {
        fall_secs: 1.2,
        scatter: 1.5,
        extra: 1,
        extra_spread: 8.0,
        look: "fire",
        damage: [2, 8],
        damage_type: "fire",
        condition: Some("burning"),
        condition_secs: 2.0,
        dc: 14,
        patch: Some(EMBERS),
        patch_secs: 3.0,
        ..super::environment::Strikes::new(id, during, 3.0, 2.2)
    }
}

/// The embers a bomb leaves (moved to where it lands).
const EMBERS: super::environment::Hazard = super::environment::Hazard {
    look: "fire",
    damage: [1, 4],
    damage_type: "fire",
    condition: Some("burning"),
    condition_secs: 1.0,
    dc: 13,
    ..super::environment::Hazard::circle("thor_embers", 0.0, 0.0, 1.6)
};

const THOR_BOMBS: [super::environment::Strikes; 2] = [bombs("thor_bombs_w", "surge_west"), bombs("thor_bombs_e", "surge_east")];
// <<< thor_volcano: env <<<

// >>> thanatos_tower: env (agent TOWER) >>>
/// Thanatos Tower (docs/locations/thanatos_tower.md §3): a tall dark hall
/// (the light ×0.8) climbed in three acts. Three one-shot stairs mark the
/// acts on the timeline (the engine re-tints the haze for each); the
/// specimen tubes of the galleries and wings crack one cluster every 30 s
/// from 2:00, each tube into two plasmas of its colour; from 7:30 the angel
/// statues judge every 90 s: beams fall on and near every player, never
/// beside a statue, and a beam that lands on angels mends them
/// (`hub/thanatos.rs`, which also brings Thanatos into the nave).
const TOWER_ENV: Environment = Environment {
    light_scale: 0.8,
    hazards: &[],
    events: &[
        MapEvent {
            id: "stair_lab",
            name: "The Laboratory",
            warning: "The stair to the Laboratory grinds open... the specimen tubes are stirring!",
            first_secs: 180.0,
            every_secs: 0.0,
            warn_secs: 10.0,
            duration_secs: 20.0,
            light_scale: 1.0,
            shake: 80,
            calls_waves: false,
        },
        MapEvent {
            id: "stair_sanctum",
            name: "The Sanctum",
            warning: "The stair to the Sanctum grinds open... wings beat somewhere above!",
            first_secs: 360.0,
            every_secs: 0.0,
            warn_secs: 10.0,
            duration_secs: 20.0,
            light_scale: 1.0,
            shake: 80,
            calls_waves: false,
        },
        MapEvent {
            id: tower::JUDGMENT,
            name: "Judgment",
            warning: "The statues open their eyes... stand beside one!",
            first_secs: 450.0,
            every_secs: 90.0,
            warn_secs: 8.0,
            duration_secs: 15.0,
            light_scale: 0.85,
            shake: 40,
            calls_waves: false,
        },
        // The summit lasts to the end of the run: the chandeliers stay out.
        MapEvent {
            id: "summit",
            name: "The Summit",
            warning: "The chandeliers go out one by one... Thanatos is coming!",
            first_secs: 600.0,
            every_secs: 0.0,
            warn_secs: 15.0,
            duration_secs: 300.0,
            light_scale: 0.85,
            shake: 160,
            calls_waves: false,
        },
    ],
    objects: &TOWER_TUBES,
    strikes: &[super::environment::Strikes {
        damage: [2, 8],
        damage_type: "radiant",
        look: "holy",
        fall_secs: 1.0,
        extra: 1,
        extra_spread: 6.0,
        dc: 14,
        mask: tower::STATUES,
        mask_r: 3.0,
        hook: Some(tower::JUDGMENT),
        ..super::environment::Strikes::new(tower::BEAM, tower::JUDGMENT, 2.5, 3.0)
    }],
};

/// Names shared with `hub/thanatos.rs`.
pub mod tower {
    /// The Judgment's map event and the strikes' hook key.
    pub const JUDGMENT: &str = "judgment";
    /// The beams' zones (effects.toml `[skills.tower_beam]`).
    pub const BEAM: &str = "tower_beam";
    /// The angel statues of the court and the apse (cells, at their bases):
    /// no beam lands within 3 cells of one.
    pub const STATUES: &[(f32, f32)] = &[
        (134.0, 138.0),
        (166.0, 138.0),
        (134.0, 162.0),
        (166.0, 162.0),
        (154.5, 192.0),
        (142.0, 187.5),
        (159.0, 189.0),
        (154.0, 185.0),
        (152.0, 200.0),
        (141.0, 201.0),
        (157.0, 201.0),
        (144.0, 206.0),
        (238.0, 64.0),
        (187.0, 275.0),
        (113.0, 275.0),
    ];
}

/// One specimen tube of colour `$c` (`r`, `b`, `g`, `p`, `y`) at a spot,
/// cracking `$secs` into the run into two plasmas of its colour.
macro_rules! tube {
    ($c:literal, $x:expr, $y:expr, $secs:expr) => {
        ObjectDef {
            monster: concat!("specimen_tube_", $c),
            spots: &[($x, $y)],
            on_felled: ObjectEffect::None,
            hatch: Some(Hatch::once($secs, concat!("plasma_", $c), 2)),
        }
    };
}

/// A cluster: one tube of each colour on a ring round a spot (checked
/// walkable in the bake's walk grid).
macro_rules! tubes {
    ($x:expr, $y:expr, $secs:expr) => {
        [
            tube!("r", $x, $y + 2.2, $secs),
            tube!("b", $x - 2.1, $y + 0.7, $secs),
            tube!("g", $x - 1.3, $y - 1.8, $secs),
            tube!("p", $x + 1.3, $y - 1.8, $secs),
            tube!("y", $x + 2.1, $y + 0.7, $secs),
        ]
    };
}

/// Eight clusters by the RSW's own tube groups: the four in the galleries
/// crack first (from 2:00), then the wings, one every 30 s.
const TOWER_CLUSTERS: [[ObjectDef; 5]; 8] = [
    tubes!(118.5, 128.5, 120.0),
    tubes!(182.5, 129.5, 150.0),
    tubes!(117.5, 170.5, 180.0),
    tubes!(182.5, 170.5, 210.0),
    tubes!(23.5, 110.5, 240.0),
    tubes!(273.5, 117.5, 270.0),
    tubes!(24.5, 181.5, 300.0),
    tubes!(276.5, 186.5, 330.0),
];

/// The clusters, flat (`Environment::objects`).
const TOWER_TUBES: [ObjectDef; 40] = {
    let mut out = [TOWER_CLUSTERS[0][0]; 40];
    let mut i = 0;
    while i < 40 {
        out[i] = TOWER_CLUSTERS[i / 5][i % 5];
        i += 1;
    }
    out
};
// <<< thanatos_tower: env <<<

pub fn find(id: &str) -> Option<&'static MissionInfo> {
    MISSIONS.iter().find(|m| m.id == id)
}

/// Whether the baked monster `d` is in `mission`'s roster: tagged with the
/// mission (`director.missions`), untagged and the mission is the legacy
/// one, or listed in its [`MissionInfo::roster`]. Plants (scenery the
/// location's `plants.json` picks from) go wherever they are not scoped
/// away from. A mission outside the ladder (tests, a custom table) takes
/// the untagged monsters too.
pub fn in_roster(mission: &MissionInfo, d: &MonsterDef) -> bool {
    let tagged = d.missions.iter().any(|m| m == mission.id);
    let untagged = d.missions.is_empty();
    tagged
        || mission.roster.iter().any(|r| r.monster == d.id)
        || (untagged && (d.is_plant() || mission.id == LEGACY_ROSTER || find(mission.id).is_none()))
}

/// `amount` scaled from level `from` to level `to` the way kill zeny
/// scales (`items::kill_zeny`: linear in level + 1), rounded.
pub fn zeny_at_level(amount: u32, from: u32, to: u32) -> u32 {
    let (from, to) = (from.max(1) as u64 + 1, to.max(1) as u64 + 1);
    ((amount as u64 * to + from / 2) / from).clamp(1, items::MAX_STACK as u64) as u32
}

/// The catalogue a stage of `mission` spawns from: the roster's monsters
/// with the mission's roster tweaks, and every monster whose RO level is
/// under the mission's monster band raised to its floor (COMBAT.md
/// "Monster level scaling"): it spawns at that level and up, so its stat
/// block, experience and kill zeny follow (they derive from the spawn
/// level), and its baked zeny drop rows scale by `(floor + 1) / (RO level + 1)`.
/// Item drops are kept. Kind indices are the roster's own (`STAGE_JOINED`
/// names them).
pub fn roster(mission: &MissionInfo, catalog: &MonsterCatalog) -> MonsterCatalog {
    let floor = mission.waves.levels.0.max(1);
    let defs: Vec<MonsterDef> = catalog
        .defs()
        .iter()
        .filter(|d| in_roster(mission, d))
        .map(|d| {
            let mut d = d.clone();
            if let Some(r) = mission.roster.iter().find(|r| r.monster == d.id) {
                if r.weight > 0 {
                    d.weight = r.weight;
                }
                if r.unlock_secs >= 0.0 {
                    d.unlock_secs = r.unlock_secs;
                }
                d.retire_secs = r.retire_secs;
            }
            if !d.is_plant() && d.level < floor {
                for row in d.drops.iter_mut().filter(|r| r.is_zeny()) {
                    row.min = zeny_at_level(row.min, d.level, floor);
                    row.max = zeny_at_level(row.max, d.level, floor).max(row.min);
                }
                d.level = floor;
            }
            d
        })
        .collect();
    MonsterCatalog::new(defs).unwrap_or_else(|e| {
        tracing::warn!("mission {}: roster rejected ({e}); it spawns nothing", mission.id);
        MonsterCatalog::default()
    })
}

/// Whether a roster can run a mission: at least one monster the game
/// master can buy.
pub fn has_monsters(roster: &MonsterCatalog) -> bool {
    roster.defs().iter().any(|d| !d.is_plant())
}

/// What the mission window and the in-run timeline show about a mission
/// (`MISSION_INFO`), derived from the live tables only: the mission's
/// [`WaveRules`] and time limit, and the roster catalogue the stage's horde
/// spawns from (`monsters.json`: roster, unlock times, levels, drop tables).
/// Nothing here is written by hand, so a changed roster or drop table shows
/// up in the window as it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct MissionBrief {
    pub mission: String,
    /// Stage map; the client finds the illustration by it.
    pub map: String,
    pub time_limit_secs: u32,
    /// Monster level band (`WaveRules::levels`).
    pub levels: (u32, u32),
    /// Monsters gain a level this often (seconds).
    pub level_every_secs: u32,
    /// Every kind that can spawn before the time limit, by unlock time.
    pub monsters: Vec<MonsterBrief>,
    /// Player level band: min, recommended, max.
    pub band: (u32, u32, u32),
    /// Light radius multiplier of the run, per mille.
    pub light_permille: u32,
    /// The map's timed events, for the timeline.
    pub events: Vec<EventBrief>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonsterBrief {
    pub id: String,
    pub name: String,
    /// `swarm`, `chaser`, `tank`, `elite`, `boss`.
    pub archetype: String,
    pub element: String,
    pub size: String,
    /// First moment the game master may send it (seconds).
    pub unlock_secs: u32,
    /// Levels it spawns at between its unlock and the time limit.
    pub levels: (u32, u32),
    /// Hit points at those levels (one player).
    pub hp: (u32, u32),
    /// Pack size range the game master sends it in.
    pub pack: (u32, u32),
    pub drops: Vec<DropBrief>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropBrief {
    /// Item id, or `zeny`.
    pub item: String,
    /// Chance in hundredths of a percent (10000 = always).
    pub rate: u32,
    pub min: u32,
    pub max: u32,
}

/// A map event on the timeline: every occurrence before the time limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventBrief {
    pub id: String,
    pub name: String,
    /// Seconds into the run of each occurrence.
    pub starts: Vec<u32>,
    pub warn_secs: u32,
    pub duration_secs: u32,
}

fn event_brief(e: &MapEvent, limit: f32) -> EventBrief {
    let mut starts = Vec::new();
    let mut n = 0;
    while let Some(s) = e.start(n) {
        if s >= limit || starts.len() >= 64 {
            break;
        }
        starts.push(s.round() as u32);
        n += 1;
    }
    EventBrief {
        id: e.id.into(),
        name: e.name.into(),
        starts,
        warn_secs: e.warn_secs.round() as u32,
        duration_secs: e.duration_secs.round() as u32,
    }
}

/// Builds the brief of `mission` from its roster catalogue ([`roster`]).
pub fn brief(mission: &MissionInfo, rules: &WaveRules, catalog: &MonsterCatalog) -> MissionBrief {
    brief_with(mission, rules, catalog, &MonsterScale::PLAIN)
}

/// [`brief`] of a run under mission modifiers: hit points as `scale`
/// makes them (a stage's own brief).
pub fn brief_with(mission: &MissionInfo, rules: &WaveRules, catalog: &MonsterCatalog, scale: &MonsterScale) -> MissionBrief {
    let limit = mission.time_limit_secs;
    let mut monsters: Vec<MonsterBrief> = catalog
        .defs()
        .iter()
        // Plants are scenery the stage plants, not waves the brief lists.
        .filter(|d| d.unlock_secs < limit && d.unlock_secs < d.retire_secs && !d.is_plant())
        .map(|d| {
            let lo = rules.level_at(d.level, d.boss, d.unlock_secs);
            // A kind that retires is last bought then.
            let hi = rules.level_at(d.level, d.boss, limit.min(d.retire_secs));
            let hp = |level: u32| scale.apply(MonsterBlock::new(level, d.archetype, 1)).hp.max(0) as u32;
            MonsterBrief {
                id: d.id.clone(),
                name: d.name.clone(),
                archetype: d.archetype.id().into(),
                element: d.element.id().into(),
                size: d.size.id().into(),
                unlock_secs: d.unlock_secs.round() as u32,
                levels: (lo, hi),
                hp: (hp(lo), hp(hi)),
                pack: (d.pack.0.into(), d.pack.1.into()),
                drops: d.drops.iter().map(|r| DropBrief { item: r.item.into(), rate: r.rate, min: r.min, max: r.max }).collect(),
            }
        })
        .collect();
    monsters.sort_by(|a, b| a.unlock_secs.cmp(&b.unlock_secs).then_with(|| a.id.cmp(&b.id)));
    let b = mission.levels;
    MissionBrief {
        mission: mission.id.into(),
        map: mission.map.into(),
        time_limit_secs: limit.round() as u32,
        levels: (rules.levels.0.max(1), rules.levels.1.max(rules.levels.0.max(1))),
        level_every_secs: rules.level_every_secs.round() as u32,
        monsters,
        band: (b.min, b.recommended, b.max),
        light_permille: (mission.env.light_scale * 1000.0).round().max(0.0) as u32,
        events: mission.env.events.iter().map(|e| event_brief(e, limit)).collect(),
    }
}

/// Whether a mission can be started on this server, and if not why: the
/// card's status (`MISSION_LIST`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Open,
    /// Its location is not in the baked pack (or has no monsters yet).
    ComingSoon,
}

/// A mission card (`MISSION_LIST`): everything a rich mission list shows,
/// from the table and the live roster. Built per request (cheap: a
/// handful of missions).
#[derive(Clone, Debug, PartialEq)]
pub struct MissionCard {
    pub id: String,
    pub name: String,
    pub description: String,
    pub story: String,
    pub map: String,
    pub place: String,
    /// `field` or `dungeon`.
    pub terrain: String,
    pub art: String,
    /// Player band: min, recommended, max.
    pub band: (u32, u32, u32),
    /// Monster levels: the band of the waves, bosses at max + 5.
    pub monster_levels: (u32, u32),
    pub time_limit_secs: u32,
    pub max_players: u8,
    /// Boss names in order of appearance.
    pub bosses: Vec<String>,
    pub tags: Vec<String>,
    pub tips: Vec<String>,
    /// One line about what a clear pays.
    pub rewards: String,
    pub available: Availability,
    /// Why it cannot be started (empty when open).
    pub reason: String,
    /// Position on the ladder (0-based).
    pub step: u8,
}

/// `golden_thief_bug` → `Golden Thief Bug`.
fn title(id: &str) -> String {
    id.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The card of `mission`; `roster` is its live roster catalogue when the
/// server has monsters, `map_loaded` whether its location is in the pack.
pub fn card(step: usize, mission: &MissionInfo, roster: Option<&MonsterCatalog>, map_loaded: bool) -> MissionCard {
    let live = roster.filter(|r| has_monsters(r));
    let available = if map_loaded && (roster.is_none() || live.is_some()) { Availability::Open } else { Availability::ComingSoon };
    let reason = match available {
        Availability::Open => String::new(),
        Availability::ComingSoon if !map_loaded => format!("{} is not open yet: coming soon.", mission.place),
        Availability::ComingSoon => format!("The monsters of {} are still on their way: coming soon.", mission.place),
    };
    // Bosses: the live roster's by unlock, then the table's not baked yet.
    let mut bosses: Vec<(f32, String, String)> = live
        .map(|r| r.defs().iter().filter(|d| d.boss && d.unlock_secs < mission.time_limit_secs).map(|d| (d.unlock_secs, d.id.clone(), d.name.clone())).collect())
        .unwrap_or_default();
    bosses.sort_by(|a, b| a.0.total_cmp(&b.0));
    for &(id, name) in mission.bosses {
        if !bosses.iter().any(|b| b.1 == id) {
            bosses.push((f32::INFINITY, id.into(), if name.is_empty() { title(id) } else { name.into() }));
        }
    }
    let mut tags: Vec<String> = mission.tags.iter().map(|t| t.to_string()).collect();
    if mission.fog.is_some() {
        tags.push("poison fog".into());
    }
    if mission.env.light_scale < 0.95 {
        tags.push("darkness".into());
    }
    tags.extend(mission.env.events.iter().map(|e| e.name.to_lowercase()));
    tags.dedup();
    let b = mission.levels;
    MissionCard {
        id: mission.id.into(),
        name: mission.name.into(),
        description: mission.description.into(),
        story: mission.story.into(),
        map: mission.map.into(),
        place: mission.place.into(),
        terrain: mission.terrain.id().into(),
        art: mission.art().into(),
        band: (b.min, b.recommended, b.max),
        monster_levels: (mission.waves.levels.0.max(1), mission.waves.levels.1),
        time_limit_secs: mission.time_limit_secs.round() as u32,
        max_players: mission.max_players,
        bosses: bosses.into_iter().map(|b| b.2).collect(),
        tags,
        tips: mission.tips.iter().map(|t| t.to_string()).collect(),
        rewards: format!(
            "Clear bonus {}+ zeny (paid at Lv {}+); monsters Lv {}–{} pay exp and zeny by level.",
            items::victory_zeny(b.min),
            b.min,
            mission.waves.levels.0.max(1),
            mission.waves.levels.1
        ),
        available,
        reason,
        step: step.min(u8::MAX as usize) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::combat::loot::Drop;
    use crate::world::combat::Archetype;
    use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog, MonsterDef};

    fn catalog(slime_drops: Vec<Drop>) -> MonsterCatalog {
        MonsterCatalog::new(vec![
            MonsterDef { level: 3, drops: slime_drops, ..test_def("slime", 2.0, 1, 0.0, false) },
            MonsterDef { level: 9, archetype: Archetype::Elite, ..test_def("knight", 2.0, 12, 240.0, false) },
            test_def("king", 3.0, 50, 600.0, true),
            test_def("never", 2.0, 1, 5000.0, false),
        ])
        .unwrap()
    }

    #[test]
    fn brief_comes_from_the_live_catalogue_and_rules() {
        let m = &MISSIONS[0];
        let rules = &m.waves;
        let c = catalog(vec![Drop::new("jellopy", 12.0, None).unwrap()]);
        let b = brief(m, rules, &c);
        assert_eq!((b.mission.as_str(), b.map.as_str()), (m.id, m.map));
        assert_eq!(b.time_limit_secs, m.time_limit_secs as u32);
        assert_eq!(b.levels, rules.levels);
        assert_eq!((b.band, b.light_permille, b.events.len()), ((1, 1, 15), 1000, 0));
        let ids: Vec<&str> = b.monsters.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["slime", "knight", "king"], "by unlock time; kinds unlocking after the limit never spawn");
        let (slime, knight, king) = (&b.monsters[0], &b.monsters[1], &b.monsters[2]);
        // Levels follow the spawn formula at unlock and at the time limit.
        assert_eq!(slime.levels, (rules.level_at(3, false, 0.0), rules.level_at(3, false, m.time_limit_secs)));
        assert_eq!(knight.levels.0, rules.level_at(9, false, 240.0));
        assert_eq!((knight.archetype.as_str(), knight.unlock_secs), ("elite", 240));
        assert_eq!((king.archetype.as_str(), king.levels.0), ("boss", rules.levels.1 + 5));
        assert_eq!(slime.hp.0, MonsterBlock::new(slime.levels.0, slime_def(&c).archetype, 1).hp as u32);
        assert_eq!(slime.drops, vec![DropBrief { item: "jellopy".into(), rate: 1200, min: 1, max: 1 }]);

        // A changed drop table or roster shows up as is.
        let c2 = catalog(vec![Drop::new("apple", 50.0, Some([2, 3])).unwrap()]);
        let b2 = brief(m, rules, &c2);
        assert_eq!(b2.monsters[0].drops, vec![DropBrief { item: "apple".into(), rate: 5000, min: 2, max: 3 }]);
        let faster = WaveRules { levels: (5, 30), level_every_secs: 60.0, ..rules.clone() };
        let b3 = brief(m, &faster, &c);
        assert_eq!((b3.levels, b3.level_every_secs), ((5, 30), 60));
        assert_eq!(b3.monsters[2].levels.0, 35);
    }

    fn slime_def(c: &MonsterCatalog) -> &MonsterDef {
        c.get(c.kind_of("slime").unwrap())
    }

    /// The real pack (when the asset submodule is baked): Poring Island's
    /// roster is every untagged monster, unchanged, so the island plays as
    /// it did before the ladder; every monster of it that can spawn is in
    /// the brief with its baked table.
    #[test]
    fn brief_matches_the_baked_pack() {
        let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
        let Ok(all) = MonsterCatalog::load(&dist) else { return };
        let m = &MISSIONS[0];
        let c = roster(m, &all);
        let legacy: Vec<&MonsterDef> = all.defs().iter().filter(|d| d.missions.is_empty()).collect();
        assert_eq!(c.defs().iter().collect::<Vec<_>>(), legacy, "Poring Island: the untagged monsters, as baked");
        let b = brief(m, &m.waves, &c);
        for d in c.defs().iter().filter(|d| d.unlock_secs < m.time_limit_secs && !d.is_plant()) {
            let got = b.monsters.iter().find(|x| x.id == d.id).unwrap_or_else(|| panic!("{} missing", d.id));
            assert_eq!(got.unlock_secs, d.unlock_secs.round() as u32);
            assert_eq!(got.drops.len(), d.drops.len());
            for (g, r) in got.drops.iter().zip(&d.drops) {
                assert_eq!((g.item.as_str(), g.rate, g.min, g.max), (r.item, r.rate, r.min, r.max));
            }
        }
        assert_eq!(b.monsters.iter().filter(|x| x.archetype == "boss").count(), c.defs().iter().filter(|d| d.boss && d.unlock_secs < m.time_limit_secs).count());
        // No mission's roster leaks into another's (plants aside).
        for other in &MISSIONS[1..] {
            let r = roster(other, &all);
            assert!(r.defs().iter().all(|d| d.is_plant() || d.missions.iter().any(|x| x == other.id) || other.roster.iter().any(|e| e.monster == d.id)));
        }
    }

    /// The baked pack: every expedition meets a mini-boss within its first
    /// minute and every elite by the middle of the run; each one has a drop
    /// table with its own card in it.
    #[test]
    fn every_expedition_meets_a_miniboss_early() {
        let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
        let Ok(all) = MonsterCatalog::load(&dist) else { return };
        for m in MISSIONS {
            let r = roster(m, &all);
            if !has_monsters(&r) {
                continue;
            }
            let elites: Vec<&MonsterDef> =
                r.defs().iter().filter(|d| d.archetype == Archetype::Elite && d.unlock_secs < m.time_limit_secs).collect();
            assert!(elites.iter().any(|d| d.unlock_secs <= 60.0), "{}: no mini-boss in the first minute", m.id);
            for d in &elites {
                assert!(d.unlock_secs <= m.time_limit_secs * 0.4, "{}: {} only at {} s", m.id, d.id, d.unlock_secs);
                let card = crate::world::combat::cards::of_monster(&d.id).unwrap_or_else(|| panic!("{} has no card", d.id));
                assert!(d.drops.iter().any(|r| r.item == card.id), "{} does not drop {}", d.id, card.id);
            }
        }
    }

    #[test]
    fn rosters_follow_the_missions_tags() {
        let tagged = |id: &str, mission: &str| MonsterDef { missions: vec![mission.into()], ..test_def(id, 2.0, 1, 0.0, false) };
        let all = MonsterCatalog::new(vec![
            test_def("poring", 2.0, 1, 0.0, false),
            tagged("thief_bug", "prontera_culvert"),
            MonsterDef { level: 60, ..tagged("golden_bug", "prontera_culvert") },
            test_plant("green_plant", (2, 3)),
            MonsterDef { missions: vec!["sograt_desert".into()], ..test_plant("cactus", (2, 3)) },
        ])
        .unwrap();
        let ids = |m: &MissionInfo| roster(m, &all).ids();
        assert_eq!(ids(&MISSIONS[0]), ["green_plant", "poring"], "the island: untagged only");
        assert_eq!(ids(find("prontera_culvert").unwrap()), ["golden_bug", "green_plant", "thief_bug"]);
        assert_eq!(ids(find("sograt_desert").unwrap()), ["cactus", "green_plant"]);
        assert!(!has_monsters(&roster(find("sograt_desert").unwrap(), &all)), "plants alone run nothing");
        // A roster row pulls in and tunes a monster of another mission.
        static SHARED: MissionInfo = MissionInfo {
            id: "shared",
            roster: &[RosterEntry::of("thief_bug").weight(77).unlock(30.0)],
            ..MissionInfo::TEMPLATE
        };
        let r = roster(&SHARED, &all);
        let bug = r.get(r.kind_of("thief_bug").unwrap());
        assert_eq!((bug.weight, bug.unlock_secs, bug.retire_secs), (77, 30.0, f32::INFINITY));
        // A roster row can retire a kind: the act it belongs to is over.
        static ACTS: MissionInfo = MissionInfo {
            id: "acts",
            roster: &[RosterEntry::of("thief_bug").unlock(0.0).retire(180.0), RosterEntry::of("poring").unlock(170.0)],
            ..MissionInfo::TEMPLATE
        };
        let r = roster(&ACTS, &all);
        assert_eq!(r.get(r.kind_of("thief_bug").unwrap()).retire_secs, 180.0);
        let b = brief(&ACTS, &ACTS.waves, &r);
        let bug = b.monsters.iter().find(|m| m.id == "thief_bug").unwrap();
        assert_eq!(bug.levels.1, ACTS.waves.level_at(1, false, 180.0), "last bought at its retire time");
        assert!(r.kind_of("golden_bug").is_none(), "another mission's monster stays out");
        assert!(r.kind_of("poring").is_some(), "a mission outside the ladder takes the untagged roster");
    }

    #[test]
    fn low_monsters_are_raised_into_the_band() {
        let drops = vec![Drop::new("zeny", 100.0, Some([10, 20])).unwrap(), Drop::new("jellopy", 50.0, Some([1, 2])).unwrap()];
        let all = MonsterCatalog::new(vec![
            MonsterDef { level: 4, drops, missions: vec!["prontera_culvert".into()], ..test_def("rat", 2.0, 1, 0.0, false) },
            MonsterDef { level: 22, missions: vec!["prontera_culvert".into()], ..test_def("croc", 2.0, 1, 0.0, false) },
        ])
        .unwrap();
        let m = find("prontera_culvert").unwrap();
        let r = roster(m, &all);
        let rat = r.get(r.kind_of("rat").unwrap());
        let croc = r.get(r.kind_of("croc").unwrap());
        assert_eq!((rat.level, croc.level), (15, 22), "raised to the floor, higher ones kept");
        // Zeny rows scale like kill zeny ((L + 1) / (l + 1) = 16 / 5), items are kept.
        assert_eq!((rat.drops[0].min, rat.drops[0].max), (32, 64));
        assert_eq!((rat.drops[1].min, rat.drops[1].max), (1, 2));
        assert_eq!(m.waves.level_at(rat.level, false, 0.0), 15);
        // The island is untouched: its floor is 1.
        assert_eq!(zeny_at_level(7, 1, 1), 7);
    }

    #[test]
    fn the_level_band_warns_and_never_locks() {
        let b = LevelBand::new(15, 18, 30);
        assert_eq!(b.fit(&[]), Fit::Ok);
        assert_eq!(b.fit(&[16]), Fit::Ok);
        assert_eq!(b.fit(&[3, 15]), Fit::Ok, "one member in the band carries the party");
        assert_eq!(b.fit(&[3, 14]), Fit::Under);
        assert_eq!(b.fit(&[40, 45]), Fit::Over);
        assert_eq!(b.fit(&[10, 40]), Fit::Ok);
        assert_eq!((b.clear_level(5), b.clear_level(25)), (15, 25));
        assert_eq!(MISSIONS[0].levels.clear_level(7), 7, "the island pays as before");
    }

    #[test]
    fn the_ladder_is_well_formed() {
        let ids: Vec<&str> = MISSIONS.iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            ["poring_island_survival", "prontera_culvert", "sograt_desert", "payon_caves", "orc_village", "toy_factory", "glast_heim", "thor_volcano", "thanatos_tower"]
        );
        for w in MISSIONS.windows(2) {
            assert!(w[1].levels.min >= w[0].levels.min, "{} after {}", w[1].id, w[0].id);
        }
        for m in MISSIONS {
            assert!(m.levels.min <= m.levels.recommended && m.levels.recommended <= m.levels.max, "{}", m.id);
            assert!(m.waves.levels.0 <= m.levels.min.max(1) && m.waves.levels.1 >= m.levels.max, "{}: monsters cover the band", m.id);
            assert!(!m.place.is_empty() && !m.story.is_empty(), "{}", m.id);
        }
        // Poring Island's rules are the ones it always had.
        let p = &MISSIONS[0];
        assert_eq!((p.waves.clone(), p.fog, p.time_limit_secs, p.env), (WaveRules::NORMAL, Some(FogRules::NORMAL), 900.0, Environment::NONE));
    }

    #[test]
    fn cards_say_what_is_open() {
        let all = catalog(Vec::new());
        let p = &MISSIONS[0];
        let open = card(0, p, Some(&roster(p, &all)), true);
        assert_eq!((open.available, open.reason.as_str(), open.art.as_str()), (Availability::Open, "", "prt_fild08"));
        assert_eq!(open.bosses, ["king", "Angeling"], "live bosses first, then the table's");
        assert!(open.tags.contains(&"poison fog".to_string()));
        assert_eq!(open.band, (1, 1, 15));
        let c = find("prontera_culvert").unwrap();
        let soon = card(1, c, Some(&roster(c, &all)), false);
        assert_eq!(soon.available, Availability::ComingSoon);
        assert!(soon.reason.contains("coming soon"));
        assert_eq!((soon.bosses.as_slice(), soon.terrain.as_str(), soon.step), (&["Golden Thief Bug".to_string()][..], "dungeon", 1));
        let empty = card(1, c, Some(&roster(c, &all)), true);
        assert_eq!(empty.available, Availability::ComingSoon, "a map without monsters is not open");
        assert_eq!(card(1, c, None, true).available, Availability::Open, "no catalogue at all: the map decides");
        // The ladder from 60 to 99: listed, coming soon until baked.
        for id in ["orc_village", "toy_factory", "glast_heim", "thor_volcano", "thanatos_tower"] {
            let m = find(id).unwrap();
            assert_eq!(card(4, m, Some(&roster(m, &all)), true).available, Availability::ComingSoon, "{id}: no monsters of its own yet");
            assert_eq!(card(4, m, None, false).available, Availability::ComingSoon, "{id}: no map yet");
        }
    }

    #[test]
    fn events_show_every_occurrence_on_the_timeline() {
        let e = MapEvent { id: "tide", name: "Tide", warning: "", first_secs: 120.0, every_secs: 300.0, warn_secs: 15.0, duration_secs: 40.0, light_scale: 1.0, shake: 0, calls_waves: false };
        let b = event_brief(&e, 900.0);
        assert_eq!((b.starts.as_slice(), b.warn_secs, b.duration_secs), (&[120, 420, 720][..], 15, 40));
    }
}
