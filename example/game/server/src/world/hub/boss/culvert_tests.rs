//! Prontera Culvert's fights (`scripts/culvert.rs`): the Golden Thief Bug's
//! burrow, shell and coins, the Thief Bug Queen's eggs, the Rat King's
//! plague trail.

use super::*;
use crate::world::combat::{Archetype, DamageType};
use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog, MonsterDef};
use crate::world::horde::storage::FLAG_REFLECT;
use crate::world::horde::{Horde, HordeConfig};
use crate::world::hub::test_support::{map, portal};
use crate::world::hub::{HubSimConfig, HubWorld, PlayerProfile};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{look, Look};
use std::sync::Arc;
use uuid::Uuid;

/// An open 80 × 40 hall on the culvert mission (its eggs' spots lie off
/// this small map and are skipped).
fn hall(seed: u64) -> HubInstance {
    let w = 80;
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
    let elite = |id: &str| MonsterDef { archetype: Archetype::Elite, radius: 0.6, ..test_def(id, 2.0, 26, 0.0, false) };
    let catalog = Arc::new(
        MonsterCatalog::new(vec![
            test_def("golden_bug", 3.0, 35, 0.0, true),
            elite("thief_bug_queen"),
            elite("rat_king"),
            test_plant("thief_bug_egg", (3, 3)),
            test_def("thief_bug_larva", 2.0, 15, 0.0, false),
            test_def("thief_bug_female", 2.0, 22, 0.0, false),
            test_def("tarou", 2.0, 17, 0.0, false),
        ])
        .unwrap(),
    );
    let cfg = HordeConfig { light_radius: 40.0, director_enabled: false, seed: Some(seed), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, "prontera_culvert", world, HubSimConfig::default()).with_horde(horde);
    stage.dice = Rng::new(seed);
    let profile = PlayerProfile {
        user_id: 1,
        character_id: 101,
        name: "p1".into(),
        archetype: "swordsman".into(),
        look: Look::default(),
        level: 25,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
    };
    let mut out = Vec::new();
    stage.join(profile, Uuid::nil(), None, 0, &mut out);
    stage
}

fn kind(stage: &mut HubInstance, id: &str) -> u8 {
    stage.horde_mut().unwrap().catalog().kind_of(id).unwrap()
}

fn stand(stage: &mut HubInstance, x: f32, y: f32) {
    let p = &mut stage.players[0];
    (p.x, p.y) = (x, y);
    let f = p.fighter.as_mut().unwrap();
    (f.max_hp, f.hp) = (1_000_000, 1_000_000);
    f.recovery = u32::MAX;
}

fn tick(stage: &mut HubInstance) -> Vec<ServerMsg> {
    let mut out = Vec::new();
    stage.tick(&mut out);
    out.into_iter()
        .filter(|(u, _)| *u == 1)
        .filter_map(|(_, e)| match e {
            HubEvent::Control(m) => Some(m),
            _ => None,
        })
        .collect()
}

fn set_hp(stage: &mut HubInstance, slot: u16, pct: i32) {
    let horde = stage.horde_mut().unwrap();
    let max = horde.block(slot).hp;
    horde.store_mut().hp[slot as usize] = (max * pct / 100).max(1);
}

fn cast(m: &ServerMsg, id: &str) -> bool {
    matches!(m, ServerMsg::BossCast { ability, .. } if ability == id)
}

#[test]
fn the_golden_thief_bug_burrows_tunnels_unseen_and_bursts_up_under_the_ripple() {
    let mut stage = hall(5);
    stand(&mut stage, 20.0, 20.0);
    let k = kind(&mut stage, "golden_bug");
    let slot = stage.horde_mut().unwrap().spawn(k, 40.0, 20.0).unwrap();
    // Wounded into its second phase, it opens with Burrow.
    set_hp(&mut stage, slot, 65);
    let mut dove = false;
    let mut seen_under = None;
    let mut hit = false;
    for t in 0..400 {
        let msgs = tick(&mut stage);
        if msgs.iter().any(|m| cast(m, "boss_burrow")) {
            dove = true;
            let zones = msgs.iter().find_map(|m| match m {
                ServerMsg::BossZones { ability, zones, .. } if ability == "boss_burrow" => Some(zones.clone()),
                _ => None,
            });
            let z = zones.expect("the ripple is telegraphed")[0];
            assert_eq!((z.area.look, z.area.follow), (look::RIPPLE, MONSTER_ENTITY_BASE + slot as u32), "a ripple riding on its mound");
        }
        let horde = stage.horde().unwrap();
        if dove && horde.store().burrowed(slot) && seen_under.is_none() {
            assert!(horde.nearest_foe(20.0, 20.0, 60.0).is_none(), "nothing can find it under the ground");
            seen_under = Some((t, horde.store().x[slot as usize]));
        }
        hit |= msgs.iter().any(|m| {
            matches!(m, ServerMsg::Combat { hits, .. } if hits.iter().any(|h| h.target == 101 && h.amount > 0 && h.flags >> 4 == DamageType::Physical as u8))
        });
        if dove && seen_under.is_some() && !stage.horde().unwrap().store().burrowed(slot) {
            break;
        }
    }
    assert!(dove, "it cast Burrow");
    let (_, x0) = seen_under.expect("it went under");
    let x1 = stage.horde().unwrap().store().x[slot as usize];
    assert!(x1 < x0 - 5.0, "it tunnelled toward the player: {x0} -> {x1}");
    assert!((x1 - 20.0).abs() < 4.0, "and came up under them: {x1}");
    assert!(!stage.horde().unwrap().store().burrowed(slot), "up again");
    assert!(hit, "the player who stood still under the ripple was struck");
}

