//! Local chat over the socket, and the admin API: announcements, the
//! restart countdown, save-all.

use futures_util::{SinkExt, StreamExt};
use game_server::db::CharacterRepo;
use game_server::world::map::{
    CameraConstraint, Dimensions, HeightBounds, Heightmap, LocationKind, LocationManifest, MapData, WalkGrid,
};
use game_server::world::{ClientMsg, HubWorld, ServerMsg, Snapshot};
use game_server::{api::AppState, config::AppConfig, create_app_with_world};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const TOKEN: &str = "test-admin-token-0123456789";

fn town() -> HubWorld {
    let walk = WalkGrid::from_ascii(&["############", "#..........#", "#..........#", "#..........#", "############"]);
    let (w, h) = (walk.width(), walk.height());
    let manifest = LocationManifest {
        id: "town".into(),
        name: "town".into(),
        kind: LocationKind::Hub,
        dimensions: Dimensions { width: w, height: h, cell_size: 0.5 },
        height_bounds: HeightBounds { min: 0.0, max: 0.0 },
        camera: CameraConstraint::Free { pitch: -45.0, min_zoom: 12.0, max_zoom: 40.0, default_zoom: 24.0 },
        portals: Vec::new(),
        bgm: None,
        npcs: Vec::new(),
        light_radius: 0.0,
        residents: Default::default(),
        spawn: None,
        return_point: None,
    };
    HubWorld::new(vec![MapData::new(manifest, walk, Heightmap::flat(w, h)).unwrap()], "town").unwrap()
}

async fn start(admin_token: Option<&str>) -> (SocketAddr, AppState) {
    let mut config = AppConfig::test_config().await;
    config.admin_token = admin_token.map(str::to_string);
    // Only the admin API saves during these tests.
    config.flush_interval = Duration::from_secs(3600);
    let (app, state) = create_app_with_world(config, Some(town())).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.into_make_service()).await.unwrap() });
    (addr, state)
}

