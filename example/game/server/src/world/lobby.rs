//! Parties: accounts that play missions together.
//!
//! A party belongs to the *accounts* in it, not to their characters or
//! sockets. It outlives its missions, the way back to town, switching
//! characters and signing out; the database keeps it across restarts
//! (`db::party_repo`, written behind every change).
//!
//! - `mission_select` at a board joins the oldest *public* party of that
//!   mission with a free slot, or opens a new one; `public: false` always
//!   opens a private party. A leader picking a mission switches the
//!   party to it.
//! - Every party has a short join code; a private party is reachable only
//!   by its code.
//! - Up to [`PARTY_MAX`] members (and the mission's own limit). The first
//!   member is the leader. The leader starts at any moment: the members in
//!   town go along; anyone offline or still on a mission stays behind and
//!   catches up after. There is no ready check.
//! - Members are *online* while a socket of theirs is in the world
//!   ([`LobbyDirectory::attach`], on every town join and stage entry,
//!   with the account's current character) and *offline* after
//!   ([`LobbyDirectory::detach`]). The party window shows offline members
//!   with their last character and how long they have been away.
//! - Leadership passes to the next oldest member when the leader leaves,
//!   and to the oldest online member once the leader has been offline for
//!   [`LEADER_GRACE`] (a reload or a character switch keeps it). The
//!   leader may kick anyone, offline members too; a member offline for
//!   [`MEMBER_TTL`] drops out on their own, and an empty party closes.
//!
//! - Party invitations (`invite` / `answer`): the leader, or a player in
//!   no party, invites someone in no party; the invitee accepts or declines
//!   within [`INVITE_TTL`]. Accepting joins the leader's party; an inviter
//!   without one gets a private party of the first playable mission with
//!   both of them in it.
//!
//! - Quick start (`quick_start`, the mission window's Start): a player in
//!   no party launches the mission alone at once, a party of one that is
//!   never listed; a party leader starts the party on that mission (it
//!   becomes the party's), with the window's modifiers; anyone else in a
//!   party cannot.
//!
//! - Mission modifiers (`modifiers.rs`): the leader sets the difficulty
//!   tier and curses; every member sees them (`MODIFIERS` after each
//!   `LOBBY_STATE`) and the launch carries them into the stage. A curse
//!   with a level requirement holds the start until everyone going meets it.
//!
//! Parties are global (players from any hub channel can party up) and change
//! only on player actions (plus a slow reaper for the time rules), so one
//! mutex is enough. Online members are reached through their socket task's
//! lobby channel.

use super::hub::PlayerProfile;
use super::missions::{self, MissionInfo, PARTY_MAX};
use super::modifiers::{self, Modifiers};
use super::protocol::{LobbyInfo, LobbyMemberInfo, ServerMsg};
use super::stage::{StageDirectory, StageTicket};
use crate::db::party_repo::{MemberRow, PartyRepo, PartyRow, StoredCharacter, StoredMods};
use crate::db::{Db, PartyWriter};
use chrono::{DateTime, Utc};
use rand::Rng;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
/// An offline leader keeps the lead this long (a reload, a character
/// switch), then the oldest online member takes over.
pub const LEADER_GRACE: Duration = Duration::from_secs(5 * 60);
/// A member offline this long leaves the party on their own.
pub const MEMBER_TTL: Duration = Duration::from_secs(14 * 24 * 3600);

