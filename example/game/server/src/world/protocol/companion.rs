//! Companion frames: the Merchant line's homunculus, plants and sphere mines
//! (`hub::companion`). Companions are not monsters and not players: their
//! entity ids start at [`COMPANION_ENTITY_BASE`], a homunculus moves in the
//! snapshot's player rows, rooted ones (plants, spheres) never do, and their
//! HP, attacks and casts reuse `PLAYER_VITALS` (0x2A), `COMBAT` (0x2B) and
//! `CAST` (0x2E) with the companion's entity id.
//!
//! Server → client:
//!
//! ```text
//! 0xA3 COMPANIONS      var n, n × companion: to everyone on the map, the
//!                      whole list on map enter (after MAP_ENTER), then only
//!                      the records that changed (a new one, a level, the
//!                      evolution, a rename, `gone`)
//! 0xA4 COMPANION_STATE u8 type (`hom_type`), u8 level, var exp, var
//!                      exp_to_next, u8 bond (0..=100), u8 stance
//!                      (`stance`), u8 state (`companion_state`), var
//!                      back_ms (until it returns: dead or resting; 0 out),
//!                      var sp, var max_sp, 4 × var cooldown_ms (its skills
//!                      in unlock order; 0 ready): the owner's homunculus, to
//!                      the owner only, on change and at most twice a second
//!
//! companion = var entity_id, var owner_entity_id, u8 class
//!             (`companion_class`), str sprite (companions pack id: `lif`,
//!             `amistr_h`, `mandragora`, `marine_sphere`), str name, u8
//!             level, u8 flags (`companion_flags`), pos x, pos y (where it
//!             stands; rooted ones stay there), var expires_ms (until a
//!             plant or sphere goes; 0 = no limit)
//! ```
//!
//! Client → server:
//!
//! ```text
//! 0xA8 COMPANION_CMD   u8 cmd: 1 choose: u8 type (`hom_type`, the first
//!                      Call Homunculus; in town later, the paid change);
//!                      2 stance: u8 stance; 3 rename: str name; 4 evolve
//!                      (in town); 5 rest (out → resting → out: a toggle)
//! ```

use super::{kind, put_list, read_list, ClientMsg, ServerMsg};
use crate::world::wire::{PutWire, Reader};
use bytes::{BufMut, BytesMut};

/// Entity id of the first companion: above every character id, below the
/// town residents (`RESIDENT_ENTITY_BASE`, 0x70_0000) and the monsters.
/// A companion's entity id is `COMPANION_ENTITY_BASE + Companion::id`.
pub const COMPANION_ENTITY_BASE: u32 = 0x60_0000;

/// [`CompanionInfo::class`] values.
pub mod companion_class {
    /// A homunculus: walks, follows its owner (snapshot rows).
    pub const HOMUNCULUS: u8 = 0;
    /// A plant of Summon Flora: rooted, lasts `expires_ms`.
    pub const PLANT: u8 = 1;
    /// A Marine Sphere of Sphere Mine: rooted, explodes.
    pub const SPHERE: u8 = 2;
}

/// [`CompanionInfo::flags`] bits.
pub mod companion_flags {
    /// An evolved homunculus (its `_h` sprite).
    pub const EVOLVED: u8 = 1;
    /// It left the map (dismissed, expired, owner gone, or dead and
    /// removed): the client drops the record and its row.
    pub const GONE: u8 = 2;
}

/// Homunculus types (`COMPANION_STATE` type, `COMPANION_CMD` choose).
pub mod hom_type {
    /// No type chosen yet: the client opens the chooser.
    pub const NONE: u8 = 0;
    pub const LIF: u8 = 1;
    pub const AMISTR: u8 = 2;
    pub const FILIR: u8 = 3;
    pub const VANILMIRTH: u8 = 4;
}

/// Homunculus stances (`COMPANION_STATE`, `COMPANION_CMD` stance).
pub mod stance {
    /// Engages threats within 9 cells of the owner.
    pub const AGGRESSIVE: u8 = 0;
    /// Within 5 cells of the owner (the default).
    pub const DEFENSIVE: u8 = 1;
    /// Never engages; follows and uses its support skills.
    pub const PASSIVE: u8 = 2;
}

/// [`CompanionState::state`] values.
pub mod companion_state {
    pub const OUT: u8 = 0;
    pub const RESTING: u8 = 1;
    pub const DEAD: u8 = 2;
}

/// `COMPANION_CMD` command bytes.
pub mod companion_cmd {
    pub const CHOOSE: u8 = 1;
    pub const STANCE: u8 = 2;
    pub const RENAME: u8 = 3;
    pub const EVOLVE: u8 = 4;
    pub const REST: u8 = 5;
}

