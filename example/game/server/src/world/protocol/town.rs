//! Town service frames: the Hatter's recipes (`combat::quests`,
//! `hub::quest`) and the Kafra's list of public parties (`lobby.rs`
//! `public_parties`, `join_public`).
//!
//! Server → client:
//!
//! ```text
//! 0x99 QUESTS      u8 op: 0 recipes (a recipe giver the recipient talked
//!                  to): str npc, str name, var n, n × {str id, str item
//!                  (what it makes), var zeny (the fee), var m, m × {str
//!                  item, var count} (the materials)};
//!                  1 crafted (a QUEST craft went through; the new
//!                  INVENTORY comes before it): str recipe, str item
//! 0x9B PARTIES     var n, n × {var lobby_id, str mission (id), str
//!                  mission_name, var capacity (members it takes), u8 state
//!                  (0 forming in town, 1 on a mission), var elapsed_secs
//!                  (into the run; 0 while forming), bool mine (the
//!                  recipient's own party), var m, m × {str name, str
//!                  archetype, var level, bool online} (the leader first)}
//!                  (the answer to PARTIES list: every public party with
//!                  someone online, plus the recipient's own)
//! ```
//!
//! Client → server:
//!
//! ```text
//! 0x98 QUEST       u8 op: 0 craft: str npc, str recipe (at a recipe giver,
//!                  in town; answered by QUESTS crafted or HUB_ERROR)
//! 0x9A PARTIES     u8 op: 0 list (answered by PARTIES); 1 join: var
//!                  lobby_id (a public party with room, or the recipient's
//!                  own: on a mission, the joiner is sent into the running
//!                  stage beside a member; errors as LOBBY_ERROR); 2 public:
//!                  bool public (the leader lists the party or hides it)
//! ```

use super::{kind, put_list, read_list, ClientMsg, ServerMsg};
use crate::world::wire::{PutWire, Reader};
use bytes::{BufMut, BytesMut};

/// One recipe of a recipe giver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecipeInfo {
    pub id: String,
    pub item: String,
    pub zeny: u32,
    pub needs: Vec<(String, u32)>,
}

/// A member line of [`PartyListing`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedMember {
    pub name: String,
    pub archetype: String,
    pub level: u32,
    pub online: bool,
}

/// [`PartyListing::state`] values.
pub mod party_state {
    pub const FORMING: u8 = 0;
    pub const ON_MISSION: u8 = 1;
}

/// One public party on the Kafra's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartyListing {
    pub lobby_id: u32,
    pub mission: String,
    pub mission_name: String,
    pub capacity: u8,
    pub state: u8,
    pub elapsed_secs: u32,
    pub mine: bool,
    /// The leader first.
    pub members: Vec<ListedMember>,
}

pub(super) fn encode(m: &ServerMsg, b: &mut BytesMut) {
    match m {
        ServerMsg::Recipes { npc, name, recipes } => {
            b.put_u8(kind::QUESTS);
            b.put_u8(0);
            b.put_str(npc);
            b.put_str(name);
            put_list(b, recipes, |b, r| {
                b.put_str(&r.id);
                b.put_str(&r.item);
                b.put_var(r.zeny.into());
                put_list(b, &r.needs, |b, (item, n)| {
                    b.put_str(item);
                    b.put_var((*n).into());
                });
            });
        }
        ServerMsg::Crafted { recipe, item } => {
            b.put_u8(kind::QUESTS);
            b.put_u8(1);
            b.put_str(recipe);
            b.put_str(item);
        }
        ServerMsg::Parties { parties } => {
            b.put_u8(kind::PARTIES);
            put_list(b, parties, |b, p| {
                b.put_var(p.lobby_id.into());
                b.put_str(&p.mission);
                b.put_str(&p.mission_name);
                b.put_var(p.capacity.into());
                b.put_u8(p.state);
                b.put_var(p.elapsed_secs.into());
                b.put_bool(p.mine);
                put_list(b, &p.members, |b, m| {
                    b.put_str(&m.name);
                    b.put_str(&m.archetype);
                    b.put_var(m.level.into());
                    b.put_bool(m.online);
                });
            });
        }
        _ => unreachable!("not a town frame"),
    }
}

pub(super) fn decode(k: u8, r: &mut Reader<'_>) -> Option<ServerMsg> {
    Some(match k {
        kind::QUESTS => match r.u8()? {
            0 => ServerMsg::Recipes {
                npc: r.str()?,
                name: r.str()?,
                recipes: read_list(r, |r| {
                    Some(RecipeInfo {
                        id: r.str()?,
                        item: r.str()?,
                        zeny: r.var32()?,
                        needs: read_list(r, |r| Some((r.str()?, r.var32()?)))?,
                    })
                })?,
            },
            1 => ServerMsg::Crafted { recipe: r.str()?, item: r.str()? },
            _ => return None,
        },
        kind::PARTIES => ServerMsg::Parties {
            parties: read_list(r, |r| {
                Some(PartyListing {
                    lobby_id: r.var32()?,
                    mission: r.str()?,
                    mission_name: r.str()?,
                    capacity: u8::try_from(r.var()?).ok()?,
                    state: r.u8()?,
                    elapsed_secs: r.var32()?,
                    mine: r.bool()?,
                    members: read_list(r, |r| {
                        Some(ListedMember { name: r.str()?, archetype: r.str()?, level: r.var32()?, online: r.bool()? })
                    })?,
                })
            })?,
        },
        _ => return None,
    })
}

