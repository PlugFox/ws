//! Orc Village's war camp on a small field (`orc.rs`): banners rally the
//! warband round them and show their reach, a horn marches a column in
//! from the side it names and replants the fallen banners, the Orc Lord
//! calls the clans; and the real mission entry on the real map.

use super::super::env::Env;
use super::super::test_support::map;
use super::super::{HubEvent, HubSimConfig, HubWorld, PlayerProfile};
use super::*;
use crate::world::combat;
use crate::world::environment::{Environment, MapEvent, ObjectDef};
use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog, MonsterDef};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::missions::{self, MissionInfo};
use crate::world::protocol::Look;
use std::sync::Arc;
use uuid::Uuid;

const TEST_HORN: MapEvent = MapEvent {
    id: HORN,
    name: "War Horn",
    warning: "",
    first_secs: 20.0,
    every_secs: 30.0,
    warn_secs: 5.0,
    duration_secs: 10.0,
    light_scale: 1.0,
    shake: 0,
};

static MISSION_TEST: MissionInfo = MissionInfo {
    id: MISSION,
    name: "Orc Village",
    map: "field",
    fog: None,
    env: Environment {
        light_scale: 1.0,
        hazards: &[],
        events: &[TEST_HORN],
        objects: &[ObjectDef { monster: BANNER, spots: &[(20.5, 20.5)], on_felled: ObjectEffect::Hook(BANNER_HOOK), hatch: None }],
        strikes: &[],
    },
    ..MissionInfo::TEMPLATE
};

/// A 100 × 100 open field: room for a column 36 cells off in any direction.
fn stage(players: u32) -> HubInstance {
    let wall = "#".repeat(100);
    let row = format!("#{}#", ".".repeat(98));
    let mut rows = vec![wall.clone()];
    rows.extend(std::iter::repeat_n(row, 98));
    rows.push(wall);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let elite = MonsterDef { archetype: crate::world::combat::Archetype::Elite, ..test_def("ork_hero", 0.5, 12, 0.0, false) };
    let catalog = Arc::new(
        MonsterCatalog::new(vec![
            test_def("ork_warrior", 0.5, 1, 0.0, false),
            test_def("orc_archer", 0.5, 1, 0.0, false),
            test_def("high_orc", 0.5, 4, 0.0, false),
            test_def("savage", 0.5, 2, 0.0, false),
            test_def("orc_lord", 0.5, 50, 0.0, true),
            elite,
            test_plant(BANNER, (5, 5)),
        ])
        .unwrap(),
    );
    let cfg = HordeConfig { light_radius: 20.0, director_enabled: false, seed: Some(2), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, MISSION_TEST.id, world, HubSimConfig { residents: 0, ..Default::default() }).with_horde(horde);
    stage.env = Env::new(Some(&MISSION_TEST), &[]);
    stage.dice = combat::Rng::new(5);
    for n in 0..players {
        let profile = PlayerProfile {
            user_id: 1 + n,
            character_id: 101 + n,
            name: format!("p{n}"),
            archetype: "swordsman".into(),
            look: Look::default(),
            level: 62,
            exp: 0,
            weapon: String::new(),
            loadout: Default::default(),
            bag: Default::default(),
            homunculus: None,
        };
        let mut out = Vec::new();
        stage.join(profile, Uuid::nil(), None, 0, &mut out);
        let i = stage.find(1 + n).unwrap();
        (stage.players[i].x, stage.players[i].y) = (50.5 + n as f32, 50.5);
        let f = stage.players[i].fighter.as_mut().unwrap();
        (f.max_hp, f.hp) = (1_000_000, 1_000_000);
    }
    stage
}

fn at(stage: &mut HubInstance, secs: f32, out: &mut Outgoing) {
    stage.horde_mut().unwrap().set_director_elapsed(secs);
    stage.tick(out);
}

