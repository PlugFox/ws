//! The Merchant line (COMBAT.md §3, the Merchant → Alchemist → Creator
//! rows) on a stage and in town: Mammonite's purse, the cart's weight,
//! acid that never misses, Acid Demonstration's bottles (and never by
//! auto-cast), Potion Pitcher throwing a real potion, the summons reaching
//! the companions pool, Maximize Power's drain, the Full Chemical
//! Protection ward, haggled shop prices and the Pharmacy. The table below
//! is the merchant rows of `skills.toml` with cooldowns cut so each test
//! is quick.

use super::fight::Fighter;
use super::test_support::{map, portal};
use super::tests::profile;
use super::*;
use crate::world::combat::items::Haggle;
use crate::world::combat::{self, Bag, Condition, Loadout, SkillBook};
use crate::world::horde::catalog::{test_def, test_plant, MonsterCatalog};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind, NpcDef};
use crate::world::protocol::{hit, HitEvent, MONSTER_ENTITY_BASE};
use crate::world::trade::ShopOrder;
use std::sync::Arc;
use uuid::Uuid;
use std::assert_matches;

const BOOK: &str = r#"{"skills":[
  {"id":"pushcart","name":"Pushcart","class":"merchant","max_level":5,"passive":true,
   "effects":[{"type":"cart","stacks":[0,20]}]},
  {"id":"discount","name":"Discount","class":"merchant","max_level":5,"passive":true,
   "effects":[{"type":"haggle","buy":[5,4]}]},
  {"id":"overcharge","name":"Overcharge","class":"merchant","max_level":5,"passive":true,
   "effects":[{"type":"haggle","sell":[5,4]}]},
  {"id":"pharmacy","name":"Pharmacy","class":"merchant","max_level":5,"passive":true,
   "effects":[{"type":"pharmacy"}]},
  {"id":"learning_potion","name":"Learning Potion","class":"merchant","max_level":10,"passive":true,
   "effects":[{"type":"buff","potion":[0,0.05]}]},
  {"id":"mammonite","name":"Mammonite","class":"merchant","max_level":10,"target":"nearest","range":2.5,"cooldown":0.2,"zeny":[5,5],
   "effects":[{"type":"damage","dice":"weapon","extra":"1d6","mult":[1,1]}]},
  {"id":"cart_revolution","name":"Cart Revolution","class":"merchant","target":"self","cooldown":0.2,
   "effects":[{"type":"damage","dice":"weapon","extra":"2d6","radius":2,"at":"caster","cart":0.01}]},
  {"id":"acid_terror","name":"Acid Terror","class":"merchant","max_level":5,"target":"nearest","range":6,"cooldown":0.2,
   "effects":[{"type":"damage","dice":"1d8","scale":"k","mult":[1,1],"sure":true,"save":{"condition":"mired","secs":8}}]},
  {"id":"acid_demonstration","name":"Acid Demonstration","class":"merchant","max_level":5,"target":"nearest","range":6,"cooldown":0.2,
   "auto":false,"reagents":{"acid_bottle":1,"bottle_grenade":1},
   "effects":[{"type":"damage","dice":"1d8","scale":"k","sure":true,"tough":0.02}]},
  {"id":"potion_pitcher","name":"Potion Pitcher","class":"merchant","max_level":5,"target":"most_wounded_ally","range":6,"cooldown":0.2,
   "effects":[{"type":"pitch","mult":[1.5,0.1],"regen":2.0}]},
  {"id":"maximize_power","name":"Maximize Power","class":"merchant","max_level":5,"target":"self","mobile":true,"cooldown":0.2,
   "effects":[{"type":"buff","toggle":true,"maximize":true,"sp_drain":[6,-1]}]},
  {"id":"full_chemical_protection","name":"FCP","class":"merchant","max_level":5,"target":"allies","range":5,"cooldown":0.2,
   "effects":[{"type":"buff","secs":[20,4],"immune":["poisoned","burning","mired"]}]},
  {"id":"summon_flora","name":"Summon Flora","class":"merchant","max_level":5,"target":"aim","range":6,"cooldown":0.2,
   "effects":[{"type":"summon","what":"flora"}]},
  {"id":"plant_cultivation","name":"Plant Cultivation","class":"merchant","max_level":2,"target":"aim","range":4,"cooldown":0.2,
   "effects":[{"type":"summon","what":"cultivation"}]}
]}"#;

