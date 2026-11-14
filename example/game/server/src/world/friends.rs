//! Friends: account-level, across hub channels and stages.
//!
//! The list lives in the database (`db::friend_repo`: requests, accepted
//! friendships, one row per direction). Presence is in memory: a socket
//! that enters a map records who it plays and where ([`Friends::present`]),
//! and the record goes when that socket closes ([`Friends::gone`]). Every
//! record carries the socket's friend channel, so a change on one side
//! reaches the other side wherever it is (any hub channel, any stage).
//!
//! Model: asking someone sends a request they accept or decline; asking
//! someone who already asked you makes you friends at once (mutual). Either
//! side removes a friendship (or withdraws / declines a request) for both.
//! Online friends hear when you come online (a toast) and see where you
//! are (character, map, channel) in their list. Requests reveal no
//! presence: only friends see where you are.

use super::protocol::ServerMsg;
use super::wire::{PutWire, Reader};
use crate::db::friend_repo::{FriendError, FriendRepo, Link, Requested};
use bytes::{BufMut, BytesMut};
use dashmap::DashMap;
use crate::db::Db;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Per-socket friend channel depth (events are tiny and rare).
pub const FRIEND_CHANNEL_CAPACITY: usize = 32;

/// `FRIEND_NOTE` ops: what happened, for a light toast.
pub mod note {
    /// A friend came online (`name` = the character they play).
    pub const ONLINE: u8 = 0;
    /// `name` asks to be friends.
    pub const REQUEST: u8 = 1;
    /// You and `name` are friends now.
    pub const ACCEPTED: u8 = 2;
    /// Your request to `name` went out.
    pub const SENT: u8 = 3;
    /// `name` is off your list (you removed them, withdrew or declined).
    pub const REMOVED: u8 = 4;
}

/// One row of the friends list (`FRIENDS`).
#[derive(Clone, Debug, PartialEq)]
pub struct FriendInfo {
    pub account: u32,
    /// 0 friend, 1 asks you, 2 you asked (`Link::code`).
    pub link: u8,
    /// The character they play now (online), else the last one played.
    pub name: String,
    pub online: bool,
    pub class: String,
    pub level: u32,
    /// Map display name; empty when offline (or not a friend yet).
    pub map: String,
    /// Hub channel; 0 = on a mission, or offline.
    pub channel: u16,
    /// Every character of the account (entity ids are character ids): the
    /// client knows a player on the map is already a friend.
    pub characters: Vec<u32>,
}

impl FriendInfo {
    pub(super) fn put(&self, b: &mut BytesMut) {
        b.put_var(self.account.into());
        b.put_u8(self.link);
        b.put_str(&self.name);
        b.put_bool(self.online);
        b.put_str(&self.class);
        b.put_var(self.level.into());
        b.put_str(&self.map);
        b.put_var(self.channel.into());
        b.put_var(self.characters.len() as u64);
        for c in &self.characters {
            b.put_var((*c).into());
        }
    }

    pub(super) fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(Self {
            account: r.var32()?,
            link: r.u8()?,
            name: r.str()?,
            online: r.bool()?,
            class: r.str()?,
            level: r.var32()?,
            map: r.str()?,
            channel: u16::try_from(r.var()?).ok()?,
            characters: {
                let n = r.var32()? as usize;
                if n > r.remaining() {
                    return None;
                }
                (0..n).map(|_| r.var32()).collect::<Option<Vec<_>>>()?
            },
        })
    }
}

/// `FRIEND` request ops (client → server).
#[derive(Clone, Debug, PartialEq)]
pub enum FriendOp {
    /// Send me the list.
    List,
    /// Ask the player behind this entity (a character id) to be friends.
    Add { entity_id: u32 },
    /// Ask the player whose character has this name.
    AddName { name: String },
    Accept { account: u32 },
    Decline { account: u32 },
    /// End a friendship or withdraw a request.
    Remove { account: u32 },
}

impl FriendOp {
    pub(super) fn put(&self, b: &mut BytesMut) {
        match self {
            FriendOp::List => b.put_u8(0),
            FriendOp::Add { entity_id } => {
                b.put_u8(1);
                b.put_var((*entity_id).into());
            }
            FriendOp::AddName { name } => {
                b.put_u8(2);
                b.put_str(name);
            }
            FriendOp::Accept { account } => {
                b.put_u8(3);
                b.put_var((*account).into());
            }
            FriendOp::Decline { account } => {
                b.put_u8(4);
                b.put_var((*account).into());
            }
            FriendOp::Remove { account } => {
                b.put_u8(5);
                b.put_var((*account).into());
            }
        }
    }

