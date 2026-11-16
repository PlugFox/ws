//! Toy Factory on a small field (`toy.rs`, `env.rs` drift): a belt carries
//! players and monsters (walls stop it), a shift reverses it, a switch
//! stops the belts near it and resets, and the real mission plants its
//! switches on the real map.

use super::super::env::Env;
use super::super::test_support::map;
use super::super::{HubEvent, HubSimConfig, HubWorld, PlayerProfile};
use super::*;
use crate::world::combat;
use crate::world::environment::{Environment, Hazard, MapEvent, ObjectDef};
use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::missions::{self, MissionInfo};
use crate::world::protocol::{zone_flags, Look};
use std::sync::Arc;
use uuid::Uuid;

/// A belt along y = 10.5 from x = 2 to 38, running +x at 2 cells/s.
const BELT: Hazard = Hazard { look: "ripple", drift: [2.0, 0.0], ..Hazard::rect("belt_0", 20.0, 10.5, 36.0, 2.0) };
/// A belt far from the switch (y = 20.5).
const FAR_BELT: Hazard = Hazard { look: "ripple", drift: [0.0, 2.0], ..Hazard::rect("belt_1", 60.0, 10.5, 2.0, 16.0) };

const SHIFT_TEST: MapEvent = MapEvent {
    id: SHIFT,
    name: "Shift Change",
    warning: "",
    first_secs: 100.0,
    every_secs: 0.0,
    warn_secs: 0.0,
    duration_secs: 30.0,
    light_scale: 1.0,
    shake: 0,
    calls_waves: false,
};

static MISSION_TEST: MissionInfo = MissionInfo {
    id: MISSION,
    name: "Toy",
    map: "field",
    fog: None,
    env: Environment {
        light_scale: 1.0,
        hazards: &[BELT, FAR_BELT],
        events: &[SHIFT_TEST],
        objects: &[ObjectDef { monster: SWITCH_MONSTER, spots: &[(10.5, 14.5)], on_felled: ObjectEffect::Hook(SWITCH_HOOK), hatch: None }],
        strikes: &[],
    },
    ..MissionInfo::TEMPLATE
};

fn stage() -> HubInstance {
    let wall = "#".repeat(72);
    let row = format!("#{}#", ".".repeat(70));
    let mut rows = vec![wall.clone()];
    rows.extend(std::iter::repeat_n(row, 21));
    rows.push(wall);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let catalog = Arc::new(
        MonsterCatalog::new(vec![
            test_def("cookie", 0.05, 1, 0.0, false),
            test_plant(SWITCH_MONSTER, (2, 2)),
            test_plant(SWITCH_OFF, (60000, 60000)),
        ])
        .unwrap(),
    );
    let cfg = HordeConfig { light_radius: 30.0, director_enabled: false, seed: Some(2), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, MISSION_TEST.id, world, HubSimConfig { residents: 0, ..Default::default() }).with_horde(horde);
    stage.env = Env::new(Some(&MISSION_TEST), &[]);
    stage.dice = combat::Rng::new(5);
    let profile = PlayerProfile {
        user_id: 1,
        character_id: 101,
        name: "p1".into(),
        archetype: "swordsman".into(),
        look: Look::default(),
        level: 70,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
        homunculus: None,
    };
    let mut out = Vec::new();
    stage.join(profile, Uuid::nil(), None, 0, &mut out);
    let i = stage.find(1).unwrap();
    (stage.players[i].x, stage.players[i].y) = (5.5, 10.5);
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

fn me(s: &HubInstance) -> (f32, f32) {
    let i = s.find(1).unwrap();
    (s.players[i].x, s.players[i].y)
}

#[test]
fn a_belt_carries_the_player_and_monsters_and_shows_its_drift() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    let zones: Vec<_> = msgs(&out)
        .into_iter()
        .filter_map(|m| match m {
            ServerMsg::BossZones { caster: 0, ability, zones } if ability == "hazard:belt" => Some(zones),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(zones.len(), 2, "both belts show");
    let b = zones.iter().find(|z| z.area.y == 10.5 && z.area.x == 20.0).unwrap();
    assert_eq!((b.area.shape, b.area.dir, b.area.a, b.area.b), (1, 0, 36.0, 2.0), "a box along +x");
    assert_eq!((b.flags & zone_flags::DRIFT, zone_flags::drift_speed(b.flags)), (zone_flags::DRIFT, 2.0));
    let up = zones.iter().find(|z| z.area.x == 60.0).unwrap();
    assert_eq!((up.area.dir, up.area.a, up.area.b), (64, 16.0, 2.0), "the other along +y");

    let kind = s.horde.as_ref().unwrap().catalog().kind_of("cookie").unwrap();
    let m = s.horde_mut().unwrap().spawn(kind, 20.5, 10.5).unwrap();
    let (x0, _) = me(&s);
    let hz = s.config.tick_hz as f32;
    for t in 0..(hz as u32) {
        at(&mut s, 1.0 + t as f32 / hz, &mut out);
    }
    let (x1, y1) = me(&s);
    assert!((x1 - x0 - 2.0).abs() < 0.11 && y1 == 10.5, "a second on the belt: 2 cells (from {x0} to {x1})");
    let mx = s.horde.as_ref().unwrap().store().x[m as usize];
    assert!(mx > 21.5, "the monster rides too ({mx})");
    // To the end wall: walls stop it.
    for t in 0..(40 * hz as u32) {
        at(&mut s, 2.0 + t as f32 / hz, &mut out);
    }
    let (x2, _) = me(&s);
    assert!(x2 < 38.2 && x2 > 37.5, "the belt ends at its box ({x2})");
}

#[test]
fn a_shift_reverses_the_belts_and_a_switch_stops_the_near_ones_then_resets() {
    let mut s = stage();
    let mut out = Vec::new();
    at(&mut s, 1.0, &mut out);
    assert_eq!(s.env.toy.lanes(), 2);
    let mut out = Vec::new();
    at(&mut s, 100.0, &mut out);
    assert_eq!(s.env.toy.scale("belt_0"), Some(SHIFT_SCALE));
    assert_eq!(s.env_drift_scale("belt_0"), Some(SHIFT_SCALE));
    let m = msgs(&out);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::ZoneEnd { .. })), "the old belt zones go");
    let re = m
        .iter()
        .find_map(|m| match m {
            ServerMsg::BossZones { caster: 0, zones, .. } => zones.iter().find(|z| z.area.x == 20.0).copied(),
            _ => None,
        })
        .expect("the belt is sent again");
    assert_eq!((re.area.dir, zone_flags::drift_speed(re.flags)), (128, 3.0), "reversed, faster");
    let (x0, _) = me(&s);
    at(&mut s, 100.5, &mut out);
    assert!(me(&s).0 < x0, "it carries the player back");

    // The switch (thrown at 100.5 s) stops the belt near it, not the far one.
    let mut out = Vec::new();
    s.toy_switch(0, 10.5, 14.5, &mut out);
    at(&mut s, 101.0, &mut out);
    assert_eq!(s.env.toy.scale("belt_0"), Some(0.0));
    assert_eq!(s.env.toy.scale("belt_1"), Some(SHIFT_SCALE), "the far belt runs on");
    let (x0, _) = me(&s);
    at(&mut s, 101.5, &mut out);
    assert_eq!(me(&s).0, x0, "a stopped belt is plain floor");
    // The shift ends meanwhile; the stop holds to its end, then it runs on.
    at(&mut s, 130.2, &mut out);
    assert_eq!(s.env.toy.scale("belt_1"), Some(1.0));
    assert_eq!(s.env.toy.scale("belt_0"), Some(0.0));
    at(&mut s, 100.5 + STOP_SECS + 0.5, &mut out);
    assert_eq!(s.env.toy.scale("belt_0"), Some(1.0));
    // The thrown switch stands live again after its cooldown.
    at(&mut s, 101.0 + SWITCH_COOLDOWN + 1.5, &mut out);
    let horde = s.horde.as_ref().unwrap();
    let live = horde.catalog().kind_of(SWITCH_MONSTER).unwrap();
    let off = horde.catalog().kind_of(SWITCH_OFF).unwrap();
    assert_eq!(horde.monsters().filter(|m| m.2 == off).count(), 0);
    assert!(horde.monsters().filter(|m| m.2 == live).count() >= 1);
}

