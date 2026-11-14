//! Payon Caves' boss patterns and tricks: the fox-fire spiral, marching
//! ranks with a gap, chasing ghost hands, the mirror images and their
//! crossfire, and a wail that terrifies (`boss.rs`, `scripts/payon.rs`).

use super::*;
use crate::world::combat::Archetype;
use crate::world::horde::catalog::{test_def, MonsterCatalog, MonsterDef};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::hub::test_support::{map, portal};
use crate::world::hub::{HubSimConfig, HubWorld, PlayerProfile};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::Look;
use std::sync::Arc;
use uuid::Uuid;

fn open(w: usize, h: usize) -> WalkGrid {
    let wall = "#".repeat(w);
    let row = format!("#{}#", ".".repeat(w - 2));
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), h - 2));
    rows.push(wall.as_str());
    WalkGrid::from_ascii(&rows)
}

fn shapes(p: Pattern, walk: &WalkGrid, anchor: (f32, f32), spin: f32) -> Vec<Shape> {
    let mut out = Vec::new();
    expand(&p, &Action::None, walk, anchor, anchor, (1.0, 0.0), spin, 1.0, &mut out);
    out
}

fn beats_on(v: &[Shape], x: f32, y: f32) -> Vec<f32> {
    v.iter().filter(|s| s.zone.contains(x, y, 0.0)).map(|s| s.delay).collect()
}

#[test]
fn a_spiral_unrolls_its_arms_one_step_a_beat() {
    let walk = open(80, 80);
    let p = Pattern::Spiral { arms: 3, steps: 4, first: 2.0, step: 2.0, turn: 30.0, r: 1.0, every: 0.2 };
    let v = shapes(p, &walk, (40.0, 40.0), 0.0);
    assert_eq!(v.len(), 12, "3 arms × 4 steps");
    // The first arm starts along the aim, 2 cells out, at once.
    assert_eq!(beats_on(&v, 42.0, 40.0), vec![0.0]);
    // Its second step is 4 cells out, turned 30°, a beat later.
    let (s, c) = 30f32.to_radians().sin_cos();
    assert_eq!(beats_on(&v, 40.0 + 4.0 * c, 40.0 + 4.0 * s), vec![0.2]);
    // The caster's own spot and the space between the arms are quiet.
    assert!(beats_on(&v, 40.0, 40.0).is_empty());
    assert!(beats_on(&v, 40.0 - 3.0, 40.0 + 0.5).is_empty());
}

#[test]
fn every_rank_leaves_one_gap_and_marches_on() {
    let walk = open(80, 80);
    let p = Pattern::Bars { count: 3, len: 20.0, width: 2.0, gap: 4.0, first: 3.0, advance: 4.0, every: 0.5 };
    for spin in [0.3, 2.0, 5.1] {
        let v = shapes(p, &walk, (40.0, 40.0), spin);
        for i in 0..3 {
            let x = 40.0 + 3.0 + i as f32 * 4.0;
            let delay = i as f32 * 0.5;
            // Walk along the rank (across the aim): it is solid but for one gap.
            let hit: Vec<bool> = (-18..=18).map(|k| beats_on(&v, x, 40.0 + k as f32 * 0.5).contains(&delay)).collect();
            assert!(hit.iter().filter(|h| !**h).count() >= 6, "rank {i} (spin {spin}) leaves a gap of ~4 cells");
            assert!(hit.iter().filter(|h| **h).count() >= 25, "and is solid elsewhere");
            let openings = hit.windows(2).filter(|w| w[0] && !w[1]).count();
            assert!(openings <= 1, "one gap, not several");
        }
    }
}

#[test]
fn a_chase_bites_where_its_target_is() {
    let walk = open(40, 40);
    let v = shapes(Pattern::Chase { count: 3, r: 1.5, every: 1.0 }, &walk, (10.0, 10.0), 0.0);
    assert_eq!(v.iter().map(|s| s.delay).collect::<Vec<_>>(), vec![0.0, 1.0, 2.0]);
}

fn profile() -> PlayerProfile {
    PlayerProfile {
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
    }
}

