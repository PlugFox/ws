//! The Archer line's arrows on a stage: Arrow Change nocking the arrows its
//! level opens (by press and by `QUIVER_REQ`), the nocked element on a
//! bow's shots and arrow skills (not on other damage), never auto-cast, and
//! Arrow Vulcan raining its arrows one at a time. The table is the archer
//! rows of `skills.toml` with numbers picked to keep each test exact.

use super::fight::Fighter;
use super::test_support::{map, portal};
use super::tests::profile;
use super::*;
use crate::world::combat::{self, DamageType, Element, Loadout, SkillBook};
use crate::world::horde::catalog::{test_def, MonsterCatalog, MonsterDef};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{HitEvent, MONSTER_ENTITY_BASE, QUIVER_NEXT};
use std::sync::Arc;
use uuid::Uuid;

const BOOK: &str = r#"{"skills":[
  {"id":"arrow_change","name":"Arrow Change","class":"archer","max_level":5,"target":"self","mobile":true,"cooldown":0.4,
   "effects":[{"type":"quiver","arrows":["physical","fire","cold","lightning","earth","radiant"]}]},
  {"id":"double_strafe","name":"Double Strafe","class":"archer","max_level":10,"target":"strongest","range":16,"cooldown":1.2,
   "effects":[{"type":"damage","dice":"weapon","extra":"1d3","mult":[0,1],"count":2,"every":0.15,"follow":"target","projectile":20,"arrow":true}]},
  {"id":"arrow_vulcan","name":"Arrow Vulcan","class":"archer","max_level":10,"target":"densest","range":14,"cooldown":9,
   "effects":[{"type":"damage","dice":"1d8","scale":"k","radius":1.5,"scatter":0.2,"count":[8,1],"every":0.12,"arrow":true}]},
  {"id":"land_mine","name":"Land Mine","class":"archer","target":"nearest","range":8,"cooldown":5,
   "effects":[{"type":"damage","dice":"2d10","radius":1}]}
]}"#;

/// A 20 × 9 field; a level-20 archer (user 1, the class's bow) at
/// (4.5, 5.5) with `skills` slotted in order; monsters barely move (an
/// earth clod: fire is its weakness).
fn stage(skills: &[(&str, u8)]) -> HubInstance {
    let row = format!("#{}#", ".".repeat(20));
    let wall = "#".repeat(22);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 9));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let clod = MonsterDef { element: Element::Earth, ..test_def("clod", 0.01, 50, 0.0, false) };
    let catalog = Arc::new(MonsterCatalog::new(vec![clod]).unwrap());
    let cfg = HordeConfig { light_radius: 16.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let book = SkillBook::from_json(BOOK).unwrap();
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(Arc::new(book));
    stage.dice = combat::Rng::new(7);
    let mut loadout = Loadout::default();
    loadout.hotbar.resize(combat::skills::SLOTS, Default::default());
    for (i, (id, lvl)) in skills.iter().enumerate() {
        loadout.skills.insert(id.to_string(), *lvl);
        loadout.hotbar[i] = id.to_string().into();
    }
    let mut out = Vec::new();
    stage.join(PlayerProfile { archetype: "archer".into(), level: 20, loadout, ..profile(1) }, Uuid::nil(), None, 0, &mut out);
    let i = stage.find(1).unwrap();
    (stage.players[i].x, stage.players[i].y) = (4.5, 5.5);
    stage
}

fn fighter(stage: &HubInstance) -> &Fighter {
    stage.players[stage.find(1).unwrap()].fighter.as_ref().unwrap()
}

fn fighter_mut(stage: &mut HubInstance) -> &mut Fighter {
    let i = stage.find(1).unwrap();
    stage.players[i].fighter.as_mut().unwrap()
}

/// A clod at `(x, y)` that does not die.
fn clod(stage: &mut HubInstance, x: f32, y: f32) -> u16 {
    let horde = stage.horde_mut().unwrap();
    let s = horde.spawn(0, x, y).unwrap();
    horde.store_mut().hp[s as usize] = 100_000;
    s
}

/// Ticks `n` times with `slots` pressed on the first; every tick's hits
/// (misses included) and the `QUIVER` frames user 1 got.
fn run(stage: &mut HubInstance, n: u32, slots: u16) -> (Vec<Vec<HitEvent>>, Vec<u8>) {
    stage.act(1, [0, 0], slots);
    let (mut ticks, mut quivers) = (Vec::new(), Vec::new());
    for _ in 0..n {
        let mut out = Vec::new();
        stage.tick(&mut out);
        let mut hits = Vec::new();
        for (u, e) in out {
            match (u, e) {
                (1, HubEvent::Control(ServerMsg::Combat { hits: h, .. })) => hits.extend(h),
                (1, HubEvent::Control(ServerMsg::Quiver { entity_id: 101, element })) => quivers.push(element),
                _ => {}
            }
        }
        ticks.push(hits);
    }
    (ticks, quivers)
}

/// The damage type a hit's flags carry.
fn kind_of(h: &HitEvent) -> u8 {
    h.flags >> 4
}

