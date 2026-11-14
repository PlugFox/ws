//! Mission modifier frames (`world::modifiers`): the table the mission
//! window draws from, a lobby's or stage's selection, and the clear bonus.
//!
//! ```text
//! 0x7B MODIFIER_DEFS  var n, n × {str id, u8 bit (0xFF: the difficulty
//!                     slider), str name, str desc (tooltip markup), str
//!                     icon (icon ref), var risk (per step), var min_level,
//!                     bool locked (coming soon: refused), var m, m × {str
//!                     label, str detail} (the slider's tiers; none for a
//!                     curse)}, var r, r × {var zeny_pct, var exp_pct, var
//!                     chest} (the clear bonus by total risk 0..r−1; higher
//!                     risks pay the last row). After NPC_DIALOG of a
//!                     mission giver, on joining a lobby and on STAGE_JOINED
//! 0x7C MODIFIERS      u8 op: 0 state: u8 scope (0 the recipient's lobby:
//!                     after every LOBBY_STATE; 1 its stage: after
//!                     STAGE_JOINED), u8 tier, var mask (bit i = the def
//!                     with bit i), var risk, var zeny_pct, var exp_pct,
//!                     var chest (the clear bonus), var loot_pct (each
//!                     kill's share of drops, zeny and experience);
//!                     1 reward (before STAGE_END of a won run with
//!                     modifiers): u8 tier, var mask, var risk, var zeny,
//!                     var exp, var n, n × {str item, var count}
//! ```
//!
//! Client → server:
//!
//! ```text
//! 0x77 LOBBY_MODIFIERS u8 tier, var mask (the leader; members are
//!                      unreadied; refused as LOBBY_ERROR `bad_modifier`)
//! ```

use super::{kind, put_list, read_list, ServerMsg};
use crate::world::wire::{PutWire, Reader};
use bytes::{BufMut, BytesMut};

/// One row of the modifier table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModifierInfo {
    pub id: String,
    /// Its bit in a mask; [`ModifierInfo::SLIDER`] for the difficulty.
    pub bit: u8,
    pub name: String,
    pub desc: String,
    pub icon: String,
    pub risk: u32,
    pub min_level: u32,
    pub locked: bool,
    /// The slider's tiers.
    pub steps: Vec<ModifierStep>,
}

impl ModifierInfo {
    pub const SLIDER: u8 = 0xFF;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModifierStep {
    pub label: String,
    pub detail: String,
}

/// The clear bonus at one risk score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RewardStep {
    pub zeny_pct: u32,
    pub exp_pct: u32,
    pub chest: u32,
}

pub(super) fn encode(m: &ServerMsg, b: &mut BytesMut) {
    match m {
        ServerMsg::ModifierDefs { defs, rewards } => {
            b.put_u8(kind::MODIFIER_DEFS);
            put_list(b, defs, |b, d| {
                b.put_str(&d.id);
                b.put_u8(d.bit);
                b.put_str(&d.name);
                b.put_str(&d.desc);
                b.put_str(&d.icon);
                b.put_var(d.risk.into());
                b.put_var(d.min_level.into());
                b.put_bool(d.locked);
                put_list(b, &d.steps, |b, s| {
                    b.put_str(&s.label);
                    b.put_str(&s.detail);
                });
            });
            put_list(b, rewards, |b, r| {
                b.put_var(r.zeny_pct.into());
                b.put_var(r.exp_pct.into());
                b.put_var(r.chest.into());
            });
        }
        ServerMsg::Modifiers { scope, tier, mask, risk, zeny_pct, exp_pct, chest, loot_pct } => {
            b.put_u8(kind::MODIFIERS);
            b.put_u8(0);
            b.put_u8(*scope);
            b.put_u8(*tier);
            b.put_var((*mask).into());
            for v in [risk, zeny_pct, exp_pct, chest, loot_pct] {
                b.put_var((*v).into());
            }
        }
        ServerMsg::ModifierReward { tier, mask, risk, zeny, exp, items } => {
            b.put_u8(kind::MODIFIERS);
            b.put_u8(1);
            b.put_u8(*tier);
            b.put_var((*mask).into());
            b.put_var((*risk).into());
            b.put_var(*zeny);
            b.put_var(*exp);
            put_list(b, items, |b, (item, n)| {
                b.put_str(item);
                b.put_var((*n).into());
            });
        }
        _ => unreachable!("not a modifier frame"),
    }
}

