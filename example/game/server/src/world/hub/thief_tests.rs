//! The Thief line (COMBAT.md §3.5) on a stage: Double Attack, the poisons,
//! hiding and Cloaking (an ambush), Back Stab's blink behind its target
//! and Tunnel Drive on the travel key. The table below is the thief rows of
//! `skills.toml` with numbers picked so each test is deterministic (Double
//! Attack and Enchant Poison at 100%).

use super::cast::TRAVEL_SLOT;
use super::fight::Fighter;
use super::test_support::{map, portal};
use super::tests::profile;
use super::*;
use crate::world::combat::{self, Condition, Loadout, SkillBook};
use crate::world::horde::catalog::{test_def, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::MONSTER_ENTITY_BASE;
use std::sync::Arc;
use uuid::Uuid;

const BOOK: &str = r#"{"skills":[
  {"id":"double_attack","name":"Double Attack","class":"thief","max_level":10,"passive":true,
   "effects":[{"type":"buff","double":[0,10]}]},
  {"id":"steal","name":"Steal","class":"thief","max_level":10,"passive":true,
   "effects":[{"type":"buff","loot":[0,0.04]}]},
  {"id":"hiding","name":"Hiding","class":"thief","max_level":10,"target":"self","cooldown":[24,-1.2],"sp":[6,0.4],
   "effects":[{"type":"buff","secs":[2,0.5],"hidden":true,"ambush":true,"hp_regen":1.0}]},
  {"id":"cloaking","name":"Cloaking","class":"thief","max_level":10,"target":"self","mobile":true,"cooldown":[30,-1.5],"sp":[12,0.8],
   "effects":[{"type":"buff","secs":[5,0.5],"hidden":true,"cloak":true,"ambush":true,"speed":[-0.25,0.05]}]},
  {"id":"back_stab","name":"Back Stab","class":"thief","max_level":10,"target":"strongest","range":6,"cooldown":[6,-0.2],"sp":[10,0.5],
   "effects":[{"type":"move","mode":"behind","distance":0.6},{"type":"damage","dice":"weapon","extra":"1d8","mult":[1,0.5]}]},
  {"id":"enchant_poison","name":"Enchant Poison","class":"thief","max_level":10,"target":"self","mobile":true,"cooldown":30,"sp":[10,0.5],
   "effects":[{"type":"buff","secs":[20,4],"venom":[10,10]}]},
  {"id":"venom_dust","name":"Venom Dust","class":"thief","max_level":10,"target":"densest","range":6,"cast":0.3,"cooldown":8,"sp":[8,0.6],
   "effects":[{"type":"condition","condition":"poisoned","radius":[1.5,0.1],"secs":[5,1]}]},
  {"id":"tunnel_drive","name":"Tunnel Drive","class":"thief","max_level":5,"target":"escape","mobile":true,"range":4,"cooldown":[8,-0.6],"sp":[4,0],
   "effects":[{"type":"move","mode":"slide","distance":[2.5,0.5]},{"type":"buff","secs":1.0,"hidden":true,"cloak":true,"ambush":true}]}
]}"#;

/// A 20 × 9 field (cells 1..21 × 1..10); a level-10 thief (user 1, a
/// knife) with `skills` slotted in order; monsters are slow blobs.
fn stage(skills: &[(&str, u8)]) -> HubInstance {
    let row = format!("#{}#", ".".repeat(20));
    let wall = "#".repeat(22);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 9));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 0.01, 50, 0.0, false)]).unwrap());
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
    stage.join(PlayerProfile { archetype: "thief".into(), level: 10, loadout, ..profile(1) }, Uuid::nil(), None, 0, &mut out);
    put(&mut stage, 1, 6.5, 5.5);
    stage
}

fn put(stage: &mut HubInstance, user: u32, x: f32, y: f32) {
    let i = stage.find(user).unwrap();
    (stage.players[i].x, stage.players[i].y) = (x, y);
}

