//! The horde's side of monster affixes (`combat::affixes`): rolling them
//! on the director's spawns under the density cap, their HP and pace,
//! minions and mirror copies, and the changes clients must hear about.
//!
//! Everything lives in the struct-of-arrays pool (`HordeStorage::affix`
//! and friends, sized once): a plain monster costs one zero test on the
//! paths that read them. What affixes *do* in a fight (damage taken, melee
//! riders, death blasts, the per-second tricks) is the stage's
//! (`hub/affix.rs`).

use super::storage::{AFFIX_FRENZIED, FLAG_BOSS, FLAG_MINION};
use super::Horde;
use crate::world::combat::affixes::{self, AffixFlags, AffixRules};
use crate::world::combat::{Archetype, MonsterBlock};
use crate::world::protocol::{affix_state, MonsterAffix};

/// Pace and attack-interval factors.
pub const HASTE_PACE: f32 = 1.3;
/// Attack interval factor of a Hasted monster (25 % faster).
pub const HASTE_INTERVAL: f32 = 0.8;
pub const FRENZY_PACE: f32 = 1.4;
/// Attack interval factor of a frenzied monster (40 % faster).
pub const FRENZY_INTERVAL: f32 = 1.0 / 1.4;
/// Minions (Summoner) alive at most on the field.
pub const MAX_MINIONS: u16 = 10;

/// Affix bookkeeping of one horde.
#[derive(Clone, Debug)]
pub struct AffixBook {
    pub rules: AffixRules,
    /// Live affixed monsters, bosses and minions aside (the density cap).
    pub affixed: u16,
    /// Live minions and mirror copies.
    pub minions: u16,
    /// Slots whose affix record changed since the stage last asked.
    news: Vec<u16>,
}

impl AffixBook {
    pub fn new(rules: AffixRules, pool: usize) -> Self {
        Self { rules, affixed: 0, minions: 0, news: Vec::with_capacity(pool.min(256)) }
    }
}

/// Movement factor of a monster's affixes (1 for a plain one).
#[inline]
pub fn pace(bits: u16, state: u8) -> f32 {
    if bits == 0 {
        return 1.0;
    }
    let f = AffixFlags(bits);
    let mut p = 1.0;
    if f.contains(AffixFlags::HASTED) {
        p *= HASTE_PACE;
    }
    if state & AFFIX_FRENZIED != 0 {
        p *= FRENZY_PACE;
    }
    p
}

/// Attack interval factor of a monster's affixes (1 for a plain one).
#[inline]
pub fn interval(bits: u16, state: u8) -> f32 {
    if bits == 0 {
        return 1.0;
    }
    let mut k = 1.0;
    if AffixFlags(bits).contains(AffixFlags::HASTED) {
        k *= HASTE_INTERVAL;
    }
    if state & AFFIX_FRENZIED != 0 {
        k *= FRENZY_INTERVAL;
    }
    k
}

impl Horde {
    /// The run's affix odds.
    pub fn affix_rules(&self) -> &AffixRules {
        &self.affixes.rules
    }

    /// Affixes of a live monster.
    pub fn affixes_of(&self, slot: u16) -> AffixFlags {
        AffixFlags(self.store.affix[slot as usize])
    }

    /// Whether a monster may stand at `(x, y)` on the horde's map.
    pub fn walkable(&self, x: f32, y: f32) -> bool {
        self.world.map(self.map).walk.is_walkable(x, y)
    }

    /// Live affixed monsters under the density cap (bosses and minions aside).
    pub fn affixed(&self) -> usize {
        self.affixes.affixed as usize
    }

    pub fn minions(&self) -> usize {
        self.affixes.minions as usize
    }

    /// A director spawn rolls its affixes: bosses always may, the rest
    /// while the field holds fewer affixed monsters than the cap.
    pub(super) fn roll_affixes(&mut self, slot: u16) {
        let s = slot as usize;
        let def = self.catalog.get(self.store.kind[s]);
        if def.is_plant() {
            return;
        }
        let boss = self.store.flags[s] & FLAG_BOSS != 0;
        let monsters = self.store.len() - self.plants as usize;
        if !boss && self.affixes.affixed as usize >= self.affixes.rules.cap(monsters) {
            return;
        }
        let archetype = if boss { Archetype::Boss } else { def.archetype };
        let flags = self.affixes.rules.roll(archetype, &mut self.rng);
        if !flags.is_empty() {
            self.set_affixes(slot, flags);
        }
    }

    /// Gives a live monster `flags` (tests, minions, a mirror's copy): its
    /// HP grows by the stars, a Shielded one raises its shield, and the
    /// clients hear of it.
    pub fn set_affixes(&mut self, slot: u16, flags: AffixFlags) {
        let s = slot as usize;
        if !self.store.is_alive(slot) || self.store.is_plant(slot) {
            return;
        }
        let counted = |h: &Self| h.store.affix[s] != 0 && h.store.flags[s] & (FLAG_BOSS | FLAG_MINION) == 0;
        if counted(self) {
            self.affixes.affixed -= 1;
        }
        let before = self.block(slot).hp.max(1);
        self.store.affix[s] = flags.0;
        let after = self.block(slot);
        // Same share of its HP as before (a fresh spawn: full).
        self.store.hp[s] = ((self.store.hp[s] as i64 * after.hp as i64) / before as i64).max(1) as i32;
        self.store.shield[s] = if flags.contains(AffixFlags::SHIELDED) { shield_of(&after) } else { 0 };
        if counted(self) {
            self.affixes.affixed += 1;
        }
        self.affix_changed(slot);
    }

