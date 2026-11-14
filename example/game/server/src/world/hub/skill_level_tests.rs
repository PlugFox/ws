//! Every skill of the baked pack, at every level (COMBAT.md §3, §5):
//!
//! - each level up is worth taking: level n + 1 is better than n on at
//!   least one number (by 3 % or more) and worse on none (SP may rise);
//! - the indicators the client draws (`dist/client/skills/tree.json`
//!   `indicator`, baked by pipeline/skill_trees.py) have the shape and size
//!   of the areas the server sends and strikes ([`cast::telegraphs`]).
//!
//! The tests skip when the asset pack is not baked.

use super::aim::Aimed;
use super::cast::{scaled, telegraphs};
use super::test_support::{map, portal};
use super::*;
use crate::world::combat::skills::{BuffFx, Effect, EffectDice, Payload, SkillBook, SkillDef};
use crate::world::combat::Dice;
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::Look;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

fn dist() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist")
}

/// Character levels the dice are checked at: every cantrip k from 1 to 7.
const CHARACTER_LEVELS: [u32; 7] = [1, 15, 30, 45, 60, 75, 90];
/// A level up must raise some number by at least this fraction.
const MEANINGFUL: f32 = 0.03;

fn avg(d: Dice) -> f32 {
    d.count as f32 * (d.sides as f32 + 1.0) / 2.0 + d.bonus as f32
}

/// The numbers of a buff at `slv`, higher is better.
fn buff_numbers(b: &BuffFx, slv: u8, cl: u32, out: &mut Vec<(String, f32)>, p: &str) {
    let m = b.mods(slv);
    let mut put = |k: &str, v: f32| out.push((format!("{p}{k}"), v));
    put("secs", b.secs.at(slv));
    put("ac", m.ac as f32);
    put("to_hit", m.to_hit as f32);
    put("to_hit_die", m.to_hit_die as f32);
    put("dice", m.damage.map_or(0.0, avg));
    put("damage_flat", m.damage_flat as f32);
    put("haste", m.haste);
    put("speed", m.speed);
    put("resist", m.resist);
    put("reduce", m.reduce as f32);
    put("block", m.block as f32);
    put("light", m.light);
    put("sp_regen", m.sp_regen);
    put("hp_regen", m.hp_regen);
    put("crit", m.crit as f32);
    put("evil_resist", m.evil_resist);
    put("evil_damage", m.evil_damage as f32);
    put("soul_drain", m.soul_drain as f32);
    put("double", m.double as f32);
    put("venom", m.venom as f32);
    put("loot", m.loot);
    put("amplify", m.amplify);
    put("temp_hp", b.temp_hp.map_or(0.0, |d| avg(scaled(d, b.scale_k, b.mult.at(slv), 0.0, cl))));
    put("shield", b.shield.at(slv));
}

/// Every number of a skill at `slv` for a character of level `cl`, as the
/// server resolves it (rounded dice, `Lin::at`), oriented so that higher
/// is better. SP is left out: it may rise with the level.
fn numbers(d: &SkillDef, slv: u8, cl: u32) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    if !d.passive {
        out.push(("cast".into(), -d.cast_secs(slv)));
        out.push(("cooldown".into(), -d.cooldown_secs(slv)));
    }
    for (i, e) in d.effects.iter().enumerate() {
        let p = format!("{i}.");
        let put = |k: &str, v: f32, out: &mut Vec<(String, f32)>| out.push((format!("{p}{k}"), v));
        match e {
            Effect::Move { distance, .. } => put("distance", distance.at(slv), &mut out),
            Effect::Passive(b) => buff_numbers(b, slv, cl, &mut out, &p),
            Effect::Act(a) => {
                let (area, t) = (&a.area, &a.timing);
                put("radius", area.radius.at(slv), &mut out);
                put("length", area.length.at(slv), &mut out);
                put("width", area.width.at(slv), &mut out);
                put("limit", area.limit.at(slv), &mut out);
                put("chain", area.chain.at(slv).round(), &mut out);
                put("count", t.count.at(slv).round().max(1.0), &mut out);
                put("ground", t.secs.at(slv), &mut out);
                match &a.payload {
                    Payload::Damage(fx) => {
                        let extra = fx.extra.map_or(0.0, |e| {
                            let n = (e.count as f32 * fx.mult.at(slv)).round().max(0.0) as u32;
                            avg(Dice { count: n, ..e })
                        });
                        let base = match fx.dice {
                            EffectDice::Fixed(dice) => avg(scaled(dice, fx.scale_k, fx.mult.at(slv), fx.bonus.at(slv), cl)),
                            EffectDice::Weapon => fx.bonus.at(slv).round(),
                        };
                        put("damage", base + extra, &mut out);
                        put("knockback", fx.knockback.at(slv).abs(), &mut out);
                        put("save", fx.save.map_or(0.0, |s| s.secs.at(slv)), &mut out);
                    }
                    Payload::Heal(h) => put("heal", avg(scaled(h.dice, h.scale_k, h.mult.at(slv), 0.0, cl)), &mut out),
                    Payload::Condition { secs, .. } => put("condition", secs.at(slv), &mut out),
                    Payload::Push(dist) => put("push", dist.at(slv).abs(), &mut out),
                    Payload::Buff(b) => buff_numbers(b, slv, cl, &mut out, &p),
                    Payload::Ward { hits, .. } => put("hits", hits.at(slv).round().max(1.0), &mut out),
                    Payload::Revive { hp } => put("revive", hp.at(slv), &mut out),
                    Payload::Barrier | Payload::Cure | Payload::Dispel => {}
                }
            }
            Effect::Mount | Effect::Pending(_) => {}
        }
    }
    out
}

