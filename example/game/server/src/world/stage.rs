//! Stage instances (`ActiveStages`): one actor per launched mission, running
//! the mission's field map for its party.
//!
//! Like the hub directory, this is touched only on launch, enter and leave;
//! per-tick traffic goes straight between socket tasks and the stage actor.
//! A stage ends when its last player leaves, or when nobody shows up within
//! the launch grace.

use super::directory::{run_instance, HubCommand};
use super::horde::catalog::MonsterCatalog;
use super::horde::{Horde, HordeConfig};
use super::hub::{HubEvent, HubInstance, HubSimConfig, HubWorld, PlayerProfile};
use super::missions::MissionInfo;
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
    /// Monster simulation of every stage (when a catalog is loaded).
    pub horde: HordeConfig,
}

impl Default for StageConfig {
    fn default() -> Self {
        Self {
            sim: HubSimConfig::default(),
            max_stages: 64,
            launch_grace: Duration::from_secs(30),
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

/// Handle to a launched stage, handed to every party member.
#[derive(Clone, Debug)]
pub struct StageTicket {
    pub stage_id: u32,
    pub mission: &'static str,
    pub commands: mpsc::Sender<HubCommand>,
}

struct Stage {
    commands: mpsc::Sender<HubCommand>,
    mission: &'static str,
    /// user id → connection id of the players inside.
    members: HashMap<u32, Uuid>,
    /// Party members handed a ticket who have not entered (or given up) yet;
    /// the stage stays up for them until the launch grace runs out.
    pending: HashSet<u32>,
    launched_at: Instant,
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
    config: StageConfig,
    state: Arc<Mutex<StageState>>,
}

impl StageDirectory {
    pub fn new(worlds: Vec<HubWorld>, config: StageConfig) -> Self {
        let worlds = worlds
            .into_iter()
            .map(|w| (w.map(w.entry()).id().to_string(), Arc::new(w)))
            .collect();
        Self { worlds: Arc::new(worlds), monsters: None, config, state: Arc::new(Mutex::new(StageState::default())) }
    }

    /// Stages launched from now on get a horde of these monsters.
    pub fn with_monsters(mut self, catalog: MonsterCatalog) -> Self {
        self.monsters = (!catalog.is_empty()).then(|| Arc::new(catalog));
        self
    }

    /// Whether `mission` can be launched at all (its map is loaded).
    pub fn supports(&self, mission: &MissionInfo) -> bool {
        self.worlds.contains_key(mission.map)
    }

    /// Spawns a stage instance for `mission`, expecting the party `members`
    /// (user ids). Must be called inside a Tokio runtime.
    pub fn launch(&self, mission: &'static MissionInfo, members: &[u32]) -> Result<StageTicket, StageError> {
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
        let mut instance = HubInstance::stage(stage_id, mission.id, Arc::clone(world), self.config.sim.clone());
        if let Some(catalog) = &self.monsters {
            let config = HordeConfig { waves: mission.waves.clone(), ..self.config.horde.clone() };
            instance = instance.with_horde(Horde::new(Arc::clone(world), Arc::clone(catalog), config));
        }
        tokio::spawn(run_instance(instance, rx));
        st.stages.insert(
            stage_id,
            Stage {
                commands: tx.clone(),
                mission: mission.id,
                members: HashMap::new(),
                pending: members.iter().copied().collect(),
                launched_at: Instant::now(),
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
        let user_id = profile.user_id;
        stage.commands.try_send(HubCommand::Join { profile, session_id, outbox, arrival: None, epoch }).map_err(
            |e| match e {
                mpsc::error::TrySendError::Full(_) => StageError::Busy(stage_id),
                mpsc::error::TrySendError::Closed(_) => StageError::Gone(stage_id),
            },
        )?;
        stage.pending.remove(&user_id);
        stage.members.insert(user_id, session_id);
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
        if stage.members.get(&user_id) != Some(&session_id) {
            return false;
        }
        stage.members.remove(&user_id);
        if notify {
            let _ = stage.commands.try_send(HubCommand::Leave { user_id, session_id });
        }
        if stage.members.is_empty() && stage.pending.is_empty() {
            // Dropping the last sender ends the actor.
            let stage = st.stages.remove(&stage_id).unwrap();
            info!("Stage-{stage_id} ({}) ended: party left", stage.mission);
        }
        true
    }

    pub fn population(&self, stage_id: u32) -> Option<usize> {
        self.state.lock().unwrap().stages.get(&stage_id).map(|s| s.members.len())
    }

    pub fn active(&self) -> usize {
        self.state.lock().unwrap().stages.len()
    }

    /// After the launch grace, stops waiting for members who never entered;
    /// a stage with nobody inside then ends.
    pub fn reap_abandoned(&self, now: Instant) -> Vec<u32> {
        reap(&self.state, self.config.launch_grace, now)
    }

    /// Spawns the periodic reaper; it stops once the directory is dropped.
    pub fn spawn_reaper(&self, every: Duration) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(&self.state);
        let grace = self.config.launch_grace;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(every);
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(state) = weak.upgrade() else { break };
                reap(&state, grace, Instant::now());
            }
        })
    }
}

fn reap(state: &Mutex<StageState>, grace: Duration, now: Instant) -> Vec<u32> {
    let mut st = state.lock().unwrap();
    for s in st.stages.values_mut() {
        if now.saturating_duration_since(s.launched_at) >= grace {
            s.pending.clear();
        }
    }
    let doomed: Vec<u32> =
        st.stages.iter().filter(|(_, s)| s.members.is_empty() && s.pending.is_empty()).map(|(id, _)| *id).collect();
    for id in &doomed {
        st.stages.remove(id);
        info!("Stage-{id} shut down: nobody entered in time");
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
        MissionInfo { id: "m1", name: "M1", description: "", map: "field", max_players: 4, waves: crate::world::horde::director::WaveRules::NORMAL };
    static MISSING: MissionInfo =
        MissionInfo { id: "m2", name: "M2", description: "", map: "nowhere", max_players: 4, waves: crate::world::horde::director::WaveRules::NORMAL };

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
        PlayerProfile { user_id, character_id: 100 + user_id, name: format!("p{user_id}"), archetype: "novice".into(), look: crate::world::protocol::Look::default(), level: 1, exp: 0, weapon: String::new() }
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
}
