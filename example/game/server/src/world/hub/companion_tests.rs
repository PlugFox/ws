//! Companions on a stage (design §3, §4, §7): a level-30 Merchant (user 1)
//! with its homunculus out, on an open 40 × 20 field (cells 1..41 × 1..21),
//! monsters placed by hand. Each scenario steps the whole instance tick by
//! tick (`HubInstance::tick`), so the horde's movement, the fight's melee,
//! the hazards and the messages all run as in a real stage.

use super::super::fight::Fighter;
use super::super::test_support::{map, portal};
use super::super::tests::profile;
use super::super::{brain, HubEvent, HubInstance, HubSimConfig, HubWorld, Outgoing, PlayerProfile};
use super::*;
use crate::world::combat::{Loadout, SkillBook};
use crate::world::horde::catalog::{test_def, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use std::sync::Arc;
use uuid::Uuid;
use std::assert_matches;

/// A caster-centred blast (the player AoE that must spare companions).
const BOOK: &str = r#"{"skills":[
  {"id":"nova","name":"Nova","class":"merchant","max_level":1,"target":"self","cooldown":1,"sp":0,
   "effects":[{"type":"damage","dice":"6d6","radius":5,"at":"caster"}]}
]}"#;

/// Monster kinds (by id: the catalog sorts them): a blob that hardly
/// moves, a runner, a boss.
const BLOB: &str = "blob";
const RUNNER: &str = "runner";
const KING: &str = "king";

fn field(rows: &[&str]) -> HubInstance {
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let catalog = Arc::new(
        MonsterCatalog::new(vec![test_def("blob", 0.01, 1, 0.0, false), test_def("runner", 2.0, 1, 0.0, false), test_def("king", 2.0, 50, 0.0, true)])
            .unwrap(),
    );
    let cfg = HordeConfig { light_radius: 30.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let book = SkillBook::from_json(BOOK).unwrap();
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(Arc::new(book));
    stage.dice = Rng::new(7);
    stage
}

fn open_rows() -> Vec<String> {
    let mut rows = vec!["#".repeat(42)];
    rows.extend(std::iter::repeat_n(format!("#{}#", ".".repeat(40)), 20));
    rows.push("#".repeat(42));
    rows
}

fn merchant(t: HomType, hom_level: u8) -> PlayerProfile {
    let mut loadout = Loadout::default();
    loadout.hotbar.resize(combat::skills::SLOTS, Default::default());
    loadout.skills.insert(CALL_SKILL.into(), 1);
    loadout.skills.insert("nova".into(), 1);
    loadout.hotbar[0] = "nova".into();
    let save = HomSave { kind: t.id().into(), name: String::new(), level: hom_level, exp: 0, bond: 0, evolved: false, stance: "defensive".into() };
    PlayerProfile { archetype: "merchant".into(), level: 30, loadout, homunculus: Some(save), ..profile(1) }
}

/// The open field, the Merchant at (10.5, 10.5) facing north, its
/// homunculus out beside it (one tick run).
fn stage(t: HomType) -> HubInstance {
    stage_on(t, &open_rows().iter().map(String::as_str).collect::<Vec<_>>(), (10.5, 10.5))
}

fn stage_on(t: HomType, rows: &[&str], at: (f32, f32)) -> HubInstance {
    let mut stage = field(rows);
    let mut out = Vec::new();
    stage.join(merchant(t, 25), Uuid::nil(), None, 0, &mut out);
    put(&mut stage, 1, at.0, at.1);
    stage.tick(&mut out);
    assert!(stage.companions.homunculus_of(1).is_some(), "a called homunculus comes out on the stage");
    stage
}

fn put(stage: &mut HubInstance, user: u32, x: f32, y: f32) {
    let i = stage.find(user).unwrap();
    (stage.players[i].x, stage.players[i].y) = (x, y);
}

fn owner_pos(stage: &HubInstance) -> (f32, f32) {
    let p = &stage.players[stage.find(1).unwrap()];
    (p.x, p.y)
}

fn hom(stage: &HubInstance) -> Companion {
    *stage.companions.homunculus_of(1).expect("out")
}

fn hom_mut(stage: &mut HubInstance) -> &mut Companion {
    stage.companions.units.iter_mut().find(|c| matches!(c.kind, Kind::Homunculus(_))).expect("out")
}

fn owner_hp(stage: &mut HubInstance) -> &mut i32 {
    let i = stage.find(1).unwrap();
    &mut stage.players[i].fighter.as_mut().unwrap().hp
}

fn spawn(stage: &mut HubInstance, kind: &str, x: f32, y: f32, hp: i32) -> u16 {
    let horde = stage.horde_mut().unwrap();
    let kind = horde.catalog().kind_of(kind).unwrap();
    let s = horde.spawn(kind, x, y).unwrap();
    horde.store_mut().hp[s as usize] = hp;
    s
}

fn run(stage: &mut HubInstance, ticks: u32) -> Outgoing {
    let mut out = Vec::new();
    for _ in 0..ticks {
        stage.tick(&mut out);
    }
    out
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

/// Every hit of the period on `entity`.
fn hits_on(out: &Outgoing, entity: u32) -> Vec<HitEvent> {
    let mut v: Vec<HitEvent> = controls(out, 1)
        .into_iter()
        .filter_map(|m| match m {
            ServerMsg::Combat { hits, .. } => Some(hits),
            _ => None,
        })
        .flatten()
        .filter(|h| h.target == entity)
        .collect();
    v.dedup();
    v
}

fn swings_by(out: &Outgoing, entity: u32) -> Vec<Swing> {
    controls(out, 1)
        .into_iter()
        .filter_map(|m| match m {
            ServerMsg::Combat { swings, .. } => Some(swings),
            _ => None,
        })
        .flatten()
        .filter(|s| s.entity_id == entity)
        .collect()
}

fn casts_by(out: &Outgoing, entity: u32) -> Vec<String> {
    controls(out, 1)
        .into_iter()
        .filter_map(|m| match m {
            ServerMsg::Cast { caster, skill, .. } if caster == entity => Some(skill),
            _ => None,
        })
        .collect()
}

fn owner(stage: &HubInstance) -> Owner {
    let p = &stage.players[stage.find(1).unwrap()];
    let f = p.fighter.as_ref().unwrap();
    Owner { uid: 1, entity: p.profile.character_id, level: f.level, max_hp: f.max_hp, to_hit: 7, tick: stage.tick }
}

/// Beside its owner: its follow slot (behind and to the side) and slack.
const NEAR: f32 = 2.6;

fn d(a: (f32, f32), b: (f32, f32)) -> f32 {
    dist(a.0, a.1, b.0, b.1)
}

// ---------------------------------------------------------------------------
// The wire: records, rows, panels
// ---------------------------------------------------------------------------

#[test]
fn the_homunculus_is_announced_rides_the_snapshot_and_its_owner_gets_the_panel() {
    let mut stage = field(&open_rows().iter().map(String::as_str).collect::<Vec<_>>());
    let mut out = Vec::new();
    stage.join(merchant(HomType::Filir, 25), Uuid::nil(), None, 0, &mut out);
    assert!(
        controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::CompanionState(s) if s.hom_type == hom_type::FILIR && s.level == 25)),
        "the panel on join"
    );
    let out = run(&mut stage, 2);
    let e = hom(&stage).entity();
    let records: Vec<CompanionInfo> = controls(&out, 1)
        .into_iter()
        .filter_map(|m| match m {
            ServerMsg::Companions { list } => Some(list),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(records.iter().any(|r| r.entity_id == e && r.sprite == "filir" && r.class == companion_class::HOMUNCULUS && r.owner_entity_id == 101));
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::PlayerVitals { entity_id, state: 0, .. } if *entity_id == e)));
    let snap = out.iter().rev().find_map(|(_, ev)| match ev {
        HubEvent::Snapshot(b) => Some(b.clone()),
        _ => None,
    });
    let snap = crate::world::protocol::Snapshot::decode(&snap.unwrap()).unwrap();
    assert!(snap.entities.iter().any(|en| en.0 == e), "its row rides the player block");
    // A second player arriving gets the whole list.
    let mut out = Vec::new();
    stage.join(profile(2), Uuid::nil(), None, 0, &mut out);
    assert!(controls(&out, 2).iter().any(|m| matches!(m, ServerMsg::Companions { list } if list.iter().any(|r| r.entity_id == e))));
    assert!(!controls(&out, 2).iter().any(|m| matches!(m, ServerMsg::CompanionState(_))), "no panel for a non-owner");
}

