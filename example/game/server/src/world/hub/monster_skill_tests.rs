//! Monster skill platform (ladder_60_99.md §3, S4–S6): corpse skills
//! (`on_death`), self-destructs (`sacrifice`), the `rally` payload with
//! its spur in the horde store, interruptible wind-ups and the Silenced
//! condition on monsters and players. The table below is made up for the
//! tests: big dice so a save for half still shows.

use super::fight::Fighter;
use super::test_support::{map, portal};
use super::tests::profile;
use super::*;
use crate::world::combat::{self, Condition, DamageType, Loadout, SkillBook};
use crate::world::horde::catalog::{test_def, MonsterCatalog, MonsterDef};
use crate::world::horde::storage::Rally;
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{hit, HitEvent, MONSTER_ENTITY_BASE};
use std::sync::Arc;
use uuid::Uuid;
use std::assert_matches;

const BOOK: &str = r#"{"skills":[
  {"id":"zap","name":"Zap","class":"basic","max_level":1,"target":"nearest","range":8,"cooldown":1,"sp":0,
   "effects":[{"type":"damage","dice":"1d4","damage":"lightning"}]}
],"monster_skills":[
  {"id":"pop","name":"Pop","target":"self","range":0,"cast":0.5,"cooldown":0,"on_death":true,
   "effects":[{"type":"damage","dice":"8d6","damage":"fire","radius":1.5,"at":"caster"}]},
  {"id":"boom","name":"Boom","target":"self","range":1.5,"cast":0.5,"cooldown":4,"sacrifice":true,
   "effects":[{"type":"damage","dice":"8d6","damage":"force","radius":1.5,"at":"caster"}]},
  {"id":"cry","name":"War Cry","target":"self","range":4,"cast":0.5,"cooldown":2,"interrupt":true,
   "effects":[{"type":"rally","radius":3,"at":"caster","secs":5,"to_hit":2,"speed":0.25,"resist":0.3,"heal":0.15,"kinds":["lady","grunt"]}]}
]}"#;

/// Kinds: 0 popper (pop), 1 bomber (boom, pop), 2 lady (cry), 3 grunt,
/// 4 other; all stand still. A level-10 thief (user 1) at (6.5, 5.5) with
/// Zap in the first slot, on a 20 × 9 field.
fn stage() -> HubInstance {
    let row = format!("#{}#", ".".repeat(20));
    let wall = "#".repeat(22);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 9));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let kind = |id: &str, skills: &[&str]| MonsterDef { skills: skills.iter().map(|s| s.to_string()).collect(), ..test_def(id, 0.01, 50, 0.0, false) };
    let catalog = Arc::new(
        MonsterCatalog::new(vec![kind("popper", &["pop"]), kind("bomber", &["boom", "pop"]), kind("lady", &["cry"]), kind("grunt", &[]), kind("other", &[])])
            .unwrap(),
    );
    let cfg = HordeConfig { light_radius: 12.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let book = SkillBook::from_json(BOOK).unwrap();
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(Arc::new(book));
    stage.dice = combat::Rng::new(5);
    let mut loadout = Loadout::default();
    loadout.hotbar.resize(combat::skills::SLOTS, Default::default());
    loadout.skills.insert("zap".into(), 1);
    loadout.hotbar[0] = "zap".into();
    let mut out = Vec::new();
    stage.join(PlayerProfile { archetype: "thief".into(), level: 10, loadout, ..profile(1) }, Uuid::nil(), None, 0, &mut out);
    let i = stage.find(1).unwrap();
    (stage.players[i].x, stage.players[i].y) = (6.5, 5.5);
    stage
}

fn fighter(stage: &mut HubInstance) -> &mut Fighter {
    let i = stage.find(1).unwrap();
    stage.players[i].fighter.as_mut().unwrap()
}

/// `kind`: 0 popper, 1 bomber, 2 lady, 3 grunt, 4 other (by id: the
/// catalog may order its kinds its own way).
fn spawn(stage: &mut HubInstance, kind: u8, x: f32, y: f32, hp: i32) -> u16 {
    let id = ["popper", "bomber", "lady", "grunt", "other"][kind as usize];
    let horde = stage.horde_mut().unwrap();
    let kind = horde.catalog().kind_of(id).unwrap();
    let s = horde.spawn(kind, x, y).unwrap();
    horde.store_mut().hp[s as usize] = hp;
    s
}

