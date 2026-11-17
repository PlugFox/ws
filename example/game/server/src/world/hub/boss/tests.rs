use super::*;
use crate::world::combat::{Archetype, DamageType};
use crate::world::horde::catalog::{test_def, MonsterCatalog, MonsterDef};
use crate::world::horde::{Horde, HordeConfig};
use crate::world::hub::test_support::{map, portal};
use crate::world::hub::{HubSimConfig, HubWorld, PlayerProfile};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::Look;
use std::sync::Arc;
use uuid::Uuid;

fn open(w: usize, h: usize) -> WalkGrid {
    let wall = "#".repeat(w);
    let row = format!("#{}#", ".".repeat(w - 2));
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), h - 2));
    rows.push(wall.as_str());
    WalkGrid::from_ascii(&rows)
}

fn shapes(p: Pattern, a: Action, walk: &WalkGrid, origin: (f32, f32), anchor: (f32, f32)) -> Vec<Shape> {
    let mut out = Vec::new();
    expand(&p, &a, walk, origin, anchor, (1.0, 0.0), 0.0, 1.0, &mut out);
    out
}

/// Which beats (by delay) hurt a body at `(x, y)`.
fn beats_on(v: &[Shape], x: f32, y: f32) -> Vec<f32> {
    v.iter().filter(|s| !s.safe && s.zone.contains(x, y, 0.0)).map(|s| s.delay).collect()
}

#[test]
fn rings_expand_in_beats_and_leave_safe_gaps() {
    let walk = open(60, 60);
    let v = shapes(Pattern::Rings { count: 3, start: 2.0, width: 2.0, step: 4.0, every: 0.5 }, Action::None, &walk, (30.0, 30.0), (30.0, 30.0));
    assert_eq!(v.len(), 3);
    assert_eq!(beats_on(&v, 33.0, 30.0), vec![0.0], "the first ring, 2..4");
    assert!(beats_on(&v, 35.0, 30.0).is_empty(), "the quiet gap between rings");
    assert_eq!(beats_on(&v, 30.0, 36.5), vec![0.5], "the second ring, 6..8, half a second later");
    assert_eq!(beats_on(&v, 30.0 - 10.5, 30.0), vec![1.0]);
    assert!(beats_on(&v, 30.0, 30.0).is_empty(), "the centre is calm (start 2)");
    // A wave from the centre starts as a full circle.
    let v = shapes(Pattern::Rings { count: 2, start: 0.0, width: 3.0, step: 5.0, every: 0.5 }, Action::None, &walk, (30.0, 30.0), (30.0, 30.0));
    assert_eq!(beats_on(&v, 30.0, 30.0), vec![0.0]);
}

#[test]
fn a_donut_spares_its_feet_and_the_far_field() {
    let walk = open(60, 60);
    let v = shapes(Pattern::Donut { inner: 3.5, outer: 15.0 }, Action::None, &walk, (30.0, 30.0), (30.0, 30.0));
    assert!(beats_on(&v, 32.0, 30.0).is_empty(), "stand close");
    assert_eq!(beats_on(&v, 30.0, 38.0).len(), 1);
    assert!(beats_on(&v, 30.0, 46.0).is_empty(), "or very far");
    // Bodies touching the inner edge count as inside the ring.
    assert!(v[0].zone.contains(33.3, 30.0, BODY));
    let t = v[0].zone.telegraph(0, 7, 0);
    assert_eq!((t.shape, t.a, t.b), (3, 15.0, 3.5));
}

#[test]
fn a_cross_turns_between_beats() {
    let walk = open(80, 80);
    let v = shapes(
        Pattern::Cross { arms: 2, len: 36.0, width: 3.0, count: 2, step: 45.0, every: 0.9 },
        Action::None,
        &walk,
        (40.0, 40.0),
        (40.0, 40.0),
    );
    assert_eq!(v.len(), 4, "two bars a beat, two beats");
    assert_eq!(beats_on(&v, 50.0, 40.0), vec![0.0], "east: the plus");
    assert_eq!(beats_on(&v, 40.0, 30.0), vec![0.0], "south: the plus");
    let d = 10.0 * std::f32::consts::FRAC_1_SQRT_2;
    assert_eq!(beats_on(&v, 40.0 + d, 40.0 + d), vec![0.9], "the diagonal: only the X");
    assert!(beats_on(&v, 47.0, 43.0).is_empty(), "between the bars");
}

#[test]
fn a_sweep_turns_one_cone_a_beat() {
    let walk = open(60, 60);
    let v = shapes(Pattern::Sweep { count: 4, r: 12.0, arc: 60.0, step: 90.0, every: 0.4 }, Action::None, &walk, (30.0, 30.0), (30.0, 30.0));
    assert_eq!(beats_on(&v, 38.0, 30.0), vec![0.0], "it starts at the aim (east)");
    assert_eq!(beats_on(&v, 30.0, 38.0), vec![0.4], "then turns counter-clockwise (north)");
    assert_eq!(beats_on(&v, 22.0, 30.0), vec![0.8]);
    assert!(beats_on(&v, 36.0, 36.0).is_empty(), "between the cones");
    assert!(beats_on(&v, 43.0, 30.0).is_empty(), "out of its reach");
}

