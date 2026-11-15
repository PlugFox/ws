//! The Thief line (COMBAT.md §3.5) on a stage: Double Attack, the poisons,
//! hiding and Cloaking (an ambush: no swing, no auto-cast, buffs keep it,
//! an attack skill ends it), Back Stab's blink behind its target and its
//! bonus from behind or out of hiding, the bombs and Tunnel Drive on the
//! travel key. The table below is the thief rows of
//! `skills.toml` with numbers picked so each test is deterministic (Double
//! Attack and Enchant Poison at 100%).

use super::cast::TRAVEL_SLOT;
use super::fight::{Fighter, STATUS_CLOAKED, STATUS_HIDDEN};
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
   "effects":[{"type":"move","mode":"behind","distance":0.6},{"type":"damage","dice":"weapon","extra":"1d8","mult":[1,0.5],"flank":[1,0]}]},
  {"id":"smoke_bomb","name":"Smoke Bomb","class":"thief","max_level":5,"target":"self","mobile":true,"cooldown":24,"sp":10,
   "effects":[{"type":"condition","condition":"blinded","radius":2.5,"at":"caster","secs":4},
              {"type":"condition","condition":"slowed","radius":2.5,"at":"caster","secs":4},
              {"type":"buff","secs":1.5,"hidden":true,"cloak":true,"ambush":true,"ac":4}]},
  {"id":"poison_bomb","name":"Poison Bomb","class":"thief","max_level":10,"target":"densest","range":7,"cast":0.25,"cooldown":10,"sp":8,
   "effects":[{"type":"damage","dice":"1d6","scale":"k","damage":"poison","radius":1.5,"secs":4,"pulse":1.0,"projectile":12,
               "save":{"ability":"body","condition":"poisoned","secs":3}}]},
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
    stage_with(BOOK, skills)
}

