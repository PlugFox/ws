//! Stage combat: basic attacks, monster melee, kills and experience,
//! downed / revive / defeat (COMBAT.md §4). Rules come from
//! [`crate::world::combat`]; this file only applies them to the instance's
//! players and horde once per tick.

use super::aim::aimed_plant;
use super::area::Area;
use super::cast::{envenom, inflict, push, ticks16, wound, Casting, Kill, VENOM_SECS};
use super::{facing_of, HubEvent, HubInstance, HubPlayer, MapIdx, Outgoing};
use crate::world::combat::skills::SLOTS;
use crate::world::combat::rules::{attack_roll, AttackOutcome};
use crate::world::combat::cards::Procs;
use crate::world::combat::items::{self, Bag, Gear};
use crate::world::combat::loot;
use crate::world::combat::{
    self, cantrip_k, exp_to_next, kill_value, share_kill, Archetype, Class, Combo, Condition, Damage, DamageType, Dice, Mods, Reach, Save,
    Status, Weapon, CONDITIONS,
};
use crate::world::horde::storage::{FLAG_BOSS, FLAG_FULL};
use crate::world::horde::Horde;
use crate::world::protocol::{hit, HitEvent, ServerMsg, Swing, MONSTER_ENTITY_BASE};
use crate::world::record::Record;
use crate::world::modifiers::ModFlags;

/// Vitals status bits past the conditions: a thief hidden under the ground
/// (the burrowed monsters' bit: clients draw the same mound) ...
pub const STATUS_HIDDEN: u32 = 1 << 15;
/// ... or cloaked (walking unseen: drawn as a shimmer).
pub const STATUS_CLOAKED: u32 = 1 << 14;

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

/// What the worn cards keep track of during a stage (cards.rs): counters
/// and cooldowns of the effects that are not plain numbers.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CardState {
    /// Kills this expedition (Mastering's feast, Phreeoni's growth).
    pub kills: u32,
    /// Damage Phreeoni's card has grown to.
    pub growth: u8,
    /// Skill casts (Nine Tail's free one).
    pub casts: u32,
    /// Ticks until the Sandman's veil, the Moonlight Flower's free skill
    /// and the Horong's wisp are ready again.
    pub veil_cd: u32,
    pub moon_cd: u32,
    pub wisp_cd: u32,
    /// Ticks left of a dodge's rush (Creamy).
    pub rush: u32,
    /// The Angeling's last stand is spent for this expedition.
    pub stood: bool,
    /// A block or dodge readied the Bongun's counter.
    pub counter: bool,
    /// Standing still 2 s (Willow) and 3 s (Hode), set each tick.
    pub planted: bool,
    pub settled: bool,
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
    /// Counters and cooldowns of the worn cards.
    pub(super) cards: CardState,
    /// A parry readied the riposte: the next basic swing comes at once,
    /// with advantage and one more weapon die on its first blow (Parry).
    pub(super) riposte: bool,
    /// Where seeking hits of this fighter's casts will land (skill, hit
    /// radius, x, y, ms from now), not yet told to the stage
    /// (`SKILL_STRIKES`).
    pub(super) strikes: Vec<(u16, f32, f32, f32, u32)>,
    /// The arrow nocked (Arrow Change): a bow's basic attack and arrow
    /// skills take its element; physical = the plain steel arrow.
    pub(super) arrow: DamageType,
    /// The nocked arrow changed: the stage is told (`QUIVER`).
    pub(super) quiver_dirty: bool,
    /// Field plants this fighter grew (Plant Cultivation, `cast.rs`):
    /// slot and generation, oldest first.
    pub(super) grown: Vec<(u16, u8)>,
    /// The combat sheet last sent to the player (`PLAYER_SHEET`,
    /// `sheet.rs`); `None` sends it with the next check.
    pub(super) sheet_sent: Option<super::sheet::Sheet>,
}

/// The terms of a fighter's armour class ([`Fighter::armour_parts`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub(super) struct ArmourParts {
    /// Armour, shield, refines and the tier (`Gear::ac_at`).
    pub worn: i32,
    /// Magic items, card sets and passives (no buffs).
    pub items: i32,
    /// Running buffs (party ones too: Angelus, Steel Body...).
    pub buffs: i32,
    /// Conditions (Mired, Taunted).
    pub conditions: i32,
    /// Cards that hold right now (standing still, under 30% HP).
    pub cards: i32,
}

