//! Snapshot traffic of the monster section on the real stage map.
//!
//! `cargo run --release --example snapshot_bench [monsters...]`
//!
//! For each crowd size and scenario (the player runs a loop so the crowd
//! keeps turning, or stands in a settled crowd), compares per frame and
//! player:
//!
//! * `full`: the previous format, 8 bytes per visible monster every tick;
//! * `delta`: dead-reckoning deltas without a budget;
//! * `budget`: the same with the default byte budget.
//!
//! and how far the client's dead-reckoned monsters are from the truth.

use game_server::world::horde::catalog::MonsterCatalog;
use game_server::world::horde::view::{predict, MonsterDelta, MonsterMirror, MonsterView, ViewConfig};
use game_server::world::horde::{Horde, HordeConfig, PlayerView};
use game_server::world::hub::HubWorld;
use game_server::world::wire::Reader;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

const HZ: u32 = 20;
const WARMUP: u32 = 20 * 10;
const MEASURE: u32 = 20 * 30;

struct Run {
    name: &'static str,
    view: MonsterView,
    mirror: MonsterMirror,
    budget: usize,
    bytes: Vec<usize>,
    deferred: u64,
    err_near: Vec<f32>,
    err_edge: Vec<f32>,
    missing: u64,
    in_reach: u64,
    nanos: u128,
}

fn pct(v: &mut [f32], p: f64) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v[((v.len() - 1) as f64 * p) as usize]
}

