//! Player feedback over the socket (at the Scribe), the MCP endpoint that
//! reads and closes it, and the public status endpoints (`/health`,
//! `/api/online`).

use futures_util::{SinkExt, StreamExt};
use game_server::feedback::Draft;
use game_server::world::map::{
    CameraConstraint, Dimensions, HeightBounds, Heightmap, LocationKind, LocationManifest, MapData, NpcDef, WalkGrid,
};
use game_server::world::{ClientMsg, HubWorld, ServerMsg, Snapshot};
use game_server::{api::AppState, config::AppConfig, create_app_with_world};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const TOKEN: &str = "test-admin-token-mcp-0123456789";

/// A small town; the Scribe stands at its centre (where players arrive).
fn town(scribe: bool) -> HubWorld {
    let walk = WalkGrid::from_ascii(&["############", "#..........#", "#..........#", "#..........#", "############"]);
    let (w, h) = (walk.width(), walk.height());
    let npcs = if scribe {
        vec![NpcDef {
            id: "scribe".into(),
            sprite: "4_m_03".into(),
            name: "Scribe".into(),
            kind: "feedback".into(),
            shop: String::new(),
            x: 6,
            y: 2,
            facing: 4,
            interact_radius: 4.0,
        }]
    } else {
        Vec::new()
    };
    let manifest = LocationManifest {
        id: "town".into(),
        name: "town".into(),
        kind: LocationKind::Hub,
        dimensions: Dimensions { width: w, height: h, cell_size: 0.5 },
        height_bounds: HeightBounds { min: 0.0, max: 0.0 },
        camera: CameraConstraint::Free { pitch: -45.0, min_zoom: 12.0, max_zoom: 40.0, default_zoom: 24.0 },
        portals: Vec::new(),
        bgm: None,
        npcs,
        light_radius: 0.0,
        residents: Default::default(),
        spawn: None,
        return_point: None,
    };
    HubWorld::new(vec![MapData::new(manifest, walk, Heightmap::flat(w, h)).unwrap()], "town").unwrap()
}

async fn start(admin_token: Option<&str>, scribe: bool) -> (SocketAddr, AppState) {
    let mut config = AppConfig::test_config().await;
    config.admin_token = admin_token.map(str::to_string);
    config.flush_interval = Duration::from_secs(3600);
    let (app, state) = create_app_with_world(config, Some(town(scribe))).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.into_make_service()).await.unwrap() });
    (addr, state)
}

