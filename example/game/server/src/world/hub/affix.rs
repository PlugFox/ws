//! Monster affixes in a fight (COMBAT.md "Monster affixes"): what the
//! bits of `HordeStorage::affix` do on a stage. The table and the odds are
//! `combat::affixes`, rolling and HP the horde's (`horde/affixes.rs`).
//!
//! - **Damage taken** ([`absorb`], inside `cast::wound`): the shield soaks
//!   first, Armoured cuts physical damage, Thorns keeps the tally.
//! - **Melee** ([`melee`], the contact blow of `fight.rs`): Vampiric heals,
//!   an enchantment adds its element.
//! - **Each second** ([`HubInstance::affix_step`], staggered by slot as
//!   ailments are): regeneration, the shield's recharge, frenzy, the
//!   mirror's split, Thorns' pulse; and, near a hunted player, the timed
//!   tricks: a blink, a summons, an arcane sweep, poison pools.
//! - **Death** ([`on_death`]): the enchantments' novas and the Volatile
//!   blast, each a telegraphed hazard (`BOSS_ZONES`) that lands later, so
//!   whoever steps out takes nothing.
//! - **Rewards** ([`reward`]): the star bonus, nothing for minions.
//!
//! Every damaging trick is a hazard with a telegraph and a saving throw
//! for half, as monster skills are (`brain.rs`): nothing is unavoidable.

use super::area::Area;
use super::aim::BODY;
use super::brain::Hazard;
use super::cast::{ticks16, ticks_of};
use super::fight::{damage_type_bits, Fighter};
use super::{HubEvent, HubInstance, MapIdx, Outgoing};
use crate::world::combat::affixes::{AffixFlags, AffixReward};
use crate::world::combat::rules::saving_throw;
use crate::world::combat::{Archetype, Condition, DamageType, Dice, MonsterBlock, Rng};
use crate::world::horde::affixes::{interval, shield_of, MAX_MINIONS};
use crate::world::horde::storage::{HordeStorage, AFFIX_FRENZIED, AFFIX_SHIELD_BROKEN, AFFIX_SPLIT, FLAG_BOSS, FLAG_FULL, FLAG_MINION};
use crate::world::horde::Horde;
use crate::world::protocol::{look, AffixEvent, BossZone, HitEvent, MonsterAffix, ServerMsg, MONSTER_ENTITY_BASE};

/// Armoured: share of physical damage that gets through (percent).
pub const ARMOUR_PCT: i32 = 60;
/// Regeneration per second, per mille of max HP (elites, bosses).
const REGEN_PERMILLE: i32 = 20;
const REGEN_ELITE_PERMILLE: i32 = 10;
const REGEN_BOSS_PERMILLE: i32 = 3;
/// A Shielded monster's shield comes back after this many quiet seconds.
pub const SHIELD_QUIET_SECS: u8 = 5;
/// Frenzied below this share of its HP (percent).
pub const FRENZY_BELOW_PCT: i32 = 35;
/// Thorns: the ring (cells, to the player's body) and the reflected share.
pub const THORNS_RADIUS: f32 = 2.5;
pub const THORNS_PCT: i32 = 25;
/// ... never more than this share of a player's max HP per pulse (percent).
const THORNS_CAP_PCT: i32 = 10;
/// Seconds between a Teleporting monster's blinks, the warning before
/// one, the distances it blinks from and how far from the player it lands.
pub const BLINK_EVERY: u16 = 8;
pub const BLINK_WARN_SECS: f32 = 0.8;
const BLINK_RANGE: (f32, f32) = (5.0, 16.0);
const BLINK_GAP: f32 = 2.5;
/// Summoner: every so many seconds, this many minions.
pub const SUMMON_EVERY: u16 = 10;
pub const SUMMON_COUNT: u32 = 2;
/// Arcane beam: every so many seconds within reach, its sweeps.
pub const ARCANE_EVERY: u16 = 7;
const ARCANE_REACH: f32 = 8.0;
const ARCANE_LEN: f32 = 7.0;
const ARCANE_WIDTH: f32 = 1.2;
/// Delays of the four sweeps (ms) and their turn between them (degrees).
const ARCANE_BEATS: [u32; 4] = [1000, 1350, 1700, 2050];
const ARCANE_TURN: f32 = 50.0;
/// Plagued: a pool's radius, life and the poison it gives.
pub const POOL_RADIUS: f32 = 1.0;
pub const POOL_SECS: f32 = 4.0;
const POOL_POISON_SECS: f32 = 3.0;
/// Death blasts: radius and delay (s) of the novas and the Volatile blast.
pub const NOVA_RADIUS: f32 = 3.0;
pub const NOVA_DELAY: f32 = 1.2;
pub const VOLATILE_RADIUS: f32 = 3.5;
pub const VOLATILE_DELAY: f32 = 2.0;
const BOLT_RADIUS: f32 = 1.3;
const BOLT_DELAYS: [f32; 3] = [0.9, 1.2, 1.5];

