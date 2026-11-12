//! Mission lobbies: parties forming in the hub before a stage launches.
//!
//! - `mission_select` at a board joins the oldest *public* lobby of that
//!   mission with a free slot, or opens a new one; `public: false` always
//!   opens a private lobby.
//! - Every lobby has a short join code; a private lobby is reachable only by
//!   its code.
//! - The first member is the leader; when the leader leaves, the next oldest
//!   member takes over. The leader starts the mission once every other
//!   member is ready (the ready check) and may kick members.
//! - Starting launches a stage instance and hands its ticket to every member;
//!   the lobby is then gone.
//!
//! Lobbies are global (players from any hub channel can party up) and change
//! only on player actions, so one mutex is enough. Members are reached through
//! their socket task's lobby channel.

use super::hub::PlayerProfile;
use super::missions::{self, MissionInfo};
use super::protocol::{LobbyInfo, LobbyMemberInfo, ServerMsg};
use super::stage::{StageDirectory, StageTicket};
use rand::Rng;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, info};
use uuid::Uuid;

/// Depth of a socket's lobby channel (lobby updates are tiny and rare).
pub const LOBBY_CHANNEL_CAPACITY: usize = 32;
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 5;

/// What a lobby sends to a member's socket task.
#[derive(Clone, Debug)]
pub enum LobbyEvent {
    Msg(ServerMsg),
    /// The mission started: move to this stage.
    Launch(StageTicket),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LobbyError {
    #[error("already in a lobby; leave it first")]
    AlreadyInLobby,
    #[error("not in a lobby")]
    NotInLobby,
    #[error("no lobby with that code")]
    NoSuchLobby,
    #[error("the lobby is full")]
    LobbyFull,
    #[error("only the party leader can do that")]
    NotLeader,
    #[error("waiting for {0} to be ready")]
    NotReady(String),
    #[error("no member with that id")]
    NoSuchMember,
    #[error("no mission {0}")]
    UnknownMission(String),
    #[error("{0} cannot be played on this server right now")]
    MissionUnavailable(String),
    #[error("all stages are busy, try again shortly")]
    StageFull,
}

impl LobbyError {
    pub fn code(&self) -> &'static str {
        match self {
            LobbyError::AlreadyInLobby => "already_in_lobby",
            LobbyError::NotInLobby => "not_in_lobby",
            LobbyError::NoSuchLobby => "no_such_lobby",
            LobbyError::LobbyFull => "lobby_full",
            LobbyError::NotLeader => "not_leader",
            LobbyError::NotReady(_) => "not_ready",
            LobbyError::NoSuchMember => "no_such_member",
            LobbyError::UnknownMission(_) => "unknown_mission",
            LobbyError::MissionUnavailable(_) => "mission_unavailable",
            LobbyError::StageFull => "stage_full",
        }
    }

    pub fn to_msg(&self) -> ServerMsg {
        ServerMsg::LobbyError { code: self.code().into(), message: self.to_string() }
    }
}

struct Member {
    profile: PlayerProfile,
    session_id: Uuid,
    ready: bool,
    tx: mpsc::Sender<LobbyEvent>,
}

struct Lobby {
    id: u32,
    code: String,
    mission: &'static MissionInfo,
    public: bool,
    /// `members[0]` is the leader.
    members: Vec<Member>,
}

impl Lobby {
    fn info(&self) -> LobbyInfo {
        LobbyInfo {
            lobby_id: self.id,
            code: self.code.clone(),
            mission: self.mission.clone(),
            public: self.public,
            members: self
                .members
                .iter()
                .enumerate()
                .map(|(i, m)| LobbyMemberInfo {
                    entity_id: m.profile.character_id,
                    name: m.profile.name.clone(),
                    archetype: m.profile.archetype.clone(),
                    ready: m.ready,
                    leader: i == 0,
                })
                .collect(),
        }
    }

    fn broadcast(&self) {
        let msg = ServerMsg::LobbyState { lobby: self.info() };
        for m in &self.members {
            // A dead socket is cleaned up by its own task (it calls leave).
            let _ = m.tx.try_send(LobbyEvent::Msg(msg.clone()));
        }
    }

    fn has_room(&self) -> bool {
        self.members.len() < self.mission.max_players as usize
    }
}