/// A 60 × 40 hall with the Payon specials and their adds.
fn hall(seed: u64) -> HubInstance {
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
    // Band-level elites (their save DC and dice are the band's).
    let elite = |id: &str| MonsterDef { archetype: Archetype::Elite, radius: 0.6, level: 58, ..test_def(id, 2.0, 12, 0.0, false) };
    let catalog = Arc::new(
        MonsterCatalog::new(vec![
            test_def("moonlight_flower", 2.0, 50, 0.0, true),
            elite("moonlight_mirror"),
            elite("weeping_bride"),
            elite("jiangshi_general"),
            test_def("nine_tail", 2.0, 2, 0.0, false),
            test_def("sohee", 2.0, 2, 0.0, false),
            test_def("bongun", 2.0, 2, 0.0, false),
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

/// Runs until ability `id` is cast (at most `limit` ticks).
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

#[test]
fn moonlight_flower_hides_behind_her_mirror_images_and_fires_between_them() {
    let mut stage = hall(3);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "moonlight_flower");
    let mirror = kind(&mut stage, "moonlight_mirror");
    let slot = stage.horde_mut().unwrap().spawn(k, 36.0, 20.0).unwrap();
    run(&mut stage, 20);
    // Under 75 % she opens the second phase with Mirror Image.
    wound(&mut stage, slot, 70);
    let msgs = until_cast(&mut stage, "boss_moon_mirror", 400);
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossCast { ability, .. } if ability == "boss_moon_mirror")));
    run(&mut stage, 40);
    let images: Vec<u16> = stage.horde_mut().unwrap().monsters().filter(|m| m.2 == mirror).map(|m| m.0).collect();
    assert_eq!(images.len(), 2, "two images for a solo player");
    // While they stand, she takes no harm.
    let hp = stage.horde_mut().unwrap().store().hp[slot as usize];
    stage.horde_mut().unwrap().store_mut().hp[slot as usize] -= 500;
    run(&mut stage, 2);
    assert_eq!(stage.horde_mut().unwrap().store().hp[slot as usize], hp, "the hit went to nothing");
    // Crossfire: a bar between every pair of the three, and one from each at the player.
    let msgs = until_cast(&mut stage, "boss_moon_crossfire", 600);
    let zones = msgs
        .iter()
        .rev()
        .find_map(|m| match m {
            ServerMsg::BossZones { ability, zones, .. } if ability == "boss_moon_crossfire" => Some(zones.clone()),
            _ => None,
        })
        .expect("the crossfire is telegraphed");
    assert_eq!(zones.len(), 6, "3 pairs + 3 bars at the player");
    // The images shatter: her hide is gone.
    for m in images {
        stage.horde_mut().unwrap().despawn(m);
    }
    let msgs = run(&mut stage, 3);
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossShout { text, .. } if text == MIRRORS_BROKEN)));
    let hp = stage.horde_mut().unwrap().store().hp[slot as usize];
    stage.horde_mut().unwrap().store_mut().hp[slot as usize] -= 100;
    run(&mut stage, 2);
    assert!(stage.horde_mut().unwrap().store().hp[slot as usize] < hp, "hits land again");
}

#[test]
fn the_brides_wail_terrifies_and_the_terrified_flee_her() {
    // A Mind save can shake it off: over a few seeded stages it lands.
    let mut feared = None;
    for seed in 1..12 {
        let mut stage = hall(seed);
        stand(&mut stage, 30.0, 20.0);
        let k = kind(&mut stage, "weeping_bride");
        stage.horde_mut().unwrap().spawn(k, 32.5, 20.0).unwrap();
        // Her rotation opens with the wail; its first ring (2..4.5) is on the player.
        let msgs = until_cast(&mut stage, "boss_bride_wail", 200);
        assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossCast { ability, .. } if ability == "boss_bride_wail")));
        // She walked up meanwhile: step into her first ring (2..4.5 cells).
        let slot = stage.bosses.fights()[0].slot;
        let (bx, by) = { let st = stage.horde_mut().unwrap().store(); (st.x[slot as usize], st.y[slot as usize]) };
        stand(&mut stage, bx - 3.2, by);
        for _ in 0..80 {
            run(&mut stage, 1);
            if stage.players[0].fighter.as_ref().unwrap().status.has(Condition::Feared) {
                feared = Some(stage);
                break;
            }
        }
        if feared.is_some() {
            break;
        }
    }
    let mut stage = feared.expect("a failed Mind save leaves the player feared");
    let before = stage.players[0].x;
    run(&mut stage, 6);
    assert!(stage.players[0].x < before, "the terrified player runs away from her ({before} -> {})", stage.players[0].x);
    assert!(stage.players[0].fighter.as_ref().unwrap().status.helpless(), "and cannot fight while it lasts");
}

#[test]
fn ghost_hands_follow_their_player() {
    let mut stage = hall(11);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "weeping_bride");
    let slot = stage.horde_mut().unwrap().spawn(k, 33.0, 20.0).unwrap();
    // Keep her from the wail's fear: the veil comes second in her rotation.
    let msgs = until_cast(&mut stage, "boss_bride_veil", 800);
    let zones = msgs
        .iter()
        .rev()
        .find_map(|m| match m {
            ServerMsg::BossZones { ability, zones, .. } if ability == "boss_bride_veil" => Some(zones.clone()),
            _ => None,
        })
        .expect("the hands are telegraphed");
    assert_eq!(zones.len(), 3);
    assert!(zones.iter().all(|z| z.area.follow == 101), "each follows the marked player");
    let _ = slot;
}

