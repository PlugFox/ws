//! Wave game master: buys every wave with the points the map's difficulty
//! grants and releases it around the players.
//!
//! Each wave the game master receives a budget
//! `budget × growth^wave × (1 + party_scale × (players − 1))` plus what it
//! saved from earlier waves (capped). It spends it on packs:
//!
//! * a boss first, when one is unlocked, affordable and none of its kind is
//!   alive;
//! * then weighted picks among unlocked kinds it can afford; a pick buys a
//!   pack sized by the kind's archetype (swarms come by the dozen, tanks
//!   alone), trimmed to what is left;
//! * each pack lands in a formation: scattered (every member on its own,
//!   random player and direction), a loose cluster from one direction, or
//!   (swarm-sized packs) an uneven surround of one player.
//!
//! A wave is not dumped in one tick: its packs are released evenly over
//! `spawn_window_secs`. Where a pack lands is the horde's job
//! ([`super::Horde`]); members it cannot place (alive cap, no free ground)
//! are refunded into the bank.

use super::catalog::MonsterCatalog;
use crate::world::combat::Archetype;
use rand::Rng;
use std::collections::VecDeque;

/// Wave settings of a mission (its map's difficulty).
#[derive(Clone, Debug, PartialEq)]
pub struct WaveRules {
    /// First wave after this many seconds.
    pub first_wave_secs: f32,
    pub wave_period_secs: f32,
    /// A wave's packs are released evenly over this window.
    pub spawn_window_secs: f32,
    /// Points for the first wave with one player.
    pub budget: f32,
    /// Budget multiplier per wave.
    pub growth: f32,
    /// Extra budget per additional player.
    pub party_scale: f32,
    /// Unspent points carried over, at most this many waves' budgets.
    pub bank_waves: f32,
    /// Chance that a pack arrives as one loose cluster.
    pub cluster_chance: f32,
    /// Chance that a swarm-sized pack surrounds a player (checked first).
    pub surround_chance: f32,
    /// Monsters land this many cells beyond a player's light radius
    /// (`[min, max)`), and never closer to anyone's light than `min`.
    pub spawn_margin: (f32, f32),
    /// Monster levels of the mission (`[min, max]`): a kind spawns at its RO
    /// level clamped into the band, plus one level per `level_every_secs`
    /// of the run (still clamped); bosses spawn at `max + 5`.
    pub levels: (u32, u32),
    pub level_every_secs: f32,
    /// Weight factor of elite kinds in the picks (mission modifiers ask
    /// for more mini-bosses; 1 = the catalogue's weights).
    pub elite_weight: f32,
}

impl WaveRules {
    /// Normal difficulty.
    pub const NORMAL: WaveRules = WaveRules {
        first_wave_secs: 2.0,
        wave_period_secs: 20.0,
        spawn_window_secs: 6.0,
        budget: 12.0,
        growth: 1.12,
        party_scale: 0.6,
        bank_waves: 1.0,
        cluster_chance: 0.35,
        surround_chance: 0.15,
        spawn_margin: (4.0, 12.0),
        levels: (1, 20),
        level_every_secs: 90.0,
        elite_weight: 1.0,
    };
}

impl WaveRules {
    /// Level a monster of RO level `level` spawns at `elapsed` seconds into
    /// the run: clamped into the band, plus one per `level_every_secs`
    /// (still clamped); bosses at the top of the band + 5. The one formula
    /// the horde spawns with and the mission brief shows.
    pub fn level_at(&self, level: u32, boss: bool, elapsed: f32) -> u32 {
        let (lo, hi) = (self.levels.0.max(1), self.levels.1.max(self.levels.0.max(1)));
        if boss {
            return (hi + 5).min(u8::MAX as u32);
        }
        let grow = if self.level_every_secs > 0.0 { (elapsed.max(0.0) / self.level_every_secs) as u32 } else { 0 };
        (level.clamp(lo, hi) + grow).min(hi)
    }
}

impl Default for WaveRules {
    fn default() -> Self {
        Self::NORMAL
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Formation {
    /// Every member on its own spot off some player's screen.
    Scatter,
    /// A loose group arriving from one direction.
    Cluster,
    /// One player approached from all sides at uneven angles and distances.
    Surround,
}

/// A bought pack waiting for (or due for) release.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pack {
    pub kind: u8,
    pub count: u16,
    pub formation: Formation,
    /// Seconds since the stage started.
    pub release_at: f32,
}

pub struct Director {
    rules: WaveRules,
    elapsed: f32,
    next_wave: f32,
    wave: u32,
    bank: f32,
    pending: VecDeque<Pack>,
    picks: Vec<(u8, u32)>,
}

impl Director {
    pub fn new(rules: WaveRules) -> Self {
        Self {
            next_wave: rules.first_wave_secs,
            rules,
            elapsed: 0.0,
            wave: 0,
            bank: 0.0,
            pending: VecDeque::with_capacity(256),
            picks: Vec::with_capacity(16),
        }
    }