/// What the stage keeps between ticks for affixes.
#[derive(Debug, Default)]
pub(super) struct AffixStage {
    /// Blinks warned of, landing later.
    blinks: Vec<Blink>,
    /// One-off moments for the clients, sent with the next step.
    events: Vec<AffixEvent>,
    /// Telegraphed blasts and sweeps for the clients (`BOSS_ZONES`).
    zones: Vec<ServerMsg>,
    /// Scratch for the records that changed.
    records: Vec<MonsterAffix>,
}

#[derive(Clone, Copy, Debug)]
struct Blink {
    slot: u16,
    generation: u8,
    at: u32,
    x: f32,
    y: f32,
}

/// Damage reaching an affixed monster (inside `cast::wound`, before it
/// comes off its HP): the shield soaks it first, Armoured cuts physical
/// damage, Thorns and the shield's quiet clock take note. Returns what is
/// left for its HP.
pub(super) fn absorb(horde: &mut Horde, s: u16, amount: i32, kind: DamageType) -> i32 {
    let i = s as usize;
    let bits = horde.store().affix[i];
    if bits == 0 || amount <= 0 {
        return amount;
    }
    let f = AffixFlags(bits);
    let mut amount = amount;
    if f.contains(AffixFlags::ARMOURED) && kind == DamageType::Physical {
        amount = (amount * ARMOUR_PCT / 100).max(1);
    }
    let st = horde.store_mut();
    st.affix_quiet[i] = 0;
    let mut broke = false;
    if f.contains(AffixFlags::SHIELDED) && st.shield[i] > 0 {
        let soak = st.shield[i].min(amount);
        st.shield[i] -= soak;
        amount -= soak;
        if st.shield[i] == 0 {
            st.affix_state[i] |= AFFIX_SHIELD_BROKEN;
            broke = true;
        }
    }
    if f.contains(AffixFlags::THORNS) {
        st.affix_taken[i] = st.affix_taken[i].saturating_add(amount);
    }
    if broke {
        horde.affix_changed(s);
    }
    amount
}

/// An affixed monster's contact blow landed `dealt` on `f` (after its own
/// damage): Vampiric heals the monster, an enchantment adds its element
/// (Cold slows on a failed save). Extra hit numbers go to `hits`.
#[allow(clippy::too_many_arguments)]
pub(super) fn melee(horde: &mut Horde, s: u16, block: &MonsterBlock, f: &mut Fighter, target: u32, dealt: i32, rng: &mut Rng, hz: f32, hits: &mut Vec<HitEvent>) {
    let i = s as usize;
    let bits = AffixFlags(horde.store().affix[i]);
    if bits.is_empty() {
        return;
    }
    let evil = horde.catalog().get(horde.store().kind[i]).evil;
    let mut drained = dealt;
    let bonus = (block.level / 5) as i32;
    let enchant = if bits.contains(AffixFlags::FIRE) {
        Some((Dice::new(1, 6, bonus), DamageType::Fire, None))
    } else if bits.contains(AffixFlags::COLD) {
        Some((Dice::new(1, 4, bonus), DamageType::Cold, Some((Condition::Slowed, ticks16(2.0, hz)))))
    } else if bits.contains(AffixFlags::LIGHTNING) {
        Some((Dice::new(1, 10, bonus), DamageType::Lightning, None))
    } else {
        None
    };
    if let Some((dice, kind, rider)) = enchant
        && f.is_up()
    {
        let taken = f.take_from(dice.roll(rng, false), false, evil);
        drained += taken;
        hits.push(HitEvent { target, amount: taken as u32, flags: damage_type_bits(kind) });
        if let Some((c, ticks)) = rider {
            let dc = 10 + block.save;
            if f.status.saves_fail() || !saving_throw(rng, f.save_against(Some(c)), dc, 0) {
                f.afflict(c, ticks);
            }
        }
    }
    if bits.contains(AffixFlags::VAMPIRIC) && drained > 0 {
        let max = horde.block(s).hp;
        let hp = &mut horde.store_mut().hp[i];
        if *hp > 0 {
            *hp = (*hp + drained).min(max);
        }
    }
}