async fn player(addr: SocketAddr, name: &str) -> String {
    let http = reqwest::Client::new();
    let reg: Value = http
        .post(format!("http://{addr}/api/auth/register"))
        .json(&json!({ "username": name, "password": "FeedbackPassword123!" }))
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
        .json(&json!({ "name": format!("{name}_char"), "archetype": "archer" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = ch["id"].as_u64().unwrap();
    let resp = http.post(format!("http://{addr}/api/characters/{id}/select")).bearer_auth(&token).send().await.unwrap();
    assert!(resp.status().is_success());
    token
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

/// Connects and joins the hub; checks the versioned welcome.
async fn enter(addr: SocketAddr, token: &str) -> Ws {
    let (mut ws, _) = connect_async(format!("ws://{addr}/api/ws?token={token}")).await.unwrap();
    match next_msg(&mut ws).await {
        ServerMsg::Connected { server_version, .. } => assert_eq!(server_version, game_server::version::version()),
        other => panic!("expected connected, got {other:?}"),
    }
    ws.send(Message::Binary(ClientMsg::HubJoin { channel: None }.encode())).await.unwrap();
    while !matches!(next_msg(&mut ws).await, ServerMsg::MapEnter { .. }) {}
    ws
}

/// Sends a report; the server's answer `(id, code)`.
async fn report(ws: &mut Ws, title: &str, body: &str) -> (u32, String) {
    let draft = Draft {
        kind: "bug".into(),
        priority: "high".into(),
        title: title.into(),
        body: body.into(),
        client_version: "0.42 (2026-11-14)".into(),
        user_agent: "TestBrowser/1.0".into(),
    };
    ws.send(Message::Binary(ClientMsg::Feedback { draft }.encode())).await.unwrap();
    loop {
        if let ServerMsg::FeedbackResult { id, code, .. } = next_msg(ws).await {
            return (id, code);
        }
    }
}

/// One JSON-RPC call to `/mcp`: (HTTP status, body or Null).
async fn mcp(addr: SocketAddr, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut req = reqwest::Client::new().post(format!("http://{addr}/mcp")).json(&body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap();
    (status, if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::String(text)) })
}

/// `tools/call` that must succeed: its `structuredContent`.
async fn tool(addr: SocketAddr, name: &str, args: Value) -> Value {
    let (status, v) = mcp(addr, Some(TOKEN), json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": name, "arguments": args } })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["id"], 7);
    assert_eq!(v["result"]["isError"], false, "{name}: {v}");
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(serde_json::from_str::<Value>(text).unwrap(), v["result"]["structuredContent"], "text mirrors the structure");
    v["result"]["structuredContent"].clone()
}

#[tokio::test]
async fn feedback_is_checked_stored_rate_limited_and_worked_through_mcp() {
    let (addr, _state) = start(Some(TOKEN), true).await;
    let token = player(addr, "fiona").await;
    let mut ws = enter(addr, &token).await;

    // The form is checked before anything is stored.
    let (id, code) = report(&mut ws, "Bug", "too short").await;
    assert_eq!((id, code.as_str()), (0, "feedback_title"));
    let (id, code) = report(&mut ws, "Arrows vanish", "too short").await;
    assert_eq!((id, code.as_str()), (0, "feedback_body"));
    let body = "My arrows vanish when I shoot at a poring next to a wall.\n\nI expected them to hit. Maybe make walls stop arrows visibly?";
    let (first, code) = report(&mut ws, "  Arrows   vanish ", body).await;
    assert!(first > 0 && code.is_empty(), "{first} {code}");
    // A second one right away: too soon.
    let (id, code) = report(&mut ws, "Another one", body).await;
    assert_eq!((id, code.as_str()), (0, "feedback_too_soon"));

    // Public status: healthy, versioned, one player online.
    let health = reqwest::get(format!("http://{addr}/health")).await.unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    let health: Value = health.json().await.unwrap();
    assert_eq!((health["status"].as_str(), health["checks"]["database"].as_str()), (Some("healthy"), Some("ok")));
    assert_eq!(health["version"], game_server::version::version());
    assert!(health["uptime_secs"].is_u64());
    let online: Value = reqwest::get(format!("http://{addr}/api/online")).await.unwrap().json().await.unwrap();
    assert_eq!(online["online"], 1);
    assert!(online["server_time"].as_i64().unwrap() > 1_700_000_000_000);

    // MCP: handshake.
    let (status, init) = mcp(
        addr,
        Some(TOKEN),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "ws-game");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    let (status, body_) = mcp(addr, Some(TOKEN), json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
    assert_eq!((status, body_), (StatusCode::ACCEPTED, Value::Null));
    let (_, list) = mcp(addr, Some(TOKEN), json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).await;
    let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in [
        "list_feedback", "get_feedback", "close_feedback", "reopen_feedback", "server_status", "list_online_players",
        "get_character", "search_characters", "db_summary", "broadcast", "restart_warning", "save_all",
    ] {
        assert!(names.contains(&want), "missing tool {want}: {names:?}");
    }

    // The report, with its context.
    let page = tool(addr, "list_feedback", json!({ "status": "open" })).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["next_cursor"], Value::Null);
    let r = tool(addr, "get_feedback", json!({ "id": first })).await;
    assert_eq!(r["title"], "Arrows vanish");
    assert_eq!(r["body"], body);
    assert_eq!((r["kind"].as_str(), r["priority"].as_str(), r["status"].as_str()), (Some("bug"), Some("high"), Some("open")));
    assert_eq!((r["character_name"].as_str(), r["class"].as_str(), r["map"].as_str()), (Some("fiona_char"), Some("archer"), Some("town")));
    assert_eq!((r["client_version"].as_str(), r["user_agent"].as_str()), (Some("0.42 (2026-11-14)"), Some("TestBrowser/1.0")));
    assert!(r["position"]["x"].is_number() && r["level"].is_number());

    // Close it, see it gone from the open list, reopen it.
    let closed = tool(addr, "close_feedback", json!({ "id": first, "resolution": "Walls now stop arrows." })).await;
    assert_eq!((closed["status"].as_str(), closed["resolution"].as_str()), (Some("closed"), Some("Walls now stop arrows.")));
    let page = tool(addr, "list_feedback", json!({ "status": "open" })).await;
    assert!(page["items"].as_array().unwrap().is_empty());
    let reopened = tool(addr, "reopen_feedback", json!({ "id": first })).await;
    assert_eq!(reopened["status"], "open");

    // Status, players, characters, counts.
    let status = tool(addr, "server_status", json!({})).await;
    assert_eq!(status["online"], 1);
    assert_eq!(status["version"], game_server::version::version());
    assert_eq!(status["database"], "ok");
    let players = tool(addr, "list_online_players", json!({})).await;
    assert_eq!(players["players"][0]["name"], "fiona_char");
    let ch = tool(addr, "get_character", json!({ "name": "FIONA_CHAR" })).await;
    assert_eq!((ch["class"].as_str(), ch["username"].as_str()), (Some("archer"), Some("fiona")));
    assert!(ch["online"].is_object(), "{ch}");
    assert!(ch["bag"]["contents"].is_array());
    let found = tool(addr, "search_characters", json!({ "query": "ion" })).await;
    assert_eq!(found["count"], 1);
    let none = tool(addr, "search_characters", json!({ "query": "%" })).await;
    assert_eq!(none["count"], 0, "LIKE wildcards are matched literally");
    let summary = tool(addr, "db_summary", json!({})).await;
    assert_eq!((summary["users"].as_i64(), summary["characters"].as_i64()), (Some(1), Some(1)));
    assert_eq!(summary["feedback_by_status"]["open"], 1);
    let saved = tool(addr, "save_all", json!({})).await;
    assert_eq!(saved["sessions"], 1);
    let sent = tool(addr, "broadcast", json!({ "text": "Patch notes are up." })).await;
    assert_eq!(sent["delivered"], 1);
    let cancelled = tool(addr, "restart_warning", json!({ "cancel": true })).await;
    assert_eq!(cancelled["cancelled"], false);

    // Tool errors are results; protocol errors are JSON-RPC errors.
    let (_, v) = mcp(addr, Some(TOKEN), json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "get_feedback", "arguments": { "id": 999 } } })).await;
    assert_eq!(v["result"]["isError"], true);
    let (_, v) = mcp(addr, Some(TOKEN), json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "drop_tables" } })).await;
    assert_eq!(v["error"]["code"], -32602);
    let (_, v) = mcp(addr, Some(TOKEN), json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/list" })).await;
    assert_eq!(v["error"]["code"], -32601);
    let raw = reqwest::Client::new().post(format!("http://{addr}/mcp")).bearer_auth(TOKEN).body("{oops").send().await.unwrap();
    let v: Value = raw.json().await.unwrap();
    assert_eq!(v["error"]["code"], -32700);
}