    pub(super) fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(match r.u8()? {
            0 => FriendOp::List,
            1 => FriendOp::Add { entity_id: r.var32()? },
            2 => FriendOp::AddName { name: r.str()? },
            3 => FriendOp::Accept { account: r.var32()? },
            4 => FriendOp::Decline { account: r.var32()? },
            5 => FriendOp::Remove { account: r.var32()? },
            _ => return None,
        })
    }
}

/// Where an online account is (one per account: its latest socket).
#[derive(Clone, Debug)]
pub struct Presence {
    /// The socket that recorded it (only that socket clears it).
    pub conn_id: Uuid,
    pub character_id: u32,
    pub name: String,
    pub class: String,
    pub level: u32,
    pub map: String,
    /// Hub channel; 0 on a stage.
    pub channel: u16,
    /// The socket's friend channel.
    pub tx: mpsc::Sender<FriendEvent>,
}

/// To a socket: its friends list changed (and maybe a toast).
#[derive(Clone, Debug, PartialEq)]
pub enum FriendEvent {
    /// Send the list again (someone moved, went offline, removed you).
    Refresh,
    /// The list again, then a `FRIEND_NOTE`.
    Note { op: u8, account: u32, name: String },
}

/// Friends service: the repository plus who is online where.
#[derive(Clone)]
pub struct Friends {
    pool: Db,
    online: Arc<DashMap<u32, Presence>>,
}

impl Friends {
    pub fn new(pool: Db) -> Self {
        Self { pool, online: Arc::new(DashMap::new()) }
    }

    /// Where `account` is now, if online.
    pub fn presence(&self, account: u32) -> Option<Presence> {
        self.online.get(&account).map(|p| p.clone())
    }

    /// How many accounts are in game now (on any map).
    pub fn online_count(&self) -> usize {
        self.online.len()
    }

    /// Every account in game now, with where it is (status and admin tools).
    pub fn online_players(&self) -> Vec<(u32, Presence)> {
        self.online.iter().map(|e| (*e.key(), e.value().clone())).collect()
    }

    /// `account` entered a map. Friends hear it: a toast when they just
    /// came online, a quiet list refresh when they moved.
    pub async fn present(&self, account: u32, p: Presence) {
        let fresh = self.online.insert(account, p.clone()).is_none();
        let ev = if fresh { FriendEvent::Note { op: note::ONLINE, account, name: p.name } } else { FriendEvent::Refresh };
        self.tell_friends(account, ev).await;
    }

    /// The socket `conn_id` of `account` closed; a newer socket's record
    /// stays.
    pub async fn gone(&self, account: u32, conn_id: Uuid) {
        if self.online.remove_if(&account, |_, p| p.conn_id == conn_id).is_some() {
            self.tell_friends(account, FriendEvent::Refresh).await;
        }
    }

    async fn tell_friends(&self, account: u32, ev: FriendEvent) {
        let Ok(friends) = FriendRepo::friends_of(&self.pool, account).await else { return };
        for f in friends {
            self.tell(f, ev.clone());
        }
    }

    /// Hands `ev` to `account`'s socket if online (dropped when its
    /// channel is full: the next refresh catches up).
    pub fn tell(&self, account: u32, ev: FriendEvent) {
        if let Some(p) = self.online.get(&account) {
            let _ = p.tx.try_send(ev);
        }
    }

    /// The name to show for `account`: the character played now, else the
    /// last one played.
    async fn name_of(&self, account: u32) -> String {
        if let Some(p) = self.presence(account) {
            return p.name;
        }
        FriendRepo::characters(&self.pool, account)
            .await
            .ok()
            .and_then(|c| c.into_iter().next().map(|c| c.1))
            .unwrap_or_else(|| "Someone".into())
    }

    /// `me`'s list: requests to them first, then friends online, offline,
    /// and their own requests; by name within each.
    pub async fn list(&self, me: u32) -> Result<Vec<FriendInfo>, FriendError> {
        let mut out = Vec::new();
        for (account, link) in FriendRepo::list(&self.pool, me).await? {
            let chars = FriendRepo::characters(&self.pool, account).await?;
            let last = chars.first().cloned().unwrap_or_default();
            let mut row = FriendInfo {
                account,
                link: link.code(),
                name: last.1,
                online: false,
                class: last.2,
                level: last.3,
                map: String::new(),
                channel: 0,
                characters: chars.iter().map(|c| c.0).collect(),
            };
            if link == Link::Friend
                && let Some(p) = self.presence(account)
            {
                row.online = true;
                row.name = p.name;
                row.class = p.class;
                row.level = p.level;
                row.map = p.map;
                row.channel = p.channel;
            }
            out.push(row);
        }
        let rank = |f: &FriendInfo| match (f.link, f.online) {
            (1, _) => 0,
            (0, true) => 1,
            (0, false) => 2,
            _ => 3,
        };
        out.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(out)
    }

