use super::state::AppState;
use crate::session::{ActiveSession, SessionSignal, StateFlusher};
use crate::session::registry::TownSpot;
use crate::world::stage::Resumable;
use crate::world::combat::items::{ItemError, Slot};
use crate::world::combat::{Bag, Class, Loadout};
use crate::world::protocol::{CastPrefs, ItemDef, UsePrefs};
use crate::world::trade::{ShopOrder, MAX_ORDER_COUNT};
use crate::world::map::LocationKind;
use crate::world::protocol::ClientDecodeError;
use crate::world::chat::{self, ChatLimiter};
use crate::world::{
    Arrival, ClientMsg, HubCommand, HubEvent, HubTicket, JoinError, LobbyError, LobbyEvent, PlayerProfile, ServerMsg,
    LOBBY_CHANNEL_CAPACITY, OUTBOX_CAPACITY,
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::mem::DropGuard;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct WsAuthQuery {
    pub token: Option<String>,
    /// The client's build (`version::client_build`): another build is
    /// told to reload and closed before it touches the session.
    pub build: Option<String>,
}

/// WebSocket upgrade handler authenticated via query param `?token=...` or Authorization header.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsAuthQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Response, (StatusCode, &'static str)> {
    let token = query
        .token
        .or_else(|| {
            headers
                .get(header::AUTHORIZATION)
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "))
                .map(|s| s.to_string())
        })
        .ok_or((StatusCode::UNAUTHORIZED, "Missing authentication token"))?;

    let claims = state
        .jwt_manager
        .verify_access_token(&token)
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid or expired token"))?;

    let session_id = Uuid::parse_str(&claims.sid)
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid session id"))?;

    let user_id = claims.sub;

    let check = crate::version::check_client_build(query.build.as_deref(), crate::version::client_build());
    Ok(ws.on_upgrade(move |socket| async move {
        if check == crate::version::ClientBuild::Current {
            handle_socket(socket, state, user_id, session_id).await;
        } else {
            refuse_build(socket, user_id, query.build.as_deref(), check).await;
        }
    }))
}

/// What a client of another build is told before the close.
pub const UPDATE_NOTICE: &str = "A new version of Mobbed! is out: reload the page (Ctrl+Shift+R) to play.";

/// Closes the socket of a client of another build without touching the
/// session (a stale tab must not displace the player's current one). A
/// client that sent its build gets [`CLOSE_UPDATE_REQUIRED`](crate::version::CLOSE_UPDATE_REQUIRED)
/// with this server's build in the reason; one from before the check
/// (no build) a notification it shows and the 4008 it does not retry.
async fn refuse_build(mut socket: WebSocket, user_id: u32, given: Option<&str>, check: crate::version::ClientBuild) {
    let server = crate::version::client_build();
    info!("user {user_id}: client build {given:?} refused (server {server})");
    let code = match check {
        crate::version::ClientBuild::Missing => {
            let note = ServerMsg::Notification { message: UPDATE_NOTICE.into() };
            let _ = socket.send(Message::Binary(note.encode())).await;
            4008
        }
        _ => crate::version::CLOSE_UPDATE_REQUIRED,
    };
    let frame = axum::extract::ws::CloseFrame { code, reason: format!("update_required:{server}").into() };
    let _ = socket.send(Message::Close(Some(frame))).await;
}

async fn handle_socket(socket: WebSocket, state: AppState, user_id: u32, session_id: Uuid) {
    info!(
        "WebSocket connection established for user {} (session {})",
        user_id, session_id
    );

    let (session, mut signal_rx) = match state.session_registry.get(user_id) {
        Some(existing) if existing.session_id == session_id => {
            let rx = existing.signal_tx.subscribe();
            (existing, rx)
        }
        _ => {
            // Register or replace active session
            let (new_active, rx) = ActiveSession::new(session_id, user_id, None, None);
            if let Some(old) = state.session_registry.register_or_displace(new_active.clone()) {
                let _ = StateFlusher::flush_session(&state.pool, &old).await;
            }
            (new_active, rx)
        }
    };

    let (mut ws_tx, mut ws_rx) = socket.split();
    let (lobby_tx, mut lobby_rx) = mpsc::channel(LOBBY_CHANNEL_CAPACITY);
    let (friend_tx, mut friend_rx) = mpsc::channel(crate::world::friends::FRIEND_CHANNEL_CAPACITY);
    let conn = Conn {
        state: state.clone(),
        session: Arc::clone(&session),
        conn_id: Uuid::new_v4(),
        link: None,
        lobby_tx,
        next_epoch: 0,
        last_channel: None,
        stranded: false,
        items_due: false,
        chat: ChatLimiter::default(),
        friend_tx,
        superseded: false,
        cast_prefs: CastPrefs::default(),
        use_prefs: UsePrefs::default(),
    };
    let mut conn = guarded(conn);

    // Send welcome / connected frame
    let welcome = ServerMsg::Connected { user_id, session_id, server_version: crate::version::version().into() };
    let _ = ws_tx.send(Message::Binary(welcome.encode())).await;

    // Pump: session signals, events from the current instance (hub channel
    // or stage), lobby events and client frames.
    loop {
        let replies: Vec<Out> = tokio::select! {
            // Signal from session registry (displacement, force kick)
            signal_res = signal_rx.recv() => {
                match signal_res {
                    Ok(SessionSignal::Displaced { reason }) => {
                        warn!("User {} session displaced: {}", user_id, reason);
                        let msg = ServerMsg::Displaced { reason: reason.clone() };
                        let _ = ws_tx.send(Message::Binary(msg.encode())).await;
                        // Close frame with custom code 4009 (Session Displaced)
                        let close_frame = axum::extract::ws::CloseFrame {
                            code: 4009,
                            reason: "Session displaced by another connection".into(),
                        };
                        let _ = ws_tx.send(Message::Close(Some(close_frame))).await;
                        break;
                    }
                    Ok(SessionSignal::ForceDisconnect { reason }) => {
                        warn!("User {} force disconnect: {}", user_id, reason);
                        let close_frame = axum::extract::ws::CloseFrame {
                            code: 4008,
                            reason: reason.into(),
                        };
                        let _ = ws_tx.send(Message::Close(Some(close_frame))).await;
                        break;
                    }
                    Ok(SessionSignal::Announce { text }) => {
                        let msg = ServerMsg::Announce { text };
                        if ws_tx.send(Message::Binary(msg.encode())).await.is_err() {
                            break;
                        }
                        Vec::new()
                    }
                    Ok(SessionSignal::Notification { message }) => {
                        let note = ServerMsg::Notification { message };
                        if let Err(e) = ws_tx.send(Message::Binary(note.encode())).await {
                            error!("Error sending notification: {}", e);
                            break;
                        }
                        Vec::new()
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }

            event = recv_link(&mut conn.link) => conn.on_instance_event(event).await,

            Some(event) = lobby_rx.recv() => conn.on_lobby_event(event).await,

            Some(event) = friend_rx.recv() => conn.on_friend_event(event).await,

            // Messages received from client
            msg_res = ws_rx.next() => {
                match msg_res {
                    // The protocol is binary; text frames are not part of it.
                    Some(Ok(Message::Text(_))) => vec![hub_error("bad_message", "text frames are not supported")],
                    Some(Ok(Message::Binary(bin))) => conn.on_frame(&bin).await,
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = ws_tx.send(Message::Pong(payload)).await;
                        Vec::new()
                    }
                    Some(Ok(Message::Pong(_))) => Vec::new(),
                    Some(Ok(Message::Close(_))) => {
                        info!("User {} closed WebSocket connection", user_id);
                        break;
                    }
                    Some(Err(e)) => {
                        warn!("WebSocket error for user {}: {}", user_id, e);
                        break;
                    }
                    None => break,
                }
            }
        };
        let mut failed = false;
        for out in replies {
            let frame = match out {
                Out::Msg(msg) => Message::Binary(msg.encode()),
                Out::Binary(bytes) => Message::Binary(bytes),
            };
            if ws_tx.send(frame).await.is_err() {
                failed = true;
                break;
            }
        }
        if failed {
            break;
        }
        if conn.superseded {
            // A newer socket of this account took the stage over: this one
            // must not reconnect and fight it for the slot.
            let close_frame = axum::extract::ws::CloseFrame { code: 4009, reason: "Resumed on another connection".into() };
            let _ = ws_tx.send(Message::Close(Some(close_frame))).await;
            break;
        }
    }

    DropGuard::dismiss(conn).finish().await;
}

/// `conn` with its cleanup ([`Conn::finish`]) armed: when the socket task
/// unwinds (a handler panicked) or is dropped before it reaches its own
/// awaited cleanup, the guard runs it on a task of its own, so no ghost
/// stays in the hub channel, the party or the friends' lists and dirty
/// character state is still flushed. The normal path dismisses the guard
/// and awaits the same cleanup in place.
fn guarded(conn: Conn) -> DropGuard<Conn, impl FnOnce(Conn)> {
    DropGuard::new(conn, |conn: Conn| {
        warn!("WebSocket task of user {} ended without its cleanup: cleaning up now", conn.user_id());
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(conn.finish());
        }
    })
}

