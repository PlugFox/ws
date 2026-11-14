//! Drop tables (COMBAT.md §2.6 Drops): what a kill leaves behind.
//!
//! Tables are baked from `assets/drops.toml` into each monster's record
//! (`monsters.json` `drops`) and resolved once, at catalogue load, into
//! [`Drop`] rows that point at the static item catalogue — rolling a table
//! on a kill allocates nothing. A chance is in hundredths of a percent, as
//! RO's `Rate` (10000 = always); every row is rolled on its own.

use super::dice::Rng;
use super::items::{self, Kind};

/// Pseudo item id of a zeny drop (`count` is the amount).
pub const ZENY: &str = "zeny";
/// A chance of 100% in [`Drop::rate`] units.
pub const ALWAYS: u32 = 10_000;
/// Drops at or under this chance (2%), and any equipment, are rare: the
/// client adds a toast to the floating name.
pub const RARE_RATE: u32 = 200;

/// One row of a drop table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drop {
    /// Catalogue id, or [`ZENY`].
    pub item: &'static str,
    /// Chance in hundredths of a percent (`1..=ALWAYS`).
    pub rate: u32,
    pub min: u32,
    pub max: u32,
    pub rare: bool,
}

impl Drop {
    /// A row from the baked table: `chance` in percent (0.01 .. 100),
    /// `count` defaults to one. Unknown items and empty chances are errors
    /// (the catalogue warns and skips them).
    pub fn new(item: &str, chance: f64, count: Option<[u32; 2]>) -> Result<Drop, String> {
        let (item, equipment) = if item == ZENY {
            (ZENY, false)
        } else {
            let e = items::find(item).ok_or_else(|| format!("unknown item {item}"))?;
            (e.id, !matches!(e.kind, Kind::Etc | Kind::Consumable(_)))
        };
        if !(chance.is_finite() && chance > 0.0) {
            return Err(format!("{item}: chance must be > 0"));
        }
        let rate = ((chance * 100.0).round() as u32).clamp(1, ALWAYS);
        let [min, max] = count.unwrap_or([1, 1]);
        if min == 0 || max < min || max > items::MAX_STACK {
            return Err(format!("{item}: count must be 1 <= min <= max"));
        }
        Ok(Drop { item, rate, min, max, rare: item != ZENY && (rate <= RARE_RATE || equipment) })
    }

    pub fn is_zeny(&self) -> bool {
        self.item == ZENY
    }
}

/// Rolls every row of `table`; `give` gets each row that dropped and how
/// many. Allocation-free.
pub fn roll(table: &[Drop], rng: &mut Rng, mut give: impl FnMut(&Drop, u32)) {
    for d in table {
        if rng.die(ALWAYS) > d.rate {
            continue;
        }
        let n = if d.max > d.min { d.min + rng.die(d.max - d.min + 1) - 1 } else { d.min };
        give(d, n);
    }
}

/// [`roll`] with every chance scaled by `permille` / 1000 (mission
/// modifiers that add monsters make each kill worth less, Steal makes the
/// thief's worth more): exact, rates under one are not rounded up, and a
/// chance never goes past 100%.
pub fn roll_scaled(table: &[Drop], permille: u32, rng: &mut Rng, mut give: impl FnMut(&Drop, u32)) {
    if permille == 1000 {
        return roll(table, rng, give);
    }
    for d in table {
        if rng.die(ALWAYS * 1000) > (d.rate.saturating_mul(permille)).min(ALWAYS * 1000) {
            continue;
        }
        let n = if d.max > d.min { d.min + rng.die(d.max - d.min + 1) - 1 } else { d.min };
        give(d, n);
    }
}

/// `amount × permille / 1000`, rounded up or down at random so the mean is
/// exact (a 2-zeny kill at 667 ‰ pays 1 or 2, 1.33 on average; above
/// 1000 ‰ it pays more: an affixed monster's bonus).
pub fn scale_amount(amount: u64, permille: u32, rng: &mut Rng) -> u64 {
    if permille == 1000 {
        return amount;
    }
    let scaled = amount * permille as u64;
    let (whole, rest) = (scaled / 1000, (scaled % 1000) as u32);
    whole + u64::from(rest > 0 && rng.die(1000) <= rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Vec<Drop> {
        vec![
            Drop::new("jellopy", 12.0, None).unwrap(),
            Drop::new("red_herb", 50.0, Some([1, 3])).unwrap(),
            Drop::new("knife", 0.05, None).unwrap(),
            Drop::new(ZENY, 100.0, Some([10, 20])).unwrap(),
        ]
    }

    #[test]
    fn rows_resolve_against_the_catalogue() {
        let t = table();
        assert_eq!((t[0].item, t[0].rate, t[0].rare), ("jellopy", 1200, false));
        assert_eq!((t[1].min, t[1].max), (1, 3));
        assert!(t[2].rare && t[2].rate == 5, "equipment and tiny chances are rare");
        assert!(t[3].is_zeny() && !t[3].rare && t[3].rate == ALWAYS);
        assert!(Drop::new("nope", 1.0, None).is_err());
        assert!(Drop::new("jellopy", 0.0, None).is_err());
        assert!(Drop::new("jellopy", 1.0, Some([2, 1])).is_err());
        assert_eq!(Drop::new("jellopy", 0.001, None).unwrap().rate, 1, "never below one in ten thousand");
        assert_eq!(Drop::new("jellopy", 250.0, None).unwrap().rate, ALWAYS);
    }

    #[test]
    fn rolls_are_deterministic_and_rates_hold() {
        let t = table();
        let run = |seed: u64| {
            let mut rng = Rng::new(seed);
            let mut got = [0u32; 4];
            let mut counts = [0u32; 4];
            for _ in 0..20_000 {
                roll(&t, &mut rng, |d, n| {
                    let i = t.iter().position(|r| r == d).unwrap();
                    got[i] += 1;
                    counts[i] += n;
                    assert!((d.min..=d.max).contains(&n));
                });
            }
            (got, counts)
        };
        assert_eq!(run(7), run(7), "same seed, same loot");
        let (got, counts) = run(7);
        // 12%, 50%, 0.05%, 100% of 20 000 kills.
        assert!((2_100..2_700).contains(&got[0]), "jellopy {}", got[0]);
        assert!((9_600..10_400).contains(&got[1]), "herbs {}", got[1]);
        assert!(got[2] <= 40, "knives {}", got[2]);
        assert_eq!(got[3], 20_000);
        // Herbs average two per drop, zeny fifteen.
        let avg = counts[1] as f32 / got[1] as f32;
        assert!((1.9..2.1).contains(&avg), "herb count {avg}");
        let z = counts[3] as f32 / got[3] as f32;
        assert!((14.7..15.3).contains(&z), "zeny {z}");
    }

    /// Steal raises the odds past the table (1400 ‰), never past a sure drop.
    #[test]
    fn raised_odds_scale_up_and_cap_at_always() {
        let t = table();
        let mut rng = Rng::new(3);
        let mut got = [0u32; 4];
        for _ in 0..20_000 {
            roll_scaled(&t, 1400, &mut rng, |d, _| got[t.iter().position(|r| r == d).unwrap()] += 1);
        }
        // 12% × 1.4 = 16.8%, 50% × 1.4 = 70%; the zeny row stays at 100%.
        assert!((3_100..3_650).contains(&got[0]), "jellopy {}", got[0]);
        assert!((13_600..14_400).contains(&got[1]), "herbs {}", got[1]);
        assert_eq!(got[3], 20_000);
    }
}
