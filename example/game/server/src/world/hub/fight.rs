//! Stage combat: basic attacks, monster melee, kills and experience,
//! downed / revive / defeat (COMBAT.md §4). Rules come from
//! [`crate::world::combat`]; this file only applies them to the instance's
//! players and horde once per tick.

use super::aim::aimed_plant;
use super::area::Area;
use super::cast::{envenom, push, ticks16, wound, Casting, Kill, VENOM_SECS};
use super::{facing_of, HubEvent, HubInstance, HubPlayer, MapIdx, Outgoing};
use crate::world::combat::skills::SLOTS;
use crate::world::combat::rules::{attack_roll, AttackOutcome};
use crate::world::combat::items::{self, Bag, Gear};
use crate::world::combat::loot;
use crate::world::combat::{
    self, cantrip_k, exp_to_next, kill_value, share_kill, Archetype, Class, Combo, Condition, Damage, DamageType, Dice, Mods, Reach, Save,
    Status, Weapon, CONDITIONS,
};
use crate::world::horde::storage::{FLAG_BOSS, FLAG_FULL};
use crate::world::protocol::{hit, HitEvent, ServerMsg, Swing, MONSTER_ENTITY_BASE};
use crate::world::record::Record;
use crate::world::modifiers::ModFlags;

/// Standing still this many ticks (0.15 s at 20 Hz) starts the basic attack:
/// releasing the stick between steps must not trigger a swing.
const STILL_TICKS: u32 = 3;
/// Monsters reach this far beyond touching (cells): they stop at contact
/// (`HordeConfig::player_radius`), so a blow needs a body against the player.
const MONSTER_REACH: f32 = 0.2;
/// Player body radius as the horde uses it (cells).
const PLAYER_RADIUS: f32 = 0.4;
/// An ally standing this close revives a downed player (cells; 1.5 m).
const REVIVE_RANGE: f32 = 3.0;
const REVIVE_SECS: f32 = 3.0;
const DOWNED_SECS: f32 = 20.0;
/// No damage dealt or taken for this long: HP regenerates.
const QUIET_SECS: f32 = 5.0;
/// Progress messages at most this often (ticks), kills are summed.
const PROGRESS_EVERY: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vital {
    Up,
    /// Out of the fight; an ally can revive before `ticks` run out.
    Downed { ticks: u32 },
    /// Back at the next wave.
    Dead,
}

impl Vital {
    fn code(self) -> u8 {
        match self {
            Vital::Up => 0,
            Vital::Downed { .. } => 1,
            Vital::Dead => 2,
        }
    }
}

/// A buff running on a player.
#[derive(Clone, Copy, Debug)]
struct Buff {
    /// The skill that gave it (a recast refreshes it).
    skill: u16,
    until: u32,
    mods: Mods,
    /// It granted temporary HP (gone with the last such buff).
    temp: bool,
}

/// A player's combat state inside a stage.
#[derive(Clone, Debug)]
pub struct Fighter {
    pub class: Class,
    /// What the basic attack swings (die, reach, speed).
    pub weapon: &'static Weapon,
    pub level: u32,
    /// Experience into the current level.
    pub exp: u64,
    pub hp: i32,
    pub max_hp: i32,
    pub sp: i32,
    pub max_sp: i32,
    pub ac: i32,
    pub state: Vital,
    /// Learned level per skill of the instance's book (0 = not learned).
    pub skills: Vec<u8>,
    /// Skill (book index) per hotbar slot.
    pub hotbar: [Option<u16>; SLOTS],
    /// The travel skill on the travel key (Space on missions): the learned
    /// movement skill (Back Slide, Blink, Charge, ...), if any.
    pub travel: Option<u16>,
    /// Ticks left per skill.
    pub(super) cooldowns: Vec<u32>,
    pub(super) casting: Option<Casting>,
    /// Ground that lasts while a slot is held.
    pub(super) channel: Option<super::cast::Channel>,
    /// Rooted after a cast; the basic attack waits.
    pub(super) recovery: u32,
    /// Riding a Peco Peco (`SkillBook::rides`, set with the passives):
    /// riding skills need it.
    pub(super) riding: bool,
    /// Modifiers of the learned passives.
    passive: Mods,
    /// Running buffs.
    buffs: Vec<Buff>,
    /// Passives plus buffs, summed once per tick.
    pub(super) mods: Mods,
    /// Temporary HP, spent before HP (Endure, Kyrie Eleison).
    pub temp_hp: i32,
    /// Ticks left per condition (monster skills), and the active set.
    cond: [u16; CONDITIONS],
    pub(super) status: Status,
    /// Where the last fear came from (cells): a feared fighter flees it.
    pub(super) fear_from: (f32, f32),
    /// Kills of the party and experience gained since the stage began
    /// (the result screen).
    pub(super) run_exp: u64,
    /// Signature of the buff set last sent (`BUFFS`).
    pub(super) buffs_sent: u64,
    /// Level changed: skill points to resend.
    pub(super) skills_dirty: bool,
    attack_cd: u32,
    still: u32,
    pub(super) quiet: u32,
    /// Ticks of ally help toward a revive (drains back while nobody helps).
    revive: u32,
    /// Revive progress last sent (`REVIVE`, percent), while downed.
    revive_sent: Option<u8>,
    regen_hp: f32,
    regen_sp: f32,
    /// Last vitals sent (hp, max_hp, sp, max_sp, state, status, temp HP,
    /// light).
    sent: Option<[i32; 8]>,
    gained: u64,
    progress_dirty: bool,
    progress_wait: u32,
    /// What the worn items add (armour class before the tier, HP, SP, saves).
    pub gear: Gear,
    /// Potions carried into the stage (id, count); drinking reports back.
    pub(super) potions: Vec<(String, u32)>,
    /// Tick of the next potion.
    pub(super) potion_ready: u32,
    /// Potion regeneration running: HP, SP (one each, never stacked).
    pub(super) regen: [Option<super::shop::Regen>; 2],
    /// Zeny earned and not yet reported to the socket task.
    pub(super) zeny: u64,
    /// Items dropped for this player and not yet reported (merged stacks;
    /// handed over whole with the next progress report).
    pub(super) loot: Vec<(&'static str, u32)>,
    /// Kill combo and style rank (read through [`Fighter::combo`]).
    pub(super) combo: Combo,
    /// Net seconds spent in the overtime poison fog (`fog.rs`), and the
    /// value last sent (`FOG` exposure).
    pub(super) fog: u8,
    pub(super) fog_sent: u8,
    /// The fog killed them: dead for the rest of the run (no comeback with
    /// the next wave).
    pub(super) fog_dead: bool,
    /// Lifetime counters gained and not yet reported (`world::record`).
    pub(super) record: Record,
    /// The run's curses (`world::modifiers`): set on arrival in a stage.
    pub(super) curses: ModFlags,
    /// Zeny kills paid this player this run (the modifiers' clear bonus
    /// is a share of it).
    pub(super) run_zeny: u64,
}

impl Fighter {
    /// A fighter in the class's starting kit holding `weapon` (see
    /// [`Fighter::wear`] for a character's own equipment).
    pub fn new(class: Class, level: u32, exp: u64, weapon: &str) -> Self {
        let level = level.clamp(1, combat::MAX_LEVEL);
        let gear = Bag::starting(class, weapon).gear(class);
        let (max_hp, max_sp) = (class.max_hp(level) + gear.hp, class.max_sp(level) + gear.sp);
        Self {
            class,
            weapon: Weapon::held(class, weapon),
            level,
            exp,
            hp: max_hp,
            max_hp,
            sp: max_sp,
            max_sp,
            ac: gear.ac_at(level),
            state: Vital::Up,
            skills: Vec::new(),
            hotbar: [None; SLOTS],
            travel: None,
            cooldowns: Vec::new(),
            casting: None,
            channel: None,
            recovery: 0,
            riding: false,
            passive: Mods::default(),
            buffs: Vec::new(),
            mods: Mods::default(),
            temp_hp: 0,
            cond: [0; CONDITIONS],
            status: Status(0),
            fear_from: (0.0, 0.0),
            run_exp: 0,
            buffs_sent: 0,
            skills_dirty: false,
            attack_cd: 0,
            still: 0,
            quiet: 0,
            revive: 0,
            revive_sent: None,
            regen_hp: 0.0,
            regen_sp: 0.0,
            sent: None,
            gained: 0,
            progress_dirty: false,
            progress_wait: 0,
            gear,
            potions: Vec::new(),
            potion_ready: 0,
            regen: [None, None],
            zeny: 0,
            loot: Vec::new(),
            combo: Combo::default(),
            fog: 0,
            fog_sent: 0,
            fog_dead: false,
            record: Record::default(),
            curses: ModFlags::NONE,
            run_zeny: 0,
        }
    }