#[test]
fn calling_without_a_type_opens_the_chooser_and_choosing_calls_it_out() {
    let mut stage = field(&open_rows().iter().map(String::as_str).collect::<Vec<_>>());
    let mut out = Vec::new();
    let p = PlayerProfile { homunculus: None, ..merchant(HomType::Lif, 1) };
    stage.join(p, Uuid::nil(), None, 0, &mut out);
    run(&mut stage, 12);
    assert_eq!(stage.companions.summon(owner(&stage), Summon::Homunculus, 1, 10.0, 10.0), Summoned::Refused);
    let out = run(&mut stage, 1);
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::CompanionState(s) if s.hom_type == hom_type::NONE)), "the chooser prompt at once");
    let mut out = Vec::new();
    stage.companion_cmd(1, CompanionCmd::Choose(hom_type::AMISTR), &mut out);
    let save = out.iter().find_map(|(_, e)| match e {
        HubEvent::Homunculus(s) => Some(s.clone()),
        _ => None,
    });
    assert_eq!(save.as_ref().map(|s| (s.kind.as_str(), s.level)), Some(("amistr", 25)), "born five levels below its owner");
    run(&mut stage, 1);
    assert_matches!(hom(&stage).kind, Kind::Homunculus(HomType::Amistr), "out without a second cast");
    // On a stage the type stays (it changes in town, for zeny).
    let mut out = Vec::new();
    stage.companion_cmd(1, CompanionCmd::Choose(hom_type::LIF), &mut out);
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::HubError { code, .. } if code == "type_change_in_town")));
    assert_matches!(hom(&stage).kind, Kind::Homunculus(HomType::Amistr));
}

#[test]
fn the_type_changes_in_town_for_zeny_keeping_the_level_and_resetting_the_bond() {
    let world = Arc::new(super::super::test_support::test_world());
    let mut town = HubInstance::new(1, world, super::super::tests::quiet());
    let mut p = merchant(HomType::Lif, 28);
    p.homunculus.as_mut().unwrap().bond = 40;
    let mut out = Vec::new();
    town.join(p, Uuid::nil(), None, 0, &mut out);
    let i = town.find(1).unwrap();
    town.players[i].profile.bag.zeny = TYPE_CHANGE_ZENY - 1;
    let mut out = Vec::new();
    town.companion_cmd(1, CompanionCmd::Choose(hom_type::FILIR), &mut out);
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::HubError { code, .. } if code == "type_change_zeny")));
    assert_eq!(town.companions.save_of(1).unwrap().kind, "lif", "not without the price");
    town.players[i].profile.bag.zeny = TYPE_CHANGE_ZENY + 7;
    let mut out = Vec::new();
    town.companion_cmd(1, CompanionCmd::Choose(hom_type::FILIR), &mut out);
    assert!(out.iter().any(|(u, e)| *u == 1 && matches!(e, HubEvent::ZenySpent { amount } if *amount == TYPE_CHANGE_ZENY)), "the socket task pays it from the bag");
    assert_eq!(town.players[i].profile.bag.zeny, 7, "the hub's copy paid already");
    let save = town.companions.save_of(1).unwrap();
    assert_eq!((save.kind.as_str(), save.level, save.bond), ("filir", 28, 0));
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::CompanionState(s) if s.hom_type == hom_type::FILIR && s.bond == 0)));
    assert!(out.iter().any(|(_, e)| matches!(e, HubEvent::Homunculus(s) if s.kind == "filir")), "saved");
    // The same type again is no change and costs nothing.
    let mut out = Vec::new();
    town.companion_cmd(1, CompanionCmd::Choose(hom_type::FILIR), &mut out);
    assert!(!out.iter().any(|(_, e)| matches!(e, HubEvent::ZenySpent { .. })));
}

#[test]
fn stance_rename_rest_and_evolve() {
    let mut stage = stage(HomType::Lif);
    let mut out = Vec::new();
    stage.companion_cmd(1, CompanionCmd::Stance(stance::AGGRESSIVE), &mut out);
    assert_eq!(hom(&stage).stance, Stance::Aggressive);
    stage.companion_cmd(1, CompanionCmd::Rename("  Pip ".into()), &mut out);
    assert_eq!(stage.companions.save_of(1).unwrap().name, "Pip");
    stage.companion_cmd(1, CompanionCmd::Rename("x".repeat(17)), &mut out);
    assert_eq!(stage.companions.save_of(1).unwrap().name, "Pip", "too long");
    let out = run(&mut stage, 2);
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::Companions { list } if list.iter().any(|r| r.name == "Pip"))));
    // Rest: it goes; wake: it is back.
    let mut out = Vec::new();
    stage.companion_cmd(1, CompanionCmd::Rest, &mut out);
    let out2 = run(&mut stage, 1);
    assert!(stage.companions.homunculus_of(1).is_none());
    assert!(controls(&out2, 1).iter().any(|m| matches!(m, ServerMsg::Companions { list } if list.iter().any(|r| r.flags & companion_flags::GONE != 0))));
    stage.companion_cmd(1, CompanionCmd::Rest, &mut out);
    run(&mut stage, 1);
    assert!(stage.companions.homunculus_of(1).is_some());
    // Evolution: in town only, a level-60 master, Bond 75, a Stone of Sage.
    let mut out = Vec::new();
    stage.companion_cmd(1, CompanionCmd::Evolve, &mut out);
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::HubError { code, .. } if code == "evolve_in_town")));
    let world = Arc::new(super::super::test_support::test_world());
    let mut town = HubInstance::new(1, world, super::super::tests::quiet());
    let mut p = merchant(HomType::Lif, 60);
    p.level = 60;
    p.homunculus.as_mut().unwrap().bond = 80;
    let mut out = Vec::new();
    town.join(p, Uuid::nil(), None, 0, &mut out);
    town.companion_cmd(1, CompanionCmd::Evolve, &mut out);
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::HubError { code, .. } if code == "evolve_stone")));
    let i = town.find(1).unwrap();
    town.players[i].profile.bag.add(STONE_OF_SAGE, 1).unwrap();
    let mut out = Vec::new();
    town.companion_cmd(1, CompanionCmd::Evolve, &mut out);
    assert!(out.iter().any(|(_, e)| matches!(e, HubEvent::ItemUsed { item } if item == STONE_OF_SAGE)), "the stone is used up");
    assert!(out.iter().any(|(_, e)| matches!(e, HubEvent::Homunculus(s) if s.evolved)));
}