#[test]
fn the_toy_mission_has_its_shift_crates_and_switches_on_walkable_ground() {
    let m = missions::find(MISSION).unwrap();
    assert!(m.env.event(SHIFT).is_some());
    assert_eq!(m.env.strikes[0].hook, Some(CRATE_HOOK));
    assert_eq!(m.env.objects[0].spots.len(), 10);
    assert!(m.env.objects[0].monster == SWITCH_MONSTER && m.env.objects[0].on_felled == ObjectEffect::Hook(SWITCH_HOOK));
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    let Ok(map) = crate::world::map::MapData::load(&dist, "xmas_dun02") else { return };
    for &(x, y) in m.env.objects.iter().flat_map(|o| o.spots) {
        assert!(map.walk.is_walkable(x, y), "switch spot ({x}, {y}) is walkable");
    }
    let belts = crate::world::environment::load_hazards(&dist.join("server/locations/xmas_dun02")).unwrap();
    assert!(belts.iter().filter(|h| h.id.starts_with(BELT_PREFIX) && h.drifts()).count() >= 15, "the baked lanes");
    // Nothing on a lane stops a rider: the bake opens the cells under the
    // boxes and toys RO set down on the belts (`belts.cargo`) and leaves
    // them out of the props. Along each lane's centre line the walkable
    // cells run unbroken; only a lane's end may sit in a wall (a block
    // castle it runs into).
    for b in belts.iter().filter(|h| h.id.starts_with(BELT_PREFIX) && h.drifts()) {
        let HazardShape::Rect { x, y, w, h } = b.shape else { continue };
        let along_x = w > h;
        let len = if along_x { w } else { h };
        let cells: Vec<bool> = (0..len.floor() as i32)
            .map(|i| {
                let t = i as f32 + 0.5 - len / 2.0;
                if along_x { map.walk.is_walkable(x + t, y) } else { map.walk.is_walkable(x, y + t) }
            })
            .collect();
        let first = cells.iter().position(|&c| c).unwrap_or(0);
        let last = cells.iter().rposition(|&c| c).unwrap_or(0);
        assert!(cells[first..=last].iter().all(|&c| c), "{}: a blocked cell mid-lane: {cells:?}", b.id);
        assert!(last + 1 - first >= cells.len() * 3 / 4, "{}: the lane runs clear for most of its length", b.id);
    }
}
