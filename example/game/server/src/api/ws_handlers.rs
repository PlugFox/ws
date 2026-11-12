use super::state::AppState;
use crate::session::{ActiveSession, SessionSignal, StateFlusher};
use crate::world::protocol::{decode_input, INPUT_KIND};
use crate::world::{
    ClientMsg, HubCommand, HubEvent, HubTicket, PlayerProfile, ServerMsg, OUTBOX_CAPACITY,
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
    let mut hub: Option<HubLink> = None;

    // Send welcome / connected frame
    let welcome = json!({
        "type": "connected",
        "user_id": user_id,
        "session_id": session_id.to_string(),
    });
    let _ = ws_tx.send(Message::Text(welcome.to_string().into())).await;

    // Run dual pump: WebSocket incoming messages + Session internal signals
    loop {
        tokio::select! {
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
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }

            // Events from the hub channel the player is in
            hub_event = recv_hub(&mut hub) => {
                let sent = match hub_event {
                    Some(HubEvent::Snapshot(frame)) => ws_tx.send(Message::Binary(frame)).await,
                    Some(HubEvent::Control(msg)) => {
                        if let ServerMsg::MapEnter { map, .. } = &msg {
                            remember_location(&session, &map.id).await;
                        }
                        ws_tx.send(Message::Text(to_json(&msg).into())).await
                    }
                    None => {
                        // The channel dropped us (outbox overflow or shutdown).
                        if let Some(link) = hub.take() {
                            leave_hub(&state, &session, link);
                        }
                        let msg = ServerMsg::HubLeft { reason: "evicted".into() };
                        ws_tx.send(Message::Text(to_json(&msg).into())).await
                    }
                };
                if sent.is_err() {
                    break;
                }
            }

            // Messages received from client
            msg_res = ws_rx.next() => {
                match msg_res {
                    Some(Ok(Message::Text(text))) => {
                        debug!("Received WS text from user {}: {}", user_id, text);
                        let replies = handle_client_text(&text, &state, &session, &mut hub).await;
                        let mut failed = false;
                        for reply in replies {
                            if ws_tx.send(Message::Text(to_json(&reply).into())).await.is_err() {
                                failed = true;
                                break;
                            }
                        }
                        if failed {
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(bin))) => {
                        let replies = handle_client_binary(&bin, &state, &session, &mut hub).await;
                        let mut failed = false;
                        for reply in replies {
                            if ws_tx.send(Message::Text(to_json(&reply).into())).await.is_err() {
                                failed = true;
                                break;
                            }
                        }
                        if failed {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = ws_tx.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Pong(_))) => {}
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
        }
    }

    // On disconnect, leave the hub and flush any dirty character state
    info!("Cleaning up WebSocket session for user {}", user_id);
    if let Some(link) = hub.take() {
        leave_hub(&state, &session, link);
    }
    let _ = StateFlusher::flush_session(&state.pool, &session).await;
}

/// The socket's membership in a hub channel.
struct HubLink {
    ticket: HubTicket,
    events: mpsc::Receiver<HubEvent>,
}

async fn recv_hub(hub: &mut Option<HubLink>) -> Option<HubEvent> {
    match hub {
        Some(link) => link.events.recv().await,
        None => std::future::pending().await,
    }
}

fn to_json(msg: &ServerMsg) -> String {
    serde_json::to_string(msg).expect("ServerMsg serializes")
}

fn hub_error(code: &str, message: impl Into<String>) -> ServerMsg {
    ServerMsg::HubError { code: code.into(), message: message.into() }
}

fn leave_hub(state: &AppState, session: &ActiveSession, link: HubLink) {
    if let Some(dir) = &state.hub {
        dir.leave(session.user_id, session.session_id);
    }
    drop(link);
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

/// Sends a command to the player's hub channel; replies come back through
/// the hub outbox.
fn forward(hub: &Option<HubLink>, cmd: HubCommand) -> Vec<ServerMsg> {
    match hub {
        Some(link) => match link.ticket.commands.try_send(cmd) {
            Ok(()) => Vec::new(),
            Err(_) => vec![hub_error("hub_busy", "hub channel is overloaded, try again")],
        },
        None => vec![hub_error("not_in_hub", "join the hub first")],
    }
}

/// Handles one client binary frame: a movement input packet, or a JSON
/// control message (first byte `{`) from clients that send everything as
/// binary.
async fn handle_client_binary(
    bin: &[u8],
    state: &AppState,
    session: &Arc<ActiveSession>,
    hub: &mut Option<HubLink>,
) -> Vec<ServerMsg> {
    match bin.first() {
        Some(b'{') => match std::str::from_utf8(bin) {
            Ok(text) => handle_client_text(text, state, session, hub).await,
            Err(_) => vec![hub_error("bad_message", "JSON frame is not UTF-8")],
        },
        Some(&INPUT_KIND) => {
            let Some((seq, dx, dy)) = decode_input(bin) else {
                return vec![hub_error("bad_message", "malformed input frame")];
            };
            if let Some(link) = hub {
                let _ = link.ticket.commands.try_send(HubCommand::Input { user_id: session.user_id, seq, dx, dy });
            }
            Vec::new()
        }
        // Unknown binary kinds are ignored, like unknown JSON types.
        _ => Vec::new(),
    }
}

/// Handles one client text frame; returns the direct replies.
async fn handle_client_text(
    text: &str,
    state: &AppState,
    session: &Arc<ActiveSession>,
    hub: &mut Option<HubLink>,
) -> Vec<ServerMsg> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return vec![hub_error("bad_message", "not JSON")];
    };
    // Unknown message types are ignored so older servers tolerate newer
    // clients; known types with a bad shape are reported.
    let known = matches!(
        value.get("type").and_then(|t| t.as_str()),
        Some(
            "ping" | "hub_join" | "hub_leave" | "hub_channels" | "hub_input" | "npc_interact" | "mission_select"
        )
    );
    if !known {
        return Vec::new();
    }
    let msg = match serde_json::from_value::<ClientMsg>(value) {
        Ok(msg) => msg,
        Err(err) => return vec![hub_error("bad_message", err.to_string())],
    };

    match msg {
        ClientMsg::Ping => vec![ServerMsg::Pong { ts: chrono::Utc::now().timestamp_millis() }],
        ClientMsg::HubInput { seq, dx, dy } => {
            if let Some(link) = hub {
                // Inputs are latest-wins; dropping one under backpressure is fine.
                let _ = link.ticket.commands.try_send(HubCommand::Input {
                    user_id: session.user_id,
                    seq,
                    dx,
                    dy,
                });
            }
            Vec::new()
        }
        ClientMsg::NpcInteract { npc } => {
            forward(hub, HubCommand::Interact { user_id: session.user_id, npc })
        }
        ClientMsg::MissionSelect { mission } => {
            forward(hub, HubCommand::SelectMission { user_id: session.user_id, mission })
        }
        ClientMsg::HubChannels => match &state.hub {
            Some(dir) => vec![ServerMsg::HubChannels { channels: dir.channels() }],
            None => vec![hub_error("hub_unavailable", "hub maps are not loaded")],
        },
        ClientMsg::HubLeave => {
            if let Some(link) = hub.take() {
                leave_hub(state, session, link);
                vec![ServerMsg::HubLeft { reason: "requested".into() }]
            } else {
                Vec::new()
            }
        }
        ClientMsg::HubJoin { channel } => {
            let Some(dir) = &state.hub else {
                return vec![hub_error("hub_unavailable", "hub maps are not loaded")];
            };
            let profile = {
                let guard = session.character.read().await;
                match guard.as_ref() {
                    Some(ch) => PlayerProfile {
                        user_id: session.user_id,
                        character_id: ch.id,
                        name: ch.name.clone(),
                        archetype: ch.archetype.clone(),
                    },
                    None => {
                        return vec![hub_error(
                            "no_character",
                            "select a character (POST /api/characters/{id}/select) first",
                        )];
                    }
                }
            };
            let (tx, rx) = mpsc::channel(OUTBOX_CAPACITY);
            match dir.join(profile, session.session_id, channel, tx) {
                Ok(ticket) => {
                    // Replacing the link drops the old receiver; the old
                    // channel already got a Leave from the directory.
                    *hub = Some(HubLink { ticket, events: rx });
                    Vec::new()
                }
                Err(err) => vec![hub_error(err.code(), err.to_string())],
            }
        }
    }
}