    /// The kill combo: count and style rank, for skills that scale with it
    /// (COMBAT.md §5.4).
    pub fn combo(&self) -> &Combo {
        &self.combo
    }

    /// The combo as the client sees it.
    pub(super) fn combo_msg(&self, hz: f32) -> ServerMsg {
        let (left, window) = self.combo.timer();
        let ms = |t: u32| (t as f32 * 1000.0 / hz).round() as u32;
        ServerMsg::Combo { count: self.combo.count(), rank: self.combo.rank().code(), window_ms: ms(window), left_ms: ms(left) }
    }

    /// Puts on a character's equipment (weapon, armour, bonuses) and packs
    /// its potions; HP and SP stay full if they were.
    pub fn wear(&mut self, bag: &Bag) {
        let full = (self.hp >= self.max_hp, self.sp >= self.max_sp);
        self.weapon = Weapon::held(self.class, bag.weapon());
        self.gear = bag.gear(self.class);
        self.rescale();
        if full.0 {
            self.hp = self.max_hp;
        }
        if full.1 {
            self.sp = self.max_sp;
        }
        self.potions = bag
            .items
            .iter()
            .filter(|s| matches!(items::find(&s.id).map(|e| e.kind), Some(items::Kind::Consumable(_))))
            .map(|s| (s.id.clone(), s.count))
            .collect();
    }

    /// The lifetime counters, to credit: reported with the next progress.
    pub(super) fn tally(&mut self) -> &mut Record {
        self.progress_dirty = true;
        &mut self.record
    }

    /// The counters gained since the last report, if any.
    pub(super) fn take_record(&mut self) -> Option<HubEvent> {
        (!self.record.is_empty()).then(|| HubEvent::Record(std::mem::take(&mut self.record)))
    }

    /// Zeny for this player (a pickup bonus): reported with the next progress.
    pub(super) fn add_zeny(&mut self, n: u64) {
        self.zeny += n;
        self.progress_dirty = true;
    }

    /// A drop for this player: kept until the next report.
    pub(super) fn add_loot(&mut self, item: &'static str, n: u32) {
        match self.loot.iter_mut().find(|(id, _)| *id == item) {
            Some(l) => l.1 = l.1.saturating_add(n),
            None => self.loot.push((item, n)),
        }
        self.progress_dirty = true;
    }

    /// The drops since the last report, for the socket task to put in the
    /// bag; herbs and food join the potions carried (drinkable at once).
    pub(super) fn take_loot(&mut self) -> Option<HubEvent> {
        if self.loot.is_empty() {
            return None;
        }
        for &(id, n) in &self.loot {
            if !matches!(items::find(id).map(|e| e.kind), Some(items::Kind::Consumable(_))) {
                continue;
            }
            match self.potions.iter_mut().find(|(p, _)| p == id) {
                Some(p) => p.1 += n,
                None => self.potions.push((id.into(), n)),
            }
        }
        Some(HubEvent::Loot { items: std::mem::take(&mut self.loot) })
    }

    /// Pools and armour class of the level and gear.
    fn rescale(&mut self) {
        self.max_hp = self.class.max_hp(self.level) + self.gear.hp;
        self.max_sp = self.class.max_sp(self.level) + self.gear.sp;
        self.ac = self.gear.ac_at(self.level);
        self.hp = self.hp.min(self.max_hp);
        self.sp = self.sp.min(self.max_sp);
    }

    pub fn is_up(&self) -> bool {
        self.state == Vital::Up
    }

    /// Standing and not playing dead: monsters come for it.
    pub fn hunted(&self) -> bool {
        self.is_up() && !self.mods.hidden
    }

    pub(super) fn set_passive(&mut self, passive: Mods) {
        self.passive = passive;
        self.sum_mods();
    }

    fn sum_mods(&mut self) {
        let mut m = self.passive;
        for b in &self.buffs {
            m.add(&b.mods);
        }
        self.mods = m;
    }

    /// Drops buffs that ran out (at tick `now`) and sums the rest.
    pub(super) fn refresh(&mut self, now: u32) {
        let n = self.buffs.len();
        self.buffs.retain(|b| b.until > now);
        if self.buffs.len() != n || self.buffs.is_empty() && self.mods != self.passive {
            self.sum_mods();
        }
        if self.temp_hp > 0 && !self.buffs.iter().any(|b| b.temp) {
            self.temp_hp = 0;
        }
    }