/// The attack interval factor of an affixed monster (Hasted, frenzied).
pub(super) fn attack_interval(store: &HordeStorage, s: u16) -> f32 {
    let i = s as usize;
    interval(store.affix[i], store.affix_state[i])
}

/// What a kill of `s` pays: the stars' bonus, nothing for a minion.
pub(super) fn reward(store: &HordeStorage, s: u16) -> AffixReward {
    let i = s as usize;
    if store.flags[i] & FLAG_MINION != 0 {
        return AffixReward::NOTHING;
    }
    AffixReward::of(AffixFlags(store.affix[i]))
}

/// Damage dice of a death blast: the monster's own blow, doubled
/// (`mult` 3 for Volatile).
fn blast_dice(block: &MonsterBlock, mult: u32) -> Dice {
    Dice { count: block.damage.count.max(1) * mult, ..block.damage }
}

/// `s` just died with its affixes: the novas and the Volatile blast are
/// marked now and land later as hazards (`hazards`); their telegraphs go
/// to the clients with the next step.
#[allow(clippy::too_many_arguments)]
pub(super) fn on_death(horde: &Horde, s: u16, now: u32, hz: f32, rng: &mut Rng, stage: &mut AffixStage, hazards: &mut Vec<Hazard>) {
    let i = s as usize;
    let st = horde.store();
    let bits = AffixFlags(st.affix[i]);
    if !bits.intersects(AffixFlags::DEATH_BLASTS) {
        return;
    }
    let (x, y) = (st.x[i], st.y[i]);
    let block = horde.block(s);
    let dc = 10 + block.save;
    let map = horde.map();
    let entity = MONSTER_ENTITY_BASE + s as u32;
    let mut zones = Vec::new();
    let mut blast = |area: Area, delay: f32, dice: Dice, kind: DamageType, look: u8, rider: Option<(Condition, u16)>| {
        let at = now + ticks_of(delay, hz).max(1);
        hazards.push(Hazard::damage(0, map, area, at, at, 0, dc, dice, kind, rider));
        zones.push(BossZone { area: area.telegraph(0, look, 0, 0), delay_ms: (delay * 1000.0).round() as u32, flags: 0 });
    };
    let nova = Area::Circle { x, y, r: NOVA_RADIUS };
    let ability = if bits.contains(AffixFlags::FIRE) {
        blast(nova, NOVA_DELAY, blast_dice(&block, 2), DamageType::Fire, look::FIRE, Some((Condition::Burning, ticks16(3.0, hz))));
        "affix_fire_nova"
    } else if bits.contains(AffixFlags::COLD) {
        blast(nova, NOVA_DELAY, blast_dice(&block, 2), DamageType::Cold, look::COLD, Some((Condition::Slowed, ticks16(2.0, hz))));
        "affix_frost_nova"
    } else if bits.contains(AffixFlags::LIGHTNING) {
        for d in BOLT_DELAYS {
            let a = rng.die(360) as f32 * std::f32::consts::TAU / 360.0;
            let r = 2.5 * (rng.die(1000) as f32 / 1000.0).sqrt();
            let (bx, by) = (x + a.cos() * r, y + a.sin() * r);
            let (bx, by) = if horde.walkable(bx, by) { (bx, by) } else { (x, y) };
            blast(Area::Circle { x: bx, y: by, r: BOLT_RADIUS }, d, blast_dice(&block, 2), DamageType::Lightning, look::LIGHTNING, None);
        }
        "affix_lightning_bolts"
    } else {
        blast(Area::Circle { x, y, r: VOLATILE_RADIUS }, VOLATILE_DELAY, blast_dice(&block, 3), DamageType::Force, look::DAMAGE, None);
        stage.events.push(AffixEvent { entity, event: AffixEvent::FUSE, x, y });
        "affix_volatile"
    };
    stage.zones.push(ServerMsg::BossZones { caster: entity, ability: ability.into(), zones });
}