impl ArmourParts {
    pub const fn total(&self) -> i32 {
        self.worn + self.items + self.buffs + self.conditions + self.cards
    }
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
            cards: CardState::default(),
            riposte: false,
            strikes: Vec::new(),
            arrow: DamageType::Physical,
            quiver_dirty: false,
            grown: Vec::new(),
            sheet_sent: None,
        }
    }

    /// The kill combo: count and style rank, for skills that scale with it
    /// (COMBAT.md §5.4).
    pub fn combo(&self) -> &Combo {
        &self.combo
    }

    /// The worn cards' effects (cards.rs).
    pub(super) fn procs(&self) -> &Procs {
        &self.gear.procs
    }

    /// Under 30% HP (the Weeping Bride's card).
    fn desperate(&self) -> bool {
        self.hp * 10 < self.max_hp * 3
    }

    /// Walking pace the cards add: under half HP, per style rank, riding,
    /// a dodge's rush.
    pub(super) fn card_pace(&self) -> f32 {
        let o = &self.gear.procs;
        let mut p = 1.0;
        if o.flee_speed > 0 && self.hp * 2 < self.max_hp {
            p += o.flee_speed as f32 / 100.0;
        }
        if o.rank_speed > 0 {
            p += o.rank_speed as f32 * self.combo.rank() as u8 as f32 / 100.0;
        }
        if o.ride_speed > 0 && self.riding {
            p += o.ride_speed as f32 / 100.0;
        }
        if self.cards.rush > 0 {
            p += 0.2;
        }
        p
    }

    /// A kill by this player: the cards' counters (Mastering's feast,
    /// Phreeoni's growth) and the longer combo window.
    fn card_kill(&mut self, hz: f32) {
        let o = self.gear.procs;
        if o.is_none() {
            return;
        }
        self.cards.kills += 1;
        if o.combo_secs > 0 {
            self.combo.stretch((o.combo_secs as f32 * hz) as u32);
        }
        if o.feast > 0 && self.cards.kills.is_multiple_of(o.feast as u32) {
            self.hp = (self.hp + self.max_hp / 4).min(self.max_hp);
            self.sp = (self.sp + self.max_sp / 4).min(self.max_sp);
        }
        if o.growth > 0 && self.cards.kills.is_multiple_of(25) {
            self.cards.growth = (self.cards.growth + 1).min(o.growth);
        }
    }

    /// One tick of the cards' timers and stances.
    fn card_tick(&mut self, hz: f32) {
        let c = &mut self.cards;
        c.veil_cd = c.veil_cd.saturating_sub(1);
        c.moon_cd = c.moon_cd.saturating_sub(1);
        c.wisp_cd = c.wisp_cd.saturating_sub(1);
        c.rush = c.rush.saturating_sub(1);
        c.planted = self.still as f32 >= 2.0 * hz;
        c.settled = self.still as f32 >= 3.0 * hz;
    }

    /// The skill about to be cast is free (no SP, and with the Moonlight
    /// Flower no cooldown): read before the SP check, nothing spent.
    pub(super) fn free_cast(&self) -> (bool, bool) {
        let o = &self.gear.procs;
        let moon = o.moon > 0 && self.cards.moon_cd == 0;
        let fox = o.free_every > 0 && (self.cards.casts + 1).is_multiple_of(o.free_every as u32);
        (moon || fox, moon)
    }

    /// A skill went off: counts it, spends the Moonlight Flower's charge.
    /// Returns (SP free, cooldown free); `roll` is a d100 when the
    /// Vocal's card is worn (else unused).
    pub(super) fn card_cast(&mut self, hz: f32, roll: impl FnOnce() -> u32) -> (bool, bool) {
        let o = self.gear.procs;
        if o.is_none() {
            return (false, false);
        }
        let (free, moon) = self.free_cast();
        self.cards.casts += 1;
        if moon {
            self.cards.moon_cd = (o.moon as f32 * hz) as u32;
        }
        let encore = o.free_cd > 0 && roll() <= o.free_cd as u32;
        (free, moon || encore)
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
        self.sum_mods();
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

    /// The passives and the magic items worn: the mods without buffs.
    pub(super) fn base_mods(&self) -> Mods {
        let mut m = self.passive;
        m.add(&self.gear.mods);
        m
    }

    fn sum_mods(&mut self) {
        // Magic items worn count like passives.
        let mut m = self.base_mods();
        for b in &self.buffs {
            m.add(&b.mods);
        }
        self.mods = m;
    }

    /// Drops buffs that ran out (at tick `now`) and sums the rest.
    pub(super) fn refresh(&mut self, now: u32) {
        let n = self.buffs.len();
        self.buffs.retain(|b| b.until > now);
        if self.buffs.len() != n || self.buffs.is_empty() && self.mods != self.base_mods() {
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

    /// The element a bow's arrows carry now (Arrow Change), `None` for the
    /// plain arrow or without a bow in hand.
    pub(super) fn nocked(&self) -> Option<DamageType> {
        (self.weapon.kind == combat::WeaponKind::Bow && self.arrow != DamageType::Physical).then_some(self.arrow)
    }

    /// Hiding under the ground (Hiding: a thief's, not playing dead) or
    /// cloaked: what clients draw ([`STATUS_HIDDEN`], [`STATUS_CLOAKED`]).
    pub(super) fn veil(&self) -> u32 {
        match (self.mods.hidden && self.mods.ambush, self.mods.cloak) {
            (true, true) => STATUS_CLOAKED,
            (true, false) => STATUS_HIDDEN,
            _ => 0,
        }
    }

    /// Arrow Change at level `slv`: the next of the first `slv + 1`
    /// `arrows` (after the last, the first again).
    pub(super) fn nock_next(&mut self, arrows: &[DamageType], slv: u8) {
        let open = &arrows[..(slv as usize + 1).min(arrows.len())];
        let at = open.iter().position(|&a| a == self.arrow);
        let next = at.map_or(0, |i| (i + 1) % open.len());
        self.nock(open[next]);
    }

    /// Nocks `arrow` (told to the stage when it changed).
    pub(super) fn nock(&mut self, arrow: DamageType) {
        if self.arrow != arrow {
            self.arrow = arrow;
            self.quiver_dirty = true;
        }
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

    /// A melee blow was caught: Blade Stop is spent.
    pub(super) fn spend_catch(&mut self) {
        if self.mods.catch > 0.0 {
            self.buffs.retain(|b| b.mods.catch <= 0.0);
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
        let o = &self.gear.procs;
        // Cards, and a ward against them (Full Chemical Protection).
        if (o.immune | self.mods.immune) & c.bit() != 0 {
            return;
        }
        let planted = o.still_ac != 0 && self.cards.planted;
        if (self.mods.steady || planted) && matches!(c, Condition::Stunned | Condition::Frozen) {
            return;
        }
        if c == Condition::Feared && o.desperate_ac != 0 && self.desperate() {
            return;
        }
        let ticks = if o.cond_short > 0 { ((ticks as u32 * (100 - o.cond_short as u32) / 100).max(1)) as u16 } else { ticks };
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

    /// Armour class with buffs and conditions (what monsters roll against).
    pub(super) fn armour(&self) -> i32 {
        self.armour_parts().total()
    }

    /// Where [`Fighter::armour`] comes from (the Equipment window's
    /// breakdown, `sheet.rs`).
    pub(super) fn armour_parts(&self) -> ArmourParts {
        let o = &self.gear.procs;
        let planted = if self.cards.planted { o.still_ac as i32 } else { 0 };
        let desperate = if o.desperate_ac != 0 && self.desperate() { o.desperate_ac as i32 } else { 0 };
        let base = self.base_mods().ac;
        ArmourParts { worn: self.ac, items: base, buffs: self.mods.ac - base, conditions: self.status.ac(), cards: planted + desperate }
    }

    /// The basic attack's bonus with `m` (the fighter's mods, maybe with a
    /// swing's situational extras), without Bless's die.
    pub(super) fn to_hit_with(&self, m: &Mods) -> i32 {
        self.weapon.to_hit(self.level) + m.to_hit
    }

    /// The basic attack's weapon dice with `m` and the element it deals.
    pub(super) fn swing_dice(&self, m: &Mods) -> (Dice, DamageType) {
        let (dice, kind) = self.weapon.damage(self.class, self.level);
        // Maximize Power: the weapon's dice at their maximum. A card's
        // element replaces the weapon's; the arrow nocked (Arrow Change)
        // replaces both.
        (m.weapon_dice(dice), self.nocked().unwrap_or(self.gear.procs.endow.unwrap_or(kind)))
    }

    /// Flat damage per basic-attack hit with `m` (masteries, refines,
    /// Phreeoni's growth); a swing may add the pack's.
    pub(super) fn swing_flat(&self, m: &Mods) -> i32 {
        m.damage_flat + self.cards.growth as i32
    }

    /// Walking speed multiplier: buffs, cards, conditions, curses.
    pub(super) fn pace(&self) -> f32 {
        let curse = if self.curses.intersects(ModFlags::SLOW) { crate::world::modifiers::SLOW_PACE } else { 1.0 };
        self.mods.pace() * self.card_pace() * self.status.speed() * curse
    }

    /// Saving throw bonus against `c` (or a plain Reflex save).
    pub(super) fn save_against(&self, c: Option<Condition>) -> i32 {
        self.save_bonus(c.map_or(Save::Reflex, Condition::save))
    }

    /// Saving throw bonus of `save` (class and gear).
    pub(super) fn save_bonus(&self, save: Save) -> i32 {
        self.class.save_bonus(self.level, save) + self.gear.saves[save as usize]
    }

    /// Takes a hit: resistance and armour, then temporary HP, then HP.
    /// Returns what came off HP and temporary HP together.
    pub(super) fn take(&mut self, amount: i32, physical: bool) -> i32 {
        let mut a = self.mods.taken(amount, physical);
        let o = self.gear.procs;
        // The Golden Thief Bug's card: magic turns to coin.
        if o.gilded > 0 && !physical && a > 0 {
            let cut = a * o.gilded as i32 / 100;
            a -= cut;
            self.add_zeny(cut as u64);
        }
        // The Jiangshi General's: no blow takes more than a share.
        if o.blow_cap > 0 {
            a = a.min((self.max_hp * o.blow_cap as i32 / 100).max(1));
        }
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

    /// Running conditions as (condition, ticks left), for the HUD.
    pub(super) fn cond_list(&self) -> impl Iterator<Item = (Condition, u32)> + '_ {
        Condition::ALL.into_iter().zip(self.cond).filter(|&(_, t)| t > 0).map(|(c, t)| (c, u32::from(t)))
    }

    /// Changes when a condition starts, ends or is lengthened by a second
    /// or more (a pool re-poisoning each tick does not resend the HUD).
    pub(super) fn cond_signature(&self, now: u32, hz: u32) -> u64 {
        self.cond_list().fold(0, |h, (c, t)| {
            h.rotate_left(11) ^ (u64::from(c as u8) << 40 | u64::from(now.saturating_add(t) / hz.max(1))).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        })
    }

    /// Downed at 0 HP (conditions end).
    pub(super) fn fall_if_spent(&mut self, hz: f32) {
        if self.hp <= 0 && self.is_up() {
            // The Angeling's card: once per expedition, not yet.
            if self.gear.procs.last_stand && !self.cards.stood {
                self.cards.stood = true;
                self.hp = (self.max_hp * 3 / 10).max(1);
                return;
            }
            self.hp = 0;
            self.tally().deaths += 1;
            let bleed = if self.gear.procs.deathless { 2.0 } else { 1.0 };
            self.state = Vital::Downed { ticks: (DOWNED_SECS * bleed * hz) as u32 };
            self.cards.growth = 0;
            self.revive = 0;
            self.clear_conditions();
            self.regen = [None, None];
        }
    }

    /// Killed outright (the overtime fog): dead for the rest of the run,
    /// whatever was standing or bleeding out.
    pub(super) fn perish(&mut self, hz: f32) {
        self.hp = 0;
        self.cards.stood = true;
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
            status: self.status.0 as u32 | self.veil(),
            temp_hp: self.temp_hp.max(0) as u32,
            light: (self.mods.light.max(0.0).round() as u32).min(255),
            level: self.level,
        }
    }

    /// `QUIVER` of a fighter with a non-plain arrow nocked (to a player
    /// arriving: the plain one is the default).
    fn quiver_msg(&self, entity_id: u32) -> Option<ServerMsg> {
        (self.arrow != DamageType::Physical).then_some(ServerMsg::Quiver { entity_id, element: self.arrow as u8 })
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
        // The cards' run counters are spent like HP.
        self.cards.stood = old.cards.stood;
        self.cards.kills = old.cards.kills;
        self.cards.growth = old.cards.growth;
        // The arrow nocked stays nocked.
        self.arrow = old.arrow;
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
        // Last tick's self-destructs (a `sacrifice` monster skill) first.
        let mut kills: Vec<Kill> = std::mem::take(&mut self.doomed);
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
        // What each player's swing struck (their homunculus helps there).
        let mut owner_swings: Vec<(u32, u16, u8)> = Vec::new();
        // The Baby Desert Wolf's card: who stands where (only when worn).
        let pack: Vec<(u32, f32, f32)> = if self.players.iter().any(|p| p.fighter.as_ref().is_some_and(|f| f.procs().pack != 0)) {
            self.players.iter().filter(|p| p.map == map && p.fighter.as_ref().is_some_and(Fighter::is_up)).map(|p| (p.profile.user_id, p.x, p.y)).collect()
        } else {
            Vec::new()
        };
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let Some(f) = p.fighter.as_mut() else { continue };
            f.attack_cd = f.attack_cd.saturating_sub(1);
            // Link-dead players do not fight back (they only take hits).
            // Hiding stops the attack (a thief strikes out of it with a
            // skill, never by an automatic swing that gives them away).
            if !f.is_up() || p.dir != (0.0, 0.0) || f.mods.hidden || f.status.helpless() || p.link_dead {
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
            let o = *f.procs();
            // Katars crit a step earlier.
            m.crit += weapon.kind.crit();
            let (dice, kind) = f.swing_dice(&m);
            let uid0 = p.profile.user_id;
            let near = o.pack != 0
                && pack.iter().any(|&(u, x, y)| u != uid0 && (x - p.x).powi(2) + (y - p.y).powi(2) <= 25.0);
            if near {
                m.to_hit += o.pack as i32;
            }
            let flat = f.swing_flat(&m) + if near { o.pack as i32 } else { 0 };
            let damage = Damage::new(Dice { bonus: dice.bonus + flat, ..dice }, kind);
            let (range, arc_cos, splash) = match weapon.reach {
                Reach::Melee { range, arc_cos } => (range, Some(arc_cos), 0.0),
                Reach::Ranged { range, splash } => (range + o.reach as f32, None, splash),
            };
            let ranged = arc_cos.is_none();
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
            // A riposte after a parry: advantage, one more weapon die.
            let riposte = std::mem::take(&mut f.riposte);
            let mut riposte_die = riposte;
            let venom = ticks16(VENOM_SECS * if o.venom_long { 2.0 } else { 1.0 }, hz);
            // The Bongun's counter: the first blow of this swing.
            let mut counter = if f.cards.counter && o.counter > 0 { o.counter as u32 } else { 0 };
            f.cards.counter = false;
            let entity = p.profile.character_id;
            let uid = p.profile.user_id;
            let k = cantrip_k(f.level);
            let h0 = hits.len();
            for &s in &targets {
                let block = horde.block(s);
                for _ in 0..attacks {
                    let id = MONSTER_ENTITY_BASE + s as u32;
                    let status = horde.store().status_of(s);
                    let advantage = (status.advantage_against() + (m.advantage || ambush || riposte) as i8 + f.status.advantage()).clamp(-1, 1);
                    let ac = block.ac + status.ac();
                    let bless = if m.to_hit_die > 0 { rng.die(m.to_hit_die) as i32 } else { 0 };
                    match attack_roll(rng, f.to_hit_with(&m) + bless, ac, advantage, m.crit_from()) {
                        AttackOutcome::Miss => hits.push(HitEvent { target: id, amount: 0, flags: hit::MISS }),
                        outcome => {
                            let def = horde.catalog().get(horde.store().kind[s as usize]);
                            let (evil, size, elite) = (def.evil, def.size, matches!(def.archetype, Archetype::Elite | Archetype::Boss));
                            let hp_now = horde.store().hp[s as usize];
                            // The Desert Wolf's card: the weak are finished.
                            let cull = o.cull && hp_now * 5 <= block.hp;
                            // A plant takes one HP whatever lands (wound).
                            let crit = (outcome == AttackOutcome::Critical || cull) && !horde.is_plant(s);
                            let mut bane = if evil { m.evil_damage } else { 0 };
                            if riposte_die && !horde.is_plant(s) {
                                bane += Dice { bonus: 0, ..dice }.roll(rng, crit);
                                riposte_die = false;
                            }
                            if !o.is_none() && !horde.is_plant(s) {
                                bane += o.size_bonus(size);
                                if status.0 != 0 {
                                    bane += o.afflicted_bane as i32;
                                }
                                if status.has(Condition::Stunned) || status.has(Condition::Frozen) || status.has(Condition::Rooted) || status.has(Condition::Slowed) {
                                    bane += o.held_bane as i32;
                                }
                                if elite || horde.store().affix[s as usize] != 0 {
                                    bane += o.elite_bane as i32;
                                }
                                if o.fresh > 0 && hp_now >= block.hp {
                                    bane += rng.die(o.fresh as u32) as i32;
                                }
                                if crit && ranged && o.crit_die > 0 {
                                    bane += rng.die(o.crit_die as u32) as i32;
                                }
                                if counter > 0 {
                                    bane += Dice::new(counter, 6, 0).roll(rng, false);
                                    counter = 0;
                                }
                            }
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
                            if !o.is_none() && amount > 0 {
                                card_hit(&o, f, horde, rng, s, crit, hz, &mut hits, p.profile.character_id);
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
            owner_swings.push((uid, target, horde.store().generation[target as usize]));
        }

        // 1b. Companions (companion.rs): homunculi think and fight, plants
        //     and spheres act; their kills join the list, paid to the owner.
        for (uid, slot, generation) in owner_swings {
            self.companions.owner_swung(uid, slot, generation);
        }
        self.companions_step(map, &mut hits, &mut kills, &mut swings);
        // A companion's kill pays its owner (experience, zeny, loot, the
        // record) but builds no combo and counts for no card.
        let pet_kills: Vec<(u16, u32)> = self.companions.killed().collect();
        let Some(horde) = self.horde.as_mut() else { return };
        let rng = &mut self.dice;

        // 2. Kills: experience for everyone in the party on this map, then
        //    the slot goes back to the pool (the client sees a REMOVE).
        let mut boss_down = false;
        let mut bursts: Vec<(f32, f32)> = Vec::new();
        // Corpse skills (`on_death`, brain.rs): telegraphs to send.
        let mut corpse: Vec<ServerMsg> = Vec::new();
        if !kills.is_empty() {
            let book = std::sync::Arc::clone(&self.skills);
            if self.kind_skills.len() != horde.catalog().defs().len() {
                self.kind_skills = super::brain::kind_skills(horde, &book);
            }
            let standing: Vec<(f32, f32)> =
                self.players.iter().filter(|p| p.map == map && p.fighter.as_ref().is_some_and(|f| f.hunted())).map(|p| (p.x, p.y)).collect();
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
                    && !pet_kills.contains(&(s, killer))
                    && let Some(f) = self.players.iter_mut().find(|p| p.profile.user_id == killer && p.map == map).and_then(|p| p.fighter.as_mut())
                    && f.is_up()
                {
                    f.combo.kill(1, hz);
                    f.card_kill(hz);
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
                let starred = matches!(def.archetype, Archetype::Elite | Archetype::Boss) || store.affix[s as usize] != 0;
                for p in self.players.iter_mut().filter(|p| p.map == map) {
                    if let Some(f) = p.fighter.as_mut() {
                        let before = f.level;
                        let share = it.next().unwrap_or(0);
                        f.gain(share);
                        // Its homunculus earns a bonus copy (nothing is
                        // taken from the owner).
                        if !plant {
                            self.companions.grant_exp(p.profile.user_id, share);
                        }
                        // Cards that pay more (rolled only when worn).
                        let o = f.procs();
                        let pct = o.zeny_pct as u32 + if starred { o.elite_zeny as u32 } else { 0 };
                        let zeny = if pct > 0 && zeny > 0 { loot::scale_amount(zeny, 1000 + 10 * pct, rng) } else { zeny };
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
                let kind = horde.store().kind[s as usize] as usize;
                if let Some(skills) = self.kind_skills.get(kind).filter(|k| !k.is_empty()) {
                    super::brain::on_death(horde, s, skills, &book, &standing, tick, hz, rng, &mut self.next_zone, &mut self.hazards, &mut hits, &mut corpse);
                }
                horde.despawn(s);
            }
        }
        for p in self.players.iter().filter(|p| p.map == map) {
            for m in &corpse {
                out.push((p.profile.user_id, HubEvent::Control(m.clone())));
            }
        }
        if boss_down {
            let owners: Vec<u32> = self.players.iter().filter(|p| p.map == map).map(|p| p.profile.user_id).collect();
            self.companions.bond_out(&owners, super::companion::BOND_BOSS);
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
            if store.flags[i] & FLAG_FULL == 0
                || (ups.is_empty() && self.companions.is_empty())
                || status.held()
                || store.staggered(s)
                || store.is_plant(s)
                || store.burrowed(s)
                // Sacrificed: dead already, paid next tick.
                || store.hp[i] <= 0
            {
                continue;
            }
            let (mx, my) = (store.x[i], store.y[i]);
            let body = horde.catalog().get(horde.store().kind[i]).radius;
            let reach = body + PLAYER_RADIUS + MONSTER_REACH;
            let taunter = if status.has(Condition::Taunted) { horde.store().taunter[i] } else { 0 };
            // Companions (companion.rs): a lure that taunted it is its
            // victim when in reach; with no player in reach it bites the
            // nearest companion in reach (a plant hedge gets eaten).
            let pet = self.companions.lure_in_reach(taunter, mx, my, body + MONSTER_REACH);
            let mut victim: Option<(usize, f32, f32, f32)> = None;
            for &(pi, px, py, uid) in ups.iter().filter(|_| pet.is_none()) {
                let (dx, dy) = (px - mx, py - my);
                // The taunter sorts first.
                let d2 = dx * dx + dy * dy - if uid == taunter && taunter != 0 { 1e6 } else { 0.0 };
                if dx * dx + dy * dy <= reach * reach && victim.is_none_or(|(_, _, _, b)| d2 < b) {
                    victim = Some((pi, dx, dy, d2));
                }
            }
            let pet = pet.or_else(|| if victim.is_none() { self.companions.nearest_in_reach(mx, my, body + MONSTER_REACH) } else { None });
            if let Some(ci) = pet {
                let block = horde.block(s);
                let (cx_, cy_, id, ac) = self.companions.target_of(ci, self.tick);
                let advantage = status.advantage().clamp(-1, 1);
                match attack_roll(rng, block.attack, ac, advantage, 20) {
                    AttackOutcome::Miss => hits.push(HitEvent { target: id, amount: 0, flags: hit::MISS }),
                    outcome => {
                        let crit = outcome == AttackOutcome::Critical;
                        let amount = self.companions.take(ci, block.damage.roll(rng, crit), self.tick);
                        let flags = damage_type_bits(DamageType::Physical) | if crit { hit::CRITICAL } else { 0 };
                        hits.push(HitEvent { target: id, amount: amount as u32, flags });
                    }
                }
                let pace = super::affix::attack_interval(horde.store(), s);
                horde.store_mut().cooldown[i] = (block.attack_interval_ms as f32 * pace * hz / 1000.0).round() as u16;
                swings.push(Swing { entity_id: MONSTER_ENTITY_BASE + s as u32, facing: facing_of(cx_ - mx, cy_ - my), target: id });
                continue;
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
            } else if f.procs().veil > 0 && f.cards.veil_cd == 0 {
                // The Sandman's card: the sand takes it.
                f.cards.veil_cd = (f.procs().veil as f32 * hz) as u32;
                hits.push(HitEvent { target: id, amount: 0, flags: hit::GUARD });
            } else if f.mods.catch > 0.0 {
                // Blade Stop: the blow is caught between the palms; its
                // attacker and every monster close by are Stunned (bosses
                // shrug it off), and the buff is spent.
                hits.push(HitEvent { target: id, amount: 0, flags: hit::GUARD });
                let (r, ticks) = (f.mods.catch, ticks16(f.mods.catch_secs, hz));
                f.spend_catch();
                let mut near = vec![s];
                Area::Circle { x: p.x, y: p.y, r }.monsters(horde, &mut near);
                for &m in &near {
                    if horde.store().flags[m as usize] & FLAG_BOSS == 0 && !horde.is_plant(m) {
                        inflict(horde, m, Condition::Stunned, ticks, 0);
                    }
                }
            } else if f.mods.parry > 0 && rng.chance(f.mods.parry) {
                // Parry: the blade turns it aside and answers at once.
                hits.push(HitEvent { target: id, amount: 0, flags: hit::GUARD });
                f.riposte = true;
                f.attack_cd = 0;
                card_guarded(f, horde, rng, s, true, hz, &mut hits);
            } else if f.mods.block > 0 && rng.chance(f.mods.block) {
                // Auto Guard: the shield takes it.
                hits.push(HitEvent { target: id, amount: 0, flags: hit::GUARD });
                card_guarded(f, horde, rng, s, true, hz, &mut hits);
            } else if f.procs().dodge > 0 && rng.chance(f.procs().dodge as u32) {
                // The Creamy's card: a flutter aside.
                hits.push(HitEvent { target: id, amount: 0, flags: hit::MISS });
                card_guarded(f, horde, rng, s, false, hz, &mut hits);
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
                        if !f.procs().is_none() {
                            card_struck(f, horde, rng, s, hz, &mut hits);
                        }
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
        if new_wave {
            // Dead homunculi come back with the wave, as players do.
            self.companions.new_wave();
        }
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
                    f.card_tick(hz);
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
                    // The Hode's card: dug in, it heals.
                    let still_regen = f.procs().still_regen;
                    if still_regen > 0 && f.cards.settled && f.hp < f.max_hp {
                        f.regen_hp += f.max_hp as f32 * still_regen as f32 / 1000.0 / hz;
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
                        f.hp = (f.max_hp / if f.procs().deathless { 2 } else { 4 }).max(1);
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
        // Companion records first: their hits below name their entities.
        self.companion_messages(map, out);
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
                (f.status.0 as u32 | f.veil()) as i32,
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
            if std::mem::take(&mut f.quiver_dirty) {
                let msg = ServerMsg::Quiver { entity_id: id, element: f.arrow as u8 };
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
                    let amount = std::mem::take(&mut f.zeny);
                    // The hub's copy of the bag is the purse skills pay from
                    // (Mammonite, `cast.rs`): what is earned is spendable.
                    p.profile.bag.earn(amount);
                    out.push((uid, HubEvent::Zeny { amount }));
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
        let quiver = f.quiver_msg(p.profile.character_id);
        for o in self.players.iter().filter(|o| o.map == p.map) {
            out.push((o.profile.user_id, HubEvent::Control(mine.clone())));
            if let Some(q) = &quiver {
                out.push((o.profile.user_id, HubEvent::Control(q.clone())));
            }
            if o.profile.user_id != p.profile.user_id
                && let Some(of) = o.fighter.as_ref()
            {
                out.push((p.profile.user_id, HubEvent::Control(of.vitals(o.profile.character_id))));
                if let Some(q) = of.quiver_msg(o.profile.character_id) {
                    out.push((p.profile.user_id, HubEvent::Control(q)));
                }
                if matches!(of.state, Vital::Downed { .. }) {
                    let hz = self.config.tick_hz as f32;
                    out.push((p.profile.user_id, HubEvent::Control(of.revive_state(o.profile.character_id, hz))));
                }
            }
        }
        out.push((p.profile.user_id, HubEvent::Control(f.progress())));
    }
}


/// Seconds of the conditions the cards put on monsters.
const CARD_COND_SECS: f32 = 3.0;
const CARD_STING_SECS: f32 = 4.0;

/// What the worn cards do when a basic attack lands on monster `s` (each
/// rolls only when its card is worn, so a fighter without cards keeps the
/// dice stream it had).
fn card_hit(o: &Procs, f: &mut Fighter, horde: &mut Horde, rng: &mut combat::Rng, s: u16, crit: bool, hz: f32, hits: &mut Vec<HitEvent>, me: u32) {
    let t = ticks16(CARD_COND_SECS, hz);
    if o.ignite > 0 && rng.chance(o.ignite as u32) {
        inflict(horde, s, Condition::Burning, t, 1);
    }
    if o.chill > 0 && rng.chance(o.chill as u32) {
        inflict(horde, s, Condition::Slowed, t, 1);
    }
    if crit && o.crit_sting {
        let t = ticks16(CARD_STING_SECS, hz);
        inflict(horde, s, Condition::Poisoned, t, 1);
        inflict(horde, s, Condition::Slowed, t, 1);
    }
    if crit && o.crit_zeny {
        let n = horde.store().level[s as usize] as u64;
        f.add_zeny(n);
        f.run_zeny += n;
    }
    if o.leech > 0 && f.hp < f.max_hp && rng.chance(o.leech as u32) {
        let h = rng.die(4) as i32;
        f.hp = (f.hp + h).min(f.max_hp);
        hits.push(HitEvent { target: me, amount: h as u32, flags: hit::HEAL });
    }
}

/// Damage to monster `s` that never kills it (thorns, ripostes): the kill
/// list of this tick is already done.
fn sting_back(horde: &mut Horde, s: u16, amount: i32, hits: &mut Vec<HitEvent>) {
    let hp = &mut horde.store_mut().hp[s as usize];
    let d = amount.min(*hp - 1).max(0);
    *hp -= d;
    if d > 0 {
        hits.push(HitEvent { target: MONSTER_ENTITY_BASE + s as u32, amount: d as u32, flags: damage_type_bits(DamageType::Physical) });
    }
}

/// A melee blow blocked (`blocked`) or dodged: the cards' answers.
fn card_guarded(f: &mut Fighter, horde: &mut Horde, rng: &mut combat::Rng, s: u16, blocked: bool, hz: f32, hits: &mut Vec<HitEvent>) {
    let o = *f.procs();
    if o.counter > 0 {
        f.cards.counter = true;
    }
    if !blocked && o.dodge_rush {
        f.cards.rush = (2.0 * hz) as u32;
    }
    if blocked && o.riposte > 0 {
        let d = rng.die(o.riposte as u32) as i32;
        sting_back(horde, s, d, hits);
    }
}

/// A melee blow landed on the fighter: thorns and what the shield spits back.
fn card_struck(f: &mut Fighter, horde: &mut Horde, rng: &mut combat::Rng, s: u16, hz: f32, hits: &mut Vec<HitEvent>) {
    let o = *f.procs();
    if o.thorns > 0 {
        let d = rng.die(o.thorns as u32) as i32;
        sting_back(horde, s, d, hits);
    }
    if o.slow_back > 0 && rng.chance(o.slow_back as u32) {
        inflict(horde, s, Condition::Slowed, ticks16(CARD_COND_SECS, hz), 1);
    }
    if o.blind_back > 0 && rng.chance(o.blind_back as u32) {
        inflict(horde, s, Condition::Blinded, ticks16(CARD_STING_SECS, hz), 1);
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

    /// Magic items worn add their enchantment to the fighter's mods (on top
    /// of the passives and buffs), their HP and every save; taking them off
    /// takes it away.
    #[test]
    fn magic_items_enchant_whoever_wears_them() {
        let mut f = Fighter::new(Class::Mage, 10, 0, "");
        let plain = (f.mods.speed, f.max_hp, f.gear.saves);
        let mut bag = Bag::starting(Class::Mage, "");
        bag.equip[items::Slot::Shoes as usize] = "striding_boots".into();
        bag.equip[items::Slot::Accessory1 as usize] = "protection_ring".into();
        bag.equip[items::Slot::Weapon as usize] = "war_mage_wand".into();
        f.wear(&bag);
        assert!((f.mods.speed - plain.0 - 0.1).abs() < 1e-6, "boots: +10% speed");
        assert_eq!(f.mods.spell_dc, 1, "the wand");
        assert_eq!(f.mods.ac, 1, "the ring's AC");
        assert_eq!(f.max_hp, plain.1 + 8, "the boots' HP");
        assert_eq!(f.gear.saves, plain.2.map(|s| s + 1), "the ring: every save");
        assert_eq!(f.weapon.id, "war_mage_wand");
        // A passive set later keeps the gear's enchantment.
        f.set_passive(Mods { haste: 0.2, ..Mods::default() });
        assert!((f.mods.speed - plain.0 - 0.1).abs() < 1e-6 && (f.mods.haste - 0.2).abs() < 1e-6);
        f.wear(&Bag::starting(Class::Mage, ""));
        assert_eq!((f.mods.spell_dc, f.mods.ac), (0, 0), "off with the items");
    }
}