    /// Ticks left on the buff `skill` gave (0 = none), as of tick `now`.
    pub(super) fn buff_left(&self, skill: u16, now: u32) -> u32 {
        self.buffs.iter().find(|b| b.skill == skill).map_or(0, |b| b.until.saturating_sub(now))
    }

    /// Starts (or refreshes) the buff of `skill` until tick `until`, with
    /// `temp_hp` temporary HP (the larger pool is kept).
    pub(super) fn buff(&mut self, skill: u16, until: u32, mods: Mods, temp_hp: i32) {
        let temp = temp_hp > 0;
        match self.buffs.iter_mut().find(|b| b.skill == skill) {
            Some(b) => {
                b.until = b.until.max(until);
                b.mods = mods;
                b.temp |= temp;
            }
            None => self.buffs.push(Buff { skill, until, mods, temp }),
        }
        self.temp_hp = self.temp_hp.max(temp_hp);
        self.sum_mods();
    }

    /// Moving ends playing dead.
    pub(super) fn unhide(&mut self) {
        if self.mods.hidden {
            self.buffs.retain(|b| !b.mods.hidden);
            self.sum_mods();
        }
    }

    /// A damaging spell went off: Mystical Amplification is spent.
    pub(super) fn spend_amplify(&mut self) {
        if self.mods.amplify > 0.0 {
            self.buffs.retain(|b| b.mods.amplify <= 0.0);
            self.sum_mods();
        }
    }

    /// Leaves condition `c` for `ticks` (the longer of two is kept). Endure
    /// keeps the player on their feet.
    pub(super) fn afflict(&mut self, c: Condition, ticks: u16) {
        self.afflict_inner(c, ticks);
    }

    /// Terrified by something at `from` (cells) for `ticks`: it flees from
    /// there, and can neither attack nor cast, until the fear wears off.
    pub(super) fn frighten(&mut self, from: (f32, f32), ticks: u16) {
        self.fear_from = from;
        self.afflict_inner(Condition::Feared, ticks);
    }

    fn afflict_inner(&mut self, c: Condition, ticks: u16) {
        if self.mods.steady && matches!(c, Condition::Stunned | Condition::Frozen) {
            return;
        }
        let t = &mut self.cond[c as usize];
        *t = (*t).max(ticks);
        self.status.0 |= c.bit();
    }

    /// One tick of the condition timers.
    fn wear_off(&mut self) {
        let mut bits = 0;
        for (i, t) in self.cond.iter_mut().enumerate() {
            *t = t.saturating_sub(1);
            if *t > 0 {
                bits |= 1 << i;
            }
        }
        self.status = Status(bits);
    }

    /// Ends condition `c` (a Green Potion).
    pub(super) fn cure(&mut self, c: Condition) {
        self.cond[c as usize] = 0;
        self.status.0 &= !c.bit();
    }

    pub(super) fn clear_conditions(&mut self) {
        self.cond = [0; CONDITIONS];
        self.status = Status(0);
    }

    /// Armour class with buffs and conditions.
    pub(super) fn armour(&self) -> i32 {
        self.ac + self.mods.ac + self.status.ac()
    }

    /// Saving throw bonus against `c` (or a plain Reflex save).
    pub(super) fn save_against(&self, c: Option<Condition>) -> i32 {
        let save = c.map_or(Save::Reflex, Condition::save);
        self.class.save_bonus(self.level, save) + self.gear.saves[save as usize]
    }

    /// Takes a hit: resistance and armour, then temporary HP, then HP.
    /// Returns what came off HP and temporary HP together.
    pub(super) fn take(&mut self, amount: i32, physical: bool) -> i32 {
        let a = self.mods.taken(amount, physical);
        let soak = a.min(self.temp_hp);
        self.temp_hp -= soak;
        let mut rest = a - soak;
        if self.mods.mana_shield && rest > 0 {
            // Energy Coat: 1 SP stops 2 HP.
            let stopped = rest.min(self.sp.max(0) * 2);
            self.sp -= (stopped + 1) / 2;
            rest -= stopped;
        }
        // Glass Bones: whatever reaches HP takes all of it.
        if rest > 0 && self.curses.intersects(ModFlags::ONE_HIT) {
            rest = rest.max(self.hp);
        }
        self.hp -= rest;
        a
    }

    /// A hit from a monster: undead and demons hurt less with Divine
    /// Protection.
    pub(super) fn take_from(&mut self, amount: i32, physical: bool, evil: bool) -> i32 {
        let amount = if evil && self.mods.evil_resist > 0.0 {
            (amount as f32 * (1.0 - self.mods.evil_resist.min(0.9))).round() as i32
        } else {
            amount
        };
        self.take(amount, physical)
    }

    /// Turns a toggled buff of `skill` on (until cast again) or off.
    pub(super) fn toggle(&mut self, skill: u16, mods: Mods) {
        if let Some(i) = self.buffs.iter().position(|b| b.skill == skill) {
            self.buffs.swap_remove(i);
        } else {
            self.buffs.push(Buff { skill, until: u32::MAX, mods, temp: false });
        }
        self.sum_mods();
    }

    /// Changes when a buff starts, is refreshed or ends.
    pub(super) fn buff_signature(&self) -> u64 {
        self.buffs.iter().fold(self.buffs.len() as u64, |h, b| {
            h.rotate_left(13) ^ (u64::from(b.skill) << 32 | u64::from(b.until)).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        })
    }

    /// Running buffs as (skill, ticks left), for the HUD.
    pub(super) fn buff_list(&self, now: u32) -> impl Iterator<Item = (u16, u32)> + '_ {
        self.buffs.iter().map(move |b| (b.skill, b.until.saturating_sub(now)))
    }

    /// Downed at 0 HP (conditions end).
    pub(super) fn fall_if_spent(&mut self, hz: f32) {
        if self.hp <= 0 && self.is_up() {
            self.hp = 0;
            self.tally().deaths += 1;
            self.state = Vital::Downed { ticks: (DOWNED_SECS * hz) as u32 };
            self.revive = 0;
            self.clear_conditions();
            self.regen = [None, None];
        }
    }

    /// Killed outright (the overtime fog): dead for the rest of the run,
    /// whatever was standing or bleeding out.
    pub(super) fn perish(&mut self, hz: f32) {
        self.hp = 0;
        self.fall_if_spent(hz);
        self.state = Vital::Dead;
        self.revive = 0;
        self.fog_dead = true;
    }

