//! Monster behaviours in the hub (`horde/tactics.rs`, `brain.rs`): a
//! kiter shoots only what it sees and a pack of them never fills the sky,
//! a healer beams the most wounded monster in range, a support rallies
//! only over a pack. The table below is made up for the tests.

use super::test_support::{map, portal};
use super::tests::profile;
use super::*;
use crate::world::combat::{self, SkillBook};
use crate::world::horde::catalog::{test_def, MonsterCatalog, MonsterDef};
use crate::world::horde::tactics::Behaviour;
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{hit, look, HitEvent, MONSTER_ENTITY_BASE};
use std::sync::Arc;
use uuid::Uuid;

const BOOK: &str = r#"{"skills":[],"monster_skills":[
  {"id":"shot","name":"Shot","target":"nearest","range":8,"cast":0.6,"cooldown":1,"ranged":true,
   "effects":[{"type":"damage","dice":"1d4","damage":"physical","shape":"line","width":0.5,"at":"caster"}]},
  {"id":"mend","name":"Mend","target":"nearest","range":5,"cast":0.6,"cooldown":2,"interrupt":true,
   "effects":[{"type":"rally","shape":"line","width":0.5,"at":"caster","heal":0.2}]},
  {"id":"cry","name":"War Cry","target":"self","range":3,"cast":0.5,"cooldown":2,"interrupt":true,
   "effects":[{"type":"rally","radius":3,"at":"caster","secs":5,"to_hit":2,"speed":0.25}]}
]}"#;