/// A 20 × 9 field; a merchant of `level` (user 1, an axe) with `skills`
/// slotted in order and `bag` as its belongings.
fn stage(skills: &[(&str, u8)], level: u32, bag: Bag) -> HubInstance {
    let row = format!("#{}#", ".".repeat(20));
    let wall = "#".repeat(22);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 9));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 0.01, 50, 0.0, false), test_plant("black_mushroom", (3, 3))]).unwrap());
    let cfg = HordeConfig { light_radius: 12.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let book = SkillBook::from_json(BOOK).unwrap();
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(Arc::new(book));
    stage.dice = combat::Rng::new(5);
    stage.join(PlayerProfile { archetype: "merchant".into(), level, loadout: loadout(skills), bag, ..profile(1) }, Uuid::nil(), None, 0, &mut Vec::new());
    put(&mut stage, 1, 6.5, 5.5);
    stage
}

fn loadout(skills: &[(&str, u8)]) -> Loadout {
    let mut l = Loadout::default();
    l.hotbar.resize(combat::skills::SLOTS, String::new());
    for (i, (id, lvl)) in skills.iter().enumerate() {
        l.skills.insert(id.to_string(), *lvl);
        l.hotbar[i] = id.to_string();
    }
    l
}

fn bag(zeny: u64, items: &[(&str, u32)]) -> Bag {
    let mut b = Bag::starting(combat::Class::Merchant, "");
    b.items.clear();
    b.zeny = zeny;
    for (id, n) in items {
        b.add(id, *n).unwrap();
    }
    b
}

fn put(stage: &mut HubInstance, user: u32, x: f32, y: f32) {
    let i = stage.find(user).unwrap();
    (stage.players[i].x, stage.players[i].y) = (x, y);
}

fn me(stage: &HubInstance) -> &HubPlayer {
    &stage.players[stage.find(1).unwrap()]
}

fn fighter(stage: &mut HubInstance) -> &mut Fighter {
    let i = stage.find(1).unwrap();
    stage.players[i].fighter.as_mut().unwrap()
}

/// A blob at `(x, y)` with `hp` HP and that much maximum.
fn blob(stage: &mut HubInstance, x: f32, y: f32, hp: i32) -> u16 {
    let horde = stage.horde_mut().unwrap();
    // The catalog is in id order: the mushroom comes first.
    let kind = horde.catalog().kind_of("blob").unwrap();
    let s = horde.spawn(kind, x, y).unwrap();
    horde.store_mut().hp[s as usize] = hp;
    s
}

/// Ticks `n` times with `slots` held until the merchant's first cast goes
/// out (a press can land on a tick that casts nothing); everything sent.
fn run(stage: &mut HubInstance, n: u32, slots: u16) -> Outgoing {
    let mut out: Outgoing = Vec::new();
    let mut held = slots;
    for _ in 0..n {
        stage.act(1, [0, 0], held);
        stage.tick(&mut out);
        let cast = out.iter().any(|(u, e)| *u == 1 && matches!(e, HubEvent::Control(ServerMsg::Cast { caster: 101, skill, .. }) if !skill.is_empty()));
        if cast {
            held = 0;
        }
    }
    stage.act(1, [0, 0], 0);
    out
}

fn hits_on(out: &Outgoing, s: u16) -> Vec<HitEvent> {
    out.iter()
        .filter_map(|(u, e)| match (*u, e) {
            (1, HubEvent::Control(ServerMsg::Combat { hits, .. })) => Some(hits.clone()),
            _ => None,
        })
        .flatten()
        .filter(|h| h.target == MONSTER_ENTITY_BASE + s as u32)
        .collect()
}

fn spent_zeny(out: &Outgoing) -> u64 {
    out.iter().filter_map(|(u, e)| matches!((*u, e), (1, HubEvent::ZenySpent { .. })).then_some(e)).map(|e| if let HubEvent::ZenySpent { amount } = e { *amount } else { 0 }).sum()
}

