//! Player to player in an instance (the player context menu): inspecting
//! someone's worn gear, and delivering a party invitation. Both need the
//! target on the sender's map; the party rules themselves live in
//! `lobby.rs` (the socket task checks them before the invitation comes
//! here).

use super::{HubEvent, HubInstance, Outgoing};
use crate::world::protocol::ServerMsg;

fn no_such_player() -> ServerMsg {
    ServerMsg::LobbyError { code: "no_such_player".into(), message: "that player is not here".into() }
}

impl HubInstance {
    /// The player with character `entity_id` on the map of player `i`,
    /// other than `i` (townsfolk are not players).
    fn player_beside(&self, i: usize, entity_id: u32) -> Option<usize> {
        let map = self.players[i].map;
        self.players.iter().position(|p| p.profile.character_id == entity_id && p.map == map).filter(|&j| j != i)
    }

    /// Answers `inspect`: the worn gear of `target` (on the map), then
    /// their live combat numbers (`PLAYER_SHEET`: buffs, auras and
    /// conditions included). The client asks again while the window is
    /// open to keep the numbers current.
    pub fn inspect(&mut self, user_id: u32, target: u32, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let Some(j) = self.player_beside(i, target) else {
            out.push((user_id, HubEvent::Control(no_such_player())));
            return;
        };
        let p = &self.players[j];
        let msg = ServerMsg::PlayerInspect {
            entity_id: target,
            name: p.profile.name.clone(),
            archetype: p.profile.archetype.clone(),
            level: p.fighter.as_ref().map_or(p.profile.level, |f| f.level),
            look: p.profile.look,
            equip: p.profile.bag.equip.to_vec(),
        };
        out.push((user_id, HubEvent::Control(msg)));
        out.push((user_id, HubEvent::Control(self.sheet_of(j).msg(target))));
    }

    /// Hands a party invitation (already accepted by the lobby directory)
    /// to `target` if they are on the inviter's map.
    pub fn party_invite(&mut self, user_id: u32, target: u32, mission: String, ttl_ms: u32, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        match self.player_beside(i, target) {
            Some(j) => {
                let from = &self.players[i].profile;
                let msg = ServerMsg::PartyInvite { from: from.character_id, name: from.name.clone(), mission, ttl_ms };
                out.push((self.players[j].profile.user_id, HubEvent::Control(msg)));
            }
            None => out.push((user_id, HubEvent::Control(no_such_player()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use super::super::test_support::*;
    use super::super::tests::{profile, quiet};
    use super::super::{HubEvent, HubInstance, Outgoing};
    use crate::world::protocol::ServerMsg;
    use std::sync::Arc;
    use uuid::Uuid;

    fn controls(out: &Outgoing, user: u32) -> Vec<ServerMsg> {
        out.iter()
            .filter(|(u, _)| *u == user)
            .filter_map(|(_, e)| match e {
                HubEvent::Control(m) => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    /// Players 1 and 2 in town, player 3 in the house.
    fn town() -> HubInstance {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        let mut out = Vec::new();
        let mut two = profile(2);
        two.bag.equip[4] = "knife".into();
        two.bag.equip[0] = "cap".into();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        hub.join(two, Uuid::nil(), None, 0, &mut out);
        hub.join(profile(3), Uuid::nil(), None, 0, &mut out);
        let i = hub.find(3).unwrap();
        hub.players[i].map = hub.world.map_index("house").unwrap();
        hub
    }

    #[test]
    fn inspect_shows_the_worn_gear_of_a_player_on_the_map() {
        let mut hub = town();
        let mut out = Vec::new();
        hub.inspect(1, 102, &mut out);
        match &controls(&out, 1)[..] {
            [ServerMsg::PlayerInspect { entity_id: 102, name, archetype, level: 1, equip, .. }, ServerMsg::PlayerSheet { entity_id: 102, sheet }] => {
                assert_eq!((name.as_str(), archetype.as_str()), ("p2", "novice"));
                assert_eq!(equip.len(), 10);
                assert_eq!((equip[0].as_str(), equip[4].as_str(), equip[1].as_str()), ("cap", "knife", ""));
                // In town: the fighter they would arrive as (gear, passives).
                let i = hub.find(2).unwrap();
                let want = super::super::fight::Fighter::arrive(&hub.players[i].profile, &hub.skills).armour();
                let sheet: serde_json::Value = serde_json::from_str(sheet).unwrap();
                assert_eq!(sheet["ac"].as_i64(), Some(i64::from(want)));
                assert_eq!(sheet["level"].as_i64(), Some(1));
            }
            other => panic!("{other:?}"),
        }
        assert!(controls(&out, 2).is_empty(), "the inspected player is not told");
        for target in [103, 101, 999] {
            out.clear();
            hub.inspect(1, target, &mut out);
            assert!(
                matches!(&controls(&out, 1)[..], [ServerMsg::LobbyError { code, .. }] if code == "no_such_player"),
                "{target}: another map, oneself or nobody"
            );
        }
    }

    #[test]
    fn party_invitation_reaches_only_a_player_on_the_map() {
        let mut hub = town();
        let mut out = Vec::new();
        hub.party_invite(1, 102, "Poring Island Survival".into(), 30_000, &mut out);
        assert!(controls(&out, 1).is_empty());
        match &controls(&out, 2)[..] {
            [ServerMsg::PartyInvite { from: 101, name, mission, ttl_ms: 30_000 }] => {
                assert_eq!((name.as_str(), mission.as_str()), ("p1", "Poring Island Survival"));
            }
            other => panic!("{other:?}"),
        }
        out.clear();
        hub.party_invite(1, 103, "M".into(), 30_000, &mut out);
        assert!(controls(&out, 3).is_empty(), "the house is another map");
        assert_matches!(&controls(&out, 1)[..], [ServerMsg::LobbyError { code, .. }] if code == "no_such_player");
    }
}
