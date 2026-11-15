//! What a player looks like on the map, declaratively: one [`Appearance`]
//! derived by the instance from the authoritative profile (class, level,
//! look), the bag (weapon, headgear) and the loadout (mount), and sent
//! whole. A client draws a character from its latest appearance alone, so
//! applying one twice, late or out of a missed change is harmless.
//!
//! ```text
//! appearance           str class (body line), u8 tier (1..=3, the body),
//!                      look, str weapon (`family:item` of the held weapon,
//!                      e.g. `spear:pike`; '' none), var n, n × str headgear
//!                      (upper, mid, lower; bare slots left out), u8 mount
//!                      (`mount::*`), u8 speed (percent the mount adds to
//!                      the walk, what the rider's prediction uses),
//!                      u8 cart (1: pushes a Pushcart, drawn trailing
//!                      behind; 0 none)
//! 0xA6 PLAYER_APPEARANCE var entity_id, appearance: to everyone on the map
//!                      when any input of it changes, and for the recipient
//!                      itself right after MAP_ENTER (others come inside
//!                      MAP_ENTER / PLAYER_APPEARED players)
//! ```

use super::{put_list, read_list, Look};
use crate::world::wire::{PutWire, Reader};
use bytes::{BufMut, BytesMut};

/// A character's whole appearance (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    /// Class line of the body (`swordsman`; residents and pets: their
    /// archetype).
    pub class: String,
    /// Body tier, from the level (`combat::tier`).
    pub tier: u8,
    pub look: Look,
    /// `family:item` of the held weapon (`spear:pike`); '' = none.
    pub weapon: String,
    /// Item ids worn on the head, upper / middle / lower.
    pub headgear: Vec<String>,
    /// [`super::mount`] value.
    pub mount: u8,
    /// Percent the mount adds to the walking pace (0 on foot).
    pub speed: u8,
    /// 1 when the character has a Pushcart (the Merchant line's passive):
    /// every client draws the cart trailing behind it. 0 none.
    pub cart: u8,
}

impl Appearance {
    pub fn put(&self, b: &mut BytesMut) {
        b.put_str(&self.class);
        b.put_u8(self.tier);
        self.look.put(b);
        b.put_str(&self.weapon);
        put_list(b, &self.headgear, |b, h| b.put_str(h));
        b.put_u8(self.mount);
        b.put_u8(self.speed);
        b.put_u8(self.cart);
    }

    pub fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(Self {
            class: r.str()?,
            tier: r.u8()?,
            look: Look::read(r)?,
            weapon: r.str()?,
            headgear: read_list(r, |r| r.str())?,
            mount: r.u8()?,
            speed: r.u8()?,
            cart: r.u8()?,
        })
    }
}
