//! Monster affixes on a stage (`affix.rs`): each affix's effect on the
//! server, the death blasts' telegraphs and their dodge, and what a
//! starred kill pays.

use super::affix::{self, *};
use super::test_support::{map, portal};
use super::tests::profile;
use super::*;
use crate::world::combat::affixes::{AffixFlags, AffixReward};
use crate::world::combat::{self, Condition, DamageType};
use crate::world::horde::catalog::{test_def, MonsterCatalog};
use crate::world::horde::storage::{AFFIX_FRENZIED, AFFIX_SHIELD_BROKEN, AFFIX_SPLIT, FLAG_MINION};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{AffixEvent, MONSTER_ENTITY_BASE};
use std::sync::Arc;
use uuid::Uuid;

/// A 30 × 9 field (cells 1..31 × 1..10) at location level 40 (every
/// affix may roll), a level-10 swordsman (user 1) at `(3.5, 5.5)`;
/// monsters are slow blobs (kind 0) and chasers (kind 1).
fn stage() -> HubInstance {
    let row = format!("#{}#", ".".repeat(30));
    let wall = "#".repeat(32);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 9));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let mut chaser = test_def("imp", 2.0, 1, 0.0, false);
    chaser.archetype = combat::Archetype::Chaser;
    let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 0.01, 1, 0.0, false), chaser]).unwrap());
    let mut cfg = HordeConfig { light_radius: 20.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    cfg.waves.levels = (40, 40);
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde);
    stage.dice = combat::Rng::new(5);
    let mut out = Vec::new();
    stage.join(PlayerProfile { archetype: "swordsman".into(), level: 10, ..profile(1) }, Uuid::nil(), None, 0, &mut out);
    put(&mut stage, 3.5, 5.5);
    stage
}

fn put(stage: &mut HubInstance, x: f32, y: f32) {
    let i = stage.find(1).unwrap();
    (stage.players[i].x, stage.players[i].y) = (x, y);
}

fn me(stage: &mut HubInstance) -> &mut fight::Fighter {
    let i = stage.find(1).unwrap();
    stage.players[i].fighter.as_mut().unwrap()
}

/// A monster of `kind` at `(x, y)` with `flags`, HP topped up.
fn mob(stage: &mut HubInstance, kind: u8, x: f32, y: f32, flags: AffixFlags) -> u16 {
    let h = stage.horde_mut().unwrap();
    let s = h.spawn(kind, x, y).unwrap();
    h.set_affixes(s, flags);
    s
}

fn hp(stage: &HubInstance, s: u16) -> i32 {
    stage.horde().unwrap().store().hp[s as usize]
}

/// Ticks `secs` seconds; the player walks into the west wall (so it never
/// swings) unless `fight`. Returns everything sent.
fn run(stage: &mut HubInstance, secs: f32, fight: bool) -> Outgoing {
    let mut out = Vec::new();
    let n = (secs * stage.config.tick_hz as f32).round() as u32;
    for k in 0..n {
        if !fight {
            stage.input(1, k + 1, -1.0, 0.0);
            put(stage, 1.45, stage.players[stage.find(1).unwrap()].y);
        }
        stage.tick(&mut out);
    }
    out
}