#[tokio::test]
async fn mcp_wants_the_admin_token() {
    let (addr, _state) = start(Some(TOKEN), false).await;
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    assert_eq!(mcp(addr, None, ping.clone()).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(mcp(addr, Some("wrong-token"), ping.clone()).await.0, StatusCode::UNAUTHORIZED);
    let (status, v) = mcp(addr, Some(TOKEN), ping.clone()).await;
    assert_eq!((status, &v["result"]), (StatusCode::OK, &json!({})));
    // A browser page from elsewhere is refused even with the token.
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .bearer_auth(TOKEN)
        .header("Origin", "https://evil.example")
        .json(&ping)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let get = reqwest::Client::new().get(format!("http://{addr}/mcp")).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(get.status(), StatusCode::METHOD_NOT_ALLOWED);

    // Without ADMIN_TOKEN the endpoint is off.
    let (off, _state) = start(None, false).await;
    assert_eq!(mcp(off, Some(TOKEN), ping).await.0, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn feedback_wants_the_scribe_in_reach() {
    let (addr, _state) = start(None, false).await;
    let token = player(addr, "gwen").await;
    let mut ws = enter(addr, &token).await;
    let (id, code) = report(&mut ws, "Nobody here", "There is nobody to take my report in this town at all.").await;
    assert_eq!((id, code.as_str()), (0, "npc_too_far"));
}