#[test]
fn a_charge_stops_at_the_wall_and_lands_there() {
    let walk = open(30, 20);
    let v = shapes(Pattern::Line { len: 16.0, width: 2.4 }, Action::Charge, &walk, (20.0, 10.0), (25.0, 10.0));
    assert_eq!(v.len(), 1);
    let (px, py) = v[0].place.expect("a charge lands");
    assert!(px < 29.0 && px > 27.0 && (py - 10.0).abs() < 1e-3, "it stops before the wall: {px}");
    assert!(v[0].zone.contains(26.0, 10.5, 0.0) && !v[0].zone.contains(26.0, 12.0, 0.0), "the line is as wide as the charge");
    assert!(!v[0].zone.contains(29.6, 10.0, 0.0), "and ends where it stops");
    // Bolts are lines too, but nothing moves.
    let v = shapes(Pattern::Line { len: 24.0, width: 1.6 }, Action::None, &walk, (5.0, 10.0), (5.0, 15.0));
    assert!(v[0].place.is_none() && v[0].zone.contains(5.0, 14.0, 0.0));
}

#[test]
fn hops_grow_toward_the_target_and_each_lands() {
    let walk = open(80, 20);
    let v = shapes(Pattern::Hops { count: 3, first: 4.0, grow: 2.0, r: 2.4, every: 0.45 }, Action::Leap, &walk, (5.0, 10.0), (40.0, 10.0));
    let places: Vec<f32> = v.iter().map(|s| s.place.unwrap().0).collect();
    let near = |a: f32, b: f32| (a - b).abs() < 0.3;
    assert!(near(places[0], 9.0) && near(places[1], 15.0) && near(places[2], 23.0), "4, 6, 8 cells: {places:?}");
    assert_eq!(v.iter().map(|s| s.delay).collect::<Vec<_>>(), vec![0.0, 0.45, 0.9]);
}

#[test]
fn a_sanctuary_nova_rings_its_caster_with_safe_spots() {
    let walk = open(60, 60);
    let mut out = Vec::new();
    let p = Pattern::Sanctuary { r: 40.0, safe: 3, safe_r: 2.6, dist: 9.0 };
    expand(&p, &Action::None, &walk, (30.0, 30.0), (30.0, 30.0), (1.0, 0.0), 0.5, 0.8, &mut out);
    assert_eq!(out.iter().filter(|s| s.safe).count(), 3);
    assert_eq!(out.iter().filter(|s| !s.safe).count(), 1, "one nova");
    for s in out.iter().filter(|s| s.safe) {
        let Zone::Area(Area::Circle { x, y, r }) = s.zone else { panic!("sanctuaries are circles") };
        assert!((((x - 30.0).powi(2) + (y - 30.0).powi(2)).sqrt() - 9.0).abs() < 0.1 && r == 2.6);
    }
    // Against a wall a sanctuary that cannot stand clear of the nova is dropped.
    let walk = open(12, 60);
    let mut out = Vec::new();
    expand(&p, &Action::None, &walk, (6.0, 30.0), (6.0, 30.0), (1.0, 0.0), 0.0, 1.0, &mut out);
    assert!(out.iter().filter(|s| s.safe).all(|s| matches!(s.zone, Zone::Area(Area::Circle { x, .. }) if (1.0..11.0).contains(&x))));
}

// --- On a stage ------------------------------------------------------------

/// A 60 × 40 field with the four scripted monsters and a poring; player 1
/// (a level 40 swordsman) joins at the entry.
fn arena(seed: u64) -> HubInstance {
    let w = 60;
    let row = format!("#{}#", ".".repeat(w - 2));
    let wall = "#".repeat(w);
    let mut rows = vec![wall.as_str()];
    rows.extend(std::iter::repeat_n(row.as_str(), 38));
    rows.push(wall.as_str());
    let field = map(
        "field",
        LocationKind::Field,
        CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
        &rows,
        vec![portal("to_town", [1, 1, 1], "town", [3, 3])],
    );
    let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
    let elite = |id: &str| MonsterDef { archetype: Archetype::Elite, radius: 0.6, ..test_def(id, 2.0, 12, 0.0, false) };
    let catalog = Arc::new(
        MonsterCatalog::new(vec![
            test_def("angeling", 2.0, 50, 0.0, true),
            elite("eclipse"),
            elite("mastering"),
            elite("vocal"),
            test_def("poring", 2.0, 1, 0.0, false),
        ])
        .unwrap(),
    );
    let cfg = HordeConfig { light_radius: 30.0, director_enabled: false, seed: Some(seed), ..HordeConfig::default() };
    let horde = Horde::new(Arc::clone(&world), catalog, cfg);
    let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde);
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
        homunculus: None,
    };
    let mut out = Vec::new();
    stage.join(profile, Uuid::nil(), None, 0, &mut out);
    stage
}