impl HubInstance {
    /// The affix table and every affixed monster alive, for a client
    /// joining the stage.
    pub(super) fn affix_hello(&self, user_id: u32, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_ref() else { return };
        out.push((user_id, HubEvent::Control(crate::world::combat::affixes::defs_msg())));
        let mut monsters = Vec::new();
        horde.affix_records(&mut monsters);
        if !monsters.is_empty() {
            out.push((user_id, HubEvent::Control(ServerMsg::MonsterAffixes { monsters })));
        }
    }

    /// One affix step, inside [`HubInstance::fight`] after the monsters'
    /// skills and before their blows: due blinks, each affixed monster's
    /// second, and what the clients must hear.
    pub(super) fn affix_step(&mut self, out: &mut Outgoing, hits: &mut Vec<HitEvent>) {
        let Some(horde) = self.horde.as_mut() else { return };
        let map = horde.map();
        let hz = self.config.tick_hz as f32;
        let now = self.tick;
        let second = (hz.round() as u32).max(1);
        let rng = &mut self.dice;
        let stage = &mut self.affix;

        // Blinks warned of a moment ago land (unless it died or is held).
        let mut k = 0;
        while k < stage.blinks.len() {
            let b = stage.blinks[k];
            if b.at > now {
                k += 1;
                continue;
            }
            stage.blinks.swap_remove(k);
            let st = horde.store();
            let i = b.slot as usize;
            if !st.is_alive(b.slot) || st.generation[i] != b.generation || st.hp[i] <= 0 || st.status_of(b.slot).held() {
                continue;
            }
            let entity = MONSTER_ENTITY_BASE + b.slot as u32;
            stage.events.push(AffixEvent { entity, event: AffixEvent::BLINK, x: st.x[i], y: st.y[i] });
            horde.place(b.slot, b.x, b.y);
        }

        // Hunted, standing players on the map: the tricks' targets.
        let ups: Vec<(f32, f32)> = self
            .players
            .iter()
            .filter(|p| p.map == map && p.fighter.as_ref().is_some_and(|f| f.hunted()))
            .map(|p| (p.x, p.y))
            .collect();

        // Each affixed monster's second (staggered by slot).
        for a in 0..horde.store().alive().len() {
            let s = horde.store().alive()[a];
            let i = s as usize;
            if horde.store().affix[i] == 0 || !(now + s as u32).is_multiple_of(second) || horde.store().hp[i] <= 0 {
                continue;
            }
            let thorns = beat(horde, s, now, hz, rng, &ups, stage, &mut self.hazards, &mut self.next_zone);
            if thorns > 0 {
                prick(&mut self.players, map, horde.store().x[i], horde.store().y[i], thorns, hz, hits);
                stage.events.push(AffixEvent { entity: MONSTER_ENTITY_BASE + s as u32, event: AffixEvent::THORNS, x: horde.store().x[i], y: horde.store().y[i] });
            }
        }

        // To everyone on the map: new and changed records, events, zones.
        stage.records.clear();
        horde.take_affix_news(&mut stage.records);
        let mut msgs: Vec<ServerMsg> = Vec::new();
        if !stage.records.is_empty() {
            msgs.push(ServerMsg::MonsterAffixes { monsters: stage.records.clone() });
        }
        if !stage.events.is_empty() {
            msgs.push(ServerMsg::AffixEvents { events: std::mem::take(&mut stage.events) });
        }
        msgs.append(&mut stage.zones);
        if msgs.is_empty() {
            return;
        }
        for p in self.players.iter().filter(|p| p.map == map) {
            for m in &msgs {
                out.push((p.profile.user_id, HubEvent::Control(m.clone())));
            }
        }
    }
}

/// Thorns: every standing player whose body is inside the ring around
/// `(x, y)` takes `amount` (at most a tenth of their HP).
fn prick(players: &mut [super::HubPlayer], map: MapIdx, x: f32, y: f32, amount: i32, hz: f32, hits: &mut Vec<HitEvent>) {
    let r = THORNS_RADIUS + BODY;
    for p in players.iter_mut().filter(|p| p.map == map) {
        let (px, py, id) = (p.x, p.y, p.profile.character_id);
        let Some(f) = p.fighter.as_mut() else { continue };
        if !f.is_up() || (px - x).powi(2) + (py - y).powi(2) > r * r {
            continue;
        }
        let hit = amount.min((f.max_hp * THORNS_CAP_PCT / 100).max(1));
        let taken = f.take(hit, true);
        f.quiet = 0;
        hits.push(HitEvent { target: id, amount: taken as u32, flags: damage_type_bits(DamageType::Physical) });
        f.fall_if_spent(hz);
    }
}

