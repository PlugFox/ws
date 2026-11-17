//! Orc Village's scripted fights (`scripts/orc.rs`) on a small hall: the
//! Goblin Leader's gang, the Orc Hero's banner, the Orc Lord's drums and
//! boulders, and a readable pace for all three.

use super::*;
use crate::world::combat::{Archetype, Rng};
use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog, MonsterDef};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::hub::boss::scripts;
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
        level: 62,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
        homunculus: None,
    }
}

/// A 70 × 50 hall with the Orc Village specials and their adds.
fn hall(seed: u64) -> HubInstance {
    let w = 70;
    let row = format!("#{}#", ".".repeat(w - 2));
    let wall = "#".repeat(w);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 48));
    rows.push(wall.as_str());
    let field = map(
        "field",
        LocationKind::Field,
        CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
        &rows,
        vec![portal("to_town", [1, 1, 1], "town", [3, 3])],
    );
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let elite = |id: &str| MonsterDef { archetype: Archetype::Elite, radius: 0.6, level: 66, ..test_def(id, 2.0, 12, 0.0, false) };
    let catalog = Arc::new(
        MonsterCatalog::new(vec![
            test_def("orc_lord", 2.0, 50, 0.0, true),
            elite("goblin_leader"),
            elite("ork_hero"),
            test_def("goblin_1", 2.0, 1, 0.0, false),
            test_def("ork_warrior", 2.0, 2, 0.0, false),
            test_def("high_orc", 2.0, 4, 0.0, false),
            test_plant("orc_banner", (5, 5)),
            test_plant("orc_drum", (4, 4)),
        ])
        .unwrap(),
    );
    let cfg = HordeConfig { light_radius: 30.0, director_enabled: false, seed: Some(seed), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde);
    stage.dice = Rng::new(seed);
    let mut out = Vec::new();
    stage.join(profile(), Uuid::nil(), None, 0, &mut out);
    stand(&mut stage, 35.0, 25.0);
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
        stand(stage, 35.0, 25.0);
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

/// Runs until ability `id` is cast (at most `limit` ticks); the messages.
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
fn the_orc_village_scripts_are_listed_with_their_ranks() {
    assert!(scripts::find("goblin_leader").is_some_and(|s| s.rank == Rank::Elite));
    assert!(scripts::find("ork_hero").is_some_and(|s| s.rank == Rank::Elite));
    let lord = scripts::find("orc_lord").expect("the Orc Lord is scripted");
    assert_eq!((lord.rank, lord.phases.len()), (Rank::Boss, 4));
    let rain = lord.abilities.iter().find(|a| a.id == "boss_orc_boulders").unwrap();
    assert_matches!(rain.pattern, Pattern::Rain { count: 8, per_player: 2, .. }, "boulders rain round the marked player");
    let charge = lord.abilities.iter().find(|a| a.id == "boss_orc_charge").unwrap();
    assert!(charge.hurt.push > 0.0 && charge.action == Action::Charge, "the charge throws you aside");
    // Call the Clans opens his third phase (hub/orc.rs forces the horn at that HP).
    let clans = lord.abilities.iter().position(|a| a.id == crate::world::hub::orc::CALL_THE_CLANS).unwrap();
    assert_eq!(lord.phases[2].opener, Some(clans as u8));
    assert_eq!(lord.phases[2].below as i32, crate::world::hub::orc::CLANS_PCT);
}

#[test]
fn the_goblin_leader_whistles_up_a_gang() {
    let mut stage = hall(3);
    let k = kind(&mut stage, "goblin_leader");
    stage.horde_mut().unwrap().spawn(k, 40.0, 25.0).unwrap();
    let msgs = until_cast(&mut stage, "boss_goblin_gang", 900);
    assert!(cast(&msgs, "boss_goblin_gang"));
    run(&mut stage, 40);
    assert!(count(&mut stage, "goblin_1") >= 4, "four goblins for a solo player");
}

#[test]
fn the_orc_hero_plants_a_war_banner_in_his_second_phase() {
    let mut stage = hall(4);
    let k = kind(&mut stage, "ork_hero");
    let slot = stage.horde_mut().unwrap().spawn(k, 40.0, 25.0).unwrap();
    run(&mut stage, 20);
    // His guard first.
    let msgs = until_cast(&mut stage, "boss_hero_vanguard", 400);
    assert!(cast(&msgs, "boss_hero_vanguard"));
    wound(&mut stage, slot, 50);
    let msgs = until_cast(&mut stage, "boss_hero_banner", 900);
    assert!(cast(&msgs, "boss_hero_banner"));
    run(&mut stage, 40);
    assert_eq!(count(&mut stage, "orc_banner"), 1, "a banner stands by him");
}

#[test]
fn the_orc_lord_lays_war_drums_round_him() {
    let mut stage = hall(5);
    let k = kind(&mut stage, "orc_lord");
    run(&mut stage, 2);
    let slot = stage.horde_mut().unwrap().spawn(k, 42.0, 25.0).unwrap();
    run(&mut stage, 2);
    wound(&mut stage, slot, 65);
    let msgs = until_cast(&mut stage, "boss_orc_drums", 900);
    assert!(cast(&msgs, "boss_orc_drums"));
    run(&mut stage, 40);
    assert_eq!(count(&mut stage, "orc_drum"), 3, "three drums for a solo player");
    // Their hatch (a High Orc each after 10 s) is the platform's Lay.
}

