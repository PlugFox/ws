use super::catalog::test_def;
use super::*;
use crate::world::hub::test_support::map;
use crate::world::map::{CameraConstraint, LocationKind};

/// Open `w×h` field with a border wall and an optional vertical wall at
/// `x = wall` spanning all rows but the top 3 (a detour).
fn world(w: usize, h: usize, wall: Option<usize>) -> Arc<HubWorld> {
    let rows: Vec<String> = (0..h)
        .map(|r| {
            (0..w)
                .map(|c| {
                    let border = r == 0 || c == 0 || r == h - 1 || c == w - 1;
                    // Row 0 is the north edge; the gap is at the top (north).
                    let inner_wall = wall == Some(c) && r > 3;
                    if border || inner_wall { '#' } else { '.' }
                })
                .collect()
        })
        .collect();
    let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
    let field = map("field", LocationKind::Field, camera, &refs, vec![]);
    Arc::new(HubWorld::new(vec![field], "field").unwrap())
}

fn catalog() -> Arc<MonsterCatalog> {
    Arc::new(
        MonsterCatalog::new(vec![test_def("blob", 3.0, 1, 0.0, false), test_def("boss", 3.0, 50, 0.0, true)])
            .unwrap(),
    )
}

fn config(light: f32) -> HordeConfig {
    HordeConfig { light_radius: light, director_enabled: false, seed: Some(7), ..HordeConfig::default() }
}

#[test]
fn double_box_filter_matches_the_circle() {
    let mut rng = StdRng::seed_from_u64(3);
    for _ in 0..100_000 {
        let (dx, dy, r) = (rng.gen_range(-20.0..20.0f32), rng.gen_range(-20.0..20.0f32), rng.gen_range(0.1..15.0f32));
        assert_eq!(in_radius(dx, dy, r), dx * dx + dy * dy <= r * r, "{dx},{dy} r={r}");
    }
}

#[test]
fn monster_walks_around_a_wall_to_the_player() {
    let w = world(40, 30, Some(20));
    let mut h = Horde::new(Arc::clone(&w), catalog(), config(30.0));
    let player = (30.5, 5.5);
    let slot = h.spawn(0, 8.5, 5.5).unwrap();
    let walk = &w.map(0).walk;
    let mut reached = None;
    for t in 0..2000 {
        h.tick(t, 20, &[player]);
        let (_, _, _, x, y) = h.monsters().find(|m| m.0 == slot).unwrap();
        assert!(walk.is_walkable(x, y), "monster inside a wall at {x},{y}");
        let (dx, dy) = (player.0 - x, player.1 - y);
        if dx * dx + dy * dy <= 1.0 {
            reached = Some(t);
            break;
        }
    }
    // Straight line is 22 cells; the detour over the top is ~ 22 + 2×20.
    let t = reached.expect("monster reached the player");
    assert!(t > 22 * 20 / 3, "it cannot have gone through the wall (t={t})");
}

#[test]
fn far_monsters_step_at_a_reduced_staggered_rate() {
    let w = world(200, 20, None);
    let mut cfg = config(10.0);
    cfg.recycle_factor = 100.0; // keep it
    let mut h = Horde::new(w, catalog(), cfg);
    let player = (5.5, 10.5);
    let near = h.spawn(0, 12.5, 10.5).unwrap();
    let far = h.spawn(0, 150.5, 10.5).unwrap();
    let pos = |h: &Horde, s: u16| h.monsters().find(|m| m.0 == s).map(|m| (m.3, m.4)).unwrap();
    let (mut near_moves, mut far_moves) = (0, 0);
    for t in 0..40 {
        let (n0, f0) = (pos(&h, near), pos(&h, far));
        h.tick(t, 20, &[player]);
        near_moves += usize::from(pos(&h, near) != n0);
        far_moves += usize::from(pos(&h, far) != f0);
    }
    assert!(near_moves >= 30, "near monster moves every tick until contact ({near_moves})");
    assert_eq!(far_moves, 10, "far monster moves every 4th tick");
    // Same distance per second either way.
    let (fx, _) = pos(&h, far);
    assert!((150.5 - fx - 3.0 * 2.0).abs() < 0.05, "far monster covered 2 s × 3 cells/s, at {fx}");
}