fn controls(out: &Outgoing) -> Vec<ServerMsg> {
    out.iter()
        .filter_map(|(u, e)| match (u, e) {
            (1, HubEvent::Control(m)) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

fn hits(out: &Outgoing) -> Vec<HitEvent> {
    controls(out)
        .into_iter()
        .flat_map(|m| match m {
            ServerMsg::Combat { hits, .. } => hits,
            _ => Vec::new(),
        })
        .collect()
}

fn typed(out: &Outgoing, target: u32, kind: DamageType) -> usize {
    hits(out).iter().filter(|h| h.target == target && h.amount > 0 && h.flags >> 4 == kind as u8 && h.flags & hit::HEAL == 0).count()
}

fn cast_of(out: &Outgoing, entity: u32, skill: &str) -> bool {
    controls(out).iter().any(|m| matches!(m, ServerMsg::Cast { caster, skill: s, .. } if *caster == entity && s == skill))
}

fn cancelled(out: &Outgoing, entity: u32) -> bool {
    controls(out).iter().any(|m| matches!(m, ServerMsg::Cast { caster, skill, .. } if *caster == entity && skill.is_empty()))
}

#[test]
fn the_skill_table_reads_the_new_monster_fields() {
    let book = SkillBook::from_json(BOOK).unwrap();
    let pop = book.monster(book.find_monster("pop").unwrap());
    let boom = book.monster(book.find_monster("boom").unwrap());
    let cry = book.monster(book.find_monster("cry").unwrap());
    assert!(pop.on_death && !pop.sacrifice && !pop.interrupt);
    assert!(boom.sacrifice && !boom.on_death);
    assert!(cry.interrupt);
    let Some(combat::skills::Payload::Rally(r)) = cry.effects[0].act().map(|a| &a.payload) else { panic!("a rally") };
    assert_eq!((r.secs, r.to_hit, r.speed, r.resist, r.heal, r.kinds.len()), (5.0, 2, 0.25, 0.3, 0.15, 2));
    // A rally belongs on monster skills only; one with nothing to do is refused.
    let player = r#"{"skills":[{"id":"x","name":"X","class":"basic","max_level":1,"target":"self","effects":[{"type":"rally","secs":5,"speed":0.2}]}]}"#;
    assert!(SkillBook::from_json(player).is_err());
    let empty = r#"{"skills":[],"monster_skills":[{"id":"x","name":"X","target":"self","range":2,"cooldown":3,"effects":[{"type":"rally","radius":2}]}]}"#;
    assert!(SkillBook::from_json(empty).is_err());
    let walking_corpse = r#"{"skills":[],"monster_skills":[{"id":"x","name":"X","target":"self","range":2,"cooldown":3,"on_death":true,
        "effects":[{"type":"move","mode":"dash","distance":2},{"type":"damage","dice":"1d4","radius":1,"at":"caster"}]}]}"#;
    assert!(SkillBook::from_json(walking_corpse).is_err());
    // Silenced is a condition the table knows.
    let hush = r#"{"skills":[],"monster_skills":[{"id":"x","name":"X","target":"nearest","range":4,"cooldown":3,
        "effects":[{"type":"condition","radius":1,"save":{"condition":"silenced","secs":2}}]}]}"#;
    let b = SkillBook::from_json(hush).unwrap();
    assert_matches!(b.monster(0).effects[0].act().unwrap().payload, combat::skills::Payload::Condition { condition: Condition::Silenced, save: true, .. });
}

