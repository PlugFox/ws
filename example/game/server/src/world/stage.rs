//! Stage instances (`ActiveStages`): one actor per launched mission, running
//! the mission's field map for its party.
//!
//! Like the hub directory, this is touched only on launch, enter and leave;
//! per-tick traffic goes straight between socket tasks and the stage actor.
//! A stage ends when its last player leaves, or when nobody shows up within
//! the launch grace.
//!
//! A member whose socket drops is not gone: they stay *link-dead* for the
//! link grace (`StageConfig::link_grace`). The actor keeps their fighter on
//! the map (still hunted and hit, never moving or attacking), the stage
//! stays up for them, and a new socket of the same character re-attaches
//! to it ([`StageDirectory::rejoin`]). The grace running out removes them
//! as a leave would; a stage whose members are all link-dead ends then.
//! Every membership change is keyed by the socket's connection id, so a
//! stale socket's late cleanup never touches its successor.

use super::directory::{run_stage, HubCommand};
use super::combat::SkillBook;
use super::horde::catalog::MonsterCatalog;
use super::horde::{Horde, HordeConfig};
use super::hub::{HubEvent, HubInstance, HubSimConfig, HubWorld, PlayerProfile};
use super::missions::MissionInfo;
use super::modifiers::Modifiers;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::info;
use uuid::Uuid;

const COMMAND_CAPACITY: usize = 1024;

#[derive(Clone, Debug)]
pub struct StageConfig {
    pub sim: HubSimConfig,
    /// Concurrent stage instances on this server.
    pub max_stages: usize,
    /// A launched stage nobody entered is shut down after this long.
    pub launch_grace: Duration,
    /// How long a member whose socket dropped stays link-dead in the stage
    /// (`STAGE_LINK_GRACE_SECS`); zero removes them at once.
    pub link_grace: Duration,
    /// Monster simulation of every stage (when a catalog is loaded).
    pub horde: HordeConfig,
}

impl Default for StageConfig {
    fn default() -> Self {
        Self {
            sim: HubSimConfig::default(),
            max_stages: 64,
            launch_grace: Duration::from_secs(30),
            link_grace: Duration::from_secs(60),
            horde: HordeConfig::default(),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StageError {
    #[error("mission map {0} is not loaded")]
    MapUnavailable(String),
    #[error("all stage slots are busy")]
    Full,
    #[error("stage {0} is over")]
    Gone(u32),
    #[error("stage {0} is overloaded, try again")]
    Busy(u32),
}

impl StageError {
    pub fn code(&self) -> &'static str {
        match self {
            StageError::MapUnavailable(_) => "mission_unavailable",
            StageError::Full => "stage_full",
            StageError::Gone(_) => "stage_gone",
            StageError::Busy(_) => "stage_busy",
        }
    }
}

/// A running stage as [`StageDirectory::summary`] reports it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct StageSummary {
    pub stage_id: u32,
    pub mission: &'static str,
    pub players: usize,
    pub pending: usize,
    /// Members whose socket dropped (inside `players`).
    pub link_dead: usize,
    pub age_secs: u64,
}

/// A stage the character can return to (`resumable`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resumable {
    pub stage_id: u32,
    pub mission: &'static str,
    /// Link grace left; `None` while a socket still holds the slot (a
    /// reconnect over a half-dead connection).
    pub left: Option<Duration>,
}

/// Handle to a launched stage, handed to every party member.
#[derive(Clone, Debug)]
pub struct StageTicket {
    pub stage_id: u32,
    pub mission: &'static str,
    pub commands: mpsc::Sender<HubCommand>,
}

/// A player inside a stage.
#[derive(Clone, Copy, Debug)]
struct Member {
    /// Connection id of the socket that owns the slot.
    conn: Uuid,
    /// Only this character re-attaches.
    character_id: u32,
    /// When the socket dropped (link-dead), else `None`.
    dropped: Option<Instant>,
}

