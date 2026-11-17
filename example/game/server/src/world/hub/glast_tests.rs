//! Glast Heim's churchyard on a small field (`glast.rs`): a grave keeps
//! letting Ghouls out until broken, the Witching Hour raises the broken
//! ones, a holy font heals the living and burns the dead then runs dry,
//! the Zombie Master raises fresh graves; and the real mission entry plants
//! its objects on the real map.

use super::super::env::Env;
use super::super::test_support::map;
use super::super::{HubEvent, HubSimConfig, HubWorld, PlayerProfile};
use super::*;
use crate::world::combat;
use crate::world::environment::{Environment, MapEvent, ObjectDef};
use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog, MonsterDef};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::missions::{self, MissionInfo};
use crate::world::protocol::Look;
use std::sync::Arc;
use uuid::Uuid;

const HOUR: MapEvent = MapEvent {
    id: WITCHING_HOUR,
    name: "The Witching Hour",
    warning: "",
    first_secs: 60.0,
    every_secs: 60.0,
    warn_secs: 0.0,
    duration_secs: 10.0,
    light_scale: 0.7,
    shake: 0,
    calls_waves: true,
};

static MISSION_TEST: MissionInfo = MissionInfo {
    id: MISSION,
    name: "Glast",
    map: "field",
    fog: None,
    env: Environment {
        light_scale: 0.7,
        hazards: &[],
        events: &[HOUR],
        objects: &[
            ObjectDef { monster: GRAVE, spots: &[(10.5, 10.5)], on_felled: ObjectEffect::Hook(GRAVE_HOOK), hatch: Some(Hatch::every(5.0, 10.0, "ghoul", 2)) },
            ObjectDef { monster: FONT, spots: &[(30.5, 10.5)], on_felled: ObjectEffect::Hook(FONT_HOOK), hatch: None },
        ],
        strikes: &[],
    },
    ..MissionInfo::TEMPLATE
};

fn stage() -> HubInstance {
    let wall = "#".repeat(42);
    let row = format!("#{}#", ".".repeat(40));
    let mut rows = vec![wall.clone()];
    rows.extend(std::iter::repeat_n(row, 21));
    rows.push(wall);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let undead = |id: &str| MonsterDef { evil: true, level: 80, ..test_def(id, 0.1, 1, 0.0, false) };
    let catalog = Arc::new(
        MonsterCatalog::new(vec![
            undead("ghoul"),
            undead("skel_prisoner"),
            undead("zombie_master"),
            test_def("hunter_fly", 0.1, 1, 0.0, false),
            test_plant(GRAVE, (6, 6)),
            test_plant(FRESH_GRAVE, (2, 2)),
            test_plant(FONT, (3, 3)),
            test_plant(DRY_FONT, (60000, 60000)),
        ])
        .unwrap(),
    );
    let cfg = HordeConfig { light_radius: 20.0, director_enabled: false, seed: Some(2), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, MISSION_TEST.id, world, HubSimConfig { residents: 0, ..Default::default() }).with_horde(horde);
    stage.env = Env::new(Some(&MISSION_TEST), &[]);
    stage.dice = combat::Rng::new(5);
    let profile = PlayerProfile {
        user_id: 1,
        character_id: 101,
        name: "p1".into(),
        archetype: "swordsman".into(),
        look: Look::default(),
        level: 75,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
        homunculus: None,
    };
    let mut out = Vec::new();
    stage.join(profile, Uuid::nil(), None, 0, &mut out);
    let i = stage.find(1).unwrap();
    (stage.players[i].x, stage.players[i].y) = (31.5, 12.5);
    stage
}

fn at(stage: &mut HubInstance, secs: f32, out: &mut Outgoing) {
    stage.horde_mut().unwrap().set_director_elapsed(secs);
    stage.tick(out);
}