fn used(out: &Outgoing) -> Vec<String> {
    out.iter()
        .filter_map(|(u, e)| match (*u, e) {
            (1, HubEvent::ItemUsed { item }) => Some(item.clone()),
            _ => None,
        })
        .collect()
}

/// Mammonite pays 5 + 5·slv zeny from the hub's copy of the bag and the
/// socket task is told; broke, it is a plain swing and costs nothing.
#[test]
fn mammonite_spends_from_the_purse_and_swings_plainly_when_broke() {
    let mut st = stage(&[("mammonite", 3)], 10, bag(100, &[]));
    blob(&mut st, 7.5, 5.5, 10_000);
    let out = run(&mut st, 2, 1);
    assert_eq!(spent_zeny(&out), 20);
    assert_eq!(me(&st).profile.bag.zeny, 80);

    let mut broke = stage(&[("mammonite", 3)], 10, bag(19, &[]));
    let s = blob(&mut broke, 7.5, 5.5, 10_000);
    let out = run(&mut broke, 2, 1);
    assert_eq!(spent_zeny(&out), 0, "nothing paid");
    assert_eq!(me(&broke).profile.bag.zeny, 19);
    assert!(!hits_on(&out, s).is_empty(), "the swing still lands (or misses)");
}

/// Earned zeny reaches the purse with the progress report, so a run's
/// kills pay for the next Mammonite.
#[test]
fn earned_zeny_tops_up_the_purse() {
    let mut st = stage(&[("mammonite", 1)], 10, bag(0, &[]));
    st.players[0].fighter.as_mut().unwrap().add_zeny(50);
    let mut out = Vec::new();
    for _ in 0..40 {
        st.tick(&mut out);
    }
    assert!(out.iter().any(|(_, e)| matches!(e, HubEvent::Zeny { amount: 50 })));
    assert_eq!(me(&st).profile.bag.zeny, 50);
}

/// Acid lands whatever the armour (no miss, never halved). The monster
/// stands beyond the axe's reach: every hit is the acid's.
#[test]
fn acid_terror_never_misses() {
    let mut st = stage(&[("acid_terror", 5)], 30, bag(0, &[]));
    let s = blob(&mut st, 14.5, 5.5, 10_000);
    let mut all = Vec::new();
    for _ in 0..12 {
        all.extend(hits_on(&run(&mut st, 8, 1), s));
    }
    assert!(all.len() >= 10, "{all:?}");
    assert!(all.iter().all(|h| h.flags & hit::MISS == 0 && h.amount > 0), "{all:?}");
}

/// Acid Demonstration uses an Acid Bottle and a Bottle Grenade per cast
/// for full power (dice and the max-HP bite); without them it lands at
/// half; an auto-cast slot never fires it.
#[test]
fn acid_demonstration_spends_its_bottles_or_lands_weaker() {
    let total = |bottles: u32| {
        let mut st = stage(&[("acid_demonstration", 1)], 60, bag(0, &[("acid_bottle", bottles), ("bottle_grenade", bottles)]));
        let s = blob(&mut st, 14.5, 5.5, 10_000);
        let mut sum = 0;
        let mut items = Vec::new();
        for _ in 0..8 {
            let out = run(&mut st, 8, 1);
            sum += hits_on(&out, s).iter().map(|h| h.amount).sum::<u32>();
            items.extend(used(&out));
        }
        (sum, items, me(&st).profile.bag.count("acid_bottle"))
    };
    let (full, items, left) = total(8);
    assert_eq!(left, 0, "one bottle a cast");
    assert_eq!(items.iter().filter(|i| *i == "acid_bottle").count(), 8);
    assert_eq!(items.iter().filter(|i| *i == "bottle_grenade").count(), 8);
    let (weak, items, _) = total(0);
    assert!(items.is_empty());
    assert!(weak > 0 && full as f32 > weak as f32 * 1.5, "full {full}, without bottles {weak}");

    // Auto-cast: never.
    let mut st = stage(&[("acid_demonstration", 1)], 60, bag(0, &[("acid_bottle", 3), ("bottle_grenade", 3)]));
    let s = blob(&mut st, 14.5, 5.5, 10_000);
    st.players[0].prefs.auto = 1;
    let out = run(&mut st, 30, 0);
    assert!(hits_on(&out, s).iter().all(|h| h.flags & hit::MISS != 0 || h.amount < 1000) && used(&out).is_empty(), "nothing thrown by itself");
    assert_eq!(me(&st).profile.bag.count("acid_bottle"), 3);
}