fn kind(stage: &mut HubInstance, id: &str) -> u8 {
    stage.horde_mut().unwrap().catalog().kind_of(id).unwrap()
}

/// Player 1 stands at `(x, y)`, sturdy enough to take a long fight and
/// holding their blows (the tests set the monsters' HP).
fn stand(stage: &mut HubInstance, x: f32, y: f32) {
    let p = &mut stage.players[0];
    (p.x, p.y) = (x, y);
    let f = p.fighter.as_mut().unwrap();
    (f.max_hp, f.hp) = (1_000_000, 1_000_000);
    f.recovery = u32::MAX;
}

fn controls(out: &Outgoing) -> Vec<ServerMsg> {
    out.iter()
        .filter(|(u, _)| *u == 1)
        .filter_map(|(_, e)| match e {
            HubEvent::Control(m) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

fn run(stage: &mut HubInstance, ticks: u32) -> Vec<ServerMsg> {
    let mut out = Vec::new();
    for _ in 0..ticks {
        stage.tick(&mut out);
    }
    controls(&out)
}

/// Sets the monster's HP to `pct` % of its maximum.
fn wound(stage: &mut HubInstance, slot: u16, pct: i32) -> i32 {
    let horde = stage.horde_mut().unwrap();
    let max = horde.block(slot).hp;
    let hp = (max * pct / 100).max(1);
    horde.store_mut().hp[slot as usize] = hp;
    hp
}

fn fight(stage: &HubInstance) -> Fight {
    stage.bosses.fights()[0]
}

#[test]
fn a_boss_fight_telegraphs_then_its_beats_land_on_the_player() {
    let mut stage = arena(7);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "angeling");
    let slot = stage.horde_mut().unwrap().spawn(k, 36.0, 20.0).unwrap();
    let msgs = run(&mut stage, 60);
    assert!(
        msgs.iter().any(|m| matches!(m, ServerMsg::BossShout { text, style: 1, .. } if text.starts_with("Mortals"))),
        "it yells as the fight starts"
    );
    assert!(
        msgs.iter().any(|m| matches!(m, ServerMsg::BossBar(b) if b.rank == 2 && b.name == "angeling" && b.pips == vec![70, 40, 15])),
        "its bar with the phase pips"
    );
    assert!(!msgs.iter().any(|m| matches!(m, ServerMsg::Boss { .. })), "no plain boss bar for a scripted boss");
    let cast = msgs.iter().find_map(|m| match m {
        ServerMsg::BossCast { caster, ability, cast_ms, .. } => Some((*caster, ability.clone(), *cast_ms)),
        _ => None,
    });
    assert_eq!(cast, Some((MONSTER_ENTITY_BASE + slot as u32, "boss_holy_bolts".into(), 900)), "its rotation opens with holy bolts");
    let zones = msgs
        .iter()
        .find_map(|m| match m {
            ServerMsg::BossZones { zones, .. } => Some(zones.clone()),
            _ => None,
        })
        .expect("the bolts are telegraphed");
    assert_eq!(zones.len(), 1, "one bolt per player");
    assert_eq!((zones[0].area.shape, zones[0].delay_ms), (1, 900));
    let hurt = msgs.iter().any(|m| {
        matches!(m, ServerMsg::Combat { hits, .. } if hits.iter().any(|h| h.target == 101 && h.amount > 0 && h.flags >> 4 == DamageType::Radiant as u8))
    });
    assert!(hurt, "the bolt lands on the player who stood still");
}

#[test]
fn stepping_out_of_a_telegraph_dodges_it() {
    let mut stage = arena(7);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "angeling");
    stage.horde_mut().unwrap().spawn(k, 36.0, 20.0).unwrap();
    let mut out = Vec::new();
    // Until the bolt is telegraphed...
    while !controls(&out).iter().any(|m| matches!(m, ServerMsg::BossZones { .. })) {
        stage.tick(&mut out);
    }
    // ...then sidestep it before it lands.
    stage.players[0].y = 26.0;
    out.clear();
    for _ in 0..30 {
        stage.tick(&mut out);
    }
    let radiant = controls(&out).iter().any(|m| {
        matches!(m, ServerMsg::Combat { hits, .. } if hits.iter().any(|h| h.target == 101 && h.flags >> 4 == DamageType::Radiant as u8))
    });
    assert!(!radiant, "a dodged bolt does nothing");
}

#[test]
fn phases_follow_hp_and_the_nova_needs_a_sanctuary_or_a_broken_shield() {
    let mut stage = arena(3);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "angeling");
    let slot = stage.horde_mut().unwrap().spawn(k, 33.0, 20.0).unwrap();
    run(&mut stage, 30);
    assert!(fight(&stage).engaged() && fight(&stage).phase() == 0);

    wound(&mut stage, slot, 65);
    let msgs = run(&mut stage, 2);
    assert_eq!(fight(&stage).phase(), 1);
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossShout { text, .. } if text.starts_with("Rise"))));

    // Straight to the third phase: its opener is the shielded nova.
    wound(&mut stage, slot, 35);
    let mut msgs = Vec::new();
    for _ in 0..200 {
        msgs.extend(run(&mut stage, 1));
        if msgs.iter().any(|m| matches!(m, ServerMsg::BossCast { ability, .. } if ability == "boss_holy_nova")) {
            break;
        }
    }
    assert_eq!(fight(&stage).phase(), 2);
    let (flags, ms) = msgs
        .iter()
        .find_map(|m| match m {
            ServerMsg::BossCast { ability, flags, cast_ms, .. } if ability == "boss_holy_nova" => Some((*flags, *cast_ms)),
            _ => None,
        })
        .expect("the nova is cast after the phase change");
    assert_eq!((flags, ms), (cast_flags::SHIELDED, 5400), "6 s × the phase's haste 0.9");
    let zones = msgs
        .iter()
        .rev()
        .find_map(|m| match m {
            ServerMsg::BossZones { ability, zones, .. } if ability == "boss_holy_nova" => Some(zones.clone()),
            _ => None,
        })
        .unwrap();
    let safe: Vec<&BossZone> = zones.iter().filter(|z| z.flags & zone_flags::SAFE != 0).collect();
    assert_eq!(safe.len(), 3, "three sanctuaries");
    let shield = fight(&stage).shield();
    assert!(shield > 0, "a shield goes up");

    // Damage is soaked by the shield...
    let hp = stage.horde_mut().unwrap().store().hp[slot as usize];
    stage.horde_mut().unwrap().store_mut().hp[slot as usize] = hp - shield / 2;
    run(&mut stage, 1);
    assert!(stage.horde_mut().unwrap().store().hp[slot as usize] >= hp - 50, "the shield takes the blow");
    assert!(fight(&stage).shield() < shield);

    // ...hide in a sanctuary and the nova passes over.
    let (sx, sy) = (safe[0].area.x, safe[0].area.y);
    stand(&mut stage, sx, sy);
    let msgs = run(&mut stage, 120);
    let nova = msgs.iter().any(|m| {
        matches!(m, ServerMsg::Combat { hits, .. } if hits.iter().any(|h| h.target == 101 && h.amount > 50 && h.flags >> 4 == DamageType::Radiant as u8))
    });
    assert!(!nova, "the sanctuary spares its guest");
}

