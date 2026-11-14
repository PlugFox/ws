//! The mission environment on a small field: hazards, events, light and
//! objects (`env.rs`).

use super::super::test_support::map;
use super::super::{HubSimConfig, HubWorld, PlayerProfile};
use super::*;
use crate::world::combat::{self, Condition};
use crate::world::environment::{Environment, ObjectDef};
use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::missions::MissionInfo;
use crate::world::protocol::Look;
use std::sync::Arc;
use uuid::Uuid;

const TAR: Hazard = Hazard {
    look: "swamp",
    damage: [1, 4],
    condition: Some("slowed"),
    every_secs: 1.0,
    ..Hazard::circle("tar", 10.5, 10.5, 3.0)
};
const WHIRL: Hazard = Hazard { pull: 4.0, ..Hazard::circle("whirl", 30.5, 10.5, 6.0) };
const SAND: Hazard = Hazard {
    damage: [2, 6],
    damage_type: "earth",
    during: Some("storm"),
    ..Hazard::circle("sand", 20.5, 10.5, 4.0)
};
const STORM: MapEvent = MapEvent {
    id: "storm",
    name: "Sandstorm",
    warning: "A sandstorm is rising!",
    first_secs: 100.0,
    every_secs: 200.0,
    warn_secs: 10.0,
    duration_secs: 30.0,
    light_scale: 0.5,
    shake: 120,
};

static MISSION: MissionInfo = MissionInfo {
    id: "env_test",
    name: "Env",
    map: "field",
    fog: None,
    env: Environment {
        light_scale: 1.0,
        hazards: &[TAR, WHIRL, SAND],
        events: &[STORM],
        objects: &[
            ObjectDef { monster: "brazier", spots: &[(5.5, 5.5)], on_felled: ObjectEffect::Light { scale: 1.5, secs: 20.0 }, hatch: None },
            ObjectDef { monster: "valve", spots: &[(6.5, 15.5), (0.2, 0.2)], on_felled: ObjectEffect::EndHazard("tar"), hatch: None },
        ],
    },
    ..MissionInfo::TEMPLATE
};

fn profile(user_id: u32) -> PlayerProfile {
    PlayerProfile {
        user_id,
        character_id: 100 + user_id,
        name: format!("p{user_id}"),
        archetype: "swordsman".into(),
        look: Look::default(),
        level: 10,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
    }
}

/// A 40 × 21 open field with walls around; player 1 joins. The director
/// is off: the tests set the run clock.
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
    let catalog = Arc::new(
        MonsterCatalog::new(vec![test_def("blob", 2.0, 1, 0.0, false), test_plant("brazier", (3, 3)), test_plant("valve", (1, 1))]).unwrap(),
    );
    let cfg = HordeConfig { light_radius: 12.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, MISSION.id, world, HubSimConfig { residents: 0, ..Default::default() }).with_horde(horde);
    stage.env = Env::new(Some(&MISSION), &[]);
    stage.dice = combat::Rng::new(5);
    let mut out = Vec::new();
    stage.join(profile(1), Uuid::nil(), None, 0, &mut out);
    put(&mut stage, 1, 2.5, 19.5);
    stage
}

fn at(stage: &mut HubInstance, secs: f32, out: &mut Outgoing) {
    stage.horde_mut().unwrap().set_director_elapsed(secs);
    stage.tick(out);
}

fn put(stage: &mut HubInstance, user: u32, x: f32, y: f32) {
    let i = stage.find(user).unwrap();
    (stage.players[i].x, stage.players[i].y) = (x, y);
}

fn pos(stage: &HubInstance, user: u32) -> (f32, f32) {
    let i = stage.find(user).unwrap();
    (stage.players[i].x, stage.players[i].y)
}

fn me(stage: &mut HubInstance) -> &mut super::super::Fighter {
    let i = stage.find(1).unwrap();
    stage.players[i].fighter.as_mut().unwrap()
}

