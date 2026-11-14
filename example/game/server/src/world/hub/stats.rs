//! Run statistics: what each party member did on a mission, for the result
//! screen (COMBAT.md §4.11, `RUN_STATS` / `RUN_REWARDS` after `STAGE_END`).
//!
//! The instance keeps one [`Seat`] per player who ever stood on the stage
//! (at most [`SEATS`]): a reconnect sits in the same seat, a player who
//! leaves keeps theirs (marked as left). Everything is fixed size and
//! allocated when the seat is taken; recording a hit, a kill or a tick
//! never allocates:
//!
//! - **Damage dealt**: HP taken off monsters by the player's basic attack
//!   and skills, per skill (the [`SKILLS`] most damaging kept by name, the
//!   rest summed as `other`) and per [`BUCKETS`] time bucket of the run
//!   clock. Plants are no damage (they are cut), burning and poison ticks
//!   are nobody's (the monster does not remember who lit it).
//! - **Damage taken**: what came off the player's HP and temporary HP
//!   (the `COMBAT` hit numbers on them), per bucket too.
//! - **Healing**: HP the player's skills restored to anyone (potions are
//!   not healing done).
//! - **Kills**: credited as the record credits them (`world::record`): the
//!   killing blow, else the looter. Per bucket.
//! - **Assists**: a kill credited to someone else where this player's own
//!   last damaging hit on that monster landed at most [`ASSIST_SECS`]
//!   before it. One assist per kill and helper, whatever the hit count.
//! - **Downs** (standing → downed), **outs** (dead for the wave: bled out
//!   or killed by the fog), **revives** (a downed ally back up: the
//!   reviving skill's caster, else the nearest standing ally, the one whose
//!   presence fills the bar), **best combo** (the highest kill combo of the
//!   run) and **MVPs** (bosses slain with the player on the map).
//!
//! The timeline marks (at most [`MARKS`]) are the run's turning points:
//! bosses arriving and slain, elites slain, players going down and out.

use super::HubPlayer;
use super::fight::Vital;
use crate::world::combat::SkillBook;
use crate::world::horde::storage::{HordeStorage, FLAG_BOSS};
use crate::world::protocol::{hit, mark, seat_flags, HitEvent, RunBucket, RunMark, RunSeat, MONSTER_ENTITY_BASE};

/// Seats of one run: a full party and its replacements.
pub(super) const SEATS: usize = 8;
/// Time buckets of the run clock; a run longer than `BUCKETS × 10 s`
/// widens them (see [`RunLog::new`]).
pub(super) const BUCKETS: usize = 90;
pub(super) const BUCKET_SECS: u32 = 10;
/// Skills kept by name per seat.
pub(super) const SKILLS: usize = 12;
/// Timeline marks of a run, and how many of them elites may fill.
pub(super) const MARKS: usize = 48;
const ELITE_MARKS: usize = 16;
/// A hit this recent on a monster someone else kills is an assist (s).
pub(super) const ASSIST_SECS: f32 = 5.0;
/// Items kept per seat's loot list (kinds).
pub(super) const LOOT_KINDS: usize = 32;
/// The skill "index" of the basic attack, and of the folded rest.
pub(super) const SKILL_ATTACK: u16 = u16::MAX - 1;
const SKILL_OTHER: u16 = u16::MAX;

/// Counters of one seat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Totals {
    pub dealt: u64,
    pub taken: u64,
    pub healed: u64,
    pub kills: u32,
    pub assists: u32,
    pub downs: u32,
    pub deaths: u32,
    pub revives: u32,
    pub best_combo: u32,
    pub mvps: u32,
}

/// One party member's run.
#[derive(Clone, Debug)]
pub(super) struct Seat {
    pub user_id: u32,
    pub character_id: u32,
    pub name: String,
    pub class: String,
    /// Left the stage while the mission ran.
    pub left: bool,
    pub totals: Totals,
    /// (skill index, damage, damaging hits); `skill_len` used.
    skills: [(u16, u64, u32); SKILLS + 1],
    skill_len: usize,
    buckets: Box<[RunBucket; BUCKETS]>,
    /// Vital state seen last tick (`Vital` code), for downs and revives.
    was: u8,
    /// What the player picked up (merged stacks, the chest too).
    pub loot: Vec<(&'static str, u32)>,
}

impl Seat {
    fn new(user_id: u32, character_id: u32, name: &str, class: &str) -> Self {
        Seat {
            user_id,
            character_id,
            name: name.into(),
            class: class.into(),
            left: false,
            totals: Totals::default(),
            skills: [(0, 0, 0); SKILLS + 1],
            skill_len: 0,
            buckets: Box::new([RunBucket::default(); BUCKETS]),
            was: 0,
            loot: Vec::with_capacity(8),
        }
    }

