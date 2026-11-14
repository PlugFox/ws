//! Balance on the baked pack. Fight cadence (COMBAT.md "Cadence"): how often a
//! monster swings, how often it casts its skills and how often a scripted
//! boss starts an ability, measured over a simulated minute against a
//! player who only stands there. The tests skip when the asset pack is not
//! baked.

use super::test_support::{map, portal};
use super::*;
use crate::world::combat::skills::SkillBook;
use crate::world::combat::Rng;
use crate::world::horde::catalog::MonsterCatalog;
use crate::world::horde::{Horde, HordeConfig};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{Look, MONSTER_ENTITY_BASE};
use std::sync::Arc;
use uuid::Uuid;

/// Simulated seconds per measurement.
const MINUTE: u32 = 60;

/// A 60 × 40 open field on the real monster catalogue and skill table,
/// player 1 (a level-40 swordsman who takes everything and never strikes
/// back) in the middle. `None` without the baked pack.
fn arena(seed: u64) -> Option<HubInstance> {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    let (Ok(catalog), Ok(book)) = (MonsterCatalog::load(&dist), SkillBook::load(&dist)) else { return None };
    Some(field(catalog, book, seed))
}

/// [`arena`] on any catalogue and skill table.
fn field(catalog: MonsterCatalog, book: SkillBook, seed: u64) -> HubInstance {
    let w = 60;
    let row = format!("#{}#", ".".repeat(w - 2));
    let wall = "#".repeat(w);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 38));
    rows.push(wall.as_str());
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let cfg = HordeConfig { light_radius: 30.0, director_enabled: false, seed: Some(seed), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), Arc::new(catalog), cfg);
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_skills(Arc::new(book)).with_horde(horde);
    stage.dice = Rng::new(seed);
    let profile = PlayerProfile {
        user_id: 1,
        character_id: 101,
        name: "p1".into(),
        archetype: "swordsman".into(),
        look: Look::default(),
        level: 40,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
    };
    let mut out = Vec::new();
    stage.join(profile, Uuid::nil(), None, 0, &mut out);
    let p = &mut stage.players[0];
    (p.x, p.y) = (30.0, 20.0);
    let f = p.fighter.as_mut().unwrap();
    (f.max_hp, f.hp) = (1_000_000_000, 1_000_000_000);
    f.recovery = u32::MAX;
    stage
}

/// What one monster did in a minute next to the player.
#[derive(Debug, Default, PartialEq)]
struct Tally {
    swings: u32,
    skills: u32,
    boss_casts: u32,
}

/// One `id` beside the player (at `hp_pct` of its HP, never dying) for a
/// minute.
fn minute_of(id: &str, hp_pct: i32) -> Option<Tally> {
    let mut stage = arena(5)?;
    let horde = stage.horde_mut().unwrap();
    let kind = horde.catalog().kind_of(id)?;
    let slot = horde.spawn(kind, 31.5, 20.0)?;
    let entity = MONSTER_ENTITY_BASE + slot as u32;
    let hz = stage.config.tick_hz;
    let mut tally = Tally::default();
    for _ in 0..MINUTE * hz {
        let horde = stage.horde_mut().unwrap();
        let top = horde.block(slot).hp;
        horde.store_mut().hp[slot as usize] = (top * hp_pct / 100).max(1);
        let mut out = Vec::new();
        stage.tick(&mut out);
        for (_, e) in out.iter().filter(|(u, _)| *u == 1) {
            match e {
                HubEvent::Control(ServerMsg::Combat { swings, .. }) => {
                    tally.swings += swings.iter().filter(|s| s.entity_id == entity).count() as u32;
                }
                HubEvent::Control(ServerMsg::Cast { caster, skill, .. }) if *caster == entity && !skill.is_empty() => tally.skills += 1,
                HubEvent::Control(ServerMsg::BossCast { caster, ability, .. }) if *caster == entity && !ability.is_empty() => {
                    tally.boss_casts += 1;
                }
                _ => {}
            }
        }
    }
    Some(tally)
}