#[test]
fn a_corpse_skill_never_winds_up_and_bursts_where_the_monster_died() {
    let mut s = stage();
    // A tough popper next to the thief: it is hit for a while and never casts.
    let popper = spawn(&mut s, 0, 7.3, 5.5, 100_000);
    let entity = MONSTER_ENTITY_BASE + popper as u32;
    let mut out = Vec::new();
    for _ in 0..200 {
        s.tick(&mut out);
    }
    assert!(!cast_of(&out, entity, "pop"), "an on_death skill is no wind-up");
    assert!(hits(&out).iter().any(|h| h.target == entity), "the thief is hitting it");
    // Now it dies to the next blow.
    s.horde_mut().unwrap().store_mut().hp[popper as usize] = 1;
    out.clear();
    let mut died = None;
    for t in 0..60 {
        s.tick(&mut out);
        if died.is_none() && hits(&out).iter().any(|h| h.target == entity && h.flags & hit::KILL != 0) {
            died = Some(t);
        }
    }
    assert!(died.is_some(), "the thief kills it");
    let zone = controls(&out).into_iter().find_map(|m| match m {
        ServerMsg::BossZones { caster, ability, zones } if caster == entity && ability == "pop" => Some(zones),
        _ => None,
    });
    let zones = zone.expect("the corpse telegraphs its burst");
    assert_eq!((zones.len(), zones[0].delay_ms, zones[0].area.a), (1, 500, 3.0), "1.5 m round the corpse, 0.5 s fuse");
    assert!((zones[0].area.x - 7.3).abs() < 0.5, "where it died");
    assert!(typed(&out, 101, DamageType::Fire) >= 1, "the burst lands on the thief standing by it");
    assert!(!s.horde().unwrap().store().is_alive(popper));
}

#[test]
fn a_sacrifice_dies_as_it_resolves_and_its_corpse_skill_follows() {
    let mut s = stage();
    let bomber = spawn(&mut s, 1, 7.8, 5.5, 100_000);
    let entity = MONSTER_ENTITY_BASE + bomber as u32;
    let kills = s.kills;
    let mut out = Vec::new();
    let mut started = false;
    for _ in 0..400 {
        s.tick(&mut out);
        started |= cast_of(&out, entity, "boom");
        if !s.horde().unwrap().store().is_alive(bomber) {
            break;
        }
    }
    assert!(started, "it winds up its blast");
    for _ in 0..30 {
        s.tick(&mut out);
    }
    assert!(!s.horde().unwrap().store().is_alive(bomber), "it blew itself up");
    let deaths = hits(&out).iter().filter(|h| h.target == entity && h.flags & hit::KILL != 0).count();
    assert!(deaths >= 1, "the clients see it die");
    assert!(typed(&out, 101, DamageType::Force) >= 1, "the blast hits the thief");
    assert_eq!(s.kills, kills + 1, "a kill through the funnel");
    assert!(
        controls(&out).iter().any(|m| matches!(m, ServerMsg::BossZones { caster, ability, .. } if *caster == entity && ability == "pop")),
        "its own corpse skill fires"
    );
}

#[test]
fn a_rally_heals_and_spurs_the_listed_kinds_and_its_spur_is_honoured() {
    let mut s = stage();
    let lady = spawn(&mut s, 2, 12.5, 5.5, 100_000);
    let grunt = spawn(&mut s, 3, 13.5, 5.5, 1);
    let other = spawn(&mut s, 4, 13.5, 6.5, 1);
    let (max, attack) = {
        let h = s.horde().unwrap();
        (h.block(grunt).hp, h.block(grunt).attack)
    };
    s.horde_mut().unwrap().store_mut().hp[grunt as usize] = max / 2;
    s.horde_mut().unwrap().store_mut().hp[other as usize] = max / 2;
    let entity = MONSTER_ENTITY_BASE + lady as u32;
    let mut out = Vec::new();
    for _ in 0..600 {
        s.tick(&mut out);
        if s.horde().unwrap().store().rally_of(grunt).ticks > 0 {
            break;
        }
    }
    assert!(cast_of(&out, entity, "cry"), "she winds up the cry");
    let r = s.horde().unwrap().store().rally_of(grunt);
    assert_eq!((r.to_hit, r.speed, r.taken), (2, 25, 30), "the grunt is spurred");
    let hz = s.config.tick_hz as u16;
    assert!(r.ticks + 2 >= 5 * hz && r.ticks <= 5 * hz, "for 5 s ({} ticks)", r.ticks);
    let h = s.horde().unwrap();
    assert_eq!(h.store().rally_of(lady).speed, 25, "and so is she");
    assert_eq!(h.store().rally_of(other), Rally::default(), "another kind is not");
    assert_eq!(h.store().hp[grunt as usize], max / 2 + (max as f32 * 0.15).round() as i32, "healed 15 %");
    assert_eq!(h.store().hp[other as usize], max / 2);
    assert_eq!(h.block(grunt).attack, attack + 2, "better attack rolls");
    assert!(hits(&out).iter().any(|e| e.target == MONSTER_ENTITY_BASE + grunt as u32 && e.flags == hit::HEAL), "the heal shows");
    // Damage taken: 30 % less while it lasts.
    let mut rng = combat::Rng::new(1);
    let mut s2 = stage();
    let a = spawn(&mut s2, 3, 15.0, 2.5, 10_000);
    let b = spawn(&mut s2, 3, 15.0, 8.5, 10_000);
    s2.horde_mut().unwrap().store_mut().rally(b, Rally { taken: 30, ticks: 100, ..Rally::default() });
    let mut rng2 = combat::Rng::new(1);
    let dmg = combat::Damage::new(combat::Dice::new(1, 1, 99), DamageType::Physical);
    let plain = super::cast::wound(s2.horde_mut().unwrap(), &mut rng, a, dmg, None, false, false, 0);
    let soaked = super::cast::wound(s2.horde_mut().unwrap(), &mut rng2, b, dmg, None, false, false, 0);
    assert_eq!((plain, soaked), (100, 70));
}