// ---------------------------------------------------------------------------
// Safety by construction
// ---------------------------------------------------------------------------

#[test]
fn a_player_blast_over_a_homunculus_and_a_plant_leaves_both_untouched() {
    let mut stage = stage(HomType::Amistr);
    let (ox, oy) = owner_pos(&stage);
    let o = owner(&stage);
    let Summoned::Unit(plant) = stage.companions.summon(o, Summon::Flora, 1, ox + 1.5, oy) else { panic!() };
    let m = spawn(&mut stage, BLOB, ox + 3.0, oy + 2.0, 10_000);
    hom_mut(&mut stage).stance = Stance::Passive;
    run(&mut stage, 1);
    let (hp_hom, e_hom) = (hom(&stage).hp, hom(&stage).entity());
    stage.act(1, [0, 0], 1);
    let out = run(&mut stage, 2);
    assert!(!hits_on(&out, MONSTER_ENTITY_BASE + m as u32).is_empty(), "the blast lands on the monster");
    assert!(hits_on(&out, e_hom).iter().all(|h| h.flags & hit::HEAL != 0 || h.amount == 0) && hom(&stage).hp >= hp_hom);
    let p = stage.companions.by_entity(plant).unwrap();
    assert_eq!(p.hp, p.max_hp, "nor on the plant");
}

#[test]
fn a_monster_near_a_companion_still_walks_to_the_player() {
    let mut stage = stage(HomType::Lif);
    hom_mut(&mut stage).stance = Stance::Passive;
    let (ox, oy) = owner_pos(&stage);
    let m = spawn(&mut stage, RUNNER, ox + 8.0, oy + 3.0, 10_000);
    {
        let h = hom_mut(&mut stage);
        (h.x, h.y) = (ox + 8.6, oy + 3.0);
    }
    run(&mut stage, 1);
    let st = stage.horde().unwrap().store();
    let before = d((st.x[m as usize], st.y[m as usize]), (ox, oy));
    run(&mut stage, 40);
    let st = stage.horde().unwrap().store();
    let after = d((st.x[m as usize], st.y[m as usize]), (ox, oy));
    assert!(after < before - 2.5, "companions pull nothing by existing: {before} -> {after}");
}

#[test]
fn a_taunted_monster_walks_to_and_strikes_the_amistr_past_a_nearer_player_and_a_boss_ignores_it() {
    let mut stage = stage(HomType::Amistr);
    let (ox, oy) = owner_pos(&stage);
    let mut out = Vec::new();
    stage.join(profile(2), Uuid::nil(), None, 0, &mut out);
    put(&mut stage, 2, ox + 8.0, oy);
    let m = spawn(&mut stage, RUNNER, ox + 6.5, oy, 10_000);
    let king = spawn(&mut stage, KING, ox - 6.0, oy + 6.0, 10_000);
    // Put the Amistr away from the owner and taunt by hand (the Provoke
    // pulse's own write).
    let lure = hom(&stage).lure_id();
    let e = hom(&stage).entity();
    {
        let h = hom_mut(&mut stage);
        (h.x, h.y) = (ox, oy - 4.0);
        h.stance = Stance::Passive;
    }
    for s in [m, king] {
        let horde = stage.horde_mut().unwrap();
        horde.store_mut().afflict(s, Condition::Taunted, 400);
        horde.store_mut().taunter[s as usize] = lure;
    }
    let out = run(&mut stage, 80);
    let st = stage.horde().unwrap().store();
    let at = (st.x[m as usize], st.y[m as usize]);
    assert!(d(at, (hom(&stage).x, hom(&stage).y)) < 2.0, "it walked to the lure past player 2: {at:?}");
    assert!(swings_by(&out, MONSTER_ENTITY_BASE + m as u32).iter().any(|s| s.target == e), "and strikes it");
    // Taunts never stick to a boss (the companion step never writes one);
    // a hand-set one on the boss is the players' business, not a lure's.
    let mut buf = Vec::new();
    let n = taunt(stage.horde_mut().unwrap(), ox - 6.0, oy + 6.0, 3.0, 6, lure, 40, &mut buf);
    assert_eq!(n, 0, "the boss is not taunted by a companion");
}

#[test]
fn a_provoke_pulse_taunts_the_crowd_but_never_a_boss() {
    let mut stage = stage(HomType::Amistr);
    let h = hom(&stage);
    let ms: Vec<u16> = (0..3).map(|i| spawn(&mut stage, BLOB, h.x + 1.0, h.y - 1.0 + i as f32, 10_000)).collect();
    let king = spawn(&mut stage, KING, h.x - 2.0, h.y, 10_000);
    let out = run(&mut stage, 10);
    assert!(casts_by(&out, h.entity()).iter().any(|s| s == "hom_provoke"));
    let st = stage.horde().unwrap().store();
    assert!(ms.iter().any(|&s| st.status_of(s).has(Condition::Taunted) && st.taunter[s as usize] == h.lure_id()));
    assert!(st.taunter[king as usize] != h.lure_id());
}

#[test]
fn a_monster_walled_off_by_plants_bites_the_plant() {
    // A corridor one cell wide: the owner at its end, a Flora in it, the
    // monster beyond.
    let mut rows = vec!["#".repeat(22), format!("#.{}", "#".repeat(20))];
    for y in 2..10 {
        rows.push(if y == 5 { format!("#{}#", ".".repeat(20)) } else { format!("##{}#", "#".repeat(19)) });
    }
    // The portal's cell (1, 1), whichever way the rows count.
    rows[9] = format!("#.{}", "#".repeat(20));
    rows.push("#".repeat(22));
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let mut stage = field(&rows);
    let mut out = Vec::new();
    stage.join(PlayerProfile { homunculus: None, ..merchant(HomType::Lif, 1) }, Uuid::nil(), None, 0, &mut out);
    put(&mut stage, 1, 1.5, 5.5);
    let o = owner(&stage);
    let Summoned::Unit(plant) = stage.companions.summon(o, Summon::Flora, 1, 4.5, 5.5) else { panic!() };
    spawn(&mut stage, RUNNER, 9.5, 5.5, 10_000);
    let out = run(&mut stage, 120);
    assert!(hits_on(&out, plant).iter().any(|h| h.flags & hit::HEAL == 0), "the hedge gets eaten");
    assert!(hits_on(&out, 101).is_empty(), "the owner behind it is out of reach");
}

