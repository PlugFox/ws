use super::state::AppState;
use crate::session::{ActiveSession, SessionSignal, StateFlusher};
use crate::world::map::LocationKind;
use crate::world::protocol::{decode_input, INPUT_KIND};
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
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct WsAuthQuery {
    pub token: Option<String>,
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

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, state, user_id, session_id)))
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
    let mut conn = Conn {
        state: state.clone(),
        session: Arc::clone(&session),
        conn_id: Uuid::new_v4(),
        link: None,
        lobby_tx,
        next_epoch: 0,
        last_channel: None,
        stranded: false,
    };

    // Send welcome / connected frame
    let welcome = json!({
        "type": "connected",
        "user_id": user_id,
        "session_id": session_id.to_string(),
    });
    let _ = ws_tx.send(Message::Text(welcome.to_string().into())).await;

    // Pump: session signals, events from the current instance (hub channel
    // or stage), lobby events and client frames.
    loop {
        let replies: Vec<Out> = tokio::select! {
            // Signal from session registry (displacement, force kick)
            signal_res = signal_rx.recv() => {
                match signal_res {
                    Ok(SessionSignal::Displaced { reason }) => {
                        warn!("User {} session displaced: {}", user_id, reason);
                        let msg = json!({
                            "type": "displaced",
                            "reason": reason
                        });
                        let _ = ws_tx.send(Message::Text(msg.to_string().into())).await;
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
                    Ok(SessionSignal::Notification { message }) => {
                        let note = json!({ "type": "notification", "message": message });
                        if let Err(e) = ws_tx.send(Message::Text(note.to_string().into())).await {
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

            // Messages received from client
            msg_res = ws_rx.next() => {
                match msg_res {
                    Some(Ok(Message::Text(text))) => {
                        debug!("Received WS text from user {}: {}", user_id, text);
                        conn.on_text(&text).await
                    }
                    Some(Ok(Message::Binary(bin))) => conn.on_binary(&bin).await,
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
                Out::Msg(msg) => Message::Text(to_json(&msg).into()),
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
    }

    // On disconnect, leave lobby and instance and flush dirty character state
    info!("Cleaning up WebSocket session for user {}", user_id);
    if let Some(lobbies) = &state.lobbies {
        lobbies.leave(user_id, conn.conn_id, "disconnected");
    }
    conn.leave_link();
    let _ = StateFlusher::flush_session(&state.pool, &session).await;
}

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
}

fn to_json(msg: &ServerMsg) -> String {
    serde_json::to_string(msg).expect("ServerMsg serializes")
}

fn hub_error(code: &str, message: impl Into<String>) -> Out {
    Out::Msg(ServerMsg::HubError { code: code.into(), message: message.into() })
}

fn lobby_error(code: &str, message: impl Into<String>) -> Out {
    Out::Msg(ServerMsg::LobbyError { code: code.into(), message: message.into() })
}

impl Conn {
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

    /// Joins (or switches) a hub channel. A stage is left only once the hub
    /// took the player, so a failed join leaves them where they were. The
    /// channel they came from is a hint: if it is gone or full, any channel
    /// will do.
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
        if hinted && matches!(result, Err(JoinError::NoSuchChannel(_) | JoinError::ChannelFull(_))) {
            self.last_channel = None;
            let (tx, rx2) = mpsc::channel(OUTBOX_CAPACITY);
            result = dir.join_at(profile, self.session_id(), None, tx, arrival, self.next_epoch);
            return self.adopt_hub(result, rx2);
        }
        self.adopt_hub(result, rx)
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
                Vec::new()
            }
            Err(err) => self.join_failed(hub_error(err.code(), err.to_string())),
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

    fn in_hub(&self) -> bool {
        matches!(self.link, Some(Link { place: Place::Hub { .. }, .. }))
    }

    async fn on_instance_event(&mut self, event: Option<HubEvent>) -> Vec<Out> {
        match event {
            Some(HubEvent::Snapshot(frame)) => vec![Out::Binary(frame)],
            Some(HubEvent::Control(msg)) => {
                if let ServerMsg::MapEnter { map, map_epoch, .. } = &msg {
                    self.next_epoch = map_epoch.wrapping_add(1);
                    // Stages are not a place to come back to.
                    if map.kind != LocationKind::Field {
                        remember_location(&self.session, &map.id).await;
                    }
                }
                vec![Out::Msg(msg)]
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
            Some(HubEvent::Transfer { target_map, arrival: (x, y) }) => {
                // Left the stage through its exit; the actor already dropped us.
                if let Some(Link { place: Place::Stage { stage_id }, .. }) = self.link.take()
                    && let Some(lobbies) = &self.state.lobbies
                {
                    lobbies.stages().leave(stage_id, self.user_id(), self.session_id(), false);
                }
                self.stranded = true;
                self.join_hub(None, Some(Arrival { map: target_map, x, y })).await
            }
            None => {
                // The instance dropped us (outbox overflow or shutdown).
                let place = self.link.as_ref().map(|l| l.place);
                self.leave_link();
                match place {
                    Some(Place::Stage { .. }) => {
                        self.stranded = true;
                        self.join_hub(None, None).await
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
                let mut out = vec![Out::Msg(ServerMsg::LobbyLeft { reason: "launched".into() })];
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

    /// Handles one client binary frame: a movement input packet, or a JSON
    /// control message (first byte `{`) from clients that send everything as
    /// binary.
    async fn on_binary(&mut self, bin: &[u8]) -> Vec<Out> {
        match bin.first() {
            Some(b'{') => match std::str::from_utf8(bin) {
                Ok(text) => self.on_text(text).await,
                Err(_) => vec![hub_error("bad_message", "JSON frame is not UTF-8")],
            },
            Some(&INPUT_KIND) => {
                let Some((seq, dx, dy)) = decode_input(bin) else {
                    return vec![hub_error("bad_message", "malformed input frame")];
                };
                if let Some(link) = &self.link {
                    // Inputs are latest-wins; dropping one under backpressure is fine.
                    let _ = link.commands.try_send(HubCommand::Input { user_id: self.user_id(), seq, dx, dy });
                }
                Vec::new()
            }
            // Unknown binary kinds are ignored, like unknown JSON types.
            _ => Vec::new(),
        }
    }

    /// Handles one client text frame; returns the direct replies.
    async fn on_text(&mut self, text: &str) -> Vec<Out> {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            return vec![hub_error("bad_message", "not JSON")];
        };
        // Unknown message types are ignored so older servers tolerate newer
        // clients; known types with a bad shape are reported.
        let known = matches!(
            value.get("type").and_then(|t| t.as_str()),
            Some(
                "ping"
                    | "hub_join"
                    | "hub_leave"
                    | "hub_channels"
                    | "hub_input"
                    | "npc_interact"
                    | "mission_select"
                    | "lobby_join"
                    | "lobby_ready"
                    | "lobby_start"
                    | "lobby_leave"
                    | "lobby_kick"
                    | "stage_leave"
            )
        );
        if !known {
            return Vec::new();
        }
        let msg = match serde_json::from_value::<ClientMsg>(value) {
            Ok(msg) => msg,
            Err(err) => return vec![hub_error("bad_message", err.to_string())],
        };
        let (user_id, session_id) = (self.user_id(), self.session_id());
        let lobby_result = |r: Result<(), LobbyError>| match r {
            Ok(()) => Vec::new(),
            Err(e) => vec![Out::Msg(e.to_msg())],
        };

        match msg {
            ClientMsg::Ping => vec![Out::Msg(ServerMsg::Pong { ts: chrono::Utc::now().timestamp_millis() })],
            ClientMsg::HubInput { seq, dx, dy } => {
                if let Some(link) = &self.link {
                    let _ = link.commands.try_send(HubCommand::Input { user_id, seq, dx, dy });
                }
                Vec::new()
            }
            ClientMsg::NpcInteract { npc } => self.forward(HubCommand::Interact { user_id, npc }),
            ClientMsg::MissionSelect { mission, public } => {
                if !self.in_hub() {
                    return vec![lobby_error("not_in_hub", "missions are picked at a board in the hub")];
                }
                self.forward(HubCommand::SelectMission { user_id, mission, public })
            }
            ClientMsg::HubChannels => match &self.state.hub {
                Some(dir) => vec![Out::Msg(ServerMsg::HubChannels { channels: dir.channels() })],
                None => vec![hub_error("hub_unavailable", "hub maps are not loaded")],
            },
            ClientMsg::HubLeave => {
                if let Some(lobbies) = &self.state.lobbies {
                    lobbies.leave(user_id, session_id, "left");
                }
                if self.link.is_some() {
                    self.leave_link();
                    vec![Out::Msg(ServerMsg::HubLeft { reason: "requested".into() })]
                } else {
                    Vec::new()
                }
            }
            ClientMsg::HubJoin { channel } => self.join_hub(channel, None).await,
            ClientMsg::StageLeave => {
                if matches!(self.link, Some(Link { place: Place::Stage { .. }, .. })) {
                    self.join_hub(None, None).await
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
            ClientMsg::LobbyReady { ready } => match &self.state.lobbies {
                Some(l) => lobby_result(l.set_ready(user_id, session_id, ready)),
                None => vec![Out::Msg(LobbyError::NotInLobby.to_msg())],
            },
            ClientMsg::LobbyStart => match &self.state.lobbies {
                Some(l) => lobby_result(l.start(user_id, session_id).map(|_| ())),
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
        }
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
