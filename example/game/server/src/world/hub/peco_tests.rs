//! Riding and the piercing arrow in an instance (COMBAT.md §3.2 Cavalry,
//! §3.3): a Crusader on a Peco Peco is shown to the map and walks faster
//! in town; on a stage the riding skills need the mount and a spear; a
//! Piercing Arrow strikes every monster on its line once, as it reaches
//! each. The skill rows are `mount.rs`'s test table.

use super::mount::tests::{rider, BOOK};
use super::test_support::{map, portal, test_world};
use super::tests::{profile, quiet};
use super::*;
use crate::world::combat::{self, Loadout, SkillBook};
use crate::world::horde::catalog::{test_def, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{mount, Appearance, MONSTER_ENTITY_BASE};
use std::sync::Arc;
use uuid::Uuid;

fn book() -> Arc<SkillBook> {
    Arc::new(SkillBook::from_json(BOOK).unwrap())
}

fn controls(out: &Outgoing, user: u32) -> Vec<ServerMsg> {
    out.iter()
        .filter(|(u, _)| *u == user)
        .filter_map(|(_, e)| match e {
            HubEvent::Control(m) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

/// `(entity, mount, speed)` of every appearance user `user` got, from
/// `player_appearance` and the players of `map_enter` / `player_appeared`.
fn mounts(out: &Outgoing, user: u32) -> Vec<(u32, u8, u8)> {
    controls(out, user)
        .into_iter()
        .flat_map(|m| match m {
            ServerMsg::PlayerAppearance { entity_id, appearance } => vec![(entity_id, appearance)],
            ServerMsg::MapEnter { players, .. } => players.into_iter().map(|p| (p.entity_id, p.appearance)).collect(),
            ServerMsg::PlayerAppeared { player } => vec![(player.entity_id, player.appearance)],
            _ => Vec::new(),
        })
        .map(|(id, a)| (id, a.mount, a.speed))
        .collect()
}

/// The appearance updates (`player_appearance`) user `user` got.
fn shown(out: &Outgoing, user: u32) -> Vec<(u32, Appearance)> {
    controls(out, user)
        .into_iter()
        .filter_map(|m| match m {
            ServerMsg::PlayerAppearance { entity_id, appearance } => Some((entity_id, appearance)),
            _ => None,
        })
        .collect()
}

/// A level-40 crusader (user `user`) holding `weapon`, with `loadout`.
fn crusader(user: u32, weapon: &str, loadout: Loadout) -> PlayerProfile {
    PlayerProfile { archetype: "swordsman".into(), level: 40, weapon: weapon.into(), loadout, ..profile(user) }
}

fn put(hub: &mut HubInstance, user: u32, x: f32, y: f32) {
    let i = hub.find(user).unwrap();
    (hub.players[i].x, hub.players[i].y) = (x, y);
}

fn pos(hub: &HubInstance, user: u32) -> (f32, f32) {
    let p = &hub.players[hub.find(user).unwrap()];
    (p.x, p.y)
}

#[test]
fn a_rider_is_shown_to_the_map_and_walks_faster_in_town() {
    let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet()).with_skills(book());
    let mut out = Vec::new();
    hub.join(crusader(1, "", rider(true)), Uuid::new_v4(), None, 0, &mut out);
    let on = (101, mount::PECO, 25);
    assert_eq!(mounts(&out, 1), vec![on], "a rider learns its own mount on arrival");

    // A newcomer on foot sees the rider (in map_enter) and its own walk; the
    // rider learns the newcomer walks.
    out.clear();
    hub.join(profile(2), Uuid::new_v4(), None, 0, &mut out);
    assert_eq!(mounts(&out, 2), vec![on, (102, mount::NONE, 0)]);
    assert_eq!(mounts(&out, 1), vec![(102, mount::NONE, 0)], "the newcomer walks");

    // The rider walks 25 % farther (rows 1..5 of the town, no portals).
    put(&mut hub, 1, 4.5, 1.5);
    put(&mut hub, 2, 6.5, 1.5);
    for seq in 1..=4 {
        for u in [1, 2] {
            assert!(hub.input(u, seq, 0.0, 1.0));
        }
        hub.tick(&mut Vec::new());
    }
    let (ride, walk) = (pos(&hub, 1).1 - 1.5, pos(&hub, 2).1 - 1.5);
    assert!(walk > 0.5 && (ride / walk - 1.25).abs() < 0.01, "rider {ride} vs walker {walk}");

    // Getting off: the whole map sees it, and the rider walks normally.
    out.clear();
    hub.set_loadout(1, rider(false), &mut out);
    let off = (101, mount::NONE, 0);
    assert_eq!((mounts(&out, 1), mounts(&out, 2)), (vec![off], vec![off]));
    assert_eq!(hub.players[hub.find(1).unwrap()].ride, 0.0);
    // A loadout change that keeps the rider on foot says nothing.
    out.clear();
    hub.set_loadout(1, rider(false), &mut out);
    assert!(mounts(&out, 2).is_empty());
    // A mount without the skill is no mount.
    let mut unskilled = rider(true);
    unskilled.skills.remove("peco_ride");
    hub.set_loadout(1, unskilled, &mut out);
    assert!(mounts(&out, 2).is_empty());
}

#[test]
fn a_skill_reset_that_drops_the_mount_skill_dismounts() {
    let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet()).with_skills(book());
    let mut out = Vec::new();
    hub.join(crusader(1, "pike", rider(true)), Uuid::new_v4(), None, 0, &mut out);
    hub.join(profile(2), Uuid::new_v4(), None, 0, &mut out);
    out.clear();
    // The reset hands back an empty loadout; a stale `mounted` (a hand
    // edit, an old client) must not keep the rider up either.
    let reset = Loadout { mounted: true, ..Loadout::default() };
    hub.set_loadout(1, reset, &mut out);
    let off = (101, mount::NONE, 0);
    assert_eq!((mounts(&out, 1), mounts(&out, 2)), (vec![off], vec![off]), "the map sees the rider get off");
    let p = &hub.players[hub.find(1).unwrap()];
    assert!(!p.profile.loadout.mounted && p.ride == 0.0, "on foot, walking at its own pace");
    // Learning the skill again does not put it back on the bird.
    out.clear();
    hub.set_loadout(1, rider(false), &mut out);
    assert!(mounts(&out, 2).is_empty(), "{out:?}");
    // Arriving again (another map, a reconnect) re-states it whole.
    out.clear();
    hub.join(crusader(1, "pike", rider(false)), Uuid::new_v4(), None, 0, &mut out);
    assert_eq!(shown(&out, 1).into_iter().map(|(id, a)| (id, a.mount, a.speed)).collect::<Vec<_>>(), vec![off]);
}

#[test]
fn a_skill_reset_on_a_stage_gets_the_fighter_off_too() {
    let mut st = stage(crusader(1, "javelin", rider(true)));
    assert!(st.players[0].fighter.as_ref().unwrap().riding);
    let mut out = Vec::new();
    st.set_loadout(1, Loadout { mounted: true, ..Loadout::default() }, &mut out);
    assert!(!st.players[0].fighter.as_ref().unwrap().riding, "the fighter loses the riding passives");
    assert_eq!(shown(&out, 1).iter().map(|(id, a)| (*id, a.mount)).collect::<Vec<_>>(), vec![(101, mount::NONE)]);
}

#[test]
fn the_held_weapon_is_part_of_the_own_and_the_peers_appearance() {
    let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet()).with_skills(book());
    let mut out = Vec::new();
    // A level-10 swordsman holding a pike: its own client is told so right
    // after map_enter (it used to fall back to the class's sword).
    let pike = PlayerProfile { archetype: "swordsman".into(), level: 10, weapon: "pike".into(), ..profile(1) };
    hub.join(pike, Uuid::new_v4(), None, 0, &mut out);
    let own = shown(&out, 1);
    assert_eq!(own.len(), 1, "{own:?}");
    let (id, a) = &own[0];
    assert_eq!((*id, a.class.as_str(), a.tier, a.weapon.as_str(), a.mount), (101, "swordsman", 1, "spear:pike", mount::NONE));
    // A peer gets the same appearance inside map_enter.
    out.clear();
    hub.join(profile(2), Uuid::new_v4(), None, 0, &mut out);
    let listed = controls(&out, 2).into_iter().find_map(|m| match m {
        ServerMsg::MapEnter { players, .. } => players.into_iter().find(|p| p.entity_id == 101).map(|p| p.appearance),
        _ => None,
    });
    assert_eq!(listed.as_ref(), Some(a));
}

#[test]
fn appearances_are_re_derived_so_a_missed_change_heals_within_a_second() {
    let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet()).with_skills(book());
    let mut out = Vec::new();
    hub.join(crusader(1, "", rider(false)), Uuid::new_v4(), None, 0, &mut out);
    hub.join(profile(2), Uuid::new_v4(), None, 0, &mut out);
    // Nothing changed: the sweep says nothing (idempotent).
    out.clear();
    for _ in 0..hub.config.tick_hz * super::appearance::SWEEP_SECS {
        hub.tick(&mut out);
    }
    assert!(shown(&out, 2).is_empty(), "{:?}", shown(&out, 2));
    // An input changed behind the instance's back (a level, a field no
    // path announces): the next sweep shows it to the map, once.
    let i = hub.find(1).unwrap();
    hub.players[i].profile.level = 60;
    hub.players[i].profile.loadout.mounted = true;
    out.clear();
    for _ in 0..hub.config.tick_hz * super::appearance::SWEEP_SECS * 2 {
        hub.tick(&mut out);
    }
    let seen = shown(&out, 2);
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!((seen[0].1.tier, seen[0].1.mount, seen[0].1.speed), (3, mount::PECO, 25));
    assert_eq!(shown(&out, 1).len(), 1, "the player too");
}