#[test]
fn a_hazard_hurts_a_companion_inside_it() {
    let mut stage = stage(HomType::Lif);
    let h = hom(&stage);
    let map = stage.horde().unwrap().map();
    let now = stage.tick + 1;
    let area = Area::Circle { x: h.x, y: h.y, r: 1.0 };
    stage.hazards.push(brain::Hazard::damage(0, map, area, now, now, 0, 5, Dice::new(4, 6, 10), DamageType::Fire, None));
    let out = run(&mut stage, 1);
    assert!(hits_on(&out, h.entity()).iter().any(|x| x.amount > 0));
    assert!(hom(&stage).hp < h.hp);
}

// ---------------------------------------------------------------------------
// Kills, experience, Bond
// ---------------------------------------------------------------------------

#[test]
fn homunculus_kills_pay_the_owner_and_build_no_combo() {
    let mut stage = stage(HomType::Filir);
    let (ox, oy) = owner_pos(&stage);
    // Out of the owner's axe, inside the defensive radius.
    let m = spawn(&mut stage, BLOB, ox + 4.2, oy, 1);
    // A monster of the owner's level pays full experience.
    stage.horde_mut().unwrap().store_mut().level[m as usize] = 30;
    let before = stage.companions.save_of(1).unwrap().exp;
    let out = run(&mut stage, 60);
    assert!(!stage.horde().unwrap().store().is_alive(m), "the homunculus killed it");
    assert!(out.iter().any(|(u, e)| *u == 1 && matches!(e, HubEvent::Progress { exp, .. } if *exp > 0)), "the owner is paid");
    let i = stage.find(1).unwrap();
    assert_eq!(stage.players[i].fighter.as_ref().unwrap().combo().count(), 0, "no combo");
    assert!(stage.companions.save_of(1).unwrap().exp > before, "and it earns its bonus copy");
}

#[test]
fn bond_grows_with_bosses_and_shrinks_with_deaths() {
    let mut stage = stage(HomType::Amistr);
    stage.companions.bond_out(&[1], BOND_BOSS);
    assert_eq!(stage.companions.save_of(1).unwrap().bond, 2);
    let tick = stage.tick;
    let i = stage.companions.units.iter().position(|c| matches!(c.kind, Kind::Homunculus(_))).unwrap();
    stage.companions.take(i, 10_000, tick);
    assert_eq!(stage.companions.save_of(1).unwrap().bond, 0, "−3 on its death, never below 0");
}

// ---------------------------------------------------------------------------
// Following, leash, teleport, leaving, the next wave
// ---------------------------------------------------------------------------

#[test]
fn it_follows_its_owner_across_the_field() {
    let mut stage = stage(HomType::Lif);
    let mut out = Vec::new();
    for seq in 1..=60 {
        stage.input(1, seq, 1.0, 0.0);
        stage.tick(&mut out);
    }
    let o = owner_pos(&stage);
    assert!(o.0 > 20.0, "the owner walked: {o:?}");
    run(&mut stage, 15);
    let h = hom(&stage);
    assert!(d((h.x, h.y), o) < 2.0, "it kept up: {:?} vs {o:?}", (h.x, h.y));
}

#[test]
fn the_leash_holds_even_on_a_focused_target() {
    let mut stage = stage(HomType::Filir);
    let (ox, oy) = owner_pos(&stage);
    hom_mut(&mut stage).stance = Stance::Aggressive;
    let m = spawn(&mut stage, BLOB, ox + 11.5, oy, 10_000);
    // The command key on it (12 cells away at most).
    let o = owner(&stage);
    stage.companions.summon(o, Summon::Homunculus, 1, ox + 11.5, oy);
    run(&mut stage, 60);
    let h = hom(&stage);
    assert_eq!(h.target.map(|t| t.0), Some(m), "focused");
    assert!(d((h.x, h.y), (ox, oy)) <= LEASH + 1.5, "{:?}", (h.x, h.y));
}

#[test]
fn the_command_key_recalls_it() {
    let mut stage = stage(HomType::Filir);
    let (ox, oy) = owner_pos(&stage);
    let m = spawn(&mut stage, BLOB, ox + 4.0, oy, 10_000);
    run(&mut stage, 10);
    assert_eq!(hom(&stage).target.map(|t| t.0), Some(m));
    let o = owner(&stage);
    stage.companions.summon(o, Summon::Homunculus, 1, ox - 5.0, oy - 5.0);
    run(&mut stage, 1);
    assert_eq!(hom(&stage).target, None, "recalled");
    assert_eq!(hom(&stage).state, State::Follow);
}

#[test]
fn far_behind_or_stuck_it_is_put_back_beside_the_owner() {
    let mut stage = stage(HomType::Lif);
    let o = owner_pos(&stage);
    {
        let h = hom_mut(&mut stage);
        (h.x, h.y) = (o.0 + 20.0, o.1);
    }
    run(&mut stage, 1);
    let h = hom(&stage);
    assert!(d((h.x, h.y), o) < NEAR, "beyond 14 cells: at once");
    // Stuck in a walled pocket with no line to the owner.
    let mut rows = open_rows();
    for row in &mut rows[3..=9] {
        row.replace_range(30..31, "#");
    }
    rows[3].replace_range(31..36, "#####");
    rows[9].replace_range(31..36, "#####");
    for row in &mut rows[3..=9] {
        row.replace_range(36..37, "#");
    }
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let mut stage = stage_on(HomType::Lif, &rows, (20.5, 6.5));
    {
        let h = hom_mut(&mut stage);
        (h.x, h.y) = (33.5, 6.5);
    }
    run(&mut stage, (STUCK_SECS.max(NO_LINE_SECS) * 20.0) as u32 + 10);
    let h = hom(&stage);
    assert!(d((h.x, h.y), (20.5, 6.5)) < NEAR, "out of the pocket: {:?}", (h.x, h.y));
}

#[test]
fn it_goes_with_its_owner_and_comes_back_with_the_next_wave() {
    let mut stage = stage(HomType::Vanilmirth);
    let e = hom(&stage).entity();
    // Dead: the row goes after the animation, back with the wave.
    let tick = stage.tick;
    let i = stage.companions.units.iter().position(|c| c.entity() == e).unwrap();
    stage.companions.take(i, 10_000, tick);
    let out = run(&mut stage, 1);
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::PlayerVitals { entity_id, state: 2, .. } if *entity_id == e)));
    run(&mut stage, 20);
    assert!(stage.companions.homunculus_of(1).is_none(), "the row is gone");
    stage.companions.new_wave();
    run(&mut stage, 1);
    let h = hom(&stage);
    assert_eq!(h.hp, h.max_hp / 2, "back at half HP");
    // The owner leaves: everything of theirs goes, the save is reported.
    let o = owner(&stage);
    stage.companions.summon(o, Summon::Flora, 1, 12.0, 12.0);
    let mut out = Vec::new();
    stage.leave(1, Uuid::nil(), &mut out);
    assert!(stage.companions.units.is_empty());
    assert!(out.iter().any(|(u, e)| *u == 1 && matches!(e, HubEvent::Homunculus(_))));
}

