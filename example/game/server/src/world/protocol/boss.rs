//! Boss encounter frames (`hub::boss`): the scripted fights of elites and
//! bosses: their ability casts, the danger zones of each beat, their
//! shouts and the bar at the top of the screen.
//!
//! ```text
//! 0x6A BOSS_CAST    var caster, str ability (effects key; empty = its
//!                   cast ended early), str name (the cast bar's label),
//!                   var cast_ms (wind-up, or the channel's length),
//!                   u8 flags (1 a channel the party can break with burst
//!                   damage, 2 shielded: break the shield to stop it,
//!                   4 interrupted: with an empty ability)
//! 0x6B BOSS_ZONES   var caster, str ability, var n, n × {telegraph
//!                   (as in CAST; shape 3 = ring: `a` outer radius, `b`
//!                   inner), var delay_ms (when it resolves, from now),
//!                   u8 flags (1 safe: standing inside spares you, 2 a
//!                   bar centred on the caster: it fills from the middle)}
//! 0x6F BOSS_SHOUT   var entity_id, str name, str text, u8 style (0 an
//!                   elite, 1 a boss): a yell over its head and in the log
//! 0x72 BOSS_BAR     var entity_id, str name, u8 rank (0 gone: drop its
//!                   bar, 1 elite, 2 boss), var hp, var max_hp, var shield
//!                   (absorb left), u8 phase (0-based), var n, n × u8 HP %
//!                   at which the next phases begin (the pips), u8 flags
//!                   (1 enraged)
//! ```
//!
//! All four go to everyone on the encounter's map.

use super::{kind, put_list, read_list, ServerMsg, Telegraph};
use crate::world::wire::{PutWire, Reader};
use bytes::{BufMut, BytesMut};

/// One danger (or safe) zone of a boss ability.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BossZone {
    pub area: Telegraph,
    /// Resolves this long after the frame.
    pub delay_ms: u32,
    /// [`zone_flags`].
    pub flags: u8,
}

/// [`BossZone::flags`] bits.
pub mod zone_flags {
    /// A safe spot: the beat spares whoever stands inside.
    pub const SAFE: u8 = 1;
    /// A bar centred on its caster (a cross): it fills from the middle out.
    pub const CENTRE: u8 = 2;
}

/// [`ServerMsg::BossCast`] flags.
pub mod cast_flags {
    pub const CHANNEL: u8 = 1;
    pub const SHIELDED: u8 = 2;
    pub const INTERRUPTED: u8 = 4;
}

/// The encounter bar of one elite or boss.
#[derive(Clone, Debug, PartialEq)]
pub struct BossBarInfo {
    pub entity_id: u32,
    pub name: String,
    /// 0 gone, 1 elite, 2 boss.
    pub rank: u8,
    pub hp: u32,
    pub max_hp: u32,
    pub shield: u32,
    pub phase: u8,
    /// HP % at which each later phase begins.
    pub pips: Vec<u8>,
    /// 1 enraged.
    pub flags: u8,
}

pub(super) fn put_telegraph(b: &mut BytesMut, t: &Telegraph) {
    b.put_var(t.id.into());
    b.put_u8(t.shape);
    b.put_u8(t.look);
    b.put_pos(t.x);
    b.put_pos(t.y);
    b.put_u8(t.dir);
    b.put_pos(t.a);
    b.put_pos(t.b);
    b.put_var(t.span_ms.into());
    b.put_var(t.follow.into());
}

pub(super) fn read_telegraph(r: &mut Reader<'_>) -> Option<Telegraph> {
    Some(Telegraph {
        id: r.var32()?,
        shape: r.u8()?,
        look: r.u8()?,
        x: r.pos()?,
        y: r.pos()?,
        dir: r.u8()?,
        a: r.pos()?,
        b: r.pos()?,
        span_ms: r.var32()?,
        follow: r.var32()?,
    })
}

/// Writes the boss frames; `false` for any other message.
pub(super) fn encode(m: &ServerMsg, b: &mut BytesMut) -> bool {
    match m {
        ServerMsg::BossCast { caster, ability, name, cast_ms, flags } => {
            b.put_u8(kind::BOSS_CAST);
            b.put_var((*caster).into());
            b.put_str(ability);
            b.put_str(name);
            b.put_var((*cast_ms).into());
            b.put_u8(*flags);
        }
        ServerMsg::BossZones { caster, ability, zones } => {
            b.put_u8(kind::BOSS_ZONES);
            b.put_var((*caster).into());
            b.put_str(ability);
            put_list(b, zones, |b, z| {
                put_telegraph(b, &z.area);
                b.put_var(z.delay_ms.into());
                b.put_u8(z.flags);
            });
        }
        ServerMsg::BossShout { entity_id, name, text, style } => {
            b.put_u8(kind::BOSS_SHOUT);
            b.put_var((*entity_id).into());
            b.put_str(name);
            b.put_str(text);
            b.put_u8(*style);
        }
        ServerMsg::BossBar(bar) => {
            b.put_u8(kind::BOSS_BAR);
            b.put_var(bar.entity_id.into());
            b.put_str(&bar.name);
            b.put_u8(bar.rank);
            b.put_var(bar.hp.into());
            b.put_var(bar.max_hp.into());
            b.put_var(bar.shield.into());
            b.put_u8(bar.phase);
            put_list(b, &bar.pips, |b, p| b.put_u8(*p));
            b.put_u8(bar.flags);
        }
        _ => return false,
    }
    true
}