/// Basic attacks and monster skills per minute of every plain monster in
/// the pack, kept under the readable ceilings of COMBAT.md "Cadence".
#[test]
fn plain_monsters_attack_at_a_readable_pace() {
    let Some(stage) = arena(5) else { return };
    let defs: Vec<(String, crate::world::combat::Archetype, bool)> = stage
        .horde
        .as_ref()
        .unwrap()
        .catalog()
        .defs()
        .iter()
        .filter(|d| !d.is_plant() && !super::boss::scripted(&d.id))
        .map(|d| (d.id.clone(), d.archetype, d.skills.is_empty()))
        .collect();
    // Every row is printed first (`--nocapture` shows the table), then judged.
    let mut bad = Vec::new();
    for (id, archetype, quiet) in defs {
        let t = minute_of(&id, 100).unwrap();
        println!("{id:>14} {:>7}: {:>3} swings/min, {:>2} skills/min", archetype.id(), t.swings, t.skills);
        if t.swings == 0 || t.swings > 40 || t.skills > 8 || (quiet && t.skills > 0) {
            bad.push(format!("{id}: {t:?}"));
        }
    }
    assert!(bad.is_empty(), "too busy: {bad:?}");
}

/// Ability starts per minute of every scripted elite and boss, in its first
/// and its last phase.
#[test]
fn scripted_bosses_pause_between_abilities() {
    if arena(5).is_none() {
        return;
    }
    let mut bad = Vec::new();
    for id in ["eclipse", "vocal", "mastering", "angeling"] {
        let Some(full) = minute_of(id, 100) else { continue };
        let low = minute_of(id, 10).unwrap();
        println!("{id:>10}: {:>2} abilities/min at full HP, {:>2} below its last phase; {} / {} swings", full.boss_casts, low.boss_casts, full.swings, low.swings);
        if full.boss_casts < 4 || full.boss_casts > 14 || low.boss_casts > 18 {
            bad.push(format!("{id}: {full:?} / {low:?}"));
        }
    }
    assert!(bad.is_empty(), "too busy: {bad:?}");
}

/// What a potion's tooltip promises is what a real stage gives: its whole
/// roll, spread over exactly `POTION_REGEN_MS`, a number once a second.
#[test]
fn potions_restore_what_their_tooltip_says_over_its_time() {
    use crate::world::combat::items::{self, Effect, Kind, POTION_REGEN_MS};
    use crate::world::combat::{Bag, Class};
    use crate::world::horde::catalog::test_def;
    let catalog = MonsterCatalog::new(vec![test_def("blob", 2.0, 1, 0.0, false)]).unwrap();
    for id in ["red_potion", "white_potion", "blue_potion", "apple", "grape"] {
        let mut stage = field(catalog.clone(), SkillBook::default(), 3);
        let mut bag = Bag::starting(Class::Mage, "");
        bag.add(id, 1).unwrap();
        let Kind::Consumable(effect) = items::find(id).unwrap().kind else { panic!("{id} is no consumable") };
        let (sp, dice) = match effect {
            Effect::Heal(d) => (false, d),
            Effect::Spirit(d) => (true, d),
            _ => unreachable!(),
        };
        let desc = &items::find(id).unwrap().desc;
        assert!(desc.contains(&format!("over {} s", POTION_REGEN_MS / 1000)), "{id}: {desc}");
        {
            let f = stage.players[0].fighter.as_mut().unwrap();
            f.wear(&bag);
            (f.max_hp, f.hp, f.max_sp, f.sp) = (10_000, 1, 10_000, 0);
        }
        let mut out = Vec::new();
        stage.use_item(1, id, &mut out);
        let f = stage.players[0].fighter.as_ref().unwrap();
        let total = f.regen[sp as usize].expect("it runs").total;
        assert!((dice.count as i32 + dice.bonus..=dice.count as i32 * dice.sides as i32 + dice.bonus).contains(&total), "{id}: {total}");
        let (hp0, sp0) = (f.hp, f.sp);
        let hz = stage.config.tick_hz;
        let mut ticks = 0;
        let mut shown = 0;
        while stage.players[0].fighter.as_ref().unwrap().regen[sp as usize].is_some() {
            // Never quiet, so the out-of-combat regeneration stays out of it.
            stage.players[0].fighter.as_mut().unwrap().quiet = 0;
            out.clear();
            stage.tick(&mut out);
            shown += out
                .iter()
                .filter_map(|(u, e)| match e {
                    HubEvent::Control(ServerMsg::Combat { hits, .. }) if *u == 1 => Some(hits.iter().filter(|h| h.target == 101 && h.flags == crate::world::protocol::hit::HEAL).map(|h| h.amount).sum::<u32>()),
                    _ => None,
                })
                .sum::<u32>();
            ticks += 1;
            assert!(ticks <= 10 * hz, "{id} never ends");
        }
        assert_eq!(ticks * 1000 / hz, POTION_REGEN_MS, "{id}: the time the tooltip names");
        let f = stage.players[0].fighter.as_ref().unwrap();
        if sp {
            // SP also regenerates on its own while standing: at least the roll.
            assert!(f.sp - sp0 >= total, "{id}: {} of {total} SP", f.sp - sp0);
        } else {
            assert_eq!(f.hp - hp0, total, "{id}: all of it");
            assert_eq!(shown as i32, total, "{id}: and every point shown");
        }
    }
}