    /// Runs `op` for `me` (playing `my_name`): the other side hears about
    /// it; returns what goes back to `me` (the list, then a note).
    pub async fn act(&self, me: u32, my_name: &str, op: FriendOp) -> Result<Vec<ServerMsg>, FriendError> {
        let pool = &self.pool;
        let mut notes = Vec::new();
        match op {
            FriendOp::List => {}
            FriendOp::Add { entity_id } => {
                let other = FriendRepo::account_of_character(pool, entity_id).await?.ok_or(FriendError::Unknown)?;
                let name = FriendRepo::characters(pool, other)
                    .await?
                    .into_iter()
                    .find(|c| c.0 == entity_id)
                    .map(|c| c.1)
                    .unwrap_or_default();
                notes.push(self.request(me, my_name, other, name).await?);
            }
            FriendOp::AddName { name } => {
                let other = FriendRepo::account_of_name(pool, name.trim()).await?.ok_or(FriendError::Unknown)?;
                notes.push(self.request(me, my_name, other, name.trim().to_string()).await?);
            }
            FriendOp::Accept { account } => {
                FriendRepo::accept(pool, me, account).await?;
                self.tell(account, FriendEvent::Note { op: note::ACCEPTED, account: me, name: my_name.into() });
                notes.push(ServerMsg::FriendNote { op: note::ACCEPTED, account, name: self.name_of(account).await });
            }
            FriendOp::Decline { account } => {
                FriendRepo::decline(pool, me, account).await?;
                self.tell(account, FriendEvent::Refresh);
            }
            FriendOp::Remove { account } => {
                if FriendRepo::remove(pool, me, account).await? {
                    self.tell(account, FriendEvent::Refresh);
                    notes.push(ServerMsg::FriendNote { op: note::REMOVED, account, name: self.name_of(account).await });
                }
            }
        }
        let mut out = vec![ServerMsg::Friends { friends: self.list(me).await? }];
        out.extend(notes);
        Ok(out)
    }

    async fn request(&self, me: u32, my_name: &str, other: u32, name: String) -> Result<ServerMsg, FriendError> {
        let op = match FriendRepo::request(&self.pool, me, other).await? {
            Requested::Sent => {
                self.tell(other, FriendEvent::Note { op: note::REQUEST, account: me, name: my_name.into() });
                note::SENT
            }
            Requested::Accepted => {
                self.tell(other, FriendEvent::Note { op: note::ACCEPTED, account: me, name: my_name.into() });
                note::ACCEPTED
            }
        };
        Ok(ServerMsg::FriendNote { op, account: other, name })
    }