fn fighter(stage: &HubInstance) -> &Fighter {
    stage.players[stage.find(1).unwrap()].fighter.as_ref().unwrap()
}

/// A blob at `(x, y)` that does not die of a few stabs.
fn tough(stage: &mut HubInstance, x: f32, y: f32) -> u16 {
    let horde = stage.horde_mut().unwrap();
    let s = horde.spawn(0, x, y).unwrap();
    horde.store_mut().hp[s as usize] = 10_000;
    s
}

/// Ticks `n` times with `slots` pressed on the first; the player's swings
/// and the hits on monster `s` (misses included).
fn run(stage: &mut HubInstance, n: u32, slots: u16, s: u16) -> (usize, usize) {
    let mut out = Vec::new();
    stage.act(1, [0, 0], slots);
    for _ in 0..n {
        stage.tick(&mut out);
    }
    let (mut swings, mut hits) = (0, 0);
    for (u, e) in &out {
        if let (1, HubEvent::Control(ServerMsg::Combat { swings: sw, hits: h })) = (*u, e) {
            swings += sw.iter().filter(|w| w.entity_id == 101).count();
            hits += h.iter().filter(|h| h.target == MONSTER_ENTITY_BASE + s as u32).count();
        }
    }
    (swings, hits)
}

fn poisoned(stage: &HubInstance, s: u16) -> bool {
    stage.horde().unwrap().store().status_of(s).has(Condition::Poisoned)
}

#[test]
fn double_attack_swings_once_more() {
    // Without it: one hit per swing at tier 1.
    let mut plain = stage(&[]);
    let s = tough(&mut plain, 7.5, 5.5);
    let (swings, hits) = run(&mut plain, 60, 0, s);
    assert!(swings >= 3, "{swings} swings");
    assert_eq!(hits, swings, "one roll per swing");
    // At 100% (the test's 10 per level): two on every swing.
    let mut double = stage(&[("double_attack", 10)]);
    assert_eq!(fighter(&double).mods.double, 100);
    let s = tough(&mut double, 7.5, 5.5);
    let (swings, hits) = run(&mut double, 60, 0, s);
    assert!(swings >= 3);
    assert_eq!(hits, 2 * swings, "a second roll on each swing");
}

#[test]
fn steal_raises_the_thiefs_own_drop_odds() {
    let stage = stage(&[("steal", 10)]);
    assert!((fighter(&stage).mods.loot - 0.4).abs() < 1e-6, "4% per level");
}

#[test]
fn hiding_loses_the_horde_until_the_thief_moves() {
    let mut stage = stage(&[("hiding", 10)]);
    // A monster in sight but out of the knife's reach.
    tough(&mut stage, 12.5, 5.5);
    run(&mut stage, 1, 1, 0);
    let f = fighter(&stage);
    assert!(f.mods.hidden && f.mods.ambush && !f.hunted(), "hidden, monsters ignore the thief");
    // Moving ends it (Hiding is no Cloaking).
    stage.input(1, 1, 0.0, 1.0);
    stage.tick(&mut Vec::new());
    assert!(!fighter(&stage).mods.hidden && fighter(&stage).hunted());
}

#[test]
fn a_swing_out_of_hiding_ends_it() {
    let mut stage = stage(&[("hiding", 10)]);
    run(&mut stage, 1, 1, 0);
    assert!(fighter(&stage).mods.hidden, "nothing in reach: still hidden");
    let s = tough(&mut stage, 7.5, 5.5);
    let (swings, hits) = run(&mut stage, 30, 0, s);
    assert!(swings >= 1 && hits >= 1, "the thief strikes out of hiding ({swings} swings)");
    assert!(!fighter(&stage).mods.hidden, "and is seen again");
}