fn main() {
    let sizes: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let sizes = if sizes.is_empty() { vec![500, 1500] } else { sizes };
    let dist = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    let world = Arc::new(HubWorld::load(&dist, &["prt_fild08".into()], "prt_fild08").expect("stage map"));
    let catalog = Arc::new(MonsterCatalog::load(&dist).expect("monsters.json"));
    let walk = &world.map(world.entry()).walk;
    let (sx, sy) = world.map(world.entry()).spawn;

    for &n in &sizes {
        for moving in [true, false] {
            let config = HordeConfig {
                director_enabled: false,
                alive_cap: n.max(1),
                pool: (n + 64).max(4096),
                seed: Some(3),
                view: ViewConfig { budget: usize::MAX, ..ViewConfig::default() },
                ..HordeConfig::default()
            };
            let light = config.light_radius;
            let reach = light + config.aoi_margin;
            let mut horde = Horde::new(Arc::clone(&world), Arc::clone(&catalog), config);
            let mut rng = StdRng::seed_from_u64(9);
            let kinds: Vec<u8> = ["poring", "fabre", "lunatic"].iter().filter_map(|k| catalog.kind_of(k)).collect();
            let mut placed = 0;
            while placed < n {
                let a = rng.gen_range(0.0..std::f32::consts::TAU);
                let r = rng.gen_range(3.0..light * 1.3);
                let (x, y) = (sx + a.cos() * r, sy + a.sin() * r);
                if walk.is_walkable(x, y) && horde.spawn(kinds[placed % kinds.len()], x, y).is_some() {
                    placed += 1;
                }
            }
            let budgets: Vec<(&'static str, usize)> = vec![("delta", usize::MAX), ("1100", 1100), ("1600", 1600), ("2400", 2400)];
            let mut runs: Vec<Run> = budgets
                .into_iter()
                .map(|(name, budget)| Run {
                    name,
                    view: MonsterView::new(horde.capacity()),
                    mirror: MonsterMirror::default(),
                    budget,
                    bytes: Vec::new(),
                    deferred: 0,
                    err_near: Vec::new(),
                    err_edge: Vec::new(),
                    missing: 0,
                    in_reach: 0,
                    nanos: 0,
                })
                .collect();
            let mut full_bytes = Vec::new();
            let mut buf = bytes::BytesMut::new();
            let mut player = (sx, sy);
            for t in 0..WARMUP + MEASURE {
                if moving {
                    // Run a 12-cell loop at walking speed (6 cells/s).
                    let a = t as f32 * 6.0 / HZ as f32 / 12.0;
                    let next = (sx + a.cos() * 12.0 - 12.0, sy + a.sin() * 12.0);
                    if walk.is_walkable(next.0, next.1) {
                        player = next;
                    }
                }
                let p = PlayerView { x: player.0, y: player.1, light, id: 0 };
                horde.tick_views(t, HZ, &[p]);
                let visible = horde
                    .monsters()
                    .filter(|m| ((m.3 - p.x).powi(2) + (m.4 - p.y).powi(2)).sqrt() <= reach)
                    .count();
                for run in runs.iter_mut() {
                    horde.set_view_budget(run.budget);
                    buf.clear();
                    let started = Instant::now();
                    let stats = horde.write_delta(&mut run.view, &p, t, &mut buf);
                    let spent = started.elapsed().as_nanos();
                    let delta = MonsterDelta::decode(&mut Reader::new(&buf)).expect("decodes");
                    run.mirror.apply(t, &delta).expect("applies");
                    if t < WARMUP {
                        continue;
                    }
                    run.nanos += spent;
                    run.bytes.push(stats.bytes);
                    run.deferred += stats.deferred as u64;
                    for (slot, _, _, x, y) in horde.monsters() {
                        let d = ((x - p.x).powi(2) + (y - p.y).powi(2)).sqrt();
                        if d > reach {
                            continue;
                        }
                        match run.mirror.monsters.get(&slot) {
                            Some(m) => {
                                run.in_reach += 1;
                                let (px, py) = predict(&m.rec, t as f64);
                                let e = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
                                if d < light * 0.5 { run.err_near.push(e) } else { run.err_edge.push(e) }
                            }
                            None => {
                                run.in_reach += 1;
                                run.missing += 1;
                            }
                        }
                    }
                }
                if t >= WARMUP {
                    full_bytes.push(2 + 8 * visible);
                }
            }

            let scenario = if moving { "running loop" } else { "standing" };
            let mean = |v: &[usize]| v.iter().sum::<usize>() as f64 / v.len() as f64;
            let p = |v: &[usize], q: f64| {
                let mut s = v.to_vec();
                s.sort_unstable();
                s[((s.len() - 1) as f64 * q) as usize]
            };
            println!(
                "\n{n} monsters, {scenario}: {:.0} in reach on average",
                runs[0].in_reach as f64 / MEASURE as f64
            );
            println!(
                "  {:<7} {:>8} {:>6} {:>6} {:>8}   {:>9} {:>9}  {:>8}  {:>7}",
                "", "B/frame", "p99", "max", "KB/s", "near p99", "edge p99", "missing", "µs"
            );
            println!(
                "  {:<7} {:>8.0} {:>6} {:>6} {:>8.1}",
                "full",
                mean(&full_bytes),
                p(&full_bytes, 0.99),
                full_bytes.iter().max().unwrap(),
                mean(&full_bytes) * HZ as f64 / 1024.0
            );
            for run in runs.iter_mut() {
                println!(
                    "  {:<7} {:>8.0} {:>6} {:>6} {:>8.1}   {:>9.3} {:>9.3}  {:>7.2}%  {:>7.1}{}",
                    run.name,
                    mean(&run.bytes),
                    p(&run.bytes, 0.99),
                    run.bytes.iter().max().unwrap(),
                    mean(&run.bytes) * HZ as f64 / 1024.0,
                    pct(&mut run.err_near, 0.99),
                    pct(&mut run.err_edge, 0.99),
                    100.0 * run.missing as f64 / run.in_reach.max(1) as f64,
                    run.nanos as f64 / MEASURE as f64 / 1000.0,
                    if run.deferred > 0 { format!("  ({} deferred/frame)", run.deferred / MEASURE as u64) } else { String::new() },
                );
            }
        }
    }
}
