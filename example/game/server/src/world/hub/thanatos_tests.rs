//! Thanatos Tower on an open hall the size of tha_t02: the boss forming in
//! the nave, the fragments' mending, Oblivion's Judgment, beams judging
//! angels, and the mission table's acts and tubes (`thanatos.rs`).

use super::super::test_support::map;
use super::super::{HubEvent, HubSimConfig, HubWorld, PlayerProfile};
use super::*;
use crate::world::combat;
use crate::world::combat::Archetype;
use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::missions;
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
        level: 95,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
        homunculus: None,
    }
}

/// A 300 × 300 open hall (walls round it) on the tower's mission; player 1
/// joins at the south gate. The director is off: the tests set the clock.
fn stage() -> HubInstance {
    let wall = "#".repeat(300);
    let row = format!("#{}#", ".".repeat(298));
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 298));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let elite = |id: &str| {
        let mut d = test_def(id, 0.5, 20, 0.0, false);
        d.archetype = Archetype::Elite;
        d
    };
    let mut defs = vec![test_def("thanatos", 0.5, 50, 0.0, true), elite("tha_dolor"), elite("tha_maero"), test_def("shelter", 0.5, 10, 0.0, false), test_def("deathword", 0.5, 1, 0.0, false)];
    for c in ["r", "b", "g", "p", "y"] {
        defs.push(test_plant(&format!("specimen_tube_{c}"), (3, 3)));
        defs.push(test_def(&format!("plasma_{c}"), 0.5, 5, 0.0, false));
    }
    let catalog = Arc::new(MonsterCatalog::new(defs).unwrap());
    let cfg = HordeConfig { light_radius: 30.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, MISSION, world, HubSimConfig { residents: 0, ..Default::default() }).with_horde(horde);
    stage.dice = combat::Rng::new(5);
    let mut out = Vec::new();
    stage.join(profile(1), Uuid::nil(), None, 0, &mut out);
    let i = stage.find(1).unwrap();
    (stage.players[i].x, stage.players[i].y) = (113.5, 23.5);
    let f = stage.players[i].fighter.as_mut().unwrap();
    (f.max_hp, f.hp) = (1_000_000, 1_000_000);
    stage
}

fn at(stage: &mut HubInstance, secs: f32, out: &mut Outgoing) {
    stage.horde_mut().unwrap().set_director_elapsed(secs);
    stage.tick(out);
}

/// A few ticks at `secs` (the tower scans every third).
fn settle(stage: &mut HubInstance, secs: f32) -> Outgoing {
    let mut out = Vec::new();
    for k in 0..4 {
        at(stage, secs + k as f32 * 0.05, &mut out);
    }
    out
}

fn spawn(stage: &mut HubInstance, id: &str, x: f32, y: f32) -> u16 {
    let h = stage.horde_mut().unwrap();
    let k = h.catalog().kind_of(id).unwrap();
    h.spawn(k, x, y).unwrap()
}

fn zones(out: &Outgoing, ability: &str) -> usize {
    out.iter()
        .filter_map(|(u, e)| match e {
            HubEvent::Control(ServerMsg::BossZones { ability: a, zones, .. }) if *u == 1 && a == ability => Some(zones.len()),
            _ => None,
        })
        .sum()
}

#[test]
fn thanatos_forms_in_the_nave() {
    let mut s = stage();
    settle(&mut s, 599.0);
    let b = spawn(&mut s, BOSS, 200.5, 250.5);
    settle(&mut s, 600.0);
    let st = s.horde_mut().unwrap().store();
    assert!((st.x[b as usize] - NAVE.0).abs() < 0.6 && (st.y[b as usize] - NAVE.1).abs() < 0.6, "in the nave: ({}, {})", st.x[b as usize], st.y[b as usize]);
}

#[test]
fn a_standing_fragment_mends_him_and_a_dead_one_does_not() {
    let mut s = stage();
    settle(&mut s, 600.0);
    let b = spawn(&mut s, BOSS, 148.5, 95.5);
    let max = s.horde_mut().unwrap().block(b).hp;
    s.horde_mut().unwrap().store_mut().hp[b as usize] = max / 2;
    settle(&mut s, 601.0);
    // No fragment: no mending.
    settle(&mut s, 603.0);
    assert_eq!(s.horde_mut().unwrap().store().hp[b as usize], max / 2);
    let d = spawn(&mut s, "tha_dolor", 160.5, 95.5);
    let out = settle(&mut s, 605.0);
    let hp = s.horde_mut().unwrap().store().hp[b as usize];
    assert!(hp > max / 2, "a fragment mends him: {hp} of {max}");
    assert!(zones(&out, "tower_mend") >= 1, "the link shows");
    s.horde_mut().unwrap().despawn(d);
    settle(&mut s, 607.0);
    settle(&mut s, 609.0);
    assert_eq!(s.horde_mut().unwrap().store().hp[b as usize], hp, "no fragment, no mending");
}