/// One companion on the map (a `COMPANIONS` record).
#[derive(Clone, Debug, PartialEq)]
pub struct CompanionInfo {
    /// `COMPANION_ENTITY_BASE + id`.
    pub entity_id: u32,
    /// The owner's player entity id.
    pub owner_entity_id: u32,
    /// [`companion_class`].
    pub class: u8,
    /// Sprite id in the companions pack.
    pub sprite: String,
    pub name: String,
    pub level: u8,
    /// [`companion_flags`].
    pub flags: u8,
    pub x: f32,
    pub y: f32,
    /// Until it goes (plants, spheres); 0 = no limit.
    pub expires_ms: u32,
}

/// The owner's homunculus panel (`COMPANION_STATE`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompanionState {
    /// [`hom_type`].
    pub hom_type: u8,
    pub level: u8,
    pub exp: u32,
    pub exp_to_next: u32,
    /// 0..=100.
    pub bond: u8,
    /// [`stance`].
    pub stance: u8,
    /// [`companion_state`].
    pub state: u8,
    /// Until it is back (dead or resting); 0 while out.
    pub back_ms: u32,
    pub sp: u32,
    pub max_sp: u32,
    /// Its four skills in unlock order (0 ready, or not learned yet).
    pub cooldowns_ms: [u32; 4],
}

/// A `COMPANION_CMD` request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompanionCmd {
    /// Pick the homunculus type ([`hom_type`], not `NONE`).
    Choose(u8),
    /// Set the stance ([`stance`]).
    Stance(u8),
    Rename(String),
    Evolve,
    /// Rest or wake (toggle).
    Rest,
}

impl CompanionInfo {
    fn put(&self, b: &mut BytesMut) {
        b.put_var(self.entity_id.into());
        b.put_var(self.owner_entity_id.into());
        b.put_u8(self.class);
        b.put_str(&self.sprite);
        b.put_str(&self.name);
        b.put_u8(self.level);
        b.put_u8(self.flags);
        b.put_pos(self.x);
        b.put_pos(self.y);
        b.put_var(self.expires_ms.into());
    }

    fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(Self {
            entity_id: r.var32()?,
            owner_entity_id: r.var32()?,
            class: r.u8()?,
            sprite: r.str()?,
            name: r.str()?,
            level: r.u8()?,
            flags: r.u8()?,
            x: r.pos()?,
            y: r.pos()?,
            expires_ms: r.var32()?,
        })
    }
}

pub(super) fn encode(m: &ServerMsg, b: &mut BytesMut) {
    match m {
        ServerMsg::Companions { list } => {
            b.put_u8(kind::COMPANIONS);
            put_list(b, list, |b, c| c.put(b));
        }
        ServerMsg::CompanionState(s) => {
            b.put_u8(kind::COMPANION_STATE);
            b.put_u8(s.hom_type);
            b.put_u8(s.level);
            b.put_var(s.exp.into());
            b.put_var(s.exp_to_next.into());
            b.put_u8(s.bond);
            b.put_u8(s.stance);
            b.put_u8(s.state);
            b.put_var(s.back_ms.into());
            b.put_var(s.sp.into());
            b.put_var(s.max_sp.into());
            for cd in s.cooldowns_ms {
                b.put_var(cd.into());
            }
        }
        _ => unreachable!("not a companion frame"),
    }
}

pub(super) fn decode(k: u8, r: &mut Reader<'_>) -> Option<ServerMsg> {
    Some(match k {
        kind::COMPANIONS => ServerMsg::Companions { list: read_list(r, CompanionInfo::read)? },
        kind::COMPANION_STATE => ServerMsg::CompanionState(CompanionState {
            hom_type: r.u8()?,
            level: r.u8()?,
            exp: r.var32()?,
            exp_to_next: r.var32()?,
            bond: r.u8()?,
            stance: r.u8()?,
            state: r.u8()?,
            back_ms: r.var32()?,
            sp: r.var32()?,
            max_sp: r.var32()?,
            cooldowns_ms: [r.var32()?, r.var32()?, r.var32()?, r.var32()?],
        }),
        _ => return None,
    })
}

pub(super) fn encode_client(m: &ClientMsg, b: &mut BytesMut) {
    let ClientMsg::CompanionCmd(cmd) = m else { unreachable!("not a companion request") };
    b.put_u8(kind::COMPANION_CMD);
    match cmd {
        CompanionCmd::Choose(t) => {
            b.put_u8(companion_cmd::CHOOSE);
            b.put_u8(*t);
        }
        CompanionCmd::Stance(s) => {
            b.put_u8(companion_cmd::STANCE);
            b.put_u8(*s);
        }
        CompanionCmd::Rename(name) => {
            b.put_u8(companion_cmd::RENAME);
            b.put_str(name);
        }
        CompanionCmd::Evolve => b.put_u8(companion_cmd::EVOLVE),
        CompanionCmd::Rest => b.put_u8(companion_cmd::REST),
    }
}

