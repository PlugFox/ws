//! Sograt Desert on a small field: sand-blast and shelter, buried hodes,
//! the boss's rise (`sograt.rs`).

use super::super::test_support::map;
use super::super::{HubEvent, HubSimConfig, HubWorld, PlayerProfile};
use super::*;
use crate::world::combat;
use crate::world::combat::{Archetype, Condition};
use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{Look, ServerMsg};
use std::sync::Arc;
use uuid::Uuid;

fn profile(user_id: u32) -> PlayerProfile {
    PlayerProfile {
        user_id,
        character_id: 100 + user_id,
        name: format!("p{user_id}"),
        archetype: "swordsman".into(),
        look: Look::default(),
        level: 30,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
    }
}

/// A 40 × 21 open field with walls around and a pillar of rock at
/// x = 20 (y 4..=8); player 1 joins. The director is off: the tests set the
/// run clock.
fn stage() -> HubInstance {
    let wall = "#".repeat(42);
    let row = format!("#{}#", ".".repeat(40));
    let pillar = format!("#{}#{}#", ".".repeat(19), ".".repeat(20));
    let mut rows = vec![wall.clone()];
    for y in (0..21).rev() {
        rows.push(if (4..=8).contains(&y) { pillar.clone() } else { row.clone() });
    }
    rows.push(wall);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let mut colossus = test_def("sand_colossus", 0.5, 20, 0.0, false);
    colossus.archetype = Archetype::Elite;
    let mut alpha = test_def("sirocco", 0.5, 18, 0.0, false);
    alpha.archetype = Archetype::Elite;
    let catalog = Arc::new(
        MonsterCatalog::new(vec![
            test_def("hode", 0.5, 1, 0.0, false),
            test_def("phreeoni", 0.5, 50, 0.0, true),
            test_plant("muka", (6, 6)),
            colossus,
            alpha,
        ])
        .unwrap(),
    );
    let cfg = HordeConfig { light_radius: 30.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, MISSION, world, HubSimConfig { residents: 0, ..Default::default() }).with_horde(horde);
    assert!(stage.desert.is_some(), "a Sograt stage runs the desert");
    stage.dice = combat::Rng::new(5);
    let mut out = Vec::new();
    stage.join(profile(1), Uuid::nil(), None, 0, &mut out);
    put(&mut stage, 2.5, 19.5);
    stage
}

fn at(stage: &mut HubInstance, secs: f32, out: &mut Outgoing) {
    stage.horde_mut().unwrap().set_director_elapsed(secs);
    stage.tick(out);
}

fn put(stage: &mut HubInstance, x: f32, y: f32) {
    let i = stage.find(1).unwrap();
    (stage.players[i].x, stage.players[i].y) = (x, y);
}

fn hits_on_me(out: &Outgoing) -> usize {
    out.iter()
        .filter_map(|(u, e)| match e {
            HubEvent::Control(ServerMsg::Combat { hits, .. }) if *u == 1 => Some(hits.iter().filter(|h| h.target == 101).count()),
            _ => None,
        })
        .sum()
}

#[test]
fn the_storm_blasts_the_open_and_spares_the_lee() {
    let m = missions::find(MISSION).unwrap();
    let storm = m.env.event(SANDSTORM).unwrap();
    let start = storm.start(0).unwrap();
    let wind = WINDS[wind_dir(start)];
    // The lee of the pillar (x = 20, y 4..=8) for this storm's wind, and
    // a spot in the open.
    let lee = (20.5 + wind.0 * 2.0, 6.5 + wind.1 * 2.0);
    let open = (8.5, 16.5);
    for (spot, blasted) in [(open, true), (lee, false)] {
        let mut s = stage();
        put(&mut s, spot.0, spot.1);
        let i = s.find(1).unwrap();
        s.players[i].fighter.as_mut().unwrap().hp = 5000;
        // Before the storm: nothing; through it, a pulse every 3 s.
        let mut out = Vec::new();
        for k in 0..40 {
            put(&mut s, spot.0, spot.1);
            at(&mut s, start - 20.0 + k as f32 * 0.5, &mut out);
        }
        assert_eq!(hits_on_me(&out), 0, "calm air");
        let mut out = Vec::new();
        for k in 0..50 {
            put(&mut s, spot.0, spot.1);
            at(&mut s, start + k as f32 * 0.5, &mut out);
        }
        let n = hits_on_me(&out);
        if blasted {
            assert!(n >= 2, "the open sand gets blasted ({n} hits in 25 s)");
        } else {
            assert_eq!(n, 0, "the lee shelters ({spot:?}, wind {wind:?})");
        }
    }
}