#[test]
fn breaking_the_shield_cancels_the_nova() {
    let mut stage = arena(3);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "angeling");
    let slot = stage.horde_mut().unwrap().spawn(k, 33.0, 20.0).unwrap();
    run(&mut stage, 30);
    wound(&mut stage, slot, 35);
    for _ in 0..200 {
        run(&mut stage, 1);
        if fight(&stage).shield() > 0 {
            break;
        }
    }
    let shield = fight(&stage).shield();
    assert!(shield > 0);
    let hp = stage.horde_mut().unwrap().store().hp[slot as usize];
    stage.horde_mut().unwrap().store_mut().hp[slot as usize] = hp - shield - 10;
    let msgs = run(&mut stage, 1);
    assert!(
        msgs.iter().any(|m| matches!(m, ServerMsg::BossCast { ability, flags, .. } if ability.is_empty() && flags & cast_flags::INTERRUPTED != 0)),
        "the cast bar shows the break"
    );
    assert!(stage.horde_mut().unwrap().store().status_of(slot).has(Condition::Stunned), "it reels");
    let msgs = run(&mut stage, 140);
    let nova = msgs.iter().any(|m| matches!(m, ServerMsg::Combat { hits, .. } if hits.iter().any(|h| h.target == 101 && h.amount > 50)));
    assert!(!nova, "no nova after the shield broke");
}

#[test]
fn bursting_a_channel_breaks_the_song() {
    let mut stage = arena(5);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "vocal");
    let slot = stage.horde_mut().unwrap().spawn(k, 33.0, 20.0).unwrap();
    stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 1_000_000;
    run(&mut stage, 30);
    // Phase two opens with the Song of Rousing (a channel).
    wound(&mut stage, slot, 55);
    let mut msgs = Vec::new();
    // The opener waits out the running ability and its pause.
    for _ in 0..240 {
        msgs.extend(run(&mut stage, 1));
        if fight(&stage).channel.is_some() {
            break;
        }
    }
    let c = fight(&stage).channel.expect("it channels");
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossCast { ability, flags, cast_ms: 4000, .. } if ability == "boss_song_of_rousing" && *flags == cast_flags::CHANNEL)));
    let hp = stage.horde_mut().unwrap().store().hp[slot as usize];
    stage.horde_mut().unwrap().store_mut().hp[slot as usize] = hp - c.breaks;
    let msgs = run(&mut stage, 1);
    assert!(fight(&stage).channel.is_none());
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossShout { text, .. } if text == BROKEN)));
    assert!(stage.horde_mut().unwrap().store().status_of(slot).has(Condition::Stunned));
}

