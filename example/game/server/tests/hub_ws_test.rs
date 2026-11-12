//! End-to-end hub flow over HTTP + WebSocket with synthetic maps.

use futures_util::{SinkExt, StreamExt};
use game_server::world::map::{
    CameraConstraint, Dimensions, HeightBounds, Heightmap, LocationKind, LocationManifest, MapData, NpcDef, PortalDef,
    WalkGrid,
};
use game_server::world::{HubWorld, Snapshot};
use game_server::{config::AppConfig, create_app_with_world, api::AppState};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn synthetic_world() -> HubWorld {
    let map = |id: &str, kind, camera, rows: &[&str], portals| {
        let walk = WalkGrid::from_ascii(rows);
        let (w, h) = (walk.width(), walk.height());
        let manifest = LocationManifest {
            id: id.into(),
            name: id.into(),
            kind,
            dimensions: Dimensions { width: w, height: h, cell_size: 0.5 },
            height_bounds: HeightBounds { min: 0.0, max: 0.0 },
            camera,
            portals,
            bgm: None,
            npcs: Vec::new(),
        };
        MapData::new(manifest, walk, Heightmap::flat(w, h)).unwrap()
    };
    let mut town = map(
        "town",
        LocationKind::Hub,
        CameraConstraint::Free { pitch: -45.0, min_zoom: 12.0, max_zoom: 40.0, default_zoom: 24.0 },
        &["############", "#..........#", "#..........#", "#..........#", "#..........#", "#..........#", "############"],
        vec![PortalDef { name: "to_house".into(), trigger: [10, 3, 1], target_map: "house".into(), target_pos: [2, 2] }],
    );
    let house = map(
        "house",
        LocationKind::Indoor,
        CameraConstraint::Fixed { yaw: 0.0, pitch: -45.0, zoom: 18.0 },
        &["######", "#....#", "#....#", "#....#", "######"],
        vec![PortalDef { name: "exit".into(), trigger: [1, 2, 1], target_map: "town".into(), target_pos: [9, 3] }],
    );
    // Spawn is (5,3); the board at (5,5) is in reach right away.
    town.manifest.npcs.push(NpcDef {
        id: "board".into(),
        sprite: "mission_npc".into(),
        name: "Board Keeper".into(),
        kind: "mission_board".into(),
        x: 5,
        y: 5,
        facing: 4,
        interact_radius: 3.0,
    });
    HubWorld::new(vec![town, house], "town").unwrap()
}

async fn start() -> (SocketAddr, AppState) {
    let (app, state) = create_app_with_world(AppConfig::test_config(), Some(synthetic_world())).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.into_make_service()).await.unwrap() });
    (addr, state)
}