#[test]
fn far_monsters_are_recycled_into_the_spawn_ring() {
    let w = world(220, 40, None);
    let mut h = Horde::new(w, catalog(), config(10.0));
    let player = (10.5, 20.5);
    let slot = h.spawn(0, 200.5, 20.5).unwrap();
    let generation0 = h.monsters().next().unwrap().1;
    for t in 0..(4 * 20 + 5) {
        h.tick(t, 20, &[player]);
    }
    assert_eq!(h.len(), 1);
    let (s, generation, _, x, y) = h.monsters().next().unwrap();
    assert!(s != slot || generation != generation0, "a recycled monster is a new entity to clients");
    let d = ((x - player.0).powi(2) + (y - player.1).powi(2)).sqrt();
    let (lo, hi) = WaveRules::NORMAL.spawn_margin;
    assert!((10.0 + lo..=10.0 + hi).contains(&d), "respawned just outside the light, at distance {d}");
    assert_eq!(h.stats().recycled, 1);
}

#[test]
fn bosses_are_never_recycled_or_demoted() {
    let w = world(220, 40, None);
    let mut h = Horde::new(w, catalog(), config(10.0));
    let boss = h.spawn(1, 200.5, 20.5).unwrap();
    for t in 0..200 {
        h.tick(t, 20, &[(10.5, 20.5)]);
        assert_eq!(h.stats().full, 1, "boss runs at full rate every tick");
    }
    assert_eq!(h.monsters().next().unwrap().0, boss);
    assert_eq!(h.stats().recycled, 0);
}

#[test]
fn a_stacked_crowd_spreads_out_and_stays_walkable() {
    let w = world(40, 40, None);
    let mut h = Horde::new(Arc::clone(&w), catalog(), config(30.0));
    for _ in 0..40 {
        h.spawn(0, 20.5, 25.5).unwrap();
    }
    let player = (20.5, 10.5);
    for t in 0..300 {
        h.tick(t, 20, &[player]);
    }
    let before: Vec<_> = h.monsters().collect();
    h.tick(300, 20, &[player]);
    let m: Vec<_> = h.monsters().collect();
    // Settled: the pile around a standing player does not wobble.
    let wobble = m.iter().zip(&before).map(|(a, b)| (a.3 - b.3).hypot(a.4 - b.4)).sum::<f32>() / m.len() as f32;
    assert!(wobble < 0.005, "settled crowd moves {wobble} cells/tick on average");
    let nearest: Vec<f32> = m
        .iter()
        .map(|a| {
            assert!(w.map(0).walk.is_walkable(a.3, a.4));
            m.iter().filter(|b| b.0 != a.0).map(|b| (a.3 - b.3).hypot(a.4 - b.4)).fold(f32::INFINITY, f32::min)
        })
        .collect();
    let worst = nearest.iter().copied().fold(f32::INFINITY, f32::min);
    let mean = nearest.iter().sum::<f32>() / nearest.len() as f32;
    // Radii are 0.35: touching is 0.7 apart. A pile pressed against a
    // player keeps a soft overlap (measured: mean ≈ 0.48, worst ≈ 0.33).
    assert!(mean > 0.42, "mean nearest-neighbour distance {mean}");
    assert!(worst > 0.28, "closest pair {worst} apart");
    // And they surround the player instead of piling up behind each other.
    let near = m.iter().filter(|a| ((a.3 - player.0).powi(2) + (a.4 - player.1).powi(2)).sqrt() < 3.0).count();
    assert!(near >= 10, "{near} monsters in contact range");
}