#[test]
fn a_link_dead_owner_keeps_it_out_five_seconds() {
    let mut stage = stage(HomType::Lif);
    let mut out = Vec::new();
    stage.detach(1, Uuid::nil(), &mut out);
    run(&mut stage, 60);
    assert!(stage.companions.homunculus_of(1).is_some());
    run(&mut stage, 50);
    assert!(stage.companions.homunculus_of(1).is_none(), "gone after 5 s");
    stage.join(merchant(HomType::Lif, 25), Uuid::nil(), None, 0, &mut out);
    run(&mut stage, 1);
    assert!(stage.companions.homunculus_of(1).is_some(), "back with the new socket");
}

// ---------------------------------------------------------------------------
// The roles
// ---------------------------------------------------------------------------

#[test]
fn lif_heals_its_owner_below_sixty_percent() {
    let mut stage = stage(HomType::Lif);
    let max = {
        let i = stage.find(1).unwrap();
        stage.players[i].fighter.as_ref().unwrap().max_hp
    };
    *owner_hp(&mut stage) = max * 7 / 10;
    let out = run(&mut stage, 10);
    assert!(casts_by(&out, hom(&stage).entity()).is_empty(), "not at 70 %");
    *owner_hp(&mut stage) = max / 2;
    let out = run(&mut stage, 15);
    assert!(casts_by(&out, hom(&stage).entity()).iter().any(|s| s == "hom_healing_hands"));
    assert!(hits_on(&out, 101).iter().any(|h| h.flags & hit::HEAL != 0 && h.amount > 0));
    assert!(*owner_hp(&mut stage) > max / 2);
}

#[test]
fn filir_retreats_below_thirty_percent_and_comes_back_above_half() {
    let mut stage = stage(HomType::Filir);
    let (ox, oy) = owner_pos(&stage);
    spawn(&mut stage, BLOB, ox + 4.0, oy, 10_000);
    run(&mut stage, 10);
    assert_eq!(hom(&stage).state, State::Engage);
    let max = hom(&stage).max_hp;
    hom_mut(&mut stage).hp = max / 4;
    let out = run(&mut stage, 6);
    assert_eq!(hom(&stage).state, State::Retreat);
    let e = hom(&stage).entity();
    let late = run(&mut stage, 10);
    assert!(swings_by(&late, e).is_empty(), "no blows while retreating");
    let _ = out;
    let h = hom(&stage);
    assert!(d((h.x, h.y), (ox, oy)) < NEAR, "back at the owner");
    hom_mut(&mut stage).hp = max * 6 / 10;
    run(&mut stage, 6);
    // Back in: striking, or already winding up Moonlight.
    assert_matches!(hom(&stage).state, State::Engage | State::Cast(_), "{:?}", hom(&stage).state);
}

#[test]
fn vanilmirth_holds_its_band_and_bolts_from_it() {
    let mut stage = stage(HomType::Vanilmirth);
    let (ox, oy) = owner_pos(&stage);
    let m = spawn(&mut stage, BLOB, ox + 3.0, oy + 3.0, 10_000);
    // Start it right on top of the monster.
    {
        let h = hom_mut(&mut stage);
        (h.x, h.y) = (ox + 3.0, oy + 2.2);
    }
    let out = run(&mut stage, 60);
    let st = stage.horde().unwrap().store();
    let h = hom(&stage);
    let gap = d((h.x, h.y), (st.x[m as usize], st.y[m as usize]));
    assert!((KITE_FLEE..=KITE_FAR + 0.5).contains(&gap), "it kites: {gap}");
    assert!(swings_by(&out, h.entity()).iter().any(|s| s.target == MONSTER_ENTITY_BASE + m as u32), "bolts");
}

#[test]
fn castling_swaps_with_a_hard_pressed_owner() {
    let mut stage = stage(HomType::Amistr);
    let (ox, oy) = owner_pos(&stage);
    {
        let h = hom_mut(&mut stage);
        h.level = 30;
        h.sp = h.max_sp;
        (h.x, h.y) = (ox - 3.0, oy);
        h.cds = [u32::MAX, u32::MAX, 0, u32::MAX];
    }
    for k in 0..4 {
        let a = k as f32 * std::f32::consts::FRAC_PI_2;
        spawn(&mut stage, BLOB, ox + a.cos() * 0.8, oy + a.sin() * 0.8, 10_000);
    }
    let max = {
        let i = stage.find(1).unwrap();
        stage.players[i].fighter.as_ref().unwrap().max_hp
    };
    *owner_hp(&mut stage) = max / 5;
    let before_hom = (hom(&stage).x, hom(&stage).y);
    let out = run(&mut stage, 6);
    assert!(casts_by(&out, hom(&stage).entity()).iter().any(|s| s == "hom_castling"));
    let o = owner_pos(&stage);
    assert!(d(o, before_hom) < 1.0, "the owner stands where it stood: {o:?}");
    let lure = hom(&stage).lure_id();
    let st = stage.horde().unwrap().store();
    assert!(st.alive().iter().filter(|&&s| st.taunter[s as usize] == lure).count() >= 3, "and the crowd turns on it");
}

// ---------------------------------------------------------------------------
// Plants and spheres
// ---------------------------------------------------------------------------

#[test]
fn flora_cycles_its_kinds_keeps_its_cap_and_withers() {
    let mut stage = stage(HomType::Lif);
    let o = owner(&stage);
    let kinds: Vec<Kind> = (0..4)
        .map(|_| {
            let Summoned::Unit(e) = stage.companions.summon(o, Summon::Flora, 2, 14.0, 14.0) else { panic!() };
            stage.companions.by_entity(e).unwrap().kind
        })
        .collect();
    assert_eq!(kinds, [Kind::Plant(PlantKind::Mandragora), Kind::Plant(PlantKind::Hydra), Kind::Plant(PlantKind::Mandragora), Kind::Plant(PlantKind::Hydra)]);
    assert_eq!(stage.companions.units.iter().filter(|c| matches!(c.kind, Kind::Plant(_))).count(), 3, "three alive at level 2");
    let p = stage.companions.units.iter().find(|c| matches!(c.kind, Kind::Plant(_))).unwrap();
    assert_eq!(p.max_hp, owner(&stage).max_hp * 20 / 100);
    run(&mut stage, (PLANT_SECS * 20.0) as u32 + 2);
    assert!(!stage.companions.units.iter().any(|c| matches!(c.kind, Kind::Plant(_))), "40 s and gone");
}

