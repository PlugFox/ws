//! Mission results frames (`hub/stats.rs`, COMBAT.md §4.11): the party's
//! run statistics and the recipient's own rewards, once at the end of a
//! mission, right after `STAGE_END`.
//!
//! ```text
//! 0x81 RUN_STATS   bool victory, var secs, var bucket_secs, var n,
//!                  n × seat {var entity_id, str name, str class, var level,
//!                  u8 flags (1 left early, 2 link-dead at the end), var
//!                  dealt, var taken, var healed, var kills, var assists,
//!                  var downs, var deaths, var revives, var best_combo,
//!                  var mvps, var exp, var b, b × {var dealt, var taken,
//!                  var kills} (bucket i covers [i, i+1) × bucket_secs; the
//!                  run's buckets, never more than 90), var s, s × {str
//!                  skill (skill id, `attack` the basic attack, `other` the
//!                  rest past 12), var dealt, var hits}}, var m, m × mark
//!                  {var secs, u8 kind (0 boss slain, 1 elite slain, 2 a
//!                  player down, 3 a player out, 4 boss arrived), u8 seat
//!                  (0xFF: nobody), str label (the monster's name)}; at most
//!                  8 seats and 48 marks
//! 0x82 RUN_REWARDS u8 seat (the recipient's in RUN_STATS), var exp (the
//!                  run's, bonus included), var zeny (kills and drops), var
//!                  clear_zeny (the victory bonus), var mod_exp, var
//!                  mod_zeny (the modifiers' clear bonus), var n, n × {str
//!                  item, var count} (everything picked up, chest included;
//!                  at most 32 kinds)
//! ```

use super::{kind, put_list, read_list, ServerMsg};
use crate::world::wire::{PutWire, Reader};
use bytes::{BufMut, BytesMut};

/// [`RunSeat::flags`].
pub mod seat_flags {
    pub const LEFT: u8 = 1;
    pub const LINK_DEAD: u8 = 2;
}

/// [`RunMark::kind`].
pub mod mark {
    pub const BOSS_SLAIN: u8 = 0;
    pub const ELITE_SLAIN: u8 = 1;
    pub const DOWN: u8 = 2;
    pub const OUT: u8 = 3;
    pub const BOSS_ARRIVED: u8 = 4;
    /// [`super::RunMark::seat`] of a mark nobody made.
    pub const NOBODY: u8 = 0xFF;
}

/// One time bucket of a seat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunBucket {
    pub dealt: u32,
    pub taken: u32,
    pub kills: u32,
}

/// One party member's run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunSeat {
    pub entity_id: u32,
    pub name: String,
    pub class: String,
    pub level: u32,
    pub flags: u8,
    pub dealt: u64,
    pub taken: u64,
    pub healed: u64,
    pub kills: u32,
    pub assists: u32,
    pub downs: u32,
    pub deaths: u32,
    pub revives: u32,
    pub best_combo: u32,
    pub mvps: u32,
    pub exp: u64,
    pub buckets: Vec<RunBucket>,
    /// (skill id, damage, damaging hits), the most damaging first.
    pub skills: Vec<(String, u64, u32)>,
}

/// A moment on the run's timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunMark {
    pub secs: u32,
    pub kind: u8,
    pub seat: u8,
    pub label: String,
}

pub(super) fn encode(m: &ServerMsg, b: &mut BytesMut) {
    match m {
        ServerMsg::RunStats { victory, secs, bucket_secs, seats, marks } => {
            b.put_u8(kind::RUN_STATS);
            b.put_bool(*victory);
            b.put_var((*secs).into());
            b.put_var((*bucket_secs).into());
            put_list(b, seats, |b, s| {
                b.put_var(s.entity_id.into());
                b.put_str(&s.name);
                b.put_str(&s.class);
                b.put_var(s.level.into());
                b.put_u8(s.flags);
                for v in [s.dealt, s.taken, s.healed] {
                    b.put_var(v);
                }
                for v in [s.kills, s.assists, s.downs, s.deaths, s.revives, s.best_combo, s.mvps] {
                    b.put_var(v.into());
                }
                b.put_var(s.exp);
                put_list(b, &s.buckets, |b, k| {
                    for v in [k.dealt, k.taken, k.kills] {
                        b.put_var(v.into());
                    }
                });
                put_list(b, &s.skills, |b, (id, dealt, hits)| {
                    b.put_str(id);
                    b.put_var(*dealt);
                    b.put_var((*hits).into());
                });
            });
            put_list(b, marks, |b, k| {
                b.put_var(k.secs.into());
                b.put_u8(k.kind);
                b.put_u8(k.seat);
                b.put_str(&k.label);
            });
        }
        ServerMsg::RunRewards { seat, exp, zeny, clear_zeny, mod_exp, mod_zeny, items } => {
            b.put_u8(kind::RUN_REWARDS);
            b.put_u8(*seat);
            for v in [exp, zeny, clear_zeny, mod_exp, mod_zeny] {
                b.put_var(*v);
            }
            put_list(b, items, |b, (item, n)| {
                b.put_str(item);
                b.put_var((*n).into());
            });
        }
        _ => unreachable!("not a results frame"),
    }
}