/// The max-HP bite is capped at twice the roll, once on a boss.
#[test]
fn the_tough_bite_is_capped_by_the_roll() {
    let mut st = stage(&[("acid_demonstration", 1)], 1, bag(0, &[("acid_bottle", 1), ("bottle_grenade", 1)]));
    let s = blob(&mut st, 14.5, 5.5, 1_000_000);
    let hits = hits_on(&run(&mut st, 8, 1), s);
    // 1d8 at level 1: at most 8, and the bite at most 16 more.
    assert!(!hits.is_empty() && hits.iter().all(|h| (1..=24).contains(&h.amount)), "{hits:?}");
}

/// The cart's weight: Cart Revolution's dice grow with the occupied bag
/// stacks (and the Pushcart passive gives the stacks room).
#[test]
fn a_loaded_cart_hits_harder() {
    let loaded: Vec<(String, u32)> = ["jellopy", "flower", "stem", "fluff", "clover", "resin", "feather", "shell", "garlet", "chrysalis"]
        .iter()
        .map(|s| (s.to_string(), 1))
        .collect();
    let many: Vec<(&str, u32)> = loaded.iter().map(|(s, n)| (s.as_str(), *n)).collect();
    let total = |b: Bag| {
        let mut st = stage(&[("cart_revolution", 1)], 10, b);
        let s = blob(&mut st, 7.0, 5.5, 100_000);
        let mut sum = 0;
        for _ in 0..30 {
            sum += hits_on(&run(&mut st, 8, 1), s).iter().map(|h| h.amount).sum::<u32>();
        }
        sum
    };
    // Ten stacks: +10% dice, too few to tell over the swing's spread;
    // pad the bag to a full hundred.
    let mut full = bag(0, &many);
    for i in 0..90 {
        full.items.push(combat::items::Stack { id: "knife".into(), count: 1, attrs: combat::items::Attrs { refine: (i % 10) as u8, ..Default::default() } });
    }
    let (empty, heavy) = (total(bag(0, &[])), total(full));
    assert!(heavy as f32 > empty as f32 * 1.3, "empty cart {empty}, full cart {heavy}");

    let book = SkillBook::from_json(BOOK).unwrap();
    assert_eq!(book.cart_stacks(&loadout(&[("pushcart", 5)])), 100);
    let mut b = bag(0, &[]);
    b.cart = 20;
    for i in 0..120 {
        b.add(&format!("knife+{}", i % 11), 1).ok();
        b.add(&format!("cutter+{}", i % 11), 1).ok();
    }
    assert!(b.items.len() <= b.capacity() && b.capacity() == 120);
}

/// Potion Pitcher throws the caster's own potion at the wounded ally: it
/// is used up (the bag is told) and restores more, faster; with nothing to
/// throw nothing happens.
#[test]
fn potion_pitcher_throws_a_real_potion() {
    let mut st = stage(&[("potion_pitcher", 5)], 10, bag(0, &[("red_potion", 2)]));
    let mut out = Vec::new();
    st.join(PlayerProfile { archetype: "swordsman".into(), level: 10, ..profile(2) }, Uuid::nil(), None, 0, &mut out);
    put(&mut st, 2, 8.5, 5.5);
    let j = st.find(2).unwrap();
    {
        let f = st.players[j].fighter.as_mut().unwrap();
        f.hp = 5;
    }
    fighter(&mut st).potions = vec![("red_potion".into(), 2)];
    let out = run(&mut st, 4, 1);
    assert_eq!(used(&out), vec!["red_potion".to_string()]);
    assert_eq!(fighter(&mut st).potions[0].1, 1);
    let r = st.players[j].fighter.as_ref().unwrap().regen[0].expect("a regeneration on the ally");
    // Red Potion 4d4+4 (8..20) × 2.0, over 2 s.
    assert!((16..=40).contains(&r.total) && r.span <= 2 * st.config.tick_hz, "{r:?}");

    fighter(&mut st).potions = vec![("red_potion".into(), 0)];
    st.players[j].fighter.as_mut().unwrap().regen[0] = None;
    let out = run(&mut st, 10, 1);
    assert!(used(&out).is_empty() && st.players[j].fighter.as_ref().unwrap().regen[0].is_none());
}