    fn vitals(&self, entity_id: u32) -> ServerMsg {
        ServerMsg::PlayerVitals {
            entity_id,
            hp: self.hp.max(0) as u32,
            max_hp: self.max_hp as u32,
            sp: self.sp.max(0) as u32,
            max_sp: self.max_sp as u32,
            state: self.state.code(),
            status: self.status.0 as u32,
            temp_hp: self.temp_hp.max(0) as u32,
            light: (self.mods.light.max(0.0).round() as u32).min(255),
            level: self.level,
        }
    }

    /// Revive progress in percent (5 % steps) and the bleed-out left.
    fn revive_state(&self, entity_id: u32, hz: f32) -> ServerMsg {
        let (progress, left_ms) = match self.state {
            Vital::Downed { ticks } => {
                let pct = (self.revive as f32 * 100.0 / (REVIVE_SECS * hz).max(1.0)) as u8;
                (pct.min(100) / 5 * 5, (ticks as f32 * 1000.0 / hz.max(1.0)) as u32)
            }
            _ => (0, 0),
        };
        ServerMsg::Revive { entity_id, progress, left_ms }
    }

    /// `REVIVE` when it changed: on going down, every 5 % of an ally's
    /// revive, and a last one (0, 0) on getting up or bleeding out.
    fn revive_msg(&mut self, entity_id: u32, hz: f32) -> Option<ServerMsg> {
        let msg = self.revive_state(entity_id, hz);
        let ServerMsg::Revive { progress, .. } = msg else { return None };
        let now = matches!(self.state, Vital::Downed { .. }).then_some(progress);
        if now == self.revive_sent {
            return None;
        }
        self.revive_sent = now;
        Some(msg)
    }

    /// The fighter a character arrives with: everything permanent derived
    /// again from what the character is (class, level, experience, worn
    /// equipment with its bonuses, learned passives, the hotbar and the
    /// potions in the bag), at full HP and SP, and nothing transient (no
    /// buff, toggle, condition, temporary HP, potion regeneration, cast or
    /// cooldown). Permanent effects only ever come from the profile, so
    /// re-deriving them is the whole "cleanse": a future mount (Peco Peco),
    /// cart or falcon belongs here too, read from the profile, never
    /// carried over from an old fighter. See [`HubInstance::settle_on_arrival`].
    pub(super) fn arrive(profile: &super::PlayerProfile, book: &combat::SkillBook) -> Fighter {
        let class = Class::from_id(&profile.archetype).unwrap_or(Class::Swordsman);
        let mut f = Fighter::new(class, profile.level, profile.exp, &profile.weapon);
        if !profile.bag.weapon().is_empty() {
            f.wear(&profile.bag);
        }
        super::cast::equip(&mut f, book, &profile.loadout);
        f
    }

    /// A player back in the same stage (a reconnect, a second session):
    /// what they had spent stays spent (HP, SP, downed or dead, skill and
    /// potion cooldowns, potions drunk, the run's experience), while every
    /// timed effect is gone (buffs, toggles, temporary HP, conditions,
    /// potion regeneration, casts, ground held), as on a fresh fighter.
    pub(super) fn resume(&mut self, old: &Fighter) {
        self.hp = old.hp.min(self.max_hp);
        self.sp = old.sp.min(self.max_sp);
        self.state = old.state;
        if self.cooldowns.len() == old.cooldowns.len() {
            for (c, o) in self.cooldowns.iter_mut().zip(&old.cooldowns) {
                *c = (*c).max(*o);
            }
        }
        self.potion_ready = self.potion_ready.max(old.potion_ready);
        for (id, n) in &mut self.potions {
            if let Some((_, left)) = old.potions.iter().find(|(o, _)| o == id) {
                *n = (*n).min(*left);
            }
        }
        self.run_exp = old.run_exp;
        self.run_zeny = old.run_zeny;
        self.attack_cd = old.attack_cd;
        // The fog's toll is not a timed effect: a relog keeps it.
        self.fog = old.fog;
        self.fog_dead = old.fog_dead;
    }

    /// The clear bonus of mission modifiers: experience (levelling as kills
    /// do), reported at once since the run is over.
    pub(super) fn grant_exp(&mut self, exp: u64) -> [HubEvent; 2] {
        self.gain(exp);
        [HubEvent::Control(self.progress()), HubEvent::Progress { level: self.level, exp: self.exp }]
    }

    fn progress(&self) -> ServerMsg {
        ServerMsg::Progress { level: self.level, exp: self.exp, exp_to_next: exp_to_next(self.level), gained: self.gained }
    }

    /// Adds experience; levels up as many times as it pays for (full heal
    /// on a level-up, as in RO).
    fn gain(&mut self, exp: u64) {
        if exp == 0 {
            return;
        }
        self.run_exp += exp;
        self.exp += exp;
        self.gained += exp;
        self.progress_dirty = true;
        while self.level < combat::MAX_LEVEL && self.exp >= exp_to_next(self.level) {
            self.exp -= exp_to_next(self.level);
            self.level += 1;
            self.rescale();
            if self.is_up() {
                self.hp = self.max_hp;
                self.sp = self.max_sp;
            }
            self.progress_wait = 0;
            self.skills_dirty = true;
        }
        if self.level >= combat::MAX_LEVEL {
            self.exp = 0;
        }
    }
}

impl HubInstance {
    /// The one place a character's statuses are settled on arrival
    /// (COMBAT.md §4.6, "Arriving"): entering the game, reaching a
    /// location, and the trip back to town when a mission ends (victory or
    /// defeat) all join an instance through here.
    ///
    /// - Town and other hubs: `None`. Nothing transient survives (buffs,
    ///   toggles, conditions, temporary HP, downed or dead, cooldowns,
    ///   potion regeneration); the character is at full HP and SP, and the
    ///   permanent numbers (equipment, level) are derived on read
    ///   (`class_stats`).
    /// - A mission: a fresh [`Fighter::arrive`] (permanent effects from the
    ///   profile, full HP and SP), except for a player back in the same
    ///   running stage, who keeps what they had spent ([`Fighter::resume`]).
    pub(super) fn settle_on_arrival(&mut self, user_id: u32, profile: &super::PlayerProfile) -> Option<Fighter> {
        let curses = self.horde.as_ref()?.modifiers().flags;
        let mut f = Fighter::arrive(profile, &self.skills);
        f.curses = curses;
        if let Some(i) = self.departed.iter().position(|(u, _)| *u == user_id) {
            let (_, old) = self.departed.swap_remove(i);
            f.resume(&old);
        }
        Some(f)
    }
}