    /// Damage of `skill` (its own row, else the folded `other` one).
    fn add_skill(&mut self, skill: u16, amount: u64) {
        let used = &mut self.skills[..self.skill_len];
        if let Some(row) = used.iter_mut().find(|r| r.0 == skill) {
            row.1 += amount;
            row.2 += 1;
            return;
        }
        let at = if self.skill_len < SKILLS {
            self.skill_len += 1;
            self.skills[self.skill_len - 1] = (skill, 0, 0);
            self.skill_len - 1
        } else {
            // The last row is `other`, made on the first overflow.
            if self.skill_len == SKILLS {
                self.skill_len += 1;
                self.skills[SKILLS] = (SKILL_OTHER, 0, 0);
            }
            SKILLS
        };
        self.skills[at].1 += amount;
        self.skills[at].2 += 1;
    }

    /// The skill rows, the most damaging first.
    pub(super) fn skills(&self) -> Vec<(u16, u64, u32)> {
        let mut rows = self.skills[..self.skill_len].to_vec();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        rows
    }

    #[cfg(test)]
    pub(super) fn bucket(&self, i: usize) -> RunBucket {
        self.buckets[i]
    }

    /// Something the player picked up (bounded to [`LOOT_KINDS`] kinds).
    pub(super) fn add_loot(&mut self, item: &'static str, n: u32) {
        if let Some(l) = self.loot.iter_mut().find(|(id, _)| *id == item) {
            l.1 = l.1.saturating_add(n);
        } else if self.loot.len() < LOOT_KINDS {
            self.loot.push((item, n));
        }
    }
}

/// The run's statistics, kept by the stage instance.
#[derive(Clone, Debug)]
pub(super) struct RunLog {
    seats: Vec<Seat>,
    /// Per monster slot: the tick (+1; 0 = never) of each seat's last
    /// damaging hit, and the slot's generation it was for.
    hit_at: Vec<[u32; SEATS]>,
    hit_gen: Vec<u8>,
    marks: Vec<RunMark>,
    elite_marks: usize,
    /// Bosses already marked as arrived (slot, generation).
    seen: Vec<(u16, u8)>,
    /// Width of a time bucket (s).
    bucket_secs: u32,
    /// The caster of a revive skill that acted this tick.
    revive_cast: Option<(u32, u32)>,
}

impl Default for RunLog {
    fn default() -> Self {
        Self::new(0.0)
    }
}

impl RunLog {
    /// A run whose clock may reach `limit_secs`: buckets of
    /// [`BUCKET_SECS`], wider when the run would not fit in [`BUCKETS`].
    pub(super) fn new(limit_secs: f32) -> Self {
        let need = if limit_secs.is_finite() { (limit_secs.max(0.0) / BUCKETS as f32).ceil() as u32 } else { 0 };
        RunLog {
            seats: Vec::new(),
            hit_at: Vec::new(),
            hit_gen: Vec::new(),
            marks: Vec::with_capacity(MARKS),
            elite_marks: 0,
            seen: Vec::new(),
            bucket_secs: need.max(BUCKET_SECS),
            revive_cast: None,
        }
    }

    pub(super) fn bucket_secs(&self) -> u32 {
        self.bucket_secs
    }

    pub(super) fn seats(&self) -> &[Seat] {
        &self.seats
    }

    fn bucket_of(&self, secs: f32) -> usize {
        ((secs.max(0.0) as u32 / self.bucket_secs) as usize).min(BUCKETS - 1)
    }

    pub(super) fn index(&self, user_id: u32) -> Option<usize> {
        self.seats.iter().position(|s| s.user_id == user_id)
    }