/// Timed buffs that are windows, not states to keep up: Mystical
/// Amplification's empowered next spell and Enchant Deadly Poison's burst.
const BURSTS: &[&str] = &["mystical_amplification", "enchant_deadly_poison"];

/// Every timed buff a player casts again and again (Blessing, Increase AGI,
/// Improve Concentration, Endure, ...) outlasts its cooldown and wind-up at
/// its top level, so it can be kept up (auto-cast included). Play Dead's
/// few seconds of hiding are a reprieve, not a buff to keep up; [`BURSTS`]
/// are windows.
#[test]
fn buffs_at_their_top_level_outlast_their_cooldown() {
    use crate::world::combat::skills::{Effect, Payload};
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    let Ok(book) = SkillBook::load(&dist) else { return };
    let mut short = Vec::new();
    for (_, d) in book.iter().filter(|(_, d)| !d.passive && d.timed_buff(d.max_level) && !BURSTS.contains(&d.id.as_str())) {
        let top = d.max_level;
        for act in d.effects.iter().filter_map(Effect::act) {
            let Payload::Buff(b) = &act.payload else { continue };
            if b.toggle || b.hidden || act.timing.lasting(top) {
                continue;
            }
            let (secs, again) = (b.secs.at(top), d.cooldown.at(top) + d.cast_secs(top));
            println!("{:>24} lv {top:>2}: {secs:>5.1} s up, {again:>5.1} s to cast again", d.id);
            if secs <= again {
                short.push(format!("{} ({secs} s <= {again} s)", d.id));
            }
        }
    }
    assert!(short.is_empty(), "buffs that lapse between casts: {short:?}");
}

/// Fire Wall grows a flame a metre with its level (the client spaces its
/// flames 1 m along the wall's long side): 3 at level 1, 10 at level 10,
/// laid across the aim (thin along it).
#[test]
fn fire_wall_widens_with_its_level() {
    use crate::world::combat::skills::{Effect, Shape};
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    let Ok(book) = SkillBook::load(&dist) else { return };
    let (_, wall) = book.find("fire_wall").expect("fire wall");
    let act = wall.effects.iter().find_map(Effect::act).unwrap();
    assert_eq!(act.area.shape, Shape::Rect);
    // Areas are in cells (2 a metre); a flame each metre, both ends included.
    let flames = |slv: u8| (act.area.width.at(slv) / 2.0).floor() as u32 + 1;
    assert!(act.area.length.at(10) < act.area.width.at(1), "thin along the aim, wide across it");
    assert_eq!((flames(1), flames(5), flames(10)), (3, 6, 10));
}

/// Izlude's Food Vendor stands with the other merchants and sells the food
/// stock: cheap HP, and grapes for SP.
#[test]
fn izlude_has_a_food_vendor() {
    use crate::world::combat::items::{self, Effect, Kind};
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    let Ok(world) = HubWorld::load(&dist, &["izlude".into()], "izlude") else { return };
    let npcs = &world.map(world.entry()).manifest.npcs;
    let Some(vendor) = npcs.iter().find(|n| n.id == "food_vendor") else { panic!("no food vendor in izlude") };
    assert_eq!((vendor.kind.as_str(), vendor.shop.as_str()), (super::SHOP, "food"));
    let tools = npcs.iter().find(|n| n.id == "tool_dealer").unwrap();
    assert!((vendor.x as i32 - tools.x as i32).abs() + (vendor.y as i32 - tools.y as i32).abs() <= 12, "beside the Tool Dealer");
    let stock = items::shop("food").unwrap();
    let red = items::find("red_potion").unwrap().price;
    for id in stock {
        let e = items::find(id).unwrap();
        assert!(matches!(e.kind, Kind::Consumable(Effect::Heal(_) | Effect::Spirit(_))), "{id} restores something");
        assert!(e.price < red, "{id} is cheaper than a Red Potion");
    }
    assert!(stock.iter().any(|id| matches!(items::find(id).unwrap().kind, Kind::Consumable(Effect::Spirit(_)))), "grapes for SP");
}
