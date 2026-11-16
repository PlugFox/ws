//! Monster behaviours (COMBAT.md §4.4 "Behaviours"): how a monster in the
//! full-rate tier places itself around its target. Data-oriented: one
//! [`Behaviour`] byte per slot (`HordeStorage::behaviour`, from the kind's
//! recipe `director.behaviour`) and a two-field state machine
//! (`HordeStorage::tactic`, `tactic_ticks`); [`plan`] is a pure function of
//! them and what the monster sees, so the hot loop pays a match and a few
//! comparisons, and nothing allocates.
//!
//! * **Chaser** (the default): the flow field and line of sight, as ever.
//! * **Kiter** (archers, casters): keeps a band of its shot's reach from its
//!   target: backs off when crowded (slower than a player, for at most
//!   [`KITE_BUDGET_SECS`], then holds its ground), strafes slowly inside the
//!   band, closes in when out of reach or out of sight. Its ranged skill is
//!   its attack (`hub/brain.rs`: line of sight, a shorter global cooldown,
//!   a cap on volleys in flight).
//! * **Skirmisher** (hit and run): chases; right after a swing it breaks
//!   off for [`SKIRMISH_BACK_SECS`], then commits to the next one.
//! * **Support** (buffers): hangs behind the pack on the flow field's
//!   distance (the pack, nearer the players, stands in front for free);
//!   its rally fires over the pack (`brain.rs`).
//! * **Healer**: further back; flees a player who closes in (for at most
//!   [`FLEE_BUDGET_SECS`], then it is winded and can be caught); its heal
//!   beam picks the most wounded monster nearby (`brain.rs`).
//!
//! Far-tier monsters (nobody sees them) all chase: cheap, and they are
//! promoted long before any band matters. A taunt overrides every
//! behaviour.

/// What a monster kind does in a fight (`director.behaviour` in the recipe).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Behaviour {
    #[default]
    Chaser = 0,
    Kiter = 1,
    Skirmisher = 2,
    Support = 3,
    Healer = 4,
}

impl Behaviour {
    /// Recipe ids; unknown ones (and none) chase.
    pub fn from_id(id: &str) -> Self {
        match id {
            "kiter" => Behaviour::Kiter,
            "skirmisher" => Behaviour::Skirmisher,
            "support" => Behaviour::Support,
            "healer" => Behaviour::Healer,
            _ => Behaviour::Chaser,
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Behaviour::Chaser => "chaser",
            Behaviour::Kiter => "kiter",
            Behaviour::Skirmisher => "skirmisher",
            Behaviour::Support => "support",
            Behaviour::Healer => "healer",
        }
    }

    /// The storage byte back (unknown bytes chase).
    pub const fn from_u8(b: u8) -> Self {
        match b {
            1 => Behaviour::Kiter,
            2 => Behaviour::Skirmisher,
            3 => Behaviour::Support,
            4 => Behaviour::Healer,
            _ => Behaviour::Chaser,
        }
    }
}

/// [`HordeStorage::tactic`](super::storage::HordeStorage::tactic) states.
pub const ADVANCE: u8 = 0;
/// Backing off / breaking off / fleeing; `tactic_ticks` is the budget left.
pub const RETREAT: u8 = 1;
/// Out of breath (or cornered): holds its ground for `tactic_ticks`.
pub const WINDED: u8 = 2;

/// A kiter without a known shot (`Horde::set_reach`) keeps to this reach (cells).
pub const DEFAULT_REACH: f32 = 12.0;
/// Kiter band as shares of its reach: backs off under `BACK`, holds still
/// between `BACK` and `HOLD_LO` (no jitter at the edge), strafes inside
/// `HOLD_LO..HOLD_HI`, closes in past `HOLD_HI` (orc archer, 8 m = 16
/// cells: under 7, 9–13, past 13).
pub const KITE_BACK: f32 = 0.45;
pub const KITE_HOLD_LO: f32 = 0.55;
pub const KITE_HOLD_HI: f32 = 0.8;
/// A kiter backs off at this share of its own walking speed, never faster
/// than [`KITE_BACK_MAX`] cells/s (a player walks 6): a melee build closes
/// in on it within a couple of seconds.
pub const KITE_BACK_PACE: f32 = 0.85;
pub const KITE_BACK_MAX: f32 = 3.6;
/// It backs off this long at most, then holds its ground (winded) for
/// [`KITE_WINDED_SECS`].
pub const KITE_BUDGET_SECS: f32 = 3.0;
pub const KITE_WINDED_SECS: f32 = 2.0;
/// Cornered (its retreat runs into a wall): it stands and fights this long.
pub const CORNERED_SECS: f32 = 3.0;
/// Strafing inside the band: this share of its speed, one direction for a
/// leg of this many seconds (hashed per slot: no storage).
pub const STRAFE_PACE: f32 = 0.33;
pub const STRAFE_LEG_SECS: f32 = 3.0;
/// A skirmisher breaks off this long after each swing, at this share of
/// its speed.
pub const SKIRMISH_BACK_SECS: f32 = 1.0;
pub const SKIRMISH_BACK_PACE: f32 = 0.85;
/// Support band on the flow field's distance (cells): closes in past
/// `HI`, backs off under `LO`.
pub const SUPPORT_LO: f32 = 5.0;
pub const SUPPORT_HI: f32 = 10.0;
/// Healer band (cells of flow-field distance): flees under `FLEE`, stops
/// fleeing past `LO`, closes in past `HI`.
pub const HEALER_FLEE: f32 = 6.0;
pub const HEALER_LO: f32 = 8.0;
pub const HEALER_HI: f32 = 12.0;
/// A healer flees this long at most, then is winded (holds still, can be
/// caught) for [`HEALER_WINDED_SECS`].
pub const FLEE_BUDGET_SECS: f32 = 2.5;
pub const HEALER_WINDED_SECS: f32 = 4.0;
/// Supports and healers back off at their own speed, at most this fast.
pub const FLEE_MAX: f32 = 4.0;

