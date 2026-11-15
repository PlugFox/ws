//! Monster cards on a stage (combat/cards.rs): what a card worn changes in
//! the fight. Fighter-level rules first (conditions, the last stand, the
//! gilded shield), then a basic attack setting a monster alight.

use super::fight::Fighter;
use super::test_support::{map, portal};
use super::tests::profile;
use super::*;
use crate::world::combat::{self, Bag, Class, Condition, Loadout, SkillBook};
use crate::world::horde::catalog::{test_def, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use std::sync::Arc;
use uuid::Uuid;

/// A swordsman wearing `keys` (slot, instance key).
fn wearing(keys: &[(combat::Slot, &str)]) -> Fighter {
    let mut bag = Bag::starting(Class::Swordsman, "");
    for (s, k) in keys {
        bag.equip[*s as usize] = (*k).into();
    }
    let mut f = Fighter::new(Class::Swordsman, 10, 0, "sword");
    f.wear(&bag);
    f
}

#[test]
fn a_card_keeps_conditions_off_and_shortens_the_rest() {
    let mut f = wearing(&[(combat::Slot::Garment, "hood[roda_frog_card]"), (combat::Slot::Armour, "cotton_shirt[fabre_card]")]);
    f.afflict(Condition::Slowed, 100);
    f.afflict(Condition::Mired, 100);
    assert!(!f.status.has(Condition::Slowed) && !f.status.has(Condition::Mired), "the pond's card");
    f.afflict(Condition::Blinded, 100);
    assert!(f.status.has(Condition::Blinded));
    // Fabre: conditions end 30% sooner.
    let plain = wearing(&[]);
    assert_eq!(plain.procs().cond_short, 0);
    assert_eq!(f.procs().cond_short, 30);
}

#[test]
fn the_angeling_saves_once_per_expedition() {
    let mut f = wearing(&[(combat::Slot::Armour, "cotton_shirt[angeling_card]")]);
    f.hp = 0;
    f.fall_if_spent(20.0);
    assert!(f.is_up() && f.hp == f.max_hp * 3 / 10, "{} of {}", f.hp, f.max_hp);
    f.hp = 0;
    f.fall_if_spent(20.0);
    assert!(!f.is_up(), "only once");
    // A reconnect keeps it spent.
    let mut again = wearing(&[(combat::Slot::Armour, "cotton_shirt[angeling_card]")]);
    again.resume(&f);
    assert!(again.cards.stood);
}

#[test]
fn the_golden_shield_turns_magic_into_zeny_and_the_general_caps_a_blow() {
    let mut f = wearing(&[(combat::Slot::Shield, "guard[golden_bug_card]")]);
    let hp = f.hp;
    let took = f.take(50, false);
    assert_eq!((took, f.zeny), (40, 10), "20% of a spell becomes coin");
    assert_eq!(f.hp, hp - 40);
    assert_eq!(f.take(10, true), 10, "physical blows are not gilded");
    let mut g = wearing(&[(combat::Slot::Armour, "cotton_shirt[jiangshi_general_card]")]);
    let cap = g.max_hp * 35 / 100;
    assert_eq!(g.take(10_000, true), cap);
}

#[test]
fn cards_change_the_walk_and_the_cast() {
    let mut f = wearing(&[(combat::Slot::Shoes, "sandals[lunatic_card]")]);
    assert_eq!(f.card_pace(), 1.0);
    f.hp = f.max_hp / 3;
    assert!((f.card_pace() - 1.1).abs() < 1e-6, "springheel under half HP");
    let mut fox = wearing(&[(combat::Slot::Accessory1, "clip[nine_tail_card]")]);
    let free: Vec<bool> = (0..18).map(|_| fox.card_cast(20.0, || 100).0).collect();
    assert_eq!(free.iter().filter(|b| **b).count(), 2, "every 9th: {free:?}");
    assert!(free[8] && free[17]);
    let mut moon = wearing(&[(combat::Slot::Garment, "hood[moonlight_flower_card]")]);
    assert_eq!(moon.card_cast(20.0, || 100), (true, true));
    assert_eq!(moon.card_cast(20.0, || 100), (false, false), "20 s before the next");
}

/// A 20 × 9 field and a swordsman (user 1) wearing `weapon`.
fn stage(weapon: &str) -> HubInstance {
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
    let book = SkillBook::from_json(r#"{"skills":[]}"#).unwrap();
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(Arc::new(book));
    stage.dice = combat::Rng::new(5);
    let mut bag = Bag::starting(Class::Swordsman, "");
    bag.equip[combat::Slot::Weapon as usize] = weapon.into();
    let mut out = Vec::new();
    let p = PlayerProfile { archetype: "swordsman".into(), level: 10, loadout: Loadout::default(), bag, ..profile(1) };
    stage.join(p, Uuid::nil(), None, 0, &mut out);
    let i = stage.find(1).unwrap();
    (stage.players[i].x, stage.players[i].y) = (6.5, 5.5);
    stage
}

/// Ticks until monster `s` burns, at most `n` ticks; whether it did.
fn burns(stage: &mut HubInstance, s: u16, n: u32) -> bool {
    let mut out = Vec::new();
    stage.act(1, [0, 0], 0);
    for _ in 0..n {
        stage.tick(&mut out);
        if stage.horde().unwrap().store().status_of(s).has(Condition::Burning) {
            return true;
        }
    }
    false
}

#[test]
fn the_drops_card_sets_monsters_alight_and_a_plain_sword_does_not() {
    for (weapon, want) in [("sword[drops_card]", true), ("sword", false)] {
        let mut st = stage(weapon);
        let horde = st.horde_mut().unwrap();
        let s = horde.spawn(0, 7.5, 5.5).unwrap();
        horde.store_mut().hp[s as usize] = 100_000;
        assert_eq!(burns(&mut st, s, 1200), want, "{weapon}");
    }
}

#[test]
fn the_opening_minibosses_cards_do_what_they_say() {
    let toad = wearing(&[(combat::Slot::Weapon, "sword[toad_card]")]);
    assert_eq!((toad.procs().reach, toad.procs().leech), (1, 6), "a longer reach and a gulp of HP");
    let bat = wearing(&[(combat::Slot::Garment, "hood[vampire_bat_card]")]);
    assert_eq!((bat.procs().dodge, bat.procs().blind_back), (8, 15));
    let mut king = wearing(&[(combat::Slot::Armour, "cotton_shirt[scorpion_king_card]")]);
    assert_eq!(king.procs().thorns, 6);
    king.afflict(Condition::Poisoned, 100);
    assert!(!king.status.has(Condition::Poisoned), "the king's venom guard");
    let captain = wearing(&[(combat::Slot::Shield, "guard[skeleton_captain_card]")]);
    assert_eq!((captain.procs().riposte, captain.procs().pack), (8, 1));
}