#[test]
fn game_master_waves_land_just_outside_the_light_in_random_directions() {
    // A wall splits the field; the east half is reachable only around it.
    let w = world(160, 160, Some(80));
    let light = 30.0;
    let mut cfg = config(light);
    cfg.director_enabled = true;
    cfg.waves.budget = 55.0; // the test boss costs 50
    let (lo, hi) = cfg.waves.spawn_margin;
    let mut h = Horde::new(Arc::clone(&w), catalog(), cfg);
    let players = [(40.5, 60.5), (50.5, 70.5)];
    let mut seen = std::collections::HashSet::new();
    let mut fresh = 0;
    let mut octants = [0u32; 8];
    for t in 0..20 * 65 {
        h.tick(t, 20, &players);
        for (slot, generation, _, x, y) in h.monsters() {
            if seen.insert((slot, generation)) {
                fresh += 1;
                let d: Vec<f32> = players.iter().map(|&(px, py)| (x - px).hypot(y - py)).collect();
                // Just outside somebody's light (one step of slack), and in
                // nobody's light.
                // A cluster's members spread up to √12 + 1 around its centre.
                assert!(d.iter().any(|&d| d <= light + hi + 5.0), "spawned too far: {d:?}");
                assert!(d.iter().all(|&d| d > light + lo - 0.5), "spawned in the light: {d:?}");
                let (dx, dy) = (x - players[0].0, y - players[0].1);
                octants[((dy.atan2(dx) + std::f32::consts::PI) / std::f32::consts::FRAC_PI_4) as usize % 8] += 1;
                assert!(h.field().dist(x as i32, y as i32) != field::UNREACHABLE);
            }
        }
    }
    assert!(h.waves() >= 3, "waves every 20 s");
    assert!(octants.iter().all(|&n| n > 0), "spawns come from every direction: {octants:?}");
    assert!(fresh > 40, "{fresh} monsters spawned");
    assert!(h.monsters().any(|m| m.2 == 1), "the boss is bought once affordable");
}

/// One player's view driven tick by tick, with the client mirror applying
/// every section.
struct Viewer {
    view: view::MonsterView,
    mirror: view::MonsterMirror,
    buf: bytes::BytesMut,
}

impl Viewer {
    fn new(h: &Horde) -> Self {
        Self { view: view::MonsterView::new(h.capacity()), mirror: Default::default(), buf: Default::default() }
    }

    fn frame(&mut self, h: &Horde, p: PlayerView, tick: u32) -> (view::DeltaStats, view::MonsterDelta) {
        self.buf.clear();
        let stats = h.write_delta(&mut self.view, &p, tick, &mut self.buf);
        assert_eq!(stats.bytes, self.buf.len());
        let mut r = crate::world::wire::Reader::new(&self.buf);
        let delta = view::MonsterDelta::decode(&mut r).expect("section decodes");
        assert!(r.is_empty());
        self.mirror.apply(tick, &delta).expect("delta applies");
        (stats, delta)
    }

    /// Worst distance between where the client draws a monster and where it is.
    fn worst_error(&self, h: &Horde, tick: u32) -> f32 {
        let mut worst = 0.0f32;
        for (slot, generation, _, x, y) in h.monsters() {
            if let Some(m) = self.mirror.monsters.get(&slot) {
                assert_eq!(m.generation, generation, "mirror holds a stale monster in slot {slot}");
                let (px, py) = view::predict(&m.rec, tick as f64);
                worst = worst.max(((px - x).powi(2) + (py - y).powi(2)).sqrt());
            }
        }
        worst
    }
}

