//! Toy Factory's scripted fights (`scripts/toy.rs`): the Foreman's clock
//! hands and his shift, Hatii's litter and blizzard, and the Stormy
//! Knight's wind that throws you and his Absolute Zero.

use super::super::{Action, Anchor, Pattern, Script, MIN_GAP_SECS};
use super::{FOREMAN, HATII, STORMY_KNIGHT};
use crate::world::combat::{Archetype, Rng};
use crate::world::horde::catalog::{test_def, MonsterCatalog, MonsterDef};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::hub::test_support::{map, portal};
use crate::world::hub::{HubEvent, HubInstance, HubSimConfig, HubWorld, PlayerProfile};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{Look, ServerMsg};
use std::sync::Arc;
use uuid::Uuid;
use std::assert_matches;

fn profile() -> PlayerProfile {
    PlayerProfile {
        user_id: 1,
        character_id: 101,
        name: "p1".into(),
        archetype: "swordsman".into(),
        look: Look::default(),
        level: 72,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
        homunculus: None,
    }
}

/// A 60 × 40 factory floor with the Toy Factory specials and their adds.
fn floor(seed: u64) -> HubInstance {
    let w = 60;
    let row = format!("#{}#", ".".repeat(w - 2));
    let wall = "#".repeat(w);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 38));
    rows.push(wall.as_str());
    let field = map(
        "field",
        LocationKind::Field,
        CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
        &rows,
        vec![portal("to_town", [1, 1, 1], "town", [3, 3])],
    );
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let elite = |id: &str| MonsterDef { archetype: Archetype::Elite, radius: 0.6, level: 72, ..test_def(id, 2.0, 12, 0.0, false) };
    let catalog = Arc::new(
        MonsterCatalog::new(vec![
            MonsterDef { level: 85, ..test_def("knight_of_windstorm", 2.0, 50, 0.0, true) },
            elite("foreman"),
            elite("hatii"),
            test_def("alarm", 2.0, 2, 0.0, false),
            test_def("garm_baby", 2.0, 2, 0.0, false),
            test_def("cruiser", 2.0, 2, 0.0, false),
        ])
        .unwrap(),
    );
    let cfg = HordeConfig { light_radius: 30.0, director_enabled: false, seed: Some(seed), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde);
    stage.dice = Rng::new(seed);
    let mut out = Vec::new();
    stage.join(profile(), Uuid::nil(), None, 0, &mut out);
    stage
}

fn stand(stage: &mut HubInstance, x: f32, y: f32) {
    let p = &mut stage.players[0];
    (p.x, p.y) = (x, y);
    let f = p.fighter.as_mut().unwrap();
    (f.max_hp, f.hp) = (1_000_000, 1_000_000);
    f.recovery = u32::MAX;
}

fn run(stage: &mut HubInstance, ticks: u32) -> Vec<ServerMsg> {
    let mut out = Vec::new();
    for _ in 0..ticks {
        stage.tick(&mut out);
    }
    out.into_iter()
        .filter(|(u, _)| *u == 1)
        .filter_map(|(_, e)| match e {
            HubEvent::Control(m) => Some(m),
            _ => None,
        })
        .collect()
}

fn kind(stage: &mut HubInstance, id: &str) -> u8 {
    stage.horde_mut().unwrap().catalog().kind_of(id).unwrap()
}

fn count(stage: &mut HubInstance, id: &str) -> usize {
    let k = kind(stage, id);
    stage.horde_mut().unwrap().monsters().filter(|m| m.2 == k).count()
}

fn wound(stage: &mut HubInstance, slot: u16, pct: i32) {
    let horde = stage.horde_mut().unwrap();
    let max = horde.block(slot).hp;
    horde.store_mut().hp[slot as usize] = (max * pct / 100).max(1);
}

fn until_cast(stage: &mut HubInstance, id: &str, limit: u32) -> Vec<ServerMsg> {
    let mut msgs = Vec::new();
    for _ in 0..limit {
        msgs.extend(run(stage, 1));
        if cast(&msgs, id) {
            break;
        }
    }
    msgs
}

fn cast(msgs: &[ServerMsg], id: &str) -> bool {
    msgs.iter().any(|m| matches!(m, ServerMsg::BossCast { ability, .. } if ability == id))
}

fn ability<'a>(s: &'a Script, id: &str) -> &'a super::super::Ability {
    s.abilities.iter().find(|a| a.id == id).unwrap_or_else(|| panic!("{}: no {id}", s.monster))
}