/// What a lobby sends to a member's socket task.
#[derive(Clone, Debug)]
pub enum LobbyEvent {
    Msg(ServerMsg),
    /// The mission started: move to this stage.
    Launch(StageTicket),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LobbyError {
    #[error("already in a party; leave it first")]
    AlreadyInLobby,
    #[error("not in a party")]
    NotInLobby,
    #[error("no party with that code")]
    NoSuchLobby,
    #[error("the party is full")]
    LobbyFull,
    #[error("only the party leader can do that")]
    NotLeader,
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
    #[error("your party is set for {0}: its leader picks the mission")]
    PartyMission(String),
    #[error("{0} takes at most {1} players")]
    MissionFull(String, u8),
    #[error("you are on a mission: start the next one from town")]
    OnMission,
}

impl LobbyError {
    pub fn code(&self) -> &'static str {
        match self {
            LobbyError::AlreadyInLobby => "already_in_lobby",
            LobbyError::NotInLobby => "not_in_lobby",
            LobbyError::NoSuchLobby => "no_such_lobby",
            LobbyError::LobbyFull => "lobby_full",
            LobbyError::NotLeader => "not_leader",
            LobbyError::NoSuchMember => "no_such_member",
            LobbyError::UnknownMission(_) => "unknown_mission",
            LobbyError::MissionUnavailable(_) => "mission_unavailable",
            LobbyError::StageFull => "stage_full",
            LobbyError::NoSuchPlayer => "no_such_player",
            LobbyError::InParty => "in_party",
            LobbyError::InviteExpired => "invite_expired",
            LobbyError::BadModifier(_) => "bad_modifier",
            LobbyError::PartyMission(_) => "party_mission",
            LobbyError::MissionFull(..) => "mission_full",
            LobbyError::OnMission => "not_in_hub",
        }
    }

    pub fn to_msg(&self) -> ServerMsg {
        ServerMsg::LobbyError { code: self.code().into(), message: self.to_string() }
    }
}

/// An online member's socket.
struct Link {
    session_id: Uuid,
    tx: mpsc::Sender<LobbyEvent>,
}

fn who(profile: &PlayerProfile) -> StoredCharacter {
    StoredCharacter { id: profile.character_id, name: profile.name.clone(), archetype: profile.archetype.clone(), level: profile.level }
}

struct Member {
    user_id: u32,
    /// The account's current character (the last one while offline).
    who: StoredCharacter,
    /// `None`: offline.
    link: Option<Link>,
    /// On a mission (sent off by a start, or back in a stage on a
    /// reconnect); a start leaves them out.
    away: bool,
    /// When they went offline (meaningful while `link` is `None`).
    last_seen: DateTime<Utc>,
}

impl Member {
    fn online(&self) -> bool {
        self.link.is_some()
    }

    fn send(&self, e: LobbyEvent) -> bool {
        // A dead socket is cleaned up by its own task (it calls detach).
        self.link.as_ref().is_some_and(|l| l.tx.try_send(e).is_ok())
    }

    fn offline_for(&self, now: DateTime<Utc>) -> Duration {
        match self.link {
            Some(_) => Duration::ZERO,
            None => (now - self.last_seen).to_std().unwrap_or_default(),
        }
    }
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
    fn info(&self, now: DateTime<Utc>) -> LobbyInfo {
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
                    entity_id: m.who.id,
                    name: m.who.name.clone(),
                    archetype: m.who.archetype.clone(),
                    leader: i == 0,
                    online: m.online(),
                    away: m.away,
                    level: m.who.level,
                    offline_secs: m.offline_for(now).as_secs().min(u32::MAX as u64) as u32,
                })
                .collect(),
        }
    }

    fn broadcast(&self) {
        let msg = ServerMsg::LobbyState { lobby: self.info(Utc::now()) };
        let mods = self.mods.state_msg(modifiers::scope::LOBBY);
        for m in &self.members {
            m.send(LobbyEvent::Msg(msg.clone()));
            m.send(LobbyEvent::Msg(mods.clone()));
        }
    }

    fn capacity(&self) -> usize {
        (self.mission.max_players as usize).min(PARTY_MAX as usize)
    }

    fn has_room(&self) -> bool {
        self.members.len() < self.capacity()
    }

    fn leader_is(&self, user_id: u32) -> bool {
        self.members[0].user_id == user_id
    }

    fn position(&self, user_id: u32) -> Option<usize> {
        self.members.iter().position(|m| m.user_id == user_id)
    }

    /// The stored form: online members have no `last_seen`.
    fn row(&self) -> PartyRow {
        PartyRow {
            id: self.id,
            code: self.code.clone(),
            mission: self.mission.id.into(),
            public: self.public,
            mods: StoredMods { tier: self.mods.tier, mask: self.mods.flags.0 },
            members: self
                .members
                .iter()
                .map(|m| MemberRow { user_id: m.user_id, character: m.who.clone(), last_seen: (!m.online()).then_some(m.last_seen) })
                .collect(),
        }
    }
}