#[test]
fn an_unbroken_song_heals_the_crowd() {
    let mut stage = arena(5);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "vocal");
    let poring = kind(&mut stage, "poring");
    let slot = stage.horde_mut().unwrap().spawn(k, 33.0, 20.0).unwrap();
    let friend = stage.horde_mut().unwrap().spawn(poring, 40.0, 25.0).unwrap();
    run(&mut stage, 30);
    wound(&mut stage, slot, 55);
    stage.horde_mut().unwrap().store_mut().hp[friend as usize] = 1;
    for _ in 0..200 {
        run(&mut stage, 1);
        if fight(&stage).channel.is_some() {
            break;
        }
    }
    // Keep the player's blows from breaking it: stand far off.
    stand(&mut stage, 10.0, 35.0);
    let before = stage.horde_mut().unwrap().store().hp[slot as usize];
    run(&mut stage, 90);
    let store = stage.horde_mut().unwrap().store();
    assert!(store.hp[slot as usize] > before, "it heals itself");
    assert!(!store.is_alive(friend) || store.hp[friend as usize] > 1, "and the monsters around");
}

#[test]
fn mastering_calls_adds_and_swallows_the_survivors() {
    let mut stage = arena(9);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "mastering");
    let poring = kind(&mut stage, "poring");
    let slot = stage.horde_mut().unwrap().spawn(k, 34.0, 20.0).unwrap();
    // Its first phase opens with Call Porings.
    run(&mut stage, 60);
    let adds = stage.horde_mut().unwrap().monsters().filter(|m| m.2 == poring).count();
    assert_eq!(adds, 3, "three porings for a solo player");
    assert_eq!(fight(&stage).n_adds, 3);
    // Wounded, it reaches Reabsorb in its rotation and eats them.
    wound(&mut stage, slot, 70);
    let hp = stage.horde_mut().unwrap().store().hp[slot as usize];
    stand(&mut stage, 5.0, 35.0);
    let mut msgs = Vec::new();
    for _ in 0..1200 {
        msgs.extend(run(&mut stage, 1));
        if msgs.iter().any(|m| matches!(m, ServerMsg::BossCast { ability, .. } if ability == "boss_reabsorb")) && fight(&stage).channel.is_none() {
            break;
        }
    }
    let left = stage.horde_mut().unwrap().monsters().filter(|m| m.2 == poring).count();
    assert!(left < adds, "the adds it reached are gone ({left} left)");
    assert!(stage.horde_mut().unwrap().store().hp[slot as usize] > hp, "and it healed on them");
}

#[test]
fn eclipse_charges_through_and_lands_where_the_line_ends() {
    let mut stage = arena(11);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "eclipse");
    let slot = stage.horde_mut().unwrap().spawn(k, 36.0, 20.0).unwrap();
    let mut out = Vec::new();
    let mut zone = None;
    for _ in 0..80 {
        stage.tick(&mut out);
        zone = controls(&out).iter().find_map(|m| match m {
            ServerMsg::BossZones { ability, zones, .. } if ability == "boss_moon_charge" => Some(zones[0]),
            _ => None,
        });
        if zone.is_some() {
            break;
        }
    }
    let z = zone.expect("a moon charge");
    assert_eq!(z.area.shape, 1);
    assert!(z.area.x < 36.0, "the line runs toward the player");
    run(&mut stage, z.delay_ms / 50 + 2);
    let x = stage.horde_mut().unwrap().store().x[slot as usize];
    assert!(x < 24.0, "it charged through, past the player ({x})");
}

#[test]
fn eclipse_leaps_are_flagged_and_it_lands_on_them() {
    use crate::world::protocol::zone_flags;
    let mut stage = arena(11);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "eclipse");
    let slot = stage.horde_mut().unwrap().spawn(k, 36.0, 20.0).unwrap();
    let mut out = Vec::new();
    let mut leap = None;
    for _ in 0..2400 {
        out.clear();
        stage.tick(&mut out);
        for m in controls(&out) {
            if let ServerMsg::BossZones { ability, zones, .. } = m {
                // A charge slides; only leaps land on their zones.
                let lands = zones.iter().all(|z| z.flags & zone_flags::LANDS != 0);
                match ability.as_str() {
                    "boss_moon_charge" => assert!(zones.iter().all(|z| z.flags & zone_flags::LANDS == 0)),
                    "boss_moon_pounce" | "boss_moon_bound" => {
                        assert!(lands, "{ability}: {zones:?}");
                        leap = zones.iter().max_by_key(|z| z.delay_ms).copied();
                    }
                    _ => {}
                }
            }
        }
        if leap.is_some() {
            break;
        }
    }
    let z = leap.expect("a pounce or a bounding run");
    run(&mut stage, z.delay_ms / 50 + 2);
    let store = stage.horde_mut().unwrap().store();
    let (x, y) = (store.x[slot as usize], store.y[slot as usize]);
    assert!((x - z.area.x).abs() < 0.2 && (y - z.area.y).abs() < 0.2, "it lands on the last zone: ({x}, {y}) vs {:?}", z.area);
}

