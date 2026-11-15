//! Skills the second classes borrow from their alternative lines, and the
//! Wizard's rework (COMBAT.md §3): Parry (a Lord Knight's, for the
//! Paladin), Blade Stop (a Monk's, for the Priest), Magic Armor and Meteor
//! Storm's seeking meteors. The rows are `skills.toml`'s with numbers picked
//! so each test is deterministic (Parry at its 75% cap).

use super::cast::seek_knots;
use super::fight::Fighter;
use super::test_support::{map, portal};
use super::tests::profile;
use super::*;
use crate::world::combat::{self, Condition, Loadout, SkillBook};
use crate::world::horde::catalog::{test_def, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{hit, HitEvent, MONSTER_ENTITY_BASE};
use std::sync::Arc;
use uuid::Uuid;

const BOOK: &str = r#"{"skills":[
  {"id":"parry","name":"Parry","class":"swordsman","tier":3,"max_level":10,"passive":true,
   "effects":[{"type":"buff","weapon":"two_hand_sword","parry":[100,0]},{"type":"buff","weapon":"sword","parry":[1,1]}]},
  {"id":"blade_stop","name":"Blade Stop","class":"acolyte","tier":2,"max_level":5,"target":"self","mobile":true,
   "cooldown":[14,-1],"sp":[6,1],"effects":[{"type":"buff","secs":[3,0.4],"catch":[2,0.2],"stun":[1.5,0.2]}]},
  {"id":"energy_coat","name":"Energy Coat","class":"mage","target":"self","sp":[5,0],
   "effects":[{"type":"buff","toggle":true,"mana_shield":true}]},
  {"id":"magic_armor","name":"Magic Armor","class":"mage","tier":2,"max_level":10,"target":"self","mobile":true,
   "cast":[0.3,0],"cooldown":[30,-1],"sp":[14,1.2],
   "effects":[{"type":"buff","secs":20,"temp_hp":"1d8","mult":[1,1],"scale":"k","shield":0.05,"ac":2}]},
  {"id":"meteor_storm","name":"Meteor Storm","class":"mage","tier":2,"max_level":10,"target":"densest","range":16,
   "cast":[1,0],"cooldown":[7,0],"sp":[14,1.5],
   "effects":[{"type":"damage","dice":"2d8","scale":"k","damage":"fire","count":[2,0.5],"every":0.25,"radius":[1.5,0.1],
     "scatter":4,"seek":true,"fall":0.6,"save":{"ability":"body","condition":"stunned","secs":[1,0.1]}}]}
]}"#;

const HZ: f32 = 20.0;
const PLAYER: u32 = 101;

/// A 20 × 9 field (cells 1..21 × 1..10) with still blobs (kind 0) and a
/// king (kind 1, a boss); user 1 joins as a level-`level` `class` holding
/// `weapon`, with `skills` slotted in order, at (6.5, 5.5).
fn stage(class: &str, level: u32, weapon: &str, skills: &[(&str, u8)]) -> HubInstance {
    let row = format!("#{}#", ".".repeat(20));
    let wall = "#".repeat(22);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 9));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let catalog =
        Arc::new(MonsterCatalog::new(vec![test_def("blob", 0.01, 50, 0.0, false), test_def("king", 0.01, 50, 0.0, true)]).unwrap());
    let cfg = HordeConfig { light_radius: 12.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let book = SkillBook::from_json(BOOK).unwrap();
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(Arc::new(book));
    stage.dice = combat::Rng::new(5);
    let mut loadout = Loadout::default();
    loadout.hotbar.resize(combat::skills::SLOTS, String::new());
    for (i, (id, lvl)) in skills.iter().enumerate() {
        loadout.skills.insert(id.to_string(), *lvl);
        loadout.hotbar[i] = id.to_string();
    }
    let mut out = Vec::new();
    let who = PlayerProfile { archetype: class.into(), level, weapon: weapon.into(), loadout, ..profile(1) };
    stage.join(who, Uuid::nil(), None, 0, &mut out);
    let i = stage.find(1).unwrap();
    (stage.players[i].x, stage.players[i].y) = (6.5, 5.5);
    stage
}

fn fighter(stage: &HubInstance) -> &Fighter {
    stage.players[stage.find(1).unwrap()].fighter.as_ref().unwrap()
}

fn fighter_mut(stage: &mut HubInstance) -> &mut Fighter {
    let i = stage.find(1).unwrap();
    stage.players[i].fighter.as_mut().unwrap()
}

/// A monster of `kind` at `(x, y)` that survives a long fight.
fn tough(stage: &mut HubInstance, kind: u8, x: f32, y: f32) -> u16 {
    let horde = stage.horde_mut().unwrap();
    let s = horde.spawn(kind, x, y).unwrap();
    horde.store_mut().hp[s as usize] = 100_000;
    s
}