#[path = "ws_friends.rs"]
mod friends;
#[path = "ws_feedback.rs"]
mod feedback;

/// Frame to send to the client.
enum Out {
    Msg(ServerMsg),
    Binary(bytes::Bytes),
}

/// Which instance the socket is in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Place {
    Hub { channel: u16 },
    Stage { stage_id: u32 },
}

/// The socket's membership in a hub channel or stage instance.
struct Link {
    place: Place,
    commands: mpsc::Sender<HubCommand>,
    events: mpsc::Receiver<HubEvent>,
}

async fn recv_link(link: &mut Option<Link>) -> Option<HubEvent> {
    match link {
        Some(link) => link.events.recv().await,
        None => std::future::pending().await,
    }
}

/// Per-socket session state: where the player is and how to reach them.
struct Conn {
    state: AppState,
    session: Arc<ActiveSession>,
    /// Ownership token for hub, lobby and stage memberships. Per socket, not
    /// per login session: a reconnect with the same access token reuses the
    /// session id, and the stale socket's late cleanup must not remove its
    /// successor.
    conn_id: Uuid,
    link: Option<Link>,
    /// Handed to the lobby directory; the receiver lives in the pump.
    lobby_tx: mpsc::Sender<LobbyEvent>,
    /// Map epoch the next instance should start from: epochs keep rising
    /// across instances, so a late snapshot from the previous one is never
    /// mistaken for the current map.
    next_epoch: u16,
    /// Hub channel to return to after a stage.
    last_channel: Option<u16>,
    /// The stage dropped the player and the hub has not taken them yet.
    stranded: bool,
    /// Joined an instance: `item_defs` and `inventory` go after its map.
    items_due: bool,
    /// Local chat rate limit (per socket: survives channel switches).
    chat: ChatLimiter,
    /// Friends' sockets reach this one here (`world::friends`).
    friend_tx: mpsc::Sender<crate::world::friends::FriendEvent>,
    /// A newer socket re-attached to this one's stage slot: close.
    superseded: bool,
    /// Manual aiming and auto-cast slots (`CAST_PREFS`), handed to the
    /// instance with every input.
    cast_prefs: CastPrefs,
    /// Auto-use thresholds (`USE_PREFS`), handed over with every input.
    use_prefs: UsePrefs,
}

/// A remembered town spot is used by a reconnect within this long.
const TOWN_SPOT_TTL: Duration = Duration::from_mins(15);

fn hub_error(code: &str, message: impl Into<String>) -> Out {
    Out::Msg(ServerMsg::HubError { code: code.into(), message: message.into() })
}

fn lobby_error(code: &str, message: impl Into<String>) -> Out {
    Out::Msg(ServerMsg::LobbyError { code: code.into(), message: message.into() })
}

impl Conn {
    /// The socket is gone: go offline in the party (it stays the
    /// account's), leave the instance and flush dirty character state.
    async fn finish(mut self) {
        let user_id = self.user_id();
        info!("Cleaning up WebSocket session for user {}", user_id);
        if let Some(lobbies) = &self.state.lobbies {
            lobbies.detach(user_id, self.conn_id);
        }
        self.drop_link().await;
        self.state.friends.gone(user_id, self.conn_id).await;
        let _ = StateFlusher::flush_session(&self.state.pool, &self.session).await;
    }

    fn user_id(&self) -> u32 {
        self.session.user_id
    }

    fn session_id(&self) -> Uuid {
        self.conn_id
    }

    async fn profile(&self) -> Option<PlayerProfile> {
        let guard = self.session.character.read().await;
        guard.as_ref().map(|ch| PlayerProfile {
            user_id: self.session.user_id,
            character_id: ch.id,
            name: ch.name.clone(),
            archetype: ch.archetype.clone(),
            look: ch.look,
            level: ch.level,
            exp: ch.base_exp,
            weapon: ch.weapon.clone(),
            loadout: ch.loadout.clone(),
            bag: ch.bag.clone(),
            homunculus: ch.homunculus.clone(),
        })
    }

    /// Leaves the current hub channel or stage, if any.
    fn leave_link(&mut self) {
        let Some(link) = self.link.take() else { return };
        match link.place {
            Place::Hub { .. } => {
                if let Some(dir) = &self.state.hub {
                    dir.leave(self.user_id(), self.session_id());
                }
            }
            Place::Stage { stage_id } => {
                if let Some(lobbies) = &self.state.lobbies {
                    lobbies.stages().leave(stage_id, self.user_id(), self.session_id(), true);
                }
            }
        }
    }