/// Auto-cast throws only below the caster's own HP potion threshold.
#[test]
fn auto_pitch_respects_the_potion_threshold() {
    let mut st = stage(&[("potion_pitcher", 1)], 10, bag(0, &[]));
    fighter(&mut st).potions = vec![("red_potion".into(), 5)];
    st.players[0].prefs.auto = 1;
    let max = fighter(&mut st).max_hp;
    fighter(&mut st).hp = max * 7 / 10;
    assert!(used(&run(&mut st, 10, 0)).is_empty(), "no threshold set: never by itself");
    st.players[0].uses.hp = 50;
    assert!(used(&run(&mut st, 10, 0)).is_empty(), "70% is above 50%");
    fighter(&mut st).hp = max * 4 / 10;
    assert_eq!(used(&run(&mut st, 10, 0)).len(), 1);
}

/// Summon Flora reaches the companions pool; Plant Cultivation grows a
/// real field plant, at most three per caster.
#[test]
fn summons_reach_the_companions_and_the_field() {
    let mut st = stage(&[("summon_flora", 2), ("plant_cultivation", 1)], 30, bag(0, &[]));
    blob(&mut st, 10.5, 5.5, 10_000);
    run(&mut st, 4, 1);
    assert!(st.companions.of(1).count() >= 1, "a plant companion is out");
    for _ in 0..5 {
        run(&mut st, 8, 2);
    }
    let horde = st.horde_mut().unwrap();
    assert_eq!(horde.plants(), 3, "three grown at most");
}

/// Maximize Power drains SP each second and switches itself off when the
/// SP runs out.
#[test]
fn maximize_power_drains_and_ends_without_sp() {
    let mut st = stage(&[("maximize_power", 1)], 10, bag(0, &[]));
    fighter(&mut st).sp = 30;
    run(&mut st, 2, 1);
    assert!(fighter(&mut st).mods.maximize);
    let hz = st.config.tick_hz;
    run(&mut st, hz * 3, 0);
    let sp = fighter(&mut st).sp;
    assert!(sp < 30 - 10, "{sp} SP left after 3 s");
    run(&mut st, hz * 8, 0);
    assert!(!fighter(&mut st).mods.maximize, "out of SP: off");
}

/// Full Chemical Protection: the conditions it names do not take hold.
#[test]
fn chemical_protection_wards_off_its_conditions() {
    let mut st = stage(&[("full_chemical_protection", 1)], 30, bag(0, &[]));
    run(&mut st, 2, 1);
    let f = fighter(&mut st);
    f.afflict(Condition::Poisoned, 100);
    f.afflict(Condition::Stunned, 100);
    assert!(!f.status.has(Condition::Poisoned) && f.status.has(Condition::Stunned));
}