/// [`stage`] on another skill table.
fn stage_with(book: &str, skills: &[(&str, u8)]) -> HubInstance {
    let row = format!("#{}#", ".".repeat(20));
    let wall = "#".repeat(22);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 9));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 0.01, 50, 0.0, false), test_def("runner", 2.0, 50, 0.0, false)]).unwrap());
    let cfg = HordeConfig { light_radius: 12.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let book = SkillBook::from_json(book).unwrap();
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

/// A quick monster at `(x, y)` that does not die (it comes at the thief).
fn runner(stage: &mut HubInstance, x: f32, y: f32) -> u16 {
    let horde = stage.horde_mut().unwrap();
    let s = horde.spawn(1, x, y).unwrap();
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

/// The status bits of user 1's vitals as the stage sees them.
fn vitals_status(stage: &mut HubInstance) -> u32 {
    let mut out = Vec::new();
    stage.announce_fighter(stage.find(1).unwrap(), &mut out);
    out.iter()
        .find_map(|(u, e)| match (u, e) {
            (1, HubEvent::Control(ServerMsg::PlayerVitals { entity_id: 101, status, .. })) => Some(*status),
            _ => None,
        })
        .expect("a vitals frame")
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
fn hiding_swings_at_nothing_until_an_attack_skill_strikes_out_of_it() {
    let mut stage = stage(&[("hiding", 10), ("back_stab", 1)]);
    run(&mut stage, 1, 1, 0);
    let s = tough(&mut stage, 7.5, 5.5);
    let (swings, hits) = run(&mut stage, 30, 0, s);
    assert_eq!((swings, hits), (0, 0), "a monster in reach: no automatic swing gives the thief away");
    assert!(fighter(&stage).mods.hidden && !fighter(&stage).hunted());
    // The stage sees a mound of earth (the burrowed bit in the vitals).
    assert_eq!(vitals_status(&mut stage) & STATUS_HIDDEN, STATUS_HIDDEN);
    // Back Stab comes out of it (and the vitals say so).
    let (_, hits) = run(&mut stage, 2, 2, s);
    assert!(hits >= 1, "the stab rolled");
    assert!(!fighter(&stage).mods.hidden, "and the thief is seen again");
    assert_eq!(vitals_status(&mut stage) & (STATUS_HIDDEN | STATUS_CLOAKED), 0);
}

#[test]
fn hidden_auto_cast_slots_wait_and_buffs_keep_the_hiding() {
    let mut stage = stage(&[("hiding", 10), ("venom_dust", 5), ("enchant_poison", 1)]);
    run(&mut stage, 1, 1, 0);
    let s = tough(&mut stage, 9.5, 5.5);
    // Venom Dust on auto-cast: it would give the thief away, so it waits.
    let i = stage.find(1).unwrap();
    stage.players[i].prefs.auto = 0b10;
    run(&mut stage, 10, 0b10, 0);
    assert!(!poisoned(&stage, s) && fighter(&stage).mods.hidden, "no auto-cast out of hiding");
    // A buff by hand keeps the thief under the ground.
    let f = stage.players[i].fighter.as_mut().unwrap();
    f.sp = f.max_sp;
    run(&mut stage, 1, 0b100, 0);
    assert!(fighter(&stage).mods.venom > 0 && fighter(&stage).mods.hidden, "Enchant Poison keeps the hiding");
    // The same attack skill pressed by hand strikes out of it.
    stage.players[i].prefs.auto = 0;
    let f = stage.players[i].fighter.as_mut().unwrap();
    f.sp = f.max_sp;
    run(&mut stage, 10, 0b10, 0);
    assert!(poisoned(&stage, s) && !fighter(&stage).mods.hidden, "an attack skill ends the hiding");
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
    // A buff keeps it; the cloak shows as a shimmer, not a mound.
    stage.input(1, 20, 0.0, 0.0);
    let i = stage.find(1).unwrap();
    let f = stage.players[i].fighter.as_mut().unwrap();
    f.sp = f.max_sp;
    run(&mut stage, 1, 2, 0);
    assert!(fighter(&stage).mods.hidden && fighter(&stage).mods.venom > 0, "a buff lands and keeps the cloak");
    assert_eq!(vitals_status(&mut stage) & (STATUS_HIDDEN | STATUS_CLOAKED), STATUS_CLOAKED);
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

/// Back Stab's damage summed over `n` stabs on monster `s`, each with the
/// cooldown and SP reset, `s` placed at `from` and given a few ticks to
/// come at the thief, out of hiding (`hidden`: Hiding first) or not.
fn stabs(stage: &mut HubInstance, s: u16, from: (f32, f32), n: u32, hidden: bool) -> u32 {
    let mut total = 0;
    for _ in 0..n {
        let i = stage.find(1).unwrap();
        let f = stage.players[i].fighter.as_mut().unwrap();
        f.cooldowns.iter_mut().for_each(|c| *c = 0);
        (f.sp, f.recovery) = (f.max_sp, 0);
        put(stage, 1, 6.5, 5.5);
        stage.horde_mut().unwrap().place(s, from.0, from.1);
        for _ in 0..6 {
            stage.tick(&mut Vec::new());
        }
        if hidden {
            run(stage, 1, 1, 0);
            assert!(fighter(stage).mods.hidden);
        }
        let mut out = Vec::new();
        stage.act(1, [0, 0], 2);
        stage.tick(&mut out);
        for (u, e) in &out {
            if let (1, HubEvent::Control(ServerMsg::Combat { hits, .. })) = (*u, e) {
                total += hits.iter().filter(|h| h.target == MONSTER_ENTITY_BASE + s as u32).map(|h| h.amount).sum::<u32>();
            }
        }
    }
    total
}

#[test]
fn back_stab_strikes_harder_from_behind_or_out_of_hiding() {
    // A runner coming at the thief: the blink lands at its back. The same
    // stabs with and without the bonus (twice the dice in the test table).
    let flat = BOOK.replace(r#""flank":[1,0]"#, r#""flank":[0,0]"#);
    let mut plain = stage_with(&flat, &[("hiding", 10), ("back_stab", 4)]);
    let s = runner(&mut plain, 14.5, 5.5);
    let without = stabs(&mut plain, s, (14.5, 5.5), 40, false);
    let mut stage = stage(&[("hiding", 10), ("back_stab", 4)]);
    let s = runner(&mut stage, 14.5, 5.5);
    let behind = stabs(&mut stage, s, (14.5, 5.5), 40, false);
    assert!(behind as f32 > without as f32 * 1.5, "from behind: twice the dice ({without} -> {behind})");
    // Which blows count: a back turned to the thief, or out of hiding.
    let horde = stage.horde_mut().unwrap();
    horde.store_mut().vel_x[s as usize] = -0.05;
    let st = horde.store();
    let flank = super::cast::Flank { more: 1.0, from: (st.x[s as usize] + 1.2, st.y[s as usize]), ambush: false };
    assert!(flank.holds(horde, s), "its back is to the thief");
    let facing = super::cast::Flank { from: (st.x[s as usize] - 3.0, st.y[s as usize]), ..flank };
    assert!(!facing.holds(horde, s), "walking at the thief: no back");
    horde.store_mut().vel_x[s as usize] = 0.0;
    assert!(!flank.holds(stage.horde().unwrap(), s), "standing still: no back");
    assert!(super::cast::Flank { ambush: true, ..facing }.holds(stage.horde().unwrap(), s), "out of hiding always");
    // Out of hiding the bonus holds on a monster that has no back to the
    // thief (a blob that barely moves), with advantage on the roll too.
    let mut plain = stage_with(&flat, &[("hiding", 10), ("back_stab", 4)]);
    let blob = tough(&mut plain, 9.5, 5.5);
    let mut flank_book = self::stage(&[("hiding", 10), ("back_stab", 4)]);
    let b = tough(&mut flank_book, 9.5, 5.5);
    let without = stabs(&mut plain, blob, (9.5, 5.5), 40, false);
    let hidden = stabs(&mut flank_book, b, (9.5, 5.5), 40, true);
    assert!(hidden as f32 > without as f32 * 1.5, "out of hiding ({without} -> {hidden})");
}

#[test]
fn smoke_bomb_blinds_and_slows_and_hides_the_thief() {
    let mut stage = stage(&[("smoke_bomb", 1)]);
    let blobs: Vec<u16> = (0..3).map(|i| tough(&mut stage, 7.0 + i as f32, 5.5)).collect();
    let far = tough(&mut stage, 18.5, 5.5);
    run(&mut stage, 2, 1, 0);
    let st = |s: u16| stage.horde().unwrap().store().status_of(s);
    assert!(blobs.iter().all(|&s| st(s).has(Condition::Blinded) && st(s).has(Condition::Slowed)), "the cloud blinds and slows");
    assert!(!st(far).has(Condition::Blinded), "not out of it");
    let f = fighter(&stage);
    assert!(f.mods.hidden && f.mods.cloak && !f.hunted() && f.mods.ac >= 4, "the thief vanishes in the smoke");
}

#[test]
fn poison_bomb_lingers_and_poisons() {
    let mut stage = stage(&[("poison_bomb", 1)]);
    let s = tough(&mut stage, 11.5, 5.5);
    let hp = stage.horde().unwrap().store().hp[s as usize];
    let hz = stage.tick_hz();
    let (_, hits) = run(&mut stage, hz * 3, 1, s);
    assert!(hits >= 2, "the cloud strikes once a second ({hits} hits)");
    assert!(stage.horde().unwrap().store().hp[s as usize] < hp);
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