    /// The socket is gone: a stage keeps the player link-dead for its link
    /// grace (`world::stage`); in town the spot is remembered, so a
    /// reconnect of this session lands there again.
    async fn drop_link(&mut self) {
        let Some(link) = self.link.take() else { return };
        match link.place {
            Place::Hub { .. } => {
                let Some(dir) = self.state.hub.clone() else { return };
                if let (Some(character_id), Some((channel, arrival))) =
                    (self.character_id().await, dir.whereabouts(self.user_id()).await)
                {
                    *self.session.town_spot.lock().unwrap() = Some(TownSpot { character_id, channel, arrival, at: Instant::now() });
                }
                dir.leave(self.user_id(), self.session_id());
            }
            Place::Stage { stage_id } => {
                if let Some(lobbies) = &self.state.lobbies {
                    lobbies.stages().detach(stage_id, self.user_id(), self.session_id(), link.events);
                }
            }
        }
    }

    async fn character_id(&self) -> Option<u32> {
        self.session.character.read().await.as_ref().map(|c| c.id)
    }

    /// The stage the selected character is link-dead in. A stage another
    /// character of the account left behind is forfeited here: one
    /// character plays at a time.
    async fn resumable(&self) -> Option<Resumable> {
        let stages = self.state.lobbies.as_ref()?.stages();
        let character_id = self.character_id().await?;
        let found = stages.resumable(self.user_id(), character_id);
        if found.is_none() {
            stages.forfeit(self.user_id());
        }
        found
    }

    /// Where a reconnect lands in town: the spot this session's last socket
    /// stood on (taken once), else where a stale socket of it still stands.
    async fn town_spot(&self) -> Option<(u16, Arrival)> {
        let character_id = self.character_id().await?;
        let remembered = self.session.town_spot.lock().unwrap().take();
        if let Some(s) = remembered.filter(|s| s.character_id == character_id && s.at.elapsed() < TOWN_SPOT_TTL) {
            return Some((s.channel, s.arrival));
        }
        self.state.hub.as_ref()?.whereabouts(self.user_id()).await
    }

    /// `hub_join` from outside any instance (sign-in, reconnect): a
    /// character link-dead in a stage is asked first (`resumable`); else
    /// the town, on the remembered spot and channel after a short drop.
    async fn enter_world(&mut self, channel: Option<u16>) -> Vec<Out> {
        if self.link.is_some() || channel.is_some() {
            return self.join_hub(channel, None).await;
        }
        if let Some(r) = self.resumable().await {
            let left = r.left.unwrap_or(self.state.lobbies.as_ref().map_or(Duration::ZERO, |l| l.stages().link_grace()));
            let mission = crate::world::missions::find(r.mission).map_or(r.mission, |m| m.name);
            return vec![Out::Msg(ServerMsg::Resumable {
                stage_id: r.stage_id,
                mission: mission.into(),
                left_ms: left.as_millis().min(u32::MAX as u128) as u32,
            })];
        }
        let spot = self.town_spot().await;
        if let Some((channel, _)) = &spot {
            self.last_channel = Some(*channel);
        }
        self.join_hub(None, spot.map(|(_, a)| a)).await
    }

    /// `stage_resume`: back to the link-dead fighter, or forfeit it (and
    /// to town, at the mission return point: they come from a mission).
    async fn resume_stage(&mut self, accept: bool) -> Vec<Out> {
        if self.link.is_some() {
            return Vec::new();
        }
        let (Some(lobbies), Some(profile)) = (self.state.lobbies.clone(), self.profile().await) else {
            return self.enter_world(None).await;
        };
        let mut out = Vec::new();
        match self.resumable().await {
            Some(r) if accept => {
                let (tx, rx) = mpsc::channel(OUTBOX_CAPACITY);
                match lobbies.stages().rejoin(r.stage_id, profile.clone(), self.session_id(), tx, self.next_epoch) {
                    Ok(commands) => {
                        self.link = Some(Link { place: Place::Stage { stage_id: r.stage_id }, commands, events: rx });
                        self.push_prefs();
                        self.attach_party(&profile, true);
                        return out;
                    }
                    Err(e) => {
                        lobbies.stages().forfeit(self.user_id());
                        out.push(lobby_error(e.code(), "The battle is over."));
                    }
                }
            }
            Some(_) => {
                lobbies.stages().forfeit(self.user_id());
            }
            None if accept => out.push(lobby_error("stage_gone", "The battle is over.")),
            None => {}
        }
        let back = self.mission_return();
        out.extend(self.join_hub(None, back).await);
        out
    }