struct Stage {
    commands: mpsc::Sender<HubCommand>,
    mission: &'static str,
    /// user id → the players inside.
    members: HashMap<u32, Member>,
    /// Party members handed a ticket who have not entered (or given up) yet;
    /// the stage stays up for them until the launch grace runs out.
    pending: HashSet<u32>,
    launched_at: Instant,
    /// The party leader's mission modifiers.
    mods: Modifiers,
}

#[derive(Default)]
struct StageState {
    stages: BTreeMap<u32, Stage>,
    next_id: u32,
}

#[derive(Clone)]
pub struct StageDirectory {
    /// Stage world per map id (built once at boot, shared by instances).
    worlds: Arc<HashMap<String, Arc<HubWorld>>>,
    /// Monster catalog; stages run without monsters when absent.
    monsters: Option<Arc<MonsterCatalog>>,
    /// Each mission's roster catalogue (`missions::roster`), built with
    /// the catalog: what its stages spawn and its brief lists.
    rosters: Arc<HashMap<&'static str, Arc<MonsterCatalog>>>,
    skills: Arc<SkillBook>,
    config: StageConfig,
    state: Arc<Mutex<StageState>>,
}

impl StageDirectory {
    pub fn new(worlds: Vec<HubWorld>, config: StageConfig) -> Self {
        let worlds = worlds
            .into_iter()
            .map(|w| (w.map(w.entry()).id().to_string(), Arc::new(w)))
            .collect();
        Self { worlds: Arc::new(worlds), monsters: None, rosters: Arc::default(), skills: Arc::new(SkillBook::default()), config, state: Arc::new(Mutex::new(StageState::default())) }
    }

    /// Stages launched from now on get a horde of these monsters.
    pub fn with_monsters(mut self, catalog: MonsterCatalog) -> Self {
        self.monsters = (!catalog.is_empty()).then(|| Arc::new(catalog));
        let rosters = self.monsters.as_deref().map_or_else(HashMap::new, |all| {
            super::missions::MISSIONS.iter().map(|m| (m.id, Arc::new(super::missions::roster(m, all)))).collect()
        });
        self.rosters = Arc::new(rosters);
        self
    }

    /// The catalogue stages of `mission` spawn from: its roster
    /// (`missions::roster`); `None` without monsters.
    pub fn roster(&self, mission: &MissionInfo) -> Option<Arc<MonsterCatalog>> {
        let all = self.monsters.as_ref()?;
        Some(match self.rosters.get(mission.id) {
            Some(r) => Arc::clone(r),
            // A mission outside the table (tests): built on the spot.
            None => Arc::new(super::missions::roster(mission, all)),
        })
    }

    /// Whether `mission`'s location is in the baked pack.
    pub fn has_map(&self, mission: &MissionInfo) -> bool {
        self.worlds.contains_key(mission.map)
    }

    /// The mission list (`MISSION_LIST`): every mission of the ladder,
    /// open or coming soon on this server.
    pub fn cards(&self) -> Vec<super::missions::MissionCard> {
        super::missions::MISSIONS
            .iter()
            .enumerate()
            .map(|(i, m)| super::missions::card(i, m, self.roster(m).as_deref(), self.has_map(m)))
            .collect()
    }

    /// The monster catalogue stages spawn from (mission briefs).
    pub fn monsters(&self) -> Option<&MonsterCatalog> {
        self.monsters.as_deref()
    }

    /// Skills stage players can cast.
    pub fn with_skills(mut self, skills: Arc<SkillBook>) -> Self {
        self.skills = skills;
        self
    }

    /// How long a dropped member stays link-dead.
    pub fn link_grace(&self) -> Duration {
        self.config.link_grace
    }

    /// Whether `mission` can be launched at all: its map is loaded and
    /// (when the server has monsters) its roster has some to send.
    pub fn supports(&self, mission: &MissionInfo) -> bool {
        self.has_map(mission) && self.roster(mission).is_none_or(|r| super::missions::has_monsters(&r))
    }

    /// Spawns a stage instance for `mission`, expecting the party `members`
    /// (user ids). Must be called inside a Tokio runtime.
    pub fn launch(&self, mission: &'static MissionInfo, members: &[u32]) -> Result<StageTicket, StageError> {
        self.launch_with(mission, Modifiers::NONE, members)
    }