pub(super) fn encode_client(m: &ClientMsg, b: &mut BytesMut) {
    match m {
        ClientMsg::Craft { npc, recipe } => {
            b.put_u8(kind::QUEST_REQ);
            b.put_u8(0);
            b.put_str(npc);
            b.put_str(recipe);
        }
        ClientMsg::PartyList => {
            b.put_u8(kind::PARTIES_REQ);
            b.put_u8(0);
        }
        ClientMsg::PartyJoin { lobby_id } => {
            b.put_u8(kind::PARTIES_REQ);
            b.put_u8(1);
            b.put_var((*lobby_id).into());
        }
        ClientMsg::PartyPublic { public } => {
            b.put_u8(kind::PARTIES_REQ);
            b.put_u8(2);
            b.put_bool(*public);
        }
        _ => unreachable!("not a town request"),
    }
}

pub(super) fn decode_client(k: u8, r: &mut Reader<'_>) -> Option<ClientMsg> {
    Some(match (k, r.u8()?) {
        (kind::QUEST_REQ, 0) => ClientMsg::Craft { npc: r.str()?, recipe: r.str()? },
        (kind::PARTIES_REQ, 0) => ClientMsg::PartyList,
        (kind::PARTIES_REQ, 1) => ClientMsg::PartyJoin { lobby_id: r.var32()? },
        (kind::PARTIES_REQ, 2) => ClientMsg::PartyPublic { public: r.bool()? },
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::world::protocol::ClientDecodeError;
    use bytes::Bytes;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|v| format!("{v:02x}")).collect()
    }

    /// Change these only together with `client_test.dart` (`town goldens`).
    pub fn goldens() -> Vec<(&'static str, Bytes)> {
        let recipe = RecipeInfo { id: "ribbon".into(), item: "ribbon".into(), zeny: 200, needs: vec![("jellopy".into(), 10), ("sticky_mucus".into(), 3)] };
        let party = PartyListing {
            lobby_id: 7,
            mission: "m1".into(),
            mission_name: "Meadow".into(),
            capacity: 4,
            state: party_state::ON_MISSION,
            elapsed_secs: 130,
            mine: false,
            members: vec![
                ListedMember { name: "Ann".into(), archetype: "mage".into(), level: 12, online: true },
                ListedMember { name: "Bo".into(), archetype: "thief".into(), level: 9, online: false },
            ],
        };
        vec![
            ("recipes", ServerMsg::Recipes { npc: "hatter".into(), name: "Mira".into(), recipes: vec![recipe] }.encode()),
            ("crafted", ServerMsg::Crafted { recipe: "ribbon".into(), item: "ribbon".into() }.encode()),
            ("parties", ServerMsg::Parties { parties: vec![party] }.encode()),
            ("craft", ClientMsg::Craft { npc: "hatter".into(), recipe: "ribbon".into() }.encode()),
            ("party_list", ClientMsg::PartyList.encode()),
            ("party_join", ClientMsg::PartyJoin { lobby_id: 300 }.encode()),
            ("party_public", ClientMsg::PartyPublic { public: true }.encode()),
        ]
    }

    const EXPECTED: &[(&str, &str)] = &[
        ("recipes", "990006686174746572044d6972610106726962626f6e06726962626f6ec80102076a656c6c6f70790a0c737469636b795f6d7563757303"),
        ("crafted", "990106726962626f6e06726962626f6e"),
        ("parties", "9b0107026d31064d6561646f7704018201000203416e6e046d6167650c0102426f0574686965660900"),
        ("craft", "98000668617474657206726962626f6e"),
        ("party_list", "9a00"),
        ("party_join", "9a01ac02"),
        ("party_public", "9a0201"),
    ];

    #[test]
    fn town_frames_round_trip_and_match_the_dart_goldens() {
        for (name, b) in goldens() {
            println!("(\"{name}\", \"{}\"),", hex(&b));
            if matches!(name, "recipes" | "crafted" | "parties") {
                assert_eq!(ServerMsg::decode(&b).unwrap().encode(), b, "{name}");
                assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none(), "truncated {name}");
            } else {
                assert_eq!(ClientMsg::decode(&b).unwrap().encode(), b, "{name}");
                assert_eq!(ClientMsg::decode(&b[..b.len() - 1]), Err(ClientDecodeError::Malformed), "truncated {name}");
            }
        }
        assert_eq!(ClientMsg::decode(&[kind::PARTIES_REQ, 9]), Err(ClientDecodeError::Malformed), "unknown op");
        let got: Vec<(&str, String)> = goldens().into_iter().map(|(n, b)| (n, hex(&b))).collect();
        let want: Vec<(&str, String)> = EXPECTED.iter().map(|(n, h)| (*n, h.to_string())).collect();
        assert_eq!(got, want);
    }
}