fn msgs(out: &Outgoing, user: u32) -> Vec<ServerMsg> {
    out.iter()
        .filter(|(u, _)| *u == user)
        .filter_map(|(_, e)| match e {
            HubEvent::Control(m) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

fn zones(out: &Outgoing) -> Vec<(String, u32, u32)> {
    msgs(out, 1)
        .into_iter()
        .filter_map(|m| match m {
            ServerMsg::BossZones { caster: 0, ability, zones } => Some((ability, zones[0].delay_ms, zones[0].area.span_ms)),
            _ => None,
        })
        .collect()
}

fn hits_on_me(out: &Outgoing) -> Vec<u32> {
    msgs(out, 1)
        .into_iter()
        .filter_map(|m| match m {
            ServerMsg::Combat { hits, .. } => Some(hits),
            _ => None,
        })
        .flatten()
        .filter(|h| h.target == 101)
        .map(|h| h.amount)
        .collect()
}

#[test]
fn always_on_hazards_show_and_bite_whoever_stands_inside() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    let shown = zones(&out);
    let names: Vec<&str> = shown.iter().map(|z| z.0.as_str()).collect();
    assert_eq!(names, ["hazard:tar", "hazard:whirl"], "the event's sand waits for the event");
    assert_eq!(shown[0].1, 0);
    assert!(shown[0].2 >= 898_000, "lasting to the time limit: {}", shown[0].2);
    // Outside: nothing.
    assert!(hits_on_me(&out).is_empty());
    // Inside the tar: slowed and hurt once a second.
    put(&mut s, 1, 10.5, 10.5);
    me(&mut s).hp = 1000;
    let mut out = Vec::new();
    for t in 0..40 {
        at(&mut s, 2.0 + t as f32 * 0.05, &mut out);
    }
    let hurt = hits_on_me(&out);
    assert_eq!(hurt.len(), 2, "two pulses in two seconds: {hurt:?}");
    assert!(hurt.iter().all(|&a| (1..=4).contains(&a)), "1d4 + level/4 (band min 1): {hurt:?}");
    assert!(me(&mut s).status.has(Condition::Slowed));
    assert_eq!(pos(&s, 1), (10.5, 10.5), "the tar does not pull");
}

#[test]
fn a_pulling_hazard_drags_toward_its_centre_and_stops_at_walls() {
    let mut s = stage();
    let mut out = Vec::new();
    put(&mut s, 1, 26.5, 10.5);
    for t in 0..20 {
        at(&mut s, 1.0 + t as f32 * 0.05, &mut out);
    }
    let (x, y) = pos(&s, 1);
    assert!(x > 27.5 && x < 30.6 && (y - 10.5).abs() < 0.01, "pulled east by about 4 cells/s: {x}");
}

#[test]
fn events_warn_begin_scale_the_light_and_end() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    put(&mut s, 1, 20.5, 10.5);
    me(&mut s).hp = 1000;
    let mut out = Vec::new();
    at(&mut s, 89.0, &mut out);
    assert!(msgs(&out, 1).iter().all(|m| !matches!(m, ServerMsg::EnvEvent { .. })), "too early to warn");
    let mut out = Vec::new();
    at(&mut s, 90.5, &mut out);
    let warned: Vec<ServerMsg> = msgs(&out, 1).into_iter().filter(|m| matches!(m, ServerMsg::EnvEvent { .. })).collect();
    assert_eq!(
        warned,
        [ServerMsg::EnvEvent {
            phase: env_phase::WARNING,
            id: "storm".into(),
            name: "Sandstorm".into(),
            text: "A sandstorm is rising!".into(),
            start_ms: 100_000,
            duration_ms: 30_000
        }]
    );
    let z = zones(&out);
    assert_eq!((z[0].0.as_str(), z[0].1, z[0].2), ("hazard:sand", 9500, 30_000), "shown with the warning, filling until it begins");
    // Not yet biting while warned.
    let mut out = Vec::new();
    at(&mut s, 95.0, &mut out);
    assert!(hits_on_me(&out).is_empty());
    // Begins: the light halves and the sand bites.
    let mut out = Vec::new();
    at(&mut s, 100.0, &mut out);
    let m = msgs(&out, 1);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::EnvEvent { phase: env_phase::BEGINS, .. })));
    assert!(m.contains(&ServerMsg::EnvLight { radius: 6.0, fade_ms: LIGHT_FADE_MS }), "{m:?}");
    assert_eq!(s.horde.as_ref().unwrap().light_radius(), 6.0);
    assert_eq!(hits_on_me(&out).len(), 1);
    // A late joiner learns the running storm and sees the sand.
    let mut late = Vec::new();
    s.join(profile(2), Uuid::nil(), None, 0, &mut late);
    let theirs = msgs(&late, 2);
    assert!(theirs.iter().any(|m| matches!(m, ServerMsg::EnvEvent { phase: env_phase::BEGINS, start_ms: 100_000, .. })));
    assert!(theirs.iter().any(|m| matches!(m, ServerMsg::BossZones { caster: 0, ability, .. } if ability == "hazard:sand")));
    assert!(theirs.iter().any(|m| matches!(m, ServerMsg::StageJoined { light_radius, .. } if *light_radius == 6.0)));
    // Ends: zone gone, light back.
    let mut out = Vec::new();
    at(&mut s, 130.5, &mut out);
    let m = msgs(&out, 1);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::EnvEvent { phase: env_phase::ENDS, .. })));
    // Its ground was sent lasting just the event: no ZONE_END per box.
    assert!(!m.iter().any(|m| matches!(m, ServerMsg::ZoneEnd { .. })));
    assert!(m.contains(&ServerMsg::EnvLight { radius: 12.0, fade_ms: LIGHT_FADE_MS }));
    let mut out = Vec::new();
    at(&mut s, 135.0, &mut out);
    assert!(hits_on_me(&out).is_empty(), "the sand is still once the storm is over");
    // The next one warns again.
    let mut out = Vec::new();
    at(&mut s, 291.0, &mut out);
    assert!(msgs(&out, 1).iter().any(|m| matches!(m, ServerMsg::EnvEvent { phase: env_phase::WARNING, start_ms: 300_000, .. })));
}

