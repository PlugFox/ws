//! The Equipment window's combat numbers (`PLAYER_SHEET`): what the fight
//! rolls with right now, read from the [`Fighter`] through the same
//! functions combat uses ([`Fighter::armour_parts`], [`Fighter::to_hit_with`],
//! [`Fighter::swing_dice`], [`Fighter::pace`], ...), so the window cannot
//! drift from the rules. Every number comes with its base: the gear and
//! the passives without buffs, conditions or situational cards.
//!
//! - The player's own sheet goes to them on arrival and whenever it
//!   changes on a mission (a buff, an aura, a condition, a level).
//! - An inspected player's sheet follows `PLAYER_INSPECT`; the client asks
//!   again while the window stays open.
//!
//! In town (no fighter) the sheet is that of the fighter the character
//! would arrive as ([`Fighter::arrive`]): gear and passives, no buffs.

use super::fight::{ArmourParts, Fighter};
use super::{HubEvent, HubInstance, HubPlayer, MapIdx, Outgoing};
use crate::world::combat::{self, Dice, Save};
use crate::world::protocol::ServerMsg;
use serde::Serialize;

/// Missions check the sheet for changes this often (ticks).
const SHEET_EVERY: u32 = 3;

/// Saving throw bonuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Saves {
    pub body: i32,
    pub reflex: i32,
    pub mind: i32,
}

/// A player's live combat numbers (`*_base`: gear and passives only).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Sheet {
    pub level: u32,
    pub max_hp: i32,
    pub max_sp: i32,
    /// Armour class monsters roll against ([`Fighter::armour`]) and its terms.
    pub ac: i32,
    pub ac_base: i32,
    pub ac_parts: ArmourParts,
    /// Basic attack bonus, and Bless's extra die (0 = none).
    pub to_hit: i32,
    pub to_hit_base: i32,
    pub to_hit_die: u32,
    /// Advantage of its own attack rolls (-1 disadvantage, 0, 1).
    pub advantage: i8,
    /// A d20 this high or more is a critical hit.
    pub crit_from: u32,
    /// Basic attack dice per hit, and the extra dice buffs add.
    #[serde(serialize_with = "dice")]
    pub damage: Dice,
    #[serde(serialize_with = "dice")]
    pub damage_base: Dice,
    #[serde(serialize_with = "opt_dice")]
    pub damage_extra: Option<Dice>,
    pub damage_type: &'static str,
    /// Basic attack interval (ms) and blows per swing; percent chance of
    /// one more (Double Attack).
    pub interval_ms: u32,
    pub interval_base_ms: u32,
    pub attacks: u32,
    pub double_pct: u32,
    /// Walking speed, percent of the plain walk.
    pub speed_pct: u32,
    pub speed_base_pct: u32,
    /// Damage taken lowered by this percent, physical hits by this much;
    /// percent of melee blows blocked and parried.
    pub resist_pct: u32,
    pub reduce: i32,
    pub block_pct: u32,
    pub parry_pct: u32,
    pub saves: Saves,
    /// Stunned: every save fails.
    pub saves_fail: bool,
    pub save_dc: i32,
    pub save_dc_base: i32,
    pub proficiency: i32,
    pub class_mod: i32,
    pub k: u32,
}

fn dice_text(d: &Dice) -> String {
    match (d.count, d.sides) {
        // Maximize Power: dice of one side, a flat number.
        (n, 1) => (n as i32 + d.bonus).to_string(),
        (0, _) => d.bonus.to_string(),
        _ if d.bonus == 0 => format!("{}d{}", d.count, d.sides),
        _ => format!("{}d{}{:+}", d.count, d.sides, d.bonus),
    }
}

fn dice<S: serde::Serializer>(d: &Dice, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&dice_text(d))
}

fn opt_dice<S: serde::Serializer>(d: &Option<Dice>, s: S) -> Result<S::Ok, S::Error> {
    match d {
        Some(d) => s.serialize_str(&dice_text(d)),
        None => s.serialize_none(),
    }
}

fn pct(f: f32) -> u32 {
    (f * 100.0).round().max(0.0) as u32
}

impl Sheet {
    /// What `f` fights with now.
    pub fn of(f: &Fighter) -> Sheet {
        let (m, base) = (f.mods, f.base_mods());
        let level = f.level;
        let ac_parts = f.armour_parts();
        let damage = |m: &combat::Mods| {
            let (d, _) = f.swing_dice(m);
            Dice { bonus: d.bonus + f.swing_flat(m), ..d }
        };
        let (_, kind) = f.swing_dice(&m);
        let interval = |m: &combat::Mods| (f.weapon.interval_ms as f32 * m.interval()).round() as u32;
        let status = f.status;
        let mut crit = m;
        crit.crit += f.weapon.kind.crit();
        Sheet {
            level,
            max_hp: f.max_hp,
            max_sp: f.max_sp,
            ac: ac_parts.total(),
            ac_base: ac_parts.worn + ac_parts.items,
            ac_parts,
            to_hit: f.to_hit_with(&m),
            to_hit_base: f.to_hit_with(&base),
            to_hit_die: m.to_hit_die,
            advantage: (i8::from(m.advantage) + status.advantage()).clamp(-1, 1),
            crit_from: crit.crit_from(),
            damage: damage(&m),
            damage_base: damage(&base),
            damage_extra: m.damage,
            damage_type: kind.ro_element(),
            interval_ms: interval(&m),
            interval_base_ms: interval(&base),
            attacks: f.class.attacks_per_action(level),
            double_pct: m.double.min(100),
            speed_pct: pct(f.pace()),
            speed_base_pct: pct(base.pace()),
            resist_pct: pct(m.resist.clamp(0.0, 1.0)),
            reduce: m.reduce,
            block_pct: m.block.min(100),
            parry_pct: m.parry.min(100),
            saves: Saves { body: f.save_bonus(Save::Body), reflex: f.save_bonus(Save::Reflex), mind: f.save_bonus(Save::Mind) },
            saves_fail: status.saves_fail(),
            save_dc: combat::save_dc(level) + m.spell_dc,
            save_dc_base: combat::save_dc(level) + base.spell_dc,
            proficiency: combat::proficiency(level),
            class_mod: combat::class_mod(level),
            k: combat::cantrip_k(level),
        }
    }

