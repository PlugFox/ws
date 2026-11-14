//! Hub channels (`Hub-1`, `Hub-2`, …): one actor task per channel plus a
//! directory that assigns players to channels.
//!
//! The directory is the single source of truth for membership and
//! population; it is touched only on join/leave/switch (rare), never per
//! tick, so a plain mutex is enough. Per-tick traffic (inputs, snapshots)
//! goes straight between the socket task and the channel actor over mpsc.

use super::combat::Loadout;
use super::hub::{Arrival, HubEvent, HubInstance, HubSimConfig, HubWorld, Outgoing, PlayerProfile};
use super::protocol::{CastPrefs, ChannelInfo, Look};
use super::trade::{Settlement, TradeError, TradeVault};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Per-player outbound queue depth. Snapshots are dropped when it is full;
/// a full queue on a control message evicts the player (the client would
/// otherwise desync).
pub const OUTBOX_CAPACITY: usize = 128;
const COMMAND_CAPACITY: usize = 1024;

#[derive(Clone, Debug)]
pub struct HubConfig {
    pub sim: HubSimConfig,
    /// Hard player cap per channel.
    pub capacity: usize,
    /// Channels that always exist (ids `1..=base_channels`).
    pub base_channels: u16,
    pub max_channels: u16,
    /// How long an empty overflow channel lingers before it is shut down.
    pub idle_grace: Duration,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            sim: HubSimConfig::default(),
            capacity: 50,
            base_channels: 1,
            max_channels: 8,
            idle_grace: Duration::from_secs(60),
        }
    }
}

#[derive(Debug)]
pub enum HubCommand {
    Join {
        profile: PlayerProfile,
        session_id: Uuid,
        outbox: mpsc::Sender<HubEvent>,
        arrival: Option<Arrival>,
        epoch: u16,
    },
    Leave { user_id: u32, session_id: Uuid },
    /// The socket owning the slot dropped (stages: the player turns
    /// link-dead, `stage.rs`). `events` is that socket's end of the outbox,
    /// dropped here: until this command arrives the outbox still has a
    /// reader, so nothing sent meanwhile is mistaken for an overflow.
    Detach { user_id: u32, session_id: Uuid, events: mpsc::Receiver<HubEvent> },
    /// Where the player stands now (`None`: not here), e.g. to bring them
    /// back to the same spot after a short drop.
    Whereabouts { user_id: u32, reply: tokio::sync::oneshot::Sender<Option<Arrival>> },
    /// `prefs`: the connection's manual aiming and auto-cast slots.
    Input { user_id: u32, seq: u32, dx: f32, dy: f32, aim: [i8; 2], slots: u16, prefs: CastPrefs },
    /// Learned skills or hotbar changed (already validated).
    Loadout { user_id: u32, loadout: Loadout },
    Interact { user_id: u32, npc: String },
    SelectMission { user_id: u32, mission: String, public: bool },
    /// Quick start at a mission giver (`MISSION_START`, in no party).
    QuickStart { user_id: u32, mission: String, tier: u8, mask: u32 },
    /// The client could not apply a monster delta.
    Resync { user_id: u32 },
    /// Restyle at a stylist NPC.
    Restyle { user_id: u32, look: Look },
    /// Forget every skill, at a skill master NPC.
    SkillReset { user_id: u32 },
    Emote { user_id: u32, emote: u8 },
    Pose { user_id: u32, pose: u8 },
    Flag { user_id: u32, flag: u8, x: f32, y: f32 },
    /// A local chat line (cleaned and rate limited by the socket).
    Chat { user_id: u32, text: String },
    /// Buy and sell at a merchant NPC (checked here, applied by the socket).
    Shop { user_id: u32, npc: String, order: crate::world::trade::ShopOrder },
    /// Drink a potion (stages).
    UseItem { user_id: u32, item: String },
    /// Another player's worn gear (the player context menu).
    Inspect { user_id: u32, target: u32 },
    /// Deliver a party invitation the lobby directory accepted.
    PartyInvite { user_id: u32, target: u32, mission: String, ttl_ms: u32 },
    /// The character's belongings changed (equipment): weapon broadcast.
    Bag { user_id: u32, bag: crate::world::combat::Bag },
    /// A player-to-player trade step (`hub::trade`).
    TradeOp { user_id: u32, op: super::trade::TradeOp },
    /// A checked feedback form: the hub wants a Scribe or Kafra in reach
    /// and adds where the player is (`hub::feedback`).
    Feedback { user_id: u32, draft: Box<crate::feedback::Draft> },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum JoinError {
    #[error("channel {0} does not exist")]
    NoSuchChannel(u16),
    #[error("channel {0} is full")]
    ChannelFull(u16),
    #[error("all hub channels are full")]
    HubFull,
    #[error("hub channel is not accepting commands")]
    Unavailable,
}

impl JoinError {
    pub fn code(&self) -> &'static str {
        match self {
            JoinError::NoSuchChannel(_) => "no_such_channel",
            JoinError::ChannelFull(_) => "channel_full",
            JoinError::HubFull => "hub_full",
            JoinError::Unavailable => "hub_unavailable",
        }
    }
}