    /// Joins (or switches) a hub channel. A stage is left only once the hub
    /// took the player, so a failed join leaves them where they were. The
    /// channel they came from is a hint: if it is gone or full, any channel
    /// will do.
    /// Where a player leaving a mission lands in town.
    fn mission_return(&self) -> Option<Arrival> {
        let dir = self.state.hub.as_ref()?;
        let seed = (self.user_id() as u64) << 32 ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64);
        Some(dir.world().mission_return(seed))
    }

    async fn join_hub(&mut self, channel: Option<u16>, arrival: Option<Arrival>) -> Vec<Out> {
        let Some(dir) = self.state.hub.clone() else {
            return self.join_failed(hub_error("hub_unavailable", "hub maps are not loaded"));
        };
        let Some(profile) = self.profile().await else {
            return self.join_failed(hub_error("no_character", "select a character (POST /api/characters/{id}/select) first"));
        };
        let hinted = channel.is_none() && self.last_channel.is_some();
        let (tx, rx) = mpsc::channel(OUTBOX_CAPACITY);
        let mut result =
            dir.join_at(profile.clone(), self.session_id(), channel.or(self.last_channel), tx, arrival.clone(), self.next_epoch);
        let out = if hinted && matches!(result, Err(JoinError::NoSuchChannel(_) | JoinError::ChannelFull(_))) {
            self.last_channel = None;
            let (tx, rx2) = mpsc::channel(OUTBOX_CAPACITY);
            result = dir.join_at(profile.clone(), self.session_id(), None, tx, arrival, self.next_epoch);
            self.adopt_hub(result, rx2)
        } else {
            self.adopt_hub(result, rx)
        };
        // In town with this character: the account's party sees them back.
        if self.in_hub() {
            self.attach_party(&profile, false);
        }
        out
    }

    /// This socket is in the world with `profile`'s character (`in_stage`:
    /// on a mission): online in the account's party, if it has one.
    fn attach_party(&self, profile: &PlayerProfile, in_stage: bool) {
        if let Some(lobbies) = &self.state.lobbies {
            lobbies.attach(profile, self.session_id(), self.lobby_tx.clone(), in_stage);
        }
    }

    fn adopt_hub(&mut self, result: Result<HubTicket, JoinError>, rx: mpsc::Receiver<HubEvent>) -> Vec<Out> {
        match result {
            Ok(ticket) => {
                self.last_channel = Some(ticket.channel);
                self.stranded = false;
                let new = Link { place: Place::Hub { channel: ticket.channel }, commands: ticket.commands, events: rx };
                // A previous hub link was already detached by the directory
                // (channel switch); a stage is left now.
                if let Some(old) = self.link.replace(new)
                    && let Place::Stage { stage_id } = old.place
                    && let Some(lobbies) = &self.state.lobbies
                {
                    lobbies.stages().leave(stage_id, self.user_id(), self.session_id(), true);
                }
                self.push_prefs();
                Vec::new()
            }
            Err(err) => self.join_failed(hub_error(err.code(), err.to_string())),
        }
    }

    /// Hands the new instance this connection's cast and auto-use
    /// preferences (inputs carry them too, but only once the player moves).
    fn push_prefs(&self) {
        if let Some(link) = &self.link {
            let (prefs, uses) = (self.cast_prefs, self.use_prefs);
            let _ = link.commands.try_send(HubCommand::Prefs { user_id: self.user_id(), prefs, uses });
        }
    }

    /// A hub join failed. If the stage already dropped the player, they are
    /// in no instance now: say so explicitly.
    fn join_failed(&mut self, err: Out) -> Vec<Out> {
        if !std::mem::take(&mut self.stranded) {
            return vec![err];
        }
        self.last_channel = None;
        vec![err, Out::Msg(ServerMsg::HubLeft { reason: "hub_unavailable".into() })]
    }

    /// Sends a command to the current instance; replies come back through its
    /// outbox.
    fn forward(&self, cmd: HubCommand) -> Vec<Out> {
        match &self.link {
            Some(link) => match link.commands.try_send(cmd) {
                Ok(()) => Vec::new(),
                Err(_) => vec![hub_error("hub_busy", "hub channel is overloaded, try again")],
            },
            None => vec![hub_error("not_in_hub", "join the hub first")],
        }
    }

    /// Sends a command whose loss does not matter (emotes, poses, flags).
    fn cosmetic(&self, cmd: HubCommand) -> Vec<Out> {
        if let Some(link) = &self.link {
            let _ = link.commands.try_send(cmd);
        }
        Vec::new()
    }

    fn in_hub(&self) -> bool {
        matches!(self.link, Some(Link { place: Place::Hub { .. }, .. }))
    }

    async fn on_instance_event(&mut self, event: Option<HubEvent>) -> Vec<Out> {
        match event {
            Some(HubEvent::Snapshot(frame)) => vec![Out::Binary(frame)],
            // The result screen (parked for a link-dead player until they
            // re-attach).
            Some(HubEvent::Results(frames)) => {
                let [stats, rewards] = *frames;
                vec![Out::Msg(stats), Out::Msg(rewards)]
            }
            Some(HubEvent::Control(msg)) => {
                if let ServerMsg::MapEnter { map, map_epoch, .. } = &msg {
                    self.next_epoch = map_epoch.wrapping_add(1);
                    // Stages are not a place to come back to.
                    if map.kind != LocationKind::Field {
                        remember_location(&self.session, &map.id).await;
                    }
                    self.present(map).await;
                }
                // Every instance join, after its first map: the catalogue
                // and the belongings.
                if matches!(msg, ServerMsg::HubJoined { .. } | ServerMsg::StageJoined { .. }) {
                    self.items_due = true;
                }
                if matches!(msg, ServerMsg::MapEnter { .. }) && std::mem::take(&mut self.items_due) {
                    let mut out = vec![Out::Msg(msg), Out::Msg(ServerMsg::ItemDefs { items: ItemDef::catalogue() })];
                    out.extend(self.inventory().await);
                    return out;
                }
                // A mission giver: what each mission spawns and drops, from
                // the catalogue the stages spawn from (the mission window).
                // The mission list first (cards: open or coming soon), then
                // each one's brief from its roster catalogue.
                if let ServerMsg::NpcDialog { missions, .. } = &msg
                    && !missions.is_empty()
                    && let Some(stages) = self.state.lobbies.as_ref().map(|l| l.stages())
                {
                    let list = Out::Msg(ServerMsg::MissionList { missions: stages.cards() });
                    let briefs: Vec<Out> = missions
                        .iter()
                        .filter_map(|m| stages.roster(m).map(|c| Out::Msg(ServerMsg::MissionInfo { brief: crate::world::missions::brief(m, &m.waves, &c) })))
                        .collect();
                    // The mission modifier table (the window's Modifiers section).
                    let mods = Out::Msg(crate::world::modifiers::defs_msg());
                    return [Out::Msg(msg.clone()), list].into_iter().chain(briefs).chain([mods]).collect();
                }
                vec![Out::Msg(msg)]
            }
            Some(HubEvent::Shop(order, haggle)) => {
                // All or nothing (`ShopOrder::apply`), at the prices the
                // hub read off the loadout; the receipt follows the new
                // inventory.
                let mut receipt = None;
                let mut out = self.change_bag(|_, bag| order.apply_at(bag, haggle).map(|r| receipt = Some(r))).await;
                if let Some(r) = receipt {
                    out.push(Out::Msg(ServerMsg::ShopDone { spent: r.spent, earned: r.earned }));
                }
                // The hub checks player trades against its copy of the bag.
                out.extend(self.share_bag().await);
                out
            }
            Some(HubEvent::Refine { item, slot, roll, slip }) => {
                let mut done = None;
                let mut out = self.change_bag(|_, bag| bag.refine(&item, slot, roll, slip).map(|r| done = Some(r))).await;
                if let Some(r) = done {
                    out.push(Out::Msg(ServerMsg::Refined { item: r.key, ok: r.ok, refine: r.refine, slip: r.slip }));
                }
                out.extend(self.share_bag().await);
                out
            }
            Some(HubEvent::Craft { recipe, count }) => {
                let Some(r) = crate::world::combat::quests::find(recipe) else { return Vec::new() };
                let mut done = false;
                let mut out = self.change_bag(|_, bag| crate::world::combat::quests::craft_n(bag, r, count).map(|()| done = true)).await;
                if done {
                    out.push(Out::Msg(ServerMsg::Crafted { recipe: r.id.into(), item: r.item.into() }));
                }
                out.extend(self.share_bag().await);
                out
            }
            // A player trade went through (already stored by the vault).
            Some(HubEvent::BagChanged) => self.inventory().await.into_iter().collect(),
            Some(HubEvent::ItemUsed { item }) => self.change_bag(|_, bag| bag.take(&item, 1)).await,
            Some(HubEvent::Zeny { amount }) => {
                self.change_bag(|_, bag| {
                    bag.earn(amount);
                    Ok(())
                })
                .await
            }
            // Mammonite: the hub's copy of the bag paid it already.
            Some(HubEvent::ZenySpent { amount }) => {
                self.change_bag(|_, bag| {
                    bag.zeny = bag.zeny.saturating_sub(amount);
                    Ok(())
                })
                .await
            }
            Some(HubEvent::Loot { items }) => {
                // Whatever fits goes in; a full bag says so once.
                let mut full = false;
                let mut out = self
                    .change_bag(|_, bag| {
                        for &(id, n) in &items {
                            full |= bag.add(id, n).is_err();
                        }
                        Ok(())
                    })
                    .await;
                if full {
                    out.push(hub_error(ItemError::BagFull.code(), "Your bag is full: some loot was left behind."));
                }
                out
            }
            Some(HubEvent::DevGive { key }) => self.change_bag(|_, bag| bag.add(&key, 1)).await,
            Some(HubEvent::LookChanged(look)) => {
                if let Some(ch) = self.session.character.write().await.as_mut() {
                    ch.set_look(look);
                }
                Vec::new()
            }
            Some(HubEvent::SkillResetApproved) => self.change_loadout(SkillChange::Reset).await,
            Some(HubEvent::FeedbackPlaced { draft, place }) => self.store_feedback(*draft, place).await,
            Some(HubEvent::Record(delta)) => {
                if let Some(ch) = self.session.character.write().await.as_mut() {
                    ch.add_record(&delta);
                }
                Vec::new()
            }
            Some(HubEvent::Homunculus(save)) => {
                if let Some(ch) = self.session.character.write().await.as_mut() {
                    ch.set_homunculus(save);
                }
                Vec::new()
            }
            Some(HubEvent::Progress { level, exp }) => {
                if let Some(ch) = self.session.character.write().await.as_mut() {
                    ch.set_progress(level, exp);
                }
                Vec::new()
            }
            Some(HubEvent::MissionChosen { mission, public }) => {
                let (Some(lobbies), Some(profile)) = (self.state.lobbies.clone(), self.profile().await) else {
                    return vec![lobby_error("mission_unavailable", "missions are not available on this server")];
                };
                match lobbies.select(profile, self.session_id(), &mission, public, self.lobby_tx.clone()) {
                    Ok(_) => Vec::new(),
                    Err(e) => vec![Out::Msg(e.to_msg())],
                }
            }
            Some(HubEvent::QuickStart { mission, tier, mask }) => {
                let (Some(lobbies), Some(profile)) = (self.state.lobbies.clone(), self.profile().await) else {
                    return vec![lobby_error("mission_unavailable", "missions are not available on this server")];
                };
                match lobbies.quick_start(profile, self.session_id(), &mission, tier, mask, self.lobby_tx.clone()) {
                    Ok(_) => Vec::new(),
                    Err(e) => vec![Out::Msg(e.to_msg())],
                }
            }
            Some(HubEvent::Transfer { target_map, arrival: (x, y) }) => {
                // Left the stage through its exit; the actor already dropped us.
                if let Some(Link { place: Place::Stage { stage_id }, .. }) = self.link.take()
                    && let Some(lobbies) = &self.state.lobbies
                {
                    lobbies.stages().leave(stage_id, self.user_id(), self.session_id(), false);
                }
                self.stranded = true;
                // Back from a mission: the town's return point (any other
                // target keeps the portal's arrival; none, from a stage
                // without a walk-out portal, is the return point too).
                let back = self.mission_return().filter(|r| r.map == target_map || target_map.is_empty());
                let portal = (!target_map.is_empty()).then_some(Arrival { map: target_map, x, y });
                self.join_hub(None, back.or(portal)).await
            }
            None => {
                // A newer socket took our stage slot over (it re-attached):
                // nothing to clean up, this socket is done.
                if let Some(Link { place: Place::Stage { stage_id }, .. }) = &self.link
                    && let Some(lobbies) = &self.state.lobbies
                    && !lobbies.stages().owns(*stage_id, self.user_id(), self.session_id())
                {
                    self.link = None;
                    self.superseded = true;
                    return Vec::new();
                }
                // The instance dropped us (outbox overflow or shutdown).
                let place = self.link.as_ref().map(|l| l.place);
                self.leave_link();
                match place {
                    Some(Place::Stage { .. }) => {
                        self.stranded = true;
                        let back = self.mission_return();
                        self.join_hub(None, back).await
                    }
                    _ => vec![Out::Msg(ServerMsg::HubLeft { reason: "evicted".into() })],
                }
            }
        }
    }

    async fn on_lobby_event(&mut self, event: LobbyEvent) -> Vec<Out> {
        match event {
            LobbyEvent::Msg(msg) => vec![Out::Msg(msg)],
            LobbyEvent::Launch(ticket) => {
                let Some(lobbies) = self.state.lobbies.clone() else { return Vec::new() };
                // The party stays the account's: no `lobby_left` here.
                let mut out = Vec::new();
                if matches!(self.link, Some(Link { place: Place::Stage { .. }, .. })) {
                    // Already on a mission (a start raced our stage entry).
                    lobbies.stages().abandon(ticket.stage_id, self.user_id());
                    return out;
                }
                let Some(profile) = self.profile().await else {
                    lobbies.stages().abandon(ticket.stage_id, self.user_id());
                    out.push(hub_error("no_character", "no character selected"));
                    return out;
                };
                let (tx, rx) = mpsc::channel(OUTBOX_CAPACITY);
                match lobbies.stages().enter(ticket.stage_id, profile, self.session_id(), tx, self.next_epoch) {
                    Ok(commands) => {
                        self.leave_link();
                        self.link = Some(Link { place: Place::Stage { stage_id: ticket.stage_id }, commands, events: rx });
                        self.push_prefs();
                    }
                    Err(e) => {
                        lobbies.stages().abandon(ticket.stage_id, self.user_id());
                        out.push(lobby_error(e.code(), e.to_string()));
                    }
                }
                out
            }
        }
    }

    /// Handles one client frame; returns the direct replies.
    async fn on_frame(&mut self, frame: &[u8]) -> Vec<Out> {
        let msg = match ClientMsg::decode(frame) {
            Ok(msg) => msg,
            // Unknown kinds are ignored so older servers tolerate newer clients.
            Err(ClientDecodeError::Unknown) => return Vec::new(),
            Err(ClientDecodeError::Malformed) => {
                debug!("Malformed frame from user {}: {:02x?}", self.user_id(), &frame[..frame.len().min(32)]);
                return vec![hub_error("bad_message", "malformed frame")];
            }
        };
        let (user_id, session_id) = (self.user_id(), self.session_id());
        let lobby_result = |r: Result<(), LobbyError>| match r {
            Ok(()) => Vec::new(),
            Err(e) => vec![Out::Msg(e.to_msg())],
        };

        match msg {
            ClientMsg::Ping => vec![Out::Msg(ServerMsg::Pong { ts: chrono::Utc::now().timestamp_millis() })],
            ClientMsg::Input { seq, dx, dy, aim, slots } => {
                if let Some(link) = &self.link {
                    // Inputs are latest-wins; dropping one under backpressure is fine.
                    let (prefs, uses) = (self.cast_prefs, self.use_prefs);
                    let _ = link.commands.try_send(HubCommand::Input { user_id, seq, dx, dy, aim, slots, prefs, uses });
                }
                Vec::new()
            }
            ClientMsg::CastPrefs(prefs) => {
                self.cast_prefs = prefs;
                self.push_prefs();
                Vec::new()
            }
            ClientMsg::UsePrefs(prefs) => {
                self.use_prefs = prefs;
                self.push_prefs();
                Vec::new()
            }
            ClientMsg::SkillLearn { skill } => self.change_loadout(SkillChange::Learn(skill)).await,
            ClientMsg::SkillSlot { slot, skill } => self.change_loadout(SkillChange::Slot(slot, skill)).await,
            // Riding is part of the loadout: the hub re-equips the fighter
            // and shows the map its new appearance (`player_appearance`).
            ClientMsg::Mount { on } => self.change_loadout(SkillChange::Mount(on)).await,
            ClientMsg::TownSkill { skill } => self.town_skill(&skill).await,
            ClientMsg::SkillReset => {
                if matches!(self.link, Some(Link { place: Place::Stage { .. }, .. })) {
                    return vec![hub_error("not_in_hub", "skills are reset in town")];
                }
                // The hub checks that a skill master is near, then hands
                // it back as `SkillResetApproved`.
                self.forward(HubCommand::SkillReset { user_id })
            }
            ClientMsg::Resync => {
                if let Some(link) = &self.link {
                    // Must not be lost: without it the monsters stay frozen.
                    let _ = link.commands.send(HubCommand::Resync { user_id }).await;
                }
                Vec::new()
            }
            // Cosmetic and rate limited by the instance: dropped silently
            // outside an instance or under backpressure.
            ClientMsg::Emote { emote } => self.cosmetic(HubCommand::Emote { user_id, emote }),
            ClientMsg::Pose { pose } => self.cosmetic(HubCommand::Pose { user_id, pose }),
            // The arrow nocked (stages); the hub checks Arrow Change.
            ClientMsg::Quiver { element } => self.cosmetic(HubCommand::Quiver { user_id, element }),
            ClientMsg::CompanionCmd(cmd) => self.cosmetic(HubCommand::Companion { user_id, cmd }),
            ClientMsg::Flag { flag, x, y } => self.cosmetic(HubCommand::Flag { user_id, flag, x, y }),
            ClientMsg::Chat { text } => {
                // Nothing left after cleaning: nothing to say.
                let Some(text) = chat::sanitize(&text) else { return Vec::new() };
                if self.link.is_none() {
                    return vec![hub_error("not_in_hub", "join the hub first")];
                }
                if !self.chat.allow(std::time::Instant::now()) {
                    return vec![hub_error("chat_too_fast", "You are talking too fast. Wait a moment.")];
                }
                self.forward(HubCommand::Chat { user_id, text })
            }
            ClientMsg::NpcInteract { npc } => self.forward(HubCommand::Interact { user_id, npc }),
            // One-item orders (older clients); the shop window sends its cart.
            ClientMsg::ShopBuy { npc, item, count } => {
                self.forward(HubCommand::Shop { user_id, npc, order: ShopOrder { buy: vec![(item, count.clamp(1, MAX_ORDER_COUNT))], sell: Vec::new() } })
            }
            ClientMsg::ShopSell { npc, item, count } => {
                self.forward(HubCommand::Shop { user_id, npc, order: ShopOrder { buy: Vec::new(), sell: vec![(item, count.clamp(1, MAX_ORDER_COUNT))] } })
            }
            ClientMsg::ShopDeal { npc, order } => self.forward(HubCommand::Shop { user_id, npc, order }),
            ClientMsg::Refine { npc, item, slot } => self.forward(HubCommand::Refine { user_id, npc, item, slot }),
            ClientMsg::ItemUse { item } => self.forward(HubCommand::UseItem { user_id, item }),
            ClientMsg::Friend { op } => self.on_friend(op).await,
            ClientMsg::Feedback { draft } => self.on_feedback(draft),
            ClientMsg::Equip { .. } | ClientMsg::Unequip { .. } | ClientMsg::Card { .. } if !self.in_hub() => {
                vec![hub_error("not_in_hub", "change your equipment in town")]
            }
            ClientMsg::Card { op, item, slot, card } => {
                let at = if slot == 0xFF { None } else { Slot::from_index(slot) };
                if slot != 0xFF && at.is_none() {
                    return vec![hub_error(ItemError::NotInBag.code(), ItemError::NotInBag.message())];
                }
                let mut done = None;
                let mut out = self
                    .change_bag(|_, bag| {
                        let key = if op == 0 { bag.socket(&item, at, &card)? } else { bag.unsocket(&item, at, &card)? };
                        done = Some(key);
                        Ok(())
                    })
                    .await;
                if let Some(key) = done {
                    out.push(Out::Msg(ServerMsg::Carded { op, item: key, card }));
                }
                out.extend(self.share_bag().await);
                out
            }
            ClientMsg::Equip { item, slot } => {
                let mut out = self.change_bag(|class, bag| bag.equip(class, &item, Slot::from_index(slot)).map(|_| ())).await;
                out.extend(self.share_bag().await);
                out
            }
            ClientMsg::Unequip { slot } => {
                let mut out = self.change_bag(|_, bag| bag.unequip(Slot::from_index(slot).ok_or(ItemError::NotEquipment)?)).await;
                out.extend(self.share_bag().await);
                out
            }
            ClientMsg::StyleSet { look } => self.forward(HubCommand::Restyle { user_id, look }),
            ClientMsg::MissionSelect { mission, public } => {
                if !self.in_hub() {
                    return vec![lobby_error("not_in_hub", "missions are picked from a Kafra employee in the hub")];
                }
                self.forward(HubCommand::SelectMission { user_id, mission, public })
            }
            // Quick start: a party leader starts from wherever the party
            // window is; alone, the hub checks the player is at a giver.
            ClientMsg::MissionStart { mission, tier, mask } => {
                let Some(lobbies) = self.state.lobbies.clone() else {
                    return vec![lobby_error("mission_unavailable", "missions are not available on this server")];
                };
                if !self.in_hub() {
                    return vec![lobby_error("not_in_hub", "missions are started from the hub")];
                }
                if lobbies.lobby_of(user_id).is_none() {
                    return self.forward(HubCommand::QuickStart { user_id, mission, tier, mask });
                }
                let Some(profile) = self.profile().await else {
                    return vec![hub_error("no_character", "select a character first")];
                };
                lobby_result(lobbies.quick_start(profile, session_id, &mission, tier, mask, self.lobby_tx.clone()).map(|_| ()))
            }
            ClientMsg::HubChannels => match &self.state.hub {
                Some(dir) => vec![Out::Msg(ServerMsg::HubChannels { channels: dir.channels() })],
                None => vec![hub_error("hub_unavailable", "hub maps are not loaded")],
            },
            ClientMsg::HubLeave => {
                // Back to the character slots: offline in the party, which
                // the next character of the account picks up.
                if let Some(lobbies) = &self.state.lobbies {
                    lobbies.detach(user_id, session_id);
                }
                if self.link.is_some() {
                    self.leave_link();
                    vec![Out::Msg(ServerMsg::HubLeft { reason: "requested".into() })]
                } else {
                    Vec::new()
                }
            }
            ClientMsg::HubJoin { channel } => self.enter_world(channel).await,
            ClientMsg::StageResume { accept } => self.resume_stage(accept).await,
            ClientMsg::StageLeave => {
                if matches!(self.link, Some(Link { place: Place::Stage { .. }, .. })) {
                    let back = self.mission_return();
                    self.join_hub(None, back).await
                } else {
                    vec![lobby_error("not_in_stage", "not in a stage")]
                }
            }
            ClientMsg::LobbyJoin { code } => {
                let Some(lobbies) = self.state.lobbies.clone() else {
                    return vec![lobby_error("mission_unavailable", "missions are not available on this server")];
                };
                if !self.in_hub() {
                    return vec![lobby_error("not_in_hub", "join a lobby from the hub")];
                }
                let Some(profile) = self.profile().await else {
                    return vec![hub_error("no_character", "select a character first")];
                };
                lobby_result(lobbies.join_code(profile, session_id, &code, self.lobby_tx.clone()).map(|_| ()))
            }
            ClientMsg::LobbyStart => match &self.state.lobbies {
                Some(_) if !self.in_hub() => vec![lobby_error("not_in_hub", "missions are started from the hub")],
                Some(l) => lobby_result(l.start(user_id, session_id).map(|_| ())),
                None => vec![Out::Msg(LobbyError::NotInLobby.to_msg())],
            },
            ClientMsg::LobbyModifiers { tier, mask } => match &self.state.lobbies {
                Some(l) => lobby_result(l.set_modifiers(user_id, session_id, tier, mask)),
                None => vec![Out::Msg(LobbyError::NotInLobby.to_msg())],
            },
            ClientMsg::LobbyKick { entity_id } => match &self.state.lobbies {
                Some(l) => lobby_result(l.kick(user_id, session_id, entity_id)),
                None => vec![Out::Msg(LobbyError::NotInLobby.to_msg())],
            },
            ClientMsg::LobbyLeave => match &self.state.lobbies {
                Some(l) if l.leave(user_id, session_id, "left") => Vec::new(),
                _ => vec![Out::Msg(LobbyError::NotInLobby.to_msg())],
            },
            // The Kafra's party list (`lobby.rs` `public_parties`): look,
            // join (a running mission too), list or hide one's own.
            ClientMsg::PartyList => match &self.state.lobbies {
                Some(l) => vec![Out::Msg(ServerMsg::Parties { parties: l.public_parties(user_id) })],
                None => vec![Out::Msg(ServerMsg::Parties { parties: Vec::new() })],
            },
            ClientMsg::PartyJoin { lobby_id } => {
                let Some(lobbies) = self.state.lobbies.clone() else {
                    return vec![lobby_error("mission_unavailable", "parties are not available on this server")];
                };
                if !self.in_hub() {
                    return vec![lobby_error("not_in_hub", "join a party in town")];
                }
                let Some(profile) = self.profile().await else {
                    return vec![hub_error("no_character", "select a character first")];
                };
                lobby_result(lobbies.join_public(profile, session_id, lobby_id, self.lobby_tx.clone()).map(|_| ()))
            }
            ClientMsg::PartyPublic { public } => match &self.state.lobbies {
                Some(l) => lobby_result(l.set_public(user_id, session_id, public)),
                None => vec![Out::Msg(LobbyError::NotInLobby.to_msg())],
            },
            ClientMsg::Craft { npc, recipe } => {
                if !self.in_hub() {
                    return vec![hub_error("not_in_hub", "Crafting is done in town.")];
                }
                self.forward(HubCommand::Craft { user_id, npc, recipe })
            }
            // The player context menu (`lobby.rs` has the party rules).
            ClientMsg::PartyInvite { entity_id } => {
                let Some(lobbies) = self.state.lobbies.clone() else {
                    return vec![lobby_error("mission_unavailable", "parties are not available on this server")];
                };
                if !self.in_hub() {
                    return vec![lobby_error("not_in_hub", "invite players in town")];
                }
                let Some(profile) = self.profile().await else {
                    return vec![hub_error("no_character", "select a character first")];
                };
                match lobbies.invite(profile, session_id, entity_id, self.lobby_tx.clone()) {
                    Ok(mission) => {
                        let ttl_ms = crate::world::lobby::INVITE_TTL.as_millis() as u32;
                        self.forward(HubCommand::PartyInvite { user_id, target: entity_id, mission, ttl_ms })
                    }
                    Err(e) => vec![Out::Msg(e.to_msg())],
                }
            }
            ClientMsg::PartyAnswer { from, accept } => {
                let Some(lobbies) = self.state.lobbies.clone() else {
                    return vec![lobby_error("mission_unavailable", "parties are not available on this server")];
                };
                if accept && !self.in_hub() {
                    return vec![lobby_error("not_in_hub", "join a party in town")];
                }
                let Some(profile) = self.profile().await else {
                    return vec![hub_error("no_character", "select a character first")];
                };
                lobby_result(lobbies.answer(profile, session_id, from, accept, self.lobby_tx.clone()))
            }
            ClientMsg::Inspect { entity_id } => self.forward(HubCommand::Inspect { user_id, target: entity_id }),
            // Must not be lost (a dropped lock or cancel would stall the
            // table); the hub refuses it on a stage.
            ClientMsg::Trade { op } => match &self.link {
                Some(link) => {
                    let _ = link.commands.send(HubCommand::TradeOp { user_id, op }).await;
                    Vec::new()
                }
                None => vec![hub_error("not_in_hub", "join the hub first")],
            },
        }
    }
}

