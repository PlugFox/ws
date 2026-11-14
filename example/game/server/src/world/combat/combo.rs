//! Kill combo and style rank (COMBAT.md §5.4): every kill a player lands
//! extends their combo and refills its timer; a window without a kill (or
//! being downed) drops it to zero. The count climbs through letter ranks
//! D … SSS, and the higher the rank the shorter the window, so the top
//! ranks need a steady stream of kills.
//!
//! Pure and tick-based like the rest of `combat`: the instance calls
//! [`Combo::kill`] per kill and [`Combo::tick`] once per tick. Skills that
//! scale with the combo read [`Combo::count`] and [`Combo::rank`] through
//! `Fighter::combo`.

/// Style ranks, low to high (`u8` on the wire: 0 none … 7 SSS).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Rank {
    /// Below the first rank: the counter shows, no letter yet.
    #[default]
    None = 0,
    D,
    C,
    B,
    A,
    S,
    SS,
    SSS,
}

/// Kills needed for each rank, D … SSS.
pub const THRESHOLDS: [u32; 7] = [10, 25, 50, 100, 175, 275, 400];

/// Seconds without a kill before the combo drops, per rank (none … SSS).
pub const WINDOW_SECS: [f32; 8] = [4.0, 4.0, 4.0, 3.5, 3.5, 3.0, 3.0, 2.5];

/// Kills before the combo shows at all (and is sent): a stray kill or two
/// is not a combo.
pub const SHOW_FROM: u32 = 2;

impl Rank {
    pub const ALL: [Rank; 8] = [Rank::None, Rank::D, Rank::C, Rank::B, Rank::A, Rank::S, Rank::SS, Rank::SSS];

    /// The rank a combo of `count` kills holds.
    pub fn of(count: u32) -> Rank {
        let reached = THRESHOLDS.iter().take_while(|&&t| count >= t).count();
        Rank::ALL[reached]
    }

    pub fn code(self) -> u8 {
        self as u8
    }

    /// Seconds a combo of this rank survives without a kill.
    pub fn window_secs(self) -> f32 {
        WINDOW_SECS[self as usize]
    }

    /// Kills at which this rank starts (0 for none).
    pub fn threshold(self) -> u32 {
        match self {
            Rank::None => 0,
            r => THRESHOLDS[r as usize - 1],
        }
    }
}

/// One player's combo.
#[derive(Clone, Debug, Default)]
pub struct Combo {
    count: u32,
    rank: Rank,
    /// Ticks until the combo drops (0 = no combo).
    left: u32,
    /// The full window at the current rank, ticks.
    window: u32,
    /// Highest count of this stage.
    best: u32,
    /// Changed since the last [`Combo::take_dirty`].
    dirty: bool,
}

impl Combo {
    /// Kills in a row (0 = no combo).
    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn rank(&self) -> Rank {
        self.rank
    }

    /// Ticks until it drops, and the full window at this rank.
    pub fn timer(&self) -> (u32, u32) {
        (self.left, self.window)
    }

    /// The best combo since the fighter entered the stage.
    pub fn best(&self) -> u32 {
        self.best
    }

    /// `kills` more kills (at `hz` ticks a second): the timer refills.
    /// Returns the new rank when it went up.
    pub fn kill(&mut self, kills: u32, hz: f32) -> Option<Rank> {
        if kills == 0 {
            return None;
        }
        let before = self.rank;
        self.count = self.count.saturating_add(kills);
        self.best = self.best.max(self.count);
        self.rank = Rank::of(self.count);
        self.window = ((self.rank.window_secs() * hz).round() as u32).max(1);
        self.left = self.window;
        self.dirty = true;
        (self.rank > before).then_some(self.rank)
    }

    /// One tick without (or after) kills; true when the combo just dropped.
    pub fn tick(&mut self) -> bool {
        if self.left == 0 {
            return false;
        }
        self.left -= 1;
        if self.left == 0 {
            return self.reset();
        }
        false
    }