#[test]
fn a_hydra_shoots_what_comes_near() {
    let mut stage = stage(HomType::Lif);
    hom_mut(&mut stage).stance = Stance::Passive;
    let o = owner(&stage);
    stage.companions.summon(o, Summon::Flora, 2, 25.0, 15.0);
    let Summoned::Unit(hydra) = stage.companions.summon(o, Summon::Flora, 2, 25.0, 15.0) else { panic!() };
    let m = spawn(&mut stage, BLOB, 28.0, 15.0, 10_000);
    let out = run(&mut stage, 40);
    assert!(swings_by(&out, hydra).iter().any(|s| s.target == MONSTER_ENTITY_BASE + m as u32));
}

#[test]
fn a_sphere_mine_pulls_then_blows_up_on_its_fuse() {
    let mut stage = stage(HomType::Lif);
    hom_mut(&mut stage).stance = Stance::Passive;
    let o = owner(&stage);
    let Summoned::Unit(e) = stage.companions.summon(o, Summon::SphereMine, 3, 25.0, 10.5) else { panic!() };
    let lure = stage.companions.by_entity(e).unwrap().lure_id();
    let ms: Vec<u16> = (0..3).map(|i| spawn(&mut stage, RUNNER, 27.5, 9.5 + i as f32, 10_000)).collect();
    let boss = spawn(&mut stage, KING, 22.0, 10.5, 10_000);
    run(&mut stage, 3);
    let st = stage.horde().unwrap().store();
    assert!(ms.iter().all(|&s| st.taunter[s as usize] == lure), "taunted toward it");
    assert_ne!(st.taunter[boss as usize], lure, "never a boss");
    let out = run(&mut stage, (SPHERE_FUSE_SECS * 20.0) as u32);
    assert!(casts_by(&out, e).iter().any(|s| s == "sphere_mine_blast"));
    assert!(ms.iter().any(|&s| !hits_on(&out, MONSTER_ENTITY_BASE + s as u32).is_empty()), "the blast lands");
    run(&mut stage, 20);
    assert!(stage.companions.by_entity(e).is_none());
}

#[test]
fn a_sphere_mine_bitten_to_death_blows_up_at_once() {
    let mut stage = stage(HomType::Lif);
    hom_mut(&mut stage).stance = Stance::Passive;
    let o = owner(&stage);
    let Summoned::Unit(e) = stage.companions.summon(o, Summon::SphereMine, 1, 25.0, 10.5) else { panic!() };
    let m = spawn(&mut stage, BLOB, 26.0, 10.5, 10_000);
    run(&mut stage, 1);
    let i = stage.companions.units.iter().position(|c| c.entity() == e).unwrap();
    let tick = stage.tick;
    for _ in 0..4 {
        stage.companions.take(i, 3, tick);
    }
    let out = run(&mut stage, 1);
    assert!(casts_by(&out, e).iter().any(|s| s == "sphere_mine_blast"));
    assert!(!hits_on(&out, MONSTER_ENTITY_BASE + m as u32).is_empty());
}

#[test]
fn cultivation_is_the_callers_field_plant_and_the_pool_is_bounded() {
    let mut stage = stage(HomType::Lif);
    let o = owner(&stage);
    assert_eq!(stage.companions.summon(o, Summon::Cultivation, 1, 12.0, 12.0), Summoned::FieldPlant);
    for k in 0..CAPACITY + 5 {
        let o = Owner { uid: 100 + k as u32, ..o };
        stage.companions.summon(o, Summon::SphereMine, 1, 12.0, 12.0);
    }
    assert_eq!(stage.companions.units.len(), CAPACITY);
}

#[test]
fn heals_and_potions_reach_companions() {
    let mut stage = stage(HomType::Lif);
    let h = hom(&stage);
    hom_mut(&mut stage).hp = 1;
    let n = stage.companions.heal_in(&Area::Circle { x: h.x, y: h.y, r: 1.0 }, 5);
    assert_eq!((n, hom(&stage).hp), (1, 6));
    let e = stage.companions.pitch_target(1, h.x + 0.5, h.y, 1.0).unwrap();
    assert_eq!(stage.companions.heal_entity(e, 4), 4);
    let out = run(&mut stage, 1);
    assert_eq!(hits_on(&out, e).iter().filter(|x| x.flags & hit::HEAL != 0).count(), 2, "both heals are shown");
}

// ---------------------------------------------------------------------------
// Heals and potions pick homunculi as allies
// ---------------------------------------------------------------------------

/// Skills that mend an ally: Potion Pitcher, Heal (an area on the picked
/// ally, as baked) and a point heal (only the picked one), plus an ally buff
/// that must never pick a companion.
const MEND_BOOK: &str = r#"{"skills":[
  {"id":"potion_pitcher","name":"Potion Pitcher","class":"merchant","max_level":5,"target":"most_wounded_ally","range":6,"cooldown":0.2,
   "effects":[{"type":"pitch","mult":[1.5,0.1],"regen":2.0}]},
  {"id":"heal","name":"Heal","class":"acolyte","max_level":10,"target":"most_wounded_ally","range":10,"cooldown":0.2,
   "effects":[{"type":"heal","dice":"1d8","mult":[0,1],"radius":1.5}]},
  {"id":"mend","name":"Mend","class":"acolyte","max_level":10,"target":"most_wounded_ally","range":10,"cooldown":0.2,
   "effects":[{"type":"heal","dice":"1d8","mult":[0,1]}]}
]}"#;

/// The open field with [`MEND_BOOK`]: the Merchant (user 1, Potion Pitcher
/// on slot 0, two Red Potions) with its Lif out at (10.5, 10.5), and a
/// priest (user 2, Heal on slot 0, the point heal on slot 1) beside it.
fn mend_stage() -> HubInstance {
    let mut stage = field(&open_rows().iter().map(String::as_str).collect::<Vec<_>>());
    stage = stage.with_skills(Arc::new(SkillBook::from_json(MEND_BOOK).unwrap()));
    let mut out = Vec::new();
    let mut m = merchant(HomType::Lif, 25);
    m.loadout.skills.insert("potion_pitcher".into(), 5);
    m.loadout.hotbar[0] = "potion_pitcher".into();
    stage.join(m, Uuid::nil(), None, 0, &mut out);
    let mut loadout = Loadout::default();
    loadout.hotbar.resize(combat::skills::SLOTS, Default::default());
    for (i, id) in ["heal", "mend"].into_iter().enumerate() {
        loadout.skills.insert(id.into(), 10);
        loadout.hotbar[i] = id.into();
    }
    stage.join(PlayerProfile { archetype: "acolyte".into(), level: 30, loadout, ..profile(2) }, Uuid::nil(), None, 0, &mut out);
    put(&mut stage, 1, 10.5, 10.5);
    put(&mut stage, 2, 12.5, 10.5);
    stage.tick(&mut out);
    assert!(stage.companions.homunculus_of(1).is_some());
    let i = stage.find(1).unwrap();
    stage.players[i].fighter.as_mut().unwrap().potions = vec![("red_potion".into(), 2)];
    stage
}