/// A merchant in town: haggled prices on the shop window and on the order
/// the socket task applies; Discount and Overcharge stop at 25%.
#[test]
fn haggling_moves_shop_prices_both_ways() {
    let wall = "#".repeat(30);
    let row = format!("#{}#", ".".repeat(28));
    let rows: Vec<&str> = [wall.as_str(), &row, &row, &row, &row, &row, wall.as_str()].to_vec();
    let mut town = map("town", LocationKind::Hub, CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 }, &rows, vec![]);
    town.manifest.npcs.push(NpcDef {
        id: "tools".into(),
        sprite: "x".into(),
        name: "Tool Dealer".into(),
        kind: SHOP.into(),
        shop: "tool".into(),
        x: 15,
        y: 3,
        facing: 4,
        interact_radius: 3.0,
    });
    let world = Arc::new(HubWorld::new(vec![town], "town").unwrap());
    let mut hub = HubInstance::new(1, world, HubSimConfig::default()).with_skills(Arc::new(SkillBook::from_json(BOOK).unwrap()));
    let mut out = Vec::new();
    hub.join(PlayerProfile { archetype: "merchant".into(), level: 20, loadout: loadout(&[("discount", 5), ("overcharge", 3)]), ..profile(1) }, Uuid::nil(), None, 0, &mut out);
    out.clear();
    hub.interact(1, "tools", &mut out);
    assert!(out.iter().any(|(_, e)| matches!(e, HubEvent::Control(ServerMsg::Shop { buy_pct: 25, sell_pct: 17, .. }))), "{out:?}");
    out.clear();
    let order = ShopOrder { buy: vec![("red_potion".into(), 4)], sell: vec![("jellopy".into(), 10)] };
    hub.shop(1, "tools", order.clone(), &mut out);
    let Some(HubEvent::Shop(o, haggle)) = out.iter().map(|(_, e)| e).find(|e| matches!(e, HubEvent::Shop(..))) else { panic!("{out:?}") };
    assert_eq!(*haggle, Haggle { buy: 25, sell: 17 });
    let mut b = bag(1000, &[("jellopy", 10)]);
    let r = o.apply_at(&mut b, *haggle).unwrap();
    // Red Potion 50 → 38 (rounded up); Jellopy 6 sells for 3 → 3.51 → 3.
    assert_eq!((r.spent, r.earned), (4 * 38, 10 * 3));
    assert_eq!(Haggle { buy: 25, sell: 25 }.sell_gain(1200, 1), 750);

    let book = SkillBook::from_json(BOOK).unwrap();
    assert_eq!(book.haggle(&loadout(&[("discount", 5), ("overcharge", 5)])), (25, 25), "capped");
    assert_eq!(book.haggle(&Loadout::default()), (0, 0));
}

/// The Pharmacy opens in town for whoever learned it, lists what its
/// level allows and brews nothing above it; on a mission it is closed.
#[test]
fn the_pharmacy_brews_by_its_level() {
    let wall = "#".repeat(12);
    let row = format!("#{}#", ".".repeat(10));
    let rows: Vec<&str> = [wall.as_str(), &row, &row, wall.as_str()].to_vec();
    let town = map("town", LocationKind::Hub, CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 }, &rows, vec![]);
    let world = Arc::new(HubWorld::new(vec![town], "town").unwrap());
    let mut hub = HubInstance::new(1, world, HubSimConfig::default()).with_skills(Arc::new(SkillBook::from_json(BOOK).unwrap()));
    let mut out = Vec::new();
    hub.join(PlayerProfile { archetype: "merchant".into(), level: 30, loadout: loadout(&[("pharmacy", 2)]), ..profile(1) }, Uuid::nil(), None, 0, &mut out);
    hub.join(PlayerProfile { archetype: "merchant".into(), level: 30, ..profile(2) }, Uuid::nil(), None, 0, &mut out);
    out.clear();
    hub.interact(1, quest::PHARMACY, &mut out);
    let Some(HubEvent::Control(ServerMsg::Recipes { recipes, .. })) = out.first().map(|(_, e)| e) else { panic!("{out:?}") };
    assert!(recipes.iter().any(|r| r.id == "acid_bottle") && !recipes.iter().any(|r| r.id == "white_potion" || r.id == "stone_of_sage"));
    out.clear();
    hub.craft(1, quest::PHARMACY, "acid_bottle", &mut out);
    assert_matches!(out[..], [(1, HubEvent::Craft { recipe: "acid_bottle", count: 1 })], "{out:?}");
    out.clear();
    hub.craft(1, quest::PHARMACY, "stone_of_sage", &mut out);
    assert_matches!(&out[..], [(1, HubEvent::Control(ServerMsg::HubError { code, .. }))] if code == "level_too_low", "{out:?}");
    out.clear();
    hub.interact(2, quest::PHARMACY, &mut out);
    assert_matches!(&out[..], [(2, HubEvent::Control(ServerMsg::HubError { code, .. }))] if code == "not_learned", "{out:?}");

    // At the top level, now and then two from one brew.
    let i = hub.find(1).unwrap();
    hub.players[i].profile.loadout = loadout(&[("pharmacy", 5)]);
    let mut twos = 0;
    for _ in 0..200 {
        out.clear();
        hub.craft(1, quest::PHARMACY, "red_potion", &mut out);
        twos += matches!(out[..], [(1, HubEvent::Craft { count: 2, .. })]) as u32;
    }
    assert!((15..=70).contains(&twos), "{twos} doubles in 200");
}

