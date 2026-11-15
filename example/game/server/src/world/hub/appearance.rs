//! How players look, declaratively (`protocol/appearance.rs`): one
//! [`Appearance`] per player, derived from the authoritative profile
//! (class, level, look), bag (weapon, headgear) and loadout (mount,
//! pushcart) by
//! [`HubPlayer::appearance`], never patched field by field.
//!
//! The instance keeps what it last showed (`HubPlayer::shown`). Every input
//! that may change it (gear, loadout, restyle, a level, a stage's fighter)
//! calls [`HubInstance::refresh_appearance`], and a sweep every
//! [`SWEEP_SECS`] re-derives everyone, so a forgotten path can only delay
//! a change, never leave a map drawing a stale character. A change goes to
//! the whole map, the player included; arrivals get everyone's in
//! `map_enter` / `player_appeared` and their own right after `map_enter`.

use super::{HubEvent, HubInstance, HubPlayer, Outgoing};
use crate::world::combat::{self, Class, SkillBook, Weapon};
use crate::world::protocol::{mount, Appearance, ServerMsg};

/// Seconds between two re-derivations of every player's appearance.
pub(super) const SWEEP_SECS: u32 = 1;

impl HubPlayer {
    /// Headgear worn, upper / middle / lower (item ids; bare slots left out).
    pub(super) fn headgear(&self) -> Vec<String> {
        use combat::items::{base_id, Slot};
        [Slot::UpperHead, Slot::MidHead, Slot::LowerHead]
            .iter()
            .map(|&s| base_id(&self.profile.bag.equip[s as usize]))
            .filter(|id| !id.is_empty())
            .map(String::from)
            .collect()
    }

    /// The weapon in hand as the client draws it: `family:item`
    /// (`spear:pike`), the family from the server's own table.
    pub(super) fn weapon_look(&self) -> String {
        let id = self.weapon_id();
        let class = Class::from_id(&self.profile.archetype).unwrap_or(Class::Swordsman);
        let held = Weapon::find(id).unwrap_or_else(|| Weapon::held(class, id));
        format!("{}:{}", held.kind.id(), held.id)
    }

    /// What this player looks like now, from the profile, bag and loadout
    /// alone (`book`: whether the loadout rides).
    pub(super) fn appearance(&self, book: &SkillBook) -> Appearance {
        let level = self.fighter.as_ref().map_or(self.profile.level, |f| f.level);
        let riding = book.rides(&self.profile.loadout);
        let speed = if riding { (self.ride_bonus(book) * 100.0).round().clamp(0.0, 255.0) as u8 } else { 0 };
        Appearance {
            class: self.profile.archetype.clone(),
            tier: combat::tier(level) as u8,
            look: self.profile.look,
            weapon: if Class::from_id(&self.profile.archetype).is_some() { self.weapon_look() } else { String::new() },
            headgear: self.headgear(),
            mount: if riding { mount::PECO } else { mount::NONE },
            speed,
            cart: u8::from(book.cart_stacks(&self.profile.loadout) > 0),
        }
    }
}

impl HubInstance {
    /// Re-derives player `idx`'s appearance (and the town pace riding
    /// gives); a change goes to everyone on the map, the player included.
    pub(super) fn refresh_appearance(&mut self, idx: usize, out: &mut Outgoing) {
        let ride = self.players[idx].ride_bonus(&self.skills);
        let fresh = self.players[idx].appearance(&self.skills);
        let p = &mut self.players[idx];
        p.ride = ride;
        if p.shown == fresh {
            return;
        }
        p.shown = fresh;
        let p = &self.players[idx];
        let msg = ServerMsg::PlayerAppearance { entity_id: p.profile.character_id, appearance: p.shown.clone() };
        self.broadcast(p.map, &msg, out);
    }

    /// Every [`SWEEP_SECS`]: everyone re-derived (see the module docs).
    pub(super) fn sweep_appearances(&mut self, out: &mut Outgoing) {
        let every = (SWEEP_SECS * self.config.tick_hz).max(1);
        if !self.tick.is_multiple_of(every) {
            return;
        }
        for idx in 0..self.players.len() {
            self.refresh_appearance(idx, out);
        }
    }

    /// The player's own appearance, to them (right after `map_enter`).
    pub(super) fn own_appearance(&self, idx: usize, out: &mut Outgoing) {
        let p = &self.players[idx];
        let msg = ServerMsg::PlayerAppearance { entity_id: p.profile.character_id, appearance: p.shown.clone() };
        out.push((p.profile.user_id, HubEvent::Control(msg)));
    }
}