#[test]
fn deltas_keep_the_client_within_the_error_bound() {
    let w = world(120, 120, None);
    let mut h = Horde::new(w, catalog(), config(20.0));
    let mut v = Viewer::new(&h);
    for i in 0..60 {
        let a = i as f32 * 0.37;
        h.spawn(0, 60.5 + a.cos() * (6.0 + i as f32 * 0.3), 60.5 + a.sin() * (6.0 + i as f32 * 0.3)).unwrap();
    }
    let edge = h.config.view.edge_error;
    for t in 0..400u32 {
        // The player circles, so the crowd keeps turning.
        let a = t as f32 * 0.02;
        let p = PlayerView { x: 60.5 + a.cos() * 15.0, y: 60.5 + a.sin() * 15.0, light: 20.0 };
        h.tick_views(t, 20, &[p]);
        let (stats, delta) = v.frame(&h, p, t);
        assert_eq!(delta.baseline, if t == 0 { 0 } else { 1 });
        assert!(stats.bytes <= h.config.view.budget, "{stats:?}");
        assert_eq!(stats.deferred, 0, "60 monsters fit the budget");
        // Everything within reach is known; nothing far beyond it is.
        let reach = p.light + h.config.aoi_margin;
        for (slot, _, _, x, y) in h.monsters() {
            let d = ((x - p.x).powi(2) + (y - p.y).powi(2)).sqrt();
            if d < reach - 0.5 {
                assert!(v.mirror.monsters.contains_key(&slot), "t={t}: slot {slot} at {d:.1} is missing");
            }
            if d > reach + 3.0 {
                assert!(!v.mirror.monsters.contains_key(&slot), "t={t}: slot {slot} at {d:.1} lingers");
            }
        }
        // The prediction never drifts past the (edge) threshold plus one
        // tick of quantisation.
        assert!(v.worst_error(&h, t) <= edge + 0.02, "t={t}: {}", v.worst_error(&h, t));
    }
}

#[test]
fn a_settled_crowd_costs_almost_nothing() {
    let w = world(60, 60, None);
    let mut h = Horde::new(w, catalog(), config(20.0));
    let mut v = Viewer::new(&h);
    for i in 0..40 {
        h.spawn(0, 20.5 + (i % 8) as f32, 20.5 + (i / 8) as f32).unwrap();
    }
    let p = PlayerView { x: 30.5, y: 30.5, light: 20.0 };
    let mut bytes = Vec::new();
    for t in 0..600u32 {
        h.tick_views(t, 20, &[p]);
        bytes.push(v.frame(&h, p, t).0.bytes);
    }
    let first: usize = bytes[..20].iter().sum();
    let last: usize = bytes[500..].iter().sum();
    // Two bytes per frame is the empty section (baseline + count).
    assert!(last <= 100 * 2 + 40, "settled: {last} bytes in 100 frames (first second: {first})");
}

#[test]
fn the_budget_spreads_a_burst_and_nearest_monsters_come_first() {
    let w = world(100, 100, None);
    let mut cfg = config(30.0);
    cfg.view.budget = 200;
    let mut h = Horde::new(w, catalog(), cfg);
    let mut v = Viewer::new(&h);
    for i in 0..300 {
        h.spawn(0, 25.5 + (i % 20) as f32 * 2.5, 25.5 + (i / 20) as f32 * 2.5).unwrap();
    }
    let p = PlayerView { x: 50.5, y: 50.5, light: 30.0 };
    h.tick_views(0, 20, &[p]);
    let (s0, d0) = v.frame(&h, p, 0);
    assert!(s0.bytes <= 200 && s0.deferred > 0, "{s0:?}");
    // The first frame holds the monsters nearest to the player.
    let dist = |slot: u16| {
        let m = h.monsters().find(|m| m.0 == slot).unwrap();
        ((m.3 - p.x).powi(2) + (m.4 - p.y).powi(2)).sqrt()
    };
    let sent: Vec<f32> = d0.entries.iter().map(|e| match e { view::DeltaEntry::Spawn { slot, .. } => dist(*slot), _ => unreachable!() }).collect();
    let nearest_left = h.monsters().filter(|m| !v.mirror.monsters.contains_key(&m.0)).map(|m| dist(m.0)).fold(f32::MAX, f32::min);
    assert!(sent.iter().all(|&d| d <= nearest_left + 0.01), "sent {sent:?}, nearest unsent {nearest_left}");
    let mut t = 1;
    while v.mirror.monsters.len() < 300 {
        h.tick_views(t, 20, &[p]);
        let (s, _) = v.frame(&h, p, t);
        assert!(s.bytes <= 200);
        t += 1;
        assert!(t < 200, "the view fills up: {} known", v.mirror.monsters.len());
    }
}