/// Registers a user, creates a character; optionally selects it. Returns
/// (access token, character id).
async fn player(addr: SocketAddr, name: &str, select: bool) -> (String, u64) {
    let http = reqwest::Client::new();
    let reg: Value = http
        .post(format!("http://{addr}/api/auth/register"))
        .json(&json!({ "username": name, "password": "HubPassword123!" }))
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
        .json(&json!({ "name": format!("{name}_char"), "archetype": "novice" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = ch["id"].as_u64().unwrap();
    if select {
        let resp = http
            .post(format!("http://{addr}/api/characters/{id}/select"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
    }
    (token, id)
}

async fn connect(addr: SocketAddr, token: &str) -> Ws {
    let (mut ws, _) = connect_async(format!("ws://{addr}/api/ws?token={token}")).await.unwrap();
    let hello = next_json(&mut ws).await;
    assert_eq!(hello["type"], "connected");
    ws
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// Next text frame, skipping binary snapshots.
async fn next_json(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(3), ws.next()).await.expect("timeout").unwrap().unwrap();
        match msg {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

async fn next_json_of(ws: &mut Ws, ty: &str) -> Value {
    loop {
        let v = next_json(ws).await;
        if v["type"] == ty {
            return v;
        }
    }
}

async fn next_snapshot(ws: &mut Ws, pred: impl Fn(&Snapshot) -> bool) -> Snapshot {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(3), ws.next()).await.expect("timeout").unwrap().unwrap();
        if let Message::Binary(b) = msg {
            let s = Snapshot::decode(&b).expect("valid snapshot");
            if pred(&s) {
                return s;
            }
        }
    }
}

#[tokio::test]
async fn join_requires_selected_character() {
    let (addr, _state) = start().await;
    let (token, _) = player(addr, "nochar", false).await;
    let mut ws = connect(addr, &token).await;
    send(&mut ws, json!({ "type": "hub_join" })).await;
    let err = next_json(&mut ws).await;
    assert_eq!(err["type"], "hub_error");
    assert_eq!(err["code"], "no_character");

    // Ping still works next to the hub protocol.
    send(&mut ws, json!({ "type": "ping" })).await;
    assert_eq!(next_json(&mut ws).await["type"], "pong");

    // Malformed known message is reported, unknown types are ignored.
    send(&mut ws, json!({ "type": "hub_input", "seq": "x" })).await;
    assert_eq!(next_json(&mut ws).await["code"], "bad_message");
}

#[tokio::test]
async fn hub_flow_join_move_portal_and_peers() {
    let (addr, state) = start().await;
    let (token_a, char_a) = player(addr, "alice", true).await;
    let (token_b, char_b) = player(addr, "bob", true).await;
    let mut a = connect(addr, &token_a).await;
    let mut b = connect(addr, &token_b).await;

    send(&mut a, json!({ "type": "hub_join" })).await;
    let joined = next_json(&mut a).await;
    assert_eq!(joined["type"], "hub_joined");
    assert_eq!(joined["channel_name"], "Hub-1");
    assert_eq!(joined["entity_id"], char_a);
    let enter = next_json(&mut a).await;
    assert_eq!(enter["type"], "map_enter");
    assert_eq!(enter["map"]["id"], "town");
    assert_eq!(enter["map"]["camera"]["type"], "free");
    assert_eq!(enter["map"]["camera"]["max_zoom"], 40.0);
    let x0 = enter["x"].as_f64().unwrap() as f32;

    // Bob joins: Alice sees him appear, Bob's map_enter lists Alice.
    send(&mut b, json!({ "type": "hub_join", "channel": 1 })).await;
    let b_enter = next_json_of(&mut b, "map_enter").await;
    assert_eq!(b_enter["players"][0]["entity_id"], char_a);
    let appeared = next_json_of(&mut a, "player_appeared").await;
    assert_eq!(appeared["player"]["entity_id"], char_b);

    send(&mut a, json!({ "type": "hub_channels" })).await;
    let list = next_json_of(&mut a, "hub_channels").await;
    assert_eq!(list["channels"][0]["population"], 2);

    // Alice walks east; the snapshot acks her input and shows her moving.
    for seq in 1..=3u32 {
        send(&mut a, json!({ "type": "hub_input", "seq": seq, "dx": 1.0, "dy": 0.0 })).await;
    }
    let snap = next_snapshot(&mut a, |s| s.ack_seq == 3).await;
    let me = snap.entities.iter().find(|e| e.0 == char_a as u32).unwrap();
    assert!(me.1 > x0, "moved east: {x0} -> {}", me.1);
    assert_eq!(snap.entities.len(), 2);

    // Keep walking into the door: fixed indoor camera arrives over the wire.
    let mut seq = 3u32;
    let indoor = loop {
        seq += 1;
        send(&mut a, json!({ "type": "hub_input", "seq": seq, "dx": 1.0, "dy": 0.0 })).await;
        let msg = tokio::time::timeout(Duration::from_secs(3), a.next()).await.unwrap().unwrap().unwrap();
        if let Message::Text(t) = msg {
            let v: Value = serde_json::from_str(&t).unwrap();
            if v["type"] == "map_enter" {
                break v;
            }
        }
        assert!(seq < 500, "never reached the door");
    };
    assert_eq!(indoor["map"]["id"], "house");
    assert_eq!(indoor["via_portal"], "to_house");
    assert_eq!(indoor["map_epoch"], 1);
    assert_eq!(indoor["map"]["camera"], json!({ "type": "fixed", "yaw": 0.0, "pitch": -45.0, "zoom": 18.0 }));
    assert_eq!(next_json_of(&mut b, "player_vanished").await["entity_id"], char_a);

    // The location is tracked on the in-memory character (flushed later).
    let session = state.session_registry.all_sessions().into_iter().find(|s| {
        s.character.try_read().map(|c| c.as_ref().map(|c| c.id as u64) == Some(char_a)).unwrap_or(false)
    });
    let session = session.expect("alice's session");
    let loc = session.character.read().await.as_ref().unwrap().last_location_id.clone();
    assert_eq!(loc, "house");

    // Disconnecting Alice frees her slot.
    a.close(None).await.unwrap();
    let hub = state.hub.as_ref().unwrap();
    for _ in 0..50 {
        if hub.channels()[0].population == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(hub.channels()[0].population, 1);
}

#[tokio::test]
async fn displaced_socket_leaves_hub_without_kicking_successor() {
    let (addr, state) = start().await;
    let (token_1, char_id) = player(addr, "carol", true).await;
    let mut ws1 = connect(addr, &token_1).await;
    send(&mut ws1, json!({ "type": "hub_join" })).await;
    next_json_of(&mut ws1, "map_enter").await;

    // Second login displaces the first session; select again and join.
    let http = reqwest::Client::new();
    let login: Value = http
        .post(format!("http://{addr}/api/auth/login"))
        .json(&json!({ "username": "carol", "password": "HubPassword123!" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token_2 = login["access_token"].as_str().unwrap().to_string();
    http.post(format!("http://{addr}/api/characters/{char_id}/select"))
        .bearer_auth(&token_2)
        .send()
        .await
        .unwrap();
    assert_eq!(next_json_of(&mut ws1, "displaced").await["type"], "displaced");

    let mut ws2 = connect(addr, &token_2).await;
    send(&mut ws2, json!({ "type": "hub_join" })).await;
    next_json_of(&mut ws2, "map_enter").await;
    // Snapshots keep flowing to the new socket after the old one is gone.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let snap = next_snapshot(&mut ws2, |_| true).await;
    assert_eq!(snap.entities.len(), 1);
    assert_eq!(state.hub.as_ref().unwrap().channels()[0].population, 1);
}

#[tokio::test]
async fn binary_frames_and_mission_board() {
    let (addr, _state) = start().await;
    let (token, char_id) = player(addr, "dave", true).await;
    let mut ws = connect(addr, &token).await;

    // JSON control message inside a binary frame.
    ws.send(Message::Binary(json!({ "type": "hub_join" }).to_string().into_bytes().into())).await.unwrap();
    let enter = next_json_of(&mut ws, "map_enter").await;
    let x0 = enter["x"].as_f64().unwrap() as f32;

    // Binary input: u8 kind, u32 seq, i16 dx, i16 dy.
    for seq in 1..=3u32 {
        let mut b = vec![0x02u8];
        b.extend_from_slice(&seq.to_le_bytes());
        b.extend_from_slice(&32767i16.to_le_bytes());
        b.extend_from_slice(&0i16.to_le_bytes());
        ws.send(Message::Binary(b.into())).await.unwrap();
    }
    let snap = next_snapshot(&mut ws, |s| s.ack_seq == 3).await;
    let me = snap.entities.iter().find(|e| e.0 == char_id as u32).unwrap();
    assert!(me.1 > x0);

    send(&mut ws, json!({ "type": "npc_interact", "npc": "board" })).await;
    let dialog = next_json_of(&mut ws, "npc_dialog").await;
    assert_eq!(dialog["kind"], "mission_board");
    assert_eq!(dialog["missions"][0]["id"], "poring_island_survival");

    send(&mut ws, json!({ "type": "mission_select", "mission": "poring_island_survival" })).await;
    assert_eq!(next_json_of(&mut ws, "hub_error").await["code"], "mission_unavailable");
}
