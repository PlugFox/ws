//! Horde tick cost on the real stage map (needs the private asset pack).
//!
//! `cargo run --release --example horde_bench [dist dir]`
//!
//! Four players walk circles around the map centre (so the flow field is
//! rebuilt continuously); monsters are placed by hand, either spread over
//! the whole map (mostly far tier) or packed around the party (worst case:
//! everything full-rate and in contact).

use game_server::world::horde::catalog::MonsterCatalog;
use game_server::world::horde::{Horde, HordeConfig};
use game_server::world::hub::HubWorld;
use rand::{Rng, SeedableRng};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let dist = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist"));
    let world = Arc::new(HubWorld::load(&dist, &["prt_fild08".into()], "prt_fild08").expect("stage map"));
    let catalog = Arc::new(MonsterCatalog::load(&dist).expect("monsters.json"));
    let regular: Vec<u8> = (0..catalog.defs().len() as u8).filter(|&k| !catalog.get(k).boss).collect();
    let map = world.map(world.entry());
    let (cx, cy) = map.spawn;

    println!("{:<8} {:>6} {:>10} {:>10} {:>10} {:>8} {:>7}", "layout", "mobs", "mean µs", "p99 µs", "max µs", "full", "builds");
    for layout in ["spread", "clumped"] {
        for n in [500usize, 1500, 4000] {
            let cfg = HordeConfig { pool: 4096, director_enabled: false, seed: Some(1), recycle_after_secs: 1e6, ..HordeConfig::default() };
            let mut horde = Horde::new(Arc::clone(&world), Arc::clone(&catalog), cfg);
            let mut rng = rand::rngs::StdRng::seed_from_u64(9);
            let mut placed = 0;
            while placed < n {
                let (x, y) = if layout == "spread" {
                    (rng.gen_range(0.0..map.walk.width() as f32), rng.gen_range(0.0..map.walk.height() as f32))
                } else {
                    (cx + rng.gen_range(-12.0..12.0), cy + rng.gen_range(-12.0..12.0))
                };
                if map.is_reachable_cell(x as u32, y as u32) && horde.spawn(regular[placed % regular.len()], x, y).is_some() {
                    placed += 1;
                }
            }
            let mut samples = Vec::new();
            let mut full = 0;
            for t in 0..800u32 {
                let a = t as f32 * 0.01;
                let players: Vec<(f32, f32)> = (0..4)
                    .map(|p| {
                        let ang = a + p as f32 * std::f32::consts::FRAC_PI_2;
                        let (x, y) = (cx + 6.0 * ang.cos(), cy + 6.0 * ang.sin());
                        if map.walk.is_walkable(x, y) { (x, y) } else { (cx, cy) }
                    })
                    .collect();
                let start = Instant::now();
                horde.tick(t, 20, &players);
                let el = start.elapsed().as_secs_f64() * 1e6;
                if t >= 100 {
                    samples.push(el);
                    full = horde.stats().full;
                }
            }
            samples.sort_by(f64::total_cmp);
            let mean = samples.iter().sum::<f64>() / samples.len() as f64;
            let p99 = samples[samples.len() * 99 / 100];
            let max = *samples.last().unwrap();
            println!(
                "{layout:<8} {n:>6} {mean:>10.1} {p99:>10.1} {max:>10.1} {full:>8} {:>7}",
                horde.stats().field_builds
            );
        }
    }
}