/// What the socket task keeps after joining.
#[derive(Clone, Debug)]
pub struct HubTicket {
    pub channel: u16,
    pub commands: mpsc::Sender<HubCommand>,
}

struct Channel {
    commands: mpsc::Sender<HubCommand>,
    population: usize,
    empty_since: Option<Instant>,
}

struct Member {
    channel: u16,
    session_id: Uuid,
}

#[derive(Default)]
struct DirState {
    channels: BTreeMap<u16, Channel>,
    members: HashMap<u32, Member>,
}

#[derive(Clone)]
pub struct HubDirectory {
    world: Arc<HubWorld>,
    config: HubConfig,
    state: Arc<Mutex<DirState>>,
    /// Settles player trades (`None`: trades fail with `trade_unavailable`).
    vault: Option<Arc<dyn TradeVault>>,
}

impl HubDirectory {
    /// Creates the directory and spawns the base channels. Must be called
    /// inside a Tokio runtime.
    pub fn new(world: Arc<HubWorld>, config: HubConfig) -> Self {
        Self::with_vault(world, config, None)
    }

    /// As [`HubDirectory::new`], with the vault that settles player trades
    /// (`session::trade::SessionVault`).
    pub fn with_vault(world: Arc<HubWorld>, config: HubConfig, vault: Option<Arc<dyn TradeVault>>) -> Self {
        assert!(config.base_channels >= 1 && config.base_channels <= config.max_channels);
        assert!(config.capacity >= 1);
        let dir = Self { world, config, state: Arc::new(Mutex::new(DirState::default())), vault };
        {
            let mut st = dir.state.lock().unwrap();
            for id in 1..=dir.config.base_channels {
                let ch = dir.spawn_channel(id);
                st.channels.insert(id, ch);
            }
        }
        dir
    }

    pub fn world(&self) -> &Arc<HubWorld> {
        &self.world
    }

    pub fn config(&self) -> &HubConfig {
        &self.config
    }

    fn spawn_channel(&self, id: u16) -> Channel {
        let (tx, rx) = mpsc::channel(COMMAND_CAPACITY);
        let hub = HubInstance::new(id, Arc::clone(&self.world), self.config.sim.clone());
        tokio::spawn(run_instance_with(hub, rx, self.vault.clone()));
        info!("Hub channel Hub-{id} started");
        Channel { commands: tx, population: 0, empty_since: None }
    }

    /// Joins the hub, or moves an existing member to another channel. On error
    /// an existing membership is left untouched.
    pub fn join(
        &self,
        profile: PlayerProfile,
        session_id: Uuid,
        preferred: Option<u16>,
        outbox: mpsc::Sender<HubEvent>,
    ) -> Result<HubTicket, JoinError> {
        self.join_at(profile, session_id, preferred, outbox, None, 0)
    }