enum SkillChange {
    Learn(String),
    Slot(u8, String),
    Reset,
    /// Mount (true) or get off: needs a learned mount skill.
    Mount(bool),
}

impl Conn {
    /// The town skill bar (`TOWN_SKILL`): one path for every skill usable
    /// in town. Learned and marked `town = true` (`SkillDef::town_use`), it
    /// mounts here (riding is the loadout's) or goes to the hub (the
    /// Pharmacy, the homunculus); anything else, or on a stage, is refused.
    async fn town_skill(&mut self, id: &str) -> Vec<Out> {
        use crate::world::combat::skills::TownUse;
        if matches!(self.link, Some(Link { place: Place::Stage { .. }, .. })) {
            return vec![hub_error("not_in_town", "On a mission skills are cast from the hotbar.")];
        }
        let book = Arc::clone(&self.state.skills);
        let (level, mounted) = match self.session.character.read().await.as_ref() {
            Some(ch) => (ch.loadout.level(id), ch.loadout.mounted),
            None => return vec![hub_error("no_character", "select a character first")],
        };
        if level == 0 {
            return vec![hub_error("not_learned", "You have not learned that skill.")];
        }
        match book.find(id).and_then(|(_, d)| d.town_use()) {
            None => vec![hub_error("not_in_town", "That skill is not used in town.")],
            Some(TownUse::Mount) => self.change_loadout(SkillChange::Mount(!mounted)).await,
            Some(town) => self.forward(HubCommand::TownSkill { user_id: self.user_id(), town }),
        }
    }