/// Who sent an invitation, as they join the party on an accept.
struct Inviter {
    user_id: u32,
    who: StoredCharacter,
    session_id: Uuid,
    tx: mpsc::Sender<LobbyEvent>,
}

/// A party invitation waiting for the invitee's answer.
struct Invite {
    from: Inviter,
    /// The inviter's party when they invited (`None`: none yet).
    lobby: Option<u32>,
    /// The invitee's entity (character) id.
    to: u32,
    at: Instant,
}

#[derive(Default)]
struct LobbyState {
    lobbies: BTreeMap<u32, Lobby>,
    invites: Vec<Invite>,
    /// user id → party id.
    by_user: HashMap<u32, u32>,
    by_code: HashMap<String, u32>,
    next_id: u32,
    /// Parties changed since the last write (gone ones are deleted).
    dirty: BTreeSet<u32>,
}

impl LobbyState {
    /// The caller's party, if `session_id` is their online socket. The
    /// time rules of that party apply first (an expired leader's lead
    /// passes before the caller acts).
    fn lobby_of(&mut self, user_id: u32, session_id: Uuid) -> Result<&mut Lobby, LobbyError> {
        let id = *self.by_user.get(&user_id).ok_or(LobbyError::NotInLobby)?;
        self.tidy(id, Utc::now());
        let lobby = self.lobbies.get_mut(&id).ok_or(LobbyError::NotInLobby)?;
        match lobby.members.iter().any(|m| m.user_id == user_id && m.link.as_ref().is_some_and(|l| l.session_id == session_id)) {
            true => Ok(lobby),
            false => Err(LobbyError::NotInLobby),
        }
    }

    fn touch(&mut self, id: u32) {
        self.dirty.insert(id);
    }

    fn open(&mut self, mission: &'static MissionInfo, public: bool) -> u32 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        while self.lobbies.contains_key(&self.next_id) {
            self.next_id = self.next_id.wrapping_add(1).max(1);
        }
        let id = self.next_id;
        let code = self.new_code();
        self.by_code.insert(code.clone(), id);
        self.lobbies.insert(id, Lobby { id, code, mission, public, members: Vec::new(), mods: Modifiers::NONE });
        self.touch(id);
        id
    }

    /// Removes a member (the next one leads if they led), closes an empty
    /// party and tells the rest. Returns the removed member.
    fn remove(&mut self, user_id: u32) -> Option<Member> {
        let id = self.by_user.remove(&user_id)?;
        let lobby = self.lobbies.get_mut(&id)?;
        let i = lobby.position(user_id)?;
        let member = lobby.members.remove(i);
        if lobby.members.is_empty() {
            let lobby = self.lobbies.remove(&id).unwrap();
            self.by_code.remove(&lobby.code);
            debug!("Party {id} closed (empty)");
        } else {
            lobby.broadcast();
        }
        self.touch(id);
        Some(member)
    }

    /// The time rules of party `id` at `now`: members offline for
    /// [`MEMBER_TTL`] drop out; a leader offline for [`LEADER_GRACE`]
    /// hands the lead to the oldest online member.
    fn tidy(&mut self, id: u32, now: DateTime<Utc>) {
        let Some(lobby) = self.lobbies.get(&id) else { return };
        let stale: Vec<u32> = lobby.members.iter().filter(|m| m.offline_for(now) >= MEMBER_TTL).map(|m| m.user_id).collect();
        for user_id in stale {
            info!("Party {id}: account {user_id} was away too long and left");
            self.remove(user_id);
        }
        let Some(lobby) = self.lobbies.get_mut(&id) else { return };
        if lobby.members[0].offline_for(now) >= LEADER_GRACE
            && let Some(i) = lobby.members.iter().position(Member::online)
        {
            let lead = lobby.members.remove(i);
            info!("Party {id}: the leader is away; {} leads now", lead.who.name);
            lobby.members.insert(0, lead);
            lobby.broadcast();
            self.touch(id);
        }
    }

    /// Party holding the character `entity_id`, if any.
    fn lobby_with(&self, entity_id: u32) -> Option<u32> {
        self.lobbies.values().find(|l| l.members.iter().any(|m| m.who.id == entity_id)).map(|l| l.id)
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
    /// Where changes are written (none: parties live in memory only).
    store: Option<PartyWriter>,
}

