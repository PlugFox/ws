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
//! - Party invitations (`invite` / `answer`): the leader, or a player in
//!   no party, invites someone in no party; the invitee accepts or declines
//!   within [`INVITE_TTL`]. Accepting joins the leader's lobby; an inviter
//!   without one gets a private lobby of the first playable mission with
//!   both of them in it.
//!
//! - Quick start (`quick_start`, the mission window's Start): a player in
//!   no party launches the mission alone at once, a party of one that is
//!   never listed; a party leader starts the party's mission under the
//!   usual rules (everyone ready, levels for the curses); anyone else in a
//!   party cannot.
//!
//! - Mission modifiers (`modifiers.rs`): the leader sets the difficulty
//!   tier and curses; every member sees them (`MODIFIERS` after each
//!   `LOBBY_STATE`), a change unreadies the others, and the launch carries
//!   them into the stage. A curse with a level requirement holds the start
//!   until every member meets it.
//!
//! Lobbies are global (players from any hub channel can party up) and change
//! only on player actions, so one mutex is enough. Members are reached through
//! their socket task's lobby channel.

use super::hub::PlayerProfile;
use super::missions::{self, MissionInfo};
use super::modifiers::{self, Modifiers};
use super::protocol::{LobbyInfo, LobbyMemberInfo, ServerMsg};
use super::stage::{StageDirectory, StageTicket};
use rand::Rng;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, info};
use uuid::Uuid;

/// Depth of a socket's lobby channel (lobby updates are tiny and rare).
pub const LOBBY_CHANNEL_CAPACITY: usize = 32;
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 5;
/// How long a party invitation waits for an answer.
pub const INVITE_TTL: Duration = Duration::from_secs(30);
/// Open invitations one player may have out at once (the oldest lapses).
const INVITES_PER_PLAYER: usize = 4;

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
    #[error("that player is not here")]
    NoSuchPlayer,
    #[error("they are already in a party")]
    InParty,
    #[error("the invitation is no longer valid")]
    InviteExpired,
    #[error("{0}")]
    BadModifier(String),
    #[error("your party is set for {0}: leave it to pick another mission")]
    PartyMission(String),
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
            LobbyError::NoSuchPlayer => "no_such_player",
            LobbyError::InParty => "in_party",
            LobbyError::InviteExpired => "invite_expired",
            LobbyError::BadModifier(_) => "bad_modifier",
            LobbyError::PartyMission(_) => "party_mission",
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
    /// The leader's difficulty and curses.
    mods: Modifiers,
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
        let mods = self.mods.state_msg(modifiers::scope::LOBBY);
        for m in &self.members {
            // A dead socket is cleaned up by its own task (it calls leave).
            let _ = m.tx.try_send(LobbyEvent::Msg(msg.clone()));
            let _ = m.tx.try_send(LobbyEvent::Msg(mods.clone()));
        }
    }

    fn has_room(&self) -> bool {
        self.members.len() < self.mission.max_players as usize
    }
}

/// A party invitation waiting for the invitee's answer.
struct Invite {
    /// The inviter, as they join the lobby on an accept.
    from: Member,
    /// The inviter's lobby when they invited (`None`: none yet).
    lobby: Option<u32>,
    /// The invitee's entity (character) id.
    to: u32,
    at: Instant,
}

#[derive(Default)]
struct LobbyState {
    lobbies: BTreeMap<u32, Lobby>,
    invites: Vec<Invite>,
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

    /// Lobby holding the character `entity_id`, if any.
    fn lobby_with(&self, entity_id: u32) -> Option<u32> {
        self.lobbies.values().find(|l| l.members.iter().any(|m| m.profile.character_id == entity_id)).map(|l| l.id)
    }