    /// Learns, slots or resets on the session's character, then hands the
    /// new loadout to the current hub or stage (which answers `skills`).
    async fn change_loadout(&mut self, change: SkillChange) -> Vec<Out> {
        let book = Arc::clone(&self.state.skills);
        let user_id = self.user_id();
        let loadout = {
            let mut guard = self.session.character.write().await;
            let Some(ch) = guard.as_mut() else {
                return vec![hub_error("no_character", "select a character first")];
            };
            let Some(class) = Class::from_id(&ch.archetype) else {
                return vec![hub_error("wrong_class", "this class has no skills")];
            };
            let mut l = ch.loadout.clone();
            let result = match change {
                SkillChange::Learn(id) => book.learn(class, ch.level, &mut l, &id).map(|_| ()),
                SkillChange::Slot(slot, id) => book.set_slot(&mut l, slot as usize, &id),
                SkillChange::Reset => {
                    l = Loadout::default();
                    Ok(())
                }
                SkillChange::Mount(on) => {
                    l.mounted = on;
                    if on && !book.rides(&l) { Err(crate::world::combat::skills::LearnError::NotLearned) } else { Ok(()) }
                }
            };
            if let Err(e) = result {
                return vec![hub_error(e.code(), e.to_string())];
            }
            // Mounted only while a mount skill is learned: a reset (or any
            // change) that drops it gets the rider off, here and saved.
            l.mounted = book.rides(&l);
            ch.set_loadout(l.clone());
            l
        };
        match &self.link {
            Some(link) => {
                // Must not be lost: the client waits for `skills`.
                let _ = link.commands.send(HubCommand::Loadout { user_id, loadout }).await;
                Vec::new()
            }
            None => Vec::new(),
        }
    }
}