#[test]
fn the_scripts_read_as_designed() {
    // Phases by the doc: 100/60/25, 100/55/20, 100/70/40/15.
    let below = |s: &Script| s.phases.iter().map(|p| p.below).collect::<Vec<_>>();
    assert_eq!(below(&FOREMAN), vec![100, 60, 25]);
    assert_eq!(below(&HATII), vec![100, 55, 20]);
    assert_eq!(below(&STORMY_KNIGHT), vec![100, 70, 40, 15]);
    for s in [&FOREMAN, &HATII, &STORMY_KNIGHT] {
        assert!(super::super::find(s.monster).is_some(), "{} is listed in SCRIPTS", s.monster);
        for p in s.phases {
            assert!(p.gap >= MIN_GAP_SECS, "{}: a readable pause", s.monster);
            assert!(p.rotation.iter().chain(p.opener.iter()).all(|&i| (i as usize) < s.abilities.len()));
        }
        // Every ability is used somewhere.
        for i in 0..s.abilities.len() as u8 {
            assert!(s.phases.iter().any(|p| p.opener == Some(i) || p.rotation.contains(&i)), "{}: ability {i} unused", s.monster);
        }
    }
    // Midnight is in the Foreman's last phase (hub/toy.rs blows the shift
    // change under 25 %); the clock hands turn opposite ways.
    let midnight = FOREMAN.abilities.iter().position(|a| a.id == "boss_foreman_midnight").unwrap() as u8;
    assert!(FOREMAN.phases[2].rotation.contains(&midnight));
    assert_eq!(crate::world::hub::toy::MIDNIGHT_BELOW, FOREMAN.phases[2].below as i64);
    let step = |id| match ability(&FOREMAN, id).pattern {
        Pattern::Cross { step, .. } => step,
        p => panic!("{p:?}"),
    };
    assert!(step("boss_foreman_minute") > 0.0 && step("boss_foreman_hour") < 0.0);
    // Hatii's and the Knight's storms rain; the gale and the lances push.
    assert_matches!(ability(&HATII, "boss_hatii_blizzard").pattern, Pattern::Rain { count: 10, .. });
    assert_matches!(ability(&STORMY_KNIGHT, "boss_stormy_gust").pattern, Pattern::Rain { count: 5, .. });
    assert_eq!(ability(&STORMY_KNIGHT, "boss_stormy_gale").hurt.push, 6.0);
    assert_eq!(ability(&STORMY_KNIGHT, "boss_stormy_lance").hurt.push, 3.0);
    assert_eq!(ability(&STORMY_KNIGHT, "boss_stormy_double").anchor, Anchor::Each(2));
    // Absolute Zero: a shield to break, three safe vents, opens the last
    // phase and comes back each cycle.
    let zero = ability(&STORMY_KNIGHT, "boss_stormy_zero");
    assert_matches!(zero.action, Action::Shield { pct: 10 });
    assert_matches!(zero.pattern, Pattern::Sanctuary { safe: 3, .. });
    assert!(!zero.hurt.save && zero.hurt.dice == (10, 10));
    let z = STORMY_KNIGHT.abilities.iter().position(|a| a.id == "boss_stormy_zero").unwrap() as u8;
    assert_eq!(STORMY_KNIGHT.phases[3].opener, Some(z));
    assert!(STORMY_KNIGHT.phases[3].rotation.contains(&z));
}

#[test]
fn the_foreman_calls_the_next_shift_in_his_last_phase() {
    let mut stage = floor(3);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "foreman");
    let slot = stage.horde_mut().unwrap().spawn(k, 35.0, 20.0).unwrap();
    run(&mut stage, 20);
    wound(&mut stage, slot, 20);
    let msgs = until_cast(&mut stage, "boss_foreman_shift", 600);
    assert!(cast(&msgs, "boss_foreman_shift"), "the last phase opens with the shift");
    run(&mut stage, 60);
    assert!(count(&mut stage, "alarm") >= 2, "alarms dropped in");
    let msgs = until_cast(&mut stage, "boss_foreman_midnight", 1200);
    assert!(cast(&msgs, "boss_foreman_midnight"), "and Midnight chimes");
}

#[test]
fn hatii_opens_with_her_litter_then_a_blizzard_rains_round_you() {
    let mut stage = floor(5);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "hatii");
    let slot = stage.horde_mut().unwrap().spawn(k, 35.0, 20.0).unwrap();
    let msgs = until_cast(&mut stage, "boss_hatii_litter", 400);
    assert!(cast(&msgs, "boss_hatii_litter"));
    run(&mut stage, 60);
    assert!(count(&mut stage, "garm_baby") >= 3, "her pups came");
    wound(&mut stage, slot, 50);
    let mut msgs = until_cast(&mut stage, "boss_hatii_blizzard", 1200);
    assert!(cast(&msgs, "boss_hatii_blizzard"), "the second phase brings the blizzard");
    msgs.extend(run(&mut stage, 120));
    let icicles: usize = msgs
        .iter()
        .filter_map(|m| match m {
            ServerMsg::BossZones { ability, zones, .. } if ability == "boss_hatii_blizzard" => Some(zones.len()),
            _ => None,
        })
        .sum();
    assert_eq!(icicles, 10, "ten icicles for a lone fighter");
}

#[test]
fn the_stormy_knights_gale_throws_you_back() {
    let mut stage = floor(7);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "knight_of_windstorm");
    let slot = stage.horde_mut().unwrap().spawn(k, 34.0, 20.0).unwrap();
    let msgs = until_cast(&mut stage, "boss_stormy_gale", 2400);
    assert!(cast(&msgs, "boss_stormy_gale"));
    let (kx, ky) = {
        let s = stage.horde_mut().unwrap().store();
        (s.x[slot as usize], s.y[slot as usize])
    };
    let dist = |s: &HubInstance| ((s.players[0].x - kx).powi(2) + (s.players[0].y - ky).powi(2)).sqrt();
    let before = dist(&stage);
    let hz = stage.config.tick_hz;
    run(&mut stage, hz * 2);
    assert!(dist(&stage) >= before + 4.0, "thrown back: {before} -> {}", dist(&stage));
}

#[test]
fn the_stormy_knight_opens_his_last_phase_with_absolute_zero() {
    let mut stage = floor(9);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "knight_of_windstorm");
    let slot = stage.horde_mut().unwrap().spawn(k, 36.0, 20.0).unwrap();
    run(&mut stage, 20);
    wound(&mut stage, slot, 50);
    let msgs = until_cast(&mut stage, "boss_stormy_parade", 1200);
    assert!(cast(&msgs, "boss_stormy_parade"));
    run(&mut stage, 60);
    assert!(count(&mut stage, "cruiser") >= 3, "the toy soldiers marched out");
    wound(&mut stage, slot, 10);
    let msgs = until_cast(&mut stage, "boss_stormy_zero", 1200);
    assert!(cast(&msgs, "boss_stormy_zero"));
}
