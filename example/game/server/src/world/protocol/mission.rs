//! Mission platform frames (`world::missions`, `world::environment`,
//! `hub/env.rs`): the mission list a mission giver shows and the live
//! environment of a run.
//!
//! ```text
//! 0x92 MISSION_LIST  var n, n × card (every mission of the ladder, open or
//!                    coming soon; after NPC_DIALOG of a mission giver,
//!                    before the MISSION_INFO briefs)
//! 0x93 ENVIRONMENT   u8 op (to everyone on the stage):
//!                    0 light: f32 radius (cells, the run's base light
//!                      radius now: the player's own upgrades add to it),
//!                      var fade_ms (ease to it over this long);
//!                    1 event: u8 phase (0 warning, 1 begins, 2 ends),
//!                      str id, str name, str text (the warning; may be
//!                      empty), var start_ms (run clock of its begin),
//!                      var duration_ms;
//!                    2 shake: u8 intensity (1..=255), var duration_ms
//!                      (the camera; clients with reduced motion skip it);
//!                    light and running events are also sent to a player
//!                    joining the stage
//!
//! card = str id, str name, str description, str story, str map,
//!        str place, str terrain (field | dungeon), str art,
//!        var band_min, var band_recommended, var band_max,
//!        var monster_level_min, var monster_level_max,
//!        var time_limit_secs, u8 max_players,
//!        var n, n × str boss name, var m, m × str tag,
//!        var k, k × str tip, str rewards,
//!        u8 status (0 open, 1 coming soon), str reason, u8 step
//! ```
//!
//! Environment hazards are drawn as lasting ground through `BOSS_ZONES`
//! (caster 0, ability `hazard:<id>`) and ended with `ZONE_END`.

use super::{kind, put_list, read_list, ServerMsg};
use crate::world::missions::{Availability, MissionCard};
use crate::world::wire::{PutWire, Reader};
use bytes::{BufMut, BytesMut};

/// [`ServerMsg::EnvEvent`] phases.
pub mod phase {
    pub const WARNING: u8 = 0;
    pub const BEGINS: u8 = 1;
    pub const ENDS: u8 = 2;
}

/// Ability key of an environment hazard's zones (`BOSS_ZONES`, caster 0).
pub fn hazard_ability(id: &str) -> String {
    format!("hazard:{id}")
}

fn put_strs(b: &mut BytesMut, v: &[String]) {
    put_list(b, v, |b, s| b.put_str(s));
}

pub(super) fn encode(m: &ServerMsg, b: &mut BytesMut) {
    match m {
        ServerMsg::MissionList { missions } => {
            b.put_u8(kind::MISSION_LIST);
            put_list(b, missions, |b, c| {
                for s in [&c.id, &c.name, &c.description, &c.story, &c.map, &c.place, &c.terrain, &c.art] {
                    b.put_str(s);
                }
                for v in [c.band.0, c.band.1, c.band.2, c.monster_levels.0, c.monster_levels.1, c.time_limit_secs] {
                    b.put_var(v.into());
                }
                b.put_u8(c.max_players);
                put_strs(b, &c.bosses);
                put_strs(b, &c.tags);
                put_strs(b, &c.tips);
                b.put_str(&c.rewards);
                b.put_u8(match c.available {
                    Availability::Open => 0,
                    Availability::ComingSoon => 1,
                });
                b.put_str(&c.reason);
                b.put_u8(c.step);
            });
        }
        ServerMsg::EnvLight { radius, fade_ms } => {
            b.put_u8(kind::ENVIRONMENT);
            b.put_u8(0);
            b.put_f32_le(*radius);
            b.put_var((*fade_ms).into());
        }
        ServerMsg::EnvEvent { phase, id, name, text, start_ms, duration_ms } => {
            b.put_u8(kind::ENVIRONMENT);
            b.put_u8(1);
            b.put_u8(*phase);
            b.put_str(id);
            b.put_str(name);
            b.put_str(text);
            b.put_var((*start_ms).into());
            b.put_var((*duration_ms).into());
        }
        ServerMsg::EnvShake { intensity, duration_ms } => {
            b.put_u8(kind::ENVIRONMENT);
            b.put_u8(2);
            b.put_u8(*intensity);
            b.put_var((*duration_ms).into());
        }
        _ => unreachable!("not a mission platform frame"),
    }
}