impl Conn {
    /// The character's belongings as `inventory`.
    async fn inventory(&self) -> Option<Out> {
        let guard = self.session.character.read().await;
        let ch = guard.as_ref()?;
        let bag = &ch.bag;
        Some(Out::Msg(ServerMsg::Inventory {
            zeny: bag.zeny,
            // One entry per stack, named by its instance key (`sword+7`).
            items: bag.items.iter().map(|s| (s.key(), s.count)).collect(),
            equip: bag.equip.to_vec(),
            // Derived numbers with this gear (the Equipment window).
            stats: super::character_handlers::class_stats(&ch.archetype, ch.level as i64, bag).to_string(),
        }))
    }

    /// Applies `change` to the character's bag: on success the new
    /// `inventory` (persisted by the flusher), else the error.
    async fn change_bag(&self, change: impl FnOnce(Class, &mut Bag) -> Result<(), ItemError>) -> Vec<Out> {
        {
            let mut guard = self.session.character.write().await;
            let Some(ch) = guard.as_mut() else {
                return vec![hub_error("no_character", "select a character first")];
            };
            let class = Class::from_id(&ch.archetype).unwrap_or(Class::Swordsman);
            let mut bag = ch.bag.clone();
            // The cart's stacks (Pushcart) come with the learned skills.
            bag.cart = self.state.skills.cart_stacks(&ch.loadout) as u16;
            if let Err(e) = change(class, &mut bag) {
                return vec![hub_error(e.code(), e.message())];
            }
            ch.set_bag(bag);
        }
        self.inventory().await.into_iter().collect()
    }

