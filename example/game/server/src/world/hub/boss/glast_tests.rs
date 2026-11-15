//! Glast Heim's scripted fights (`scripts/glast.rs`): Dark Illusion hides
//! behind its images, the Bloody Knight's swords rain round you, and the
//! Dark Lord opens his last phase with a Hell's Gate the party shares.

use super::super::{Action, Anchor, Script, MIN_GAP_SECS};
use super::{BLOODY_KNIGHT, DARK_ILLUSION, DARK_LORD};
use crate::world::combat::{Archetype, Rng};
use crate::world::horde::catalog::{test_def, MonsterCatalog, MonsterDef};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::hub::test_support::{map, portal};
use crate::world::hub::{HubEvent, HubInstance, HubSimConfig, HubWorld, PlayerProfile};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{Look, ServerMsg};
use std::sync::Arc;
use uuid::Uuid;

fn profile() -> PlayerProfile {
    PlayerProfile {
        user_id: 1,
        character_id: 101,
        name: "p1".into(),
        archetype: "acolyte".into(),
        look: Look::default(),
        level: 78,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
        homunculus: None,
    }
}

/// A 60 × 40 churchyard court with the Glast Heim specials and their adds.
fn court(seed: u64) -> HubInstance {
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
    let elite = |id: &str| MonsterDef { archetype: Archetype::Elite, radius: 0.6, level: 80, ..test_def(id, 2.0, 12, 0.0, false) };
    let catalog = Arc::new(
        MonsterCatalog::new(vec![
            MonsterDef { level: 93, ..test_def("dark_lord", 2.0, 50, 0.0, true) },
            elite("dark_illusion"),
            elite("dark_illusion_image"),
            elite("bloody_knight"),
            test_def("raydric", 2.0, 2, 0.0, false),
            test_def("ghoul", 2.0, 2, 0.0, false),
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

fn wound(stage: &mut HubInstance, slot: u16, pct: i32) {
    let horde = stage.horde_mut().unwrap();
    let max = horde.block(slot).hp;
    horde.store_mut().hp[slot as usize] = (max * pct / 100).max(1);
}

fn until_cast(stage: &mut HubInstance, id: &str, limit: u32) -> Vec<ServerMsg> {
    let mut msgs = Vec::new();
    for _ in 0..limit {
        msgs.extend(run(stage, 1));
        if msgs.iter().any(|m| matches!(m, ServerMsg::BossCast { ability, .. } if ability == id)) {
            break;
        }
    }
    msgs
}

fn cast(msgs: &[ServerMsg], id: &str) -> bool {
    msgs.iter().any(|m| matches!(m, ServerMsg::BossCast { ability, .. } if ability == id))
}

#[test]
fn the_scripts_read_as_designed() {
    // Phases by the doc: 100/60/25, 100/55/20, 100/75/50/25.
    let below = |s: &Script| s.phases.iter().map(|p| p.below).collect::<Vec<_>>();
    assert_eq!(below(&DARK_ILLUSION), vec![100, 60, 25]);
    assert_eq!(below(&BLOODY_KNIGHT), vec![100, 55, 20]);
    assert_eq!(below(&DARK_LORD), vec![100, 75, 50, 25]);
    for s in [&DARK_ILLUSION, &BLOODY_KNIGHT, &DARK_LORD] {
        for p in s.phases {
            assert!(p.gap >= MIN_GAP_SECS, "{}: a readable pause", s.monster);
            assert!(p.rotation.iter().chain(p.opener.iter()).all(|&i| (i as usize) < s.abilities.len()));
        }
    }
    // The Hell's Gate is a shared blow, capped at 8 dice alone.
    let gate = DARK_LORD.abilities.iter().find(|a| a.id == "boss_lord_gate").unwrap();
    assert_eq!((gate.hurt.dice, gate.hurt.split, gate.anchor), ((24, 6), 8, Anchor::Marked));
    assert_eq!(DARK_LORD.phases[3].opener, Some(9));
    // Open the Graves opens the churchyard's spawners.
    assert!(DARK_LORD.abilities.iter().any(|a| matches!(a.action, Action::HatchAll { .. })));
}

#[test]
fn dark_illusion_hides_behind_its_images() {
    let mut stage = court(4);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "dark_illusion");
    let image = kind(&mut stage, "dark_illusion_image");
    let slot = stage.horde_mut().unwrap().spawn(k, 36.0, 20.0).unwrap();
    run(&mut stage, 20);
    wound(&mut stage, slot, 55);
    let msgs = until_cast(&mut stage, "boss_illusion_images", 400);
    assert!(cast(&msgs, "boss_illusion_images"));
    run(&mut stage, 40);
    let images = stage.horde_mut().unwrap().monsters().filter(|m| m.2 == image).count();
    assert_eq!(images, 2, "two images");
    let hp = stage.horde_mut().unwrap().store().hp[slot as usize];
    stage.horde_mut().unwrap().store_mut().hp[slot as usize] -= hp / 2;
    run(&mut stage, 2);
    assert_eq!(stage.horde_mut().unwrap().store().hp[slot as usize], hp, "no harm while they stand");
}

#[test]
fn the_bloody_knights_blade_storm_rains_round_you() {
    let mut stage = court(6);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "bloody_knight");
    let slot = stage.horde_mut().unwrap().spawn(k, 34.0, 20.0).unwrap();
    run(&mut stage, 20);
    wound(&mut stage, slot, 50);
    let msgs = until_cast(&mut stage, "boss_knight_storm", 1200);
    assert!(cast(&msgs, "boss_knight_storm"), "the second phase rains swords");
    let raydric = kind(&mut stage, "raydric");
    assert!(stage.horde_mut().unwrap().monsters().filter(|m| m.2 == raydric).count() >= 2, "his guard came first");
}

#[test]
fn the_dark_lord_opens_his_last_phase_with_the_hells_gate() {
    let mut stage = court(8);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "dark_lord");
    let slot = stage.horde_mut().unwrap().spawn(k, 36.0, 20.0).unwrap();
    run(&mut stage, 20);
    wound(&mut stage, slot, 20);
    let msgs = until_cast(&mut stage, "boss_lord_gate", 1200);
    assert!(cast(&msgs, "boss_lord_gate"));
}