pub(super) fn decode(k: u8, r: &mut Reader<'_>) -> Option<ServerMsg> {
    Some(match k {
        kind::RUN_STATS => ServerMsg::RunStats {
            victory: r.bool()?,
            secs: r.var32()?,
            bucket_secs: r.var32()?,
            seats: read_list(r, |r| {
                Some(RunSeat {
                    entity_id: r.var32()?,
                    name: r.str()?,
                    class: r.str()?,
                    level: r.var32()?,
                    flags: r.u8()?,
                    dealt: r.var()?,
                    taken: r.var()?,
                    healed: r.var()?,
                    kills: r.var32()?,
                    assists: r.var32()?,
                    downs: r.var32()?,
                    deaths: r.var32()?,
                    revives: r.var32()?,
                    best_combo: r.var32()?,
                    mvps: r.var32()?,
                    exp: r.var()?,
                    buckets: read_list(r, |r| Some(RunBucket { dealt: r.var32()?, taken: r.var32()?, kills: r.var32()? }))?,
                    skills: read_list(r, |r| Some((r.str()?, r.var()?, r.var32()?)))?,
                })
            })?,
            marks: read_list(r, |r| Some(RunMark { secs: r.var32()?, kind: r.u8()?, seat: r.u8()?, label: r.str()? }))?,
        },
        kind::RUN_REWARDS => ServerMsg::RunRewards {
            seat: r.u8()?,
            exp: r.var()?,
            zeny: r.var()?,
            clear_zeny: r.var()?,
            mod_exp: r.var()?,
            mod_zeny: r.var()?,
            items: read_list(r, |r| Some((r.str()?, r.var32()?)))?,
        },
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// Results frames. Change these only together with `client_test.dart`
    /// (`results goldens`).
    pub fn goldens() -> Vec<(&'static str, ServerMsg)> {
        vec![
            (
                "run_stats",
                ServerMsg::RunStats {
                    victory: true,
                    secs: 125,
                    bucket_secs: 10,
                    seats: vec![RunSeat {
                        entity_id: 300,
                        name: "Ann".into(),
                        class: "mage".into(),
                        level: 31,
                        flags: seat_flags::LINK_DEAD,
                        dealt: 1500,
                        taken: 200,
                        healed: 40,
                        kills: 12,
                        assists: 3,
                        downs: 1,
                        deaths: 0,
                        revives: 2,
                        best_combo: 9,
                        mvps: 1,
                        exp: 4000,
                        buckets: vec![RunBucket { dealt: 300, taken: 0, kills: 2 }, RunBucket { dealt: 0, taken: 20, kills: 0 }],
                        skills: vec![("bolt".into(), 1200, 10), ("attack".into(), 300, 6)],
                    }],
                    marks: vec![RunMark { secs: 61, kind: mark::BOSS_SLAIN, seat: 0, label: "Angeling".into() }],
                },
            ),
            (
                "run_rewards",
                ServerMsg::RunRewards { seat: 0, exp: 4000, zeny: 900, clear_zeny: 250, mod_exp: 100, mod_zeny: 50, items: vec![("jellopy".into(), 7)] },
            ),
        ]
    }

    const EXPECTED: &[(&str, &str)] = &[
        (
            "run_stats",
            "81017d0a01ac0203416e6e046d6167651f02dc0bc801280c030100020901a01f02ac0200020014000204626f6c74b0090a0661747461636bac0206013d000008416e67656c696e67",
        ),
        ("run_rewards", "8200a01f8407fa01643201076a656c6c6f707907"),
    ];

    #[test]
    fn results_frames_round_trip_and_match_the_dart_goldens() {
        for ((name, m), (_, want)) in goldens().into_iter().zip(EXPECTED) {
            let b = m.encode();
            assert_eq!(ServerMsg::decode(&b).as_ref(), Some(&m), "{name}");
            assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none(), "truncated {name}");
            assert_eq!(hex(&b), *want, "{name}");
        }
    }
}