    /// Hands the belongings to the hub (what other players see held).
    async fn share_bag(&self) -> Vec<Out> {
        let bag = self.session.character.read().await.as_ref().map(|c| c.bag.clone());
        if let (Some(bag), Some(link)) = (bag, &self.link) {
            let _ = link.commands.send(HubCommand::Bag { user_id: self.user_id(), bag }).await;
        }
        Vec::new()
    }
}

/// Persists the map the character is on (dirty flag; flushed later).
async fn remember_location(session: &ActiveSession, map_id: &str) {
    let mut guard = session.character.write().await;
    if let Some(ch) = guard.as_mut()
        && ch.last_location_id != map_id
    {
        ch.set_location(map_id.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::world::friends::Presence;

    fn conn(state: &AppState, user_id: u32) -> Conn {
        let (session, _signals) = ActiveSession::new(Uuid::new_v4(), user_id, None, None);
        Conn {
            state: state.clone(),
            session,
            conn_id: Uuid::new_v4(),
            link: None,
            lobby_tx: mpsc::channel(1).0,
            next_epoch: 0,
            last_channel: None,
            stranded: false,
            items_due: false,
            chat: ChatLimiter::default(),
            friend_tx: mpsc::channel(1).0,
            superseded: false,
            cast_prefs: CastPrefs::default(),
            use_prefs: UsePrefs::default(),
        }
    }

    fn presence(conn: &Conn) -> Presence {
        Presence {
            conn_id: conn.conn_id,
            character_id: 1,
            name: "Ghost".into(),
            class: "novice".into(),
            level: 1,
            map: "prontera".into(),
            channel: 1,
            tx: conn.friend_tx.clone(),
        }
    }

    /// A handler that panics mid-socket still takes the player offline: the
    /// guard runs the cleanup the unwound task never reached.
    #[tokio::test]
    async fn a_panicking_socket_task_still_cleans_up() {
        let (_, state) = crate::create_app(AppConfig::test_config().await).await.unwrap();
        let c = conn(&state, 7);
        state.friends.present(7, presence(&c)).await;
        assert!(state.friends.presence(7).is_some());
        let task = tokio::spawn(async move {
            let conn = guarded(c);
            assert!(!conn.superseded, "the guard derefs to the connection");
            panic!("a handler bug");
        });
        assert!(task.await.unwrap_err().is_panic());
        let gone = async {
            while state.friends.presence(7).is_some() {
                tokio::task::yield_now().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), gone).await.expect("the guard's cleanup ran");
    }

    /// A cancelled socket task (dropped mid-await) cleans up the same way.
    #[tokio::test]
    async fn a_cancelled_socket_task_still_cleans_up() {
        let (_, state) = crate::create_app(AppConfig::test_config().await).await.unwrap();
        let c = conn(&state, 8);
        state.friends.present(8, presence(&c)).await;
        let task = tokio::spawn(async move {
            let _conn = guarded(c);
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let gone = async {
            while state.friends.presence(8).is_some() {
                tokio::task::yield_now().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), gone).await.expect("the guard's cleanup ran");
    }

    /// The normal path dismisses the guard: the cleanup runs once, awaited
    /// in place, and the record of a newer socket of the account stays.
    #[tokio::test]
    async fn the_normal_path_cleans_up_once_in_place() {
        let (_, state) = crate::create_app(AppConfig::test_config().await).await.unwrap();
        let c = conn(&state, 9);
        state.friends.present(9, presence(&c)).await;
        DropGuard::dismiss(guarded(c)).finish().await;
        assert!(state.friends.presence(9).is_none(), "awaited, not deferred");
    }
}