    pub fn msg(&self, entity_id: u32) -> ServerMsg {
        ServerMsg::PlayerSheet { entity_id, sheet: serde_json::to_string(self).unwrap_or_default() }
    }
}

impl HubInstance {
    /// The sheet of player `i`: their fighter's, else (town) the one they
    /// would arrive with.
    pub(super) fn sheet_of(&self, i: usize) -> Sheet {
        let p: &HubPlayer = &self.players[i];
        match p.fighter.as_ref() {
            Some(f) => Sheet::of(f),
            None => Sheet::of(&Fighter::arrive(&p.profile, &self.skills)),
        }
    }

    /// Sends player `i` their own sheet now (arrival; in town after a gear
    /// or skill change). Mission fighters are kept current by [`Self::sheets`].
    pub(super) fn send_own_sheet(&self, i: usize, out: &mut Outgoing) {
        let p = &self.players[i];
        if p.fighter.is_none() {
            out.push((p.profile.user_id, HubEvent::Control(self.sheet_of(i).msg(p.profile.character_id))));
        }
    }

    /// Each fighter's own sheet, when it changed (buffs, auras, conditions,
    /// level, HP-gated cards).
    pub(super) fn sheets(&mut self, map: MapIdx, out: &mut Outgoing) {
        if !self.tick.is_multiple_of(SHEET_EVERY) {
            return;
        }
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let Some(f) = p.fighter.as_mut() else { continue };
            let now = Sheet::of(f);
            if f.sheet_sent.as_ref() == Some(&now) {
                continue;
            }
            out.push((p.profile.user_id, HubEvent::Control(now.msg(p.profile.character_id))));
            f.sheet_sent = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::combat::{Class, Condition, Mods};

    fn json(s: &Sheet) -> serde_json::Value {
        serde_json::from_str(&serde_json::to_string(s).unwrap()).unwrap()
    }

    #[test]
    fn the_sheet_is_what_combat_rolls_with() {
        let mut f = Fighter::new(Class::Swordsman, 10, 0, "sword");
        let plain = Sheet::of(&f);
        assert_eq!(plain.ac, f.armour());
        assert_eq!((plain.ac, plain.to_hit, plain.speed_pct), (plain.ac_base, plain.to_hit_base, plain.speed_base_pct));
        // A party buff (Angelus-like AC, Blessing-like to-hit, haste, speed).
        f.buff(9000, u32::MAX, Mods { ac: 2, to_hit: 1, to_hit_die: 4, haste: 0.25, speed: 0.2, resist: 0.1, ..Mods::default() }, 0);
        let buffed = Sheet::of(&f);
        assert_eq!(buffed.ac, f.armour());
        assert_eq!((buffed.ac, buffed.ac_base, buffed.ac_parts.buffs), (plain.ac + 2, plain.ac_base, 2));
        assert_eq!((buffed.to_hit, buffed.to_hit_base, buffed.to_hit_die), (plain.to_hit + 1, plain.to_hit, 4));
        assert_eq!(buffed.interval_ms, (plain.interval_ms as f32 * 0.75).round() as u32);
        assert_eq!((buffed.speed_pct, buffed.resist_pct), (120, 10));
        // A condition (Mired: -2 AC, half speed) on top.
        f.afflict(Condition::Mired, 100);
        let mired = Sheet::of(&f);
        assert_eq!(mired.ac, f.armour());
        assert_eq!((mired.ac, mired.ac_parts.conditions), (buffed.ac - 2, -2));
        assert_ne!(mired, buffed, "a change resends the sheet");
        let j = json(&mired);
        assert_eq!(j["ac"].as_i64(), Some(i64::from(mired.ac)));
        assert_eq!(j["ac_parts"]["conditions"].as_i64(), Some(-2));
        assert!(j["damage"].as_str().is_some_and(|d| d.contains('d')), "{j}");
        assert!(j["damage_extra"].is_null());
    }

    #[test]
    fn dice_read_like_the_tooltips() {
        assert_eq!(dice_text(&Dice::new(1, 8, 3)), "1d8+3");
        assert_eq!(dice_text(&Dice::new(2, 6, -1)), "2d6-1");
        assert_eq!(dice_text(&Dice::new(1, 4, 0)), "1d4");
        assert_eq!(dice_text(&Dice::new(1, 1, 9)), "10", "Maximize Power: a flat number");
    }
}