    /// [`Self::join`] with an arrival spot and the player's next map epoch
    /// (used when coming back from a stage).
    pub fn join_at(
        &self,
        profile: PlayerProfile,
        session_id: Uuid,
        preferred: Option<u16>,
        outbox: mpsc::Sender<HubEvent>,
        arrival: Option<Arrival>,
        epoch: u16,
    ) -> Result<HubTicket, JoinError> {
        let mut st = self.state.lock().unwrap();
        let user_id = profile.user_id;
        let current = st.members.get(&user_id).map(|m| m.channel);
        // A member re-joining its own channel does not need a free slot.
        let has_room = |id: u16, ch: &Channel| {
            let own = usize::from(current == Some(id));
            ch.population - own < self.config.capacity
        };

        let target = match preferred {
            Some(id) => {
                let ch = st.channels.get(&id).ok_or(JoinError::NoSuchChannel(id))?;
                if !has_room(id, ch) {
                    return Err(JoinError::ChannelFull(id));
                }
                id
            }
            None => match current.filter(|id| has_room(*id, &st.channels[id])) {
                Some(id) => id,
                None => match st.channels.iter().find(|(id, ch)| has_room(**id, ch)) {
                    Some((id, _)) => *id,
                    None => {
                        if st.channels.len() >= self.config.max_channels as usize {
                            return Err(JoinError::HubFull);
                        }
                        let id = (1..=self.config.max_channels)
                            .find(|id| !st.channels.contains_key(id))
                            .expect("fewer channels than max");
                        let ch = self.spawn_channel(id);
                        st.channels.insert(id, ch);
                        id
                    }
                },
            },
        };

        // Rejoining the same channel: leave first (the Leave must not land
        // after the new Join). Switching: join first, so a failed join keeps
        // the old membership.
        if current == Some(target)
            && let Some(old) = st.members.remove(&user_id)
        {
            self.detach(&mut st, user_id, old);
        }
        let ch = st.channels.get_mut(&target).expect("target exists");
        ch.commands
            .try_send(HubCommand::Join { profile, session_id, outbox, arrival, epoch })
            .map_err(|_| JoinError::Unavailable)?;
        ch.population += 1;
        ch.empty_since = None;
        let commands = ch.commands.clone();
        if let Some(old) = st.members.remove(&user_id) {
            self.detach(&mut st, user_id, old);
        }
        st.members.insert(user_id, Member { channel: target, session_id });
        debug!("User {user_id} joined Hub-{target}");
        Ok(HubTicket { channel: target, commands })
    }

    /// Leaves the hub if `session_id` still owns the membership (a displaced
    /// socket's late leave must not remove its successor).
    pub fn leave(&self, user_id: u32, session_id: Uuid) -> bool {
        let mut st = self.state.lock().unwrap();
        match st.members.get(&user_id) {
            Some(m) if m.session_id == session_id => {
                let m = st.members.remove(&user_id).unwrap();
                self.detach(&mut st, user_id, m);
                true
            }
            _ => false,
        }
    }

    fn detach(&self, st: &mut DirState, user_id: u32, member: Member) {
        if let Some(ch) = st.channels.get_mut(&member.channel) {
            let _ = ch
                .commands
                .try_send(HubCommand::Leave { user_id, session_id: member.session_id });
            ch.population = ch.population.saturating_sub(1);
            if ch.population == 0 {
                ch.empty_since = Some(Instant::now());
            }
        }
    }

    /// Shuts down overflow channels that have been empty for `idle_grace`.
    /// Returns the removed channel ids.
    pub fn reap_idle(&self, now: Instant) -> Vec<u16> {
        reap(&self.state, self.config.base_channels, self.config.idle_grace, now)
    }

    pub fn channels(&self) -> Vec<ChannelInfo> {
        let st = self.state.lock().unwrap();
        st.channels
            .iter()
            .map(|(id, ch)| ChannelInfo {
                id: *id,
                name: format!("Hub-{id}"),
                population: ch.population,
                capacity: self.config.capacity,
            })
            .collect()
    }

    pub fn channel_of(&self, user_id: u32) -> Option<u16> {
        self.state.lock().unwrap().members.get(&user_id).map(|m| m.channel)
    }

