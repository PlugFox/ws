//! Balance aid: what the wave game master buys on the real stage map.
//!
//! `cargo run --release --example wave_timeline [minutes]`
//!
//! Simulates a solo player and a party of four standing at the spawn with
//! the default (Normal) wave rules and prints each wave's purchases. Nothing
//! dies yet (Phase 6), so the alive count only grows up to the cap.

use game_server::world::horde::catalog::MonsterCatalog;
use game_server::world::horde::{Horde, HordeConfig};
use game_server::world::hub::HubWorld;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    let minutes: u32 = std::env::args().nth(1).and_then(|m| m.parse().ok()).unwrap_or(7);
    let dist = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
    let world = Arc::new(HubWorld::load(&dist, &["prt_fild08".into()], "prt_fild08").expect("stage map"));
    let catalog = Arc::new(MonsterCatalog::load(&dist).expect("monsters.json"));
    let (sx, sy) = world.map(world.entry()).spawn;
    for party in [1usize, 4] {
        let config = HordeConfig { seed: Some(2), ..HordeConfig::default() };
        let mut horde = Horde::new(Arc::clone(&world), Arc::clone(&catalog), config);
        let players: Vec<(f32, f32)> = (0..party).map(|i| (sx + i as f32, sy)).collect();
        let mut seen = HashSet::new();
        let mut waves: BTreeMap<u32, BTreeMap<&str, u32>> = BTreeMap::new();
        for t in 0..20 * 60 * minutes {
            horde.tick(t, 20, &players);
            let wave = horde.waves();
            for (slot, generation, kind, _, _) in horde.monsters() {
                // Recycled monsters come back as new (slot, generation) pairs
                // but were not bought; standing players recycle nobody.
                if seen.insert((slot, generation)) {
                    *waves.entry(wave).or_default().entry(catalog.get(kind).id.as_str()).or_default() += 1;
                }
            }
        }
        println!("party of {party}: {} alive after {minutes} min, {} recycled", horde.len(), horde.stats().recycled);
        for (wave, bought) in waves {
            println!("  wave {wave:>2} (t≈{:>3} s): {bought:?}", 2 + (wave.saturating_sub(1)) * 20);
        }
    }
}