    pub(super) fn seat_mut(&mut self, user_id: u32) -> Option<&mut Seat> {
        self.seats.iter_mut().find(|s| s.user_id == user_id)
    }

    /// A player arrives on the stage: their seat (the old one when they
    /// come back). `None` when the seats are full.
    pub(super) fn sit(&mut self, user_id: u32, character_id: u32, name: &str, class: &str) -> Option<usize> {
        if let Some(i) = self.index(user_id) {
            self.seats[i].left = false;
            return Some(i);
        }
        if self.seats.len() >= SEATS {
            return None;
        }
        self.seats.push(Seat::new(user_id, character_id, name, class));
        Some(self.seats.len() - 1)
    }

    /// The player left the running mission (their numbers stay).
    pub(super) fn leave(&mut self, user_id: u32) {
        if let Some(s) = self.seat_mut(user_id) {
            s.left = true;
        }
    }

    /// The hits `user_id` just landed with `skill` (a skill book index or
    /// [`SKILL_ATTACK`]): damage on monsters (not plants), healing on
    /// players, and who hit what for the assists. `tick` is the instance
    /// tick, `secs` the run clock.
    pub(super) fn credit(&mut self, user_id: u32, skill: u16, hits: &[HitEvent], tick: u32, secs: f32, store: &HordeStorage) {
        let Some(si) = self.index(user_id) else { return };
        let b = self.bucket_of(secs);
        for h in hits {
            if h.flags & hit::MISS != 0 {
                continue;
            }
            if h.target < MONSTER_ENTITY_BASE {
                if h.flags & hit::HEAL != 0 {
                    self.seats[si].totals.healed += u64::from(h.amount);
                }
                continue;
            }
            let slot = (h.target - MONSTER_ENTITY_BASE) as usize;
            if slot >= store.capacity() || store.is_plant(slot as u16) {
                continue;
            }
            let seat = &mut self.seats[si];
            seat.totals.dealt += u64::from(h.amount);
            seat.buckets[b].dealt = seat.buckets[b].dealt.saturating_add(h.amount);
            seat.add_skill(skill, u64::from(h.amount));
            if self.hit_at.len() < store.capacity() {
                // Once per stage: the table grows to the horde's pool.
                self.hit_at.resize(store.capacity(), [0; SEATS]);
                self.hit_gen.resize(store.capacity(), 0);
            }
            let generation = store.generation[slot];
            if self.hit_gen[slot] != generation {
                self.hit_gen[slot] = generation;
                self.hit_at[slot] = [0; SEATS];
            }
            self.hit_at[slot][si] = tick.wrapping_add(1).max(1);
        }
    }

    /// Drops handed to the socket task (`HubEvent::Loot`): the seat's
    /// loot list.
    pub(super) fn looted(&mut self, user_id: u32, event: &super::HubEvent) {
        if let (super::HubEvent::Loot { items }, Some(seat)) = (event, self.seats.iter_mut().find(|s| s.user_id == user_id)) {
            for &(id, n) in items {
                seat.add_loot(id, n);
            }
        }
    }

    /// A revive skill of `user_id` acted this tick: a downed ally getting
    /// up now owes it to them.
    pub(super) fn revive_cast(&mut self, user_id: u32, tick: u32) {
        self.revive_cast = Some((user_id, tick));
    }