/// Every level of every skill is worth its point (the owner's report:
/// "+1d6 every 2 levels" left every other level dead), and none takes
/// anything away. Checked at every cantrip k, since dice counts round
/// after the k.
#[test]
fn every_skill_level_is_an_upgrade() {
    let Ok(book) = SkillBook::load(&dist()) else { return };
    let mut bad = Vec::new();
    for (_, d) in book.iter().filter(|(_, d)| d.ready() && d.max_level > 1) {
        for slv in 1..d.max_level {
            for cl in CHARACTER_LEVELS {
                let (a, b) = (numbers(d, slv, cl), numbers(d, slv + 1, cl));
                let worse: Vec<&str> = a.iter().zip(&b).filter(|((_, x), (_, y))| *y < *x - 1e-4).map(|((k, _), _)| k.as_str()).collect();
                let gain = a.iter().zip(&b).map(|((_, x), (_, y))| if x.abs() < 1e-6 { if *y > 1e-6 { 1.0 } else { 0.0 } } else { (y - x) / x.abs() }).fold(0.0, f32::max);
                if !worse.is_empty() || gain < MEANINGFUL {
                    bad.push(format!("{} {slv}->{} (character level {cl}): worse {worse:?}, best gain {:.1}%", d.id, slv + 1, gain * 100.0));
                    break;
                }
            }
        }
    }
    assert!(bad.is_empty(), "level ups that give nothing or take something:\n{}", bad.join("\n"));
}

/// Ground looks in wire order (protocol.rs `look`, telegraph.ts `LOOKS`).
const LOOKS: [&str; 16] =
    ["damage", "heal", "fire", "swamp", "ward", "cold", "lightning", "holy", "trap", "taunt", "barrier", "buff", "earth", "curse", "poison", "dark"];

/// One player on an open field (the caster of the telegraphs).
fn caster() -> HubInstance {
    let w = 60;
    let row = format!("#{}#", ".".repeat(w - 2));
    let wall = "#".repeat(w);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 38));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default());
    let profile = PlayerProfile {
        user_id: 1,
        character_id: 101,
        name: "p1".into(),
        archetype: "mage".into(),
        look: Look::default(),
        level: 60,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
    };
    stage.join(profile, Uuid::nil(), None, 0, &mut Vec::new());
    (stage.players[0].x, stage.players[0].y) = (30.0, 20.0);
    stage
}

/// The client's aim preview and cast indicator come from the same table
/// as the server's areas: for every skill at every level, the baked
/// `indicator` shapes are the telegraphs the server sends at the press
/// (which are the areas it strikes), in order, in kind and size.
#[test]
fn indicators_match_the_server_areas_at_every_level() {
    let (Ok(book), Ok(tree)) = (SkillBook::load(&dist()), std::fs::read_to_string(dist().join("client/skills/tree.json"))) else { return };
    let tree: serde_json::Value = serde_json::from_str(&tree).unwrap();
    let stage = caster();
    let me = &stage.players[0];
    // Aimed 3 m east of the caster.
    let aimed = Aimed { x: me.x + 6.0, y: me.y, ox: me.x, oy: me.y, dir: (1.0, 0.0), ally: None, monster: None };
    let mut checked = 0;
    let mut bad = Vec::new();
    for s in tree["skills"].as_array().unwrap() {
        let id = s["id"].as_str().unwrap();
        let (_, def) = book.find(id).unwrap_or_else(|| panic!("{id} is in the client tree but not the server table"));
        let levels = s["indicator"].as_array().unwrap();
        if def.passive || !def.ready() {
            continue;
        }
        assert_eq!(levels.len(), def.max_level as usize, "{id}: one indicator per level");
        for (i, shapes) in levels.iter().enumerate() {
            let slv = i as u8 + 1;
            let sent = telegraphs(def, slv, me, &aimed, 1, &stage.players);
            let baked: Vec<&serde_json::Value> = shapes.as_array().unwrap().iter().filter(|a| a["kind"] != "move").collect();
            let moves = shapes.as_array().unwrap().iter().filter(|a| a["kind"] == "move").count();
            assert_eq!(moves, def.effects.iter().filter(|e| matches!(e, Effect::Move { .. })).count(), "{id}: every move is shown");
            if baked.len() != sent.len() {
                bad.push(format!("{id} {slv}: {} indicator areas, the server sends {}", baked.len(), sent.len()));
                continue;
            }
            for (b, t) in baked.iter().zip(&sent) {
                let (a, w) = (b["a"].as_f64().unwrap() as f32 * 2.0, b["b"].as_f64().unwrap() as f32);
                // Server shapes: 0 circle (a = radius), 1 box (a = length,
                // b = width; lines too), 2 cone (a = radius, b = degrees).
                // A point is drawn as a marker on the picked target (the
                // server sends it as a one-cell circle).
                let ok = match b["kind"].as_str().unwrap() {
                    "circle" => t.shape == 0 && (t.a - a).abs() < 1e-3,
                    "point" => t.shape == 0 && (t.a - 1.0).abs() < 1e-3,
                    "rect" | "line" => t.shape == 1 && (t.a - a).abs() < 1e-3 && (t.b - w * 2.0).abs() < 1e-3,
                    "arc" => t.shape == 2 && (t.a - a).abs() < 1e-3 && (t.b - w).abs() < 0.05,
                    _ => false,
                };
                // Lines start at the caster, boxes are centred on the aim.
                let placed = match b["kind"].as_str().unwrap() {
                    "line" => (t.x - (me.x + t.a / 2.0)).abs() < 1e-3,
                    "arc" => (t.x - me.x).abs() < 1e-3,
                    _ if b["at"] == "caster" => (t.x - me.x).abs() < 1e-3,
                    _ => (t.x - aimed.x).abs() < 1e-3,
                };
                // The tint: the look the server sends.
                let look = LOOKS.iter().position(|l| *l == b["look"].as_str().unwrap_or("")).map(|i| i as u8);
                if !ok || !placed || look != Some(t.look) {
                    bad.push(format!("{id} {slv}: indicator {b} vs sent shape {} a {} b {} at x {} look {}", t.shape, t.a, t.b, t.x - me.x, t.look));
                }
                checked += 1;
            }
        }
    }
    assert!(bad.is_empty(), "indicators that differ from the server areas:\n{}", bad.join("\n"));
    assert!(checked > 500, "{checked} areas checked");
}