    /// [`Self::launch`] under mission modifiers (`modifiers.rs`): tougher
    /// and more monsters, a darker field, an earlier fog, the curses.
    pub fn launch_with(&self, mission: &'static MissionInfo, mods: Modifiers, members: &[u32]) -> Result<StageTicket, StageError> {
        let world = self.worlds.get(mission.map).ok_or_else(|| StageError::MapUnavailable(mission.map.into()))?;
        let mut st = self.state.lock().unwrap();
        if st.stages.len() >= self.config.max_stages {
            return Err(StageError::Full);
        }
        st.next_id = st.next_id.wrapping_add(1).max(1);
        while st.stages.contains_key(&st.next_id) {
            st.next_id = st.next_id.wrapping_add(1).max(1);
        }
        let stage_id = st.next_id;
        let (tx, rx) = mpsc::channel(COMMAND_CAPACITY);
        let mut instance = HubInstance::stage(stage_id, mission.id, Arc::clone(world), self.config.sim.clone())
            .with_skills(Arc::clone(&self.skills))
            .with_modifiers(mods);
        if let Some(catalog) = self.roster(mission) {
            // The mission's own light (a dark cave) under the modifiers'.
            let light_radius = self.config.horde.light_radius * mission.env.light_scale;
            let config = HordeConfig { waves: mission.waves.clone(), light_radius, ..self.config.horde.clone() }.with_modifiers(mods);
            instance = instance.with_horde(Horde::new(Arc::clone(world), catalog, config));
        }
        // A link-dead player the actor let go (the mission ended and they
        // were sent home) is no longer waited for.
        let weak = Arc::downgrade(&self.state);
        let released = Box::new(move |user_id| {
            if let Some(state) = weak.upgrade() {
                release(&state, stage_id, user_id);
            }
        });
        tokio::spawn(run_stage(instance, rx, released));
        st.stages.insert(
            stage_id,
            Stage {
                commands: tx.clone(),
                mission: mission.id,
                members: HashMap::new(),
                pending: members.iter().copied().collect(),
                launched_at: Instant::now(),
                mods,
            },
        );
        info!("Stage-{stage_id} launched for {}", mission.id);
        Ok(StageTicket { stage_id, mission: mission.id, commands: tx })
    }

    /// Puts a player into a launched stage.
    pub fn enter(
        &self,
        stage_id: u32,
        profile: PlayerProfile,
        session_id: Uuid,
        outbox: mpsc::Sender<HubEvent>,
        epoch: u16,
    ) -> Result<mpsc::Sender<HubCommand>, StageError> {
        let mut st = self.state.lock().unwrap();
        let stage = st.stages.get_mut(&stage_id).ok_or(StageError::Gone(stage_id))?;
        let (user_id, character_id) = (profile.user_id, profile.character_id);
        stage.commands.try_send(HubCommand::Join { profile, session_id, outbox, arrival: None, epoch }).map_err(
            |e| match e {
                mpsc::error::TrySendError::Full(_) => StageError::Busy(stage_id),
                mpsc::error::TrySendError::Closed(_) => StageError::Gone(stage_id),
            },
        )?;
        stage.pending.remove(&user_id);
        stage.members.insert(user_id, Member { conn: session_id, character_id, dropped: None });
        Ok(stage.commands.clone())
    }

    /// A party member will not enter after all (their ticket could not be
    /// delivered or used). Ends the stage if nobody else is in or coming.
    pub fn abandon(&self, stage_id: u32, user_id: u32) {
        let mut st = self.state.lock().unwrap();
        let Some(stage) = st.stages.get_mut(&stage_id) else { return };
        if stage.pending.remove(&user_id) && stage.members.is_empty() && stage.pending.is_empty() {
            st.stages.remove(&stage_id);
            info!("Stage-{stage_id} ended: nobody entered");
        }
    }