#[test]
fn hodes_travel_buried_surface_by_a_player_and_dive_again() {
    let mut s = stage();
    let kind = s.horde_mut().unwrap().catalog().kind_of("hode").unwrap();
    let slot = s.horde_mut().unwrap().spawn(kind, 30.5, 15.5).unwrap();
    // Tough enough to outlive the player's swings while it is up.
    s.horde_mut().unwrap().store_mut().hp[slot as usize] = 1_000_000;
    let mut out = Vec::new();
    for k in 0..6 {
        at(&mut s, 1.0 + k as f32 * 0.1, &mut out);
    }
    assert!(s.horde_mut().unwrap().store().burrowed(slot), "it arrives under the sand");
    // A player walks up to it: it surfaces.
    let (hx, hy) = {
        let st = s.horde_mut().unwrap().store();
        (st.x[slot as usize], st.y[slot as usize])
    };
    for k in 0..6 {
        put(&mut s, hx + 1.0, hy);
        at(&mut s, 2.0 + k as f32 * 0.1, &mut out);
    }
    assert!(!s.horde_mut().unwrap().store().burrowed(slot), "up to strike");
    // The player runs off; after its time up it dives again.
    for k in 0..12 {
        put(&mut s, 2.5, 2.5);
        at(&mut s, 3.0 + k as f32 * 0.5, &mut out);
    }
    assert!(s.horde_mut().unwrap().store().burrowed(slot), "back under the sand");
}

#[test]
fn the_boss_rises_once_with_a_quake() {
    let mut s = stage();
    let kind = s.horde_mut().unwrap().catalog().kind_of("phreeoni").unwrap();
    s.horde_mut().unwrap().spawn(kind, 30.5, 15.5).unwrap();
    let mut out = Vec::new();
    for k in 0..6 {
        at(&mut s, 600.0 + k as f32 * 0.1, &mut out);
    }
    let quakes = |out: &Outgoing| out.iter().filter(|(u, e)| *u == 1 && matches!(e, HubEvent::Control(ServerMsg::EnvShake { intensity: 220, .. }))).count();
    assert_eq!(quakes(&out), 1, "one quake as it rises");
    let mut out = Vec::new();
    for k in 0..6 {
        at(&mut s, 601.0 + k as f32 * 0.1, &mut out);
    }
    assert_eq!(quakes(&out), 0, "only once");
}

fn me(stage: &mut HubInstance) -> &mut super::super::Fighter {
    let i = stage.find(1).unwrap();
    stage.players[i].fighter.as_mut().unwrap()
}

#[test]
fn a_blow_on_a_muka_pricks_whoever_stands_by_it() {
    let mut s = stage();
    let kind = s.horde_mut().unwrap().catalog().kind_of("muka").unwrap();
    let slot = s.horde_mut().unwrap().place_object(kind, 20.5, 15.5).unwrap();
    me(&mut s).hp = 5000;
    let mut out = Vec::new();
    for k in 0..4 {
        put(&mut s, 10.5, 15.5);
        at(&mut s, 1.0 + k as f32 * 0.1, &mut out);
    }
    // A blow from afar (a bolt): nobody in reach, no thorns.
    s.horde_mut().unwrap().store_mut().hp[slot as usize] -= 1;
    let mut out = Vec::new();
    at(&mut s, 1.5, &mut out);
    assert_eq!(hits_on_me(&out), 0, "out of reach");
    // A blow from beside it: pricked.
    put(&mut s, 21.5, 15.5);
    s.horde_mut().unwrap().store_mut().hp[slot as usize] -= 1;
    let mut out = Vec::new();
    put(&mut s, 21.5, 15.5);
    at(&mut s, 1.6, &mut out);
    assert_eq!(hits_on_me(&out), 1, "thorns");
}

#[test]
fn a_fallen_colossus_leaves_quicksand_that_pulls_and_mires() {
    let mut s = stage();
    let kind = s.horde_mut().unwrap().catalog().kind_of("sand_colossus").unwrap();
    let slot = s.horde_mut().unwrap().spawn(kind, 30.5, 15.5).unwrap();
    let mut out = Vec::new();
    for k in 0..4 {
        put(&mut s, 2.5, 2.5);
        at(&mut s, 1.0 + k as f32 * 0.1, &mut out);
    }
    // It falls.
    s.horde_mut().unwrap().store_mut().hp[slot as usize] = 0;
    s.horde_mut().unwrap().despawn(slot);
    let mut out = Vec::new();
    for k in 0..4 {
        put(&mut s, 2.5, 2.5);
        at(&mut s, 2.0 + k as f32 * 0.1, &mut out);
    }
    let shown = out.iter().any(|(u, e)| *u == 1 && matches!(e, HubEvent::Control(ServerMsg::BossZones { caster: 0, ability, .. }) if ability == "boss_sand_vortex"));
    assert!(shown, "the quicksand shows");
    // Standing at its rim: pulled in and mired.
    put(&mut s, 25.5, 15.5);
    let mut out = Vec::new();
    for k in 0..10 {
        at(&mut s, 3.0 + k as f32 * 0.1, &mut out);
    }
    let i = s.find(1).unwrap();
    assert!(s.players[i].x > 25.6, "pulled toward the middle: {}", s.players[i].x);
    assert!(me(&mut s).status.has(Condition::Mired), "mired");
    // It dries up.
    let mut out = Vec::new();
    put(&mut s, 25.5, 15.5);
    for k in 0..10 {
        put(&mut s, 25.5, 15.5);
        at(&mut s, 20.0 + k as f32 * 0.1, &mut out);
    }
    let i = s.find(1).unwrap();
    assert!((s.players[i].x - 25.5).abs() < 1e-3, "no pull once it is gone");
}