/// Mystical Amplification: the next damaging spell gets the top rolls of
/// a tenth of its dice per level, and casting it spends the buff.
#[test]
fn mystical_amplification_empowers_the_next_spell_once() {
    use super::cast::amplified;
    use super::tests::profile;
    use crate::world::combat::{self, Loadout};
    use crate::world::horde::catalog::{test_def, MonsterCatalog};
    use crate::world::horde::{Horde, HordeConfig};
    // 4d8: each maxed die is 3.5 over its average.
    assert_eq!(amplified(Dice::new(4, 8, 0), 0.5), 7);
    assert_eq!(amplified(Dice::new(4, 8, 0), 1.0), 14);
    assert_eq!(amplified(Dice::new(4, 8, 0), 0.0), 0);
    let book = SkillBook::from_json(
        r#"{"skills":[
      {"id":"amp","name":"Amp","class":"mage","max_level":10,"target":"self","cooldown":15,
       "effects":[{"type":"buff","amplify":[0,0.1],"secs":10}]},
      {"id":"bolt","name":"Bolt","class":"mage","max_level":1,"target":"nearest","range":10,"cooldown":1,
       "effects":[{"type":"damage","dice":"4d8","damage":"fire"}]}
    ]}"#,
    )
    .unwrap();
    let mut stage = caster();
    let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 0.01, 50, 0.0, false)]).unwrap());
    let cfg = HordeConfig { light_radius: 12.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let world = Arc::clone(&stage.world);
    stage = HubInstance::stage(1, "m1", world.clone(), HubSimConfig::default())
        .with_horde(Horde::new(world, catalog, cfg))
        .with_skills(Arc::new(book));
    stage.dice = combat::Rng::new(5);
    let mut loadout = Loadout::default();
    loadout.hotbar.resize(combat::skills::SLOTS, String::new());
    for (i, (id, lvl)) in [("amp", 10u8), ("bolt", 1)].iter().enumerate() {
        loadout.skills.insert(id.to_string(), *lvl);
        loadout.hotbar[i] = id.to_string();
    }
    stage.join(PlayerProfile { archetype: "mage".into(), level: 30, loadout, ..profile(1) }, Uuid::nil(), None, 0, &mut Vec::new());
    (stage.players[0].x, stage.players[0].y) = (30.0, 20.0);
    let s = stage.horde_mut().unwrap().spawn(0, 34.0, 20.0).unwrap();
    stage.horde_mut().unwrap().store_mut().hp[s as usize] = 10_000;
    let amp = |stage: &HubInstance| stage.players[0].fighter.as_ref().unwrap().mods.amplify;
    let press = |stage: &mut HubInstance, slots: u16, n: u32| {
        stage.act(1, [0, 0], slots);
        for _ in 0..n {
            stage.tick(&mut Vec::new());
        }
    };
    press(&mut stage, 1, 3);
    assert!((amp(&stage) - 1.0).abs() < 1e-6, "level 10: all of the dice");
    press(&mut stage, 0, 40);
    press(&mut stage, 2, 3);
    assert_eq!(amp(&stage), 0.0, "the bolt spent it");
}