    /// The channel `user_id` is in and where they stand there, asked of the
    /// channel's actor (`None` when not in the hub or it does not answer).
    pub async fn whereabouts(&self, user_id: u32) -> Option<(u16, Arrival)> {
        let (channel, commands) = {
            let st = self.state.lock().unwrap();
            let channel = st.members.get(&user_id)?.channel;
            (channel, st.channels.get(&channel)?.commands.clone())
        };
        let (reply, answer) = tokio::sync::oneshot::channel();
        commands.try_send(HubCommand::Whereabouts { user_id, reply }).ok()?;
        let spot = tokio::time::timeout(Duration::from_millis(500), answer).await.ok()?.ok()??;
        Some((channel, spot))
    }

    /// Spawns the periodic reaper. The task stops once the directory has been
    /// dropped everywhere else.
    pub fn spawn_reaper(&self, every: Duration) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(&self.state);
        let (base, grace) = (self.config.base_channels, self.config.idle_grace);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(every);
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(state) = weak.upgrade() else { break };
                reap(&state, base, grace, Instant::now());
            }
        })
    }
}

fn reap(state: &Mutex<DirState>, base: u16, grace: Duration, now: Instant) -> Vec<u16> {
    let mut st = state.lock().unwrap();
    let doomed: Vec<u16> = st
        .channels
        .iter()
        .filter(|(id, ch)| {
            **id > base
                && ch.population == 0
                && ch.empty_since.is_some_and(|t| now.saturating_duration_since(t) >= grace)
        })
        .map(|(id, _)| *id)
        .collect();
    for id in &doomed {
        // Dropping the last sender ends the actor loop.
        st.channels.remove(id);
        info!("Hub channel Hub-{id} shut down (idle)");
    }
    doomed
}

/// Told the user id of a link-dead player the instance let go on its own
/// (a stage sends everyone home when the mission ends).
pub(crate) type Released = Box<dyn Fn(u32) + Send>;

/// The actor of a stage: as [`run_instance_with`] without a vault (no
/// player trades on missions); link-dead players' slots are released
/// through `released`.
pub(crate) async fn run_stage(hub: HubInstance, commands: mpsc::Receiver<HubCommand>, released: Released) {
    run_loop(hub, commands, None, Some(released)).await
}

/// Instance actor of a hub channel: applies commands as they arrive and
/// ticks at a fixed rate; ends when every command sender is dropped. With
/// a vault, deals the instance's players confirmed
/// ([`HubInstance::take_deals`]) are settled by it off the actor, and the
/// answers come back as [`HubInstance::trade_settled`].
pub(crate) async fn run_instance_with(hub: HubInstance, commands: mpsc::Receiver<HubCommand>, vault: Option<Arc<dyn TradeVault>>) {
    run_loop(hub, commands, vault, None).await
}