/// A 20 × 9 field (cells 1..21 × 1..10) with sturdy, nearly still blobs;
/// user 1 joins as `who`, with every skill of `loadout` slotted in order.
fn stage(who: PlayerProfile) -> HubInstance {
    let row = format!("#{}#", ".".repeat(20));
    let wall = "#".repeat(22);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 9));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 0.01, 50, 0.0, false)]).unwrap());
    let cfg = HordeConfig { light_radius: 12.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(book());
    stage.dice = combat::Rng::new(5);
    let mut who = who;
    let l = &mut who.loadout;
    l.hotbar = vec![String::new(); combat::skills::SLOTS];
    for (i, id) in l.skills.keys().filter(|id| !matches!(id.as_str(), "spear_mastery")).enumerate() {
        l.hotbar[i] = id.clone();
    }
    stage.join(who, Uuid::nil(), None, 0, &mut Vec::new());
    put(&mut stage, 1, 2.5, 5.5);
    stage
}

fn slot_of(stage: &HubInstance, id: &str) -> u16 {
    let l = &stage.players[stage.find(1).unwrap()].profile.loadout;
    1 << l.hotbar.iter().position(|s| s == id).unwrap()
}

fn tough(stage: &mut HubInstance, x: f32, y: f32) -> u16 {
    let horde = stage.horde_mut().unwrap();
    let s = horde.spawn(0, x, y).unwrap();
    horde.store_mut().hp[s as usize] = 10_000;
    s
}