/// The hit numbers of a tick's frames to user 1.
fn hits(out: &Outgoing) -> Vec<HitEvent> {
    out.iter()
        .filter(|(u, _)| *u == 1)
        .flat_map(|(_, e)| match e {
            HubEvent::Control(ServerMsg::Combat { hits, .. }) => hits.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn controls(out: &Outgoing) -> Vec<ServerMsg> {
    out.iter()
        .filter(|(u, _)| *u == 1)
        .filter_map(|(_, e)| match e {
            HubEvent::Control(m) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

fn stunned(stage: &HubInstance, s: u16) -> bool {
    stage.horde().unwrap().store().status_of(s).has(Condition::Stunned)
}

#[test]
fn parry_needs_a_sword_in_hand() {
    // Two-handed (a katana): the cap; one-handed: its own row; a dagger: none.
    assert_eq!(fighter(&stage("swordsman", 60, "katana", &[("parry", 10)])).mods.parry, 75, "capped at 75%");
    assert_eq!(fighter(&stage("swordsman", 60, "sword", &[("parry", 10)])).mods.parry, 11);
    assert_eq!(fighter(&stage("swordsman", 60, "knife", &[("parry", 10)])).mods.parry, 0);
}

#[test]
fn a_parry_turns_the_blow_aside_and_answers_with_a_riposte() {
    let mut stage = stage("swordsman", 60, "katana", &[("parry", 10)]);
    let s = tough(&mut stage, 0, 7.4, 5.5);
    let max = fighter(&stage).max_hp;
    let mut out = Vec::new();
    let mut parried = false;
    for _ in 0..200 {
        out.clear();
        stage.tick(&mut out);
        if hits(&out).iter().any(|h| h.target == PLAYER && h.flags == hit::GUARD) {
            parried = true;
            break;
        }
    }
    assert!(parried, "75% of the blows are parried");
    // The riposte is ready at once: no wait for the swing timer (only for
    // the moment a player must stand before swinging at all).
    assert!(fighter(&stage).riposte);
    let mut swung = false;
    for _ in 0..3 {
        out.clear();
        stage.tick(&mut out);
        swung = hits(&out).iter().any(|h| h.target == MONSTER_ENTITY_BASE + s as u32);
        if swung {
            break;
        }
    }
    assert!(swung, "the riposte swings at once");
    assert!(!fighter(&stage).riposte, "spent");
    // Parried blows do no damage: only the 25% that get through can.
    assert!(fighter(&stage).hp > max / 2);
}

#[test]
fn blade_stop_catches_a_blow_and_stuns_the_crowd_around() {
    let mut stage = stage("acolyte", 30, "", &[("blade_stop", 5)]);
    // Two attackers in reach, one 2.5 m off (within 2 m + 1 m = 6 cells),
    // one far away, and a boss close by.
    let near = tough(&mut stage, 0, 7.4, 5.5);
    let side = tough(&mut stage, 0, 6.5, 6.4);
    let off = tough(&mut stage, 0, 10.5, 5.5);
    let far = tough(&mut stage, 0, 18.5, 5.5);
    let king = tough(&mut stage, 1, 5.0, 5.5);
    let max = fighter(&stage).max_hp;
    let mut out = Vec::new();
    stage.act(1, [0, 0], 1);
    stage.tick(&mut out);
    // Ready to catch (or it caught a blow on the very tick it was cast).
    let guard = |out: &Outgoing| hits(out).iter().any(|h| h.target == PLAYER && h.flags == hit::GUARD);
    assert!(fighter(&stage).mods.catch > 0.0 || guard(&out), "ready to catch");
    let mut caught = false;
    for _ in 0..40 {
        if hits(&out).iter().any(|h| h.target == PLAYER && h.flags == hit::GUARD) {
            caught = true;
            break;
        }
        out.clear();
        stage.tick(&mut out);
    }
    assert!(caught, "the next blow is caught");
    assert_eq!(fighter(&stage).mods.catch, 0.0, "one catch, then the buff is spent");
    for (s, name) in [(near, "near"), (side, "side"), (off, "2.5 m off")] {
        assert!(stunned(&stage, s), "{name} is stunned");
    }
    assert!(!stunned(&stage, far), "out of reach");
    assert!(!stunned(&stage, king), "bosses shrug it off");
    // The caught blow took nothing (a stunned monster does not swing).
    let taken: u32 = hits(&out).iter().filter(|h| h.target == PLAYER && h.flags & hit::HEAL == 0).map(|h| h.amount).sum();
    assert_eq!(taken, 0);
    assert!(fighter(&stage).hp >= max - 30);
}

#[test]
fn magic_armor_soaks_damage_before_hp_and_energy_coat() {
    let mut stage = stage("mage", 30, "", &[("magic_armor", 10), ("energy_coat", 1)]);
    let mut out = Vec::new();
    stage.act(1, [0, 0], 1);
    for _ in 0..10 {
        stage.tick(&mut out);
    }
    let f = fighter(&stage);
    // 5% of max HP + 11d8 × k: at least 11 + the share.
    let pool = f.temp_hp;
    assert!(pool >= 11 + (f.max_hp as f32 * 0.05).round() as i32, "pool {pool}");
    assert_eq!(f.mods.ac, 2);
    // Energy Coat on too: the plates go first, SP and HP stay.
    stage.act(1, [0, 0], 2);
    for _ in 0..3 {
        stage.tick(&mut out);
    }
    assert!(fighter(&stage).mods.mana_shield);
    let f = fighter_mut(&mut stage);
    let (hp, sp) = (f.hp, f.sp);
    let took = f.take(pool - 1, false);
    assert_eq!(took, pool - 1);
    assert_eq!((f.hp, f.sp, f.temp_hp), (hp, sp, 1), "soaked whole");
    // The rest: the last point of plate, then SP at 1 per 2 HP.
    f.take(5, false);
    assert_eq!((f.hp, f.temp_hp), (hp, 0));
    assert_eq!(f.sp, sp - 2);
    // The plates fall away with the buff.
    let f = fighter_mut(&mut stage);
    f.temp_hp = 5;
    for _ in 0..(21.0 * HZ) as u32 {
        stage.tick(&mut out);
    }
    assert_eq!(fighter(&stage).temp_hp, 0, "gone after 20 s");
}

#[test]
fn seeking_hits_take_the_densest_knots_first() {
    let mut stage = stage("mage", 30, "", &[]);
    // A knot of five, a pair, a loner, and one out of the 8-cell reach.
    let knot: Vec<u16> = [(10.0, 5.0), (10.4, 5.2), (9.8, 5.4), (10.2, 4.7), (10.5, 5.5)].iter().map(|&(x, y)| tough(&mut stage, 0, x, y)).collect();
    let pair = [tough(&mut stage, 0, 14.0, 5.0), tough(&mut stage, 0, 14.3, 5.2)];
    let lone = tough(&mut stage, 0, 12.0, 8.0);
    tough(&mut stage, 0, 20.5, 9.5);
    let _ = (knot, pair, lone);
    let horde = stage.horde().unwrap();
    let at = |k: Option<(f32, f32)>| k.expect("a knot");
    let near = |p: (f32, f32), x: f32, y: f32| (p.0 - x).abs() < 0.6 && (p.1 - y).abs() < 0.6;
    let knots = seek_knots(horde, 12.0, 6.0, 8.0, 1.0, 4);
    assert_eq!(knots.len(), 4);
    assert!(near(at(knots[0]), 10.2, 5.2), "the five first: {:?}", knots[0]);
    assert!(near(at(knots[1]), 14.15, 5.1), "then the pair: {:?}", knots[1]);
    assert!(near(at(knots[2]), 12.0, 8.0), "then the loner: {:?}", knots[2]);
    assert!(near(at(knots[3]), 10.2, 5.2), "all taken: the densest again: {:?}", knots[3]);
    // Nothing in reach: the hits scatter.
    assert_eq!(seek_knots(horde, 3.0, 2.0, 1.0, 1.0, 2), vec![None, None]);
}

#[test]
fn meteor_storm_marks_each_meteor_and_lands_it_on_the_crowd() {
    let mut stage = stage("mage", 30, "", &[("meteor_storm", 10)]);
    let knot: Vec<u16> = [(14.0, 5.0), (14.4, 5.3), (13.7, 5.4), (14.2, 4.7)].iter().map(|&(x, y)| tough(&mut stage, 0, x, y)).collect();
    let mut out = Vec::new();
    stage.act(1, [0, 0], 1);
    let mut strikes = None;
    let mut early = 0;
    for _ in 0..(1.2 * HZ) as u32 {
        out.clear();
        stage.tick(&mut out);
        early += hits(&out).iter().filter(|h| h.target >= MONSTER_ENTITY_BASE).count();
        if let Some(ServerMsg::SkillStrikes { caster, skill, radius, strikes: s }) =
            controls(&out).into_iter().find(|m| matches!(m, ServerMsg::SkillStrikes { .. }))
        {
            assert_eq!((caster, skill.as_str()), (PLAYER, "meteor_storm"));
            assert!((radius - 5.0).abs() < 1e-3, "1.5 m + 0.1 m × 10 = 5 cells: {radius}");
            strikes = Some(s);
            break;
        }
    }
    let strikes = strikes.expect("the meteors are told as the cast lands");
    assert_eq!(early, 0, "nothing lands before it is shown");
    // 2 + 10/2 meteors, each at least the fall (600 ms) away, 250 ms apart.
    assert_eq!(strikes.len(), 7);
    for (i, &(x, y, ms)) in strikes.iter().enumerate() {
        assert!(ms >= 600 + 250 * i as u32 - 50, "meteor {i} at {ms} ms");
        assert!((x - 14.0).abs() < 1.5 && (y - 5.0).abs() < 1.5, "on the knot: {x}, {y}");
    }
    // Then they land on the knot: every monster of it is struck.
    let mut struck = std::collections::BTreeSet::new();
    for _ in 0..(2.5 * HZ) as u32 {
        out.clear();
        stage.tick(&mut out);
        for h in hits(&out) {
            if h.target >= MONSTER_ENTITY_BASE && h.amount > 0 {
                struck.insert((h.target - MONSTER_ENTITY_BASE) as u16);
            }
        }
    }
    for s in knot {
        assert!(struck.contains(&s), "{s} was struck");
    }
}