    pub fn rules(&self) -> &WaveRules {
        &self.rules
    }

    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// Test hook: jumps the run clock.
    #[cfg(test)]
    pub fn set_elapsed(&mut self, secs: f32) {
        self.elapsed = secs;
    }

    /// Waves bought so far.
    pub fn waves(&self) -> u32 {
        self.wave
    }

    /// Unspent points.
    pub fn bank(&self) -> f32 {
        self.bank
    }

    fn wave_budget(&self, wave: u32, players: usize) -> f32 {
        self.rules.budget
            * self.rules.growth.powi(wave as i32)
            * (1.0 + self.rules.party_scale * players.saturating_sub(1) as f32)
    }

    /// Advances by `dt` seconds; buys a wave when one is due and appends the
    /// packs due for release to `out`. `boss_alive(kind)` tells whether a
    /// boss of that kind is on the field.
    pub fn tick(
        &mut self,
        dt: f32,
        players: usize,
        catalog: &MonsterCatalog,
        rng: &mut impl Rng,
        boss_alive: impl Fn(u8) -> bool,
        out: &mut Vec<Pack>,
    ) {
        if players == 0 {
            return;
        }
        self.elapsed += dt;
        if self.elapsed >= self.next_wave {
            self.buy_wave(players, catalog, rng, &boss_alive);
            self.next_wave += self.rules.wave_period_secs;
        }
        while self.pending.front().is_some_and(|p| p.release_at <= self.elapsed) {
            out.push(self.pending.pop_front().unwrap());
        }
    }

    fn buy_wave(&mut self, players: usize, catalog: &MonsterCatalog, rng: &mut impl Rng, boss_alive: &impl Fn(u8) -> bool) {
        let budget = self.wave_budget(self.wave, players);
        self.wave += 1;
        let mut points = self.bank + budget;
        let start = self.elapsed;
        let first = self.pending.len();

        // Bosses first: one of each kind at a time.
        for (kind, def) in catalog.defs().iter().enumerate() {
            let kind = kind as u8;
            if def.boss && self.elapsed >= def.unlock_secs && def.spawn_cost as f32 <= points && !boss_alive(kind) {
                points -= def.spawn_cost as f32;
                self.pending.push_back(Pack { kind, count: 1, formation: Formation::Scatter, release_at: start });
            }
        }
        loop {
            self.picks.clear();
            let mut total = 0u32;
            for (kind, def) in catalog.defs().iter().enumerate() {
                // Plants are planted once at creation, never bought.
                if !def.boss && !def.is_plant() && self.elapsed >= def.unlock_secs && def.spawn_cost as f32 <= points {
                    total += if def.archetype == Archetype::Elite {
                        (def.weight as f32 * self.rules.elite_weight).round().max(1.0) as u32
                    } else {
                        def.weight
                    };
                    self.picks.push((kind as u8, total));
                }
            }
            if total == 0 {
                break;
            }
            let roll = rng.gen_range(0..total);
            let kind = self.picks.iter().find(|(_, acc)| roll < *acc).unwrap().0;
            let def = catalog.get(kind);
            let affordable = (points / def.spawn_cost as f32) as u16;
            let count = rng.gen_range(def.pack.0..=def.pack.1).min(affordable).max(1);
            points -= (count as u32 * def.spawn_cost) as f32;
            let formation = if def.pack.1 >= 6 && count >= 6 && rng.gen_bool(self.rules.surround_chance as f64) {
                Formation::Surround
            } else if count > 1 && rng.gen_bool(self.rules.cluster_chance as f64) {
                Formation::Cluster
            } else {
                Formation::Scatter
            };
            self.pending.push_back(Pack { kind, count, formation, release_at: start });
        }
        self.bank = points.min(budget * self.rules.bank_waves);

        // Spread this wave's packs over the window (bosses lead).
        let n = self.pending.len() - first;
        for (i, p) in self.pending.iter_mut().skip(first).enumerate() {
            p.release_at = start + self.rules.spawn_window_secs * i as f32 / n.max(1) as f32;
        }
    }

