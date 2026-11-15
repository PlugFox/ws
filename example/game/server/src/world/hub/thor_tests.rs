//! Thor Volcano on a test field: obsidian (cold on lava), Ifrit's chamber,
//! hidden Byorgues (`thor.rs`), and the mission's data.

use super::super::area::Area;
use super::super::env::Env;
use super::super::test_support::map;
use super::super::{HubEvent, HubSimConfig, HubWorld, PlayerProfile};
use super::*;
use crate::world::combat;
use crate::world::combat::DamageType;
use crate::world::environment::Hazard;
use crate::world::horde::catalog::{test_def, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::missions;
use crate::world::protocol::{zone_flags, Look, ServerMsg};
use std::sync::Arc;
use uuid::Uuid;

/// A vent of lava (always on) at (10.5, 10.5).
const VENT: Hazard = Hazard { look: "fire", damage: [1, 6], damage_type: "fire", condition: Some("burning"), ..Hazard::circle("vent_0", 10.5, 10.5, 4.0) };

/// A 240 × 200 open field (Ifrit's chamber is inside it) with walls round;
/// player 1 joins at (10.5, 10.5) on the vent. The director is off.
fn stage() -> HubInstance {
    let wall = "#".repeat(242);
    let row = format!("#{}#", ".".repeat(240));
    let mut rows = vec![wall.clone()];
    rows.extend(std::iter::repeat_n(row, 200));
    rows.push(wall);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let world = Arc::new(HubWorld::new(vec![map("field", LocationKind::Field, camera, &rows, vec![])], "field").unwrap());
    let catalog = Arc::new(MonsterCatalog::new(vec![test_def("byorgue", 0.5, 1, 0.0, false), test_def("ifrit", 0.5, 50, 0.0, true)]).unwrap());
    let cfg = HordeConfig { light_radius: 30.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, MISSION, world, HubSimConfig { residents: 0, ..Default::default() }).with_horde(horde);
    stage.env = Env::new(missions::find(MISSION), &[VENT]);
    stage.dice = combat::Rng::new(3);
    let mut out = Vec::new();
    let profile = PlayerProfile {
        user_id: 1,
        character_id: 101,
        name: "p1".into(),
        archetype: "swordsman".into(),
        look: Look::default(),
        level: 85,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
        homunculus: None,
    };
    stage.join(profile, Uuid::nil(), None, 0, &mut out);
    put(&mut stage, 10.5, 10.5);
    let f = stage.players[0].fighter.as_mut().unwrap();
    (f.max_hp, f.hp) = (100_000, 100_000);
    f.recovery = u32::MAX;
    stage
}

fn put(stage: &mut HubInstance, x: f32, y: f32) {
    let i = stage.find(1).unwrap();
    (stage.players[i].x, stage.players[i].y) = (x, y);
}

fn run(stage: &mut HubInstance, from: f32, secs: f32) -> Outgoing {
    let mut out = Vec::new();
    let mut t = from;
    while t < from + secs {
        stage.horde_mut().unwrap().set_director_elapsed(t);
        stage.tick(&mut out);
        t += 0.05;
    }
    out
}

fn bites(out: &Outgoing) -> bool {
    out.iter().any(|(_, e)| matches!(e, HubEvent::Control(ServerMsg::Combat { hits, .. }) if hits.iter().any(|h| h.target == 101 && h.amount > 0)))
}

#[test]
fn cold_on_lava_leaves_obsidian_and_fire_does_nothing() {
    let mut stage = stage();
    assert!(bites(&run(&mut stage, 0.0, 3.0)), "the vent burns");
    assert!(stage.env.ground_is(10.5, 10.5, LAVA));
    // A fire spell on lava: no meeting at all.
    assert!(stage.env.ground_meet(10.5, 10.5, DamageType::Fire).is_none());
    let mut msgs = Vec::new();
    stage.ground_meets(&[(DamageType::Fire, Area::Circle { x: 10.5, y: 10.5, r: 2.0 })], &mut msgs);
    assert!(msgs.is_empty());
    // A cold spell: obsidian, safe ground shown, the vent spares whoever stands on it.
    stage.ground_meets(&[(DamageType::Cold, Area::Circle { x: 10.5, y: 10.5, r: 3.0 })], &mut msgs);
    let zone = msgs.iter().find_map(|m| match m {
        ServerMsg::BossZones { caster: 0, ability, zones } if ability == "thor_obsidian" => Some(zones[0]),
        _ => None,
    });
    let zone = zone.expect("the obsidian is shown");
    assert_eq!((zone.flags, zone.area.span_ms, zone.area.a), (zone_flags::SAFE, 8_000, 3.0));
    assert!(!bites(&run(&mut stage, 3.0, 7.0)), "no burn on obsidian");
    // The obsidian shelters from a nova the way a lit light does.
    assert_eq!(stage.env.calm_spots(5.0).count(), 1);
    assert!(bites(&run(&mut stage, 11.5, 4.0)), "it melts again");
    assert_eq!(stage.env.calm_spots(12.0).count(), 0);
    // Off the lava a cold spell meets nothing.
    assert!(stage.env.ground_meet(40.5, 40.5, DamageType::Cold).is_none());
}

#[test]
fn ifrit_rises_in_his_chamber() {
    let mut stage = stage();
    let horde = stage.horde_mut().unwrap();
    let kind = horde.catalog().kind_of("ifrit").unwrap();
    let slot = horde.spawn(kind, 30.5, 30.5).unwrap();
    let out = run(&mut stage, 600.0, 0.5);
    let st = stage.horde_mut().unwrap().store();
    let (x, y) = (st.x[slot as usize], st.y[slot as usize]);
    assert!((x - IFRIT_CHAMBER.0).abs() < 1.0 && (y - IFRIT_CHAMBER.1).abs() < 1.0, "in his chamber: ({x}, {y})");
    assert!(out.iter().any(|(_, e)| matches!(e, HubEvent::Control(ServerMsg::EnvShake { .. }))), "the ground shakes");
    // Once only: moved away, he stays where he is.
    stage.horde_mut().unwrap().place(slot, 100.5, 100.5);
    run(&mut stage, 601.0, 0.5);
    assert!((stage.horde_mut().unwrap().store().x[slot as usize] - 100.5).abs() < 1.0);
}

#[test]
fn byorgues_travel_hidden_and_come_out_by_a_player() {
    let mut stage = stage();
    put(&mut stage, 60.5, 60.5);
    let horde = stage.horde_mut().unwrap();
    let kind = horde.catalog().kind_of("byorgue").unwrap();
    let slot = horde.spawn(kind, 90.5, 60.5).unwrap();
    horde.store_mut().hp[slot as usize] = 1_000_000;
    run(&mut stage, 1.0, 0.3);
    assert!(stage.horde_mut().unwrap().store().burrowed(slot), "it arrives hidden");
    // Next to a player: out.
    stage.horde_mut().unwrap().place(slot, 62.5, 60.5);
    run(&mut stage, 2.0, 0.3);
    assert!(!stage.horde_mut().unwrap().store().burrowed(slot), "out to strike");
    // The player gets away: hidden again after its time up.
    put(&mut stage, 160.5, 160.5);
    run(&mut stage, 8.0, 0.3);
    assert!(stage.horde_mut().unwrap().store().burrowed(slot), "slips away");
}

#[test]
fn the_volcano_surges_in_turn_and_bombs_fall_while_it_does() {
    let m = missions::find(MISSION).unwrap();
    assert!((m.env.light_scale - 0.9).abs() < 1e-6);
    let ev: Vec<_> = m.env.events.iter().map(|e| (e.id, e.first_secs, e.every_secs)).collect();
    assert_eq!(ev, vec![("surge_west", 150.0, 300.0), ("surge_east", 300.0, 300.0), ("tremor", 600.0, 0.0)]);
    // The surges alternate: never both at once on schedule.
    let (w, e) = (m.env.event("surge_west").unwrap(), m.env.event("surge_east").unwrap());
    for s in (0..900).step_by(5) {
        assert!(!(w.running_at(s as f32).is_some() && e.running_at(s as f32).is_some()), "both surges at {s}");
    }
    // The tremor warns at 9:45 of Ifrit at 10:00.
    let t = m.env.event("tremor").unwrap();
    assert_eq!(t.first_secs - t.warn_secs, 585.0);
    assert!(m.env.strikes.iter().all(|s| s.problem(m.env.events).is_none()));
    let during: Vec<_> = m.env.strikes.iter().map(|s| s.during).collect();
    assert_eq!(during, vec!["surge_west", "surge_east"]);
    assert_eq!(m.tips.len(), 6);
    assert_eq!(m.levels, missions::LevelBand::new(81, 83, 91));
}

#[test]
fn bombs_fall_on_the_party_during_a_surge() {
    let mut stage = stage();
    put(&mut stage, 60.5, 60.5);
    let out = run(&mut stage, 149.0, 5.0);
    let shadows = out
        .iter()
        .filter(|(_, e)| matches!(e, HubEvent::Control(ServerMsg::BossZones { caster: 0, ability, .. }) if ability == "thor_bombs_w"))
        .count();
    assert!(shadows >= 1, "a volley of bombs in the first seconds of the west surge");
}

#[test]
fn the_baked_volcano_loads_with_its_vents_and_surges() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    if !dist.join("server/locations/thor_v03").exists() {
        return; // no baked pack here
    }
    let world = HubWorld::load(&dist, &["thor_v03".into()], "thor_v03").expect("thor_v03 loads");
    let hazards = &world.map(world.entry()).hazards;
    assert_eq!(hazards.iter().filter(|h| h.id.starts_with("vent_")).count(), 18, "the vents");
    let west = hazards.iter().filter(|h| h.during == Some("surge_west")).count();
    let east = hazards.iter().filter(|h| h.during == Some("surge_east")).count();
    assert!(west >= 8 && east >= 8, "every chamber floods: {west} west, {east} east");
    // West surge circles all lie west of the divide, east ones east.
    for h in hazards.iter().filter(|h| h.during.is_some()) {
        let (x, _) = h.shape.centre();
        assert_eq!(x < 150.0, h.during == Some("surge_west"), "{}", h.id);
    }
    let walk = &world.map(world.entry()).walk;
    assert!(walk.is_walkable(IFRIT_CHAMBER.0, IFRIT_CHAMBER.1), "Ifrit's chamber");
    for h in hazards.iter() {
        let (x, y) = h.shape.centre();
        assert!(walk.is_walkable(x, y), "{} on open ground", h.id);
    }
}