/// Who picks up a kill's drops: the player who landed the blow when they
/// are on the map, else the nearest fighter there.
pub(super) fn looter(players: &[HubPlayer], map: MapIdx, killer: u32, x: f32, y: f32) -> Option<usize> {
    let here = |p: &HubPlayer| p.map == map && p.fighter.is_some();
    if killer != 0
        && let Some(i) = players.iter().position(|p| p.profile.user_id == killer && here(p))
    {
        return Some(i);
    }
    let d2 = |p: &HubPlayer| (p.x - x) * (p.x - x) + (p.y - y) * (p.y - y);
    players.iter().enumerate().filter(|(_, p)| here(p)).min_by(|a, b| d2(a.1).total_cmp(&d2(b.1))).map(|(i, _)| i)
}

/// Cells around a levelling player that monsters are thrown out of, and how far.
const LEVEL_UP_RADIUS: f32 = 5.0;
const LEVEL_UP_PUSH: f32 = 3.5;

pub(super) fn damage_type_bits(kind: DamageType) -> u8 {
    (kind as u8) << 4
}

impl HubInstance {
    /// One combat step on the horde's map, after movement and the horde.
    pub(super) fn fight(&mut self, out: &mut Outgoing) {
        if self.horde.is_none() {
            return;
        }
        let mut swings: Vec<Swing> = Vec::new();
        let mut hits: Vec<HitEvent> = Vec::new();
        let mut kills: Vec<Kill> = Vec::new();
        // 0. Skills: presses, wind-ups and due hits.
        self.cast_step(out, &mut hits, &mut kills);
        // Pickup bonuses: spawns, pickups and their effects (bonus.rs).
        self.bonus_step(out, &mut hits);
        // The overtime poison fog: rising, closing in, its toll (fog.rs).
        self.fog_step(out, &mut hits);
        // The mission's environment: events, light, hazards (env.rs).
        self.env_step(out, &mut hits);
        // Sograt Desert's sand-blast, hodes and boss (sograt.rs).
        self.desert_step(out, &mut hits);
        let Some(horde) = self.horde.as_mut() else { return };
        let map = horde.map();
        let hz = self.config.tick_hz as f32;
        let rng = &mut self.dice;
        // The run clock and tick the result screen's numbers are kept at.
        let (tick, secs) = (self.tick, horde.director_elapsed());

        // 1. Players: the basic attack while standing. It is an area too: a
        //    cone in front for melee, a splash at the impact for ranged; the
        //    target is the nearest monster in reach (index queries only).
        let mut targets: Vec<u16> = Vec::new();
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let Some(f) = p.fighter.as_mut() else { continue };
            f.attack_cd = f.attack_cd.saturating_sub(1);
            // Link-dead players do not fight back (they only take hits).
            // Hiding stops the attack, but for a thief's: it strikes out of it.
            if !f.is_up() || p.dir != (0.0, 0.0) || (f.mods.hidden && !f.mods.ambush) || f.status.helpless() || p.link_dead {
                f.still = 0;
                continue;
            }
            if f.casting.is_some() || f.channel.is_some() || f.recovery > 0 {
                continue;
            }
            f.still += 1;
            if f.still < STILL_TICKS || f.attack_cd > 0 {
                continue;
            }
            let weapon = f.weapon;
            let mut m = f.mods;
            // Katars crit a step earlier.
            m.crit += weapon.kind.crit();
            let (dice, kind) = weapon.damage(f.class, f.level);
            let damage = Damage::new(Dice { bonus: dice.bonus + m.damage_flat, ..dice }, kind);
            let (range, arc_cos, splash) = match weapon.reach {
                Reach::Melee { range, arc_cos } => (range, Some(arc_cos), 0.0),
                Reach::Ranged { range, splash } => (range, None, splash),
            };
            // Monsters first; a plant only when the player means it (aim.rs).
            let Some(target) = horde.nearest_foe(p.x, p.y, range).map(|(s, _)| s).or_else(|| aimed_plant(horde, p.x, p.y, p.aim, range)) else {
                continue;
            };
            let store = horde.store();
            let (tx, ty) = (store.x[target as usize], store.y[target as usize]);
            let (ax, ay) = (tx - p.x, ty - p.y);
            let alen = (ax * ax + ay * ay).sqrt().max(1e-3);
            let area = match arc_cos {
                Some(arc_cos) => Area::Arc { x: p.x, y: p.y, dir: (ax / alen, ay / alen), r: range, arc_cos },
                None => Area::Circle { x: tx, y: ty, r: splash },
            };
            targets.clear();
            area.monsters(horde, &mut targets);
            if !targets.contains(&target) {
                targets.push(target);
            }
            // Double Attack: now and then one more swing on every target.
            let attacks = f.class.attacks_per_action(f.level) + u32::from(m.double > 0 && rng.die(100) <= m.double);
            // Out of hiding: the swing ends it, with advantage.
            let ambush = m.lurking();
            if ambush {
                f.unhide();
            }
            let venom = ticks16(VENOM_SECS, hz);
            let entity = p.profile.character_id;
            let uid = p.profile.user_id;
            let k = cantrip_k(f.level);
            let h0 = hits.len();
            for &s in &targets {
                let block = horde.block(s);
                for _ in 0..attacks {
                    let id = MONSTER_ENTITY_BASE + s as u32;
                    let status = horde.store().status_of(s);
                    let advantage = (status.advantage_against() + (m.advantage || ambush) as i8 + f.status.advantage()).clamp(-1, 1);
                    let ac = block.ac + status.ac();
                    let bless = if m.to_hit_die > 0 { rng.die(m.to_hit_die) as i32 } else { 0 };
                    match attack_roll(rng, weapon.to_hit(f.level) + m.to_hit + bless, ac, advantage, m.crit_from()) {
                        AttackOutcome::Miss => hits.push(HitEvent { target: id, amount: 0, flags: hit::MISS }),
                        outcome => {
                            // A plant takes one HP whatever lands (wound).
                            let crit = outcome == AttackOutcome::Critical && !horde.is_plant(s);
                            let bane = if horde.catalog().get(horde.store().kind[s as usize]).evil { m.evil_damage } else { 0 };
                            let amount = wound(horde, rng, s, damage, m.damage, crit, false, k) + bane;
                            horde.store_mut().hp[s as usize] -= bane;
                            let dead = horde.store().hp[s as usize] <= 0;
                            let mut flags = damage_type_bits(damage.kind);
                            if crit {
                                flags |= hit::CRITICAL;
                            }
                            if dead {
                                flags |= hit::KILL;
                                kills.push((s, uid));
                            }
                            hits.push(HitEvent { target: id, amount: amount as u32, flags });
                            if dead {
                                break;
                            }
                            if m.venom > 0 && amount > 0 && rng.die(100) <= m.venom {
                                envenom(horde, s, venom);
                            }
                        }
                    }
                }
            }
            self.run.credit(uid, super::stats::SKILL_ATTACK, &hits[h0..], tick, secs, horde.store());
            f.attack_cd = (weapon.interval_ms as f32 * m.interval() * hz / 1000.0).round() as u32;
            f.quiet = 0;
            p.facing = facing_of(ax, ay);
            swings.push(Swing { entity_id: entity, facing: p.facing, target: MONSTER_ENTITY_BASE + target as u32 });
        }

