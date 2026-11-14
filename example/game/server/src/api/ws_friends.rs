//! The socket's side of friends (`world::friends`): requests from the
//! client, events from friends' sockets, and presence on every map entry.

use super::{Conn, Out, Place, hub_error};
use crate::world::friends::{FriendEvent, FriendOp, Presence};
use crate::world::protocol::MapInfo;

impl Conn {
    /// `FRIEND` from the client: the list, then a note (or an error).
    pub(super) async fn on_friend(&mut self, op: FriendOp) -> Vec<Out> {
        let name = self.session.character.read().await.as_ref().map(|c| c.name.clone());
        let Some(name) = name else {
            return vec![hub_error("no_character", "select a character first")];
        };
        match self.state.friends.act(self.user_id(), &name, op).await {
            Ok(msgs) => msgs.into_iter().map(Out::Msg).collect(),
            Err(e) => vec![hub_error(e.code(), e.to_string())],
        }
    }

    /// A friend's socket changed something for us.
    pub(super) async fn on_friend_event(&self, ev: FriendEvent) -> Vec<Out> {
        self.state.friends.on_event(self.user_id(), ev).await.into_iter().map(Out::Msg).collect()
    }

    /// Entered `map`: friends see where we are (and hear we came online).
    /// The first map also brings our own list (through the pump).
    pub(super) async fn present(&self, map: &MapInfo) {
        let Some((character_id, name, class, level)) =
            self.session.character.read().await.as_ref().map(|c| (c.id, c.name.clone(), c.archetype.clone(), c.level))
        else {
            return;
        };
        let channel = match self.link.as_ref().map(|l| l.place) {
            Some(Place::Hub { channel }) => channel,
            _ => 0,
        };
        let friends = &self.state.friends;
        let first = friends.presence(self.user_id()).is_none_or(|p| p.conn_id != self.conn_id);
        let p = Presence {
            conn_id: self.conn_id,
            character_id,
            name,
            class,
            level,
            map: map.name.clone(),
            channel,
            tx: self.friend_tx.clone(),
        };
        friends.present(self.user_id(), p).await;
        if first {
            let _ = self.friend_tx.try_send(FriendEvent::Refresh);
        }
    }
}