    /// Takes a player out of a stage if `session_id` still owns the slot. With
    /// `notify`, the actor is told to drop the player (not needed when the
    /// player already left through the exit portal). The stage ends with its
    /// last player.
    pub fn leave(&self, stage_id: u32, user_id: u32, session_id: Uuid, notify: bool) -> bool {
        let mut st = self.state.lock().unwrap();
        let Some(stage) = st.stages.get_mut(&stage_id) else { return false };
        if stage.members.get(&user_id).map(|m| m.conn) != Some(session_id) {
            return false;
        }
        stage.members.remove(&user_id);
        if notify {
            let _ = stage.commands.try_send(HubCommand::Leave { user_id, session_id });
        }
        end_if_empty(&mut st, stage_id, "party left");
        true
    }

    /// The socket owning the slot dropped: the player turns link-dead for
    /// the link grace instead of leaving. `events` (the socket's end of the
    /// actor's outbox) goes to the actor with the news, so nothing sent
    /// before it arrives counts as an overflow. Without a grace this is a
    /// leave. Returns whether `session_id` owned the slot.
    pub fn detach(&self, stage_id: u32, user_id: u32, session_id: Uuid, events: mpsc::Receiver<HubEvent>) -> bool {
        if self.config.link_grace.is_zero() {
            return self.leave(stage_id, user_id, session_id, true);
        }
        let mut st = self.state.lock().unwrap();
        let Some(stage) = st.stages.get_mut(&stage_id) else { return false };
        let Some(member) = stage.members.get_mut(&user_id).filter(|m| m.conn == session_id) else { return false };
        if stage.commands.try_send(HubCommand::Detach { user_id, session_id, events }).is_err() {
            // The actor is gone or swamped: no grace to give.
            stage.members.remove(&user_id);
            end_if_empty(&mut st, stage_id, "party left");
            return true;
        }
        member.dropped = Some(Instant::now());
        info!("Stage-{stage_id}: user {user_id} is link-dead");
        true
    }

    /// The stage `character_id` of `user_id` is in, if any: link-dead (with
    /// the grace left) or still held by another socket.
    pub fn resumable(&self, user_id: u32, character_id: u32) -> Option<Resumable> {
        let st = self.state.lock().unwrap();
        let grace = self.config.link_grace;
        st.stages.iter().find_map(|(&stage_id, s)| {
            let m = s.members.get(&user_id).filter(|m| m.character_id == character_id)?;
            Some(Resumable { stage_id, mission: s.mission, left: m.dropped.map(|t| grace.saturating_sub(t.elapsed())) })
        })
    }

    /// Re-attaches a member's new socket (`session_id`) to their fighter:
    /// the actor swaps the outbox and resends the stage, the slot changes
    /// owner (the old socket's cleanup is then a no-op). Only the character
    /// that is in the stage may come back.
    pub fn rejoin(
        &self,
        stage_id: u32,
        profile: PlayerProfile,
        session_id: Uuid,
        outbox: mpsc::Sender<HubEvent>,
        epoch: u16,
    ) -> Result<mpsc::Sender<HubCommand>, StageError> {
        let mut st = self.state.lock().unwrap();
        let stage = st.stages.get_mut(&stage_id).ok_or(StageError::Gone(stage_id))?;
        let user_id = profile.user_id;
        let member = stage
            .members
            .get_mut(&user_id)
            .filter(|m| m.character_id == profile.character_id)
            .ok_or(StageError::Gone(stage_id))?;
        stage.commands.try_send(HubCommand::Join { profile, session_id, outbox, arrival: None, epoch }).map_err(
            |e| match e {
                mpsc::error::TrySendError::Full(_) => StageError::Busy(stage_id),
                mpsc::error::TrySendError::Closed(_) => StageError::Gone(stage_id),
            },
        )?;
        *member = Member { conn: session_id, dropped: None, ..*member };
        info!("Stage-{stage_id}: user {user_id} re-attached");
        Ok(stage.commands.clone())
    }