#[test]
fn phreeoni_under_forty_percent_roars_a_storm_up() {
    let mut s = stage();
    let kind = s.horde_mut().unwrap().catalog().kind_of("phreeoni").unwrap();
    let slot = s.horde_mut().unwrap().spawn(kind, 30.5, 15.5).unwrap();
    let mut out = Vec::new();
    for k in 0..4 {
        at(&mut s, 400.0 + k as f32 * 0.1, &mut out);
    }
    assert_eq!(s.env_event_start(SANDSTORM), None, "calm at 6:40");
    let max = s.horde_mut().unwrap().block(slot).hp;
    s.horde_mut().unwrap().store_mut().hp[slot as usize] = max * 35 / 100;
    let mut out = Vec::new();
    for k in 0..4 {
        at(&mut s, 401.0 + k as f32 * 0.1, &mut out);
    }
    let start = s.env_event_start(SANDSTORM).expect("a storm now");
    assert!((401.0..402.0).contains(&start), "begun at the roar: {start}");
    let begins = out
        .iter()
        .filter(|(u, e)| *u == 1 && matches!(e, HubEvent::Control(ServerMsg::EnvEvent { phase, id, .. }) if *phase == crate::world::protocol::env_phase::BEGINS && id == SANDSTORM))
        .count();
    assert_eq!(begins, 1, "announced once");
    assert!(s.env_light_scale() < 0.7, "the light closes in");
    // It ends after its 30 s.
    let mut out = Vec::new();
    at(&mut s, start + 31.0, &mut out);
    assert_eq!(s.env_event_start(SANDSTORM), None);
}

#[test]
fn sirocco_without_its_pack_turns_lone_wolf() {
    let mut s = stage();
    let kind = s.horde_mut().unwrap().catalog().kind_of("sirocco").unwrap();
    let slot = s.horde_mut().unwrap().spawn(kind, 12.5, 15.5).unwrap();
    s.horde_mut().unwrap().store_mut().hp[slot as usize] = 1_000_000;
    me(&mut s).hp = 1_000_000;
    let lone = |out: &Outgoing| {
        out.iter().filter(|(u, e)| *u == 1 && matches!(e, HubEvent::Control(ServerMsg::BossShout { text, .. }) if text == LONE_WOLF)).count()
    };
    // The fight starts; before 20 s it is not lone yet.
    let mut out = Vec::new();
    for k in 0..40 {
        put(&mut s, 8.5, 15.5);
        at(&mut s, 1.0 + k as f32 * 0.25, &mut out);
    }
    assert_eq!(lone(&out), 0, "too early");
    let mut out = Vec::new();
    for k in 0..40 {
        put(&mut s, 8.5, 15.5);
        at(&mut s, 21.0 + k as f32 * 0.25, &mut out);
    }
    assert_eq!(lone(&out), 1, "alone after 20 s, said once");
}

#[test]
fn spells_in_flight_drift_downwind_in_a_storm() {
    use super::super::cast::{Live, Load};
    use super::super::area::Area;
    let mut s = stage();
    let map = s.horde_mut().unwrap().map();
    let mut out = Vec::new();
    at(&mut s, 50.0, &mut out);
    assert!(s.env_force_event(map, SANDSTORM, 50.0, &mut out));
    let start = s.env_event_start(SANDSTORM).unwrap();
    let wind = WINDS[wind_dir(start)];
    let now = s.tick;
    let live = |at: u32, follow: Follow| Live {
        id: 0,
        map,
        skill: 0,
        caster: 1,
        next: at,
        until: at,
        pulse: 0,
        area: Area::Circle { x: 20.5, y: 10.5, r: 1.0 },
        follow,
        point: false,
        trap: None,
        limit: 0,
        chain: (0, 0.0),
        ally: None,
        load: Load::Cure,
    };
    s.lives.push(live(now + 1000, Follow::None));
    s.lives.push(live(now + 1000, Follow::Monster(0, 0)));
    let hz = s.config.tick_hz as f32;
    for k in 0..(hz as u32) {
        at(&mut s, 50.0 + k as f32 / hz, &mut out);
    }
    let (x, y) = s.lives[0].area.centre();
    let moved = ((x - 20.5) * wind.0 + (y - 10.5) * wind.1) / DRIFT;
    assert!((0.8..1.2).contains(&moved), "a second of drift downwind: ({x}, {y}), wind {wind:?}");
    assert_eq!(s.lives[1].area.centre(), (20.5, 10.5), "a homing hit is not blown off");
}