fn vitals(stage: &mut HubInstance, user: u32) -> &mut Fighter {
    let i = stage.find(user).unwrap();
    stage.players[i].fighter.as_mut().unwrap()
}

/// `user` holds `slots` until a cast of theirs goes out (at most 6 ticks),
/// then a few more ticks let it land; everything sent.
fn press(stage: &mut HubInstance, user: u32, slots: u16) -> Outgoing {
    let mut out = Vec::new();
    let mut held = slots;
    for _ in 0..12 {
        stage.act(user, [0, 0], held);
        stage.tick(&mut out);
        if controls(&out, user).iter().any(|m| matches!(m, ServerMsg::Cast { caster, skill, .. } if *caster == 100 + user && !skill.is_empty())) {
            held = 0;
        }
    }
    stage.act(user, [0, 0], 0);
    out
}

fn used_items(out: &Outgoing) -> Vec<String> {
    out.iter()
        .filter_map(|(u, e)| match (*u, e) {
            (1, HubEvent::ItemUsed { item }) => Some(item.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn potion_pitcher_throws_at_the_most_wounded_homunculus() {
    let mut stage = mend_stage();
    // The owner scratched (99 %), its homunculus at a quarter.
    let max = vitals(&mut stage, 1).max_hp;
    vitals(&mut stage, 1).hp = max - 1;
    let hom_max = hom(&stage).max_hp;
    hom_mut(&mut stage).hp = hom_max / 4;
    let at = hom(&stage).x;
    let out = press(&mut stage, 1, 1);
    let e = hom(&stage).entity();
    assert_eq!(used_items(&out), vec!["red_potion".to_string()], "one potion thrown");
    assert!(hits_on(&out, e).iter().any(|h| h.flags & hit::HEAL != 0 && h.amount > 0), "it lands on the homunculus");
    assert!(vitals(&mut stage, 1).regen[0].is_none(), "not on the scratched owner");
    let cast = controls(&out, 1).into_iter().find_map(|m| match m {
        ServerMsg::Cast { caster: 101, skill, x, .. } if skill == "potion_pitcher" => Some(x),
        _ => None,
    });
    assert!(cast.is_some_and(|x| (x - at).abs() < 0.5), "aimed at the homunculus: {cast:?}, it stood at {at}");

    // A more wounded player still comes first.
    vitals(&mut stage, 1).hp = max / 5;
    hom_mut(&mut stage).hp = hom_max / 2;
    let out = press(&mut stage, 1, 1);
    assert_eq!(used_items(&out).len(), 1);
    assert!(vitals(&mut stage, 1).regen[0].is_some(), "the owner at 20 % is the most wounded");
}

#[test]
fn auto_pitch_on_a_homunculus_waits_for_the_potion_threshold() {
    let mut stage = mend_stage();
    let i = stage.find(1).unwrap();
    stage.auto_slots(1, 1);
    stage.players[i].uses.hp = 50;
    let hom_max = hom(&stage).max_hp;
    hom_mut(&mut stage).hp = hom_max * 7 / 10;
    assert!(used_items(&press(&mut stage, 1, 0)).is_empty(), "70 % is above the 50 % threshold");
    hom_mut(&mut stage).hp = hom_max * 3 / 10;
    // The AI throws by itself (again once its pace allows, while still low).
    assert_eq!(used_items(&press(&mut stage, 1, 0)).first().map(String::as_str), Some("red_potion"));
}

#[test]
fn a_priest_heals_a_party_members_homunculus() {
    let mut stage = mend_stage();
    // Both players unhurt: before, Heal found no one to cast on.
    let hom_max = hom(&stage).max_hp;
    hom_mut(&mut stage).hp = 5;
    let out = press(&mut stage, 2, 1);
    let e = hom(&stage).entity();
    assert!(controls(&out, 2).iter().any(|m| matches!(m, ServerMsg::Cast { caster: 102, skill, .. } if skill == "heal")), "Heal is cast");
    assert!(hits_on(&out, e).iter().any(|h| h.flags & hit::HEAL != 0 && h.amount > 0), "Heal mends the homunculus");
    assert!(hom(&stage).hp > 5);

    // A heal with no area (only the picked ally) follows the homunculus
    // and lands on it.
    hom_mut(&mut stage).hp = hom_max / 4;
    let before = hom(&stage).hp;
    let out = press(&mut stage, 2, 2);
    let follow = controls(&out, 2).into_iter().find_map(|m| match m {
        ServerMsg::Cast { caster: 102, skill, areas, .. } if skill == "mend" => areas.first().map(|a| a.follow),
        _ => None,
    });
    assert_eq!(follow, Some(e), "its marker follows the homunculus");
    assert!(hits_on(&out, e).iter().any(|h| h.flags & hit::HEAL != 0 && h.amount > 0));
    assert!(hom(&stage).hp > before);
}

// ---------------------------------------------------------------------------
// Town: it follows, nothing else
// ---------------------------------------------------------------------------

fn snapshot_ids(out: &Outgoing, user: u32) -> Vec<u32> {
    out.iter()
        .rev()
        .find_map(|(u, e)| match e {
            HubEvent::Snapshot(b) if *u == user => crate::world::protocol::Snapshot::decode(b),
            _ => None,
        })
        .map(|s| s.entities.iter().map(|e| e.0).collect())
        .unwrap_or_default()
}

#[test]
fn in_town_it_follows_its_owner_is_seen_by_everyone_and_never_fights() {
    let world = Arc::new(super::super::test_support::test_world());
    let mut town = HubInstance::new(1, world, super::super::tests::quiet());
    let mut out = Vec::new();
    town.join(merchant(HomType::Amistr, 25), Uuid::nil(), None, 0, &mut out);
    // The street (map 0; the profile came in indoors).
    let i = town.find(1).unwrap();
    town.players[i].map = 0;
    put(&mut town, 1, 3.5, 3.5);
    let out = run(&mut town, 2);
    let c = hom(&town);
    assert_eq!(c.map, town.players[i].map, "on its owner's map");
    assert_eq!(c.hp, c.max_hp, "whole in town");
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::Companions { list } if list.iter().any(|r| r.entity_id == c.entity()))));
    assert!(snapshot_ids(&out, 1).contains(&c.entity()), "it walks in the player rows");
    // The owner walks off east: it follows.
    town.players[i].facing = 2;
    put(&mut town, 1, 7.5, 3.5);
    run(&mut town, 40);
    let c = hom(&town);
    assert!(d((c.x, c.y), owner_pos(&town)) < NEAR, "beside its owner, at {:?}", (c.x, c.y));
    // Another player arriving gets its record and sees it in snapshots.
    let mut out = Vec::new();
    town.join(profile(2), Uuid::nil(), None, 0, &mut out);
    assert!(controls(&out, 2).iter().any(|m| matches!(m, ServerMsg::Companions { list } if list.iter().any(|r| r.entity_id == c.entity()))));
    let out = run(&mut town, 2);
    assert!(snapshot_ids(&out, 2).contains(&c.entity()));
    assert!(!controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::Combat { .. } | ServerMsg::Cast { .. })), "no fighting in town");
    // Indoors (another map of the town): it goes along, gone from the street.
    let house = 1;
    town.players[i].map = house;
    put(&mut town, 1, 3.5, 3.5);
    let out = run(&mut town, 3);
    assert_eq!(hom(&town).map, house);
    assert!(controls(&out, 2).iter().any(|m| matches!(m, ServerMsg::Companions { list } if list.iter().any(|r| r.flags & companion_flags::GONE != 0))), "the street saw it go");
    assert_eq!(town.companions.units().len(), 1, "never twice");
    // Rest: it goes; and it leaves with its owner.
    let mut out = Vec::new();
    town.companion_cmd(1, CompanionCmd::Rest, &mut out);
    run(&mut town, 1);
    assert!(town.companions.homunculus_of(1).is_none());
    town.companion_cmd(1, CompanionCmd::Rest, &mut out);
    run(&mut town, 1);
    assert!(town.companions.homunculus_of(1).is_some());
    let mut out = Vec::new();
    town.leave(1, Uuid::nil(), &mut out);
    run(&mut town, 1);
    assert!(town.companions.units().is_empty());
}