#[derive(Default)]
struct LobbyState {
    lobbies: BTreeMap<u32, Lobby>,
    /// user id → lobby id.
    by_user: HashMap<u32, u32>,
    by_code: HashMap<String, u32>,
    next_id: u32,
}

impl LobbyState {
    /// The caller's lobby, if `session_id` still owns the membership.
    fn lobby_of(&mut self, user_id: u32, session_id: Uuid) -> Result<&mut Lobby, LobbyError> {
        let id = *self.by_user.get(&user_id).ok_or(LobbyError::NotInLobby)?;
        let lobby = self.lobbies.get_mut(&id).ok_or(LobbyError::NotInLobby)?;
        match lobby.members.iter().any(|m| m.profile.user_id == user_id && m.session_id == session_id) {
            true => Ok(lobby),
            false => Err(LobbyError::NotInLobby),
        }
    }

    /// Removes a member; promotes a new leader or deletes an empty lobby, and
    /// tells the rest. Returns the removed member's channel.
    fn remove(&mut self, user_id: u32) -> Option<mpsc::Sender<LobbyEvent>> {
        let id = self.by_user.remove(&user_id)?;
        let lobby = self.lobbies.get_mut(&id)?;
        let i = lobby.members.iter().position(|m| m.profile.user_id == user_id)?;
        let member = lobby.members.remove(i);
        if lobby.members.is_empty() {
            let lobby = self.lobbies.remove(&id).unwrap();
            self.by_code.remove(&lobby.code);
            debug!("Lobby {id} closed (empty)");
        } else {
            lobby.broadcast();
        }
        Some(member.tx)
    }

    fn new_code(&self) -> String {
        let mut rng = rand::thread_rng();
        loop {
            let code: String =
                (0..CODE_LEN).map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char).collect();
            if !self.by_code.contains_key(&code) {
                return code;
            }
        }
    }
}

#[derive(Clone)]
pub struct LobbyDirectory {
    stages: StageDirectory,
    state: Arc<Mutex<LobbyState>>,
}

impl LobbyDirectory {
    pub fn new(stages: StageDirectory) -> Self {
        Self { stages, state: Arc::new(Mutex::new(LobbyState::default())) }
    }

    pub fn stages(&self) -> &StageDirectory {
        &self.stages
    }

    /// Mission board choice: matchmake into a public lobby or open a lobby.
    /// A membership owned by another session of the same user (a displaced
    /// tab) is replaced.
    pub fn select(
        &self,
        profile: PlayerProfile,
        session_id: Uuid,
        mission: &str,
        public: bool,
        tx: mpsc::Sender<LobbyEvent>,
    ) -> Result<u32, LobbyError> {
        let mission = missions::find(mission).ok_or_else(|| LobbyError::UnknownMission(mission.into()))?;
        if !self.stages.supports(mission) {
            return Err(LobbyError::MissionUnavailable(mission.name.into()));
        }
        let mut st = self.state.lock().unwrap();
        Self::evict_stale(&mut st, profile.user_id, session_id)?;
        let found = public
            .then(|| {
                st.lobbies.values().find(|l| l.public && l.mission.id == mission.id && l.has_room()).map(|l| l.id)
            })
            .flatten();
        let id = match found {
            Some(id) => id,
            None => {
                st.next_id = st.next_id.wrapping_add(1).max(1);
                let id = st.next_id;
                let code = st.new_code();
                st.by_code.insert(code.clone(), id);
                st.lobbies.insert(id, Lobby { id, code, mission, public, members: Vec::new() });
                info!("Lobby {id} opened for {} ({})", mission.id, if public { "public" } else { "private" });
                id
            }
        };
        Self::add(&mut st, id, profile, session_id, tx);
        Ok(id)
    }

    /// Joins a lobby by code (case-insensitive).
    pub fn join_code(
        &self,
        profile: PlayerProfile,
        session_id: Uuid,
        code: &str,
        tx: mpsc::Sender<LobbyEvent>,
    ) -> Result<u32, LobbyError> {
        let code = code.trim().to_ascii_uppercase();
        let mut st = self.state.lock().unwrap();
        let id = *st.by_code.get(&code).ok_or(LobbyError::NoSuchLobby)?;
        // Drops an older session's membership first, so a new tab can rejoin
        // the lobby its predecessor is still listed in.
        Self::evict_stale(&mut st, profile.user_id, session_id)?;
        let lobby = st.lobbies.get(&id).ok_or(LobbyError::NoSuchLobby)?;
        if !lobby.has_room() {
            return Err(LobbyError::LobbyFull);
        }
        Self::add(&mut st, id, profile, session_id, tx);
        Ok(id)
    }