    /// Gives up the stage `user_id` is in (declined "Return to battle", or
    /// another character entered the world): removed now, as on expiry.
    pub fn forfeit(&self, user_id: u32) -> Option<u32> {
        let mut st = self.state.lock().unwrap();
        let stage_id = st.stages.iter().find(|(_, s)| s.members.contains_key(&user_id)).map(|(id, _)| *id)?;
        let stage = st.stages.get_mut(&stage_id).unwrap();
        let m = stage.members.remove(&user_id).unwrap();
        let _ = stage.commands.try_send(HubCommand::Leave { user_id, session_id: m.conn });
        end_if_empty(&mut st, stage_id, "forfeited");
        Some(stage_id)
    }

    /// Whether `session_id` still owns `user_id`'s slot in the stage.
    pub fn owns(&self, stage_id: u32, user_id: u32, session_id: Uuid) -> bool {
        let st = self.state.lock().unwrap();
        st.stages.get(&stage_id).and_then(|s| s.members.get(&user_id)).is_some_and(|m| m.conn == session_id)
    }

    /// Whether `user_id` is link-dead in the stage.
    pub fn is_link_dead(&self, stage_id: u32, user_id: u32) -> bool {
        let st = self.state.lock().unwrap();
        st.stages.get(&stage_id).and_then(|s| s.members.get(&user_id)).is_some_and(|m| m.dropped.is_some())
    }

    pub fn population(&self, stage_id: u32) -> Option<usize> {
        self.state.lock().unwrap().stages.get(&stage_id).map(|s| s.members.len())
    }

    pub fn active(&self) -> usize {
        self.state.lock().unwrap().stages.len()
    }

    /// The mission modifiers stage `stage_id` runs under.
    pub fn modifiers(&self, stage_id: u32) -> Option<Modifiers> {
        self.state.lock().unwrap().stages.get(&stage_id).map(|s| s.mods)
    }

    /// Every running stage: id, mission, players inside, waiting members,
    /// seconds since launch (status and admin tools).
    pub fn summary(&self) -> Vec<StageSummary> {
        let st = self.state.lock().unwrap();
        st.stages
            .iter()
            .map(|(&id, s)| StageSummary {
                stage_id: id,
                mission: s.mission,
                players: s.members.len(),
                pending: s.pending.len(),
                link_dead: s.members.values().filter(|m| m.dropped.is_some()).count(),
                age_secs: s.launched_at.elapsed().as_secs(),
            })
            .collect()
    }

    /// After the launch grace, stops waiting for members who never entered;
    /// after the link grace, removes link-dead members; a stage with nobody
    /// inside then ends.
    pub fn reap_abandoned(&self, now: Instant) -> Vec<u32> {
        reap(&self.state, self.config.launch_grace, self.config.link_grace, now)
    }

    /// Spawns the periodic reaper; it stops once the directory is dropped.
    pub fn spawn_reaper(&self, every: Duration) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(&self.state);
        let (grace, link) = (self.config.launch_grace, self.config.link_grace);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(every);
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(state) = weak.upgrade() else { break };
                reap(&state, grace, link, Instant::now());
            }
        })
    }
}

/// Ends `stage_id` once nobody is inside or coming. Dropping the last
/// sender ends the actor.
fn end_if_empty(st: &mut StageState, stage_id: u32, why: &str) {
    if st.stages.get(&stage_id).is_some_and(|s| s.members.is_empty() && s.pending.is_empty()) {
        let stage = st.stages.remove(&stage_id).unwrap();
        info!("Stage-{stage_id} ({}) ended: {why}", stage.mission);
    }
}

/// The actor removed link-dead `user_id` on its own (sent home at the end
/// of the mission): their slot goes.
fn release(state: &Mutex<StageState>, stage_id: u32, user_id: u32) {
    let mut st = state.lock().unwrap();
    let Some(stage) = st.stages.get_mut(&stage_id) else { return };
    if stage.members.get(&user_id).is_some_and(|m| m.dropped.is_some()) {
        stage.members.remove(&user_id);
        end_if_empty(&mut st, stage_id, "the mission is over");
    }
}