#[test]
fn the_fight_ends_with_its_bar_and_its_ground() {
    let mut stage = arena(13);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "mastering");
    let slot = stage.horde_mut().unwrap().spawn(k, 33.0, 20.0).unwrap();
    // Until a slime pool is down.
    let mut msgs = Vec::new();
    for _ in 0..800 {
        msgs.extend(run(&mut stage, 1));
        if msgs.iter().any(|m| matches!(m, ServerMsg::BossZones { ability, .. } if ability == "boss_slime_rain")) {
            break;
        }
    }
    let pool = msgs
        .iter()
        .find_map(|m| match m {
            ServerMsg::BossZones { ability, zones, .. } if ability == "boss_slime_rain" => Some(zones[0].area.id),
            _ => None,
        })
        .expect("slime rain");
    assert!(pool > 0, "a pool is lasting ground");
    // Killed: the field takes it away at zero HP.
    stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 0;
    stage.horde_mut().unwrap().despawn(slot);
    let msgs = run(&mut stage, 1);
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossBar(b) if b.rank == 0)), "the bar goes");
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::ZoneEnd { id } if *id == pool)), "its pools dry up");
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossShout { text, .. } if text == "Pop...")));
    assert!(stage.bosses.fights().is_empty() && stage.bosses.beats.is_empty());
}

#[test]
fn a_seeded_fight_replays_exactly() {
    let script = |seed| {
        let mut stage = arena(seed);
        stand(&mut stage, 30.0, 20.0);
        let k = kind(&mut stage, "angeling");
        let slot = stage.horde_mut().unwrap().spawn(k, 36.0, 20.0).unwrap();
        let mut log = run(&mut stage, 200);
        wound(&mut stage, slot, 30);
        log.extend(run(&mut stage, 300));
        log.into_iter().filter(|m| matches!(m, ServerMsg::BossZones { .. } | ServerMsg::BossCast { .. } | ServerMsg::Combat { .. })).collect::<Vec<_>>()
    };
    let a = script(21);
    assert!(a.len() > 10);
    assert_eq!(a, script(21));
}

#[test]
fn enrage_follows_time() {
    let mut stage = arena(2);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "eclipse");
    let slot = stage.horde_mut().unwrap().spawn(k, 36.0, 20.0).unwrap();
    run(&mut stage, 30);
    assert!(!fight(&stage).enraged());
    // Two and a half minutes on.
    stage.tick += 150 * stage.tick_hz();
    stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 1_000_000;
    let msgs = run(&mut stage, 2);
    assert!(fight(&stage).enraged());
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossShout { text, .. } if text.starts_with("Too slow"))));
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossBar(b) if b.flags & 1 != 0)), "the bar shows it");
    // Enraged wind-ups shrink, but a damaging one never under 0.8 s (its
    // Bounding Run winds up 0.9 s: × 0.85 would be 0.77 s).
    let msgs = run(&mut stage, 400);
    let winds: Vec<u32> = msgs.iter().filter_map(|m| if let ServerMsg::BossCast { cast_ms, .. } = m { Some(*cast_ms) } else { None }).collect();
    assert!(winds.len() >= 3 && winds.iter().all(|&w| w >= 800), "wind-ups stay dodgeable: {winds:?}");
}

#[test]
fn an_elite_left_behind_drops_out_and_takes_its_bar() {
    let mut stage = arena(4);
    stand(&mut stage, 30.0, 20.0);
    let k = kind(&mut stage, "eclipse");
    let slot = stage.horde_mut().unwrap().spawn(k, 44.0, 20.0).unwrap();
    run(&mut stage, 30);
    assert!(fight(&stage).engaged());
    // It reels (no new abilities); the player walks far off.
    stage.horde_mut().unwrap().store_mut().afflict(slot, Condition::Stunned, 2000);
    stand(&mut stage, 2.0, 38.0);
    let msgs = run(&mut stage, 20 * 11);
    assert!(!fight(&stage).engaged());
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossBar(b) if b.rank == 0)), "its bar goes");
    // Back in range: the fight picks up again (with its intro).
    stand(&mut stage, 40.0, 20.0);
    let msgs = run(&mut stage, 20);
    assert!(fight(&stage).engaged());
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::BossBar(b) if b.rank == 1)));
}

// --- The ladder's DSL: rain, push and pull, shared blows ---------------------