    /// Returns the points of `count` members of `kind` that could not be
    /// placed.
    pub fn refund(&mut self, kind: u8, count: u16, catalog: &MonsterCatalog) {
        self.bank += (count as u32 * catalog.get(kind).spawn_cost) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::super::catalog::test_def;
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn catalog() -> MonsterCatalog {
        let mut swarm = test_def("a_swarm", 2.0, 1, 0.0, false);
        swarm.pack = (6, 12);
        let mut tank = test_def("b_tank", 2.0, 4, 60.0, false);
        tank.pack = (1, 2);
        MonsterCatalog::new(vec![swarm, tank, test_def("c_boss", 4.0, 50, 120.0, true)]).unwrap()
    }

    /// Runs `secs` and returns every released pack.
    fn run(secs: f32, players: usize, rules: WaveRules) -> (Vec<Pack>, Director) {
        let c = catalog();
        let mut d = Director::new(rules);
        let mut rng = StdRng::seed_from_u64(1);
        let mut out = Vec::new();
        let dt = 1.0 / 20.0;
        for _ in 0..(secs / dt).round() as usize {
            d.tick(dt, players, &c, &mut rng, |_| false, &mut out);
        }
        (out, d)
    }

    fn cost(packs: &[Pack]) -> u32 {
        let c = catalog();
        packs.iter().map(|p| p.count as u32 * c.get(p.kind).spawn_cost).sum()
    }

    #[test]
    fn waves_arrive_on_schedule_spread_over_the_window() {
        let rules = WaveRules { budget: 40.0, ..WaveRules::NORMAL };
        let (early, _) = run(rules.first_wave_secs - 0.1, 1, rules.clone());
        assert!(early.is_empty(), "nothing before the first wave");
        let (first, d) = run(rules.first_wave_secs + rules.spawn_window_secs, 1, rules.clone());
        assert_eq!(d.waves(), 1);
        assert!(first.len() >= 2, "a wave is several packs");
        let span = first.last().unwrap().release_at - first[0].release_at;
        assert!(span > rules.spawn_window_secs * 0.5, "released over the window, not at once ({span} s)");
        assert!(first.iter().all(|p| p.kind == 0), "only unlocked kinds");
        assert!(first.iter().all(|p| (6..=12).contains(&p.count) || p.count < 6), "swarm pack sizes");
        let spent = cost(&first) as f32;
        assert!(spent <= rules.budget + 0.01 && spent >= rules.budget - 1.0, "spent {spent} of {}", rules.budget);
    }

    #[test]
    fn budget_grows_per_wave_and_with_the_party() {
        let rules = WaveRules::NORMAL;
        let span = rules.first_wave_secs + rules.wave_period_secs * 9.0 + rules.spawn_window_secs;
        let (solo, d) = run(span, 1, rules.clone());
        assert_eq!(d.waves(), 10);
        let expected: f32 = (0..10).map(|w| rules.budget * rules.growth.powi(w)).sum();
        let got = cost(&solo) as f32 + d.bank();
        assert!((got - expected).abs() < 1.0, "spent+bank {got}, budget {expected}");
        let (party, _) = run(span, 4, rules.clone());
        assert!(cost(&party) as f32 > 2.5 * cost(&solo) as f32, "4 players: ×(1 + 3×0.6)");
        assert!(solo.iter().chain(&party).any(|p| p.kind == 1), "tank unlocks at 60 s");
    }

    #[test]
    fn boss_is_bought_when_unlocked_affordable_and_absent() {
        let mut rules = WaveRules::NORMAL;
        rules.budget = 60.0;
        rules.growth = 1.0;
        let c = catalog();
        let mut d = Director::new(rules);
        let mut rng = StdRng::seed_from_u64(3);
        let mut out = Vec::new();
        let mut alive = false;
        let mut bosses = 0;
        for _ in 0..20 * 240 {
            out.clear();
            d.tick(0.05, 1, &c, &mut rng, |_| alive, &mut out);
            for p in &out {
                if p.kind == 2 {
                    assert!(d.elapsed() >= 120.0, "boss before its unlock");
                    bosses += 1;
                    alive = true;
                }
            }
        }
        assert_eq!(bosses, 1, "never a second one while the first is alive");
    }

    #[test]
    fn formations_vary_and_refunds_bank() {
        let rules = WaveRules { budget: 200.0, ..WaveRules::NORMAL };
        let (packs, mut d) = run(rules.first_wave_secs + rules.spawn_window_secs, 1, rules);
        for f in [Formation::Scatter, Formation::Cluster, Formation::Surround] {
            assert!(packs.iter().any(|p| p.formation == f), "{f:?} never chosen");
        }
        let before = d.bank();
        d.refund(0, 5, &catalog());
        assert_eq!(d.bank(), before + 5.0);
    }
}