// ---------------------------------------------------------------------------
// The town skill bar (`TOWN_SKILL`)
// ---------------------------------------------------------------------------

const TOWN_BOOK: &str = r#"{"skills":[
  {"id":"pharmacy","name":"Pharmacy","class":"merchant","max_level":5,"passive":true,"town":true,"effects":[{"type":"pharmacy"}]},
  {"id":"call_homunculus","name":"Call Homunculus","class":"merchant","target":"self","town":true,"auto":false,
   "effects":[{"type":"summon","what":"homunculus"}]},
  {"id":"peco_ride","name":"Peco Peco Ride","class":"swordsman","target":"self","town":true,"effects":[{"type":"mount"}]},
  {"id":"summon_flora","name":"Summon Flora","class":"merchant","target":"aim","range":6,"effects":[{"type":"summon","what":"flora"}]},
  {"id":"rogue","name":"Marked but no town use","class":"merchant","target":"self","town":true,
   "effects":[{"type":"damage","dice":"1d6","radius":2,"at":"caster"}]}
]}"#;

#[test]
fn only_the_marked_town_skills_have_a_town_use() {
    use crate::world::combat::skills::TownUse;
    let book = SkillBook::from_json(TOWN_BOOK).unwrap();
    let town = |id: &str| book.find(id).unwrap().1.town_use();
    assert_eq!(town("pharmacy"), Some(TownUse::Pharmacy));
    assert_eq!(town("call_homunculus"), Some(TownUse::Homunculus));
    assert_eq!(town("peco_ride"), Some(TownUse::Mount));
    assert_eq!(town("summon_flora"), None, "not marked: refused in town");
    assert_eq!(town("rogue"), None, "marked, but nothing it does works in town");
}

#[test]
fn town_skills_open_the_pharmacy_and_call_the_homunculus_in_town_only() {
    use crate::world::combat::skills::TownUse;
    let world = Arc::new(super::super::test_support::test_world());
    let mut town = HubInstance::new(1, world, super::super::tests::quiet()).with_skills(Arc::new(SkillBook::from_json(TOWN_BOOK).unwrap()));
    let mut p = PlayerProfile { homunculus: None, ..merchant(HomType::Lif, 1) };
    p.loadout.skills.insert("pharmacy".into(), 1);
    let mut out = Vec::new();
    town.join(p, Uuid::nil(), None, 0, &mut out);
    let mut out = Vec::new();
    town.town_skill(1, TownUse::Pharmacy, &mut out);
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::Recipes { npc, .. } if npc == "pharmacy")), "{out:?}");
    // No type yet: the panel says so (the client opens the chooser).
    let mut out = Vec::new();
    town.town_skill(1, TownUse::Homunculus, &mut out);
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::CompanionState(s) if s.hom_type == hom_type::NONE)));
    town.companion_cmd(1, CompanionCmd::Choose(hom_type::LIF), &mut out);
    run(&mut town, 1);
    assert!(town.companions.homunculus_of(1).is_some(), "out beside its owner");
    // The bar's key again: to rest; again: back out.
    town.town_skill(1, TownUse::Homunculus, &mut out);
    run(&mut town, 1);
    assert!(town.companions.homunculus_of(1).is_none());
    town.town_skill(1, TownUse::Homunculus, &mut out);
    run(&mut town, 1);
    assert!(town.companions.homunculus_of(1).is_some());
    // On a stage: refused.
    let mut stage = stage(HomType::Lif);
    let mut out = Vec::new();
    stage.town_skill(1, TownUse::Pharmacy, &mut out);
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::HubError { code, .. } if code == "not_in_town")));
}

// ---------------------------------------------------------------------------
// Boss beats
// ---------------------------------------------------------------------------

#[test]
fn a_boss_beat_lands_on_the_companions_inside_it() {
    // The Angeling's scripted fight (hub/boss/scripts.rs): its first beat
    // is a holy bolt at each player.
    let rows = open_rows();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let catalog = Arc::new(MonsterCatalog::new(vec![test_def("angeling", 2.0, 50, 0.0, true)]).unwrap());
    let cfg = HordeConfig { light_radius: 30.0, director_enabled: false, seed: Some(7), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde);
    stage.dice = Rng::new(7);
    let mut out = Vec::new();
    stage.join(PlayerProfile { level: 40, ..merchant(HomType::Amistr, 35) }, Uuid::nil(), None, 0, &mut out);
    put(&mut stage, 1, 20.5, 10.5);
    {
        let i = stage.find(1).unwrap();
        let f = stage.players[i].fighter.as_mut().unwrap();
        (f.max_hp, f.hp) = (1_000_000, 1_000_000);
        f.recovery = u32::MAX;
    }
    // A plant rooted on the player's spot: whatever lands on them lands on it.
    let at = owner_pos(&stage);
    let Summoned::Unit(plant) = stage.companions.summon(owner(&stage), Summon::Flora, 1, at.0, at.1) else { panic!("planted") };
    let k = stage.horde_mut().unwrap().catalog().kind_of("angeling").unwrap();
    stage.horde_mut().unwrap().spawn(k, 26.0, 10.5).unwrap();
    let out = run(&mut stage, 60);
    assert!(controls(&out, 1).iter().any(|m| matches!(m, ServerMsg::BossZones { .. })), "the boss telegraphs");
    let hits = hits_on(&out, plant);
    assert!(hits.iter().any(|h| h.amount > 0 && h.flags >> 4 == DamageType::Radiant as u8), "the holy bolt hit the plant: {hits:?}");
}