fn read_strs(r: &mut Reader<'_>) -> Option<Vec<String>> {
    read_list(r, |r| r.str())
}

pub(super) fn decode(k: u8, r: &mut Reader<'_>) -> Option<ServerMsg> {
    Some(match k {
        kind::MISSION_LIST => ServerMsg::MissionList {
            missions: read_list(r, |r| {
                Some(MissionCard {
                    id: r.str()?,
                    name: r.str()?,
                    description: r.str()?,
                    story: r.str()?,
                    map: r.str()?,
                    place: r.str()?,
                    terrain: r.str()?,
                    art: r.str()?,
                    band: (r.var32()?, r.var32()?, r.var32()?),
                    monster_levels: (r.var32()?, r.var32()?),
                    time_limit_secs: r.var32()?,
                    max_players: r.u8()?,
                    bosses: read_strs(r)?,
                    tags: read_strs(r)?,
                    tips: read_strs(r)?,
                    rewards: r.str()?,
                    available: if r.u8()? == 0 { Availability::Open } else { Availability::ComingSoon },
                    reason: r.str()?,
                    step: r.u8()?,
                })
            })?,
        },
        kind::ENVIRONMENT => match r.u8()? {
            0 => ServerMsg::EnvLight { radius: r.f32()?, fade_ms: r.var32()? },
            1 => ServerMsg::EnvEvent {
                phase: r.u8()?,
                id: r.str()?,
                name: r.str()?,
                text: r.str()?,
                start_ms: r.var32()?,
                duration_ms: r.var32()?,
            },
            2 => ServerMsg::EnvShake { intensity: r.u8()?, duration_ms: r.var32()? },
            _ => return None,
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

    pub fn card() -> MissionCard {
        MissionCard {
            id: "m".into(),
            name: "M".into(),
            description: "d".into(),
            story: "s".into(),
            map: "f".into(),
            place: "P".into(),
            terrain: "dungeon".into(),
            art: "a".into(),
            band: (15, 18, 30),
            monster_levels: (15, 35),
            time_limit_secs: 900,
            max_players: 8,
            bosses: vec!["B".into()],
            tags: vec!["fog".into()],
            tips: vec![],
            rewards: "r".into(),
            available: Availability::ComingSoon,
            reason: "x".into(),
            step: 1,
        }
    }

    /// Mission platform frames. Change these only together with
    /// `client_test.dart` (`mission platform goldens`).
    pub fn frames() -> Vec<(&'static str, ServerMsg, &'static str)> {
        vec![
            ("mission_list", ServerMsg::MissionList { missions: vec![card()] }, "9201016d014d01640173016601500764756e67656f6e01610f121e0f238407080101420103666f6700017201017801"),
            ("env_light", ServerMsg::EnvLight { radius: 28.0, fade_ms: 1500 }, "93000000e041dc0b"),
            (
                "env_event",
                ServerMsg::EnvEvent { phase: phase::WARNING, id: "tide".into(), name: "Tide".into(), text: "Up!".into(), start_ms: 120_000, duration_ms: 40_000 },
                "9301000474696465045469646503557021c0a907c0b802",
            ),
            ("env_shake", ServerMsg::EnvShake { intensity: 200, duration_ms: 800 }, "9302c8a006"),
        ]
    }

    #[test]
    fn mission_platform_frames_round_trip_and_match_the_dart_goldens() {
        for (name, msg, want) in frames() {
            let b = msg.encode();
            assert_eq!(ServerMsg::decode(&b).as_ref(), Some(&msg), "{name}");
            assert_eq!(hex(&b), want, "{name}");
        }
    }
}
