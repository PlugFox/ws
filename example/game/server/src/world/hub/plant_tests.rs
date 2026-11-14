//! Plants on a stage (COMBAT.md §4.10): every hit takes one HP, no
//! condition or push takes, automatic targeting leaves them alone, a cut
//! plant drops its herbs and is no kill.

use super::fight::Fighter;
use super::test_support::{map, portal};
use super::tests::profile;
use super::*;
use crate::world::combat::loot::Drop;
use crate::world::combat::skills::tests::book;
use crate::world::combat::{self, Condition, Damage, DamageType, Dice, Loadout};
use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog, MonsterDef};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{hit, HitEvent, MONSTER_ENTITY_BASE};
use std::sync::Arc;
use uuid::Uuid;

/// Kind indices of [`stage`]'s catalogue (sorted by id).
const BLOB: u8 = 0;
const HERB: u8 = 1;
/// Hits a herb takes.
const HITS: u16 = 5;

/// A 20 × 9 field with a blob kind and a herb plant (felled in 5 hits,
/// always dropping a Green Herb); player 1, a level-10 `archetype`, joins
/// with `skills` slotted in order.
fn stage(archetype: &str, skills: &[(&str, u8)]) -> HubInstance {
    let row = format!("#{}#", ".".repeat(20));
    let wall = "#".repeat(22);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 9));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let herb = MonsterDef { drops: vec![Drop::new("green_herb", 100.0, None).unwrap()], ..test_plant("herb", (HITS, HITS)) };
    let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 2.0, 1, 0.0, false), herb]).unwrap());
    assert!(catalog.get(HERB).is_plant());
    let cfg = HordeConfig { light_radius: 12.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(Arc::new(book()));
    stage.dice = combat::Rng::new(5);
    let mut loadout = Loadout::default();
    loadout.hotbar.resize(combat::skills::SLOTS, String::new());
    for (i, (id, lvl)) in skills.iter().enumerate() {
        loadout.skills.insert(id.to_string(), *lvl);
        loadout.hotbar[i] = id.to_string();
    }
    let mut out = Vec::new();
    stage.join(PlayerProfile { archetype: archetype.into(), level: 10, loadout, ..profile(1) }, Uuid::nil(), None, 0, &mut out);
    stage
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

/// Every hit event on `slot`.
fn hits_on(out: &Outgoing, slot: u16) -> Vec<HitEvent> {
    controls(out)
        .into_iter()
        .filter_map(|m| match m {
            ServerMsg::Combat { hits, .. } => Some(hits),
            _ => None,
        })
        .flatten()
        .filter(|h| h.target == MONSTER_ENTITY_BASE + slot as u32)
        .collect()
}

/// Entities player 1 swung at.
fn swung_at(out: &Outgoing) -> Vec<u32> {
    controls(out)
        .into_iter()
        .filter_map(|m| match m {
            ServerMsg::Combat { swings, .. } => Some(swings),
            _ => None,
        })
        .flatten()
        .filter(|s| s.entity_id == 101)
        .map(|s| s.target)
        .collect()
}

fn me(stage: &HubInstance) -> &Fighter {
    stage.players[stage.find(1).unwrap()].fighter.as_ref().unwrap()
}

fn run(stage: &mut HubInstance, ticks: usize, out: &mut Outgoing, until: impl Fn(&HubInstance) -> bool) {
    for _ in 0..ticks {
        stage.tick(out);
        if until(stage) {
            break;
        }
    }
}

#[test]
fn every_blow_takes_one_hp_and_the_herb_drops() {
    let mut stage = stage("swordsman", &[]);
    let (_, px, py) = stage.position_of(1).unwrap();
    let plant = stage.horde_mut().unwrap().spawn(HERB, px + 1.0, py).unwrap();
    assert_eq!(stage.horde().unwrap().store().hp[plant as usize], HITS as i32);
    let (exp, kills) = (me(&stage).exp, stage.kills);
    let mut out = Vec::new();
    // No cursor (touch): with no monster in reach the swing goes for the plant.
    run(&mut stage, 400, &mut out, |s| !s.horde().unwrap().store().is_alive(plant));
    assert!(!stage.horde().unwrap().store().is_alive(plant), "cut down");
    let landed: Vec<HitEvent> = hits_on(&out, plant).into_iter().filter(|h| h.flags & hit::MISS == 0).collect();
    assert_eq!(landed.len(), HITS as usize, "{landed:?}");
    assert!(landed.iter().all(|h| h.amount == 1 && h.flags & hit::CRITICAL == 0), "one HP a blow, never a crit: {landed:?}");
    assert!(landed.last().unwrap().flags & hit::KILL != 0);
    assert!(
        controls(&out).iter().any(|m| matches!(m, ServerMsg::Loot { item, count: 1, .. } if item == "green_herb")),
        "its herb drops for the cutter"
    );
    assert_eq!((me(&stage).exp, me(&stage).combo().count(), stage.kills), (exp, 0, kills), "no exp, no combo, no kill");
}

#[test]
fn any_damage_is_one_hp_and_nothing_else_takes() {
    let mut stage = stage("mage", &[]);
    let (_, px, py) = stage.position_of(1).unwrap();
    let horde = stage.horde_mut().unwrap();
    let plant = horde.spawn(HERB, px + 3.0, py).unwrap();
    let mut rng = combat::Rng::new(9);
    for kind in [DamageType::Physical, DamageType::Fire, DamageType::Poison, DamageType::Radiant] {
        let blow = Damage::new(Dice::new(10, 10, 50), kind);
        assert_eq!(super::cast::wound(horde, &mut rng, plant, blow, Some(Dice::new(4, 6, 0)), true, false, 3), 1);
    }
    assert_eq!(horde.store().hp[plant as usize], HITS as i32 - 4);
    for c in Condition::ALL {
        horde.store_mut().afflict(plant, c, 100);
    }
    horde.flinch(plant);
    let at = (horde.store().x[plant as usize], horde.store().y[plant as usize]);
    horde.place(plant, at.0 + 2.0, at.1);
    let store = horde.store();
    assert_eq!(store.status_of(plant).0, 0, "no condition takes");
    assert!(!store.staggered(plant));
    assert_eq!((store.x[plant as usize], store.y[plant as usize]), at, "rooted");
}

#[test]
fn burning_flames_and_a_knockback_cut_a_plant_for_one_a_hit() {
    let mut stage = stage("mage", &[("flames", 10), ("pull", 1)]);
    let (_, px, py) = stage.position_of(1).unwrap();
    let plant = stage.horde_mut().unwrap().spawn(HERB, px + 3.0, py).unwrap();
    let at = {
        let s = stage.horde().unwrap().store();
        (s.x[plant as usize], s.y[plant as usize])
    };
    let mut out = Vec::new();
    // Flames aimed onto the plant (3 m east = 6 cells... aim 1.5 m: 3 cells).
    stage.act(1, [12, 0], 1);
    run(&mut stage, 60, &mut out, |_| false);
    stage.act(1, [12, 0], 2); // the pull, around the caster
    run(&mut stage, 20, &mut out, |_| false);
    let landed: Vec<HitEvent> = hits_on(&out, plant).into_iter().filter(|h| h.flags & hit::MISS == 0).collect();
    assert!(!landed.is_empty(), "the flames reach it");
    assert!(landed.iter().all(|h| h.amount == 1), "{landed:?}");
    let s = stage.horde().unwrap().store();
    if s.is_alive(plant) {
        assert_eq!(s.hp[plant as usize], HITS as i32 - landed.len() as i32);
        assert_eq!(s.status_of(plant).0, 0, "never burning");
        assert_eq!((s.x[plant as usize], s.y[plant as usize]), at, "no knockback, no pull");
    }
}

#[test]
fn automatic_targeting_leaves_plants_alone() {
    let mut stage = stage("mage", &[("bolt", 3)]);
    let (_, px, py) = stage.position_of(1).unwrap();
    let plant = stage.horde_mut().unwrap().spawn(HERB, px + 1.0, py).unwrap();
    let mut out = Vec::new();
    // A bolt (nearest) with only a plant in range: nothing to cast at.
    let sp = me(&stage).sp;
    stage.act(1, [0, 0], 1);
    stage.tick(&mut out);
    assert!(!controls(&out).iter().any(|m| matches!(m, ServerMsg::Cast { caster: 101, skill, .. } if skill == "bolt")));
    assert_eq!(me(&stage).sp, sp, "nothing spent");
    // The cursor elsewhere (a mouse resting on the ground): no swing at it.
    out.clear();
    stage.act(1, [-16, 8], 0);
    run(&mut stage, 60, &mut out, |_| false);
    assert!(swung_at(&out).is_empty(), "{:?}", swung_at(&out));
    // A monster in reach is always first, even behind the plant.
    let blob = stage.horde_mut().unwrap().spawn(BLOB, px + 3.0, py).unwrap();
    stage.horde_mut().unwrap().store_mut().hp[blob as usize] = 100_000;
    stage.act(1, [4, 0], 0); // the cursor on the plant
    out.clear();
    run(&mut stage, 60, &mut out, |_| false);
    let targets = swung_at(&out);
    assert!(!targets.is_empty() && targets.iter().all(|&t| t == MONSTER_ENTITY_BASE + blob as u32), "{targets:?}");
    // With the monster gone, the cursor on the plant cuts it.
    stage.horde_mut().unwrap().despawn(blob);
    out.clear();
    run(&mut stage, 60, &mut out, |_| false);
    assert!(swung_at(&out).contains(&(MONSTER_ENTITY_BASE + plant as u32)), "pointed at, it is cut");
}