/// The night's safe spots: the lit braziers near her, or moonlit circles
/// when too few burn.
fn night_safe_spots(lit: &[(f32, f32)]) -> Vec<(f32, f32, f32)> {
    let mut stage = hall(4);
    stand(&mut stage, 30.0, 20.0);
    let mut out = Vec::new();
    for &(x, y) in lit {
        stage.payon_kindle(0, x, y, &mut out);
    }
    let k = kind(&mut stage, "moonlight_flower");
    run(&mut stage, 2);
    let slot = stage.horde_mut().unwrap().spawn(k, 34.0, 20.0).unwrap();
    run(&mut stage, 2);
    wound(&mut stage, slot, 20);
    let msgs = until_cast(&mut stage, "boss_moon_night", 600);
    let zones = msgs
        .iter()
        .rev()
        .find_map(|m| match m {
            ServerMsg::BossZones { ability, zones, .. } if ability == "boss_moon_night" => Some(zones.clone()),
            _ => None,
        })
        .expect("the night is telegraphed");
    zones.iter().filter(|z| z.flags & zone_flags::SAFE != 0).map(|z| (z.area.x, z.area.y, z.area.a)).collect()
}

#[test]
fn lanternless_night_spares_whoever_stands_by_a_lit_brazier() {
    let safe = night_safe_spots(&[(20.0, 20.0), (44.0, 28.0)]);
    assert_eq!(safe.len(), 2, "two lit braziers: no moonlit circle needed");
    assert!(safe.iter().any(|s| (s.0, s.1, s.2) == (20.0, 20.0, 4.0)));
    assert!(safe.iter().any(|s| (s.0, s.1, s.2) == (44.0, 28.0, 4.0)));
    let dark = night_safe_spots(&[]);
    assert_eq!(dark.len(), 2, "none lit: two moonlit circles");
    assert!(dark.iter().all(|s| s.2 == 2.6));
    let one = night_safe_spots(&[(22.0, 14.0)]);
    assert_eq!((one.len(), one.iter().filter(|s| s.2 == 2.6).count()), (2, 1), "one lit, one moonlit");
}

/// Records every Payon ability as the server sends it (`BOSS_CAST` +
/// `BOSS_ZONES`, with where the caster, its images and the players stood)
/// for the engine demo `demo/payon.html` (replayed on pay_dun04):
///
///     PAYON_DUMP=../../assets/engine/demo/payon_casts.json cargo test record_payon_casts -- --ignored
#[test]
#[ignore]
fn record_payon_casts() {
    let Ok(path) = std::env::var("PAYON_DUMP") else { return };
    let mut rows = Vec::new();
    for monster in ["weeping_bride", "jiangshi_general", "moonlight_flower"] {
        let abilities = scripts::find(monster).unwrap().abilities;
        let mut stage = hall(21);
        let mut out = Vec::new();
        let mut second = profile();
        (second.user_id, second.character_id, second.name) = (2, 102, "p2".into());
        stage.join(second, Uuid::nil(), None, 0, &mut out);
        let k = kind(&mut stage, monster);
        let mirror = kind(&mut stage, "moonlight_mirror");
        // A tick first: the stage counts its party (boss HP scales with it).
        run(&mut stage, 2);
        let slot = stage.horde_mut().unwrap().spawn(k, 34.0, 20.0).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        'phases: for pct in [100, 58, 40, 18] {
            // Its images shatter first (else they keep her HP where it was).
            let images: Vec<u16> = stage.horde_mut().unwrap().monsters().filter(|m| m.2 == mirror).map(|m| m.0).collect();
            for m in images {
                stage.horde_mut().unwrap().despawn(m);
            }
            run(&mut stage, 1);
            wound(&mut stage, slot, pct);
            for _ in 0..1600 {
                // Players hold still a few cells off (the patterns read best).
                for (i, (x, y)) in [(28.0, 18.0), (31.0, 25.0)].into_iter().enumerate() {
                    let p = &mut stage.players[i];
                    (p.x, p.y) = (x, y);
                    let f = p.fighter.as_mut().unwrap();
                    (f.max_hp, f.hp) = (1_000_000, 1_000_000);
                    f.recovery = u32::MAX;
                }
                let msgs = run(&mut stage, 1);
                let store = stage.horde_mut().unwrap().store();
                let at = (store.x[slot as usize], store.y[slot as usize]);
                let images: Vec<(f32, f32)> =
                    stage.horde_mut().unwrap().monsters().filter(|m| m.2 == mirror).map(|m| (m.3, m.4)).collect();
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
                            "shout": shout, "at": [at.0, at.1], "images": images, "players": [[28.0, 18.0], [31.0, 25.0]], "zones": zones,
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