        // 2. Kills: experience for everyone in the party on this map, then
        //    the slot goes back to the pool (the client sees a REMOVE).
        let mut boss_down = false;
        let mut bursts: Vec<(f32, f32)> = Vec::new();
        if !kills.is_empty() {
            let levels: Vec<u32> =
                self.players.iter().filter(|p| p.map == map).filter_map(|p| p.fighter.as_ref().map(|f| f.level)).collect();
            for &(s, killer) in &kills {
                let store = horde.store();
                if !store.is_alive(s) {
                    continue;
                }
                boss_down |= store.flags[s as usize] & FLAG_BOSS != 0;
                // A cut plant is no kill: no combo, no record (COMBAT.md §4.10).
                let plant = store.is_plant(s);
                self.kills += u32::from(!plant);
                // The combo is the killer's own (a standing one's).
                if !plant
                    && let Some(f) = self.players.iter_mut().find(|p| p.profile.user_id == killer && p.map == map).and_then(|p| p.fighter.as_mut())
                    && f.is_up()
                {
                    f.combo.kill(1, hz);
                    let best = f.combo.count();
                    let r = f.tally();
                    r.best_combo = r.best_combo.max(best);
                }
                // The record: the kill is the blow's (else the looter's),
                // a boss is an MVP for everyone who shares it.
                let (mx, my) = (store.x[s as usize], store.y[s as usize]);
                let credited = if plant { None } else { looter(&self.players, map, killer, mx, my) };
                if let Some(f) = credited.and_then(|i| self.players[i].fighter.as_mut()) {
                    f.tally().kills += 1;
                }
                // The result screen: the same kill credit, assists, MVPs.
                if !plant {
                    let def = horde.catalog().get(store.kind[s as usize]);
                    let who = credited.map_or(0, |i| self.players[i].profile.user_id);
                    let on_map = self.players.iter().filter(|p| p.map == map && p.fighter.is_some()).map(|p| p.profile.user_id);
                    self.run.kill(s, who, tick, hz, secs, store, &def.name, def.archetype == Archetype::Elite, on_map);
                }
                if store.flags[s as usize] & FLAG_BOSS != 0 {
                    for f in self.players.iter_mut().filter(|p| p.map == map).filter_map(|p| p.fighter.as_mut()) {
                        f.tally().mvps += 1;
                    }
                }
                let def = horde.catalog().get(store.kind[s as usize]);
                let level = store.level[s as usize] as u32;
                // Mission modifiers that add monsters make each one worth
                // less: experience, zeny and drop chances alike.
                let share = horde.loot_permille(def.archetype);
                // Affixes: each star pays more, minions nothing (affix.rs).
                let bonus = super::affix::reward(store, s);
                let paid = (share as u64 * bonus.permille as u64 / 1000) as u32;
                let shares = share_kill(loot::scale_amount(kill_value(level, def.archetype), paid, rng), level, &levels);
                let zeny = loot::scale_amount(items::kill_zeny(level, def.archetype), paid, rng);
                let mut it = shares.into_iter();
                for p in self.players.iter_mut().filter(|p| p.map == map) {
                    if let Some(f) = p.fighter.as_mut() {
                        let before = f.level;
                        f.gain(it.next().unwrap_or(0));
                        f.zeny += zeny;
                        f.run_zeny += zeny;
                        if f.level > before && f.is_up() {
                            bursts.push((p.x, p.y));
                        }
                    }
                }
                // Drops: the killer's (or the nearest member's); elites and
                // bosses roll once for everyone on the map.
                if !def.drops.is_empty() && bonus.drop_permille > 0 {
                    let (mx, my) = (store.x[s as usize], store.y[s as usize]);
                    // A starred monster may roll its whole table twice.
                    let rolls = 1 + u32::from(bonus.extra_roll_pct > 0 && rng.chance(bonus.extra_roll_pct));
                    let entity_id = MONSTER_ENTITY_BASE + s as u32;
                    let shared = matches!(def.archetype, Archetype::Elite | Archetype::Boss);
                    let owner = if shared { None } else { looter(&self.players, map, killer, mx, my) };
                    for (i, p) in self.players.iter_mut().enumerate() {
                        if p.map != map || !(shared || owner == Some(i)) {
                            continue;
                        }
                        let uid = p.profile.user_id;
                        let Some(f) = p.fighter.as_mut() else { continue };
                        // Steal: the player's own roll at better odds.
                        let odds = (share as f32 * (1.0 + f.mods.loot) * bonus.drop_permille as f32 / 1000.0).round() as u32;
                        for _ in 0..rolls {
                            loot::roll_scaled(&def.drops, odds, rng, |d, n| {
                                if d.is_zeny() {
                                    f.zeny += n as u64;
                                    f.run_zeny += n as u64;
                                    f.progress_dirty = true;
                                } else {
                                    f.add_loot(d.item, n);
                                }
                                let msg = ServerMsg::Loot { entity_id, x: mx, y: my, item: d.item.into(), count: n, rare: d.rare };
                                out.push((uid, HubEvent::Control(msg)));
                            });
                        }
                    }
                }
                // Novas and the Volatile blast: marked now, landing later.
                super::affix::on_death(horde, s, tick, hz, rng, &mut self.affix, &mut self.hazards);
                horde.despawn(s);
            }
        }
        // A level-up bursts out (the heal is in `gain`): monsters around
        // are thrown back and flinch.
        if !bursts.is_empty() {
            let walk = &self.world.map(map).walk;
            let mut near = Vec::new();
            for &(x, y) in &bursts {
                let area = Area::Circle { x, y, r: LEVEL_UP_RADIUS };
                near.clear();
                area.monsters(horde, &mut near);
                for &s in &near {
                    push(horde, walk, area, s, LEVEL_UP_PUSH);
                    horde.flinch(s);
                }
            }
        }

        // 3. Monster skills: wind-ups, landings, hazards on players.
        self.monster_skills(out, &mut hits);
        // Scripted elites and bosses: phases, patterns, shouts (boss.rs).
        self.boss_step(out, &mut hits);
        // Monster affixes: blinks, regeneration, summons, sweeps (affix.rs).
        self.affix_step(out, &mut hits);
        let Some(horde) = self.horde.as_mut() else { return };
        let rng = &mut self.dice;

