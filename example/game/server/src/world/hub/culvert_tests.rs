//! Sewage meets spells (`culvert.rs`), and the culvert's mission data.

use super::super::env::Env;
use super::super::test_support::map;
use super::super::{HubEvent, HubSimConfig, HubWorld, Outgoing, PlayerProfile};
use super::*;
use crate::world::combat;
use crate::world::environment::{Environment, Hazard};
use crate::world::horde::catalog::{test_def, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::missions::{self, MissionInfo};
use crate::world::protocol::Look;
use std::sync::Arc;
use uuid::Uuid;

const BANK: Hazard = Hazard { look: "sludge", damage: [1, 4], condition: Some("slowed"), ..Hazard::circle("bank", 10.5, 10.5, 4.0) };

static MISSION: MissionInfo = MissionInfo {
    id: "sewer_test",
    name: "Sewer",
    map: "field",
    fog: None,
    env: Environment { light_scale: 1.0, hazards: &[BANK], events: &[], objects: &[] },
    ..MissionInfo::TEMPLATE
};

fn stage() -> HubInstance {
    let wall = "#".repeat(32);
    let row = format!("#{}#", ".".repeat(30));
    let mut rows = vec![wall.clone()];
    rows.extend(std::iter::repeat_n(row, 20));
    rows.push(wall);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let world = Arc::new(HubWorld::new(vec![map("field", LocationKind::Field, camera, &rows, vec![])], "field").unwrap());
    let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 2.0, 1, 0.0, false)]).unwrap());
    let cfg = HordeConfig { light_radius: 12.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, MISSION.id, world, HubSimConfig { residents: 0, ..Default::default() }).with_horde(horde);
    stage.env = Env::new(Some(&MISSION), &[]);
    stage.dice = combat::Rng::new(3);
    let mut out = Vec::new();
    let profile = PlayerProfile {
        user_id: 1,
        character_id: 101,
        name: "p1".into(),
        archetype: "swordsman".into(),
        look: Look::default(),
        level: 20,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
    };
    stage.join(profile, Uuid::nil(), None, 0, &mut out);
    let p = &mut stage.players[0];
    (p.x, p.y) = (10.5, 10.5);
    let f = p.fighter.as_mut().unwrap();
    (f.max_hp, f.hp) = (100_000, 100_000);
    f.recovery = u32::MAX;
    stage
}

fn hp(stage: &HubInstance) -> i32 {
    stage.players[0].fighter.as_ref().unwrap().hp
}

fn run(stage: &mut HubInstance, from: f32, secs: f32) -> Outgoing {
    let mut out = Vec::new();
    let mut t = from;
    while t < from + secs {
        stage.horde_mut().unwrap().set_director_elapsed(t);
        stage.tick(&mut out);
        t += 0.05;
    }
    out
}

#[test]
fn cold_freezes_sewage_over_and_fire_only_steams() {
    let mut stage = stage();
    run(&mut stage, 0.0, 3.0);
    let before = hp(&stage);
    assert!(before < 100_000, "the bank bites");
    assert!(stage.env.ground_is(10.5, 10.5, SEWAGE) && !stage.env.ground_is(25.5, 10.5, SEWAGE));
    // A Frost Diver on the bank: the ice spares whoever stands on it.
    let mut msgs = Vec::new();
    stage.sewage_meets(0, &[(DamageType::Cold, Area::Circle { x: 10.5, y: 10.5, r: 1.0 })], &mut msgs);
    let ice = msgs.iter().find_map(|m| match m {
        ServerMsg::BossZones { caster: 0, ability, zones } if ability == "culvert_ice" => Some(zones[0]),
        _ => None,
    });
    let ice = ice.expect("the ice is shown");
    assert_eq!((ice.flags, ice.area.look, ice.area.span_ms, ice.area.a), (zone_flags::SAFE, look::COLD, 10_000, 2.5));
    let bites = |out: &Outgoing| {
        out.iter().any(|(_, e)| matches!(e, HubEvent::Control(ServerMsg::Combat { hits, .. }) if hits.iter().any(|h| h.target == 101 && h.amount > 0)))
    };
    assert!(!bites(&run(&mut stage, 3.0, 8.0)), "no bite while it is frozen");
    assert!(bites(&run(&mut stage, 11.0, 4.0)), "thawed, it bites again");
    // Fire on sewage: steam, no calm.
    let mut msgs = Vec::new();
    stage.sewage_meets(0, &[(DamageType::Fire, Area::Circle { x: 10.5, y: 10.5, r: 2.0 })], &mut msgs);
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossZones { ability, .. } if ability == "culvert_steam")));
    assert!(bites(&run(&mut stage, 15.0, 3.0)), "steam calms nothing");
}

#[test]
fn the_culvert_mission_is_dark_flushes_in_turn_and_hatches_its_clusters_in_order() {
    let m = missions::find("prontera_culvert").unwrap();
    assert!((m.env.light_scale - 0.85).abs() < 1e-6);
    let flush: Vec<_> = m.env.events.iter().map(|e| (e.id, e.first_secs, e.every_secs)).collect();
    assert_eq!(flush, vec![("flush_west", 150.0, 360.0), ("flush_east", 330.0, 360.0)]);
    let hatch: Vec<f32> = m.env.objects.iter().map(|o| o.hatch.unwrap().secs).collect();
    assert!(hatch.windows(2).all(|w| w[1] > w[0]) && hatch[0] >= 120.0, "{hatch:?}");
    assert!(m.env.objects.iter().all(|o| o.monster == "thief_bug_egg" && o.spots.len() == 6 && o.hatch.unwrap().monster == "thief_bug_larva"));
}

#[test]
fn the_baked_culvert_loads_with_its_hazards_and_opens() {
    use crate::world::horde::catalog::MonsterCatalog;
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    if !dist.join("server/locations/prt_sewb1").exists() {
        return; // no baked pack here
    }
    let world = HubWorld::load(&dist, &["prt_sewb1".into()], "prt_sewb1").expect("prt_sewb1 loads");
    let hazards = &world.map(world.entry()).hazards;
    assert!(hazards.iter().filter(|h| h.id.starts_with("bank_")).count() > 50, "the banks are baked");
    assert!(hazards.iter().any(|h| h.during == Some("flush_west")) && hazards.iter().any(|h| h.during == Some("flush_east")));
    let all = MonsterCatalog::load(&dist).expect("monsters.json loads");
    let m = missions::find("prontera_culvert").unwrap();
    let roster = missions::roster(m, &all);
    for id in ["thief_bug_larva", "thief_bug", "tarou", "golden_bug", "thief_bug_queen", "rat_king", "thief_bug_egg"] {
        assert!(roster.kind_of(id).is_some(), "{id} is in the culvert's roster");
    }
    assert!(roster.kind_of("poring").is_none(), "no slime from the island");
    let card = missions::card(1, m, Some(&roster), true);
    assert_eq!(card.available, missions::Availability::Open, "{}", card.reason);
}