impl LobbyDirectory {
    /// Parties in memory only (tests, tools).
    pub fn new(stages: StageDirectory) -> Self {
        Self { stages, state: Arc::new(Mutex::new(LobbyState::default())), store: None }
    }

    /// Parties kept in the database: loads the stored ones (everyone
    /// offline, as of now) and writes every change from here on. A party of
    /// a mission this server no longer has moves to the first playable one.
    pub async fn persistent(stages: StageDirectory, pool: Db) -> Result<Self, sqlx::Error> {
        let rows = PartyRepo::load(&pool).await?;
        let mut st = LobbyState::default();
        let now = Utc::now();
        let fallback = missions::MISSIONS.iter().find(|m| stages.supports(m)).or(missions::MISSIONS.first());
        for row in rows {
            let mission = missions::find(&row.mission).or(fallback);
            let (Some(mission), false) = (mission, row.members.is_empty()) else {
                PartyRepo::delete(&pool, row.id).await?;
                continue;
            };
            let mods = Modifiers::validate(row.mods.tier, row.mods.mask, u32::MAX).unwrap_or(Modifiers::NONE);
            let members = row
                .members
                .into_iter()
                .map(|m| Member { user_id: m.user_id, who: m.character, link: None, away: false, last_seen: m.last_seen.unwrap_or(now) })
                .collect::<Vec<_>>();
            for m in &members {
                st.by_user.insert(m.user_id, row.id);
            }
            st.by_code.insert(row.code.clone(), row.id);
            st.next_id = st.next_id.max(row.id);
            st.lobbies.insert(row.id, Lobby { id: row.id, code: row.code, mission, public: row.public, members, mods });
            // Boot time becomes the offline mark of who was online.
            st.touch(row.id);
        }
        info!("Loaded {} part{}", st.lobbies.len(), if st.lobbies.len() == 1 { "y" } else { "ies" });
        let dir = Self { stages, state: Arc::new(Mutex::new(st)), store: Some(PartyWriter::spawn(pool)) };
        dir.locked(|_| ());
        Ok(dir)
    }

    pub fn stages(&self) -> &StageDirectory {
        &self.stages
    }

    /// Runs `f` on the state, then queues the parties it changed for the
    /// database.
    fn locked<R>(&self, f: impl FnOnce(&mut LobbyState) -> R) -> R {
        let mut st = self.state.lock().unwrap();
        let r = f(&mut st);
        let dirty = std::mem::take(&mut st.dirty);
        if let Some(store) = &self.store {
            for id in dirty {
                match st.lobbies.get(&id) {
                    Some(l) => store.save(l.row()),
                    None => store.delete(id),
                }
            }
        }
        r
    }

    /// Waits until every change so far is in the database (tests, shutdown).
    pub async fn flush(&self) {
        if let Some(store) = &self.store {
            store.flush().await;
        }
    }