#[test]
fn cloaking_holds_while_walking_and_a_skill_ends_it() {
    let mut stage = stage(&[("cloaking", 10), ("enchant_poison", 1)]);
    run(&mut stage, 1, 1, 0);
    assert!(fighter(&stage).mods.cloak && fighter(&stage).mods.hidden);
    assert!((fighter(&stage).mods.speed - 0.25).abs() < 1e-6, "+25% at level 10");
    let (_, x0, _) = stage.position_of(1).unwrap();
    for seq in 1..10 {
        stage.input(1, seq, 1.0, 0.0);
        stage.tick(&mut Vec::new());
    }
    let (_, x1, _) = stage.position_of(1).unwrap();
    assert!(x1 > x0 + 1.0, "walked {x0} -> {x1}");
    assert!(fighter(&stage).mods.hidden && !fighter(&stage).hunted(), "walking keeps the cloak");
    // Any skill comes out of it.
    stage.input(1, 20, 0.0, 0.0);
    let i = stage.find(1).unwrap();
    let f = stage.players[i].fighter.as_mut().unwrap();
    f.sp = f.max_sp;
    run(&mut stage, 1, 2, 0);
    assert!(!fighter(&stage).mods.hidden, "a cast ends the cloak");
    assert!(fighter(&stage).mods.venom > 0, "and still lands");
}

#[test]
fn back_stab_blinks_behind_the_strongest_and_stabs_it() {
    let mut stage = stage(&[("back_stab", 5)]);
    let s = tough(&mut stage, 10.5, 5.5);
    let (_, hits) = run(&mut stage, 2, 1, s);
    let (_, x, y) = stage.position_of(1).unwrap();
    assert!(x > 10.5 && x < 12.5 && (y - 5.5).abs() < 0.5, "behind the blob, at ({x}, {y})");
    assert!(hits >= 1, "the stab rolled against it");
    // It is no travel skill: the travel key stays free.
    assert_eq!(fighter(&stage).travel, None);
}

#[test]
fn enchant_poison_poisons_on_weapon_hits() {
    let mut stage = stage(&[("enchant_poison", 9)]);
    run(&mut stage, 1, 1, 0);
    assert_eq!(fighter(&stage).mods.venom, 100);
    let s = tough(&mut stage, 7.5, 5.5);
    let hp = stage.horde().unwrap().store().hp[s as usize];
    let mut ticks = 0;
    while stage.horde().unwrap().store().hp[s as usize] == hp && ticks < 100 {
        run(&mut stage, 1, 0, s);
        ticks += 1;
    }
    assert!(ticks < 100, "a stab landed");
    assert!(poisoned(&stage, s), "the hit left it Poisoned");
}

#[test]
fn venom_dust_poisons_without_a_save() {
    let mut stage = stage(&[("venom_dust", 5)]);
    let blobs: Vec<u16> = (0..3).map(|i| tough(&mut stage, 11.5, 4.5 + i as f32)).collect();
    run(&mut stage, 10, 1, 0);
    assert!(blobs.iter().all(|&s| poisoned(&stage, s)), "every blob in the cloud is Poisoned");
}

#[test]
fn tunnel_drive_slips_away_unseen_on_the_travel_key() {
    let mut stage = stage(&[("tunnel_drive", 5), ("back_stab", 1)]);
    let book_index = stage.skills.find("tunnel_drive").unwrap().0;
    assert_eq!(fighter(&stage).travel, Some(book_index), "the travel key is Tunnel Drive, not Back Stab");
    tough(&mut stage, 8.5, 5.5);
    let (_, x0, _) = stage.position_of(1).unwrap();
    run(&mut stage, 1, 1 << TRAVEL_SLOT, 0);
    let (_, x1, _) = stage.position_of(1).unwrap();
    assert!(x1 < x0 - 2.0, "slid away from the blob: {x0} -> {x1}");
    assert!(fighter(&stage).mods.hidden && fighter(&stage).mods.cloak, "unseen where it ends");
    // A second later it wears off.
    let hz = stage.tick_hz();
    run(&mut stage, hz + 2, 0, 0);
    assert!(!fighter(&stage).mods.hidden);
}