#[test]
fn its_shell_turns_spells_until_cracked_then_it_bleeds_gold() {
    let mut stage = hall(6);
    stand(&mut stage, 20.0, 20.0);
    let k = kind(&mut stage, "golden_bug");
    let slot = stage.horde_mut().unwrap().spawn(k, 30.0, 20.0).unwrap();
    for _ in 0..30 {
        tick(&mut stage);
    }
    assert_ne!(stage.horde().unwrap().store().flags[slot as usize] & FLAG_REFLECT, 0, "whole: it reflects");
    // Cracked (phase 3): no more reflection; every 10 % lost spills zeny.
    set_hp(&mut stage, slot, 34);
    let mut msgs = Vec::new();
    for _ in 0..5 {
        msgs.extend(tick(&mut stage));
    }
    assert_eq!(stage.horde().unwrap().store().flags[slot as usize] & FLAG_REFLECT, 0, "cracked: spells land");
    set_hp(&mut stage, slot, 12);
    for _ in 0..5 {
        msgs.extend(tick(&mut stage));
    }
    let coins = msgs.iter().filter(|m| matches!(m, ServerMsg::Loot { item, entity_id, .. } if item == "zeny" && *entity_id == MONSTER_ENTITY_BASE + slot as u32)).count();
    // 34 % -> 12 %, and the cracked shell takes a quarter more of that
    // (~6.5 %): it spilled at 25 % and 15 %, not yet at 5 %.
    assert_eq!(coins, 2);
    let hp = stage.horde().unwrap().store().hp[slot as usize];
    let max = stage.horde_mut().unwrap().block(slot).hp;
    assert!(hp * 100 < max * 8, "the vulnerable shell took more: {hp} / {max}");
}

#[test]
fn the_queen_lays_eggs_and_her_brood_call_hatches_them() {
    let mut stage = hall(7);
    stand(&mut stage, 20.0, 20.0);
    let k = kind(&mut stage, "thief_bug_queen");
    let egg = kind(&mut stage, "thief_bug_egg");
    let larva = kind(&mut stage, "thief_bug_larva");
    stage.horde_mut().unwrap().spawn(k, 28.0, 20.0).unwrap();
    let count = |stage: &HubInstance, kind: u8| stage.horde().unwrap().monsters().filter(|m| m.2 == kind).count();
    let mut laid = 0;
    let mut called = false;
    for _ in 0..1500 {
        let msgs = tick(&mut stage);
        laid = laid.max(count(&stage, egg));
        called |= msgs.iter().any(|m| cast(m, "boss_brood_call"));
        if called && count(&stage, larva) > 0 {
            break;
        }
        // Keep the player out of its acid and dives (no blows on the eggs).
        stand(&mut stage, 20.0, 20.0);
    }
    assert_eq!(laid, 3, "three eggs for a solo player");
    assert!(called, "it called the brood");
    assert_eq!(count(&stage, larva), 6, "each egg broke into two larvae");
    assert_eq!(count(&stage, egg), 0);
}

#[test]
fn the_rat_kings_plague_trail_drops_puddles_where_its_quarry_was() {
    let mut stage = hall(8);
    stand(&mut stage, 20.0, 20.0);
    let k = kind(&mut stage, "rat_king");
    stage.horde_mut().unwrap().spawn(k, 30.0, 20.0).unwrap();
    let mut puddles = Vec::new();
    let mut x = 20.0;
    for _ in 0..2000 {
        let msgs = tick(&mut stage);
        for m in &msgs {
            if let ServerMsg::BossZones { ability, zones, .. } = m
                && ability == "boss_plague_trail"
            {
                puddles.extend(zones.iter().map(|z| (z.area.x, z.area.id, z.area.span_ms)));
            }
        }
        if puddles.len() >= 8 {
            break;
        }
        // Walk west through the hall, then back east from the far side.
        x = if x < 4.0 { 70.0 } else { x - 0.2 };
        stand(&mut stage, x, 20.0);
    }
    assert_eq!(puddles.len(), 8, "eight puddles, one at a time");
    assert!(puddles.iter().all(|&(_, id, span)| id != 0 && span == 5000), "each is lasting ground");
    let xs: Vec<f32> = puddles.iter().map(|p| p.0).collect();
    assert!(xs.windows(2).all(|w| w[1] < w[0] || w[1] > 60.0), "each one where the player had walked to: {xs:?}");
}
