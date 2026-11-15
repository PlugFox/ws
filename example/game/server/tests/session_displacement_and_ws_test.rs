use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use futures_util::{SinkExt, StreamExt};
use game_server::{
    config::AppConfig,
    create_app,
    db::SessionRepo,
    session::SessionSignal,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use game_server::world::{ClientMsg, ServerMsg};
use tokio_tungstenite::{connect_async, tungstenite::protocol::frame::coding::CloseCode};
use tower::ServiceExt;
use uuid::Uuid;
use std::assert_matches;

#[tokio::test]
async fn test_single_active_session_mutual_exclusion() {
    let config = AppConfig::test_config().await;
    let (app, state) = create_app(config).await.expect("Failed to create test app");

    // 1. Register a user
    let register_body = json!({
        "username": "lone_wolf",
        "password": "WolfPassword123!"
    });

    let reg_req = Request::builder()
        .method("POST")
        .uri("/api/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&register_body).unwrap()))
        .unwrap();

    let reg_resp = app.clone().oneshot(reg_req).await.unwrap();
    let reg_bytes = reg_resp.into_body().collect().await.unwrap().to_bytes();
    let reg_json: Value = serde_json::from_slice(&reg_bytes).unwrap();

    let user_id = reg_json["user"]["id"].as_u64().unwrap() as u32;
    let session_1_id: Uuid = {
        let active = state.session_registry.get(user_id).expect("Session 1 must be active");
        active.session_id
    };

    // Verify Session 1 is live in DB
    let s1_db = SessionRepo::find_by_id(&state.pool, &session_1_id).await.unwrap().unwrap();
    assert!(s1_db.revoked_at.is_none());

    // Subscribe to Session 1 signals to verify displacement notification
    let s1_active = state.session_registry.get(user_id).unwrap();
    let mut s1_rx = s1_active.signal_tx.subscribe();

    // 2. User logs in from a second location / device / tab
    let login_body = json!({
        "username": "lone_wolf",
        "password": "WolfPassword123!"
    });

    let login_req = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&login_body).unwrap()))
        .unwrap();

    let login_resp = app.clone().oneshot(login_req).await.unwrap();
    assert_eq!(login_resp.status(), StatusCode::OK);

    // 3. Verify Session 1 received displacement signal
    let signal = s1_rx.recv().await.expect("Should receive displacement signal");
    match signal {
        SessionSignal::Displaced { ref reason } => {
            assert!(reason.contains("another"));
        }
        _ => panic!("Expected SessionSignal::Displaced, got {:?}", signal),
    }

    // 4. Verify Session 1 is marked as revoked in DB
    let s1_db_revoked = SessionRepo::find_by_id(&state.pool, &session_1_id).await.unwrap().unwrap();
    assert!(s1_db_revoked.revoked_at.is_some());
    assert_eq!(s1_db_revoked.revoked_reason, Some("session_displaced".to_string()));

    // 5. Verify Session 2 is now the single active session in registry
    let current_active = state.session_registry.get(user_id).expect("Session 2 must be active");
    assert_ne!(current_active.session_id, session_1_id);
    assert_eq!(state.session_registry.active_count(), 1);
}