    /// What a socket sends on a [`FriendEvent`].
    pub async fn on_event(&self, me: u32, ev: FriendEvent) -> Vec<ServerMsg> {
        let Ok(friends) = self.list(me).await else { return Vec::new() };
        let mut out = vec![ServerMsg::Friends { friends }];
        if let FriendEvent::Note { op, account, name } = ev {
            out.push(ServerMsg::FriendNote { op, account, name });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn setup() -> (Friends, Vec<u32>) {
        let pool = crate::db::testing::pool().await;
        let mut ids = Vec::new();
        for name in ["ann", "bob", "cid"] {
            let id: i64 = sqlx::query_scalar("INSERT INTO users (username, password_hash) VALUES ($1, 'x') RETURNING id")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO characters (user_id, name, archetype, level) VALUES ($1, $2, 'mage', 7)")
                .bind(id)
                .bind(name.to_uppercase())
                .execute(&pool)
                .await
                .unwrap();
            ids.push(id as u32);
        }
        (Friends::new(pool), ids)
    }

    fn online(account: u32, name: &str, map: &str, channel: u16) -> (Uuid, mpsc::Receiver<FriendEvent>, Presence) {
        let (tx, rx) = mpsc::channel(FRIEND_CHANNEL_CAPACITY);
        let conn = Uuid::new_v4();
        let p = Presence {
            conn_id: conn,
            character_id: account,
            name: name.into(),
            class: "mage".into(),
            level: 9,
            map: map.into(),
            channel,
            tx,
        };
        (conn, rx, p)
    }

    #[tokio::test]
    async fn presence_reaches_friends_across_instances() {
        let (f, ids) = setup().await;
        let (ann, bob, cid) = (ids[0], ids[1], ids[2]);
        let (_, mut ann_rx, p) = online(ann, "ANN", "Prontera", 1);
        f.present(ann, p).await;

        // Bob asks by entity id (Ann's character id = her account id here).
        let ann_char = FriendRepo::characters(&f.pool, ann).await.unwrap()[0].0;
        let out = f.act(bob, "BOB", FriendOp::Add { entity_id: ann_char }).await.unwrap();
        assert!(matches!(&out[1], ServerMsg::FriendNote { op: note::SENT, name, .. } if name == "ANN"));
        assert_eq!(ann_rx.try_recv().unwrap(), FriendEvent::Note { op: note::REQUEST, account: bob, name: "BOB".into() });

        // Requests show no presence.
        let ServerMsg::Friends { friends } = &out[0] else { panic!() };
        assert_eq!((friends[0].link, friends[0].online, friends[0].map.as_str()), (2, false, ""));

        let out = f.act(ann, "ANN", FriendOp::Accept { account: bob }).await.unwrap();
        assert!(matches!(&out[1], ServerMsg::FriendNote { op: note::ACCEPTED, account, .. } if *account == bob));

        // Bob comes online on a stage (another instance): Ann hears it, and
        // her list says where he is.
        let (bob_conn, mut bob_rx, p) = online(bob, "BOB", "Poring Island", 0);
        f.present(bob, p).await;
        assert_eq!(ann_rx.try_recv().unwrap(), FriendEvent::Note { op: note::ONLINE, account: bob, name: "BOB".into() });
        let list = f.list(ann).await.unwrap();
        assert_eq!((list[0].online, list[0].map.as_str(), list[0].channel, list[0].level), (true, "Poring Island", 0, 9));
        // Ann's presence for Bob too.
        let list = f.list(bob).await.unwrap();
        assert_eq!((list[0].name.as_str(), list[0].map.as_str(), list[0].channel), ("ANN", "Prontera", 1));

        // Moving refreshes quietly; Cid (not a friend) hears nothing.
        let (_, _cid_rx, p) = online(cid, "CID", "Prontera", 1);
        f.present(cid, p).await;
        let (_, _bob2, mut p) = online(bob, "BOB", "Prontera", 2);
        p.conn_id = bob_conn;
        p.tx = f.presence(bob).unwrap().tx;
        f.present(bob, p).await;
        assert_eq!(ann_rx.try_recv().unwrap(), FriendEvent::Refresh);
        assert!(ann_rx.try_recv().is_err(), "cid is no friend of ann");

        // A stale socket does not take a newer one offline.
        f.gone(bob, Uuid::new_v4()).await;
        assert!(f.presence(bob).is_some());
        f.gone(bob, bob_conn).await;
        assert!(f.presence(bob).is_none());
        assert_eq!(ann_rx.try_recv().unwrap(), FriendEvent::Refresh);
        assert!(!f.list(ann).await.unwrap()[0].online);

        // Removing: both lists empty, the other side refreshes.
        let out = f.act(ann, "ANN", FriendOp::Remove { account: bob }).await.unwrap();
        assert!(matches!(&out[0], ServerMsg::Friends { friends } if friends.is_empty()));
        assert!(f.list(bob).await.unwrap().is_empty());
        let _ = bob_rx.try_recv();
    }

    #[tokio::test]
    async fn bad_requests() {
        let (f, ids) = setup().await;
        let ann = ids[0];
        let mine = FriendRepo::characters(&f.pool, ann).await.unwrap()[0].0;
        assert_eq!(f.act(ann, "ANN", FriendOp::Add { entity_id: mine }).await, Err(FriendError::SelfAdd));
        assert_eq!(f.act(ann, "ANN", FriendOp::Add { entity_id: 0x70_0001 }).await, Err(FriendError::Unknown));
        assert_eq!(f.act(ann, "ANN", FriendOp::AddName { name: "nobody".into() }).await, Err(FriendError::Unknown));
        assert_eq!(f.act(ann, "ANN", FriendOp::Accept { account: ids[1] }).await, Err(FriendError::NoRequest));
        // By name, any case; twice is a duplicate.
        f.act(ann, "ANN", FriendOp::AddName { name: " bob ".into() }).await.unwrap();
        assert_eq!(f.act(ann, "ANN", FriendOp::AddName { name: "BOB".into() }).await, Err(FriendError::AlreadyAsked));
    }
}