/// A 40 × 13 field; with `wall`, a wall across x = 20 but for the top
/// and bottom rows. Kinds: archer (kiter, shot), priest (healer, mend),
/// lady (support, cry), grunt; all but stand still. A level-40
/// swordsman (user 1, sturdy) at (6.5, 6.5).
fn stage(wall: bool) -> HubInstance {
    let rows: Vec<String> = (0..13)
        .map(|r| {
            (0..40)
                .map(|c| {
                    let border = r == 0 || c == 0 || r == 12 || c == 39;
                    if border || (wall && c == 20 && (2..=10).contains(&r)) { '#' } else { '.' }
                })
                .collect()
        })
        .collect();
    let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &refs, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let kind = |id: &str, b: Behaviour, skills: &[&str]| MonsterDef {
        behaviour: b,
        skills: skills.iter().map(|s| s.to_string()).collect(),
        ..test_def(id, 0.01, 50, 0.0, false)
    };
    let catalog = Arc::new(
        MonsterCatalog::new(vec![
            kind("archer", Behaviour::Kiter, &["shot"]),
            kind("priest", Behaviour::Healer, &["mend"]),
            kind("lady", Behaviour::Support, &["cry"]),
            kind("grunt", Behaviour::Chaser, &[]),
        ])
        .unwrap(),
    );
    let cfg = HordeConfig { light_radius: 30.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let book = SkillBook::from_json(BOOK).unwrap();
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(Arc::new(book));
    stage.dice = combat::Rng::new(5);
    let mut out = Vec::new();
    stage.join(PlayerProfile { archetype: "swordsman".into(), level: 40, ..profile(1) }, Uuid::nil(), None, 0, &mut out);
    let i = stage.find(1).unwrap();
    (stage.players[i].x, stage.players[i].y) = (6.5, 6.5);
    let f = stage.players[i].fighter.as_mut().unwrap();
    (f.max_hp, f.hp) = (1_000_000, 1_000_000);
    stage
}

fn spawn(stage: &mut HubInstance, id: &str, x: f32, y: f32) -> u16 {
    let horde = stage.horde_mut().unwrap();
    let kind = horde.catalog().kind_of(id).unwrap();
    horde.spawn(kind, x, y).unwrap()
}

/// A `CAST` the player saw: caster, skill, telegraph looks, aim x, y.
type Seen = (u32, String, Vec<u8>, f32, f32);

/// Every `CAST` the player saw over `ticks`, and every hit number.
fn run(stage: &mut HubInstance, ticks: u32) -> (Vec<Seen>, Vec<HitEvent>) {
    let mut out = Vec::new();
    for _ in 0..ticks {
        stage.tick(&mut out);
    }
    let mut casts = Vec::new();
    let mut hits = Vec::new();
    for (u, e) in &out {
        if *u != 1 {
            continue;
        }
        match e {
            HubEvent::Control(ServerMsg::Cast { caster, skill, areas, x, y, .. }) if !skill.is_empty() => {
                casts.push((*caster, skill.clone(), areas.iter().map(|a| a.look).collect(), *x, *y))
            }
            HubEvent::Control(ServerMsg::Combat { hits: h, .. }) => hits.extend(h.iter().cloned()),
            _ => {}
        }
    }
    (casts, hits)
}

#[test]
fn a_kiter_shoots_only_what_it_sees() {
    // In the open, 10 cells off: it looses its shot.
    let mut open = stage(false);
    let archer = spawn(&mut open, "archer", 16.5, 6.5);
    let (casts, _) = run(&mut open, 80);
    let me = MONSTER_ENTITY_BASE + archer as u32;
    assert!(casts.iter().filter(|c| c.0 == me && c.1 == "shot").count() >= 2, "it shoots: {casts:?}");
    // Behind a wall, as close: nothing (it would walk round it; this one
    // is rooted in place by its speed).
    let mut walled = stage(true);
    let archer = spawn(&mut walled, "archer", 26.5, 6.5);
    let (casts, _) = run(&mut walled, 80);
    let me = MONSTER_ENTITY_BASE + archer as u32;
    assert!(!casts.iter().any(|c| c.0 == me), "no shot through a wall: {casts:?}");
}

#[test]
fn a_pack_of_kiters_keeps_three_shots_in_the_air_at_most() {
    let mut stage = stage(false);
    for i in 0..8 {
        spawn(&mut stage, "archer", 15.5 + (i % 4) as f32, 4.5 + (i / 4) as f32 * 4.0);
    }
    let mut out = Vec::new();
    let mut most = 0;
    for _ in 0..200 {
        stage.tick(&mut out);
        let st = stage.horde.as_ref().unwrap().store();
        let flying = stage.mob_casts.iter().filter(|c| st.behaviour[c.slot as usize] == Behaviour::Kiter as u8).count();
        most = most.max(flying);
    }
    assert_eq!(most, 3, "a solo player faces three volleys at once at most");
}

#[test]
fn a_healer_beams_the_most_wounded_monster_in_range() {
    let mut stage = stage(false);
    let priest = spawn(&mut stage, "priest", 20.5, 6.5);
    let hurt = spawn(&mut stage, "grunt", 23.5, 6.5);
    let scratched = spawn(&mut stage, "grunt", 20.5, 9.5);
    let far = spawn(&mut stage, "grunt", 34.5, 6.5);
    let max = stage.horde_mut().unwrap().block(hurt).hp;
    let horde = stage.horde_mut().unwrap();
    horde.store_mut().hp[hurt as usize] = max / 4;
    horde.store_mut().hp[scratched as usize] = max * 7 / 10;
    horde.store_mut().hp[far as usize] = 1;
    let (casts, hits) = run(&mut stage, 60);
    let me = MONSTER_ENTITY_BASE + priest as u32;
    let beam = casts.iter().find(|c| c.0 == me).expect("it beams");
    assert_eq!((beam.1.as_str(), beam.2.as_slice()), ("mend", &[look::HEAL][..]), "a heal-coloured beam");
    assert!((beam.3 - 23.5).abs() < 0.6 && (beam.4 - 6.5).abs() < 0.6, "at the most wounded in reach, not the one out of it: {beam:?}");
    let healed = hits.iter().filter(|h| h.flags & hit::HEAL != 0 && h.target == MONSTER_ENTITY_BASE + hurt as u32).count();
    assert!(healed >= 1, "its patient is healed");
    assert!(stage.horde.as_ref().unwrap().store().hp[hurt as usize] > max / 4);
    // Nobody wounded: no beam.
    let mut calm = super::tactics_tests::stage(false);
    let priest = spawn(&mut calm, "priest", 20.5, 6.5);
    spawn(&mut calm, "grunt", 23.5, 6.5);
    let (casts, _) = run(&mut calm, 60);
    assert!(!casts.iter().any(|c| c.0 == MONSTER_ENTITY_BASE + priest as u32), "nothing to mend");
}

#[test]
fn a_support_rallies_only_over_a_pack() {
    let mut alone = stage(false);
    let lady = spawn(&mut alone, "lady", 20.5, 6.5);
    let (casts, _) = run(&mut alone, 80);
    assert!(!casts.iter().any(|c| c.0 == MONSTER_ENTITY_BASE + lady as u32), "no cry over nobody");
    let mut pack = stage(false);
    // 14 cells from the player: out of the cry's own range (3 m), inside
    // a support's trigger.
    let lady = spawn(&mut pack, "lady", 20.5, 6.5);
    let a = spawn(&mut pack, "grunt", 21.5, 6.5);
    spawn(&mut pack, "grunt", 20.5, 7.5);
    let (casts, _) = run(&mut pack, 80);
    assert!(casts.iter().any(|c| c.0 == MONSTER_ENTITY_BASE + lady as u32 && c.1 == "cry"), "a cry over the pack: {casts:?}");
    assert!(pack.horde.as_ref().unwrap().store().rally_of(a).ticks > 0, "the pack is spurred");
}