async fn run_loop(
    mut hub: HubInstance,
    mut commands: mpsc::Receiver<HubCommand>,
    vault: Option<Arc<dyn TradeVault>>,
    released: Option<Released>,
) {
    let (settled_tx, mut settled_rx) = mpsc::unbounded_channel::<(u32, Settlement)>();
    let period = Duration::from_secs_f64(1.0 / hub.tick_hz() as f64);
    let mut interval = tokio::time::interval(period);
    // Under load, skip missed ticks instead of bursting to catch up.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut outboxes: HashMap<u32, mpsc::Sender<HubEvent>> = HashMap::new();
    // Link-dead players: no outbox, but what the socket task would apply to
    // the character (loot, zeny, experience, records) waits here for them.
    let mut parked: HashMap<u32, Vec<HubEvent>> = HashMap::new();
    let mut out: Outgoing = Vec::with_capacity(256);

    loop {
        // Unbiased on purpose: a client flooding inputs must not starve the
        // tick for everyone else on the channel.
        tokio::select! {
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    HubCommand::Join { profile, session_id, outbox, arrival, epoch } => {
                        let user_id = profile.user_id;
                        outboxes.insert(user_id, outbox);
                        hub.join(profile, session_id, arrival, epoch, &mut out);
                        // Back from link-dead: what happened meanwhile.
                        out.extend(parked.remove(&user_id).into_iter().flatten().map(|e| (user_id, e)));
                    }
                    HubCommand::Leave { user_id, session_id } => {
                        if hub.leave(user_id, session_id, &mut out) {
                            outboxes.remove(&user_id);
                            parked.remove(&user_id);
                        }
                    }
                    HubCommand::Detach { user_id, session_id, events } => {
                        if let Some(kept) = hub.detach(user_id, session_id, &mut out) {
                            outboxes.remove(&user_id);
                            if kept {
                                parked.insert(user_id, Vec::new());
                            }
                        }
                        drop(events);
                    }
                    HubCommand::Whereabouts { user_id, reply } => {
                        let spot = hub.position_of(user_id).map(|(map, x, y)| Arrival { map: map.into(), x, y });
                        let _ = reply.send(spot);
                    }
                    HubCommand::Input { user_id, seq, dx, dy, aim, slots, prefs } => {
                        if hub.input(user_id, seq, dx, dy) {
                            hub.act(user_id, aim, slots);
                            hub.cast_prefs(user_id, prefs);
                        }
                    }
                    HubCommand::Loadout { user_id, loadout } => hub.set_loadout(user_id, loadout, &mut out),
                    HubCommand::Interact { user_id, npc } => hub.interact(user_id, &npc, &mut out),
                    HubCommand::Feedback { user_id, draft } => hub.feedback(user_id, draft, &mut out),
                    HubCommand::SelectMission { user_id, mission, public } => {
                        hub.select_mission(user_id, &mission, public, &mut out)
                    }
                    HubCommand::QuickStart { user_id, mission, tier, mask } => {
                        hub.quick_start(user_id, &mission, tier, mask, &mut out)
                    }
                    HubCommand::Resync { user_id } => hub.resync(user_id),
                    HubCommand::Restyle { user_id, look } => hub.restyle(user_id, look, &mut out),
                    HubCommand::SkillReset { user_id } => hub.skill_reset(user_id, &mut out),
                    HubCommand::Emote { user_id, emote } => hub.emote(user_id, emote, &mut out),
                    HubCommand::Pose { user_id, pose } => hub.set_pose(user_id, pose, &mut out),
                    HubCommand::Flag { user_id, flag, x, y } => hub.flag(user_id, flag, x, y, &mut out),
                    HubCommand::Chat { user_id, text } => hub.chat(user_id, text, &mut out),
                    HubCommand::Shop { user_id, npc, order } => hub.shop(user_id, &npc, order, &mut out),
                    HubCommand::UseItem { user_id, item } => hub.use_item(user_id, &item, &mut out),
                    HubCommand::Bag { user_id, bag } => hub.set_bag(user_id, bag, &mut out),
                    HubCommand::Inspect { user_id, target } => hub.inspect(user_id, target, &mut out),
                    HubCommand::PartyInvite { user_id, target, mission, ttl_ms } => {
                        hub.party_invite(user_id, target, mission, ttl_ms, &mut out)
                    }
                    HubCommand::TradeOp { user_id, op } => hub.trade_op(user_id, op, &mut out),
                }
            }
            _ = interval.tick() => hub.tick(&mut out),
            Some((id, result)) = settled_rx.recv() => hub.trade_settled(id, result, &mut out),
        }
        for deal in hub.take_deals() {
            match &vault {
                Some(vault) => {
                    let (vault, tx, id) = (Arc::clone(vault), settled_tx.clone(), deal.id);
                    // Off the actor: the vault waits on locks and storage.
                    tokio::spawn(async move {
                        let _ = tx.send((id, vault.settle(deal).await));
                    });
                }
                None => hub.trade_settled(deal.id, Err(TradeError::Unavailable), &mut out),
            }
        }
        dispatch(&mut hub, &mut outboxes, &mut parked, released.as_deref(), &mut out);
    }
    info!("Hub channel {} stopped", hub.name());
}