#[test]
fn rain_falls_on_open_ground_round_the_anchor_one_drop_at_a_time() {
    let walk = open(60, 40);
    let mut dice = Rng::new(11);
    let mut out = Vec::new();
    rain(&walk, &mut dice, (30.0, 20.0), 8, 2.2, 9.0, 0.3, 0.8, &mut out);
    assert_eq!(out.len(), 8);
    for (i, s) in out.iter().enumerate() {
        let Zone::Area(Area::Circle { x, y, r }) = s.zone else { panic!("raindrops are circles") };
        assert!(((x - 30.0).powi(2) + (y - 30.0 + 10.0).powi(2)).sqrt() <= 9.0 + 1e-3, "within the spread: ({x}, {y})");
        assert!(walk.is_walkable(x, y) && r == 2.2 && !s.safe && s.place.is_none());
        assert!((s.delay - i as f32 * 0.3 * 0.8).abs() < 1e-4, "one every 0.3 s × the pace");
    }
    // The same dice, the same storm; another seed, another.
    let mut again = Vec::new();
    rain(&walk, &mut Rng::new(11), (30.0, 20.0), 8, 2.2, 9.0, 0.3, 0.8, &mut again);
    assert_eq!(out, again);
    // Against the wall no drop lands in it; with no open ground none lands.
    let mut edge = Vec::new();
    rain(&walk, &mut Rng::new(3), (1.5, 20.0), 12, 2.0, 6.0, 0.25, 1.0, &mut edge);
    assert!(!edge.is_empty() && edge.iter().all(|s| matches!(s.zone, Zone::Area(Area::Circle { x, y, .. }) if walk.is_walkable(x, y))));
    let mut none = Vec::new();
    rain(&walk, &mut Rng::new(3), (-20.0, -20.0), 5, 2.0, 3.0, 0.25, 1.0, &mut none);
    assert!(none.is_empty());
    // The pattern itself is expanded at cast time, not by `expand`.
    assert!(shapes(Pattern::Rain { count: 4, per_player: 2, r: 2.0, spread: 6.0, every: 0.3 }, Action::None, &walk, (30.0, 20.0), (30.0, 20.0)).is_empty());
}

/// A blow of `hurt` on the arena's player standing at `(x, y)`.
fn blow_on(stage: &mut HubInstance, zone: Zone, hurt: Hurt, x: f32, y: f32) -> (f32, f32, Vec<HitEvent>) {
    stand(stage, x, y);
    let walk = open(60, 40);
    let map = stage.players[0].map;
    let b = Blow { zone, hurt, map, safe: &[], level: 0, dc: 10, boost: 1.0, evil: false, from: (0.0, 0.0) };
    let mut hits = Vec::new();
    land(&b, &mut stage.players, &stage.lives, &walk, &mut stage.dice, 20.0, &mut hits);
    (stage.players[0].x, stage.players[0].y, hits)
}

#[test]
fn a_push_throws_the_victim_away_and_walls_stop_it() {
    let mut stage = arena(5);
    let push = |p: f32| Hurt { push: p, save: false, ..scripts::hurt(1, 4, DamageType::Force) };
    let near = |a: f32, b: f32| (a - b).abs() < 0.3;
    // Away from a circle's centre.
    let (x, y, hits) = blow_on(&mut stage, circle(28.0, 20.0, 4.0), push(3.0), 30.0, 20.0);
    assert!(near(x, 33.0) && near(y, 20.0), "thrown 3 cells east: ({x}, {y})");
    assert!(hits.iter().any(|h| h.target == 101 && h.amount > 0), "and hurt");
    // A wall stops it.
    let (x, _, _) = blow_on(&mut stage, circle(55.0, 20.0, 4.0), push(8.0), 57.0, 20.0);
    assert!(x < 59.0 && x > 57.5, "stopped at the wall: {x}");
    // Negative pulls in, never past the centre.
    let (x, _, _) = blow_on(&mut stage, circle(30.0, 20.0, 6.0), push(-3.0), 31.0, 20.0);
    assert!(near(x, 30.0), "pulled onto the centre, not through it: {x}");
    let (x, _, _) = blow_on(&mut stage, circle(30.0, 20.0, 6.0), push(-2.0), 35.0, 20.0);
    assert!(near(x, 33.0), "pulled 2 cells in: {x}");
    // A charge's bar throws sideways out of its line.
    let line = Zone::Area(Area::line(20.0, 20.0, (1.0, 0.0), 16.0, 3.0));
    let (x, y, _) = blow_on(&mut stage, line, push(2.0), 25.0, 20.5);
    assert!(near(x, 25.0) && near(y, 22.5), "aside: ({x}, {y})");
    let (_, y, _) = blow_on(&mut stage, line, push(2.0), 25.0, 19.5);
    assert!(near(y, 17.5), "the other side: {y}");
    // A ring throws outward from its centre; a cone from its apex.
    let ring = Zone::Ring { x: 30.0, y: 20.0, inner: 3.0, outer: 6.0 };
    let (_, y, _) = blow_on(&mut stage, ring, push(2.0), 30.0, 24.0);
    assert!(near(y, 26.0), "outward: {y}");
    // No push: nobody moves.
    let (x, y, _) = blow_on(&mut stage, circle(28.0, 20.0, 4.0), push(0.0), 30.0, 20.0);
    assert!((x, y) == (30.0, 20.0));
}