fn msgs(out: &Outgoing) -> Vec<ServerMsg> {
    out.iter()
        .filter(|(u, _)| *u == 1)
        .filter_map(|(_, e)| match e {
            HubEvent::Control(m) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

fn count(s: &HubInstance, monster: &str) -> usize {
    let horde = s.horde.as_ref().unwrap();
    let kind = horde.catalog().kind_of(monster).unwrap();
    horde.monsters().filter(|m| m.2 == kind).count()
}

/// Fells the standing object `monster` and runs an object scan.
fn fell(s: &mut HubInstance, monster: &str, from: f32, out: &mut Outgoing) {
    let horde = s.horde_mut().unwrap();
    let kind = horde.catalog().kind_of(monster).unwrap();
    let slot = horde.monsters().find(|m| m.2 == kind).expect("the object stands").0;
    horde.despawn(slot);
    for t in 0..10 {
        at(s, from + t as f32 * 0.05, out);
    }
}

fn zones_of(m: &[ServerMsg], key: &str) -> Option<Vec<BossZone>> {
    m.iter().find_map(|m| match m {
        ServerMsg::BossZones { caster: 0, ability, zones } if ability == key => Some(zones.clone()),
        _ => None,
    })
}

#[test]
fn a_grave_keeps_opening_until_broken_and_the_witching_hour_raises_it() {
    let mut s = stage();
    let mut out = Vec::new();
    // Two broods: at 5 s and 15 s.
    for t in 0..170 {
        at(&mut s, 0.5 + t as f32 * 0.1, &mut out);
    }
    assert_eq!(count(&s, "ghoul"), 4, "two broods of two");
    assert_eq!(count(&s, GRAVE), 1, "the grave stays");

    let mut out = Vec::new();
    fell(&mut s, GRAVE, 18.0, &mut out);
    assert!(zones_of(&msgs(&out), CONSECRATE_ABILITY).is_some(), "the consecration shows");
    assert_eq!(s.env.glast.felled(), 1);
    for t in 0..300 {
        at(&mut s, 19.0 + t as f32 * 0.1, &mut out);
    }
    assert_eq!(count(&s, "ghoul"), 4, "a broken grave lets nothing out");

    // The Witching Hour (60 s) raises it; its first brood comes RISE_SECS later.
    let mut out = Vec::new();
    for t in 0..10 {
        at(&mut s, 60.0 + t as f32 * 0.1, &mut out);
    }
    assert!(zones_of(&msgs(&out), RISE_ABILITY).is_some(), "the rising shows");
    assert_eq!((count(&s, GRAVE), s.env.glast.felled()), (1, 0), "risen where it stood");
    for t in 0..80 {
        at(&mut s, 61.0 + t as f32 * 0.1, &mut out);
    }
    assert_eq!(count(&s, "ghoul"), 6, "it opens again");
    // Broken again, the next hour raises it with Skeleton Prisoners.
    fell(&mut s, GRAVE, 70.0, &mut out);
    for t in 0..120 {
        at(&mut s, 120.0 + t as f32 * 0.1, &mut out);
    }
    assert_eq!(count(&s, "skel_prisoner"), 2, "the second rising lets prisoners out");
}

#[test]
fn a_holy_font_heals_the_living_burns_the_dead_and_runs_dry() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    let g = s.horde.as_ref().unwrap().catalog().kind_of("ghoul").unwrap();
    let f = s.horde.as_ref().unwrap().catalog().kind_of("hunter_fly").unwrap();
    let ghoul = s.horde_mut().unwrap().spawn(g, 33.0, 10.5).unwrap();
    let fly = s.horde_mut().unwrap().spawn(f, 33.0, 11.5).unwrap();
    let i = s.find(1).unwrap();
    let max = s.players[i].fighter.as_ref().unwrap().max_hp;
    s.players[i].fighter.as_mut().unwrap().hp = max / 2;

    let mut out = Vec::new();
    fell(&mut s, FONT, 2.0, &mut out);
    let pool = zones_of(&msgs(&out), FONT_ABILITY).expect("the hallowed pool shows");
    assert_eq!((pool[0].area.a, pool[0].flags), (POOL_R, crate::world::protocol::zone_flags::SAFE));
    assert_eq!(s.env.glast.pools(), 1);
    assert_eq!((count(&s, FONT), count(&s, DRY_FONT)), (0, 1), "a dry font stands there");
    for t in 0..30 {
        at(&mut s, 3.0 + t as f32 * 0.1, &mut out);
    }
    let hp = s.players[i].fighter.as_ref().unwrap().hp;
    assert!(hp > max / 2, "the pool heals: {hp} of {max}");
    let store = s.horde.as_ref().unwrap().store();
    assert!(store.status_of(ghoul).has(Condition::Burning), "the dead burn");
    assert!(!store.status_of(fly).has(Condition::Burning), "the living do not");

    // The pool dries up; the font fills again after DRY_SECS.
    let mut out = Vec::new();
    at(&mut s, 2.0 + POOL_SECS + 0.5, &mut out);
    assert_eq!(s.env.glast.pools(), 0);
    assert!(msgs(&out).iter().any(|m| matches!(m, ServerMsg::ZoneEnd { .. })));
    at(&mut s, 2.0 + DRY_SECS + 1.5, &mut out);
    assert_eq!((count(&s, FONT), count(&s, DRY_FONT)), (1, 0), "ready again");
}

#[test]
fn a_zombie_master_raises_fresh_graves_that_open_unless_broken() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 0.5, &mut out);
    // The test grave out of the way (its broods would count).
    fell(&mut s, GRAVE, 0.6, &mut out);
    let z = s.horde.as_ref().unwrap().catalog().kind_of("zombie_master").unwrap();
    s.horde_mut().unwrap().spawn(z, 20.0, 12.5).unwrap();
    let mut out = Vec::new();
    for t in 0..80 {
        at(&mut s, 1.0 + t as f32 * 0.1, &mut out);
    }
    assert!(zones_of(&msgs(&out), RAISE_ABILITY).is_some(), "the raise shows");
    assert_eq!(count(&s, FRESH_GRAVE), 2, "two fresh graves");
    for t in 0..80 {
        at(&mut s, 9.0 + t as f32 * 0.1, &mut out);
    }
    assert_eq!((count(&s, FRESH_GRAVE), count(&s, "ghoul")), (0, 2), "they opened into a Ghoul each");
}