    /// Monster slot `s` died (not a plant), credited to `killer` (0 =
    /// nobody); `on_map`: user ids of the players on the stage map (they
    /// share a boss). Assists go to the others who hit it lately.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn kill(
        &mut self,
        s: u16,
        killer: u32,
        tick: u32,
        hz: f32,
        secs: f32,
        store: &HordeStorage,
        name: &str,
        elite: bool,
        on_map: impl Iterator<Item = u32>,
    ) {
        let b = self.bucket_of(secs);
        let ki = self.index(killer);
        if let Some(k) = ki {
            let seat = &mut self.seats[k];
            seat.totals.kills += 1;
            seat.buckets[b].kills += 1;
        }
        let slot = s as usize;
        if slot < self.hit_at.len() && self.hit_gen[slot] == store.generation[slot] {
            let window = (ASSIST_SECS * hz).round() as u32;
            let row = std::mem::take(&mut self.hit_at[slot]);
            for (i, &at) in row.iter().enumerate().take(self.seats.len()) {
                if at != 0 && Some(i) != ki && tick.wrapping_add(1).wrapping_sub(at) <= window {
                    self.seats[i].totals.assists += 1;
                }
            }
        }
        let boss = store.flags[slot] & FLAG_BOSS != 0;
        if boss {
            for uid in on_map {
                if let Some(seat) = self.seat_mut(uid) {
                    seat.totals.mvps += 1;
                }
            }
        }
        let seat = ki.map_or(mark::NOBODY, |k| k as u8);
        if boss {
            self.mark(secs, mark::BOSS_SLAIN, seat, name);
        } else if elite && self.elite_marks < ELITE_MARKS {
            self.elite_marks += 1;
            self.mark(secs, mark::ELITE_SLAIN, seat, name);
        }
    }

    /// A boss on the field (slot, generation, name): marked once.
    pub(super) fn boss_seen(&mut self, s: u16, generation: u8, secs: f32, name: &str) {
        if self.seen.contains(&(s, generation)) || self.seen.len() >= MARKS {
            return;
        }
        self.seen.push((s, generation));
        self.mark(secs, mark::BOSS_ARRIVED, mark::NOBODY, name);
    }

    fn mark(&mut self, secs: f32, kind: u8, seat: u8, label: &str) {
        if self.marks.len() < MARKS {
            self.marks.push(RunMark { secs: secs.max(0.0) as u32, kind, seat, label: label.into() });
        }
    }

    /// Once per combat tick, after every hit landed: damage taken by the
    /// players (the hit numbers on them), downs, outs, revives and the best
    /// combo.
    pub(super) fn observe(&mut self, tick: u32, secs: f32, hits: &[HitEvent], players: &[HubPlayer], map: super::MapIdx) {
        let b = self.bucket_of(secs);
        for h in hits {
            if h.target >= MONSTER_ENTITY_BASE || h.flags & (hit::MISS | hit::HEAL) != 0 || h.amount == 0 {
                continue;
            }
            if let Some(seat) = self.seats.iter_mut().find(|s| s.character_id == h.target) {
                seat.totals.taken += u64::from(h.amount);
                seat.buckets[b].taken = seat.buckets[b].taken.saturating_add(h.amount);
            }
        }
        let cast = self.revive_cast.take().filter(|&(_, t)| t == tick).map(|(u, _)| u);
        for p in players.iter().filter(|p| p.map == map) {
            let Some(f) = p.fighter.as_ref() else { continue };
            let Some(si) = self.index(p.profile.user_id) else { continue };
            let now = match f.state {
                Vital::Up => 0,
                Vital::Downed { .. } => 1,
                Vital::Dead => 2,
            };
            let was = std::mem::replace(&mut self.seats[si].was, now);
            let seat = &mut self.seats[si];
            seat.totals.best_combo = seat.totals.best_combo.max(f.combo().count());
            if now == was {
                continue;
            }
            let at = si as u8;
            if was == 0 {
                seat.totals.downs += 1;
                self.mark(secs, mark::DOWN, at, "");
            }
            if now == 2 {
                self.seats[si].totals.deaths += 1;
                self.mark(secs, mark::OUT, at, "");
            }
            if was == 1 && now == 0 {
                // Who got them up: the revive skill's caster, else the
                // nearest ally standing (the one filling the bar).
                let d2 = |o: &HubPlayer| (o.x - p.x).powi(2) + (o.y - p.y).powi(2);
                let helper = cast.filter(|&u| u != p.profile.user_id).or_else(|| {
                    players
                        .iter()
                        .filter(|o| o.map == map && o.profile.user_id != p.profile.user_id && o.fighter.as_ref().is_some_and(|f| f.is_up()))
                        .min_by(|a, b| d2(a).total_cmp(&d2(b)))
                        .map(|o| o.profile.user_id)
                });
                if let Some(seat) = helper.and_then(|u| self.seat_mut(u)) {
                    seat.totals.revives += 1;
                }
            }
        }
    }