fn events(out: &Outgoing) -> Vec<AffixEvent> {
    out.iter()
        .filter(|(u, _)| *u == 1)
        .filter_map(|(_, e)| match e {
            HubEvent::Control(ServerMsg::AffixEvents { events }) => Some(events.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn zones(out: &Outgoing, ability: &str) -> usize {
    out.iter()
        .filter(|(u, e)| *u == 1 && matches!(e, HubEvent::Control(ServerMsg::BossZones { ability: a, .. }) if a == ability))
        .count()
}

#[test]
fn armour_cuts_physical_damage_and_the_shield_soaks_then_recharges() {
    let mut st = stage();
    let s = mob(&mut st, 0, 25.5, 5.5, AffixFlags::ARMOURED);
    let h = st.horde_mut().unwrap();
    assert_eq!(absorb(h, s, 10, DamageType::Physical), 6, "40% less physical");
    assert_eq!(absorb(h, s, 10, DamageType::Fire), 10, "magic gets through");
    let sh = mob(&mut st, 0, 25.5, 7.5, AffixFlags::SHIELDED);
    let h = st.horde_mut().unwrap();
    let full = h.store().shield[sh as usize];
    assert!(full > 0);
    assert_eq!(absorb(h, sh, full - 1, DamageType::Physical), 0, "the shield takes it all");
    assert_eq!(absorb(h, sh, 5, DamageType::Physical), 4, "the rest goes through");
    assert!(h.store().affix_state[sh as usize] & AFFIX_SHIELD_BROKEN != 0);
    // Five quiet seconds and it is back, and the clients hear of it.
    let out = run(&mut st, 6.5, false);
    let h = st.horde().unwrap();
    assert_eq!(h.store().shield[sh as usize], full);
    assert_eq!(h.store().affix_state[sh as usize] & AFFIX_SHIELD_BROKEN, 0);
    assert!(out.iter().any(|(_, e)| matches!(e, HubEvent::Control(ServerMsg::MonsterAffixes { monsters }) if monsters.iter().any(|m| m.slot == sh && m.state == 0))));
}

#[test]
fn regeneration_heals_unless_burning_or_poisoned() {
    let mut st = stage();
    let s = mob(&mut st, 0, 25.5, 5.5, AffixFlags::REGENERATING);
    let max = st.horde().unwrap().block(s).hp;
    st.horde_mut().unwrap().store_mut().hp[s as usize] = max / 2;
    run(&mut st, 3.0, false);
    let healed = hp(&st, s) - max / 2;
    assert!(healed >= 2 * (max * 20 / 1000).max(1), "2% a second: {healed} of {max}");
    st.horde_mut().unwrap().store_mut().afflict(s, Condition::Poisoned, 200);
    let before = hp(&st, s);
    run(&mut st, 3.0, false);
    assert!(hp(&st, s) <= before, "poison stops it");
}

#[test]
fn vampiric_heals_and_enchantments_add_their_element() {
    let mut st = stage();
    let s = mob(&mut st, 1, 4.5, 5.5, AffixFlags(AffixFlags::VAMPIRIC.0 | AffixFlags::COLD.0));
    let max = st.horde().unwrap().block(s).hp;
    st.horde_mut().unwrap().store_mut().hp[s as usize] = 1;
    let block = st.horde().unwrap().block(s);
    let i = st.find(1).unwrap();
    let HubInstance { horde, players, dice, .. } = &mut st;
    let (h, f) = (horde.as_mut().unwrap(), players[i].fighter.as_mut().unwrap());
    let before = f.hp;
    let mut hits = Vec::new();
    affix::melee(h, s, &block, f, 101, 3, dice, 20.0, &mut hits);
    assert_eq!(hits.len(), 1, "one cold rider");
    assert_eq!(hits[0].flags >> 4, DamageType::Cold as u8);
    assert_eq!(before - f.hp, hits[0].amount as i32);
    assert_eq!(h.store().hp[s as usize], (1 + 3 + hits[0].amount as i32).min(max), "it drinks both");
}

#[test]
fn frenzy_starts_below_a_third_of_its_hp() {
    let mut st = stage();
    let s = mob(&mut st, 0, 25.5, 5.5, AffixFlags::FRENZIED);
    let max = st.horde().unwrap().block(s).hp;
    run(&mut st, 1.5, false);
    assert_eq!(st.horde().unwrap().store().affix_state[s as usize] & AFFIX_FRENZIED, 0);
    st.horde_mut().unwrap().store_mut().hp[s as usize] = max * 3 / 10;
    let out = run(&mut st, 1.5, false);
    assert!(st.horde().unwrap().store().affix_state[s as usize] & AFFIX_FRENZIED != 0);
    assert!(events(&out).iter().any(|e| e.event == AffixEvent::FRENZY));
    assert!(attack_interval(st.horde().unwrap().store(), s) < 0.75);
}

/// A death blast lands where it was marked after its delay: whoever
/// stands in it takes damage, whoever stepped out takes none.
fn blast(flags: AffixFlags, ability: &str, delay: f32, inside: bool) -> (i32, Outgoing) {
    let mut st = stage();
    let s = mob(&mut st, 0, 6.0, 5.5, flags);
    let x = if inside { 5.0 } else { 15.5 };
    put(&mut st, x, 5.5);
    me(&mut st).max_hp = 5000;
    me(&mut st).hp = 5000;
    let HubInstance { horde, dice, affix, hazards, tick, .. } = &mut st;
    on_death(horde.as_ref().unwrap(), s, *tick, 20.0, dice, affix, hazards);
    st.horde_mut().unwrap().despawn(s);
    let mut out = Vec::new();
    // Stand still (no swing reaches anything: the field is empty).
    for _ in 0..((delay + 0.5) * 20.0) as u32 {
        put(&mut st, x, 5.5);
        st.tick(&mut out);
    }
    assert_eq!(zones(&out, ability), 1, "{ability} is telegraphed once");
    (5000 - me(&mut st).hp, out)
}

#[test]
fn death_blasts_are_telegraphed_and_dodged_by_stepping_out() {
    for (f, ability, delay) in [
        (AffixFlags::FIRE, "affix_fire_nova", NOVA_DELAY),
        (AffixFlags::COLD, "affix_frost_nova", NOVA_DELAY),
        (AffixFlags::VOLATILE, "affix_volatile", VOLATILE_DELAY),
    ] {
        let (taken, _) = blast(f, ability, delay, true);
        assert!(taken > 0, "{ability} hurts inside");
        let (taken, _) = blast(f, ability, delay, false);
        assert_eq!(taken, 0, "{ability} spares whoever stepped out");
    }
    let (_, out) = blast(AffixFlags::VOLATILE, "affix_volatile", VOLATILE_DELAY, false);
    assert!(events(&out).iter().any(|e| e.event == AffixEvent::FUSE));
    // Nothing happens before the delay.
    let mut st = stage();
    let s = mob(&mut st, 0, 6.0, 5.5, AffixFlags::FIRE);
    put(&mut st, 5.0, 5.5);
    me(&mut st).hp = 500;
    me(&mut st).max_hp = 500;
    let HubInstance { horde, dice, affix, hazards, tick, .. } = &mut st;
    on_death(horde.as_ref().unwrap(), s, *tick, 20.0, dice, affix, hazards);
    st.horde_mut().unwrap().despawn(s);
    let mut out = Vec::new();
    for _ in 0..((NOVA_DELAY - 0.2) * 20.0) as u32 {
        put(&mut st, 5.0, 5.5);
        st.tick(&mut out);
    }
    assert_eq!(me(&mut st).hp, 500, "the nova waits for its telegraph");
}

#[test]
fn thorns_prick_only_inside_the_ring() {
    let mut st = stage();
    let s = mob(&mut st, 0, 25.5, 5.5, AffixFlags::THORNS);
    put(&mut st, 23.5, 5.5);
    let h = st.horde_mut().unwrap();
    absorb(h, s, 40, DamageType::Physical);
    st.horde_mut().unwrap().store_mut().hp[s as usize] = 10_000;
    me(&mut st).max_hp = 1000;
    me(&mut st).hp = 1000;
    let mut out = Vec::new();
    for _ in 0..25 {
        put(&mut st, 23.5, 5.5);
        stage_tick_still(&mut st, &mut out);
    }
    let taken = 1000 - me(&mut st).hp;
    assert!(taken >= 10, "a quarter of 40 comes back: {taken}");
    assert!(events(&out).iter().any(|e| e.event == AffixEvent::THORNS));
    // From range: nothing.
    let s = mob(&mut st, 0, 25.5, 7.5, AffixFlags::THORNS);
    absorb(st.horde_mut().unwrap(), s, 40, DamageType::Physical);
    me(&mut st).hp = 1000;
    for _ in 0..25 {
        put(&mut st, 12.5, 5.5);
        stage_tick_still(&mut st, &mut out);
    }
    assert_eq!(me(&mut st).hp, 1000);
}

/// One tick with the player's basic attack held back: it is on the move
/// (put back in place by the callers).
fn stage_tick_still(st: &mut HubInstance, out: &mut Outgoing) {
    let i = st.find(1).unwrap();
    st.players[i].dir = (0.0, 1.0);
    st.tick(out);
}

#[test]
fn a_teleporter_warns_then_blinks_next_to_its_target() {
    let mut st = stage();
    let s = mob(&mut st, 0, 14.5, 5.5, AffixFlags::TELEPORTING);
    let mut out = Vec::new();
    for _ in 0..20 * 10 {
        put(&mut st, 3.5, 5.5);
        stage_tick_still(&mut st, &mut out);
    }
    let ev = events(&out);
    let mark = ev.iter().position(|e| e.event == AffixEvent::BLINK_MARK).expect("the arrival is marked");
    let blink = ev.iter().position(|e| e.event == AffixEvent::BLINK).expect("then it blinks");
    assert!(mark < blink);
    let h = st.horde().unwrap();
    let d = (h.store().x[s as usize] - 3.5).hypot(h.store().y[s as usize] - 5.5);
    assert!(d < 4.0, "next to the player: {d}");
}

#[test]
fn a_summoner_calls_minions_that_pay_nothing() {
    let mut st = stage();
    let s = mob(&mut st, 0, 12.5, 5.5, AffixFlags::SUMMONER);
    let mut out = Vec::new();
    for _ in 0..20 * 22 {
        put(&mut st, 3.5, 5.5);
        stage_tick_still(&mut st, &mut out);
    }
    let h = st.horde().unwrap();
    let minions: Vec<u16> = h.store().alive().iter().copied().filter(|&m| h.store().flags[m as usize] & FLAG_MINION != 0).collect();
    assert!(minions.len() >= 2 && minions.len() <= MAX_MINIONS_FOR_TEST, "{} minions", minions.len());
    assert_eq!(h.minions(), minions.len());
    assert!(events(&out).iter().any(|e| e.event == AffixEvent::SUMMON && e.entity == MONSTER_ENTITY_BASE + s as u32));
    assert_eq!(affix::reward(h.store(), minions[0]), AffixReward::NOTHING);
    assert_eq!(affix::reward(h.store(), s), AffixReward::of(AffixFlags::SUMMONER));
}

const MAX_MINIONS_FOR_TEST: usize = crate::world::horde::affixes::MAX_MINIONS as usize;

#[test]
fn a_mirror_splits_once_at_half_hp() {
    let mut st = stage();
    let f = AffixFlags(AffixFlags::MIRROR.0 | AffixFlags::HASTED.0);
    let s = mob(&mut st, 0, 25.5, 5.5, f);
    let max = st.horde().unwrap().block(s).hp;
    st.horde_mut().unwrap().store_mut().hp[s as usize] = max * 2 / 5;
    let out = run(&mut st, 3.5, false);
    let h = st.horde().unwrap();
    assert!(h.store().affix_state[s as usize] & AFFIX_SPLIT != 0);
    let copies: Vec<u16> = h.store().alive().iter().copied().filter(|&m| m != s).collect();
    assert_eq!(copies.len(), 1, "splits once");
    let c = copies[0] as usize;
    assert_eq!(h.store().affix[c], AffixFlags::HASTED.0, "the copy keeps the rest");
    assert!(h.store().flags[c] & FLAG_MINION != 0);
    assert!((h.store().hp[c] - h.store().hp[s as usize]).abs() <= max / 10, "the same HP");
    assert_eq!(events(&out).iter().filter(|e| e.event == AffixEvent::SPLIT).count(), 1);
}

#[test]
fn the_arcane_beam_sweeps_four_marked_lines_and_roots_it() {
    let mut st = stage();
    let s = mob(&mut st, 0, 8.5, 5.5, AffixFlags::ARCANE);
    let mut out = Vec::new();
    let mut rooted = false;
    for _ in 0..20 * 9 {
        put(&mut st, 3.5, 5.5);
        stage_tick_still(&mut st, &mut out);
        rooted |= st.horde().unwrap().store().status_of(s).has(Condition::Rooted);
    }
    let sweeps: Vec<usize> = out
        .iter()
        .filter_map(|(u, e)| match e {
            HubEvent::Control(ServerMsg::BossZones { ability, zones, .. }) if *u == 1 && ability == "affix_arcane_beam" => Some(zones.len()),
            _ => None,
        })
        .collect();
    assert!(!sweeps.is_empty() && sweeps.iter().all(|&n| n == 4), "{sweeps:?}");
    assert!(rooted, "it holds still through the sweep");
}

#[test]
fn a_plagued_monster_leaves_poison_pools_while_it_walks() {
    let mut st = stage();
    mob(&mut st, 1, 20.5, 5.5, AffixFlags::PLAGUED);
    let mut out = Vec::new();
    for _ in 0..20 * 4 {
        put(&mut st, 3.5, 5.5);
        stage_tick_still(&mut st, &mut out);
    }
    assert!(zones(&out, "affix_plague_pool") >= 2, "pools behind it");
    assert!(st.hazards.iter().any(|h| h.id != 0 && h.pulse > 0), "lasting ground");
}

#[test]
fn hasted_monsters_walk_faster() {
    let walked = |flags: AffixFlags| {
        let mut st = stage();
        let s = mob(&mut st, 1, 25.5, 5.5, flags);
        let mut out = Vec::new();
        for _ in 0..20 {
            put(&mut st, 3.5, 5.5);
            stage_tick_still(&mut st, &mut out);
        }
        25.5 - st.horde().unwrap().store().x[s as usize]
    };
    let (plain, hasted) = (walked(AffixFlags::NONE), walked(AffixFlags::HASTED));
    assert!((hasted / plain - 1.3).abs() < 0.05, "{plain} vs {hasted}");
}

/// A starred kill pays its stars: experience and zeny ×(1 + 0.75 per
/// star); a minion's pays nothing.
#[test]
fn starred_kills_pay_more_and_minions_nothing() {
    let gain = |flags: AffixFlags, minion: bool| {
        let mut st = stage();
        let s = if minion {
            st.horde_mut().unwrap().spawn_minion(0, 4.5, 5.5, 40, flags).unwrap()
        } else {
            mob(&mut st, 0, 4.5, 5.5, flags)
        };
        st.horde_mut().unwrap().store_mut().hp[s as usize] = 1;
        st.horde_mut().unwrap().store_mut().shield[s as usize] = 0;
        let before = me(&mut st).run_exp;
        let mut out = Vec::new();
        for _ in 0..60 {
            put(&mut st, 3.5, 5.5);
            st.tick(&mut out);
            if !st.horde().unwrap().store().is_alive(s) {
                break;
            }
        }
        assert!(!st.horde().unwrap().store().is_alive(s), "killed");
        me(&mut st).run_exp - before
    };
    let plain = gain(AffixFlags::NONE, false);
    let two = gain(AffixFlags(AffixFlags::HASTED.0 | AffixFlags::THORNS.0), false);
    assert!(plain > 0);
    let ratio = two as f64 / plain as f64;
    assert!((ratio - 2.5).abs() < 0.1, "two stars pay ×2.5: {plain} → {two}");
    assert_eq!(gain(AffixFlags::NONE, true), 0, "a minion pays nothing");
}

#[test]
fn joining_hears_the_table_and_every_affixed_monster() {
    let mut st = stage();
    let s = mob(&mut st, 0, 25.5, 5.5, AffixFlags::FIRE);
    let mut out = Vec::new();
    st.join(profile(2), Uuid::nil(), None, 0, &mut out);
    let mine: Vec<&ServerMsg> = out.iter().filter(|(u, _)| *u == 2).filter_map(|(_, e)| if let HubEvent::Control(m) = e { Some(m) } else { None }).collect();
    assert!(mine.iter().any(|m| matches!(m, ServerMsg::AffixDefs { defs, .. } if defs.len() == crate::world::combat::affixes::AFFIXES.len())));
    assert!(mine.iter().any(|m| matches!(m, ServerMsg::MonsterAffixes { monsters } if monsters.iter().any(|a| a.slot == s && a.bits == AffixFlags::FIRE.0))));
}