#[test]
fn no_witching_hour_and_no_raise_once_the_fog_closed_the_waves() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 0.5, &mut out);
    fell(&mut s, GRAVE, 0.6, &mut out);
    assert_eq!(s.env.glast.felled(), 1);
    s.horde_mut().unwrap().close_spawning();
    let z = s.horde.as_ref().unwrap().catalog().kind_of("zombie_master").unwrap();
    s.horde_mut().unwrap().spawn(z, 20.0, 12.5).unwrap();
    let mut out = Vec::new();
    for t in 0..700 {
        at(&mut s, 1.0 + t as f32 * 0.1, &mut out);
    }
    let m = msgs(&out);
    assert!(!m.iter().any(|m| matches!(m, ServerMsg::EnvEvent { id, .. } if id == WITCHING_HOUR)), "the bell does not toll");
    assert!(zones_of(&m, RISE_ABILITY).is_none() && zones_of(&m, RAISE_ABILITY).is_none(), "no grave rises or is raised");
    assert_eq!((count(&s, GRAVE), count(&s, FRESH_GRAVE), s.env.glast.felled()), (0, 0, 1));
}

#[test]
fn the_glast_mission_plants_graves_and_fonts_on_walkable_ground() {
    let m = missions::find(MISSION).unwrap();
    assert!((m.env.light_scale - 0.7).abs() < 1e-6);
    let graves: Vec<_> = m.env.objects.iter().filter(|o| o.monster == GRAVE).collect();
    assert_eq!(graves.len(), 10);
    assert!(graves.iter().all(|o| o.on_felled == ObjectEffect::Hook(GRAVE_HOOK) && o.hatch.is_some_and(|h| h.every_secs > 0.0)));
    let first: Vec<f32> = graves.iter().map(|o| o.hatch.unwrap().secs).collect();
    assert_eq!(first[0], 90.0, "the graves start opening at 1:30");
    assert!(m.env.event(WITCHING_HOUR).is_some_and(|e| e.first_secs == 240.0 && e.every_secs == 240.0 && e.calls_waves));
    assert_eq!(m.env.hazards.iter().filter(|h| h.during == Some(WITCHING_HOUR)).count(), 10, "a cursed plot per grave");
    assert!(m.env.hazards.iter().all(|h| h.problem().is_none()));
    // On the baked map (when the pack has it): every spot is walkable.
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    let Ok(map) = crate::world::map::MapData::load(&dist, "gl_chyard") else { return };
    for &(x, y) in m.env.objects.iter().flat_map(|o| o.spots) {
        assert!(map.walk.is_walkable(x, y), "object spot ({x}, {y}) is walkable");
    }
}