    /// The `RUN_STATS` seats. `live(user_id)`: the player's level, run
    /// experience and whether they are link-dead, from their fighter
    /// (`None`: gone, numbers of their last fighter if known).
    pub(super) fn seats_msg(&self, secs: u32, book: &SkillBook, live: impl Fn(u32) -> (u32, u64, bool)) -> Vec<RunSeat> {
        let used = ((secs / self.bucket_secs) as usize + 1).min(BUCKETS);
        self.seats
            .iter()
            .map(|s| {
                let (level, exp, link_dead) = live(s.user_id);
                let flags = if s.left { seat_flags::LEFT } else { 0 } | if link_dead { seat_flags::LINK_DEAD } else { 0 };
                let t = s.totals;
                RunSeat {
                    entity_id: s.character_id,
                    name: s.name.clone(),
                    class: s.class.clone(),
                    level,
                    flags,
                    dealt: t.dealt,
                    taken: t.taken,
                    healed: t.healed,
                    kills: t.kills,
                    assists: t.assists,
                    downs: t.downs,
                    deaths: t.deaths,
                    revives: t.revives,
                    best_combo: t.best_combo,
                    mvps: t.mvps,
                    exp,
                    buckets: s.buckets[..used].to_vec(),
                    skills: s.skills().into_iter().map(|(k, d, n)| (skill_id(book, k), d, n)).collect(),
                }
            })
            .collect()
    }

    pub(super) fn marks(&self) -> &[RunMark] {
        &self.marks
    }
}