#[tokio::test]
async fn test_websocket_connection_and_displacement_close_code() {
    let mut config = AppConfig::test_config().await;
    config.port = 0; // OS assigns free port

    // Start live TCP listener for real WebSocket handshake
    let (app, state) = create_app(config).await.expect("Failed to create app");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service()).await.unwrap();
    });

    // Register a user via HTTP
    let client = reqwest::Client::new();
    let reg_url = format!("http://{}/api/auth/register", local_addr);
    let reg_resp = client
        .post(&reg_url)
        .json(&json!({
            "username": "ws_pilot",
            "password": "WsPassword123!"
        }))
        .send()
        .await
        .unwrap();

    let reg_json: Value = reg_resp.json().await.unwrap();
    let user_id = reg_json["user"]["id"].as_u64().unwrap() as u32;
    let access_token_1 = reg_json["access_token"].as_str().unwrap();

    // 1. Establish WebSocket Connection 1
    let ws_url_1 = format!("ws://{}/api/ws?token={}&build={}", local_addr, access_token_1, game_server::version::client_build());
    let (mut ws_stream_1, _) = connect_async(&ws_url_1).await.expect("WS 1 connect failed");

    // Read connected welcome frame
    let welcome = server_msg(ws_stream_1.next().await.unwrap().unwrap());
    assert_matches!(welcome, ServerMsg::Connected { user_id: id, .. } if id == user_id, "{welcome:?}");

    // Send a ping and expect a pong
    ws_stream_1
        .send(tokio_tungstenite::tungstenite::Message::Binary(ClientMsg::Ping.encode()))
        .await
        .unwrap();
    let pong = server_msg(ws_stream_1.next().await.unwrap().unwrap());
    assert_matches!(pong, ServerMsg::Pong { .. }, "{pong:?}");

    // 2. User logs in from tab 2 and connects WebSocket Connection 2
    let login_url = format!("http://{}/api/auth/login", local_addr);
    let login_resp = client
        .post(&login_url)
        .json(&json!({
            "username": "ws_pilot",
            "password": "WsPassword123!"
        }))
        .send()
        .await
        .unwrap();

    let login_json: Value = login_resp.json().await.unwrap();
    let access_token_2 = login_json["access_token"].as_str().unwrap();

    let ws_url_2 = format!("ws://{}/api/ws?token={}&build={}", local_addr, access_token_2, game_server::version::client_build());
    let (mut ws_stream_2, _) = connect_async(&ws_url_2).await.expect("WS 2 connect failed");

    let welcome_2 = server_msg(ws_stream_2.next().await.unwrap().unwrap());
    assert_matches!(welcome_2, ServerMsg::Connected { .. });

    // 3. Verify WebSocket 1 receives the displacement frame and is closed with code 4009
    let displaced = server_msg(ws_stream_1.next().await.unwrap().unwrap());
    assert_matches!(displaced, ServerMsg::Displaced { .. }, "{displaced:?}");

    // Next message on WS 1 must be Close with code 4009
    let close_msg = ws_stream_1.next().await.unwrap().unwrap();
    match close_msg {
        tokio_tungstenite::tungstenite::Message::Close(Some(frame)) => {
            assert_eq!(frame.code, CloseCode::from(4009u16));
        }
        other => panic!("Expected Close frame with code 4009, got {:?}", other),
    }

    // Verify session registry only has 1 active session
    assert_eq!(state.session_registry.active_count(), 1);
}

fn server_msg(m: tokio_tungstenite::tungstenite::Message) -> ServerMsg {
    match m {
        tokio_tungstenite::tungstenite::Message::Binary(b) => ServerMsg::decode(&b).expect("control message"),
        other => panic!("expected a binary frame, got {other:?}"),
    }
}

/// A client of another build, or one from before the check (no build), is
/// closed before the session sees it: a stale tab never displaces the
/// player's current one.
#[tokio::test]
async fn test_websocket_refuses_other_client_builds() {
    let (app, state) = create_app(AppConfig::test_config().await).await.expect("Failed to create app");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.into_make_service()).await.unwrap() });
    let reg: Value = reqwest::Client::new()
        .post(format!("http://{addr}/api/auth/register"))
        .json(&json!({ "username": "stale_tab", "password": "StalePassword123!" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_id = reg["user"]["id"].as_u64().unwrap() as u32;
    let token = reg["access_token"].as_str().unwrap();
    let session = state.session_registry.get(user_id).expect("signed in").session_id;
    let server = game_server::version::client_build();
    let expected_reason = format!("update_required:{server}");

    // Another build: closed 4010 with the server's build, no welcome.
    let other = if server == "0badf00d" { "deadbeef" } else { "0badf00d" };
    let (mut ws, _) = connect_async(format!("ws://{addr}/api/ws?token={token}&build={other}")).await.unwrap();
    match ws.next().await.unwrap().unwrap() {
        tokio_tungstenite::tungstenite::Message::Close(Some(frame)) => {
            assert_eq!(frame.code, CloseCode::from(game_server::version::CLOSE_UPDATE_REQUIRED));
            assert_eq!(frame.reason.as_str(), expected_reason);
        }
        other => panic!("expected the update close, got {other:?}"),
    }

    // No build (a client from before the check): told in a notification it
    // can show, then the 4008 it does not retry.
    let (mut ws, _) = connect_async(format!("ws://{addr}/api/ws?token={token}")).await.unwrap();
    match server_msg(ws.next().await.unwrap().unwrap()) {
        ServerMsg::Notification { message } => assert!(message.contains("reload"), "{message}"),
        other => panic!("expected the update notification, got {other:?}"),
    }
    match ws.next().await.unwrap().unwrap() {
        tokio_tungstenite::tungstenite::Message::Close(Some(frame)) => assert_eq!(frame.code, CloseCode::from(4008u16)),
        other => panic!("expected a 4008 close, got {other:?}"),
    }

    // The session was left alone; the current build still gets in.
    assert_eq!(state.session_registry.get(user_id).map(|s| s.session_id), Some(session));
    let (mut ws, _) = connect_async(format!("ws://{addr}/api/ws?token={token}&build={server}")).await.unwrap();
    assert_matches!(server_msg(ws.next().await.unwrap().unwrap()), ServerMsg::Connected { .. });
}