    /// Fails if this session is already in a lobby; drops a membership held
    /// by an older session of the same user.
    fn evict_stale(st: &mut LobbyState, user_id: u32, session_id: Uuid) -> Result<(), LobbyError> {
        if st.lobby_of(user_id, session_id).is_ok() {
            return Err(LobbyError::AlreadyInLobby);
        }
        if let Some(tx) = st.remove(user_id) {
            let _ = tx.try_send(LobbyEvent::Msg(ServerMsg::LobbyLeft { reason: "replaced".into() }));
        }
        Ok(())
    }

    fn add(st: &mut LobbyState, id: u32, profile: PlayerProfile, session_id: Uuid, tx: mpsc::Sender<LobbyEvent>) {
        st.by_user.insert(profile.user_id, id);
        let lobby = st.lobbies.get_mut(&id).expect("lobby exists");
        lobby.members.push(Member { profile, session_id, ready: false, tx });
        lobby.broadcast();
    }

    pub fn set_ready(&self, user_id: u32, session_id: Uuid, ready: bool) -> Result<(), LobbyError> {
        let mut st = self.state.lock().unwrap();
        let lobby = st.lobby_of(user_id, session_id)?;
        let m = lobby.members.iter_mut().find(|m| m.profile.user_id == user_id).unwrap();
        if m.ready != ready {
            m.ready = ready;
            lobby.broadcast();
        }
        Ok(())
    }

    /// Leaves the lobby if `session_id` owns the membership.
    pub fn leave(&self, user_id: u32, session_id: Uuid, reason: &str) -> bool {
        let mut st = self.state.lock().unwrap();
        if st.lobby_of(user_id, session_id).is_err() {
            return false;
        }
        if let Some(tx) = st.remove(user_id) {
            let _ = tx.try_send(LobbyEvent::Msg(ServerMsg::LobbyLeft { reason: reason.into() }));
        }
        true
    }

    /// Leader removes a member by entity (character) id.
    pub fn kick(&self, user_id: u32, session_id: Uuid, entity_id: u32) -> Result<(), LobbyError> {
        let mut st = self.state.lock().unwrap();
        let lobby = st.lobby_of(user_id, session_id)?;
        if lobby.members[0].profile.user_id != user_id {
            return Err(LobbyError::NotLeader);
        }
        let target = lobby
            .members
            .iter()
            .find(|m| m.profile.character_id == entity_id && m.profile.user_id != user_id)
            .map(|m| m.profile.user_id)
            .ok_or(LobbyError::NoSuchMember)?;
        if let Some(tx) = st.remove(target) {
            let _ = tx.try_send(LobbyEvent::Msg(ServerMsg::LobbyLeft { reason: "kicked".into() }));
        }
        Ok(())
    }

    /// Leader starts the mission: everyone else must be ready. Launches the
    /// stage and hands every member its ticket; the lobby is closed.
    pub fn start(&self, user_id: u32, session_id: Uuid) -> Result<u32, LobbyError> {
        let mut st = self.state.lock().unwrap();
        let lobby = st.lobby_of(user_id, session_id)?;
        if lobby.members[0].profile.user_id != user_id {
            return Err(LobbyError::NotLeader);
        }
        if let Some(m) = lobby.members[1..].iter().find(|m| !m.ready) {
            return Err(LobbyError::NotReady(m.profile.name.clone()));
        }
        let party: Vec<u32> = lobby.members.iter().map(|m| m.profile.user_id).collect();
        let ticket = self.stages.launch(lobby.mission, &party).map_err(|e| match e {
            super::stage::StageError::Full => LobbyError::StageFull,
            _ => LobbyError::MissionUnavailable(lobby.mission.name.into()),
        })?;
        let id = lobby.id;
        let lobby = st.lobbies.remove(&id).unwrap();
        st.by_code.remove(&lobby.code);
        for m in &lobby.members {
            st.by_user.remove(&m.profile.user_id);
            if m.tx.try_send(LobbyEvent::Launch(ticket.clone())).is_err() {
                // Dead or stalled socket: it will not come; do not hold the
                // stage open for it.
                self.stages.abandon(ticket.stage_id, m.profile.user_id);
            }
        }
        info!("Lobby {id} launched Stage-{} with {} player(s)", ticket.stage_id, lobby.members.len());
        Ok(ticket.stage_id)
    }