pub(super) fn decode(k: u8, r: &mut Reader<'_>) -> Option<ServerMsg> {
    Some(match k {
        kind::MODIFIER_DEFS => ServerMsg::ModifierDefs {
            defs: read_list(r, |r| {
                Some(ModifierInfo {
                    id: r.str()?,
                    bit: r.u8()?,
                    name: r.str()?,
                    desc: r.str()?,
                    icon: r.str()?,
                    risk: r.var32()?,
                    min_level: r.var32()?,
                    locked: r.bool()?,
                    steps: read_list(r, |r| Some(ModifierStep { label: r.str()?, detail: r.str()? }))?,
                })
            })?,
            rewards: read_list(r, |r| Some(RewardStep { zeny_pct: r.var32()?, exp_pct: r.var32()?, chest: r.var32()? }))?,
        },
        kind::MODIFIERS => match r.u8()? {
            0 => ServerMsg::Modifiers {
                scope: r.u8()?,
                tier: r.u8()?,
                mask: r.var32()?,
                risk: r.var32()?,
                zeny_pct: r.var32()?,
                exp_pct: r.var32()?,
                chest: r.var32()?,
                loot_pct: r.var32()?,
            },
            1 => ServerMsg::ModifierReward {
                tier: r.u8()?,
                mask: r.var32()?,
                risk: r.var32()?,
                zeny: r.var()?,
                exp: r.var()?,
                items: read_list(r, |r| Some((r.str()?, r.var32()?)))?,
            },
            _ => return None,
        },
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::world::protocol::{ClientDecodeError, ClientMsg};

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// Modifier frames. Change these only together with `client_test.dart`
    /// (`modifier goldens`).
    pub fn goldens() -> Vec<(&'static str, ServerMsg)> {
        vec![
            (
                "modifier_defs",
                ServerMsg::ModifierDefs {
                    defs: vec![
                        ModifierInfo {
                            id: "difficulty".into(),
                            bit: ModifierInfo::SLIDER,
                            name: "D".into(),
                            desc: "d".into(),
                            icon: "i".into(),
                            risk: 4,
                            min_level: 0,
                            locked: false,
                            steps: vec![ModifierStep { label: "N".into(), detail: "x".into() }],
                        },
                        ModifierInfo {
                            id: "slow".into(),
                            bit: 1,
                            name: "S".into(),
                            desc: "s".into(),
                            icon: "j".into(),
                            risk: 6,
                            min_level: 10,
                            locked: true,
                            steps: Vec::new(),
                        },
                    ],
                    rewards: vec![RewardStep { zeny_pct: 0, exp_pct: 0, chest: 0 }, RewardStep { zeny_pct: 200, exp_pct: 100, chest: 5 }],
                },
            ),
            (
                "modifiers",
                ServerMsg::Modifiers { scope: 1, tier: 3, mask: 0b100_0010, risk: 24, zeny_pct: 48, exp_pct: 24, chest: 1, loot_pct: 59 },
            ),
            (
                "modifier_reward",
                ServerMsg::ModifierReward { tier: 3, mask: 2, risk: 18, zeny: 300, exp: 1500, items: vec![("jellopy".into(), 2)] },
            ),
        ]
    }

    const EXPECTED: &[(&str, &str)] = &[
        ("modifier_defs", "7b020a646966666963756c7479ff01440164016904000001014e017804736c6f770101530173016a060a010002000000c8016405"),
        ("modifiers", "7c00010342183018013b"),
        ("modifier_reward", "7c01030212ac02dc0b01076a656c6c6f707902"),
    ];

    #[test]
    fn modifier_frames_round_trip_and_match_the_dart_goldens() {
        for ((name, m), (_, want)) in goldens().into_iter().zip(EXPECTED) {
            let b = m.encode();
            assert_eq!(ServerMsg::decode(&b).as_ref(), Some(&m), "{name}");
            assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none(), "truncated {name}");
            assert_eq!(hex(&b), *want, "{name}");
        }
        let req = ClientMsg::LobbyModifiers { tier: 3, mask: 0b100_0010 }.encode();
        assert_eq!(hex(&req), "770342");
        assert_eq!(ClientMsg::decode(&req), Ok(ClientMsg::LobbyModifiers { tier: 3, mask: 0b100_0010 }));
        assert_eq!(ClientMsg::decode(&req[..2]), Err(ClientDecodeError::Malformed));
    }
}