fn msgs(out: &Outgoing) -> Vec<ServerMsg> {
    out.iter()
        .filter(|(u, _)| *u == 1)
        .filter_map(|(_, e)| match e {
            HubEvent::Control(m) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

fn kind(s: &HubInstance, id: &str) -> u8 {
    s.horde.as_ref().unwrap().catalog().kind_of(id).unwrap()
}

fn count(s: &HubInstance, id: &str) -> usize {
    let k = kind(s, id);
    s.horde.as_ref().unwrap().monsters().filter(|m| m.2 == k).count()
}

#[test]
fn a_banner_spurs_the_warband_round_it_and_shows_its_reach() {
    let mut s = stage(1);
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    let (w, sv, hero) = (kind(&s, "ork_warrior"), kind(&s, "savage"), kind(&s, "ork_hero"));
    let near = s.horde_mut().unwrap().spawn(w, 24.0, 20.5).unwrap();
    let boar = s.horde_mut().unwrap().spawn(sv, 22.0, 22.0).unwrap();
    let elite = s.horde_mut().unwrap().spawn(hero, 21.0, 19.0).unwrap();
    let far = s.horde_mut().unwrap().spawn(w, 20.5 + RALLY_R + 4.0, 20.5).unwrap();
    at(&mut s, 2.5, &mut out);
    let m = msgs(&out);
    let ring = m.iter().find_map(|m| match m {
        ServerMsg::BossZones { caster: 0, ability, zones } if ability == RALLY_ABILITY => Some(zones.clone()),
        _ => None,
    });
    let ring = ring.expect("the banner's reach shows");
    assert_eq!((ring.len(), ring[0].area.shape, ring[0].area.a), (1, 3, RALLY_R), "one ring at its reach");
    assert_eq!(s.env.orc.banners(), 1);
    let store = s.horde.as_ref().unwrap().store();
    assert_eq!(store.rally_of(near).to_hit, RALLY_HIT, "an orc under the banner is spurred");
    assert!((store.rally_of(near).pace() - (1.0 + RALLY_SPEED as f32 / 100.0)).abs() < 1e-4);
    assert_eq!(store.rally_of(boar).ticks, 0, "a boar is no orc");
    assert_eq!(store.rally_of(elite).ticks, 0, "elites fight on their own");
    assert_eq!(store.rally_of(far).ticks, 0, "out of its reach");

    // Felled: the ring goes, and the spur runs out soon after.
    let bk = kind(&s, BANNER);
    let b = s.horde.as_ref().unwrap().monsters().find(|m| m.2 == bk).unwrap().0;
    s.horde_mut().unwrap().despawn(b);
    let mut out = Vec::new();
    for t in 0..60 {
        at(&mut s, 3.0 + t as f32 * 0.05, &mut out);
    }
    assert!(msgs(&out).iter().any(|m| matches!(m, ServerMsg::ZoneEnd { .. })), "its ring ends");
    assert_eq!(s.env.orc.banners(), 0);
    assert_eq!(s.horde.as_ref().unwrap().store().rally_of(near).ticks, 0, "the spur is over");
}

#[test]
fn a_horn_marches_a_column_in_from_its_side_and_replants_the_banners() {
    let mut s = stage(2);
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    let bk = kind(&s, BANNER);
    let b = s.horde.as_ref().unwrap().monsters().find(|m| m.2 == bk).unwrap().0;
    s.horde_mut().unwrap().despawn(b);
    at(&mut s, 2.0, &mut out);
    assert_eq!(count(&s, BANNER), 0);
    let mut out = Vec::new();
    at(&mut s, TEST_HORN.first_secs + 0.1, &mut out);
    assert_eq!(s.env.orc.horns(), 1);
    assert_eq!(count(&s, BANNER), 1, "the fallen banner is replanted");
    let bodies = count(&s, "ork_warrior") + count(&s, "orc_archer");
    assert_eq!(bodies as u32, COLUMN_BASE + COLUMN_PER, "8 + 4 for the second player");
    assert!(count(&s, "orc_archer") >= 2, "archers behind the line");
    assert_eq!(count(&s, "high_orc"), 0, "no heavy before the third horn");
    // On the side the horn named, 30-36 cells from the party.
    let (dx, dy) = SIDES[horn_side(TEST_HORN.first_secs)];
    let (w, a) = (kind(&s, "ork_warrior"), kind(&s, "orc_archer"));
    for m in s.horde.as_ref().unwrap().monsters().filter(|m| m.2 == w || m.2 == a) {
        let (rx, ry) = (m.3 - 51.0, m.4 - 50.5);
        let along = rx * dx + ry * dy;
        assert!((COLUMN_NEAR - 3.0..=COLUMN_FAR + 6.0).contains(&along), "({}, {}) is {along} toward the side", m.3, m.4);
    }
    // The horn's later beats spawn nothing more; the third horn brings a heavy.
    let before = count(&s, "ork_warrior");
    at(&mut s, TEST_HORN.first_secs + 5.0, &mut out);
    assert_eq!(count(&s, "ork_warrior"), before);
    for n in 1..3 {
        // A tick to end the last horn, then one in the next.
        at(&mut s, TEST_HORN.first_secs + TEST_HORN.every_secs * n as f32 - 1.0, &mut out);
        at(&mut s, TEST_HORN.first_secs + TEST_HORN.every_secs * n as f32 + 0.1, &mut out);
    }
    assert_eq!(s.env.orc.horns(), 3);
    assert_eq!(count(&s, "high_orc"), 1, "a High Orc marches with the third column");
}

#[test]
fn every_horn_names_one_of_four_sides_and_they_vary() {
    let mut seen = [0; 4];
    for k in 0..40 {
        seen[horn_side(120.0 + 150.0 * k as f32)] += 1;
    }
    assert!(seen.iter().all(|&n| n > 0), "every side comes up: {seen:?}");
    assert_eq!(horn_side(120.0), horn_side(120.2), "by the start second");
}

#[test]
fn the_orc_lord_calls_the_clans_and_his_banners_fall_with_him() {
    let mut s = stage(1);
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    let k = kind(&s, "orc_lord");
    let lord = s.horde_mut().unwrap().spawn(k, 60.0, 50.5).unwrap();
    at(&mut s, 2.5, &mut out);
    let max = s.horde.as_ref().unwrap().block(lord).hp;
    s.horde_mut().unwrap().store_mut().hp[lord as usize] = max * (CLANS_PCT - 2) / 100;
    let mut out = Vec::new();
    at(&mut s, 4.0, &mut out);
    let m = msgs(&out);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::EnvEvent { id, phase: 1, .. } if id == HORN)), "the horn is forced");
    at(&mut s, 4.1, &mut out);
    assert_eq!(s.env.orc.horns(), 1, "and its column marches in");
    assert!(count(&s, "ork_warrior") > 0);
    // He falls: so do the banners.
    s.horde_mut().unwrap().despawn(lord);
    at(&mut s, 6.0, &mut out);
    assert_eq!(count(&s, BANNER), 0);
}

#[test]
fn the_orc_village_mission_plants_its_banners_on_walkable_ground_and_sounds_the_horn() {
    let m = missions::find(MISSION).unwrap();
    let horn = m.env.event(HORN).expect("the war horn");
    assert_eq!((horn.first_secs, horn.every_secs, horn.duration_secs), (120.0, 150.0, 25.0));
    assert!(m.env.strikes.iter().any(|s| s.during == HORN && s.ranged), "arrows fall while it sounds");
    assert_eq!(m.env.objects[0].monster, BANNER);
    assert_eq!(m.env.objects[0].spots.len(), 8);
    assert!(m.tips.len() >= 6);
    // On the baked map (when the pack has it): every spot is walkable.
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    let Ok(map) = crate::world::map::MapData::load(&dist, "gef_fild10") else { return };
    for &(x, y) in m.env.objects.iter().flat_map(|o| o.spots) {
        assert!(map.walk.is_walkable(x, y), "banner spot ({x}, {y}) is walkable");
    }
}