/// Presses `slots` once and ticks `n` times: whether user 1 started a cast
/// of `skill`, and the tick of each hit per monster entity (until user 1's
/// first basic attack).
fn press(stage: &mut HubInstance, slots: u16, skill: &str, n: u32) -> (bool, Vec<(u32, u32)>) {
    stage.act(1, [0, 0], slots);
    let (mut cast, mut hits) = (false, Vec::new());
    for t in 0..n {
        let mut out = Vec::new();
        stage.tick(&mut out);
        for m in controls(&out, 1) {
            match m {
                ServerMsg::Cast { caster: 101, skill: s, .. } if s == skill => cast = true,
                ServerMsg::Combat { swings, .. } if swings.iter().any(|w| w.entity_id == 101) => return (cast, hits),
                ServerMsg::Combat { hits: h, .. } => hits.extend(h.iter().map(|h| (h.target, t))),
                _ => {}
            }
        }
    }
    (cast, hits)
}

#[test]
fn riding_skills_need_the_mount_and_a_spear() {
    // Mounted with a javelin: the fighter rides with its passives.
    let mut st = stage(crusader(1, "javelin", rider(true)));
    let f = st.players[0].fighter.as_ref().unwrap();
    assert!(f.riding);
    assert!((f.mods.speed - 0.25).abs() < 1e-6 && f.mods.damage_flat == 10, "{:?}", f.mods);
    tough(&mut st, 4.5, 5.5);
    let slot = slot_of(&st, "brandish");
    assert!(press(&mut st, slot, "brandish", 3).0, "mounted, spear in hand: it swings");

    // On foot, or riding with a sword: no Brandish Spear.
    for (weapon, mounted) in [("javelin", false), ("", true)] {
        let mut st = stage(crusader(1, weapon, rider(mounted)));
        assert_eq!(st.players[0].fighter.as_ref().unwrap().riding, mounted);
        tough(&mut st, 4.5, 5.5);
        let slot = slot_of(&st, "brandish");
        assert!(!press(&mut st, slot, "brandish", 3).0, "{weapon:?} mounted {mounted}");
    }

    // The mount skill itself is never cast from a slot (MOUNT_REQ does it).
    let mut st = stage(crusader(1, "javelin", rider(false)));
    let slot = slot_of(&st, "peco_ride");
    assert!(!press(&mut st, slot, "peco_ride", 3).0);
    // Getting on mid-stage re-equips the fighter.
    st.set_loadout(1, rider(true), &mut Vec::new());
    assert!(st.players[0].fighter.as_ref().unwrap().riding);
}