#[test]
fn lasting_ground_drags_toward_its_centre_one_step_a_tick() {
    let mut stage = arena(5);
    stand(&mut stage, 34.0, 20.0);
    let walk = open(60, 40);
    let map = stage.players[0].map;
    let pool = circle(30.0, 20.0, 6.0);
    pull_in(&pool, map, 0.5, &mut stage.players, &walk);
    assert!((stage.players[0].x - 33.5).abs() < 1e-3);
    for _ in 0..20 {
        pull_in(&pool, map, 0.5, &mut stage.players, &walk);
    }
    assert!((stage.players[0].x - 30.0).abs() < 1e-3, "it holds them at the centre: {}", stage.players[0].x);
    // Negative pushes out, until they leave it.
    for _ in 0..40 {
        pull_in(&pool, map, -0.5, &mut stage.players, &walk);
    }
    let x = stage.players[0].x;
    assert!(x > 36.0 && x < 37.0, "pushed out to the rim: {x}");
    // Out of it: untouched.
    stand(&mut stage, 45.0, 20.0);
    pull_in(&pool, map, 0.5, &mut stage.players, &walk);
    assert_eq!(stage.players[0].x, 45.0);
}

#[test]
fn a_split_blow_shares_one_roll_and_caps_a_lone_victim() {
    let mut stage = arena(9);
    let big = Hurt { save: false, ..scripts::hurt(28, 6, DamageType::Force) };
    let shared = Hurt { split: 8, ..big };
    let zone = circle(30.0, 20.0, 4.5);
    let taken = |stage: &mut HubInstance, h: Hurt| -> u32 { blow_on(stage, zone, h, 30.0, 20.0).2.iter().filter(|e| e.target == 101).map(|e| e.amount).sum() };
    let (mut full, mut alone) = (0, 0);
    for _ in 0..20 {
        full += taken(&mut stage, big);
        alone += taken(&mut stage, shared);
    }
    // A lone victim takes 8 of the 28 dice's worth.
    let ratio = alone as f32 / full as f32;
    assert!(ratio > 0.2 && ratio < 0.37, "8/28 of the roll alone: {ratio} ({alone} of {full})");
    for _ in 0..20 {
        assert!(taken(&mut stage, shared) <= 48, "never more than 8d6 alone");
    }

    // Two inside: one roll, each the same share.
    let mut p2 = stage.players[0].profile.clone();
    (p2.user_id, p2.character_id) = (2, 102);
    let mut out = Vec::new();
    stage.join(p2, Uuid::nil(), None, 0, &mut out);
    let i = stage.players.iter().position(|p| p.profile.user_id == 2).unwrap();
    (stage.players[i].x, stage.players[i].y) = (31.0, 20.0);
    {
        let f = stage.players[i].fighter.as_mut().unwrap();
        (f.max_hp, f.hp) = (1_000_000, 1_000_000);
    }
    for _ in 0..5 {
        let (_, _, hits) = blow_on(&mut stage, zone, shared, 30.0, 20.0);
        let a: Vec<u32> = [101, 102].iter().map(|id| hits.iter().filter(|h| h.target == *id).map(|h| h.amount).sum()).collect();
        assert!(a[0] > 0 && a[0] == a[1], "one roll shared: {a:?}");
    }
}

#[test]
fn boss_zones_hit_two_and_a_half_times_their_dice_but_shares_stay_plain() {
    let mut stage = arena(11);
    stand(&mut stage, 30.0, 20.0);
    let map = stage.players[0].map;
    let walk = open(60, 40);
    // 4d1: always 4 before the multiplier.
    let hurt = Hurt { save: false, ..scripts::hurt(4, 1, DamageType::Force) };
    let beat = |hurt: Hurt, boost: f32| Beat {
        fight: 1,
        cast: 1,
        map,
        next: 0,
        until: 0,
        pulse: 0,
        zone: circle(30.0, 20.0, 3.0),
        zone_id: 0,
        safe: [(0.0, 0.0, 0.0); MAX_SAFE],
        n_safe: 0,
        hurt,
        level: 0,
        dc: 10,
        boost,
        evil: false,
        from: (30.0, 20.0),
        follow: 0,
        follow_mob: None,
        pull: 0.0,
        live: 0,
    };
    let taken = |stage: &mut HubInstance, b: Beat| -> u32 {
        let mut hits = Vec::new();
        strike(&b, &mut stage.players, &stage.lives, &walk, &mut stage.dice, 20.0, &mut hits);
        hits.iter().filter(|h| h.target == 101).map(|h| h.amount).sum()
    };
    assert_eq!(taken(&mut stage, beat(hurt, 1.0)), 10, "4 × 2.5");
    assert_eq!(taken(&mut stage, beat(hurt, ENRAGE_DAMAGE)), 15, "enraged: × 1.5 on top");
    // A shared blow (stack up on purpose) keeps its plain roll.
    assert_eq!(taken(&mut stage, beat(Hurt { split: 4, ..hurt }, 1.0)), 4);
    // The event strikes and the plain blow (`land`) are untouched.
    let (_, _, hits) = blow_on(&mut stage, circle(30.0, 20.0, 3.0), hurt, 30.0, 20.0);
    assert_eq!(hits.iter().filter(|h| h.target == 101).map(|h| h.amount).sum::<u32>(), 4);
}
