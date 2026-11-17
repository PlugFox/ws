//! Dice and the per-instance random source.

/// xoshiro128++: small, fast, seedable, good enough for dice. Not for
/// anything security related.
#[derive(Clone, Debug)]
pub struct Rng {
    s: [u32; 4],
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        // splitmix64 to spread any seed (including 0) over the state.
        let mut z = seed;
        let mut next = || {
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            x ^ (x >> 31)
        };
        let (a, b) = (next(), next());
        Self { s: [a as u32, (a >> 32) as u32, b as u32, (b >> 32) as u32] }
    }

    pub fn next_u32(&mut self) -> u32 {
        let s = &mut self.s;
        let result = s[0].wrapping_add(s[3]).rotate_left(7).wrapping_add(s[0]);
        let t = s[1] << 9;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(11);
        result
    }

    /// Uniform in `1..=sides` (Lemire's multiply-shift; bias < 2^-32·sides).
    pub fn die(&mut self, sides: u32) -> u32 {
        debug_assert!(sides > 0);
        ((self.next_u32() as u64 * sides as u64) >> 32) as u32 + 1
    }

    /// `true` with probability `percent / 100`.
    pub fn chance(&mut self, percent: u32) -> bool {
        self.die(100) <= percent
    }

    /// d20 with advantage (`+1`), disadvantage (`-1`) or neither (`0`).
    pub fn d20(&mut self, advantage: i8) -> u32 {
        let a = self.die(20);
        match advantage.signum() {
            1 => a.max(self.die(20)),
            -1 => a.min(self.die(20)),
            _ => a,
        }
    }
}

/// `count`d`sides` + `bonus`, the unit every skill and monster is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dice {
    pub count: u32,
    pub sides: u32,
    pub bonus: i32,
}

impl Dice {
    pub const fn new(count: u32, sides: u32, bonus: i32) -> Self {
        Self { count, sides, bonus }
    }

    /// The same dice rolled `k` times as many (cantrip scaling); the flat
    /// bonus is not multiplied.
    pub const fn times(self, k: u32) -> Self {
        Self { count: self.count * k, ..self }
    }

    /// Next die size up (d4 → d6 → d8 → d10 → d12 → 2d8), used for weapon
    /// and hit dice at each tier.
    pub const fn step_up(self) -> Self {
        match self.sides {
            4 | 6 | 8 | 10 => Self { sides: self.sides + 2, ..self },
            _ => Self { count: self.count * 2, sides: 8, ..self },
        }
    }

    /// Average, rounded down (tooltips, balance tables).
    pub const fn average_x2(self) -> i32 {
        (self.count * (self.sides + 1)) as i32 + 2 * self.bonus
    }

    pub const fn max(self) -> i32 {
        (self.count * self.sides) as i32 + self.bonus
    }

    /// Roll; a critical rolls the dice twice (5e), never the bonus.
    pub fn roll(self, rng: &mut Rng, critical: bool) -> i32 {
        let n = if critical { self.count * 2 } else { self.count };
        let mut total = self.bonus;
        for _ in 0..n {
            total += rng.die(self.sides) as i32;
        }
        total.max(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dice_stay_in_range_and_average_out() {
        let mut rng = Rng::new(7);
        let d = Dice::new(3, 6, 2);
        let mut sum = 0i64;
        for _ in 0..20_000 {
            let r = d.roll(&mut rng, false);
            assert!((5..=20).contains(&r));
            sum += r as i64;
        }
        let mean = sum as f64 / 20_000.0;
        assert!((mean - 12.5).abs() < 0.1, "mean {mean}");
        assert_eq!(d.average_x2(), 25);
    }

    #[test]
    fn every_face_shows_up() {
        let mut rng = Rng::new(0);
        let mut seen = [0u32; 21];
        for _ in 0..10_000 {
            seen[rng.die(20) as usize] += 1;
        }
        assert_eq!(seen[0], 0);
        assert!(seen[1..].iter().all(|&c| (380..620).contains(&c)), "{seen:?}");
    }

    #[test]
    fn advantage_shifts_the_mean() {
        let mut rng = Rng::new(3);
        let avg = |rng: &mut Rng, a: i8| (0..20_000).map(|_| rng.d20(a) as f64).sum::<f64>() / 20_000.0;
        let (plain, adv, dis) = (avg(&mut rng, 0), avg(&mut rng, 1), avg(&mut rng, -1));
        // Expected: 10.5, 13.825, 7.175.
        assert!((plain - 10.5).abs() < 0.15 && (adv - 13.825).abs() < 0.15 && (dis - 7.175).abs() < 0.15);
    }

    #[test]
    fn same_seed_same_rolls() {
        let (mut a, mut b) = (Rng::new(42), Rng::new(42));
        assert!((0..100).all(|_| a.next_u32() == b.next_u32()));
    }

    #[test]
    fn scaling_helpers() {
        assert_eq!(Dice::new(1, 4, 0).times(5), Dice::new(5, 4, 0));
        assert_eq!(Dice::new(1, 8, 3).step_up(), Dice::new(1, 10, 3));
        assert_eq!(Dice::new(1, 12, 0).step_up(), Dice::new(2, 8, 0));
        let mut rng = Rng::new(1);
        let crit = Dice::new(1, 1, 5).roll(&mut rng, true);
        assert_eq!(crit, 7, "a crit doubles the dice, not the bonus");
    }
}