/// Registers a user with a selected character: (access token, character id).
async fn player(addr: SocketAddr, name: &str) -> (String, u32) {
    let http = reqwest::Client::new();
    let reg: Value = http
        .post(format!("http://{addr}/api/auth/register"))
        .json(&json!({ "username": name, "password": "ChatPassword123!" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = reg["access_token"].as_str().unwrap().to_string();
    let ch: Value = http
        .post(format!("http://{addr}/api/characters"))
        .bearer_auth(&token)
        .json(&json!({ "name": format!("{name}_char"), "archetype": "mage" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = ch["id"].as_u64().unwrap() as u32;
    let resp =
        http.post(format!("http://{addr}/api/characters/{id}/select")).bearer_auth(&token).send().await.unwrap();
    assert!(resp.status().is_success());
    (token, id)
}

async fn connect(addr: SocketAddr, token: &str) -> Ws {
    let (mut ws, _) = connect_async(format!("ws://{addr}/api/ws?token={token}")).await.unwrap();
    assert!(matches!(next_msg(&mut ws).await, ServerMsg::Connected { .. }));
    ws
}

async fn send(ws: &mut Ws, m: ClientMsg) {
    ws.send(Message::Binary(m.encode())).await.unwrap();
}

async fn next_msg(ws: &mut Ws) -> ServerMsg {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(3), ws.next()).await.expect("timeout").unwrap().unwrap();
        match msg {
            Message::Binary(b) if Snapshot::decode(&b).is_some() => continue,
            Message::Binary(b) => return ServerMsg::decode(&b).unwrap_or_else(|| panic!("undecodable frame {b:02x?}")),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

/// Next chat-related message (say, announce, hub error), skipping the rest.
async fn next_chat(ws: &mut Ws) -> ServerMsg {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "no chat message within 5 s");
        let m = next_msg(ws).await;
        if matches!(m, ServerMsg::Chat { .. } | ServerMsg::Announce { .. } | ServerMsg::HubError { .. }) {
            return m;
        }
    }
}

/// No chat-related message for `ms`.
async fn quiet_for(ws: &mut Ws, ms: u64) {
    let until = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let left = until.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return;
        }
        match tokio::time::timeout(left, ws.next()).await {
            Err(_) => return,
            Ok(Some(Ok(Message::Binary(b)))) => {
                if let Some(m) = ServerMsg::decode(&b) {
                    assert!(
                        !matches!(m, ServerMsg::Chat { .. } | ServerMsg::Announce { .. }),
                        "unexpected {m:?}"
                    );
                }
            }
            Ok(other) => panic!("unexpected {other:?}"),
        }
    }
}

async fn join_hub(ws: &mut Ws) {
    send(ws, ClientMsg::HubJoin { channel: None }).await;
    loop {
        if matches!(next_msg(ws).await, ServerMsg::MapEnter { .. }) {
            return;
        }
    }
}

fn admin(addr: SocketAddr, method: reqwest::Method, path: &str, token: Option<&str>) -> reqwest::RequestBuilder {
    let req = reqwest::Client::new().request(method, format!("http://{addr}{path}"));
    match token {
        Some(t) => req.bearer_auth(t),
        None => req,
    }
}

#[tokio::test]
async fn local_chat_is_cleaned_delivered_and_rate_limited() {
    let (addr, _state) = start(None).await;
    let (ta, ca) = player(addr, "anna").await;
    let (tb, _) = player(addr, "boris").await;
    let mut a = connect(addr, &ta).await;
    let mut b = connect(addr, &tb).await;

    // Not in an instance yet: nobody to talk to.
    send(&mut a, ClientMsg::Chat { text: "hello?".into() }).await;
    assert!(matches!(next_chat(&mut a).await, ServerMsg::HubError { code, .. } if code == "not_in_hub"));

    join_hub(&mut a).await;
    join_hub(&mut b).await;
    send(&mut a, ClientMsg::Chat { text: "  hi\u{0007} there \u{202E}all ".into() }).await;
    let want = ServerMsg::Chat { entity_id: ca, name: "anna_char".into(), text: "hi there all".into() };
    assert_eq!(next_chat(&mut b).await, want, "the listener");
    assert_eq!(next_chat(&mut a).await, want, "the speaker sees their own line");

    // Long lines are cut, blank ones dropped.
    send(&mut a, ClientMsg::Chat { text: "x".repeat(1000) }).await;
    let ServerMsg::Chat { text, .. } = next_chat(&mut b).await else { panic!("chat") };
    assert_eq!(text.chars().count(), game_server::world::chat::CHAT_MAX_CHARS);
    send(&mut a, ClientMsg::Chat { text: " \r\n\t ".into() }).await;

    // The burst is 4 lines (two spent): two more, then "too fast".
    for i in 0..2 {
        send(&mut a, ClientMsg::Chat { text: format!("line {i}") }).await;
    }
    send(&mut a, ClientMsg::Chat { text: "spam".into() }).await;
    let mut got = Vec::new();
    for _ in 0..2 {
        match next_chat(&mut b).await {
            ServerMsg::Chat { text, .. } => got.push(text),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(got, ["line 0", "line 1"], "the blank line was not sent");
    quiet_for(&mut b, 200).await;
    // The speaker: own lines (long one included), then the error.
    loop {
        match next_chat(&mut a).await {
            ServerMsg::Chat { .. } => continue,
            ServerMsg::HubError { code, .. } => {
                assert_eq!(code, "chat_too_fast");
                break;
            }
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test]
async fn admin_api_needs_its_token() {
    let (addr, _) = start(None).await;
    let r = admin(addr, reqwest::Method::POST, "/admin/save", Some(TOKEN)).send().await.unwrap();
    assert_eq!(r.status(), 503, "disabled without ADMIN_TOKEN");

    let (addr, _) = start(Some(TOKEN)).await;
    for token in [None, Some("wrong"), Some("test-admin-token-0123456788")] {
        let r = admin(addr, reqwest::Method::POST, "/admin/save", token).send().await.unwrap();
        assert_eq!(r.status(), 401, "{token:?}");
        let r = admin(addr, reqwest::Method::POST, "/admin/broadcast", token).json(&json!({"text": "x"})).send().await.unwrap();
        assert_eq!(r.status(), 401);
    }
    let r = admin(addr, reqwest::Method::POST, "/admin/save", Some(TOKEN)).send().await.unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn broadcast_reaches_every_socket_in_and_out_of_the_hub() {
    let (addr, _) = start(Some(TOKEN)).await;
    let (ta, _) = player(addr, "carl").await;
    let (tb, _) = player(addr, "dana").await;
    let mut a = connect(addr, &ta).await;
    let mut b = connect(addr, &tb).await;
    join_hub(&mut a).await;

    let r: Value = admin(addr, reqwest::Method::POST, "/admin/broadcast", Some(TOKEN))
        .json(&json!({ "text": "Double EXP\u{0000} weekend!" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["delivered"], 2);
    for ws in [&mut a, &mut b] {
        assert_eq!(next_chat(ws).await, ServerMsg::Announce { text: "Double EXP weekend!".into() });
    }
    let r = admin(addr, reqwest::Method::POST, "/admin/broadcast", Some(TOKEN)).json(&json!({ "text": "  " })).send().await.unwrap();
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn restart_countdown_announces_each_mark_and_can_be_cancelled() {
    let (addr, state) = start(Some(TOKEN)).await;
    // A countdown minute lasts 60 ms here.
    state.admin.set_minute(Duration::from_millis(60));
    let (ta, ca) = player(addr, "erik").await;
    let mut a = connect(addr, &ta).await;
    join_hub(&mut a).await;

    let r: Value = admin(addr, reqwest::Method::POST, "/admin/restart-warning", Some(TOKEN))
        .json(&json!({ "minutes": 3, "message": "Patch 1.2." }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["scheduled"]["minutes"], 3);
    assert_eq!(r["scheduled"]["pending"], json!([3, 2, 1, 0]));
    // Unsaved progress is written at the restart mark.
    if let Some(ch) = state.session_registry.all_sessions()[0].character.write().await.as_mut() {
        ch.add_exp(77);
    }
    let mut got = Vec::new();
    for _ in 0..4 {
        match next_chat(&mut a).await {
            ServerMsg::Announce { text } => got.push(text),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        got,
        [
            "Server restart in 3 minutes. Patch 1.2.",
            "Server restart in 2 minutes. Patch 1.2.",
            "Server restart in 1 minute. Patch 1.2.",
            "The server is restarting now.",
        ]
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let rec = CharacterRepo::find_by_id(&state.pool, ca).await.unwrap().unwrap();
    assert_eq!(rec.base_exp, 77, "saved at the restart");
    let r: Value =
        admin(addr, reqwest::Method::GET, "/admin/restart-warning", Some(TOKEN)).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["scheduled"], Value::Null, "done");

    // A long one, cancelled after the first announcement.
    state.admin.set_minute(Duration::from_secs(60));
    let r = admin(addr, reqwest::Method::POST, "/admin/restart-warning", Some(TOKEN)).json(&json!({ "minutes": 0 })).send().await.unwrap();
    assert_eq!(r.status(), 400);
    admin(addr, reqwest::Method::POST, "/admin/restart-warning", Some(TOKEN)).json(&json!({ "minutes": 30 })).send().await.unwrap();
    assert_eq!(next_chat(&mut a).await, ServerMsg::Announce { text: "Server restart in 30 minutes.".into() });
    let r: Value =
        admin(addr, reqwest::Method::GET, "/admin/restart-warning", Some(TOKEN)).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["scheduled"]["pending"], json!([15, 10, 5, 4, 3, 2, 1, 0]));
    let r: Value =
        admin(addr, reqwest::Method::DELETE, "/admin/restart-warning", Some(TOKEN)).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["cancelled"], true);
    assert_eq!(next_chat(&mut a).await, ServerMsg::Announce { text: "The server restart was cancelled.".into() });
    let r: Value =
        admin(addr, reqwest::Method::DELETE, "/admin/restart-warning", Some(TOKEN)).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["cancelled"], false, "nothing left to cancel");
}

#[tokio::test]
async fn save_writes_dirty_characters_once_even_when_called_concurrently() {
    let (addr, state) = start(Some(TOKEN)).await;
    let mut chars = Vec::new();
    let mut sockets = Vec::new();
    for name in ["fred", "gina", "hugo"] {
        let (t, c) = player(addr, name).await;
        sockets.push(connect(addr, &t).await);
        chars.push(c);
    }
    // Two of the three have unsaved progress.
    for s in state.session_registry.all_sessions() {
        if let Some(ch) = s.character.write().await.as_mut()
            && ch.id != chars[2]
        {
            ch.add_exp(ch.id as u64 * 10);
        }
    }
    let calls = (0..6).map(|_| {
        tokio::spawn(async move {
            let r: Value =
                admin(addr, reqwest::Method::POST, "/admin/save", Some(TOKEN)).send().await.unwrap().json().await.unwrap();
            r
        })
    });
    let mut saved = 0;
    for call in calls.collect::<Vec<_>>() {
        let r = call.await.unwrap();
        assert_eq!(r["sessions"], 3);
        assert_eq!(r["failed"], 0);
        saved += r["saved"].as_u64().unwrap();
    }
    assert_eq!(saved, 2, "each dirty character saved exactly once");
    for &c in &chars[..2] {
        let rec = CharacterRepo::find_by_id(&state.pool, c).await.unwrap().unwrap();
        assert_eq!(rec.base_exp, c as i64 * 10);
    }
    let r: Value = admin(addr, reqwest::Method::POST, "/admin/save", Some(TOKEN)).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["saved"], 0, "nothing left unsaved");
    drop(sockets);
}