#[test]
fn oblivion_calls_the_judgment_in_his_last_phase() {
    let mut s = stage();
    settle(&mut s, 700.0);
    let b = spawn(&mut s, BOSS, 148.5, 95.5);
    settle(&mut s, 701.0);
    // 10:30 + 15 s is past: no Judgment runs at 11:40.
    assert!(s.env_event_start(tower::JUDGMENT).is_none());
    let max = s.horde_mut().unwrap().block(b).hp;
    s.horde_mut().unwrap().store_mut().hp[b as usize] = max * 20 / 100;
    let out = settle(&mut s, 702.0);
    assert!(s.env_event_start(tower::JUDGMENT).is_some(), "Oblivion forced it");
    // The statues open their eyes: a calm gold ring at each.
    let out2 = settle(&mut s, 703.0);
    assert_eq!(zones(&out, "tower_eyes") + zones(&out2, "tower_eyes"), tower::STATUES.len());
}

#[test]
fn a_beam_on_angels_mends_them_and_spares_the_rest() {
    let mut s = stage();
    settle(&mut s, 450.0);
    let a = spawn(&mut s, "shelter", 100.5, 100.5);
    let w = spawn(&mut s, "deathword", 101.0, 100.5);
    let (ma, mw) = (s.horde_mut().unwrap().block(a).hp, s.horde_mut().unwrap().block(w).hp);
    s.horde_mut().unwrap().store_mut().hp[a as usize] = ma / 2;
    s.horde_mut().unwrap().store_mut().hp[w as usize] = (mw / 2).max(1);
    let wv = s.horde_mut().unwrap().store().hp[w as usize];
    let mut out = Vec::new();
    let map = s.horde_mut().unwrap().map();
    s.tower_judged(map, 100.5, 100.5, &mut out);
    let st = s.horde_mut().unwrap().store();
    assert!(st.hp[a as usize] > ma / 2, "the angel is judged worthy");
    assert_eq!(st.hp[w as usize], wv, "a book is not");
    assert_eq!(zones(&out, "tower_judged"), 1);
}

#[test]
fn beams_never_land_beside_a_statue() {
    let m = missions::find(MISSION).unwrap();
    let st = m.env.strikes.iter().find(|s| s.id == tower::BEAM).expect("the Judgment's beams");
    assert_eq!(st.during, tower::JUDGMENT);
    for &(x, y) in tower::STATUES {
        assert!(!st.allowed(x + 2.0, y), "beside a statue");
        assert!(STATUE_RING < st.mask_r, "the calm ring lies inside the mask");
    }
    assert!(st.allowed(60.5, 60.5));
}

#[test]
fn the_climb_is_three_acts_with_their_own_rosters() {
    let m = missions::find(MISSION).unwrap();
    let ev = |id: &str| m.env.event(id).unwrap_or_else(|| panic!("event {id}"));
    assert_eq!((ev("stair_lab").first_secs, ev("stair_sanctum").first_secs, ev("summit").first_secs), (180.0, 360.0, 600.0));
    assert_eq!(ev(tower::JUDGMENT).start(0), Some(450.0));
    let retire = |id: &str| m.roster.iter().find(|r| r.monster == id).map_or(f32::INFINITY, |r| r.retire_secs);
    // Act I retires as the Sanctum opens, Act II as Thanatos comes.
    assert_eq!(retire("deathword"), ev("stair_sanctum").first_secs);
    assert_eq!(retire("plasma_y"), ev("summit").first_secs);
    assert!(retire("shelter").is_infinite() && retire("thanatos").is_infinite());
    assert!(m.tips.len() >= 6 && m.bosses == [("thanatos", "Thanatos")]);
}

#[test]
fn every_tube_hatches_its_own_colour_from_two_minutes() {
    let m = missions::find(MISSION).unwrap();
    assert_eq!(m.env.objects.len(), 40);
    let mut spots = Vec::new();
    for o in m.env.objects {
        let c = o.monster.strip_prefix("specimen_tube_").expect("a tube");
        let h = o.hatch.expect("it hatches");
        assert_eq!(h.monster, format!("plasma_{c}"));
        assert!((120.0..=330.0).contains(&h.secs) && h.count == 2 && h.every_secs == 0.0);
        spots.extend_from_slice(o.spots);
    }
    for (i, a) in spots.iter().enumerate() {
        for b in &spots[i + 1..] {
            assert!((a.0 - b.0).hypot(a.1 - b.1) >= 1.5, "tubes crowd each other at {a:?}");
        }
    }
    // Planted on the stage and cracking on time.
    let mut s = stage();
    settle(&mut s, 1.0);
    let alive = |s: &mut HubInstance, id: &str| {
        let h = s.horde_mut().unwrap();
        let k = h.catalog().kind_of(id).unwrap();
        h.store().alive().iter().filter(|&&x| h.store().kind[x as usize] == k).count()
    };
    assert_eq!(alive(&mut s, "specimen_tube_r"), 8);
    settle(&mut s, 121.0);
    assert_eq!((alive(&mut s, "specimen_tube_r"), alive(&mut s, "plasma_r")), (7, 2), "the first cluster cracked");
}

#[test]
fn the_scripts_are_in_and_his_armour_breaks() {
    use super::super::boss::scripted;
    for id in ["tha_odium", "tha_despero", BOSS] {
        assert!(scripted(id), "{id} fights by a script");
    }
    for id in FRAGMENTS {
        assert!(!scripted(id), "the fragments are plain elites");
    }
}