    /// The slot's record must go out again (state change).
    pub fn affix_changed(&mut self, slot: u16) {
        if !self.affixes.news.contains(&slot) {
            self.affixes.news.push(slot);
        }
    }

    /// The records that changed since the last call, for every client on
    /// the map (slots despawned since are skipped).
    pub fn take_affix_news(&mut self, out: &mut Vec<MonsterAffix>) {
        for &slot in &self.affixes.news {
            if self.store.is_alive(slot) && self.store.affix[slot as usize] != 0 {
                out.push(self.affix_record(slot));
            }
        }
        self.affixes.news.clear();
    }

    /// Every live affixed monster (a joining client).
    pub fn affix_records(&self, out: &mut Vec<MonsterAffix>) {
        for &slot in self.store.alive() {
            if self.store.affix[slot as usize] != 0 {
                out.push(self.affix_record(slot));
            }
        }
    }

    fn affix_record(&self, slot: u16) -> MonsterAffix {
        let s = slot as usize;
        let st = &self.store;
        let mut state = st.affix_state[s] & (affix_state::FRENZIED | affix_state::SHIELD_BROKEN);
        if st.flags[s] & FLAG_MINION != 0 {
            state |= affix_state::MINION;
        }
        MonsterAffix { slot, generation: st.generation[s], bits: st.affix[s], state }
    }

    /// A minion (Summoner) or a copy (Mirror) at `(x, y)`: level `level`,
    /// `flags` affixes, paying nothing. `None` when the pool is full or the
    /// spot is a wall.
    pub fn spawn_minion(&mut self, kind: u8, x: f32, y: f32, level: u32, flags: AffixFlags) -> Option<u16> {
        if !self.world.map(self.map).walk.is_walkable(x, y) {
            return None;
        }
        let def = self.catalog.get(kind);
        if def.is_plant() || def.boss {
            return None;
        }
        let block = self.scale.apply(MonsterBlock::new(level, def.archetype, self.party));
        let slot = self.store.spawn(kind, x, y, block.hp, FLAG_MINION)?;
        let s = slot as usize;
        self.store.level[s] = level.min(u8::MAX as u32) as u8;
        self.store.cooldown[s] = slot % 8;
        self.affixes.minions += 1;
        self.stats.spawned += 1;
        // Into the query index as `Horde::spawn` does.
        if !self.index.hold {
            if self.index.fresh.len() >= super::MAX_FRESH {
                self.rebuild_index();
            } else if !self.index.fresh.contains(&slot) {
                self.index.fresh.push(slot);
            }
        }
        if !flags.is_empty() {
            self.set_affixes(slot, flags);
        }
        Some(slot)
    }

    /// The kind a Summoner calls: its own when it is a small one, else the
    /// cheapest regular monster of the roster.
    pub fn minion_kind(&self, summoner: u16) -> u8 {
        let own = self.store.kind[summoner as usize];
        if matches!(self.catalog.get(own).archetype, Archetype::Swarm | Archetype::Chaser) {
            return own;
        }
        self.catalog
            .defs()
            .iter()
            .enumerate()
            .filter(|(_, d)| !d.boss && !d.is_plant() && d.archetype != Archetype::Elite)
            .min_by_key(|(_, d)| d.spawn_cost)
            .map_or(own, |(k, _)| k as u8)
    }

    /// Bookkeeping of a slot about to be freed.
    pub(super) fn affix_despawn(&mut self, slot: u16) {
        let s = slot as usize;
        if !self.store.is_alive(slot) {
            return;
        }
        let flags = self.store.flags[s];
        if flags & FLAG_MINION != 0 {
            self.affixes.minions = self.affixes.minions.saturating_sub(1);
        } else if self.store.affix[s] != 0 && flags & FLAG_BOSS == 0 {
            self.affixes.affixed = self.affixes.affixed.saturating_sub(1);
        }
    }

    /// The block's HP grown by the monster's stars.
    pub(super) fn affix_block(&self, slot: u16, mut b: MonsterBlock) -> MonsterBlock {
        let s = slot as usize;
        let bits = self.store.affix[s];
        if bits != 0 {
            let pct = affixes::hp_pct(AffixFlags(bits), self.store.flags[s] & FLAG_BOSS != 0);
            b.hp = ((b.hp as i64 * pct as i64 + 50) / 100).max(1) as i32;
        }
        b
    }
}

/// A Shielded monster's full shield: 30 % of its HP.
pub fn shield_of(b: &MonsterBlock) -> i32 {
    (b.hp * 3 / 10).max(1)
}