pub(super) fn decode_client(r: &mut Reader<'_>) -> Option<ClientMsg> {
    Some(ClientMsg::CompanionCmd(match r.u8()? {
        companion_cmd::CHOOSE => CompanionCmd::Choose(r.u8()?),
        companion_cmd::STANCE => CompanionCmd::Stance(r.u8()?),
        companion_cmd::RENAME => CompanionCmd::Rename(r.str()?),
        companion_cmd::EVOLVE => CompanionCmd::Evolve,
        companion_cmd::REST => CompanionCmd::Rest,
        _ => return None,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::world::protocol::ClientDecodeError;
    use bytes::Bytes;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|v| format!("{v:02x}")).collect()
    }

    /// Change these only together with `client_test.dart` (`companion
    /// goldens`).
    pub fn goldens() -> Vec<(&'static str, Bytes)> {
        let lif = CompanionInfo {
            entity_id: COMPANION_ENTITY_BASE + 3,
            owner_entity_id: 300,
            class: companion_class::HOMUNCULUS,
            sprite: "lif_h".into(),
            name: "Lif".into(),
            level: 31,
            flags: companion_flags::EVOLVED,
            x: 12.5,
            y: 3.25,
            expires_ms: 0,
        };
        let plant = CompanionInfo {
            entity_id: COMPANION_ENTITY_BASE + 4,
            owner_entity_id: 300,
            class: companion_class::PLANT,
            sprite: "mandragora".into(),
            name: "Mandragora".into(),
            level: 30,
            flags: companion_flags::GONE,
            x: 1.0,
            y: 2.0,
            expires_ms: 40000,
        };
        let state = CompanionState {
            hom_type: hom_type::VANILMIRTH,
            level: 31,
            exp: 1500,
            exp_to_next: 9000,
            bond: 75,
            stance: stance::DEFENSIVE,
            state: companion_state::DEAD,
            back_ms: 12000,
            sp: 40,
            max_sp: 120,
            cooldowns_ms: [0, 2500, 0, 60000],
        };
        vec![
            ("companions", ServerMsg::Companions { list: vec![lif, plant] }.encode()),
            ("companions_empty", ServerMsg::Companions { list: vec![] }.encode()),
            ("companion_state", ServerMsg::CompanionState(state).encode()),
            ("cmd_choose", ClientMsg::CompanionCmd(CompanionCmd::Choose(hom_type::AMISTR)).encode()),
            ("cmd_stance", ClientMsg::CompanionCmd(CompanionCmd::Stance(stance::PASSIVE)).encode()),
            ("cmd_rename", ClientMsg::CompanionCmd(CompanionCmd::Rename("Bo".into())).encode()),
            ("cmd_evolve", ClientMsg::CompanionCmd(CompanionCmd::Evolve).encode()),
            ("cmd_rest", ClientMsg::CompanionCmd(CompanionCmd::Rest).encode()),
        ]
    }

    const EXPECTED: &[(&str, &str)] = &[
        ("companions", "a30283808003ac0200056c69665f68034c69661f012003d0000084808003ac02010a6d616e647261676f72610a4d616e647261676f72611e0240008000c0b802"),
        ("companions_empty", "a300"),
        ("companion_state", "a4041fdc0ba8464b0102e05d287800c41300e0d403"),
        ("cmd_choose", "a80102"),
        ("cmd_stance", "a80202"),
        ("cmd_rename", "a80302426f"),
        ("cmd_evolve", "a804"),
        ("cmd_rest", "a805"),
    ];

    #[test]
    fn companion_frames_round_trip_and_match_the_dart_goldens() {
        for (name, b) in goldens() {
            println!("(\"{name}\", \"{}\"),", hex(&b));
            if name.starts_with("cmd_") {
                assert_eq!(ClientMsg::decode(&b).unwrap().encode(), b, "{name}");
                if b.len() > 2 {
                    assert_eq!(ClientMsg::decode(&b[..b.len() - 1]), Err(ClientDecodeError::Malformed), "truncated {name}");
                }
            } else {
                assert_eq!(ServerMsg::decode(&b).unwrap().encode(), b, "{name}");
                assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none(), "truncated {name}");
            }
        }
        assert_eq!(ClientMsg::decode(&[kind::COMPANION_CMD, 9]), Err(ClientDecodeError::Malformed), "unknown cmd");
        assert_eq!(ClientMsg::decode(&[kind::COMPANION_CMD]), Err(ClientDecodeError::Malformed), "no cmd");
        let got: Vec<(&str, String)> = goldens().into_iter().map(|(n, b)| (n, hex(&b))).collect();
        let want: Vec<(&str, String)> = EXPECTED.iter().map(|(n, h)| (*n, h.to_string())).collect();
        assert_eq!(got, want);
    }
}