/// One second of an affixed monster. Returns the damage Thorns reflects
/// this second (0: none).
#[allow(clippy::too_many_arguments)]
fn beat(
    horde: &mut Horde,
    s: u16,
    now: u32,
    hz: f32,
    rng: &mut Rng,
    ups: &[(f32, f32)],
    stage: &mut AffixStage,
    hazards: &mut Vec<Hazard>,
    next_zone: &mut u32,
) -> i32 {
    let i = s as usize;
    let block = horde.block(s);
    let max = block.hp.max(1);
    let entity = MONSTER_ENTITY_BASE + s as u32;
    let map = horde.map();
    let archetype = horde.catalog().get(horde.store().kind[i]).archetype;
    let st = horde.store_mut();
    let f = AffixFlags(st.affix[i]);
    let boss = st.flags[i] & FLAG_BOSS != 0;
    st.affix_beat[i] = st.affix_beat[i].wrapping_add(1);
    st.affix_quiet[i] = st.affix_quiet[i].saturating_add(1);
    let clock = st.affix_beat[i];
    let phase = s % 64;
    let status = st.status_of(s);
    let (x, y) = (st.x[i], st.y[i]);
    let mut changed = false;

    // Regenerating: not while burning or poisoned.
    if f.contains(AffixFlags::REGENERATING) && st.hp[i] < max && !status.ailing() {
        let rate = if boss {
            REGEN_BOSS_PERMILLE
        } else if archetype == Archetype::Elite {
            REGEN_ELITE_PERMILLE
        } else {
            REGEN_PERMILLE
        };
        st.hp[i] = (st.hp[i] + (max * rate / 1000).max(1)).min(max);
    }
    // Shielded: back in full after a quiet spell (the clients hear of it
    // when it had broken).
    let full = shield_of(&block);
    if f.contains(AffixFlags::SHIELDED) && st.shield[i] < full && st.affix_quiet[i] >= SHIELD_QUIET_SECS {
        st.shield[i] = full;
        changed |= st.affix_state[i] & AFFIX_SHIELD_BROKEN != 0;
        st.affix_state[i] &= !AFFIX_SHIELD_BROKEN;
    }
    // Frenzied: once, below a third of its HP.
    if f.contains(AffixFlags::FRENZIED) && st.affix_state[i] & AFFIX_FRENZIED == 0 && st.hp[i] * 100 < max * FRENZY_BELOW_PCT {
        st.affix_state[i] |= AFFIX_FRENZIED;
        stage.events.push(AffixEvent { entity, event: AffixEvent::FRENZY, x, y });
        changed = true;
    }
    let thorns = if f.contains(AffixFlags::THORNS) {
        let t = std::mem::take(&mut st.affix_taken[i]);
        if t > 0 { (t * THORNS_PCT / 100).max(1) } else { 0 }
    } else {
        0
    };
    let moving = st.vel_x[i] != 0.0 || st.vel_y[i] != 0.0;
    let split = f.contains(AffixFlags::MIRROR) && st.affix_state[i] & AFFIX_SPLIT == 0 && st.hp[i] * 2 < max;
    if split {
        st.affix_state[i] |= AFFIX_SPLIT;
    }
    let near = st.flags[i] & FLAG_FULL != 0;
    let held = status.held();
    if changed {
        horde.affix_changed(s);
    }

    // Mirror: a copy with the same HP, a cell aside.
    if split {
        let (kind, level, hp) = (horde.store().kind[i], horde.store().level[i] as u32, horde.store().hp[i]);
        let a = rng.die(360) as f32 * std::f32::consts::TAU / 360.0;
        let spot = [(x + a.cos(), y + a.sin()), (x - a.cos(), y - a.sin()), (x, y)];
        if let Some(c) = spot.iter().find_map(|&(cx, cy)| horde.spawn_minion(kind, cx, cy, level, f.without(AffixFlags::MIRROR))) {
            horde.store_mut().hp[c as usize] = hp.max(1);
            stage.events.push(AffixEvent { entity, event: AffixEvent::SPLIT, x, y });
        }
    }

    // The timed tricks: only near a hunted player, never while held.
    let nearest = ups.iter().map(|&(px, py)| ((px - x).powi(2) + (py - y).powi(2), px, py)).min_by(|a, b| a.0.total_cmp(&b.0));
    let Some((d2, px, py)) = nearest else { return thorns };
    if !near || held {
        return thorns;
    }
    let dist = d2.sqrt();
    let due = |every: u16| clock % every == phase % every;
    if f.contains(AffixFlags::TELEPORTING) && due(BLINK_EVERY) && (BLINK_RANGE.0..=BLINK_RANGE.1).contains(&dist) {
        // Lands between the player and where it stood, a few cells short.
        let (ux, uy) = ((x - px) / dist, (y - py) / dist);
        let (tx, ty) = (px + ux * BLINK_GAP, py + uy * BLINK_GAP);
        if horde.walkable(tx, ty) {
            stage.events.push(AffixEvent { entity, event: AffixEvent::BLINK_MARK, x: tx, y: ty });
            stage.blinks.push(Blink { slot: s, generation: horde.store().generation[i], at: now + ticks_of(BLINK_WARN_SECS, hz), x: tx, y: ty });
            // It stands still while the spot glows (the blow waits too).
            let st = horde.store_mut();
            st.afflict(s, Condition::Rooted, ticks16(BLINK_WARN_SECS, hz) + 1);
        }
    }
    if f.contains(AffixFlags::SUMMONER) && due(SUMMON_EVERY) && dist <= BLINK_RANGE.1 && (horde.minions() as u16) < MAX_MINIONS {
        let kind = horde.minion_kind(s);
        let level = horde.store().level[i] as u32;
        let mut called = 0;
        for n in 0..SUMMON_COUNT * 3 {
            if called == SUMMON_COUNT || horde.minions() as u16 >= MAX_MINIONS {
                break;
            }
            let a = (n as f32 * 2.4) + rng.die(100) as f32 / 100.0;
            if horde.spawn_minion(kind, x + a.cos() * 1.5, y + a.sin() * 1.5, level, AffixFlags::NONE).is_some() {
                called += 1;
            }
        }
        if called > 0 {
            stage.events.push(AffixEvent { entity, event: AffixEvent::SUMMON, x, y });
        }
    }
    if f.contains(AffixFlags::ARCANE) && due(ARCANE_EVERY) && dist <= ARCANE_REACH {
        // Rooted through the sweep, so the beam stays where it is drawn.
        let span = *ARCANE_BEATS.last().unwrap_or(&0) as f32 / 1000.0;
        horde.store_mut().afflict(s, Condition::Rooted, ticks16(span + 0.2, hz));
        let base = (py - y).atan2(px - x) - (ARCANE_TURN * 1.5).to_radians();
        let dc = 10 + block.save;
        let dice = blast_dice(&block, 2);
        let mut zones = Vec::with_capacity(ARCANE_BEATS.len());
        for (n, &ms) in ARCANE_BEATS.iter().enumerate() {
            let a = base + (ARCANE_TURN * n as f32).to_radians();
            let dir = (a.cos(), a.sin());
            let area = Area::line(x, y, dir, ARCANE_LEN, ARCANE_WIDTH);
            let at = now + ticks_of(ms as f32 / 1000.0, hz).max(1);
            hazards.push(Hazard::damage(0, map, area, at, at, 0, dc, dice, DamageType::Force, None));
            zones.push(BossZone { area: area.telegraph(0, look::CURSE, 0, 0), delay_ms: ms, flags: 0 });
        }
        stage.zones.push(ServerMsg::BossZones { caster: entity, ability: "affix_arcane_beam".into(), zones });
    }
    if f.contains(AffixFlags::PLAGUED) && moving {
        let id = *next_zone;
        *next_zone = next_zone.wrapping_add(1).max(1);
        let area = Area::Circle { x, y, r: POOL_RADIUS };
        let (at, span) = (now + 1, ticks_of(POOL_SECS, hz));
        let dc = 10 + block.save;
        hazards.push(Hazard::condition(id, map, area, at, at + span, second_ticks(hz), dc, Condition::Poisoned, ticks16(POOL_POISON_SECS, hz)));
        let span_ms = (POOL_SECS * 1000.0) as u32;
        stage.zones.push(ServerMsg::BossZones {
            caster: entity,
            ability: "affix_plague_pool".into(),
            zones: vec![BossZone { area: area.telegraph(id, look::POISON, span_ms, 0), delay_ms: 0, flags: 0 }],
        });
    }
    thorns
}

fn second_ticks(hz: f32) -> u32 {
    (hz.round() as u32).max(1)
}