fn reap(state: &Mutex<StageState>, grace: Duration, link_grace: Duration, now: Instant) -> Vec<u32> {
    let mut st = state.lock().unwrap();
    for (id, s) in st.stages.iter_mut() {
        if now.saturating_duration_since(s.launched_at) >= grace {
            s.pending.clear();
        }
        // The link grace ran out: removed as a leave would.
        let commands = &s.commands;
        s.members.retain(|&user_id, m| {
            let expired = m.dropped.is_some_and(|t| now.saturating_duration_since(t) >= link_grace);
            if expired {
                let _ = commands.try_send(HubCommand::Leave { user_id, session_id: m.conn });
                info!("Stage-{id}: link-dead user {user_id} timed out");
            }
            !expired
        });
    }
    let doomed: Vec<u32> =
        st.stages.iter().filter(|(_, s)| s.members.is_empty() && s.pending.is_empty()).map(|(id, _)| *id).collect();
    for id in &doomed {
        st.stages.remove(id);
        info!("Stage-{id} shut down: nobody left inside");
    }
    doomed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::hub::test_support::map;
    use crate::world::map::{CameraConstraint, LocationKind};
    use crate::world::protocol::ServerMsg;
    use crate::world::OUTBOX_CAPACITY;

    static MISSION: MissionInfo =
        MissionInfo { id: "m1", name: "M1", map: "field", max_players: 4, fog: None, ..MissionInfo::TEMPLATE };
    static MISSING: MissionInfo =
        MissionInfo { id: "m2", name: "M2", map: "nowhere", max_players: 4, fog: None, ..MissionInfo::TEMPLATE };

    fn stages(max: usize) -> StageDirectory {
        let field = map(
            "field",
            LocationKind::Field,
            CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
            &["#####", "#...#", "#...#", "#####"],
            vec![],
        );
        StageDirectory::new(
            vec![HubWorld::new(vec![field], "field").unwrap()],
            StageConfig { max_stages: max, ..Default::default() },
        )
    }

    fn profile(user_id: u32) -> PlayerProfile {
        PlayerProfile { user_id, character_id: 100 + user_id, name: format!("p{user_id}"), archetype: "novice".into(), look: crate::world::protocol::Look::default(), level: 1, exp: 0, weapon: String::new(), loadout: Default::default(), bag: Default::default() }
    }

    #[tokio::test]
    async fn launch_enter_leave_lifecycle() {
        let dir = stages(2);
        assert!(dir.supports(&MISSION));
        assert!(!dir.supports(&MISSING));
        assert_eq!(dir.launch(&MISSING, &[1]).unwrap_err(), StageError::MapUnavailable("nowhere".into()));

        let t = dir.launch(&MISSION, &[1, 2]).unwrap();
        let (tx, mut rx) = mpsc::channel(OUTBOX_CAPACITY);
        dir.enter(t.stage_id, profile(1), Uuid::nil(), tx, 3).unwrap();
        let (tx2, _rx2) = mpsc::channel(OUTBOX_CAPACITY);
        dir.enter(t.stage_id, profile(2), Uuid::nil(), tx2, 0).unwrap();
        match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap() {
            HubEvent::Control(ServerMsg::StageJoined { stage_id, mission, .. }) => {
                assert_eq!((stage_id, mission.as_str()), (t.stage_id, "m1"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(dir.population(t.stage_id), Some(2));
        assert!(!dir.leave(t.stage_id, 1, Uuid::from_u128(9), true), "stale session");
        assert!(dir.leave(t.stage_id, 1, Uuid::nil(), true));
        assert_eq!(dir.active(), 1);
        assert!(dir.leave(t.stage_id, 2, Uuid::nil(), false));
        assert_eq!(dir.active(), 0, "the stage ends with its last player");
        let (tx, _rx) = mpsc::channel(OUTBOX_CAPACITY);
        assert_eq!(dir.enter(t.stage_id, profile(3), Uuid::nil(), tx, 0).unwrap_err(), StageError::Gone(t.stage_id));
    }

    #[tokio::test]
    async fn capacity_and_abandoned_reap() {
        let dir = stages(1);
        let t = dir.launch(&MISSION, &[1]).unwrap();
        assert_eq!(dir.launch(&MISSION, &[2]).unwrap_err(), StageError::Full);
        let now = Instant::now();
        assert!(dir.reap_abandoned(now).is_empty(), "within grace");
        assert_eq!(dir.reap_abandoned(now + Duration::from_secs(31)), vec![t.stage_id]);
        assert!(dir.launch(&MISSION, &[2]).is_ok(), "slot freed");
    }

    #[tokio::test]
    async fn stage_waits_for_pending_members() {
        let dir = stages(4);
        let t = dir.launch(&MISSION, &[1, 2, 3]).unwrap();
        let (tx, _rx) = mpsc::channel(OUTBOX_CAPACITY);
        dir.enter(t.stage_id, profile(1), Uuid::nil(), tx, 0).unwrap();
        // 1 leaves before 2 has entered: the stage must stay up for 2.
        assert!(dir.leave(t.stage_id, 1, Uuid::nil(), true));
        assert_eq!(dir.active(), 1);
        let (tx, _rx2) = mpsc::channel(OUTBOX_CAPACITY);
        dir.enter(t.stage_id, profile(2), Uuid::nil(), tx, 0).unwrap();
        dir.abandon(t.stage_id, 3);
        assert_eq!(dir.active(), 1, "2 is inside");
        assert!(dir.leave(t.stage_id, 2, Uuid::nil(), true));
        assert_eq!(dir.active(), 0);

        // Pending members that never show up expire with the grace.
        let t = dir.launch(&MISSION, &[1, 2]).unwrap();
        let (tx, _rx3) = mpsc::channel(OUTBOX_CAPACITY);
        dir.enter(t.stage_id, profile(1), Uuid::nil(), tx, 0).unwrap();
        assert!(dir.reap_abandoned(Instant::now() + Duration::from_secs(31)).is_empty(), "1 is inside");
        assert!(dir.leave(t.stage_id, 1, Uuid::nil(), true));
        assert_eq!(dir.active(), 0, "2 no longer pending after the grace");
    }

    fn next_joined(rx: &mut mpsc::Receiver<HubEvent>) -> impl std::future::Future<Output = u32> + '_ {
        async move {
            loop {
                match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap() {
                    HubEvent::Control(ServerMsg::StageJoined { stage_id, .. }) => return stage_id,
                    _ => continue,
                }
            }
        }
    }

    #[tokio::test]
    async fn a_dropped_member_is_kept_for_the_link_grace_then_removed() {
        let dir = stages(4);
        let t = dir.launch(&MISSION, &[1]).unwrap();
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let (tx, mut rx) = mpsc::channel(OUTBOX_CAPACITY);
        dir.enter(t.stage_id, profile(1), a, tx, 0).unwrap();
        next_joined(&mut rx).await;

        // The socket drops: link-dead, the stage stays up for them.
        assert!(!dir.detach(t.stage_id, 1, b, mpsc::channel(1).1), "not b's slot");
        assert!(dir.detach(t.stage_id, 1, a, rx));
        assert!(dir.is_link_dead(t.stage_id, 1));
        assert_eq!(dir.active(), 1, "everyone is link-dead: kept for the grace");
        let r = dir.resumable(1, 101).unwrap();
        assert_eq!((r.stage_id, r.mission), (t.stage_id, "m1"));
        assert!(r.left.unwrap() > Duration::from_secs(59));
        assert_eq!(dir.resumable(1, 999), None, "another character");
        assert_eq!(dir.summary()[0].link_dead, 1);

        // Within the grace nothing is reaped; after it the slot and the
        // stage go.
        assert!(dir.reap_abandoned(Instant::now() + Duration::from_secs(30)).is_empty());
        assert_eq!(dir.reap_abandoned(Instant::now() + Duration::from_secs(61)), vec![t.stage_id]);
        assert_eq!(dir.resumable(1, 101), None);
    }

    #[tokio::test]
    async fn a_new_socket_re_attaches_and_the_stale_one_cannot_remove_it() {
        let dir = stages(4);
        let t = dir.launch(&MISSION, &[1, 2]).unwrap();
        let (old, new) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let (tx, mut rx) = mpsc::channel(OUTBOX_CAPACITY);
        dir.enter(t.stage_id, profile(1), old, tx, 0).unwrap();
        let (tx2, _rx2) = mpsc::channel(OUTBOX_CAPACITY);
        dir.enter(t.stage_id, profile(2), Uuid::nil(), tx2, 0).unwrap();
        next_joined(&mut rx).await;

        // Another character cannot take the slot.
        let (tx, _) = mpsc::channel(OUTBOX_CAPACITY);
        let other = PlayerProfile { character_id: 999, ..profile(1) };
        assert_eq!(dir.rejoin(t.stage_id, other, new, tx, 0).unwrap_err(), StageError::Gone(t.stage_id));

        // A reconnect over a half-dead connection: the old socket has not
        // even noticed yet. The new one takes over.
        let (tx, mut rx_new) = mpsc::channel(OUTBOX_CAPACITY);
        dir.rejoin(t.stage_id, profile(1), new, tx, 1).unwrap();
        assert_eq!(next_joined(&mut rx_new).await, t.stage_id);
        assert!(dir.owns(t.stage_id, 1, new) && !dir.owns(t.stage_id, 1, old));
        // The old outbox was dropped by the actor (the old socket closes).
        assert!(tokio::time::timeout(Duration::from_secs(2), async { while rx.recv().await.is_some() {} }).await.is_ok());
        // Its late cleanup is a no-op.
        assert!(!dir.detach(t.stage_id, 1, old, rx));
        assert!(!dir.leave(t.stage_id, 1, old, true));
        assert!(!dir.is_link_dead(t.stage_id, 1));
        assert_eq!(dir.population(t.stage_id), Some(2));

        // Drop and come back within the grace.
        assert!(dir.detach(t.stage_id, 1, new, rx_new));
        let (tx, mut rx3) = mpsc::channel(OUTBOX_CAPACITY);
        let third = Uuid::from_u128(3);
        dir.rejoin(t.stage_id, profile(1), third, tx, 2).unwrap();
        next_joined(&mut rx3).await;
        assert!(!dir.is_link_dead(t.stage_id, 1));
        assert!(dir.reap_abandoned(Instant::now() + Duration::from_secs(120)).is_empty(), "nobody link-dead");
    }

    #[tokio::test]
    async fn forfeit_and_release_free_the_slot() {
        let dir = stages(4);
        let t = dir.launch(&MISSION, &[1, 2]).unwrap();
        let mut keep = Vec::new();
        for u in [1, 2] {
            let (tx, rx) = mpsc::channel(OUTBOX_CAPACITY);
            dir.enter(t.stage_id, profile(u), Uuid::from_u128(u as u128), tx, 0).unwrap();
            keep.push(rx);
        }
        let rx2 = keep.pop().unwrap();
        assert!(dir.detach(t.stage_id, 2, Uuid::from_u128(2), rx2));
        // Declined "Return to battle": gone now, not after the grace.
        assert_eq!(dir.forfeit(2), Some(t.stage_id));
        assert_eq!(dir.population(t.stage_id), Some(1));
        assert_eq!(dir.forfeit(2), None);

        // The actor sent link-dead 1 home (the mission ended): released.
        let rx1 = keep.pop().unwrap();
        assert!(dir.detach(t.stage_id, 1, Uuid::from_u128(1), rx1));
        release(&dir.state, t.stage_id, 1);
        assert_eq!(dir.active(), 0, "the last one was released");
    }

    #[tokio::test]
    async fn without_a_link_grace_a_drop_is_a_leave() {
        let dir = StageDirectory { config: StageConfig { link_grace: Duration::ZERO, ..stages(4).config }, ..stages(4) };
        let t = dir.launch(&MISSION, &[1]).unwrap();
        let (tx, rx) = mpsc::channel(OUTBOX_CAPACITY);
        dir.enter(t.stage_id, profile(1), Uuid::nil(), tx, 0).unwrap();
        assert!(dir.detach(t.stage_id, 1, Uuid::nil(), rx));
        assert_eq!(dir.active(), 0);
    }
}
