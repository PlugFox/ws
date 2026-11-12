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
use tokio_tungstenite::{connect_async, tungstenite::protocol::frame::coding::CloseCode};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn test_single_active_session_mutual_exclusion() {
    let config = AppConfig::test_config();
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
    let mut config = AppConfig::test_config();
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
    let ws_url_1 = format!("ws://{}/api/ws?token={}", local_addr, access_token_1);
    let (mut ws_stream_1, _) = connect_async(&ws_url_1).await.expect("WS 1 connect failed");

    // Read connected welcome frame
    let welcome_msg = ws_stream_1.next().await.unwrap().unwrap();
    let welcome_val: Value = serde_json::from_str(&welcome_msg.to_string()).unwrap();
    assert_eq!(welcome_val["type"], "connected");
    assert_eq!(welcome_val["user_id"], user_id);

    // Send a ping and expect a pong
    let ping_msg = json!({ "type": "ping" });
    ws_stream_1
        .send(tokio_tungstenite::tungstenite::Message::Text(ping_msg.to_string().into()))
        .await
        .unwrap();

    let pong_msg = ws_stream_1.next().await.unwrap().unwrap();
    let pong_val: Value = serde_json::from_str(&pong_msg.to_string()).unwrap();
    assert_eq!(pong_val["type"], "pong");

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

    let ws_url_2 = format!("ws://{}/api/ws?token={}", local_addr, access_token_2);
    let (mut ws_stream_2, _) = connect_async(&ws_url_2).await.expect("WS 2 connect failed");

    let welcome_2 = ws_stream_2.next().await.unwrap().unwrap();
    let welcome_2_val: Value = serde_json::from_str(&welcome_2.to_string()).unwrap();
    assert_eq!(welcome_2_val["type"], "connected");

    // 3. Verify WebSocket 1 receives displacement JSON frame and is closed with code 4009
    let displaced_text = ws_stream_1.next().await.unwrap().unwrap();
    let displaced_val: Value = serde_json::from_str(&displaced_text.to_string()).unwrap();
    assert_eq!(displaced_val["type"], "displaced");

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