/// Reads the body of boss frame kind `k`; `None` when malformed or not one.
pub(super) fn decode(k: u8, r: &mut Reader<'_>) -> Option<ServerMsg> {
    Some(match k {
        kind::BOSS_CAST => ServerMsg::BossCast { caster: r.var32()?, ability: r.str()?, name: r.str()?, cast_ms: r.var32()?, flags: r.u8()? },
        kind::BOSS_ZONES => ServerMsg::BossZones {
            caster: r.var32()?,
            ability: r.str()?,
            zones: read_list(r, |r| Some(BossZone { area: read_telegraph(r)?, delay_ms: r.var32()?, flags: r.u8()? }))?,
        },
        kind::BOSS_SHOUT => ServerMsg::BossShout { entity_id: r.var32()?, name: r.str()?, text: r.str()?, style: r.u8()? },
        kind::BOSS_BAR => ServerMsg::BossBar(BossBarInfo {
            entity_id: r.var32()?,
            name: r.str()?,
            rank: r.u8()?,
            hp: r.var32()?,
            max_hp: r.var32()?,
            shield: r.var32()?,
            phase: r.u8()?,
            pips: read_list(r, |r| r.u8())?,
            flags: r.u8()?,
        }),
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) mod golden {
    use super::*;
    use crate::world::protocol::{look, MONSTER_ENTITY_BASE};
    use bytes::Bytes;

    /// Boss frames: a list of their own. Change these only together with
    /// `client_test.dart` (`boss goldens`).
    pub fn boss() -> Vec<(&'static str, Bytes)> {
        let ring = Telegraph { id: 0, shape: 3, look: look::HOLY, x: 10.5, y: 3.25, dir: 0, a: 6.0, b: 2.5, span_ms: 0, follow: 0 };
        let safe = Telegraph { id: 0, shape: 0, look: look::WARD, x: 12.0, y: 4.0, dir: 0, a: 2.0, b: 0.0, span_ms: 0, follow: 0 };
        let id = MONSTER_ENTITY_BASE + 5;
        vec![
            ("boss_cast", ServerMsg::BossCast { caster: id, ability: "boss_prayer".into(), name: "Prayer".into(), cast_ms: 4500, flags: cast_flags::CHANNEL }.encode()),
            (
                "boss_zones",
                ServerMsg::BossZones {
                    caster: id,
                    ability: "boss_halo".into(),
                    zones: vec![BossZone { area: ring, delay_ms: 1600, flags: 0 }, BossZone { area: safe, delay_ms: 1600, flags: zone_flags::SAFE }],
                }
                .encode(),
            ),
            ("boss_shout", ServerMsg::BossShout { entity_id: id, name: "Angeling".into(), text: "Kneel!".into(), style: 1 }.encode()),
            (
                "boss_bar",
                ServerMsg::BossBar(BossBarInfo {
                    entity_id: id,
                    name: "Angeling".into(),
                    rank: 2,
                    hp: 2000,
                    max_hp: 2790,
                    shield: 300,
                    phase: 1,
                    pips: vec![70, 40, 15],
                    flags: 1,
                })
                .encode(),
            ),
        ]
    }

    const BOSS_EXPECTED: &[(&str, &str)] = &[
        ("boss_cast", "6a858080040b626f73735f70726179657206507261796572942301"),
        ("boss_zones", "6b8580800409626f73735f68616c6f02000307a002d000008001a0000000c00c000000040003000100800000000000c00c01"),
        ("boss_shout", "6f8580800408416e67656c696e67064b6e65656c2101"),
        ("boss_bar", "728580800408416e67656c696e6702d00fe615ac02010346280f01"),
    ];

    #[test]
    fn boss_frames_round_trip_and_match_the_dart_goldens() {
        for (name, b) in boss() {
            let m = ServerMsg::decode(&b).unwrap();
            assert_eq!(m.encode(), b, "{name}");
            assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none(), "truncated {name}");
        }
        let got: Vec<(&str, String)> = boss().into_iter().map(|(n, b)| (n, crate::world::protocol::golden::hex(&b))).collect();
        let want: Vec<(&str, String)> = BOSS_EXPECTED.iter().map(|(n, h)| (*n, h.to_string())).collect();
        assert_eq!(got, want);
    }
}
