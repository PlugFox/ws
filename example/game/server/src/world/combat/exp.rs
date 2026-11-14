//! Experience: the RO-shaped curve and level-weighted party sharing
//! (COMBAT.md §1.1).

use super::monster::Archetype;

/// Experience of a standard (swarm) monster of level `ml`: `round(3·ml^1.4 + 2)`.
pub fn swarm_exp(ml: u32) -> u64 {
    (3.0 * (ml.max(1) as f64).powf(1.4) + 2.0).round() as u64
}

/// Same-level swarm kills a level takes, with the RO wall after 80.
pub fn kills_per_level(level: u32) -> f64 {
    let l = level as f64;
    let k = 15.0 + 6.0 * l + 0.06 * l * l;
    if level > 80 { k * (1.0 + ((l - 80.0) / 7.0).powi(2)) } else { k }
}

/// Experience needed to go from `level` to `level + 1`; 0 at the cap.
pub fn exp_to_next(level: u32) -> u64 {
    if level == 0 || level >= super::MAX_LEVEL {
        return 0;
    }
    (swarm_exp(level) as f64 * kills_per_level(level)).round() as u64
}

/// Base value of a kill before level differences and sharing.
pub fn kill_value(ml: u32, archetype: Archetype) -> u64 {
    let tenths: u64 = match archetype {
        Archetype::Swarm => 10,
        Archetype::Chaser => 12,
        Archetype::Tank => 30,
        Archetype::Elite => 80,
        Archetype::Boss => 1500,
        // Plants feed nobody (RO's give 0-1 exp): their herbs are the reward.
        Archetype::Plant => 0,
    };
    (swarm_exp(ml) * tenths + 5) / 10
}

/// Level-difference modifier in percent, `diff = monster − player`.
pub const fn level_diff_percent(diff: i32) -> u32 {
    if diff >= 11 {
        150
    } else if diff >= 1 {
        100 + 5 * diff as u32
    } else if diff >= -5 {
        100
    } else if diff >= -20 {
        let p = 100 - 6 * (-diff - 5);
        if p < 10 { 10 } else { p as u32 }
    } else {
        10
    }
}

/// Splits a kill between the party members in the instance (`levels`, in
/// the order the result comes back). Each member's share of the pool is
/// weighted by how close their level is to the monster's, so high monsters
/// feed high characters and low monsters feed low ones; the party bonus is
/// +25% per extra member; each share then takes the member's own
/// level-difference modifier.
pub fn share_kill(value: u64, ml: u32, levels: &[u32]) -> Vec<u64> {
    if levels.is_empty() {
        return Vec::new();
    }
    let weight = |l: u32| {
        let d = (ml as f64 - l as f64) / 10.0;
        1.0 / (1.0 + d * d)
    };
    let total: f64 = levels.iter().map(|&l| weight(l)).sum();
    let pool = value as f64 * (1.0 + 0.25 * (levels.len() - 1) as f64);
    levels
        .iter()
        .map(|&l| {
            let pct = level_diff_percent(ml as i32 - l as i32) as f64 / 100.0;
            (pool * weight(l) / total * pct).round() as u64
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows of the COMBAT.md §1.1 table: (L, e(L), need(L), total to L+1).
    #[test]
    fn curve_matches_the_design_table() {
        let rows = [
            (1, 5, 105, 105),
            (5, 31, 1_442, 3_275),
            (10, 77, 6_237, 23_221),
            (30, 353, 87_897, 804_554),
            (60, 928, 548_448, 9_265_044),
            (90, 1_635, 5_175_576, 53_947_154),
            (98, 1_842, 16_535_014, 140_079_415),
        ];
        for (l, e, need, total) in rows {
            assert_eq!(swarm_exp(l), e, "e({l})");
            assert_eq!(exp_to_next(l), need, "need({l})");
            assert_eq!((1..=l).map(exp_to_next).sum::<u64>(), total, "total to {}", l + 1);
        }
        assert_eq!(exp_to_next(99), 0);
    }

    #[test]
    fn level_difference_table() {
        let cases = [(15, 150), (11, 150), (10, 150), (1, 105), (0, 100), (-5, 100), (-6, 94), (-20, 10), (-40, 10)];
        for (d, p) in cases {
            assert_eq!(level_diff_percent(d), p, "diff {d}");
        }
    }

    #[test]
    fn sharing_follows_levels() {
        // Solo: value × modifier.
        assert_eq!(share_kill(100, 10, &[10]), vec![100]);
        assert_eq!(share_kill(100, 10, &[20]), vec![70]);
        // Four equal players: 43.75% each, 175% together.
        assert_eq!(share_kill(100, 30, &[30, 30, 30, 30]), vec![44, 44, 44, 44]);
        // A high monster feeds the high character, a low one the low.
        let high = share_kill(1000, 50, &[10, 50]);
        assert!(high[1] > 10 * high[0], "{high:?}");
        let low = share_kill(1000, 10, &[10, 50]);
        assert!(low[0] > 100 * low[1].max(1), "{low:?}");
        assert!(share_kill(5, 1, &[]).is_empty());
    }

    #[test]
    fn archetype_values() {
        assert_eq!(kill_value(1, Archetype::Swarm), 5);
        assert_eq!(kill_value(1, Archetype::Chaser), 6);
        assert_eq!(kill_value(25, Archetype::Boss), swarm_exp(25) * 150);
    }
}