#[test]
fn a_piercing_arrow_hits_every_monster_on_its_line_as_it_passes() {
    let mut l = Loadout::default();
    l.skills.insert("piercing_arrow".into(), 1);
    let archer = PlayerProfile { archetype: "archer".into(), level: 40, loadout: l, ..profile(1) };
    let mut st = stage(archer);
    // A row east of the archer (2.5, 5.5) and one off the line.
    let row: Vec<u16> = [4.5, 7.5, 10.5, 13.5].iter().map(|&x| tough(&mut st, x, 5.5)).collect();
    let aside = tough(&mut st, 7.5, 8.5);
    let slot = slot_of(&st, "piercing_arrow");
    let (cast, hits) = press(&mut st, slot, "piercing_arrow", 8);
    assert!(cast);
    // 30 m/s = 3 cells a tick: 2, 5, 8 and 11 cells out land in turn.
    let tick_of = |s: u16| -> Vec<u32> { hits.iter().filter(|h| h.0 == MONSTER_ENTITY_BASE + s as u32).map(|h| h.1).collect() };
    let ticks: Vec<Vec<u32>> = row.iter().map(|&s| tick_of(s)).collect();
    assert!(ticks.iter().all(|t| t.len() == 1), "each monster once: {ticks:?}");
    let firsts: Vec<u32> = ticks.iter().map(|t| t[0]).collect();
    assert!(firsts.windows(2).all(|w| w[0] < w[1]), "nearest first, one after another: {firsts:?}");
    assert!(tick_of(aside).is_empty(), "off the line");
}