#[test]
fn arrow_change_nocks_the_arrows_its_level_opens_in_turn() {
    let mut stage = stage(&[("arrow_change", 2)]);
    assert_eq!(fighter(&stage).arrow, DamageType::Physical, "the plain steel arrow to start with");
    let hz = stage.tick_hz();
    let gap = hz / 2 + 1;
    let mut nocked = Vec::new();
    let mut told = Vec::new();
    for _ in 0..4 {
        let (_, q) = run(&mut stage, gap, 1);
        nocked.push(fighter(&stage).arrow);
        told.extend(q);
    }
    // Level 2 opens steel, fire and crystal (cold); then round again.
    assert_eq!(nocked, [DamageType::Fire, DamageType::Cold, DamageType::Physical, DamageType::Fire]);
    assert_eq!(told, [1, 2, 0, 1], "every change goes to the stage as QUIVER");
    // QUIVER_REQ picks one the level opens, and nothing else.
    stage.quiver(1, DamageType::Radiant as u8);
    assert_eq!(fighter(&stage).arrow, DamageType::Fire, "Silver needs level 5");
    stage.quiver(1, DamageType::Cold as u8);
    assert_eq!(fighter(&stage).arrow, DamageType::Cold);
    stage.quiver(1, QUIVER_NEXT);
    assert_eq!(fighter(&stage).arrow, DamageType::Physical);
    // Without the skill the request does nothing.
    let mut plain = self::stage(&[]);
    plain.quiver(1, DamageType::Fire as u8);
    assert_eq!(fighter(&plain).arrow, DamageType::Physical);
}

#[test]
fn arrow_change_is_never_auto_cast() {
    let mut stage = stage(&[("arrow_change", 5)]);
    stage.auto_slots(1, 1);
    let hz = stage.tick_hz();
    run(&mut stage, hz, 0);
    assert_eq!(fighter(&stage).arrow, DamageType::Physical, "the AI would only spin the quiver");
}

#[test]
fn the_nocked_arrow_sets_the_element_of_bow_shots_and_arrow_skills() {
    let mut stage = stage(&[("arrow_change", 5), ("double_strafe", 5), ("land_mine", 1)]);
    clod(&mut stage, 9.5, 5.5);
    // Steel: physical bow shots.
    let hz = stage.tick_hz();
    let (ticks, _) = run(&mut stage, hz * 2, 0);
    let shots: Vec<HitEvent> = ticks.into_iter().flatten().collect();
    assert!(!shots.is_empty(), "the bow shoots");
    assert!(shots.iter().all(|h| kind_of(h) == DamageType::Physical as u8));
    // Fire: the bow's shots and Double Strafe burn.
    stage.quiver(1, DamageType::Fire as u8);
    let (ticks, q) = run(&mut stage, hz * 2, 0);
    assert_eq!(q, [DamageType::Fire as u8], "the stage is told");
    let shots: Vec<HitEvent> = ticks.into_iter().flatten().collect();
    assert!(!shots.is_empty() && shots.iter().all(|h| kind_of(h) == DamageType::Fire as u8), "{shots:?}");
    let (ticks, _) = run(&mut stage, hz / 2, 2);
    let strafe: Vec<HitEvent> = ticks.into_iter().flatten().collect();
    assert!(strafe.len() >= 2, "both arrows of the strafe ({strafe:?})");
    assert!(strafe.iter().all(|h| kind_of(h) == DamageType::Fire as u8));
    // Damage that is no arrow keeps its own type.
    fighter_mut(&mut stage).recovery = 0;
    let (ticks, _) = run(&mut stage, 1, 4);
    let mine: Vec<&HitEvent> = ticks.iter().flatten().filter(|h| h.flags & crate::world::protocol::hit::MISS == 0).collect();
    assert!(mine.iter().any(|h| kind_of(h) == DamageType::Physical as u8), "the mine is no arrow: {mine:?}");
    // Without a bow in hand the arrow does nothing.
    let f = fighter_mut(&mut stage);
    f.weapon = combat::Weapon::held(combat::Class::Thief, "knife");
    assert_eq!(f.nocked(), None);
}

#[test]
fn arrow_vulcan_rains_its_arrows_one_at_a_time() {
    let mut stage = stage(&[("arrow_vulcan", 4)]);
    let s = clod(&mut stage, 12.5, 5.5);
    let target = MONSTER_ENTITY_BASE + s as u32;
    let hz = stage.tick_hz();
    // Loosed, then walking along the wall: the bow holds its basic shots
    // while moving, so only the volley strikes.
    stage.act(1, [0, 0], 1);
    let mut landed = Vec::new();
    for t in 0..hz * 2 {
        let mut out = Vec::new();
        stage.tick(&mut out);
        stage.input(1, t + 1, -1.0, 0.0);
        let n = out
            .iter()
            .filter_map(|(u, e)| match (u, e) {
                (1, HubEvent::Control(ServerMsg::Combat { hits, .. })) => Some(hits.iter().filter(|h| h.target == target).count()),
                _ => None,
            })
            .sum::<usize>();
        landed.extend(std::iter::repeat_n(t, n));
    }
    // 8 + 4 arrows, each on its own tick, 0.12 s apart.
    assert_eq!(landed.len(), 12, "every arrow of the volley falls on the clod: {landed:?}");
    assert!(landed.windows(2).all(|w| w[1] > w[0]), "one by one: {landed:?}");
    let span = (landed[11] - landed[0]) as f32 / hz as f32;
    assert!((span - 11.0 * 0.12).abs() < 0.1, "spread over {span} s");
}
