//! Monster affix frames (`world::combat::affixes`, `world::hub::affix`):
//! the table clients draw plates and tooltips from, which monsters carry
//! affixes, and the one-off moments of their tricks.
//!
//! The monster snapshot section is untouched: affixed monsters are a
//! capped few, so their bits travel once per spawn in their own frame,
//! keyed like the snapshot's monsters by `(slot, generation)`. A client
//! keeps them until that monster dies; the snapshot's SPAWN looks them up.
//!
//! ```text
//! 0xA0 MONSTER_AFFIXES var n, n × {var slot, u8 generation, var bits
//!                      (AffixFlags: bit i = the def with bit i), u8 state
//!                      (bit 0 frenzied, bit 1 shield broken, bit 2 a
//!                      minion or mirror copy: pays nothing)}. Upserts:
//!                      on spawn, on a state change, and every affixed
//!                      monster alive on STAGE_JOINED
//! 0xA1 AFFIX_EVENT     var n, n × {var entity, u8 event (AffixEvent::*),
//!                      pos x, pos y (cells: where it plays)}
//! 0xA2 AFFIX_DEFS      var n, n × {str id, u8 bit, str name, str desc
//!                      (tooltip markup), var color (0xRRGGBB)}, var m,
//!                      m × str rank (1..=m stars), var exp_pct, var
//!                      drop_pct, var extra_pct, var hp_pct (per star).
//!                      On STAGE_JOINED
//! ```

use super::{kind, put_list, read_list, ServerMsg};
use crate::world::wire::{PutWire, Reader};
use bytes::{BufMut, BytesMut};

/// One row of the affix table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffixInfo {
    pub id: String,
    pub bit: u8,
    pub name: String,
    pub desc: String,
    pub color: u32,
}

/// One affixed monster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonsterAffix {
    pub slot: u16,
    pub generation: u8,
    pub bits: u16,
    /// [`affix_state`] bits.
    pub state: u8,
}

/// [`MonsterAffix::state`] bits.
pub mod affix_state {
    pub const FRENZIED: u8 = 1;
    pub const SHIELD_BROKEN: u8 = 2;
    pub const MINION: u8 = 4;
}

/// A moment of an affix's trick, for its effect and sound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AffixEvent {
    pub entity: u32,
    pub event: u8,
    pub x: f32,
    pub y: f32,
}

impl AffixEvent {
    /// Where a Teleporting monster will arrive (0.8 s ahead).
    pub const BLINK_MARK: u8 = 0;
    /// It blinked: from its old spot (x, y) to the mark.
    pub const BLINK: u8 = 1;
    /// A Summoner calls its minions (at its feet).
    pub const SUMMON: u8 = 2;
    /// A Mirror splits.
    pub const SPLIT: u8 = 3;
    /// A Frenzied monster flies into its frenzy.
    pub const FRENZY: u8 = 4;
    /// Reserved: a shield breaking (5) and coming back (6) travel as the
    /// record's state bit; clients play them from its change.
    pub const SHIELD_BREAK: u8 = 5;
    pub const SHIELD_UP: u8 = 6;
    /// Thorns pricked someone inside the ring.
    pub const THORNS: u8 = 7;
    /// A Volatile monster's fuse is lit (it explodes later).
    pub const FUSE: u8 = 8;
}