#[cfg(test)]
mod tests {
    use super::super::catalog::{test_def, MonsterCatalog};
        use super::super::{Horde, HordeConfig};
    use super::*;
    use crate::world::hub::test_support::map;
    use crate::world::hub::HubWorld;
    use crate::world::map::{CameraConstraint, LocationKind};
    use crate::world::modifiers::{ModFlags, Modifiers, MAX_TIER};
    use std::sync::Arc;

    fn world(n: usize) -> Arc<HubWorld> {
        let rows: Vec<String> = (0..n)
            .map(|r| (0..n).map(|c| if r == 0 || c == 0 || r == n - 1 || c == n - 1 { '#' } else { '.' }).collect())
            .collect();
        let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
        let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
        Arc::new(HubWorld::new(vec![map("field", LocationKind::Field, camera, &refs, vec![])], "field").unwrap())
    }

    fn horde(mods: Modifiers, director: bool) -> Horde {
        let mut tank = test_def("tank", 0.5, 2, 0.0, false);
        tank.archetype = Archetype::Tank;
        tank.pack = (2, 4);
        let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 0.5, 1, 0.0, false), tank]).unwrap());
        let mut cfg = HordeConfig { light_radius: 20.0, director_enabled: director, seed: Some(9), ..HordeConfig::default() };
        cfg.waves.budget = 400.0;
        Horde::new(world(160), catalog, cfg.with_modifiers(mods))
    }

    /// The director's spawns roll under the density cap: some monsters of
    /// a hard run carry stars, never more than the cap allows, their HP
    /// grown by the stars; every one of them is news for the clients once.
    #[test]
    fn director_spawns_roll_affixes_under_the_density_cap() {
        let mut h = horde(Modifiers { tier: MAX_TIER, flags: ModFlags::SWARM }, true);
        let player = [(80.5, 80.5)];
        let mut news = Vec::new();
        let mut heard = std::collections::HashSet::new();
        for t in 0..20 * 90 {
            h.tick(t, 20, &player);
            let alive = h.len();
            assert!(h.affixed() <= h.affix_rules().cap(alive), "{} affixed of {alive}", h.affixed());
            news.clear();
            h.take_affix_news(&mut news);
            for r in &news {
                assert!(heard.insert((r.slot, r.generation)), "each spawn is news once");
                assert_ne!(r.bits, 0);
            }
        }
        let starred: Vec<u16> = h.store().alive().iter().copied().filter(|&s| h.store().affix[s as usize] != 0).collect();
        assert!(!starred.is_empty(), "a hard run rolls some affixes ({} alive)", h.len());
        assert!(starred.len() * 4 < h.len(), "but a crowd is not all rares: {} of {}", starred.len(), h.len());
        assert_eq!(heard.len(), starred.len());
        for &s in &starred {
            let full = h.block(s).hp;
            let plain = h.scale.apply(MonsterBlock::new(h.store().level[s as usize] as u32, h.catalog().get(h.store().kind[s as usize]).archetype, 1)).hp;
            assert_eq!(h.store().hp[s as usize], full);
            assert!(full > plain, "stars add HP");
        }
        assert_eq!(h.minions(), 0);
        // Freeing them gives the cap back.
        for s in starred {
            h.despawn(s);
        }
        assert_eq!(h.affixed(), 0);
    }

    #[test]
    fn hand_spawns_stay_plain_and_set_affixes_keeps_the_hp_share() {
        let mut h = horde(Modifiers::NONE, false);
        let s = h.spawn(1, 40.5, 40.5).unwrap();
        assert!(h.affixes_of(s).is_empty());
        let plain = h.block(s).hp;
        h.store_mut().hp[s as usize] = plain / 2;
        h.set_affixes(s, AffixFlags(AffixFlags::SHIELDED.0 | AffixFlags::HASTED.0));
        let full = h.block(s).hp;
        assert_eq!(full, (plain as i64 * 140 / 100) as i32);
        assert!((h.store().hp[s as usize] - full / 2).abs() <= 1, "half HP stays half");
        assert_eq!(h.store().shield[s as usize], shield_of(&h.block(s)));
        assert_eq!((h.affixed(), pace(h.store().affix[s as usize], 0)), (1, HASTE_PACE));
        assert_eq!(pace(h.store().affix[s as usize], AFFIX_FRENZIED), HASTE_PACE * FRENZY_PACE);
        assert_eq!(pace(0, AFFIX_FRENZIED), 1.0, "plain monsters are never paced");
        assert_eq!(interval(AffixFlags::HASTED.0, 0), HASTE_INTERVAL);
        // Minions: flagged, counted apart, never under the cap.
        let m = h.spawn_minion(0, 41.5, 40.5, 5, AffixFlags::NONE).unwrap();
        assert!(h.store().flags[m as usize] & FLAG_MINION != 0);
        assert_eq!((h.minions(), h.affixed(), h.store().level[m as usize]), (1, 1, 5));
        h.despawn(m);
        assert_eq!(h.minions(), 0);
        assert_eq!(h.minion_kind(s), 0, "a tank calls the roster's cheapest");
    }
}