/// What a behaviour monster sees this tick.
#[derive(Clone, Copy, Debug)]
pub struct Sight {
    /// Straight-line distance to its target (cells).
    pub dist: f32,
    /// Flow-field (walking) distance to the nearest player (cells).
    pub path: f32,
    /// Line of sight to the target (the last probe).
    pub los: bool,
    /// Its shot's reach (kiters; cells).
    pub reach: f32,
    /// Its melee cooldown now (ticks).
    pub cooldown: u16,
    /// Simulation rate.
    pub hz: f32,
}

/// The move [`plan`] picks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Move {
    /// The plain chase (field, line of sight, contact).
    Chase,
    /// It stands where it is (and shoots, or rallies).
    Stay,
    /// Away from its target (up the flow field), at this share of its
    /// speed, capped at `max` cells/s.
    Away { pace: f32, max: f32 },
    /// Sideways round its target at this share of its speed.
    Strafe { pace: f32 },
}

fn ticks(secs: f32, hz: f32) -> u16 {
    (secs * hz).round().clamp(1.0, u16::MAX as f32) as u16
}

/// The state a freshly spawned monster of `b` starts in: `(tactic, ticks)`.
/// A skirmisher's ticks hold the melee cooldown it last saw (a jump up is
/// a swing); `u16::MAX` so its spawn stagger is not taken for one.
pub const fn initial(b: Behaviour) -> (u8, u16) {
    match b {
        Behaviour::Skirmisher => (ADVANCE, u16::MAX),
        _ => (ADVANCE, 0),
    }
}