pub(super) fn encode(m: &ServerMsg, b: &mut BytesMut) {
    match m {
        ServerMsg::MonsterAffixes { monsters } => {
            b.put_u8(kind::MONSTER_AFFIXES);
            put_list(b, monsters, |b, a| {
                b.put_var(a.slot.into());
                b.put_u8(a.generation);
                b.put_var(a.bits.into());
                b.put_u8(a.state);
            });
        }
        ServerMsg::AffixEvents { events } => {
            b.put_u8(kind::AFFIX_EVENT);
            put_list(b, events, |b, e| {
                b.put_var(e.entity.into());
                b.put_u8(e.event);
                b.put_pos(e.x);
                b.put_pos(e.y);
            });
        }
        ServerMsg::AffixDefs { defs, ranks, exp_pct, drop_pct, extra_pct, hp_pct } => {
            b.put_u8(kind::AFFIX_DEFS);
            put_list(b, defs, |b, d| {
                b.put_str(&d.id);
                b.put_u8(d.bit);
                b.put_str(&d.name);
                b.put_str(&d.desc);
                b.put_var(d.color.into());
            });
            put_list(b, ranks, |b, r| b.put_str(r));
            for v in [exp_pct, drop_pct, extra_pct, hp_pct] {
                b.put_var((*v).into());
            }
        }
        _ => unreachable!("not an affix frame"),
    }
}

pub(super) fn decode(k: u8, r: &mut Reader<'_>) -> Option<ServerMsg> {
    Some(match k {
        kind::MONSTER_AFFIXES => ServerMsg::MonsterAffixes {
            monsters: read_list(r, |r| {
                Some(MonsterAffix {
                    slot: u16::try_from(r.var32()?).ok()?,
                    generation: r.u8()?,
                    bits: u16::try_from(r.var32()?).ok()?,
                    state: r.u8()?,
                })
            })?,
        },
        kind::AFFIX_EVENT => ServerMsg::AffixEvents {
            events: read_list(r, |r| Some(AffixEvent { entity: r.var32()?, event: r.u8()?, x: r.pos()?, y: r.pos()? }))?,
        },
        kind::AFFIX_DEFS => ServerMsg::AffixDefs {
            defs: read_list(r, |r| Some(AffixInfo { id: r.str()?, bit: r.u8()?, name: r.str()?, desc: r.str()?, color: r.var32()? }))?,
            ranks: read_list(r, |r| r.str())?,
            exp_pct: r.var32()?,
            drop_pct: r.var32()?,
            extra_pct: r.var32()?,
            hp_pct: r.var32()?,
        },
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::world::protocol::MONSTER_ENTITY_BASE;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// Affix frames. Change these only together with `client_test.dart`
    /// (`affix goldens`).
    pub fn goldens() -> Vec<(&'static str, ServerMsg)> {
        vec![
            (
                "monster_affixes",
                ServerMsg::MonsterAffixes {
                    monsters: vec![
                        MonsterAffix { slot: 5, generation: 2, bits: 0x0011, state: 0 },
                        MonsterAffix { slot: 300, generation: 255, bits: 0x8000, state: affix_state::FRENZIED | affix_state::MINION },
                    ],
                },
            ),
            (
                "affix_event",
                ServerMsg::AffixEvents { events: vec![AffixEvent { entity: MONSTER_ENTITY_BASE + 5, event: AffixEvent::BLINK, x: 10.5, y: 3.25 }] },
            ),
            (
                "affix_defs",
                ServerMsg::AffixDefs {
                    defs: vec![AffixInfo { id: "hasted".into(), bit: 0, name: "H".into(), desc: "d".into(), color: 0x9fe8ff }],
                    ranks: vec!["Magic".into(), "Rare".into()],
                    exp_pct: 75,
                    drop_pct: 50,
                    extra_pct: 20,
                    hp_pct: 20,
                },
            ),
        ]
    }

    const EXPECTED: &[(&str, &str)] = &[
        ("monster_affixes", "a00205021100ac02ff80800205"),
        ("affix_event", "a1018580800401a002d000"),
        ("affix_defs", "a201066861737465640001480164ffd1ff0402054d6167696304526172654b321414"),
    ];

    #[test]
    fn affix_frames_round_trip_and_match_the_dart_goldens() {
        for ((name, m), (_, want)) in goldens().into_iter().zip(EXPECTED) {
            let b = m.encode();
            assert_eq!(ServerMsg::decode(&b).as_ref(), Some(&m), "{name}");
            assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none(), "truncated {name}");
            assert_eq!(hex(&b), *want, "{name}");
        }
    }
}