    pub fn lobby_of(&self, user_id: u32) -> Option<u32> {
        self.state.lock().unwrap().by_user.get(&user_id).copied()
    }

    pub fn open_lobbies(&self) -> usize {
        self.state.lock().unwrap().lobbies.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::hub::test_support::map;
    use crate::world::hub::HubWorld;
    use crate::world::map::{CameraConstraint, LocationKind};
    use crate::world::stage::StageConfig;

    const MISSION: &str = "poring_island_survival";

    fn lobbies(max_stages: usize) -> LobbyDirectory {
        let field = map(
            "prt_fild08",
            LocationKind::Field,
            CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
            &["#####", "#...#", "#...#", "#####"],
            vec![],
        );
        let stages = StageDirectory::new(
            vec![HubWorld::new(vec![field], "prt_fild08").unwrap()],
            StageConfig { max_stages, ..Default::default() },
        );
        LobbyDirectory::new(stages)
    }

    fn profile(user_id: u32) -> PlayerProfile {
        PlayerProfile { user_id, character_id: 100 + user_id, name: format!("p{user_id}"), archetype: "novice".into() }
    }

    fn sid(user_id: u32) -> Uuid {
        Uuid::from_u128(user_id as u128)
    }

    struct Client {
        user: u32,
        tx: mpsc::Sender<LobbyEvent>,
        rx: mpsc::Receiver<LobbyEvent>,
    }

    impl Client {
        fn new(user: u32) -> Self {
            let (tx, rx) = mpsc::channel(LOBBY_CHANNEL_CAPACITY);
            Self { user, tx, rx }
        }

        /// Drains events; returns the last lobby state and anything else.
        fn drain(&mut self) -> (Option<LobbyInfo>, Vec<LobbyEvent>) {
            let mut state = None;
            let mut other = Vec::new();
            while let Ok(e) = self.rx.try_recv() {
                match e {
                    LobbyEvent::Msg(ServerMsg::LobbyState { lobby }) => state = Some(lobby),
                    e => other.push(e),
                }
            }
            (state, other)
        }

        fn select(&self, dir: &LobbyDirectory, public: bool) -> Result<u32, LobbyError> {
            dir.select(profile(self.user), sid(self.user), MISSION, public, self.tx.clone())
        }
    }

    #[tokio::test]
    async fn public_lobbies_fill_to_four_then_open_another() {
        let dir = lobbies(4);
        let mut clients: Vec<Client> = (1..=5).map(Client::new).collect();
        let ids: Vec<u32> = clients.iter().map(|c| c.select(&dir, true).unwrap()).collect();
        assert_eq!(&ids[..4], &[ids[0]; 4], "first four share a lobby");
        assert_ne!(ids[4], ids[0], "fifth opens a new one");
        let (state, _) = clients[0].drain();
        let state = state.unwrap();
        assert_eq!(state.members.len(), 4);
        assert!(state.members[0].leader && !state.members[1].leader);
        assert_eq!(state.code.len(), CODE_LEN);
        assert_eq!(clients[0].select(&dir, true).unwrap_err(), LobbyError::AlreadyInLobby);
    }

    #[tokio::test]
    async fn private_lobby_only_by_code() {
        let dir = lobbies(4);
        let mut a = Client::new(1);
        let b = Client::new(2);
        let c = Client::new(3);
        let private = a.select(&dir, false).unwrap();
        let public = b.select(&dir, true).unwrap();
        assert_ne!(private, public, "public matchmaking skips private lobbies");
        let code = a.drain().0.unwrap().code;
        assert_eq!(
            dir.join_code(profile(3), sid(3), &code.to_lowercase(), c.tx.clone()).unwrap(),
            private,
            "codes are case-insensitive"
        );
        assert_eq!(dir.join_code(profile(4), sid(4), "ZZZZZ", c.tx.clone()).unwrap_err(), LobbyError::NoSuchLobby);
        assert_eq!(a.drain().0.unwrap().members.len(), 2);
    }

    #[tokio::test]
    async fn ready_check_gates_start_and_launch_hands_tickets() {
        let dir = lobbies(4);
        let mut a = Client::new(1);
        let mut b = Client::new(2);
        a.select(&dir, true).unwrap();
        b.select(&dir, true).unwrap();
        assert_eq!(dir.start(2, sid(2)).unwrap_err(), LobbyError::NotLeader);
        assert_eq!(dir.start(1, sid(1)).unwrap_err(), LobbyError::NotReady("p2".into()));
        dir.set_ready(2, sid(2), true).unwrap();
        assert!(b.drain().0.unwrap().members[1].ready);
        let stage = dir.start(1, sid(1)).unwrap();
        for c in [&mut a, &mut b] {
            let (_, events) = c.drain();
            assert!(
                matches!(&events[..], [LobbyEvent::Launch(t)] if t.stage_id == stage && t.mission == MISSION),
                "{events:?}"
            );
        }
        assert_eq!(dir.open_lobbies(), 0);
        assert_eq!(dir.lobby_of(1), None);
        assert_eq!(dir.stages().active(), 1);
    }

    #[tokio::test]
    async fn solo_leader_can_start_and_stage_full_keeps_lobby() {
        let dir = lobbies(1);
        let a = Client::new(1);
        let b = Client::new(2);
        a.select(&dir, false).unwrap();
        b.select(&dir, false).unwrap();
        dir.start(1, sid(1)).unwrap();
        assert_eq!(dir.start(2, sid(2)).unwrap_err(), LobbyError::StageFull);
        assert!(dir.lobby_of(2).is_some(), "lobby survives a failed start");
    }

    #[tokio::test]
    async fn leader_leaving_promotes_next_and_kick_works() {
        let dir = lobbies(4);
        let mut a = Client::new(1);
        let mut b = Client::new(2);
        let mut c = Client::new(3);
        for x in [&a, &b, &c] {
            x.select(&dir, true).unwrap();
        }
        assert!(dir.leave(1, sid(1), "left"));
        let (_, events) = a.drain();
        assert!(matches!(&events[..], [LobbyEvent::Msg(ServerMsg::LobbyLeft { reason })] if reason == "left"));
        let state = b.drain().0.unwrap();
        assert_eq!(state.members.len(), 2);
        assert!(state.members[0].leader && state.members[0].entity_id == 102, "p2 leads now");

        assert_eq!(dir.kick(3, sid(3), 102).unwrap_err(), LobbyError::NotLeader);
        assert_eq!(dir.kick(2, sid(2), 102).unwrap_err(), LobbyError::NoSuchMember, "cannot kick yourself");
        dir.kick(2, sid(2), 103).unwrap();
        let (_, events) = c.drain();
        assert!(matches!(&events[..], [LobbyEvent::Msg(ServerMsg::LobbyLeft { reason })] if reason == "kicked"));
        assert!(dir.leave(2, sid(2), "left"));
        assert_eq!(dir.open_lobbies(), 0, "empty lobby closes");
    }

    #[tokio::test]
    async fn stale_session_is_replaced_and_cannot_act() {
        let dir = lobbies(4);
        let mut old = Client::new(1);
        old.select(&dir, true).unwrap();
        old.drain();
        // Same user from a new session (displaced tab): the old membership goes.
        let fresh = Client::new(1);
        dir.select(profile(1), Uuid::from_u128(99), MISSION, true, fresh.tx.clone()).unwrap();
        let (_, events) = old.drain();
        assert!(matches!(&events[..], [LobbyEvent::Msg(ServerMsg::LobbyLeft { reason })] if reason == "replaced"));
        assert!(!dir.leave(1, sid(1), "left"), "the old session's late leave is ignored");
        assert_eq!(dir.set_ready(1, sid(1), true).unwrap_err(), LobbyError::NotInLobby);
        assert!(dir.lobby_of(1).is_some());
    }

    #[tokio::test]
    async fn unknown_and_unavailable_missions() {
        let dir = lobbies(4);
        let a = Client::new(1);
        assert_eq!(
            dir.select(profile(1), sid(1), "nope", true, a.tx.clone()).unwrap_err(),
            LobbyError::UnknownMission("nope".into())
        );
        let empty = LobbyDirectory::new(StageDirectory::new(vec![], StageConfig::default()));
        assert!(matches!(
            empty.select(profile(1), sid(1), MISSION, true, a.tx.clone()),
            Err(LobbyError::MissionUnavailable(_))
        ));
    }
}