        // 4. Monsters in contact strike the nearest standing player (a
        //    taunted one its taunter, when in reach).
        let ups: Vec<(usize, f32, f32, u32)> = self
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.map == map && p.fighter.as_ref().is_some_and(Fighter::hunted))
            .map(|(i, p)| (i, p.x, p.y, p.profile.user_id))
            .collect();
        let alive = horde.store().alive().len();
        for k in 0..alive {
            let s = horde.store().alive()[k];
            let i = s as usize;
            let store = horde.store_mut();
            if store.cooldown[i] > 0 {
                store.cooldown[i] -= 1;
                continue;
            }
            let status = store.status_of(s);
            // Burrowed: it cannot bite from under the ground.
            if store.flags[i] & FLAG_FULL == 0 || ups.is_empty() || status.held() || store.staggered(s) || store.is_plant(s) || store.burrowed(s) {
                continue;
            }
            let (mx, my) = (store.x[i], store.y[i]);
            let reach = horde.catalog().get(horde.store().kind[i]).radius + PLAYER_RADIUS + MONSTER_REACH;
            let taunter = if status.has(Condition::Taunted) { horde.store().taunter[i] } else { 0 };
            let mut victim: Option<(usize, f32, f32, f32)> = None;
            for &(pi, px, py, uid) in &ups {
                let (dx, dy) = (px - mx, py - my);
                // The taunter sorts first.
                let d2 = dx * dx + dy * dy - if uid == taunter && taunter != 0 { 1e6 } else { 0.0 };
                if dx * dx + dy * dy <= reach * reach && victim.is_none_or(|(_, _, _, b)| d2 < b) {
                    victim = Some((pi, dx, dy, d2));
                }
            }
            let Some((pi, dx, dy, _)) = victim else { continue };
            // Provoked: it fights badly against anyone but its taunter.
            let off_target = taunter != 0 && ups.iter().find(|u| u.0 == pi).is_some_and(|u| u.3 != taunter);
            let block = horde.block(s);
            let p = &mut self.players[pi];
            let Some(f) = p.fighter.as_mut() else { continue };
            if !f.is_up() {
                continue;
            }
            let id = p.profile.character_id;
            if let Some(w) = self.lives.iter_mut().find(|z| z.shields(map, p.x, p.y)) {
                // Safety Wall: the blow lands on the ward.
                w.absorb();
                hits.push(HitEvent { target: id, amount: 0, flags: hit::GUARD });
            } else if f.mods.block > 0 && rng.chance(f.mods.block) {
                // Auto Guard: the shield takes it.
                hits.push(HitEvent { target: id, amount: 0, flags: hit::GUARD });
            } else {
                let advantage = (status.advantage() + f.status.advantage_against() - off_target as i8).clamp(-1, 1);
                match attack_roll(rng, block.attack, f.armour(), advantage, 20) {
                    AttackOutcome::Miss => hits.push(HitEvent { target: id, amount: 0, flags: hit::MISS }),
                    outcome => {
                        let crit = outcome == AttackOutcome::Critical;
                        let evil = horde.catalog().get(horde.store().kind[i]).evil;
                        let amount = f.take_from(block.damage.roll(rng, crit), true, evil);
                        let flags = damage_type_bits(DamageType::Physical) | if crit { hit::CRITICAL } else { 0 };
                        hits.push(HitEvent { target: id, amount: amount as u32, flags });
                        // Affixes: Vampiric heals, an enchantment adds its element.
                        super::affix::melee(horde, s, &block, f, id, amount, rng, hz, &mut hits);
                        f.fall_if_spent(hz);
                    }
                }
            }
            f.quiet = 0;
            let pace = super::affix::attack_interval(horde.store(), s);
            horde.store_mut().cooldown[i] = (block.attack_interval_ms as f32 * pace * hz / 1000.0).round() as u16;
            swings.push(Swing { entity_id: MONSTER_ENTITY_BASE + s as u32, facing: facing_of(dx, dy), target: id });
        }

        // 5. Downed, revive, regeneration.
        let wave = horde.waves();
        let new_wave = wave != self.last_wave;
        self.last_wave = wave;
        let positions: Vec<(u32, f32, f32, bool)> = self
            .players
            .iter()
            .filter(|p| p.map == map)
            .map(|p| (p.profile.user_id, p.x, p.y, p.fighter.as_ref().is_some_and(Fighter::is_up)))
            .collect();
        let tick = self.tick;
        let every = (hz as u32).max(1);
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let (uid, x, y) = (p.profile.user_id, p.x, p.y);
            let Some(f) = p.fighter.as_mut() else { continue };
            // The combo runs down between kills; going down ends it.
            if f.is_up() {
                f.combo.tick();
            } else {
                f.combo.reset();
            }
            match f.state {
                Vital::Up => {
                    // Burning and poison bite once a second (staggered).
                    if f.status.ailing() && (tick + uid).is_multiple_of(every) {
                        for (c, kind) in [(Condition::Burning, DamageType::Fire), (Condition::Poisoned, DamageType::Poison)] {
                            if f.status.has(c) {
                                let amount = f.take(rng.die(4) as i32, false);
                                hits.push(HitEvent { target: p.profile.character_id, amount: amount as u32, flags: damage_type_bits(kind) });
                                f.quiet = 0;
                            }
                        }
                        f.fall_if_spent(hz);
                    }
                    f.wear_off();
                    if !f.is_up() {
                        continue;
                    }
                    f.quiet = f.quiet.saturating_add(1);
                    if f.quiet as f32 >= QUIET_SECS * hz && f.hp < f.max_hp {
                        f.regen_hp += f.max_hp as f32 * 0.005 / hz;
                    }
                    if f.mods.hp_regen > 0.0 && f.hp < f.max_hp {
                        f.regen_hp += f.max_hp as f32 * f.mods.hp_regen / hz;
                    }
                    let sp_rate = if p.dir == (0.0, 0.0) { 0.02 } else { 0.005 } + f.mods.sp_regen;
                    if f.sp < f.max_sp {
                        f.regen_sp += f.max_sp as f32 * sp_rate / hz;
                    }
                    let (h, s) = (f.regen_hp.floor(), f.regen_sp.floor());
                    f.hp = (f.hp + h as i32).min(f.max_hp);
                    f.sp = (f.sp + s as i32).min(f.max_sp);
                    f.regen_hp -= h;
                    f.regen_sp -= s;
                    // Potions: healing over time, shown once a second.
                    if let Some(amount) = f.potion_tick(every) {
                        hits.push(HitEvent { target: p.profile.character_id, amount: amount as u32, flags: hit::HEAL });
                    }
                }
                Vital::Downed { ticks } => {
                    let helped = positions.iter().any(|&(other, ox, oy, up)| {
                        up && other != uid && (ox - x).powi(2) + (oy - y).powi(2) <= REVIVE_RANGE * REVIVE_RANGE
                    });
                    // Help fills the revive and holds the bleed-out; without
                    // it the progress drains back as fast as it filled.
                    f.revive = if helped { f.revive + 1 } else { f.revive.saturating_sub(1) };
                    if f.revive as f32 >= REVIVE_SECS * hz {
                        f.state = Vital::Up;
                        f.hp = (f.max_hp / 4).max(1);
                        f.revive = 0;
                        f.quiet = 0;
                    } else if !helped && ticks <= 1 {
                        f.state = Vital::Dead;
                        f.revive = 0;
                    } else if !helped {
                        f.state = Vital::Downed { ticks: ticks - 1 };
                    }
                }
                Vital::Dead => {
                    // The fog's dead stay dead.
                    if new_wave && !f.fog_dead {
                        f.state = Vital::Up;
                        f.hp = (f.max_hp / 2).max(1);
                    }
                }
            }
        }

        // The result screen: damage taken, downs, revives, combos.
        self.run.observe(tick, secs, &hits, &self.players, map);

        // 6. Messages: combat to the map, vitals on change, progress to self.
        let recipients: Vec<u32> = self.players.iter().filter(|p| p.map == map).map(|p| p.profile.user_id).collect();
        let mut skill_updates: Vec<u32> = Vec::new();
        if !swings.is_empty() || !hits.is_empty() {
            let msg = ServerMsg::Combat { swings, hits };
            for &uid in &recipients {
                out.push((uid, HubEvent::Control(msg.clone())));
            }
        }
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let id = p.profile.character_id;
            let uid = p.profile.user_id;
            let Some(f) = p.fighter.as_mut() else { continue };
            let now = [
                f.hp.max(0),
                f.max_hp,
                f.sp,
                f.max_sp,
                f.state.code() as i32,
                f.status.0 as i32,
                f.temp_hp,
                f.mods.light.round() as i32,
            ];
            if f.sent != Some(now) {
                f.sent = Some(now);
                let msg = f.vitals(id);
                for &r in &recipients {
                    out.push((r, HubEvent::Control(msg.clone())));
                }
            }
            if let Some(msg) = f.revive_msg(id, hz) {
                for &r in &recipients {
                    out.push((r, HubEvent::Control(msg.clone())));
                }
            }
            if std::mem::take(&mut f.skills_dirty) {
                skill_updates.push(uid);
            }
            if f.combo.take_dirty() {
                out.push((uid, HubEvent::Control(f.combo_msg(hz))));
            }
            f.progress_wait = f.progress_wait.saturating_sub(1);
            if f.progress_dirty && f.progress_wait == 0 {
                out.push((uid, HubEvent::Control(f.progress())));
                out.push((uid, HubEvent::Progress { level: f.level, exp: f.exp }));
                if f.zeny > 0 {
                    out.push((uid, HubEvent::Zeny { amount: std::mem::take(&mut f.zeny) }));
                }
                if let Some(loot) = f.take_loot() {
                    self.run.looted(uid, &loot);
                    out.push((uid, loot));
                }
                if let Some(record) = f.take_record() {
                    out.push((uid, record));
                }
                f.gained = 0;
                f.progress_dirty = false;
                f.progress_wait = PROGRESS_EVERY;
            }
        }

        for uid in skill_updates {
            if let Some(i) = self.find(uid) {
                out.push((uid, HubEvent::Control(self.skills_msg(i))));
            }
        }

        // 7. Defeat: nobody on the map is standing.
        let fighters = self.players.iter().filter(|p| p.map == map && p.fighter.is_some()).count();
        let standing = self.players.iter().filter(|p| p.map == map && p.fighter.as_ref().is_some_and(Fighter::is_up)).count();
        if fighters > 0 && standing == 0 {
            self.defeat(map, out);
        }
        self.mission_step(map, boss_down, out);
    }

    /// The whole party is down: each member loses 1% of their level's
    /// requirement (never a level); the result screen, then the exit.
    fn defeat(&mut self, map: super::MapIdx, out: &mut Outgoing) {
        if self.outcome != super::mission::Outcome::Running {
            return;
        }
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let uid = p.profile.user_id;
            if let Some(f) = p.fighter.as_mut() {
                let penalty = exp_to_next(f.level) / 100;
                f.exp = f.exp.saturating_sub(penalty);
                f.gained = 0;
                out.push((uid, HubEvent::Control(f.progress())));
                out.push((uid, HubEvent::Progress { level: f.level, exp: f.exp }));
            }
        }
        self.end_stage(map, false, out);
    }

    /// Vitals of everyone on the newcomer's map to them, theirs to everyone,
    /// and their progress to them.
    pub(super) fn announce_fighter(&self, idx: usize, out: &mut Outgoing) {
        let p = &self.players[idx];
        let Some(f) = p.fighter.as_ref() else { return };
        let mine = f.vitals(p.profile.character_id);
        for o in self.players.iter().filter(|o| o.map == p.map) {
            out.push((o.profile.user_id, HubEvent::Control(mine.clone())));
            if o.profile.user_id != p.profile.user_id
                && let Some(of) = o.fighter.as_ref()
            {
                out.push((p.profile.user_id, HubEvent::Control(of.vitals(o.profile.character_id))));
                if matches!(of.state, Vital::Downed { .. }) {
                    let hz = self.config.tick_hz as f32;
                    out.push((p.profile.user_id, HubEvent::Control(of.revive_state(o.profile.character_id, hz))));
                }
            }
        }
        out.push((p.profile.user_id, HubEvent::Control(f.progress())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_levels_up_and_heals() {
        let mut f = Fighter::new(Class::Mage, 1, 0, "");
        f.hp = 1;
        f.gain(exp_to_next(1) + exp_to_next(2) + 7);
        assert_eq!((f.level, f.exp), (3, 7));
        assert_eq!((f.hp, f.max_hp), (Class::Mage.max_hp(3), Class::Mage.max_hp(3)));
        assert!(f.progress_dirty);
    }

    #[test]
    fn fighters_hold_a_weapon_their_class_allows() {
        assert_eq!(Fighter::new(Class::Swordsman, 1, 0, "katana").weapon.id, "katana");
        assert_eq!(Fighter::new(Class::Archer, 1, 0, "katana").weapon.id, "bow");
    }
}