/// The baked table: every Merchant-line skill the server casts, and the
/// signature skills the class screen shows exist.
#[test]
fn the_baked_merchant_skills_are_cast() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    let Ok(book) = SkillBook::load(&dist) else { return };
    let mine: Vec<_> = book.iter().filter(|(_, d)| d.class == Some(combat::Class::Merchant)).collect();
    assert!(mine.len() >= 25);
    for (_, d) in &mine {
        assert!(d.ready(), "{} is not cast", d.id);
    }
    let (_, ad) = book.find("acid_demonstration").unwrap();
    assert!(ad.never_auto() && ad.reagents.len() == 2);
    assert!(book.find("call_homunculus").unwrap().1.never_auto());
    assert!(book.find("pushcart").is_some());
}

/// Every class line has a travel skill for the travel key; the merchant's
/// is Cart Boost (a dash, then a short run) from tier 1, so the Alchemist
/// and the Creator keep it.
#[test]
fn every_class_has_a_travel_skill() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    let Ok(book) = SkillBook::load(&dist) else { return };
    for class in combat::Class::ALL {
        assert!(
            book.iter().any(|(_, d)| d.class == Some(class) && !d.passive && d.travels()),
            "{class:?} has no travel skill"
        );
    }
    let (_, cb) = book.find("cart_boost").unwrap();
    assert!(cb.travels() && cb.mobile && cb.tier == 1 && cb.target == combat::skills::Target::Escape);
}

/// Cart Boost on the travel key: the merchant dashes where it walks and
/// runs faster for a moment.
#[test]
fn cart_boost_dashes_on_the_travel_key_and_speeds_the_merchant_up() {
    const CART: &str = r#"{"skills":[
      {"id":"pushcart","name":"Pushcart","class":"merchant","max_level":5,"passive":true,"effects":[{"type":"cart","stacks":[0,20]}]},
      {"id":"cart_boost","name":"Cart Boost","class":"merchant","max_level":5,"target":"escape","mobile":true,"range":5,"cooldown":[7,-0.4],
       "effects":[{"type":"move","mode":"dash","distance":[2.5,0.5]},{"type":"buff","secs":[2,0.4],"speed":[0.1,0.04]}]}
    ]}"#;
    let row = format!("#{}#", ".".repeat(20));
    let wall = "#".repeat(22);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 9));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 0.01, 50, 0.0, false)]).unwrap());
    let horde = Horde::new(Arc::clone(&world), catalog, HordeConfig { director_enabled: false, seed: Some(1), ..HordeConfig::default() });
    let mut st = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(Arc::new(SkillBook::from_json(CART).unwrap()));
    let mut l = Loadout::default();
    l.skills.insert("pushcart".into(), 1);
    l.skills.insert("cart_boost".into(), 3);
    st.join(PlayerProfile { archetype: "merchant".into(), level: 10, loadout: l, ..profile(1) }, Uuid::nil(), None, 0, &mut Vec::new());
    put(&mut st, 1, 6.5, 5.5);
    let (book_index, _) = st.skills.find("cart_boost").unwrap();
    assert_eq!(fighter(&mut st).travel, Some(book_index), "Cart Boost is on the travel key, no hotbar slot needed");
    let speed = fighter(&mut st).mods.speed;
    let mut out = Vec::new();
    st.act(1, [8, 0], 1 << super::cast::TRAVEL_SLOT);
    st.tick(&mut out);
    let moved = me(&st).x - 6.5;
    assert!(moved > 2.0, "dashed toward the pointer: {moved}");
    assert!(fighter(&mut st).mods.speed > speed, "and runs faster after it");
}