    fn prune_invites(&mut self) {
        self.invites.retain(|i| i.at.elapsed() < INVITE_TTL);
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
                st.lobbies.insert(id, Lobby { id, code, mission, public, members: Vec::new(), mods: Modifiers::NONE });
                info!("Lobby {id} opened for {} ({})", mission.id, if public { "public" } else { "private" });
                id
            }
        };
        self.add(&mut st, id, profile, session_id, tx);
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
        self.add(&mut st, id, profile, session_id, tx);
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

    fn add(&self, st: &mut LobbyState, id: u32, profile: PlayerProfile, session_id: Uuid, tx: mpsc::Sender<LobbyEvent>) {
        st.by_user.insert(profile.user_id, id);
        let lobby = st.lobbies.get_mut(&id).expect("lobby exists");
        // The modifier table and the mission's brief first, so the mission
        // window can show the party's choice (a member who joined by code
        // never talked to a mission giver).
        let _ = tx.try_send(LobbyEvent::Msg(modifiers::defs_msg()));
        if let Some(catalog) = self.stages.monsters() {
            let brief = missions::brief(lobby.mission, &lobby.mission.waves, catalog);
            let _ = tx.try_send(LobbyEvent::Msg(ServerMsg::MissionInfo { brief }));
        }
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

    /// Leader sets the run's difficulty tier and curses (validated against
    /// the table and the leader's level). Everyone else is unreadied: they
    /// agreed to other terms.
    pub fn set_modifiers(&self, user_id: u32, session_id: Uuid, tier: u8, mask: u32) -> Result<(), LobbyError> {
        let mut st = self.state.lock().unwrap();
        let lobby = st.lobby_of(user_id, session_id)?;
        if lobby.members[0].profile.user_id != user_id {
            return Err(LobbyError::NotLeader);
        }
        let mods = Modifiers::validate(tier, mask, lobby.members[0].profile.level)
            .map_err(|e| LobbyError::BadModifier(e.to_string()))?;
        if mods != lobby.mods {
            lobby.mods = mods;
            for m in &mut lobby.members[1..] {
                m.ready = false;
            }
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
        let id = lobby.id;
        self.launch(&mut st, id)
    }

    /// Quick start (`MISSION_START`). In no party: checks the mission and
    /// the modifiers against the player's level, then launches a party of
    /// one at once (never listed, so nobody can join it). In a party: the
    /// leader starts it as with [`Self::start`], after applying `tier` and
    /// `mask` like [`Self::set_modifiers`] (a change unreadies the others,
    /// so the start then waits for them); the mission must be the party's.
    pub fn quick_start(
        &self,
        profile: PlayerProfile,
        session_id: Uuid,
        mission: &str,
        tier: u8,
        mask: u32,
        tx: mpsc::Sender<LobbyEvent>,
    ) -> Result<u32, LobbyError> {
        let mut st = self.state.lock().unwrap();
        let user_id = profile.user_id;
        if let Ok(lobby) = st.lobby_of(user_id, session_id) {
            if lobby.members[0].profile.user_id != user_id {
                return Err(LobbyError::NotLeader);
            }
            if lobby.mission.id != mission {
                return Err(LobbyError::PartyMission(lobby.mission.name.into()));
            }
            let mods = Modifiers::validate(tier, mask, lobby.members[0].profile.level)
                .map_err(|e| LobbyError::BadModifier(e.to_string()))?;
            if mods != lobby.mods {
                lobby.mods = mods;
                for m in &mut lobby.members[1..] {
                    m.ready = false;
                }
                lobby.broadcast();
            }
            let id = lobby.id;
            return self.launch(&mut st, id);
        }
        let mission = missions::find(mission).ok_or_else(|| LobbyError::UnknownMission(mission.into()))?;
        if !self.stages.supports(mission) {
            return Err(LobbyError::MissionUnavailable(mission.name.into()));
        }
        let mods = Modifiers::validate(tier, mask, profile.level).map_err(|e| LobbyError::BadModifier(e.to_string()))?;
        // A membership an older tab of this player holds is dropped.
        Self::evict_stale(&mut st, user_id, session_id)?;
        let ticket = self.stages.launch_with(mission, mods, &[user_id]).map_err(|e| match e {
            super::stage::StageError::Full => LobbyError::StageFull,
            _ => LobbyError::MissionUnavailable(mission.name.into()),
        })?;
        st.invites.retain(|i| i.from.profile.user_id != user_id);
        if tx.try_send(LobbyEvent::Launch(ticket.clone())).is_err() {
            self.stages.abandon(ticket.stage_id, user_id);
        }
        info!("Quick start: Stage-{} for {} (solo)", ticket.stage_id, mission.id);
        Ok(ticket.stage_id)
    }

    /// Launches lobby `id` once every member but the leader is ready and
    /// meets the curses' levels; hands out the tickets and closes it.
    fn launch(&self, st: &mut LobbyState, id: u32) -> Result<u32, LobbyError> {
        let lobby = st.lobbies.get_mut(&id).ok_or(LobbyError::NotInLobby)?;
        if let Some(m) = lobby.members[1..].iter().find(|m| !m.ready) {
            return Err(LobbyError::NotReady(m.profile.name.clone()));
        }
        for m in &lobby.members {
            lobby.mods.check_level(m.profile.level).map_err(|e| LobbyError::BadModifier(format!("{}: {e}", m.profile.name)))?;
        }
        let party: Vec<u32> = lobby.members.iter().map(|m| m.profile.user_id).collect();
        let ticket = self.stages.launch_with(lobby.mission, lobby.mods, &party).map_err(|e| match e {
            super::stage::StageError::Full => LobbyError::StageFull,
            _ => LobbyError::MissionUnavailable(lobby.mission.name.into()),
        })?;
        let lobby = st.lobbies.remove(&id).unwrap();
        st.by_code.remove(&lobby.code);
        st.invites.retain(|i| !lobby.members.iter().any(|m| m.profile.user_id == i.from.profile.user_id));
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

    /// The mission a party formed by invitation starts with.
    fn default_mission(&self) -> Option<&'static MissionInfo> {
        missions::MISSIONS.iter().find(|m| self.stages.supports(m))
    }

    /// Invites the character `target` into the sender's party: they must
    /// lead their lobby (with room) or be in none, and the target must be
    /// in no party. Returns the mission name for the offer; the caller
    /// (the hub, which checks that the target is on the map) delivers it.
    /// A repeated invitation renews the old one.
    pub fn invite(
        &self,
        profile: PlayerProfile,
        session_id: Uuid,
        target: u32,
        tx: mpsc::Sender<LobbyEvent>,
    ) -> Result<String, LobbyError> {
        if target == profile.character_id {
            return Err(LobbyError::NoSuchPlayer);
        }
        let mut st = self.state.lock().unwrap();
        st.prune_invites();
        let user_id = profile.user_id;
        let (lobby, mission) = if st.by_user.contains_key(&user_id) {
            let lobby = st.lobby_of(user_id, session_id)?;
            if lobby.members[0].profile.user_id != user_id {
                return Err(LobbyError::NotLeader);
            }
            if !lobby.has_room() {
                return Err(LobbyError::LobbyFull);
            }
            (Some(lobby.id), lobby.mission)
        } else {
            let mission = self.default_mission().ok_or_else(|| LobbyError::MissionUnavailable("a mission".into()))?;
            (None, mission)
        };
        if st.lobby_with(target).is_some() {
            return Err(LobbyError::InParty);
        }
        st.invites.retain(|i| !(i.from.profile.user_id == user_id && i.to == target));
        if let Some(oldest) = st.invites.iter().position(|i| i.from.profile.user_id == user_id)
            && st.invites.iter().filter(|i| i.from.profile.user_id == user_id).count() >= INVITES_PER_PLAYER
        {
            st.invites.remove(oldest);
        }
        st.invites.push(Invite { from: Member { profile, session_id, ready: false, tx }, lobby, to: target, at: Instant::now() });
        Ok(mission.name.to_string())
    }

    /// The invitee answers the invitation of `from` (the inviter's entity
    /// id). Declining tells the inviter; accepting joins their lobby, or
    /// opens a private one for both when they had none. Fails when the
    /// invitation lapsed or no longer holds (the inviter left, lost the
    /// lead, launched; the party filled up).
    pub fn answer(
        &self,
        profile: PlayerProfile,
        session_id: Uuid,
        from: u32,
        accept: bool,
        tx: mpsc::Sender<LobbyEvent>,
    ) -> Result<(), LobbyError> {
        let mut st = self.state.lock().unwrap();
        st.prune_invites();
        let i = st
            .invites
            .iter()
            .position(|i| i.from.profile.character_id == from && i.to == profile.character_id)
            .ok_or(LobbyError::InviteExpired)?;
        let invite = st.invites.remove(i);
        let inviter_tx = invite.from.tx.clone();
        let reply = |accepted: bool| {
            let msg = ServerMsg::PartyAnswer { entity_id: profile.character_id, name: profile.name.clone(), accepted };
            let _ = inviter_tx.try_send(LobbyEvent::Msg(msg));
        };
        if !accept {
            reply(false);
            return Ok(());
        }
        let inviter = invite.from.profile.user_id;
        // Where the inviter is now must still be where they invited from.
        let now = st.by_user.get(&inviter).copied();
        if now != invite.lobby {
            return Err(LobbyError::InviteExpired);
        }
        if let Some(id) = now {
            let lobby = st.lobbies.get(&id).ok_or(LobbyError::InviteExpired)?;
            let lead = &lobby.members[0];
            if lead.profile.user_id != inviter || lead.session_id != invite.from.session_id {
                return Err(LobbyError::InviteExpired);
            }
            if !lobby.has_room() {
                return Err(LobbyError::LobbyFull);
            }
        }
        Self::evict_stale(&mut st, profile.user_id, session_id)?;
        let id = match now {
            Some(id) => id,
            None => {
                let mission = self.default_mission().ok_or(LobbyError::InviteExpired)?;
                st.next_id = st.next_id.wrapping_add(1).max(1);
                let id = st.next_id;
                let code = st.new_code();
                st.by_code.insert(code.clone(), id);
                st.lobbies.insert(id, Lobby { id, code, mission, public: false, members: Vec::new(), mods: Modifiers::NONE });
                info!("Lobby {id} opened for {} (invitation)", mission.id);
                let Member { profile, session_id, tx, .. } = invite.from;
                self.add(&mut st, id, profile, session_id, tx);
                id
            }
        };
        reply(true);
        self.add(&mut st, id, profile.clone(), session_id, tx);
        Ok(())
    }

    /// Drops the invitations `user_id` sent (they left the town or went).
    pub fn forget_invites(&self, user_id: u32) {
        self.state.lock().unwrap().invites.retain(|i| i.from.profile.user_id != user_id);
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
        PlayerProfile { user_id, character_id: 100 + user_id, name: format!("p{user_id}"), archetype: "novice".into(), look: crate::world::protocol::Look::default(), level: 1, exp: 0, weapon: String::new(), loadout: Default::default(), bag: Default::default() }
    }

    fn sid(user_id: u32) -> Uuid {
        Uuid::from_u128(user_id as u128)
    }

    struct Client {
        user: u32,
        tx: mpsc::Sender<LobbyEvent>,
        rx: mpsc::Receiver<LobbyEvent>,
        /// The last `MODIFIERS` state seen.
        mods: Option<ServerMsg>,
    }

    impl Client {
        fn new(user: u32) -> Self {
            let (tx, rx) = mpsc::channel(LOBBY_CHANNEL_CAPACITY);
            Self { user, tx, rx, mods: None }
        }

        /// Drains events; returns the last lobby state and anything else
        /// (the modifier table and state aside: [`Client::mods`]).
        fn drain(&mut self) -> (Option<LobbyInfo>, Vec<LobbyEvent>) {
            let mut state = None;
            let mut other = Vec::new();
            while let Ok(e) = self.rx.try_recv() {
                match e {
                    LobbyEvent::Msg(ServerMsg::LobbyState { lobby }) => state = Some(lobby),
                    LobbyEvent::Msg(ServerMsg::ModifierDefs { .. } | ServerMsg::MissionInfo { .. }) => {}
                    LobbyEvent::Msg(m @ ServerMsg::Modifiers { .. }) => self.mods = Some(m),
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

    fn invite(dir: &LobbyDirectory, from: &Client, target: u32) -> Result<String, LobbyError> {
        dir.invite(profile(from.user), sid(from.user), target, from.tx.clone())
    }

    fn answer(dir: &LobbyDirectory, to: &Client, from: u32, accept: bool) -> Result<(), LobbyError> {
        dir.answer(profile(to.user), sid(to.user), from, accept, to.tx.clone())
    }

    fn answers(events: &[LobbyEvent]) -> Vec<(u32, bool)> {
        events
            .iter()
            .filter_map(|e| match e {
                LobbyEvent::Msg(ServerMsg::PartyAnswer { entity_id, accepted, .. }) => Some((*entity_id, *accepted)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn invitation_without_a_party_opens_a_private_one_for_both() {
        let dir = lobbies(4);
        let mut a = Client::new(1);
        let mut b = Client::new(2);
        assert_eq!(invite(&dir, &a, 102).unwrap(), "Poring Island Survival");
        assert_eq!(dir.open_lobbies(), 0, "nothing opens before the answer");
        answer(&dir, &b, 101, true).unwrap();
        let (state, events) = a.drain();
        let state = state.unwrap();
        assert!(!state.public);
        assert_eq!(state.members.iter().map(|m| (m.entity_id, m.leader)).collect::<Vec<_>>(), vec![(101, true), (102, false)]);
        assert_eq!(answers(&events), vec![(102, true)]);
        assert_eq!(b.drain().0.unwrap().members.len(), 2);
        assert_eq!(answer(&dir, &b, 101, true).unwrap_err(), LobbyError::InviteExpired, "used up");
    }

    #[tokio::test]
    async fn leader_invites_into_the_lobby_and_decline_tells_the_inviter() {
        let dir = lobbies(4);
        let mut a = Client::new(1);
        let b = Client::new(2);
        let c = Client::new(3);
        let id = a.select(&dir, true).unwrap();
        invite(&dir, &a, 102).unwrap();
        invite(&dir, &a, 103).unwrap();
        answer(&dir, &b, 101, false).unwrap();
        assert_eq!(answers(&a.drain().1), vec![(102, false)]);
        assert_eq!(dir.lobby_of(2), None);
        answer(&dir, &c, 101, true).unwrap();
        assert_eq!(dir.lobby_of(3), Some(id), "joins the leader's lobby");
        assert_eq!(a.drain().0.unwrap().members.len(), 2);
        assert_eq!(answer(&dir, &c, 999, true).unwrap_err(), LobbyError::InviteExpired, "nobody invited from there");
    }

    #[tokio::test]
    async fn invitation_rules() {
        let dir = lobbies(4);
        let a = Client::new(1);
        let b = Client::new(2);
        let c = Client::new(3);
        let d = Client::new(4);
        assert_eq!(invite(&dir, &a, 101).unwrap_err(), LobbyError::NoSuchPlayer, "not oneself");
        a.select(&dir, true).unwrap();
        b.select(&dir, true).unwrap();
        assert_eq!(invite(&dir, &b, 103).unwrap_err(), LobbyError::NotLeader);
        assert_eq!(invite(&dir, &c, 102).unwrap_err(), LobbyError::InParty, "the target is in a party");
        assert_eq!(invite(&dir, &a, 102).unwrap_err(), LobbyError::InParty, "already in yours");
        // A full lobby takes no invitations.
        c.select(&dir, true).unwrap();
        d.select(&dir, true).unwrap();
        assert_eq!(invite(&dir, &a, 105).unwrap_err(), LobbyError::LobbyFull);
    }

    #[tokio::test]
    async fn invitations_lapse_when_the_inviter_moves_on() {
        let dir = lobbies(4);
        let a = Client::new(1);
        let b = Client::new(2);
        let c = Client::new(3);
        // The invitee joined another party meanwhile.
        invite(&dir, &a, 102).unwrap();
        c.select(&dir, false).unwrap();
        let code = dir.state.lock().unwrap().lobbies.values().next().unwrap().code.clone();
        dir.join_code(profile(2), sid(2), &code, b.tx.clone()).unwrap();
        assert_eq!(answer(&dir, &b, 101, true).unwrap_err(), LobbyError::AlreadyInLobby);
        assert!(dir.leave(2, sid(2), "left"));

        // The inviter (no party) joined one since: the invitation is void.
        invite(&dir, &a, 102).unwrap();
        dir.join_code(profile(1), sid(1), &code, a.tx.clone()).unwrap();
        assert_eq!(answer(&dir, &b, 101, true).unwrap_err(), LobbyError::InviteExpired);
        assert!(dir.leave(1, sid(1), "left"));

        // The leader left the party they invited into.
        invite(&dir, &c, 102).unwrap();
        assert!(dir.leave(3, sid(3), "left"));
        assert_eq!(answer(&dir, &b, 103, true).unwrap_err(), LobbyError::InviteExpired);

        // Forgotten (the inviter left town) and timed out.
        invite(&dir, &a, 102).unwrap();
        dir.forget_invites(1);
        assert_eq!(answer(&dir, &b, 101, true).unwrap_err(), LobbyError::InviteExpired);
        invite(&dir, &a, 102).unwrap();
        dir.state.lock().unwrap().invites[0].at -= INVITE_TTL;
        assert_eq!(answer(&dir, &b, 101, false).unwrap_err(), LobbyError::InviteExpired);
        assert_eq!(dir.open_lobbies(), 0);
    }

    #[tokio::test]
    async fn launching_drops_the_party_s_invitations() {
        let dir = lobbies(4);
        let a = Client::new(1);
        let b = Client::new(2);
        a.select(&dir, false).unwrap();
        invite(&dir, &a, 102).unwrap();
        dir.start(1, sid(1)).unwrap();
        assert_eq!(answer(&dir, &b, 101, true).unwrap_err(), LobbyError::InviteExpired, "the inviter is on a mission");
        assert_eq!(dir.lobby_of(2), None);
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

    #[tokio::test]
    async fn leader_sets_modifiers_members_see_them_and_the_launch_carries_them() {
        use crate::world::modifiers::{ModFlags, MAX_TIER};
        let dir = lobbies(4);
        let mut a = Client::new(1);
        let mut b = Client::new(2);
        a.select(&dir, true).unwrap();
        b.select(&dir, true).unwrap();
        dir.set_ready(2, sid(2), true).unwrap();
        let mask = ModFlags::SLOW.0 | ModFlags::DARKNESS.0;
        assert_eq!(dir.set_modifiers(2, sid(2), 3, mask).unwrap_err(), LobbyError::NotLeader);
        assert!(matches!(dir.set_modifiers(1, sid(1), MAX_TIER + 1, 0), Err(LobbyError::BadModifier(_))));
        assert!(matches!(dir.set_modifiers(1, sid(1), 0, ModFlags::VOLATILE.0), Err(LobbyError::BadModifier(_))), "locked");
        assert!(matches!(dir.set_modifiers(1, sid(1), 0, 1 << 31), Err(LobbyError::BadModifier(_))), "unknown bit");
        assert!(matches!(dir.set_modifiers(1, sid(1), 0, ModFlags::ONE_HIT.0), Err(LobbyError::BadModifier(_))), "level 1 leader");
        a.drain();
        b.drain();
        dir.set_modifiers(1, sid(1), 3, mask).unwrap();
        for c in [&mut a, &mut b] {
            let (state, _) = c.drain();
            assert!(!state.unwrap().members[1].ready, "a change unreadies the others");
            assert!(
                matches!(c.mods, Some(ServerMsg::Modifiers { scope: 0, tier: 3, mask: m, risk, .. }) if m == mask && risk == 12 + 6 + 8),
                "{:?}",
                c.mods
            );
        }
        // A late joiner gets the table and the current choice.
        let mut c = Client::new(3);
        c.select(&dir, true).unwrap();
        let mut table = false;
        while let Ok(e) = c.rx.try_recv() {
            table |= matches!(e, LobbyEvent::Msg(ServerMsg::ModifierDefs { .. }));
            if let LobbyEvent::Msg(m @ ServerMsg::Modifiers { .. }) = e {
                c.mods = Some(m);
            }
        }
        assert!(table && matches!(c.mods, Some(ServerMsg::Modifiers { tier: 3, .. })));
        assert!(dir.leave(3, sid(3), "left"));
        dir.set_ready(2, sid(2), true).unwrap();
        let stage = dir.start(1, sid(1)).unwrap();
        assert_eq!(dir.stages().modifiers(stage), Some(Modifiers { tier: 3, flags: ModFlags(mask) }));
    }

    #[tokio::test]
    async fn quick_start_alone_launches_at_once_with_the_modifiers() {
        use crate::world::modifiers::ModFlags;
        let dir = lobbies(4);
        let mut a = Client::new(1);
        let mask = ModFlags::SLOW.0;
        assert!(
            matches!(dir.quick_start(profile(1), sid(1), MISSION, 0, ModFlags::ONE_HIT.0, a.tx.clone()), Err(LobbyError::BadModifier(_))),
            "the curses' levels hold for a party of one too"
        );
        assert!(matches!(dir.quick_start(profile(1), sid(1), "nope", 0, 0, a.tx.clone()), Err(LobbyError::UnknownMission(_))));
        let stage = dir.quick_start(profile(1), sid(1), MISSION, 2, mask, a.tx.clone()).unwrap();
        let (state, events) = a.drain();
        assert!(state.is_none(), "no lobby is ever listed");
        assert!(matches!(&events[..], [LobbyEvent::Launch(t)] if t.stage_id == stage && t.mission == MISSION), "{events:?}");
        assert_eq!(dir.open_lobbies(), 0);
        assert_eq!(dir.lobby_of(1), None);
        assert_eq!(dir.stages().modifiers(stage), Some(Modifiers { tier: 2, flags: ModFlags(mask) }));
    }

    #[tokio::test]
    async fn quick_start_in_a_party_follows_the_lobby_rules() {
        let dir = lobbies(4);
        let mut a = Client::new(1);
        let mut b = Client::new(2);
        a.select(&dir, true).unwrap();
        b.select(&dir, true).unwrap();
        assert_eq!(dir.quick_start(profile(2), sid(2), MISSION, 0, 0, b.tx.clone()).unwrap_err(), LobbyError::NotLeader);
        assert_eq!(
            dir.quick_start(profile(1), sid(1), MISSION, 0, 0, a.tx.clone()).unwrap_err(),
            LobbyError::NotReady("p2".into()),
            "the ready check still holds"
        );
        assert!(matches!(dir.quick_start(profile(1), sid(1), "other", 0, 0, a.tx.clone()), Err(LobbyError::PartyMission(_))));
        dir.set_ready(2, sid(2), true).unwrap();
        // New terms unready the others: the start waits for them again.
        assert_eq!(dir.quick_start(profile(1), sid(1), MISSION, 1, 0, a.tx.clone()).unwrap_err(), LobbyError::NotReady("p2".into()));
        dir.set_ready(2, sid(2), true).unwrap();
        let stage = dir.quick_start(profile(1), sid(1), MISSION, 1, 0, a.tx.clone()).unwrap();
        for c in [&mut a, &mut b] {
            let (_, events) = c.drain();
            assert!(events.iter().any(|e| matches!(e, LobbyEvent::Launch(t) if t.stage_id == stage)), "{events:?}");
        }
        assert_eq!(dir.stages().modifiers(stage).map(|m| m.tier), Some(1));
        assert_eq!(dir.open_lobbies(), 0);
    }

    #[tokio::test]
    async fn a_curse_s_level_holds_the_start_for_everyone() {
        use crate::world::modifiers::ModFlags;
        let dir = lobbies(4);
        let strong = |u: u32| PlayerProfile { level: 30, ..profile(u) };
        let (a, b) = (Client::new(1), Client::new(2));
        dir.select(strong(1), sid(1), MISSION, true, a.tx.clone()).unwrap();
        dir.select(profile(2), sid(2), MISSION, true, b.tx.clone()).unwrap();
        dir.set_modifiers(1, sid(1), 0, ModFlags::ONE_HIT.0).unwrap();
        dir.set_ready(2, sid(2), true).unwrap();
        assert!(matches!(dir.start(1, sid(1)), Err(LobbyError::BadModifier(m)) if m.starts_with("p2")));
        dir.set_modifiers(1, sid(1), 0, 0).unwrap();
        dir.set_ready(2, sid(2), true).unwrap();
        assert!(dir.start(1, sid(1)).is_ok());
    }
}
