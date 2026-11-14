//! Payon Caves' braziers on a small field (`payon.rs`): kindling one lights
//! a pool and the run, the dead in it burn, it burns out and stands cold
//! again; and the real mission entry plants them on the real map.

use super::super::env::Env;
use super::super::test_support::map;
use super::super::{HubEvent, HubSimConfig, HubWorld, PlayerProfile};
use super::*;
use crate::world::combat;
use crate::world::environment::{Environment, ObjectDef};
use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog, MonsterDef};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::missions::{self, MissionInfo};
use crate::world::protocol::Look;
use std::sync::Arc;
use uuid::Uuid;

static MISSION_TEST: MissionInfo = MissionInfo {
    id: MISSION,
    name: "Payon",
    map: "field",
    fog: None,
    env: Environment {
        light_scale: 0.5,
        hazards: &[],
        events: &[],
        objects: &[
            ObjectDef { monster: BRAZIER_MONSTER, spots: &[(10.5, 10.5)], on_felled: ObjectEffect::Hook(BRAZIER_HOOK), hatch: None },
            ObjectDef { monster: BELL_MONSTER, spots: &[(30.5, 10.5)], on_felled: ObjectEffect::Hook(BELL_HOOK), hatch: None },
        ],
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
    let undead = MonsterDef { evil: true, ..test_def("zombie", 0.5, 1, 0.0, false) };
    let catalog = Arc::new(
        MonsterCatalog::new(vec![
            undead,
            test_def("lunatic", 0.5, 1, 0.0, false),
            test_def("familiar", 0.1, 1, 0.0, false),
            test_def("horong", 0.1, 1, 0.0, false),
            test_plant(BRAZIER_MONSTER, (4, 4)),
            test_plant(BELL_MONSTER, (3, 3)),
            test_plant(QUIET_BELL, (60000, 60000)),
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
        level: 50,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
    };
    let mut out = Vec::new();
    stage.join(profile, Uuid::nil(), None, 0, &mut out);
    let i = stage.find(1).unwrap();
    (stage.players[i].x, stage.players[i].y) = (35.5, 18.5);
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

fn kindle(s: &mut HubInstance, from: f32, out: &mut Outgoing) {
    fell(s, BRAZIER_MONSTER, from, out);
}

fn count(s: &HubInstance, monster: &str) -> usize {
    let horde = s.horde.as_ref().unwrap();
    let kind = horde.catalog().kind_of(monster).unwrap();
    horde.monsters().filter(|m| m.2 == kind).count()
}

#[test]
fn a_kindled_brazier_lights_a_pool_burns_the_dead_and_stands_cold_again() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    assert_eq!(s.horde.as_ref().unwrap().plants(), 2, "the cold brazier and the bell are planted");
    let z = s.horde.as_ref().unwrap().catalog().kind_of("zombie").unwrap();
    let l = s.horde.as_ref().unwrap().catalog().kind_of("lunatic").unwrap();
    let zombie = s.horde_mut().unwrap().spawn(z, 12.0, 10.5).unwrap();
    let lunatic = s.horde_mut().unwrap().spawn(l, 12.0, 11.5).unwrap();

    let mut out = Vec::new();
    kindle(&mut s, 2.0, &mut out);
    let m = msgs(&out);
    let pool = m.iter().find_map(|m| match m {
        ServerMsg::BossZones { caster: 0, ability, zones } if ability == BRAZIER_ABILITY => Some(zones.clone()),
        _ => None,
    });
    let pool = pool.expect("the lit pool shows");
    assert_eq!(pool.len(), 2, "the pool and the flame");
    assert_eq!((pool[0].area.a, pool[0].flags), (POOL_R, crate::world::protocol::zone_flags::SAFE));
    assert!(m.iter().any(|m| matches!(m, ServerMsg::EnvLight { .. })), "the run's light grows");
    assert!((s.env_light_scale() - LIGHT).abs() < 1e-3);
    assert_eq!(s.env.payon.lit(), 1);
    let store = s.horde.as_ref().unwrap().store();
    assert!(store.status_of(zombie).has(crate::world::combat::Condition::Burning), "the dead in the pool burn");
    assert!(!store.status_of(lunatic).has(crate::world::combat::Condition::Burning), "the living do not");

    // It burns out, the light falls back and a cold one stands again.
    let mut out = Vec::new();
    at(&mut s, 2.0 + LIT_SECS + 0.5, &mut out);
    let m = msgs(&out);
    assert_eq!(m.iter().filter(|m| matches!(m, ServerMsg::ZoneEnd { .. })).count(), 2);
    assert_eq!(s.env.payon.lit(), 0);
    assert_eq!(count(&s, BRAZIER_MONSTER), 1, "planted cold again");
    let mut out = Vec::new();
    at(&mut s, 2.0 + LIT_SECS + 1.0, &mut out);
    assert!((s.env_light_scale() - 1.0).abs() < 1e-3);
    // And it can be kindled again.
    let mut out = Vec::new();
    kindle(&mut s, 70.0, &mut out);
    assert_eq!(s.env.payon.lit(), 1);
}

#[test]
fn the_light_grows_with_at_most_four_braziers() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    for i in 0..6 {
        s.payon_kindle(0, 5.5 + i as f32 * 4.0, 5.5, &mut out);
    }
    at(&mut s, 1.5, &mut out);
    assert!((s.env_light_scale() - LIGHT.powi(MAX_BOOSTS as i32)).abs() < 1e-3, "{}", s.env_light_scale());
}

#[test]
fn the_payon_mission_is_dark_and_plants_its_braziers_and_bells_on_walkable_ground() {
    let m = missions::find(MISSION).unwrap();
    assert!((m.env.light_scale - 0.55).abs() < 1e-6);
    assert_eq!(m.env.objects[0].spots.len(), 23);
    assert!(m.env.objects[0].monster == BRAZIER_MONSTER && m.env.objects[0].on_felled == ObjectEffect::Hook(BRAZIER_HOOK));
    assert!(m.env.objects[1].monster == BELL_MONSTER && m.env.objects[1].on_felled == ObjectEffect::Hook(BELL_HOOK));
    // On the baked map (when the pack has it): every spot is walkable.
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    let Ok(map) = crate::world::map::MapData::load(&dist, "pay_dun04") else { return };
    for &(x, y) in m.env.objects.iter().flat_map(|o| o.spots) {
        assert!(map.walk.is_walkable(x, y), "object spot ({x}, {y}) is walkable");
    }
}

#[test]
fn a_rung_bell_stuns_the_dead_then_rests_before_it_rings_again() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    let z = s.horde.as_ref().unwrap().catalog().kind_of("zombie").unwrap();
    let l = s.horde.as_ref().unwrap().catalog().kind_of("lunatic").unwrap();
    let zombie = s.horde_mut().unwrap().spawn(z, 34.0, 10.5).unwrap();
    let far = s.horde_mut().unwrap().spawn(z, 8.0, 18.0).unwrap();
    let lunatic = s.horde_mut().unwrap().spawn(l, 33.0, 12.0).unwrap();
    let mut out = Vec::new();
    fell(&mut s, BELL_MONSTER, 2.0, &mut out);
    let m = msgs(&out);
    let rings = m.iter().find_map(|m| match m {
        ServerMsg::BossZones { caster: 0, ability, zones } if ability == BELL_ABILITY => Some(zones.clone()),
        _ => None,
    });
    let rings = rings.expect("the shockwave shows");
    assert_eq!(rings.len(), 3);
    assert!(rings.iter().all(|r| r.area.shape == 3 && r.flags == crate::world::protocol::zone_flags::SAFE), "calm gold rings");
    assert!(m.iter().any(|m| matches!(m, ServerMsg::EnvShake { .. })));
    let store = s.horde.as_ref().unwrap().store();
    assert!(store.status_of(zombie).has(crate::world::combat::Condition::Stunned), "the dead nearby reel");
    assert!(!store.status_of(far).has(crate::world::combat::Condition::Stunned), "out of its reach");
    assert!(!store.status_of(lunatic).has(crate::world::combat::Condition::Stunned), "the living do not");
    assert_eq!((count(&s, BELL_MONSTER), count(&s, QUIET_BELL)), (0, 1), "a quiet bell hangs there");
    // Before its cooldown nothing changes; after it, the bell is ready again.
    let mut out = Vec::new();
    at(&mut s, 2.0 + BELL_COOLDOWN - 5.0, &mut out);
    assert_eq!(count(&s, QUIET_BELL), 1);
    at(&mut s, 2.0 + BELL_COOLDOWN + 1.5, &mut out);
    assert_eq!((count(&s, BELL_MONSTER), count(&s, QUIET_BELL)), (1, 0), "ready to ring");
}

#[test]
fn familiars_snuff_lit_braziers_and_horongs_light_cold_ones() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    // A Horong drifting by the cold brazier sets it alight.
    let h = s.horde.as_ref().unwrap().catalog().kind_of("horong").unwrap();
    let skull = s.horde_mut().unwrap().spawn(h, 11.0, 10.5).unwrap();
    let mut out = Vec::new();
    for t in 0..30 {
        at(&mut s, 2.0 + t as f32 * 0.1, &mut out);
    }
    assert_eq!(s.env.payon.lit(), 1, "the Horong lit it");
    s.horde_mut().unwrap().despawn(skull);
    // Two Familiars hanging over it take ten seconds a second off it.
    let f = s.horde.as_ref().unwrap().catalog().kind_of("familiar").unwrap();
    s.horde_mut().unwrap().spawn(f, 10.5, 11.0).unwrap();
    s.horde_mut().unwrap().spawn(f, 11.0, 10.0).unwrap();
    let mut out = Vec::new();
    for t in 0..80 {
        at(&mut s, 5.0 + t as f32 * 0.1, &mut out);
    }
    // 60 s of fire, ~8 s of bats: it is out long before its time.
    for t in 0..300 {
        at(&mut s, 13.0 + t as f32 * 0.1, &mut out);
        if s.env.payon.lit() == 0 {
            break;
        }
    }
    assert_eq!(s.env.payon.lit(), 0, "snuffed");
    assert!((s.env_light_scale() - 1.0).abs() < 1e-3, "its light went with it");
}