fn dispatch(
    hub: &mut HubInstance,
    outboxes: &mut HashMap<u32, mpsc::Sender<HubEvent>>,
    parked: &mut HashMap<u32, Vec<HubEvent>>,
    released: Option<&(dyn Fn(u32) + Send)>,
    out: &mut Outgoing,
) {
    // Evictions emit `player_vanished` to others, hence the loop.
    while !out.is_empty() {
        let mut evict = Vec::new();
        for (user_id, event) in out.drain(..) {
            let Some(tx) = outboxes.get(&user_id) else {
                if let Some(held) = parked.get_mut(&user_id) {
                    match event {
                        // Frames are for a client that is not there.
                        HubEvent::Control(_) | HubEvent::Snapshot(_) => {}
                        // Sent home while link-dead: nobody to take them
                        // there; the slot goes.
                        HubEvent::Transfer { .. } => {
                            parked.remove(&user_id);
                            if let Some(released) = released {
                                released(user_id);
                            }
                        }
                        other => held.push(other),
                    }
                }
                continue;
            };
            // A transfer is the player's last event here: the instance has
            // already removed them.
            let last = matches!(event, HubEvent::Transfer { .. });
            match tx.try_send(event) {
                Ok(()) => {}
                // Monster sections are deltas against the previous frame:
                // the next one must be a keyframe.
                Err(mpsc::error::TrySendError::Full(HubEvent::Snapshot(_))) => hub.resync(user_id),
                Err(mpsc::error::TrySendError::Full(_)) => {
                    warn!("Evicting user {user_id} from {}: outbox full", hub.name());
                    evict.push(user_id);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => evict.push(user_id),
            }
            if last {
                outboxes.remove(&user_id);
            }
        }
        for user_id in evict {
            // Dropping the sender tells the socket task it left the hub; the
            // socket task then releases the directory slot.
            if outboxes.remove(&user_id).is_some() {
                hub.evict(user_id, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::protocol::ServerMsg;

    fn world() -> Arc<HubWorld> {
        Arc::new(crate::world::hub::test_support::test_world())
    }

    fn profile(user_id: u32) -> PlayerProfile {
        PlayerProfile { user_id, character_id: 100 + user_id, name: format!("p{user_id}"), archetype: "novice".into(), look: crate::world::protocol::Look::default(), level: 1, exp: 0, weapon: String::new(), loadout: Default::default(), bag: Default::default() }
    }

    fn cfg(capacity: usize) -> HubConfig {
        let sim = HubSimConfig { residents: 0, ..Default::default() };
        HubConfig { sim, capacity, base_channels: 1, max_channels: 3, idle_grace: Duration::from_secs(60) }
    }

    fn outbox() -> (mpsc::Sender<HubEvent>, mpsc::Receiver<HubEvent>) {
        mpsc::channel(OUTBOX_CAPACITY)
    }

    async fn next_control(rx: &mut mpsc::Receiver<HubEvent>) -> ServerMsg {
        loop {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.expect("timed out").expect("closed") {
                HubEvent::Control(m) => return m,
                _ => continue,
            }
        }
    }

    #[tokio::test]
    async fn overflow_spawns_channels_and_respects_caps() {
        let dir = HubDirectory::new(world(), cfg(2));
        let mut keep = Vec::new();
        let mut channels = Vec::new();
        for u in 1..=6 {
            let (tx, rx) = outbox();
            keep.push(rx);
            channels.push(dir.join(profile(u), Uuid::nil(), None, tx).unwrap().channel);
        }
        assert_eq!(channels, vec![1, 1, 2, 2, 3, 3]);
        let (tx, _rx) = outbox();
        assert_eq!(dir.join(profile(7), Uuid::nil(), None, tx).unwrap_err(), JoinError::HubFull);
        let (tx, _rx) = outbox();
        assert_eq!(dir.join(profile(7), Uuid::nil(), Some(2), tx).unwrap_err(), JoinError::ChannelFull(2));
        let (tx, _rx) = outbox();
        assert_eq!(dir.join(profile(7), Uuid::nil(), Some(9), tx).unwrap_err(), JoinError::NoSuchChannel(9));
        let pops: Vec<_> = dir.channels().iter().map(|c| c.population).collect();
        assert_eq!(pops, vec![2, 2, 2]);
    }

    #[tokio::test]
    async fn switch_channel_moves_membership_and_rejoin_same_channel_fits() {
        let dir = HubDirectory::new(world(), HubConfig { base_channels: 2, ..cfg(1) });
        let (tx, mut rx1) = outbox();
        assert_eq!(dir.join(profile(1), Uuid::nil(), None, tx).unwrap().channel, 1);
        assert!(matches!(next_control(&mut rx1).await, ServerMsg::HubJoined { channel: 1, .. }));

        // Re-joining the full channel you are already in is allowed.
        let (tx, _rx) = outbox();
        assert_eq!(dir.join(profile(1), Uuid::nil(), Some(1), tx).unwrap().channel, 1);

        let (tx, mut rx2) = outbox();
        assert_eq!(dir.join(profile(1), Uuid::nil(), Some(2), tx).unwrap().channel, 2);
        assert!(matches!(next_control(&mut rx2).await, ServerMsg::HubJoined { channel: 2, .. }));
        assert_eq!(dir.channel_of(1), Some(2));
        let pops: Vec<_> = dir.channels().iter().map(|c| c.population).collect();
        assert_eq!(pops, vec![0, 1]);
    }

    #[tokio::test]
    async fn stale_session_leave_is_ignored() {
        let dir = HubDirectory::new(world(), cfg(10));
        let (old, new) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let (tx, _a) = outbox();
        dir.join(profile(1), old, None, tx).unwrap();
        let (tx, _b) = outbox();
        dir.join(profile(1), new, None, tx).unwrap();
        assert!(!dir.leave(1, old));
        assert_eq!(dir.channel_of(1), Some(1));
        assert!(dir.leave(1, new));
        assert_eq!(dir.channel_of(1), None);
        assert_eq!(dir.channels()[0].population, 0);
    }

    #[tokio::test]
    async fn idle_overflow_channels_are_reaped_after_grace() {
        let dir = HubDirectory::new(world(), cfg(1));
        let (tx, _a) = outbox();
        dir.join(profile(1), Uuid::nil(), None, tx).unwrap();
        let (tx, _b) = outbox();
        assert_eq!(dir.join(profile(2), Uuid::nil(), None, tx).unwrap().channel, 2);
        dir.leave(2, Uuid::nil());
        dir.leave(1, Uuid::nil());
        let now = Instant::now();
        assert!(dir.reap_idle(now).is_empty(), "within grace");
        assert_eq!(dir.reap_idle(now + Duration::from_secs(61)), vec![2], "base channel 1 survives");
        assert_eq!(dir.channels().len(), 1);
    }

    #[tokio::test]
    async fn actor_ticks_and_sends_snapshots_and_evicts_dead_outbox() {
        let dir = HubDirectory::new(world(), cfg(10));
        let (tx1, mut rx1) = outbox();
        let t1 = dir.join(profile(1), Uuid::nil(), None, tx1).unwrap();
        let (tx2, rx2) = outbox();
        dir.join(profile(2), Uuid::nil(), None, tx2).unwrap();

        t1.commands.try_send(HubCommand::Input { user_id: 1, seq: 1, dx: 1.0, dy: 0.0, aim: [0, 0], slots: 0, prefs: CastPrefs::default() }).unwrap();
        let snap = loop {
            if let HubEvent::Snapshot(b) = tokio::time::timeout(Duration::from_secs(2), rx1.recv()).await.unwrap().unwrap() {
                let s = crate::world::protocol::Snapshot::decode(&b).unwrap();
                if s.ack_seq == 1 {
                    break s;
                }
            }
        };
        assert_eq!(snap.entities.len(), 2);

        // Player 2's socket dies: the actor notices on the next send and
        // player 1 sees it vanish.
        drop(rx2);
        loop {
            if let ServerMsg::PlayerVanished { entity_id } = next_control(&mut rx1).await {
                assert_eq!(entity_id, 102);
                break;
            }
        }
    }
}