/// Steps the state machine of a behaviour monster by `period` ticks and
/// picks its move. `cornered`: its retreat step is blocked by a wall.
pub fn plan(b: Behaviour, tactic: &mut u8, left: &mut u16, see: Sight, period: u16, cornered: bool) -> Move {
    let tick_down = |left: &mut u16| *left = left.saturating_sub(period);
    match b {
        Behaviour::Chaser => Move::Chase,
        Behaviour::Kiter => {
            let r = if see.reach > 0.0 { see.reach } else { DEFAULT_REACH };
            let (back, lo, hi) = (r * KITE_BACK, r * KITE_HOLD_LO, r * KITE_HOLD_HI);
            if !see.los || see.dist > hi {
                // Out of reach or out of sight: close in (and get one's
                // breath back on the way).
                *tactic = ADVANCE;
                return Move::Chase;
            }
            match *tactic {
                WINDED => {
                    tick_down(left);
                    if *left == 0 {
                        *tactic = ADVANCE;
                    }
                    Move::Stay
                }
                RETREAT => {
                    tick_down(left);
                    if see.dist >= lo {
                        *tactic = ADVANCE;
                        Move::Stay
                    } else if *left == 0 || cornered {
                        *tactic = WINDED;
                        *left = ticks(if cornered { CORNERED_SECS } else { KITE_WINDED_SECS }, see.hz);
                        Move::Stay
                    } else {
                        Move::Away { pace: KITE_BACK_PACE, max: KITE_BACK_MAX }
                    }
                }
                _ => {
                    if see.dist < back {
                        if cornered {
                            *tactic = WINDED;
                            *left = ticks(CORNERED_SECS, see.hz);
                            return Move::Stay;
                        }
                        *tactic = RETREAT;
                        *left = ticks(KITE_BUDGET_SECS, see.hz);
                        Move::Away { pace: KITE_BACK_PACE, max: KITE_BACK_MAX }
                    } else if see.dist >= lo {
                        Move::Strafe { pace: STRAFE_PACE }
                    } else {
                        Move::Stay
                    }
                }
            }
        }
        Behaviour::Skirmisher => match *tactic {
            RETREAT => {
                tick_down(left);
                if *left == 0 || cornered {
                    // Commit to the next swing.
                    *tactic = ADVANCE;
                    *left = see.cooldown;
                    Move::Chase
                } else {
                    Move::Away { pace: SKIRMISH_BACK_PACE, max: f32::INFINITY }
                }
            }
            _ => {
                // Its melee cooldown jumped up since the last look: it swung.
                let swung = see.cooldown > *left;
                *left = see.cooldown;
                if swung && !cornered {
                    *tactic = RETREAT;
                    *left = ticks(SKIRMISH_BACK_SECS, see.hz);
                    Move::Away { pace: SKIRMISH_BACK_PACE, max: f32::INFINITY }
                } else {
                    Move::Chase
                }
            }
        },
        Behaviour::Support | Behaviour::Healer => {
            let healer = b == Behaviour::Healer;
            let (flee, lo, hi, budget, winded) = if healer {
                (HEALER_FLEE, HEALER_LO, HEALER_HI, FLEE_BUDGET_SECS, HEALER_WINDED_SECS)
            } else {
                (SUPPORT_LO, SUPPORT_LO + 1.0, SUPPORT_HI, KITE_BUDGET_SECS, KITE_WINDED_SECS)
            };
            match *tactic {
                WINDED => {
                    tick_down(left);
                    if *left == 0 {
                        *tactic = ADVANCE;
                    }
                    if see.path > hi { Move::Chase } else { Move::Stay }
                }
                RETREAT => {
                    tick_down(left);
                    if see.path >= lo {
                        *tactic = ADVANCE;
                        Move::Stay
                    } else if *left == 0 || cornered {
                        *tactic = WINDED;
                        *left = ticks(if cornered { CORNERED_SECS } else { winded }, see.hz);
                        Move::Stay
                    } else {
                        Move::Away { pace: 1.0, max: FLEE_MAX }
                    }
                }
                _ => {
                    if see.path > hi {
                        Move::Chase
                    } else if see.path < flee && !cornered {
                        *tactic = RETREAT;
                        *left = ticks(budget, see.hz);
                        Move::Away { pace: 1.0, max: FLEE_MAX }
                    } else {
                        Move::Stay
                    }
                }
            }
        }
    }
}