    /// Applies the time rules ([`LEADER_GRACE`], [`MEMBER_TTL`]) to every
    /// party every `every`, so they hold without anyone acting.
    pub fn spawn_reaper(&self, every: Duration) -> tokio::task::JoinHandle<()> {
        let dir = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            loop {
                tick.tick().await;
                dir.reap(Utc::now());
            }
        })
    }

    fn reap(&self, now: DateTime<Utc>) {
        self.locked(|st| {
            let ids: Vec<u32> = st.lobbies.keys().copied().collect();
            for id in ids {
                st.tidy(id, now);
            }
        });
    }

    /// A socket of the account entered the world (a town join, a stage
    /// entry: `in_stage`) with the character in `profile`: if the account
    /// is in a party, the member is online on this socket with this
    /// character (the party sees the change), and a new socket gets the
    /// mission's brief and the modifier table. Returns whether they are in
    /// a party.
    pub fn attach(&self, profile: &PlayerProfile, session_id: Uuid, tx: mpsc::Sender<LobbyEvent>, in_stage: bool) -> bool {
        self.locked(|st| self.relink(st, profile.user_id, who(profile), session_id, tx, Some(in_stage)))
    }

    /// [`Self::attach`] under the lock; `away: None` keeps the member's
    /// mission flag (an action from the town).
    fn relink(
        &self,
        st: &mut LobbyState,
        user_id: u32,
        who: StoredCharacter,
        session_id: Uuid,
        tx: mpsc::Sender<LobbyEvent>,
        away: Option<bool>,
    ) -> bool {
        let Some(&id) = st.by_user.get(&user_id) else { return false };
        let Some(lobby) = st.lobbies.get_mut(&id) else { return false };
        let Some(i) = lobby.position(user_id) else { return false };
        let m = &mut lobby.members[i];
        let fresh = m.link.as_ref().is_none_or(|l| l.session_id != session_id);
        let mut changed = fresh || m.who != who;
        if fresh {
            // The mission's brief and the modifier table for the mission
            // window, as on joining.
            let _ = tx.try_send(LobbyEvent::Msg(modifiers::defs_msg()));
            if let Some(catalog) = self.stages.roster(lobby.mission) {
                let brief = missions::brief(lobby.mission, &lobby.mission.waves, &catalog);
                let _ = tx.try_send(LobbyEvent::Msg(ServerMsg::MissionInfo { brief }));
            }
            m.link = Some(Link { session_id, tx });
        }
        m.who = who;
        if let Some(away) = away {
            changed |= m.away != away;
            m.away = away;
        }
        if changed {
            lobby.broadcast();
            st.touch(id);
        }
        true
    }

    /// The socket `session_id` of the account left the world (closed, or
    /// back to the character slots): the member is offline, still in the
    /// party. A newer socket's membership is left alone.
    pub fn detach(&self, user_id: u32, session_id: Uuid) {
        self.locked(|st| {
            st.invites.retain(|i| i.from.user_id != user_id);
            let Some(&id) = st.by_user.get(&user_id) else { return };
            let Some(lobby) = st.lobbies.get_mut(&id) else { return };
            let Some(m) = lobby.members.iter_mut().find(|m| m.user_id == user_id) else { return };
            if m.link.as_ref().is_none_or(|l| l.session_id != session_id) {
                return;
            }
            m.link = None;
            m.last_seen = Utc::now();
            lobby.broadcast();
            st.touch(id);
        })
    }

    /// Mission board choice: matchmake into a public party or open one. In
    /// a party already, the leader switches it to `mission`.
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
        self.locked(|st| {
            if self.relink(st, profile.user_id, who(&profile), session_id, tx.clone(), None) {
                let lobby = st.lobby_of(profile.user_id, session_id)?;
                let id = lobby.id;
                self.switch_mission(st, id, profile.user_id, mission)?;
                return Ok(id);
            }
            let found = public
                .then(|| st.lobbies.values().find(|l| l.public && l.mission.id == mission.id && l.has_room()).map(|l| l.id))
                .flatten();
            let id = found.unwrap_or_else(|| {
                let id = st.open(mission, public);
                info!("Party {id} opened for {} ({})", mission.id, if public { "public" } else { "private" });
                id
            });
            self.add(st, id, &profile, session_id, tx);
            Ok(id)
        })
    }

    /// The leader of party `id` (`user_id`, checked) makes `mission` the
    /// party's; everyone gets its brief.
    fn switch_mission(&self, st: &mut LobbyState, id: u32, user_id: u32, mission: &'static MissionInfo) -> Result<(), LobbyError> {
        let lobby = st.lobbies.get_mut(&id).ok_or(LobbyError::NotInLobby)?;
        if lobby.mission.id == mission.id {
            return Ok(());
        }
        if !lobby.leader_is(user_id) {
            return Err(LobbyError::PartyMission(lobby.mission.name.into()));
        }
        if !self.stages.supports(mission) {
            return Err(LobbyError::MissionUnavailable(mission.name.into()));
        }
        lobby.mission = mission;
        if let Some(catalog) = self.stages.roster(mission) {
            let brief = ServerMsg::MissionInfo { brief: missions::brief(mission, &mission.waves, &catalog) };
            for m in &lobby.members {
                m.send(LobbyEvent::Msg(brief.clone()));
            }
        }
        lobby.broadcast();
        st.touch(id);
        Ok(())
    }

    /// Joins a party by code (case-insensitive).
    pub fn join_code(
        &self,
        profile: PlayerProfile,
        session_id: Uuid,
        code: &str,
        tx: mpsc::Sender<LobbyEvent>,
    ) -> Result<u32, LobbyError> {
        let code = code.trim().to_ascii_uppercase();
        self.locked(|st| {
            let id = *st.by_code.get(&code).ok_or(LobbyError::NoSuchLobby)?;
            if let Some(&mine) = st.by_user.get(&profile.user_id) {
                // Their own party's code just brings this socket in.
                return match mine == id {
                    true => {
                        self.relink(st, profile.user_id, who(&profile), session_id, tx, None);
                        Ok(id)
                    }
                    false => Err(LobbyError::AlreadyInLobby),
                };
            }
            if !st.lobbies.get(&id).ok_or(LobbyError::NoSuchLobby)?.has_room() {
                return Err(LobbyError::LobbyFull);
            }
            self.add(st, id, &profile, session_id, tx);
            Ok(id)
        })
    }

    fn add(&self, st: &mut LobbyState, id: u32, profile: &PlayerProfile, session_id: Uuid, tx: mpsc::Sender<LobbyEvent>) {
        st.by_user.insert(profile.user_id, id);
        let lobby = st.lobbies.get_mut(&id).expect("party exists");
        lobby.members.push(Member { user_id: profile.user_id, who: who(profile), link: None, away: false, last_seen: Utc::now() });
        self.relink(st, profile.user_id, who(profile), session_id, tx, Some(false));
    }

    /// Leader sets the run's difficulty tier and curses (validated against
    /// the table and the leader's level).
    pub fn set_modifiers(&self, user_id: u32, session_id: Uuid, tier: u8, mask: u32) -> Result<(), LobbyError> {
        self.locked(|st| {
            let lobby = st.lobby_of(user_id, session_id)?;
            if !lobby.leader_is(user_id) {
                return Err(LobbyError::NotLeader);
            }
            let mods = Modifiers::validate(tier, mask, lobby.members[0].who.level).map_err(|e| LobbyError::BadModifier(e.to_string()))?;
            if mods != lobby.mods {
                lobby.mods = mods;
                lobby.broadcast();
                let id = lobby.id;
                st.touch(id);
            }
            Ok(())
        })
    }

    /// Leaves the party if `session_id` is the member's socket.
    pub fn leave(&self, user_id: u32, session_id: Uuid, reason: &str) -> bool {
        self.locked(|st| {
            if st.lobby_of(user_id, session_id).is_err() {
                return false;
            }
            if let Some(m) = st.remove(user_id) {
                m.send(LobbyEvent::Msg(ServerMsg::LobbyLeft { reason: reason.into() }));
            }
            true
        })
    }

    /// Leader removes a member (online or not) by entity (character) id.
    pub fn kick(&self, user_id: u32, session_id: Uuid, entity_id: u32) -> Result<(), LobbyError> {
        self.locked(|st| {
            let lobby = st.lobby_of(user_id, session_id)?;
            if !lobby.leader_is(user_id) {
                return Err(LobbyError::NotLeader);
            }
            let target = lobby
                .members
                .iter()
                .find(|m| m.who.id == entity_id && m.user_id != user_id)
                .map(|m| m.user_id)
                .ok_or(LobbyError::NoSuchMember)?;
            if let Some(m) = st.remove(target) {
                m.send(LobbyEvent::Msg(ServerMsg::LobbyLeft { reason: "kicked".into() }));
            }
            Ok(())
        })
    }

    /// Leader starts the party's mission now: the members in town go.
    pub fn start(&self, user_id: u32, session_id: Uuid) -> Result<u32, LobbyError> {
        self.locked(|st| {
            let lobby = st.lobby_of(user_id, session_id)?;
            if !lobby.leader_is(user_id) {
                return Err(LobbyError::NotLeader);
            }
            let id = lobby.id;
            self.launch(st, id)
        })
    }

    /// Quick start (`MISSION_START`). In no party: checks the mission and
    /// the modifiers against the player's level, then launches a party of
    /// one at once (never listed, so nobody can join it). In a party: the
    /// leader starts it on `mission` (switching the party to it) after
    /// applying `tier` and `mask` like [`Self::set_modifiers`].
    pub fn quick_start(
        &self,
        profile: PlayerProfile,
        session_id: Uuid,
        mission: &str,
        tier: u8,
        mask: u32,
        tx: mpsc::Sender<LobbyEvent>,
    ) -> Result<u32, LobbyError> {
        let user_id = profile.user_id;
        let mission = missions::find(mission).ok_or_else(|| LobbyError::UnknownMission(mission.into()))?;
        self.locked(|st| {
            if self.relink(st, user_id, who(&profile), session_id, tx.clone(), None) {
                let lobby = st.lobby_of(user_id, session_id)?;
                if !lobby.leader_is(user_id) {
                    return Err(LobbyError::NotLeader);
                }
                let id = lobby.id;
                let mods = Modifiers::validate(tier, mask, profile.level).map_err(|e| LobbyError::BadModifier(e.to_string()))?;
                self.switch_mission(st, id, user_id, mission)?;
                let lobby = st.lobbies.get_mut(&id).ok_or(LobbyError::NotInLobby)?;
                if mods != lobby.mods {
                    lobby.mods = mods;
                    lobby.broadcast();
                    st.touch(id);
                }
                return self.launch(st, id);
            }
            if !self.stages.supports(mission) {
                return Err(LobbyError::MissionUnavailable(mission.name.into()));
            }
            let mods = Modifiers::validate(tier, mask, profile.level).map_err(|e| LobbyError::BadModifier(e.to_string()))?;
            let ticket = self.stages.launch_with(mission, mods, &[user_id]).map_err(|e| match e {
                super::stage::StageError::Full => LobbyError::StageFull,
                _ => LobbyError::MissionUnavailable(mission.name.into()),
            })?;
            st.invites.retain(|i| i.from.user_id != user_id);
            if tx.try_send(LobbyEvent::Launch(ticket.clone())).is_err() {
                self.stages.abandon(ticket.stage_id, user_id);
            }
            info!("Quick start: Stage-{} for {} (solo)", ticket.stage_id, mission.id);
            Ok(ticket.stage_id)
        })
    }

    /// Launches party `id` with its members in town (online, not on a
    /// mission), once they all meet the curses' levels; hands them their
    /// tickets. The party stays: they come back to it.
    fn launch(&self, st: &mut LobbyState, id: u32) -> Result<u32, LobbyError> {
        let lobby = st.lobbies.get_mut(&id).ok_or(LobbyError::NotInLobby)?;
        let going: Vec<usize> = (0..lobby.members.len()).filter(|&i| lobby.members[i].online() && !lobby.members[i].away).collect();
        // The leader (the caller) goes too.
        if going.first() != Some(&0) {
            return Err(LobbyError::OnMission);
        }
        for &i in &going {
            let m = &lobby.members[i];
            lobby.mods.check_level(m.who.level).map_err(|e| LobbyError::BadModifier(format!("{}: {e}", m.who.name)))?;
        }
        if going.len() > lobby.mission.max_players as usize {
            return Err(LobbyError::MissionFull(lobby.mission.name.into(), lobby.mission.max_players));
        }
        let party: Vec<u32> = going.iter().map(|&i| lobby.members[i].user_id).collect();
        let ticket = self.stages.launch_with(lobby.mission, lobby.mods, &party).map_err(|e| match e {
            super::stage::StageError::Full => LobbyError::StageFull,
            _ => LobbyError::MissionUnavailable(lobby.mission.name.into()),
        })?;
        for &i in &going {
            let m = &mut lobby.members[i];
            m.away = m.send(LobbyEvent::Launch(ticket.clone()));
            if !m.away {
                // Dead or stalled socket: it will not come; do not hold the
                // stage open for it.
                self.stages.abandon(ticket.stage_id, m.user_id);
            }
        }
        lobby.broadcast();
        st.invites.retain(|i| !party.contains(&i.from.user_id));
        info!("Party {id} launched Stage-{} with {} player(s)", ticket.stage_id, party.len());
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
    /// lead their party (with room) or be in none, and the target must be
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
        let user_id = profile.user_id;
        self.locked(|st| {
            st.prune_invites();
            let (lobby, mission) = if self.relink(st, user_id, who(&profile), session_id, tx.clone(), None) {
                let lobby = st.lobby_of(user_id, session_id)?;
                if !lobby.leader_is(user_id) {
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
            st.invites.retain(|i| !(i.from.user_id == user_id && i.to == target));
            if let Some(oldest) = st.invites.iter().position(|i| i.from.user_id == user_id)
                && st.invites.iter().filter(|i| i.from.user_id == user_id).count() >= INVITES_PER_PLAYER
            {
                st.invites.remove(oldest);
            }
            let from = Inviter { user_id, who: who(&profile), session_id, tx };
            st.invites.push(Invite { from, lobby, to: target, at: Instant::now() });
            Ok(mission.name.to_string())
        })
    }

    /// The invitee answers the invitation of `from` (the inviter's entity
    /// id). Declining tells the inviter; accepting joins their party, or
    /// opens a private one for both when they had none. Fails when the
    /// invitation lapsed or no longer holds (the inviter left, lost the
    /// lead, launched; the party filled up; the invitee is in one now).
    pub fn answer(
        &self,
        profile: PlayerProfile,
        session_id: Uuid,
        from: u32,
        accept: bool,
        tx: mpsc::Sender<LobbyEvent>,
    ) -> Result<(), LobbyError> {
        self.locked(|st| {
            st.prune_invites();
            let i = st
                .invites
                .iter()
                .position(|i| i.from.who.id == from && i.to == profile.character_id)
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
            let inviter = invite.from.user_id;
            // Where the inviter is now must still be where they invited from.
            let now = st.by_user.get(&inviter).copied();
            if now != invite.lobby {
                return Err(LobbyError::InviteExpired);
            }
            if let Some(id) = now {
                let lobby = st.lobbies.get(&id).ok_or(LobbyError::InviteExpired)?;
                let lead = &lobby.members[0];
                if lead.user_id != inviter || lead.link.as_ref().is_none_or(|l| l.session_id != invite.from.session_id) {
                    return Err(LobbyError::InviteExpired);
                }
                if !lobby.has_room() {
                    return Err(LobbyError::LobbyFull);
                }
            }
            if st.by_user.contains_key(&profile.user_id) {
                return Err(LobbyError::AlreadyInLobby);
            }
            let id = match now {
                Some(id) => id,
                None => {
                    let mission = self.default_mission().ok_or(LobbyError::InviteExpired)?;
                    let id = st.open(mission, false);
                    info!("Party {id} opened for {} (invitation)", mission.id);
                    let Inviter { user_id, who, session_id, tx } = invite.from;
                    st.by_user.insert(user_id, id);
                    let lobby = st.lobbies.get_mut(&id).expect("just opened");
                    lobby.members.push(Member { user_id, who: who.clone(), link: None, away: false, last_seen: Utc::now() });
                    self.relink(st, user_id, who, session_id, tx, Some(false));
                    id
                }
            };
            reply(true);
            self.add(st, id, &profile, session_id, tx);
            Ok(())
        })
    }

    /// Drops the invitations `user_id` sent (they left the town or went).
    pub fn forget_invites(&self, user_id: u32) {
        self.state.lock().unwrap().invites.retain(|i| i.from.user_id != user_id);
    }

    pub fn open_lobbies(&self) -> usize {
        self.state.lock().unwrap().lobbies.len()
    }
}

#[cfg(test)]
#[path = "lobby_tests.rs"]
mod tests;