#[test]
fn a_flinch_calls_an_interruptible_wind_up_off() {
    let mut s = stage();
    let lady = spawn(&mut s, 2, 12.5, 5.5, 100_000);
    let grunt = spawn(&mut s, 3, 13.5, 5.5, 100);
    let entity = MONSTER_ENTITY_BASE + lady as u32;
    let mut out = Vec::new();
    let mut started = false;
    for _ in 0..600 {
        s.tick(&mut out);
        if cast_of(&out, entity, "cry") {
            started = true;
            break;
        }
    }
    assert!(started);
    s.horde_mut().unwrap().flinch(lady);
    out.clear();
    for _ in 0..3 {
        s.tick(&mut out);
    }
    assert!(cancelled(&out, entity), "the clients hear the cry is off");
    for _ in 0..20 {
        s.tick(&mut out);
    }
    assert_eq!(s.horde().unwrap().store().rally_of(grunt), Rally::default(), "nobody is spurred");
}

#[test]
fn silenced_monsters_cast_nothing_and_a_wind_up_under_way_ends() {
    let mut s = stage();
    let lady = spawn(&mut s, 2, 12.5, 5.5, 100_000);
    let entity = MONSTER_ENTITY_BASE + lady as u32;
    s.horde_mut().unwrap().store_mut().afflict(lady, Condition::Silenced, 400);
    let mut out = Vec::new();
    for _ in 0..350 {
        s.tick(&mut out);
    }
    assert!(!cast_of(&out, entity, "cry"), "silenced: no skills");
    // Free again it casts; silenced mid-wind-up it stops.
    let mut started = false;
    for _ in 0..600 {
        s.tick(&mut out);
        if cast_of(&out, entity, "cry") {
            started = true;
            break;
        }
    }
    assert!(started, "the silence ran out");
    s.horde_mut().unwrap().store_mut().afflict(lady, Condition::Silenced, 60);
    out.clear();
    for _ in 0..3 {
        s.tick(&mut out);
    }
    assert!(cancelled(&out, entity));
}

#[test]
fn a_silenced_player_casts_nothing_but_still_swings() {
    let mut s = stage();
    let blob = spawn(&mut s, 3, 7.3, 5.5, 100_000);
    let entity = MONSTER_ENTITY_BASE + blob as u32;
    fighter(&mut s).afflict(Condition::Silenced, 90);
    let mut out = Vec::new();
    s.act(1, [0, 0], 1);
    for _ in 0..60 {
        s.tick(&mut out);
    }
    assert!(!cast_of(&out, 101, "zap"), "no skill while silenced");
    assert_eq!(typed(&out, entity, DamageType::Lightning), 0);
    let swings = controls(&out)
        .iter()
        .map(|m| match m {
            ServerMsg::Combat { swings, .. } => swings.iter().filter(|w| w.entity_id == 101).count(),
            _ => 0,
        })
        .sum::<usize>();
    assert!(swings > 0, "basic attacks go on");
    assert!(fighter(&mut s).status.silenced());
    for _ in 0..60 {
        s.tick(&mut out);
    }
    out.clear();
    s.act(1, [0, 0], 1);
    for _ in 0..30 {
        s.tick(&mut out);
    }
    assert!(typed(&out, entity, DamageType::Lightning) >= 1 || cast_of(&out, 101, "zap"), "free again, it zaps");
}