/// The wire id of a skill row.
fn skill_id(book: &SkillBook, skill: u16) -> String {
    match skill {
        SKILL_ATTACK => "attack".into(),
        SKILL_OTHER => "other".into(),
        i if (i as usize) < book.len() => book.get(i).id.clone(),
        _ => "other".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> HordeStorage {
        let mut s = HordeStorage::new(16);
        for _ in 0..4 {
            s.spawn(0, 1.0, 1.0, 10, 0);
        }
        s
    }

    fn dmg(slot: u16, amount: u32) -> HitEvent {
        HitEvent { target: MONSTER_ENTITY_BASE + slot as u32, amount, flags: 0 }
    }

    fn log() -> RunLog {
        let mut l = RunLog::new(900.0);
        l.sit(1, 101, "Ann", "mage");
        l.sit(2, 102, "Bob", "swordsman");
        l.sit(3, 103, "Cid", "acolyte");
        l
    }

    #[test]
    fn damage_goes_to_its_skill_and_bucket_and_misses_do_not_count() {
        let st = store();
        let mut l = log();
        l.credit(1, 3, &[dmg(0, 10), dmg(1, 5), HitEvent { target: MONSTER_ENTITY_BASE, amount: 0, flags: hit::MISS }], 1, 4.0, &st);
        l.credit(1, SKILL_ATTACK, &[dmg(0, 7)], 2, 15.0, &st);
        l.credit(1, 3, &[dmg(2, 1)], 3, 899.0, &st);
        let s = &l.seats()[0];
        assert_eq!(s.totals.dealt, 23);
        assert_eq!(s.skills(), vec![(3, 16, 3), (SKILL_ATTACK, 7, 1)]);
        assert_eq!((s.bucket(0).dealt, s.bucket(1).dealt, s.bucket(BUCKETS - 1).dealt), (15, 7, 1));
        assert_eq!(l.bucket_secs(), 10);
        // A heal on a player is healing.
        l.credit(3, 9, &[HitEvent { target: 101, amount: 30, flags: hit::HEAL }], 4, 20.0, &st);
        assert_eq!((l.seats()[2].totals.healed, l.seats()[2].totals.dealt), (30, 0));
    }

    #[test]
    fn long_runs_widen_the_buckets_and_skills_past_the_cap_fold() {
        assert_eq!(RunLog::new(1800.0).bucket_secs(), 20);
        assert_eq!(RunLog::new(f32::INFINITY).bucket_secs(), BUCKET_SECS);
        let st = store();
        let mut l = log();
        for k in 0..(SKILLS as u16 + 3) {
            l.credit(2, k, &[dmg(0, 1)], 1, 0.0, &st);
        }
        let rows = l.seats()[1].skills();
        assert_eq!(rows.len(), SKILLS + 1);
        assert!(rows.contains(&(SKILL_OTHER, 3, 3)), "{rows:?}");
        assert_eq!(skill_id(&SkillBook::default(), SKILL_OTHER), "other");
        assert_eq!(skill_id(&SkillBook::default(), SKILL_ATTACK), "attack");
    }

    #[test]
    fn assists_are_recent_hits_by_others_on_the_kill() {
        let st = store();
        let mut l = log();
        let hz = 20.0;
        // Ann hit slot 0 at tick 100, Cid at tick 10 (too long ago), Bob kills it at 150.
        l.credit(1, 0, &[dmg(0, 5)], 100, 5.0, &st);
        l.credit(3, 0, &[dmg(0, 5)], 10, 0.5, &st);
        l.credit(2, 0, &[dmg(0, 5)], 150, 7.5, &st);
        l.kill(0, 2, 150, hz, 7.5, &st, "Poring", false, [1, 2, 3].into_iter());
        let t: Vec<Totals> = l.seats().iter().map(|s| s.totals).collect();
        assert_eq!((t[0].assists, t[1].assists, t[2].assists), (1, 0, 0));
        assert_eq!((t[1].kills, l.seats()[1].bucket(0).kills), (1, 1));
        // Exactly the window still counts; the row is spent by the kill.
        l.credit(1, 0, &[dmg(1, 5)], 200, 10.0, &st);
        l.kill(1, 2, 200 + (ASSIST_SECS * hz) as u32, hz, 15.0, &st, "Poring", false, std::iter::empty());
        l.kill(1, 2, 300, hz, 15.0, &st, "Poring", false, std::iter::empty());
        assert_eq!(l.seats()[0].totals.assists, 2);
        // A new monster in the slot (another generation) owes nothing.
        let mut st2 = store();
        l.credit(1, 0, &[dmg(2, 5)], 400, 20.0, &st2);
        st2.generation[2] = st2.generation[2].wrapping_add(1);
        l.kill(2, 2, 401, hz, 20.0, &st2, "Poring", false, std::iter::empty());
        assert_eq!(l.seats()[0].totals.assists, 2);
    }

    #[test]
    fn a_boss_is_an_mvp_for_everyone_on_the_map_and_marks_the_timeline() {
        let mut st = store();
        st.flags[3] |= FLAG_BOSS;
        let mut l = log();
        l.boss_seen(3, st.generation[3], 600.0, "Angeling");
        l.boss_seen(3, st.generation[3], 601.0, "Angeling");
        l.kill(3, 1, 13_000, 20.0, 650.0, &st, "Angeling", false, [1, 2].into_iter());
        l.kill(0, 0, 13_001, 20.0, 650.0, &st, "Hornet", true, [1, 2].into_iter());
        let mvps: Vec<u32> = l.seats().iter().map(|s| s.totals.mvps).collect();
        assert_eq!(mvps, vec![1, 1, 0]);
        let kinds: Vec<(u8, u8, &str)> = l.marks().iter().map(|m| (m.kind, m.seat, m.label.as_str())).collect();
        assert_eq!(kinds, vec![(mark::BOSS_ARRIVED, mark::NOBODY, "Angeling"), (mark::BOSS_SLAIN, 0, "Angeling"), (mark::ELITE_SLAIN, mark::NOBODY, "Hornet")]);
        assert_eq!(l.marks()[1].secs, 650);
    }

    #[test]
    fn seats_are_kept_across_a_return_and_capped() {
        let mut l = log();
        l.leave(2);
        assert!(l.seats()[1].left);
        assert_eq!(l.sit(2, 102, "Bob", "swordsman"), Some(1));
        assert!(!l.seats()[1].left, "back in the same seat");
        for u in 10..20 {
            l.sit(u, u, "x", "mage");
        }
        assert_eq!(l.seats().len(), SEATS);
        assert_eq!(l.sit(99, 99, "late", "mage"), None);
        let seat = l.seat_mut(1).unwrap();
        for i in 0..40 {
            seat.add_loot(crate::world::combat::items::ITEMS[i % crate::world::combat::items::ITEMS.len()].id, 1);
        }
        assert!(seat.loot.len() <= LOOT_KINDS);
    }
}
