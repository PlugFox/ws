//! Who a skill picks (COMBAT.md §4.3): support skills go to the nearest
//! member still without their buff, auto-cast fires only at a target that
//! needs it, manual aiming follows the pointer, travel skills go where the
//! player walks or points.

use super::fight::Fighter;
use super::test_support::{map, portal};
use super::tests::profile;
use super::*;
use crate::world::combat::skills::tests::book;
use crate::world::combat::{self, Loadout, Mods};
use crate::world::horde::catalog::{test_def, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use std::sync::Arc;
use uuid::Uuid;

/// A 20 × 9 field (cells 1..21 × 1..10) on the test skill table; player 1,
/// a level-10 `archetype`, joins with `skills` slotted in order, then
/// `allies` level-10 mages (users 2, 3, ...).
fn stage(archetype: &str, skills: &[(&str, u8)], allies: u32) -> HubInstance {
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
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(Arc::new(book()));
    stage.dice = combat::Rng::new(5);
    let mut loadout = Loadout::default();
    loadout.hotbar.resize(combat::skills::SLOTS, Default::default());
    for (i, (id, lvl)) in skills.iter().enumerate() {
        loadout.skills.insert(id.to_string(), *lvl);
        loadout.hotbar[i] = id.to_string().into();
    }
    let mut out = Vec::new();
    stage.join(PlayerProfile { archetype: archetype.into(), level: 10, loadout, ..profile(1) }, Uuid::nil(), None, 0, &mut out);
    for u in 2..2 + allies {
        stage.join(PlayerProfile { archetype: "mage".into(), level: 10, ..profile(u) }, Uuid::nil(), None, 0, &mut out);
    }
    stage
}

fn put(stage: &mut HubInstance, user: u32, x: f32, y: f32) {
    let i = stage.find(user).unwrap();
    (stage.players[i].x, stage.players[i].y) = (x, y);
}

fn fighter(stage: &HubInstance, user: u32) -> &Fighter {
    stage.players[stage.find(user).unwrap()].fighter.as_ref().unwrap()
}

fn fighter_mut(stage: &mut HubInstance, user: u32) -> &mut Fighter {
    let i = stage.find(user).unwrap();
    stage.players[i].fighter.as_mut().unwrap()
}

fn skill(stage: &HubInstance, id: &str) -> u16 {
    stage.skills.find(id).unwrap().0
}

/// Ticks left on `user`'s buff from skill `id`.
fn buff_left(stage: &HubInstance, user: u32, id: &str) -> u32 {
    fighter(stage, user).buff_left(skill(stage, id), stage.tick)
}

/// Gives `user` the buff of skill `id` for `secs`.
fn give(stage: &mut HubInstance, user: u32, id: &str, secs: f32) {
    let (s, until) = (skill(stage, id), stage.tick + (secs * stage.config.tick_hz as f32) as u32);
    fighter_mut(stage, user).buff(s, until, Mods::default(), 0);
}

/// One tick with `slots` pressed (aim in 1/8 m); the casts it announced:
/// `(skill, x, y)`.
fn press(stage: &mut HubInstance, aim: [i8; 2], slots: u16) -> Vec<(String, f32, f32)> {
    let mut out = Vec::new();
    stage.act(1, aim, slots);
    stage.tick(&mut out);
    out.iter()
        .filter(|(u, _)| *u == 1)
        .filter_map(|(_, e)| match e {
            HubEvent::Control(ServerMsg::Cast { caster: 101, skill, x, y, .. }) if !skill.is_empty() => Some((skill.clone(), *x, *y)),
            _ => None,
        })
        .collect()
}

fn auto(stage: &mut HubInstance, mask: u16) {
    stage.auto_slots(1, mask);
}

fn manual(stage: &mut HubInstance, on: bool) {
    let ai = stage.players[0].prefs.ai;
    stage.cast_prefs(1, CastPrefs { manual_aim: on, ai });
}

#[test]
fn a_party_buff_goes_to_the_nearest_member_without_it() {
    let mut stage = stage("acolyte", &[("haste", 2)], 2);
    put(&mut stage, 1, 4.5, 5.5);
    put(&mut stage, 2, 7.5, 5.5);
    put(&mut stage, 3, 12.5, 5.5);
    give(&mut stage, 1, "haste", 20.0);
    give(&mut stage, 2, "haste", 20.0);
    let casts = press(&mut stage, [0, 0], 1);
    assert_eq!(casts.len(), 1);
    assert!((casts[0].1 - 12.5).abs() < 1e-3, "aimed at the unbuffed member 3: {casts:?}");
    assert!(buff_left(&stage, 3, "haste") > 0, "member 3 got it");
}

#[test]
fn the_caster_is_a_candidate_and_nearest_of_all() {
    let mut stage = stage("acolyte", &[("haste", 2)], 1);
    put(&mut stage, 1, 4.5, 5.5);
    put(&mut stage, 2, 7.5, 5.5);
    let casts = press(&mut stage, [0, 0], 1);
    assert!((casts[0].1 - 4.5).abs() < 1e-3, "nobody has it: the caster first, {casts:?}");
    // Member 2 stood inside the buff's radius around the caster.
    assert!(buff_left(&stage, 1, "haste") > 0 && buff_left(&stage, 2, "haste") > 0);
}

#[test]
fn auto_cast_waits_while_everyone_has_the_buff_and_renews_it_before_it_ends() {
    let mut stage = stage("acolyte", &[("haste", 2)], 1);
    put(&mut stage, 1, 4.5, 5.5);
    put(&mut stage, 2, 7.5, 5.5);
    auto(&mut stage, 1);
    give(&mut stage, 1, "haste", 10.0);
    give(&mut stage, 2, "haste", 10.0);
    assert!(press(&mut stage, [0, 0], 0).is_empty(), "everyone has it: no auto-cast");
    // Member 2's runs out within the refresh window: renewed on them.
    give(&mut stage, 1, "haste", 30.0);
    let i2 = stage.find(2).unwrap();
    let mut f2 = stage.players[i2].fighter.take().unwrap();
    f2.refresh(u32::MAX); // drop the 10 s buff
    stage.players[i2].fighter = Some(f2);
    give(&mut stage, 2, "haste", 2.0);
    let casts = press(&mut stage, [0, 0], 0);
    assert_eq!(casts.len(), 1, "about to expire counts as missing");
    assert!((casts[0].1 - 7.5).abs() < 1e-3, "renewed on member 2: {casts:?}");
    assert!(buff_left(&stage, 2, "haste") > 5 * stage.config.tick_hz);
}

#[test]
fn a_press_by_hand_renews_a_party_buff_everyone_has() {
    let mut stage = stage("acolyte", &[("haste", 2)], 1);
    put(&mut stage, 1, 4.5, 5.5);
    put(&mut stage, 2, 7.5, 5.5);
    give(&mut stage, 1, "haste", 8.0);
    give(&mut stage, 2, "haste", 8.0);
    let casts = press(&mut stage, [0, 0], 1);
    assert!((casts[0].1 - 4.5).abs() < 1e-3, "renewed around the caster: {casts:?}");
    assert!(buff_left(&stage, 2, "haste") > 9 * stage.config.tick_hz, "10 s again");
}

#[test]
fn auto_cast_keeps_a_self_buff_up_without_recasting_it_early() {
    let mut stage = stage("swordsman", &[("endure", 1)], 0);
    auto(&mut stage, 1);
    assert_eq!(press(&mut stage, [0, 0], 0).len(), 1, "not up yet: the AI casts it");
    for _ in 0..40 {
        assert!(press(&mut stage, [0, 0], 0).is_empty(), "up: auto-cast waits");
    }
    auto(&mut stage, 0);
    assert_eq!(press(&mut stage, [0, 0], 1).len(), 1, "a press by hand recasts it");
}

#[test]
fn heal_takes_the_lowest_hp_share_and_auto_cast_leaves_scratches() {
    let mut stage = stage("acolyte", &[("mending", 2)], 2);
    put(&mut stage, 1, 4.5, 5.5);
    put(&mut stage, 2, 7.5, 5.5);
    put(&mut stage, 3, 12.5, 5.5);
    let max2 = fighter(&stage, 2).max_hp;
    let max3 = fighter(&stage, 3).max_hp;
    fighter_mut(&mut stage, 2).hp = max2 - 1;
    fighter_mut(&mut stage, 3).hp = max3 - 1;
    auto(&mut stage, 1);
    assert!(press(&mut stage, [0, 0], 0).is_empty(), "a scratch is no reason to auto-heal");
    fighter_mut(&mut stage, 3).hp = max3 / 3;
    fighter_mut(&mut stage, 2).hp = max2 / 2;
    let casts = press(&mut stage, [0, 0], 0);
    assert!((casts[0].1 - 12.5).abs() < 1e-3, "the lowest share first: {casts:?}");
}

#[test]
fn manual_aiming_picks_the_monster_nearest_the_pointer() {
    let mut stage = stage("mage", &[("chill", 1)], 0);
    put(&mut stage, 1, 4.5, 5.5);
    let h = stage.horde_mut().unwrap();
    h.spawn(0, 6.5, 5.5).unwrap(); // next to the caster
    h.spawn(0, 12.5, 5.5).unwrap(); // under the pointer
    let pointer = [(8.0 * 4.0) as i8, 0]; // 8 cells east
    let casts = press(&mut stage, pointer, 1);
    assert!(casts[0].1 < 9.5, "automatic: the nearest to the caster, {casts:?}");
    manual(&mut stage, true);
    stage.tick(&mut Vec::new());
    let casts = press(&mut stage, pointer, 1);
    assert!(casts[0].1 > 9.5, "manual: the nearest to the pointer, {casts:?}");
    // An auto-cast slot still aims itself.
    auto(&mut stage, 1);
    fighter_mut(&mut stage, 1).cooldowns.fill(0);
    let casts = press(&mut stage, pointer, 0);
    assert!(casts[0].1 < 9.5, "auto-cast: automatic, {casts:?}");
}

#[test]
fn manual_aiming_lays_ground_skills_at_the_pointer_within_range() {
    let mut stage = stage("mage", &[("swamp", 1)], 0);
    put(&mut stage, 1, 4.5, 5.5);
    stage.horde_mut().unwrap().spawn(0, 6.5, 5.5).unwrap();
    manual(&mut stage, true);
    let casts = press(&mut stage, [12, 4], 1); // 3 cells east, 1 north
    assert!((casts[0].1 - 7.5).abs() < 1e-3 && (casts[0].2 - 6.5).abs() < 1e-3, "on the pointer: {casts:?}");
    let far = press(&mut stage, [127, 0], 1); // past the 20-cell range
    assert!((far[0].1 - 24.5).abs() < 1e-3, "clamped to the range: {far:?}");
    // Without a pointer (gamepad, touch) it aims itself as before.
    let casts = press(&mut stage, [0, 0], 1);
    assert!((casts[0].1 - 6.5).abs() < 1.0, "automatic without a pointer: {casts:?}");
}

#[test]
fn travel_goes_where_the_player_walks_else_where_they_point() {
    let mut stage = stage("swordsman", &[("slide", 1)], 0);
    let travel = 1 << super::cast::TRAVEL_SLOT;
    // Walking north, pointing east: north.
    put(&mut stage, 1, 6.5, 2.5);
    stage.input(1, 1, 0.0, 1.0);
    press(&mut stage, [16, 0], travel);
    let (_, x, y) = stage.position_of(1).unwrap();
    assert!(y > 2.5 + 4.0 && (x - 6.5).abs() < 0.01, "slid north along the walk: ({x}, {y})");
    // Standing, pointing east: east.
    fighter_mut(&mut stage, 1).cooldowns.fill(0);
    stage.input(1, 2, 0.0, 0.0);
    put(&mut stage, 1, 6.5, 5.5);
    press(&mut stage, [16, 0], travel);
    let (_, x, y) = stage.position_of(1).unwrap();
    assert!(x > 6.5 + 4.0 && (y - 5.5).abs() < 0.01, "slid east toward the pointer: ({x}, {y})");
    assert_eq!(stage.players[0].facing, 2, "faces where it went");
}

#[test]
fn a_press_keeps_its_aim_when_a_later_frame_lands_in_the_same_tick() {
    let mut stage = stage("swordsman", &[("slide", 1)], 0);
    put(&mut stage, 1, 6.5, 5.5);
    // The press, then the next frame (no press, no aim) before the tick.
    stage.act(1, [16, 0], 1 << super::cast::TRAVEL_SLOT);
    stage.act(1, [0, 0], 0);
    stage.tick(&mut Vec::new());
    let (_, x, y) = stage.position_of(1).unwrap();
    assert!(x > 6.5 + 4.0 && (y - 5.5).abs() < 0.01, "slid east toward the pointer: ({x}, {y})");
}

#[test]
fn auto_cast_aims_itself_while_a_press_by_hand_follows_the_pointer() {
    let mut stage = stage("archer", &[("arrow", 1)], 0);
    put(&mut stage, 1, 10.5, 5.5);
    stage.horde_mut().unwrap().spawn(0, 13.5, 5.5).unwrap(); // east
    let west = [-32, 0]; // pointing away from it
    let casts = press(&mut stage, west, 1);
    assert!(casts[0].1 < 10.5, "by hand: toward the pointer, {casts:?}");
    fighter_mut(&mut stage, 1).cooldowns.fill(0);
    manual(&mut stage, true);
    auto(&mut stage, 1);
    fighter_mut(&mut stage, 1).cooldowns.fill(0);
    let casts = press(&mut stage, west, 0);
    assert!(casts[0].1 > 10.5, "auto-cast: toward the monster, wherever the mouse is, {casts:?}");
}

/// Player 1, a mage with Chill and Homing chained on slot 1 (the hotbar's
/// first), a tough monster in reach.
fn chained() -> HubInstance {
    let mut stage = stage("mage", &[("chill", 1), ("homing", 1)], 0);
    put(&mut stage, 1, 4.5, 5.5);
    let h = stage.horde_mut().unwrap();
    let m = h.spawn(0, 7.5, 5.5).unwrap();
    h.store_mut().hp[m as usize] = 1_000_000;
    let mut l = stage.players[0].profile.loadout.clone();
    stage.skills.set_chain(&mut l, 0, &["chill".into(), "homing".into()]).unwrap();
    stage.set_loadout(1, l, &mut Vec::new());
    stage
}

/// Ready to cast again at once: no cooldowns, no wind-up, full SP.
fn rested(stage: &mut HubInstance) {
    let f = fighter_mut(stage, 1);
    f.cooldowns.fill(0);
    f.casting = None;
    f.sp = f.max_sp;
}

/// Keys up long enough to count as released (a one-tick gap is not).
fn release(stage: &mut HubInstance) {
    press(stage, [0, 0], 0);
    press(stage, [0, 0], 0);
}

fn names(casts: &[(String, f32, f32)]) -> Vec<&str> {
    casts.iter().map(|c| c.0.as_str()).collect()
}

#[test]
fn a_chain_casts_its_next_skill_on_each_press() {
    let mut stage = chained();
    let mut seen = Vec::new();
    for _ in 0..3 {
        rested(&mut stage);
        seen.extend(press(&mut stage, [0, 0], 1).into_iter().map(|c| c.0));
        release(&mut stage);
    }
    assert_eq!(seen, ["chill", "homing", "chill"], "1 → 2 → back to 1");
}

#[test]
fn a_chain_skips_what_cannot_be_cast() {
    let mut stage = chained();
    rested(&mut stage);
    let chill = skill(&stage, "chill");
    fighter_mut(&mut stage, 1).cooldowns[chill as usize] = 1000;
    assert_eq!(names(&press(&mut stage, [0, 0], 1)), ["homing"], "Chill cooling: Homing instead");
    release(&mut stage);
    rested(&mut stage);
    assert_eq!(names(&press(&mut stage, [0, 0], 1)), ["chill"], "the cursor went past Homing");
}

#[test]
fn a_held_chain_rotates_at_its_own_pace_and_a_pause_starts_it_over() {
    let mut stage = chained();
    let hz = stage.tick_hz();
    let mut seen = Vec::new();
    // Held for one second: a cast on the press, then one per repeat gap.
    for _ in 0..hz {
        rested(&mut stage);
        seen.extend(press(&mut stage, [0, 0], 1).into_iter().map(|c| c.0));
    }
    let repeat = (0.3 * hz as f32).round() as usize;
    assert_eq!(seen.len(), (hz as usize - 1) / repeat + 1, "{seen:?}");
    assert!(seen.windows(2).all(|w| w[0] != w[1]), "it alternates: {seen:?}");
    // Released after Chill: three seconds later the chain starts at Chill again.
    while seen.last().map(String::as_str) != Some("chill") {
        rested(&mut stage);
        seen.extend(press(&mut stage, [0, 0], 1).into_iter().map(|c| c.0));
    }
    for _ in 0..3 * hz + 1 {
        press(&mut stage, [0, 0], 0);
    }
    rested(&mut stage);
    assert_eq!(names(&press(&mut stage, [0, 0], 1)), ["chill"]);
}

#[test]
fn the_ai_casts_auto_skills_off_the_hotbar_only_while_it_is_on() {
    let mut stage = stage("mage", &[("chill", 1)], 0);
    put(&mut stage, 1, 4.5, 5.5);
    let h = stage.horde_mut().unwrap();
    let m = h.spawn(0, 7.5, 5.5).unwrap();
    h.store_mut().hp[m as usize] = 1_000_000;
    // Chill auto-cast but on no slot.
    let mut l = stage.players[0].profile.loadout.clone();
    stage.skills.set_slot(&mut l, 0, "").unwrap();
    stage.skills.set_auto(&mut l, &["chill".into()]).unwrap();
    stage.set_loadout(1, l, &mut Vec::new());
    for _ in 0..10 {
        assert!(press(&mut stage, [0, 0], 0).is_empty(), "the AI is off");
    }
    stage.cast_prefs(1, CastPrefs { manual_aim: false, ai: true });
    assert_eq!(names(&press(&mut stage, [0, 0], 0)), ["chill"], "on: it casts by itself");
    // Its pace: not again at once, even when ready.
    rested(&mut stage);
    assert!(press(&mut stage, [0, 0], 0).is_empty(), "the AI keeps a gap between casts");
}

#[test]
fn a_cast_by_hand_gives_the_player_the_turn() {
    let mut stage = stage("mage", &[("chill", 1), ("homing", 1)], 0);
    put(&mut stage, 1, 4.5, 5.5);
    let h = stage.horde_mut().unwrap();
    let m = h.spawn(0, 7.5, 5.5).unwrap();
    h.store_mut().hp[m as usize] = 1_000_000;
    let mut l = stage.players[0].profile.loadout.clone();
    stage.skills.set_auto(&mut l, &["chill".into()]).unwrap();
    stage.set_loadout(1, l, &mut Vec::new());
    stage.cast_prefs(1, CastPrefs { manual_aim: false, ai: true });
    rested(&mut stage);
    assert_eq!(names(&press(&mut stage, [0, 0], 0b10)), ["homing"], "the press goes first");
    let hz = stage.tick_hz();
    let mut quiet = 0;
    loop {
        rested(&mut stage);
        if !press(&mut stage, [0, 0], 0).is_empty() {
            break;
        }
        quiet += 1;
        assert!(quiet < hz, "the AI comes back");
    }
    assert!(quiet as f32 >= 0.4 * hz as f32, "quiet for about half a second: {quiet} ticks");
}

#[test]
fn a_held_chain_keeps_its_pace_when_input_frames_skip_ticks() {
    let mut stage = chained();
    let hz = stage.tick_hz();
    let mut seen = Vec::new();
    // Held for a second, but every third tick gets no input frame.
    for t in 0..hz {
        rested(&mut stage);
        let slots = if t % 3 == 2 { 0 } else { 1 };
        seen.extend(press(&mut stage, [0, 0], slots).into_iter().map(|c| c.0));
    }
    let repeat = (0.3 * hz as f32).round() as usize;
    assert!(seen.len() <= (hz as usize - 1) / repeat + 1, "no extra presses from gaps: {seen:?}");
}

#[test]
fn a_cancelled_wind_up_gives_the_chain_step_back() {
    let mut stage = chained();
    rested(&mut stage);
    let casts = press(&mut stage, [0, 0], 1);
    assert_eq!(names(&casts), ["chill"]);
    if fighter(&stage, 1).casting.is_some() {
        // Walking cancels the wind-up: Chill is next again.
        let i = stage.find(1).unwrap();
        stage.players[i].dir = (1.0, 0.0);
        stage.tick(&mut Vec::new());
        stage.players[i].dir = (0.0, 0.0);
        assert_eq!(fighter(&stage, 1).hotbar[0].cursor, 0);
    }
}