    /// Drops the combo (downed, left the fight); true if there was one.
    pub fn reset(&mut self) -> bool {
        if self.count == 0 {
            return false;
        }
        let shown = self.count >= SHOW_FROM;
        *self = Combo { best: self.best, ..Combo::default() };
        // A combo the client never saw needs no message to end it.
        self.dirty = shown;
        shown
    }

    /// Whether the client should hear about it (a change of a combo that
    /// shows, or the end of one). Clears the flag.
    pub fn take_dirty(&mut self) -> bool {
        let shown = self.count == 0 || self.count >= SHOW_FROM;
        std::mem::take(&mut self.dirty) && shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HZ: f32 = 20.0;

    #[test]
    fn ranks_climb_at_the_thresholds() {
        assert_eq!(Rank::of(0), Rank::None);
        assert_eq!(Rank::of(9), Rank::None);
        assert_eq!(Rank::of(10), Rank::D);
        assert_eq!((Rank::of(24), Rank::of(25)), (Rank::D, Rank::C));
        assert_eq!((Rank::of(49), Rank::of(50)), (Rank::C, Rank::B));
        assert_eq!((Rank::of(99), Rank::of(100)), (Rank::B, Rank::A));
        assert_eq!((Rank::of(174), Rank::of(175)), (Rank::A, Rank::S));
        assert_eq!((Rank::of(274), Rank::of(275)), (Rank::S, Rank::SS));
        assert_eq!((Rank::of(399), Rank::of(400)), (Rank::SS, Rank::SSS));
        assert_eq!(Rank::of(u32::MAX), Rank::SSS);
        for r in Rank::ALL {
            assert_eq!(Rank::of(r.threshold()), r);
        }
        // Higher ranks never forgive longer pauses.
        assert!(WINDOW_SECS.windows(2).all(|w| w[1] <= w[0]));
        assert_eq!(Rank::SSS.code(), 7);
    }

    #[test]
    fn kills_extend_and_report_rank_ups() {
        let mut c = Combo::default();
        assert_eq!(c.kill(9, HZ), None);
        assert_eq!((c.count(), c.rank()), (9, Rank::None));
        assert_eq!(c.kill(1, HZ), Some(Rank::D));
        assert_eq!(c.kill(1, HZ), None, "no rank-up within a rank");
        // An area kill can jump several ranks: the new one is reported.
        assert_eq!(c.kill(100, HZ), Some(Rank::A));
        assert_eq!(c.count(), 111);
        assert_eq!(c.timer(), (70, 70), "A: 3.5 s at 20 Hz");
        assert_eq!(c.kill(0, HZ), None);
    }

    #[test]
    fn a_kill_refills_the_timer() {
        let mut c = Combo::default();
        c.kill(3, HZ);
        for _ in 0..79 {
            assert!(!c.tick());
        }
        assert_eq!(c.timer(), (1, 80));
        c.kill(1, HZ);
        assert_eq!(c.timer(), (80, 80));
        assert_eq!(c.count(), 4);
    }

    #[test]
    fn it_drops_after_the_window() {
        let mut c = Combo::default();
        c.kill(400, HZ);
        assert_eq!(c.rank(), Rank::SSS);
        assert!(c.take_dirty());
        for _ in 0..49 {
            assert!(!c.tick());
        }
        assert!(c.tick(), "2.5 s at SSS");
        assert_eq!((c.count(), c.rank(), c.timer()), (0, Rank::None, (0, 0)));
        assert_eq!(c.best(), 400);
        assert!(c.take_dirty(), "the end is sent");
        assert!(!c.tick(), "nothing left to drop");
        assert!(!c.take_dirty());
    }

    #[test]
    fn single_kills_stay_quiet() {
        let mut c = Combo::default();
        c.kill(1, HZ);
        assert!(!c.take_dirty(), "one kill is not a combo");
        assert!(!c.reset());
        assert!(!c.take_dirty());
        c.kill(2, HZ);
        assert!(c.take_dirty());
        assert!(c.reset(), "downed");
        assert!(c.take_dirty());
        assert_eq!(c.count(), 0);
    }
}