#[test]
fn a_lost_frame_resyncs_through_a_keyframe() {
    let w = world(60, 60, None);
    let mut h = Horde::new(w, catalog(), config(20.0));
    let mut v = Viewer::new(&h);
    for i in 0..10 {
        h.spawn(0, 22.5 + i as f32, 25.5).unwrap();
    }
    let p = PlayerView { x: 30.5, y: 40.5, light: 20.0 };
    for t in 0..5 {
        h.tick_views(t, 20, &[p]);
        v.frame(&h, p, t);
    }
    // Frame 5 is lost on the way: frame 6 no longer applies.
    h.tick_views(5, 20, &[p]);
    v.buf.clear();
    h.write_delta(&mut v.view, &p, 5, &mut v.buf);
    h.tick_views(6, 20, &[p]);
    v.buf.clear();
    h.write_delta(&mut v.view, &p, 6, &mut v.buf);
    let delta = view::MonsterDelta::decode(&mut crate::world::wire::Reader::new(&v.buf)).unwrap();
    assert!(v.mirror.apply(6, &delta).is_err(), "a gap is detected");
    // The instance resets the view; the next frame is a keyframe.
    v.view.reset();
    h.tick_views(7, 20, &[p]);
    let (_, d) = v.frame(&h, p, 7);
    assert_eq!(d.baseline, 0);
    assert_eq!(v.mirror.monsters.len(), 10);
}

#[test]
fn a_reused_slot_is_a_new_monster_to_the_client() {
    let w = world(60, 60, None);
    let mut cfg = config(20.0);
    cfg.pool = 2;
    let mut h = Horde::new(w, catalog(), cfg);
    let mut v = Viewer::new(&h);
    let p = PlayerView { x: 30.5, y: 30.5, light: 20.0 };
    let a = h.spawn(0, 25.5, 30.5).unwrap();
    h.tick_views(0, 20, &[p]);
    v.frame(&h, p, 0);
    // Free the slot and refill both (FIFO: the second spawn reuses `a`).
    h.despawn(a);
    let b = h.spawn(0, 35.5, 30.5).unwrap();
    let c = h.spawn(0, 30.5, 25.5).unwrap();
    assert_ne!(b, a);
    assert_eq!(c, a);
    h.tick_views(1, 20, &[p]);
    let (_, d) = v.frame(&h, p, 1);
    assert!(d.entries.iter().all(|e| !matches!(e, view::DeltaEntry::Remove { .. })), "SPAWN supersedes REMOVE: {d:?}");
    assert_eq!(v.mirror.monsters.len(), 2);
    assert!(v.worst_error(&h, 1) < 0.1, "the new monster is where it stands, not where the old one was");
}

#[test]
fn removed_and_dead_monsters_leave_the_view() {
    let w = world(100, 60, None);
    let mut h = Horde::new(w, catalog(), config(10.0));
    let mut v = Viewer::new(&h);
    let a = h.spawn(0, 15.5, 30.5).unwrap();
    h.spawn(0, 18.5, 30.5).unwrap();
    let mut p = PlayerView { x: 10.5, y: 30.5, light: 10.0 };
    h.tick_views(0, 20, &[p]);
    v.frame(&h, p, 0);
    assert_eq!(v.mirror.monsters.len(), 2);
    h.despawn(a);
    h.tick_views(1, 20, &[p]);
    let (s, _) = v.frame(&h, p, 1);
    assert_eq!((s.removes, v.mirror.monsters.len()), (1, 1));
    // The player teleports away: the rest drops out of the light.
    p.x = 80.5;
    h.tick_views(2, 20, &[p]);
    v.frame(&h, p, 2);
    assert!(v.mirror.monsters.is_empty());
}

#[test]
fn no_players_freezes_the_horde() {
    let w = world(40, 40, None);
    let mut cfg = config(10.0);
    cfg.director_enabled = true;
    let mut h = Horde::new(w, catalog(), cfg);
    h.spawn(0, 20.5, 20.5).unwrap();
    for t in 0..100 {
        h.tick(t, 20, &[]);
    }
    assert_eq!(h.len(), 1);
    assert_eq!(h.monsters().next().unwrap().3, 20.5);
    assert_eq!(h.director_elapsed(), 0.0);
}