/// The strafing side (+1 / −1) of a slot for the leg `tick` falls in:
/// a hash of slot, generation and leg (no storage), legs staggered by slot.
pub fn strafe_side(slot: u16, generation: u8, tick: u32, hz: u32) -> f32 {
    let leg_ticks = ((STRAFE_LEG_SECS * hz as f32) as u32).max(1);
    let leg = tick.wrapping_add((slot as u32).wrapping_mul(53)) / leg_ticks;
    let mut h = (slot as u64) << 40 | (generation as u64) << 32 | leg as u64;
    h = (h ^ (h >> 31)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= h >> 29;
    if h & 1 == 0 { 1.0 } else { -1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sight(dist: f32) -> Sight {
        Sight { dist, path: dist, los: true, reach: 16.0, cooldown: 0, hz: 20.0 }
    }

    #[test]
    fn behaviour_ids_round_trip() {
        for b in [Behaviour::Chaser, Behaviour::Kiter, Behaviour::Skirmisher, Behaviour::Support, Behaviour::Healer] {
            assert_eq!(Behaviour::from_id(b.id()), b);
            assert_eq!(Behaviour::from_u8(b as u8), b);
        }
        assert_eq!(Behaviour::from_id(""), Behaviour::Chaser);
        assert_eq!(Behaviour::from_id("wizard"), Behaviour::Chaser);
    }

    #[test]
    fn a_kiter_keeps_its_band_backs_off_for_a_while_then_holds() {
        let (mut t, mut left) = initial(Behaviour::Kiter);
        let go = |d: f32, t: &mut u8, l: &mut u16| plan(Behaviour::Kiter, t, l, sight(d), 1, false);
        // Reach 16: past 12.8 it closes in, inside 8.8..12.8 it strafes,
        // 7.2..8.8 it stands, under 7.2 it backs off.
        assert_eq!(go(14.0, &mut t, &mut left), Move::Chase);
        assert_eq!(go(10.0, &mut t, &mut left), Move::Strafe { pace: STRAFE_PACE });
        assert_eq!(go(8.0, &mut t, &mut left), Move::Stay);
        assert!(matches!(go(5.0, &mut t, &mut left), Move::Away { .. }));
        assert_eq!(t, RETREAT);
        // A player keeps on it: after 3 s (60 ticks) it gives up and holds.
        let mut moves = 0;
        for _ in 0..80 {
            if matches!(go(5.0, &mut t, &mut left), Move::Away { .. }) {
                moves += 1;
            }
        }
        assert!((58..=60).contains(&moves), "backs off for 3 s: {moves} ticks");
        assert_eq!(t, WINDED);
        // Winded: stands its ground for 2 s, then backs off again.
        for _ in 0..40 {
            go(5.0, &mut t, &mut left);
        }
        assert!(matches!(go(5.0, &mut t, &mut left), Move::Away { .. }));
        // Far enough again: it stops.
        assert_eq!(go(9.0, &mut t, &mut left), Move::Stay);
        assert_eq!(t, ADVANCE);
    }

    #[test]
    fn a_kiter_without_sight_closes_in_and_a_cornered_one_fights() {
        let (mut t, mut left) = initial(Behaviour::Kiter);
        let blind = Sight { los: false, ..sight(10.0) };
        assert_eq!(plan(Behaviour::Kiter, &mut t, &mut left, blind, 1, false), Move::Chase);
        assert_eq!(plan(Behaviour::Kiter, &mut t, &mut left, sight(4.0), 1, true), Move::Stay, "cornered");
        assert_eq!((t, left), (WINDED, 60));
    }

    #[test]
    fn a_skirmisher_breaks_off_after_a_swing_then_commits() {
        let (mut t, mut left) = initial(Behaviour::Skirmisher);
        let at = |cooldown: u16| Sight { cooldown, ..sight(1.0) };
        // Its spawn stagger is no swing.
        assert_eq!(plan(Behaviour::Skirmisher, &mut t, &mut left, at(5), 1, false), Move::Chase);
        assert_eq!(plan(Behaviour::Skirmisher, &mut t, &mut left, at(4), 1, false), Move::Chase);
        assert_eq!(plan(Behaviour::Skirmisher, &mut t, &mut left, at(0), 1, false), Move::Chase);
        // A swing: the cooldown jumps to the interval.
        assert!(matches!(plan(Behaviour::Skirmisher, &mut t, &mut left, at(32), 1, false), Move::Away { .. }));
        let mut away = 1;
        for c in (0..31).rev() {
            if matches!(plan(Behaviour::Skirmisher, &mut t, &mut left, at(c), 1, false), Move::Away { .. }) {
                away += 1;
            }
        }
        assert_eq!(away, 20, "1 s at 20 Hz");
        assert_eq!(t, ADVANCE);
        // Coming back while the cooldown still runs down: no false swing.
        assert_eq!(plan(Behaviour::Skirmisher, &mut t, &mut left, at(0), 1, false), Move::Chase);
    }

    #[test]
    fn a_healer_hangs_back_flees_then_tires() {
        let (mut t, mut left) = initial(Behaviour::Healer);
        let go = |d: f32, t: &mut u8, l: &mut u16| plan(Behaviour::Healer, t, l, sight(d), 1, false);
        assert_eq!(go(20.0, &mut t, &mut left), Move::Chase);
        assert_eq!(go(10.0, &mut t, &mut left), Move::Stay);
        assert!(matches!(go(4.0, &mut t, &mut left), Move::Away { .. }));
        let mut fled = 1;
        for _ in 0..100 {
            if matches!(go(4.0, &mut t, &mut left), Move::Away { .. }) {
                fled += 1;
            }
        }
        assert_eq!(fled, 50, "2.5 s of flight, then winded for 4 s");
        assert_eq!(t, WINDED);
        // A support stands closer to the pack.
        let (mut t, mut left) = initial(Behaviour::Support);
        assert_eq!(plan(Behaviour::Support, &mut t, &mut left, sight(7.0), 1, false), Move::Stay);
        assert_eq!(plan(Behaviour::Support, &mut t, &mut left, sight(11.0), 1, false), Move::Chase);
        assert!(matches!(plan(Behaviour::Support, &mut t, &mut left, sight(3.0), 1, false), Move::Away { .. }));
    }

    #[test]
    fn strafing_sides_hold_for_a_leg_and_differ_between_slots() {
        let a: Vec<f32> = (0..60).map(|t| strafe_side(3, 0, t, 20)).collect();
        assert!(a.windows(2).filter(|w| w[0] != w[1]).count() <= 1, "one flip at most within a 3 s leg");
        let sides: Vec<f32> = (0..32).map(|s| strafe_side(s, 0, 0, 20)).collect();
        assert!(sides.contains(&1.0) && sides.contains(&-1.0));
    }
}