#[test]
fn boulder_rain_falls_one_after_another_round_the_marked_player() {
    let mut stage = hall(6);
    let k = kind(&mut stage, "orc_lord");
    run(&mut stage, 2);
    stage.horde_mut().unwrap().spawn(k, 42.0, 25.0).unwrap();
    let msgs = until_cast(&mut stage, "boss_orc_boulders", 1600);
    let zones: Vec<_> = msgs
        .iter()
        .filter_map(|m| match m {
            ServerMsg::BossZones { ability, zones, .. } if ability == "boss_orc_boulders" => Some(zones.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(zones.len(), 8, "eight boulders for a solo player");
    let mut delays: Vec<u32> = zones.iter().map(|z| z.delay_ms).collect();
    delays.sort();
    assert!(delays.windows(2).all(|w| w[1] > w[0]), "one after another: {delays:?}");
    assert!(zones.iter().all(|z| ((z.area.x - 35.0).powi(2) + (z.area.y - 25.0).powi(2)).sqrt() <= 9.5), "within the spread");
}

/// Ability starts in a minute beside the player, at full HP and in the
/// last phase: a readable pace (the ceilings of `balance_tests.rs`).
#[test]
fn the_orc_village_fights_pause_between_abilities() {
    for id in ["goblin_leader", "ork_hero", "orc_lord"] {
        for pct in [100, 10] {
            let mut stage = hall(7);
            let k = kind(&mut stage, id);
            run(&mut stage, 2);
            let slot = stage.horde_mut().unwrap().spawn(k, 40.0, 25.0).unwrap();
            let mut casts = 0;
            for _ in 0..60 * 20 {
                wound(&mut stage, slot, pct);
                let msgs = run(&mut stage, 1);
                casts += msgs.iter().filter(|m| matches!(m, ServerMsg::BossCast { ability, .. } if !ability.is_empty())).count();
            }
            println!("{id:>14} at {pct:>3} %: {casts} abilities/min");
            if pct == 100 {
                assert!((4..=14).contains(&casts), "{id}: {casts} abilities a minute at full HP");
            } else {
                assert!(casts <= 18, "{id}: {casts} abilities a minute in its last phase");
            }
        }
    }
}

/// Records every Orc Village ability as the server sends it (`BOSS_CAST` +
/// `BOSS_ZONES`, with where the caster and the player stood) for the
/// engine demo `demo/orc_boss.html` (replayed on gef_fild10):
///
///     ORC_DUMP=../../assets/engine/demo/orc_casts.json cargo test record_orc_casts -- --ignored
#[test]
#[ignore]
fn record_orc_casts() {
    use crate::world::protocol::zone_flags;
    let Ok(path) = std::env::var("ORC_DUMP") else { return };
    let mut rows = Vec::new();
    for monster in ["goblin_leader", "ork_hero", "orc_lord"] {
        let abilities = scripts::find(monster).unwrap().abilities;
        let mut stage = hall(21);
        let k = kind(&mut stage, monster);
        run(&mut stage, 2);
        let slot = stage.horde_mut().unwrap().spawn(k, 42.0, 25.0).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        'phases: for pct in [100, 58, 38, 14] {
            wound(&mut stage, slot, pct);
            for _ in 0..2400 {
                let msgs = run(&mut stage, 1);
                let store = stage.horde_mut().unwrap().store();
                let at = (store.x[slot as usize], store.y[slot as usize]);
                for m in &msgs {
                    if let ServerMsg::BossCast { ability, name, cast_ms, flags, .. } = m
                        && !ability.is_empty()
                        && seen.insert(ability.clone())
                    {
                        let zones: Vec<serde_json::Value> = msgs
                            .iter()
                            .filter_map(|z| match z {
                                ServerMsg::BossZones { ability: a, zones, .. } if a == ability => Some(zones.clone()),
                                _ => None,
                            })
                            .flatten()
                            .map(|z| {
                                serde_json::json!({
                                    "shape": z.area.shape, "look": z.area.look, "x": z.area.x, "y": z.area.y, "dir": z.area.dir,
                                    "a": z.area.a, "b": z.area.b, "spanMs": z.area.span_ms, "follow": z.area.follow,
                                    "delayMs": z.delay_ms, "safe": z.flags & zone_flags::SAFE != 0,
                                    "centre": z.flags & zone_flags::CENTRE != 0, "lands": z.flags & zone_flags::LANDS != 0,
                                })
                            })
                            .collect();
                        let shout = abilities.iter().find(|a| a.id == ability).map_or("", |a| a.shout);
                        rows.push(serde_json::json!({
                            "monster": monster, "ability": ability, "name": name, "castMs": cast_ms, "flags": flags,
                            "shout": shout, "at": [at.0, at.1], "players": [[35.0, 25.0]], "zones": zones,
                        }));
                    }
                }
                if seen.len() == abilities.len() {
                    break 'phases;
                }
            }
        }
    }
    std::fs::write(&path, serde_json::to_string_pretty(&rows).unwrap()).unwrap();
}