#[test]
fn felled_objects_light_the_field_and_turn_hazards_off() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    assert_eq!(s.env.objects.len(), 2, "the off-map valve spot is skipped");
    assert_eq!(s.horde.as_ref().unwrap().plants(), 2);
    let brazier = s.env.objects.iter().find(|o| matches!(o.effect, ObjectEffect::Light { .. })).unwrap().slot;
    let valve = s.env.objects.iter().find(|o| matches!(o.effect, ObjectEffect::EndHazard(_))).unwrap().slot;
    s.horde.as_mut().unwrap().despawn(brazier);
    let mut out = Vec::new();
    for t in 0..OBJECT_SCAN {
        at(&mut s, 2.0 + t as f32 * 0.05, &mut out);
    }
    assert!(msgs(&out, 1).contains(&ServerMsg::EnvLight { radius: 18.0, fade_ms: LIGHT_FADE_MS }), "lit: ×1.5");
    assert!((s.env_light_scale() - 1.5).abs() < 1e-3);
    // It burns out after 20 s.
    let mut out = Vec::new();
    at(&mut s, 23.0, &mut out);
    assert!(msgs(&out, 1).contains(&ServerMsg::EnvLight { radius: 12.0, fade_ms: LIGHT_FADE_MS }));
    // The valve shuts the tar off for good.
    s.horde.as_mut().unwrap().despawn(valve);
    let mut out = Vec::new();
    for t in 0..OBJECT_SCAN {
        at(&mut s, 24.0 + t as f32 * 0.05, &mut out);
    }
    assert!(msgs(&out, 1).iter().any(|m| matches!(m, ServerMsg::ZoneEnd { .. })));
    put(&mut s, 1, 10.5, 10.5);
    let mut out = Vec::new();
    for t in 0..40 {
        at(&mut s, 30.0 + t as f32 * 0.05, &mut out);
    }
    assert!(hits_on_me(&out).is_empty() && !me(&mut s).status.has(Condition::Slowed));
}

#[test]
fn a_mission_without_an_environment_does_nothing() {
    let mut s = stage();
    s.env = Env::new(Some(&crate::world::missions::MISSIONS[0]), &[]);
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    assert!(zones(&out).is_empty());
    assert!(msgs(&out, 1).iter().all(|m| !matches!(m, ServerMsg::EnvLight { .. } | ServerMsg::EnvEvent { .. })));
    assert_eq!(s.horde.as_ref().unwrap().light_radius(), 12.0);
}

static HATCHERY: MissionInfo = MissionInfo {
    id: "env_hatch",
    name: "Hatch",
    map: "field",
    fog: None,
    env: Environment {
        light_scale: 1.0,
        hazards: &[],
        events: &[],
        objects: &[ObjectDef {
            monster: "valve",
            spots: &[(20.5, 10.5), (30.5, 10.5)],
            on_felled: ObjectEffect::None,
            hatch: Some(crate::world::environment::Hatch { secs: 10.0, monster: "blob", count: 3 }),
        }],
    },
    ..MissionInfo::TEMPLATE
};

#[test]
fn eggs_left_standing_hatch_and_broken_ones_do_not() {
    let mut s = stage();
    s.env = Env::new(Some(&HATCHERY), &[]);
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    let first = s.env.objects[0].slot;
    s.horde.as_mut().unwrap().despawn(first);
    for t in 0..OBJECT_SCAN {
        at(&mut s, 2.0 + t as f32 * 0.05, &mut out);
    }
    assert_eq!(s.env.objects.len(), 1, "the broken egg is gone, nothing hatched");
    let alive = |s: &HubInstance| s.horde.as_ref().unwrap().store().alive().len();
    assert_eq!(alive(&s), 1);
    for t in 0..OBJECT_SCAN {
        at(&mut s, 11.0 + t as f32 * 0.05, &mut out);
    }
    assert!(s.env.objects.is_empty());
    assert_eq!(alive(&s), 3, "three blobs out of the second egg");
}

#[test]
fn an_event_can_shake_the_camera_as_it_begins() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    at(&mut s, 100.0, &mut out);
    assert!(msgs(&out, 1).contains(&ServerMsg::EnvShake { intensity: 120, duration_ms: 1000 }));
}
