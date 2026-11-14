//! Friends over HTTP + WebSocket: a request from the player menu (by
//! entity id), accepting it, presence across sockets (online toast, map and
//! channel, offline), persistence across reconnects, and the bad cases.

use futures_util::{SinkExt, StreamExt};
use game_server::world::friends::{note, FriendInfo, FriendOp};
use game_server::world::map::{
    CameraConstraint, Dimensions, HeightBounds, Heightmap, LocationKind, LocationManifest, MapData, WalkGrid,
};
use game_server::world::{ClientMsg, HubWorld, ServerMsg, Snapshot};
use game_server::{config::AppConfig, create_app_with_world};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn town() -> HubWorld {
    let walk = WalkGrid::from_ascii(&["############", "#..........#", "#..........#", "#..........#", "############"]);
    let (w, h) = (walk.width(), walk.height());
    let manifest = LocationManifest {
        id: "town".into(),
        name: "Town".into(),
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

async fn start() -> SocketAddr {
    let (app, _state) = create_app_with_world(AppConfig::test_config().await, Some(town())).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.into_make_service()).await.unwrap() });
    addr
}

/// Registers, creates and selects a character: (token, character id).
async fn player(addr: SocketAddr, name: &str) -> (String, u32) {
    let http = reqwest::Client::new();
    let reg: Value = http
        .post(format!("http://{addr}/api/auth/register"))
        .json(&json!({ "username": name, "password": "FriendPassword123!" }))
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
    let resp = http.post(format!("http://{addr}/api/characters/{id}/select")).bearer_auth(&token).send().await.unwrap();
    assert!(resp.status().is_success());
    (token, id)
}

async fn send(ws: &mut Ws, m: ClientMsg) {
    ws.send(Message::Binary(m.encode())).await.unwrap();
}

async fn next_msg(ws: &mut Ws) -> ServerMsg {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(3), ws.next()).await.expect("timeout").unwrap().unwrap();
        match msg {
            Message::Binary(b) if Snapshot::decode(&b).is_some() => continue,
            Message::Binary(b) => match ServerMsg::decode(&b) {
                Some(m) => return m,
                // Kinds this test does not know about (other features).
                None => continue,
            },
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

async fn next_where(ws: &mut Ws, what: &str, pred: impl Fn(&ServerMsg) -> bool) -> ServerMsg {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "no {what} within 5 s");
        let m = next_msg(ws).await;
        if pred(&m) {
            return m;
        }
    }
}

/// Connects and joins the hub; returns the socket, the account id and the
/// first friends list (sent after the first map).
async fn enter(addr: SocketAddr, token: &str) -> (Ws, u32, Vec<FriendInfo>) {
    let (mut ws, _) = connect_async(format!("ws://{addr}/api/ws?token={token}")).await.unwrap();
    let ServerMsg::Connected { user_id, .. } = next_msg(&mut ws).await else { panic!("no connected") };
    send(&mut ws, ClientMsg::HubJoin { channel: None }).await;
    let list = friends(&mut ws).await;
    (ws, user_id, list)
}

async fn friends(ws: &mut Ws) -> Vec<FriendInfo> {
    match next_where(ws, "friends", |m| matches!(m, ServerMsg::Friends { .. })).await {
        ServerMsg::Friends { friends } => friends,
        _ => unreachable!(),
    }
}

async fn note(ws: &mut Ws) -> (u8, u32, String) {
    match next_where(ws, "friend note", |m| matches!(m, ServerMsg::FriendNote { .. })).await {
        ServerMsg::FriendNote { op, account, name } => (op, account, name),
        _ => unreachable!(),
    }
}

async fn error(ws: &mut Ws) -> String {
    match next_where(ws, "hub error", |m| matches!(m, ServerMsg::HubError { .. })).await {
        ServerMsg::HubError { code, .. } => code,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn request_accept_presence_and_persistence() {
    let addr = start().await;
    let (ann_token, ann_char) = player(addr, "fr_ann").await;
    let (bob_token, bob_char) = player(addr, "fr_bob").await;

    let (mut ann, ann_id, list) = enter(addr, &ann_token).await;
    assert!(list.is_empty());

    // Bad cases: herself, a townsperson / unknown entity, a missing name,
    // accepting a request nobody sent.
    send(&mut ann, ClientMsg::Friend { op: FriendOp::Add { entity_id: ann_char } }).await;
    assert_eq!(error(&mut ann).await, "friend_self");
    send(&mut ann, ClientMsg::Friend { op: FriendOp::Add { entity_id: 0x70_0001 } }).await;
    assert_eq!(error(&mut ann).await, "friend_unknown");
    send(&mut ann, ClientMsg::Friend { op: FriendOp::AddName { name: "nobody".into() } }).await;
    assert_eq!(error(&mut ann).await, "friend_unknown");
    send(&mut ann, ClientMsg::Friend { op: FriendOp::Accept { account: 12345 } }).await;
    assert_eq!(error(&mut ann).await, "friend_no_request");

    // Ann asks Bob (offline) from the player menu: by his entity id.
    send(&mut ann, ClientMsg::Friend { op: FriendOp::Add { entity_id: bob_char } }).await;
    let list = friends(&mut ann).await;
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].link, list[0].name.as_str(), list[0].online), (2, "fr_bob_char", false));
    assert_eq!(list[0].characters, vec![bob_char]);
    assert_eq!(note(&mut ann).await.0, note::SENT);
    // Twice is a duplicate.
    send(&mut ann, ClientMsg::Friend { op: FriendOp::Add { entity_id: bob_char } }).await;
    assert_eq!(error(&mut ann).await, "friend_pending");

    // Bob comes in: the request waits for him, without Ann's whereabouts.
    let (mut bob, bob_id, list) = enter(addr, &bob_token).await;
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].account, list[0].link, list[0].online, list[0].map.as_str()), (ann_id, 1, false, ""));

    send(&mut bob, ClientMsg::Friend { op: FriendOp::Accept { account: ann_id } }).await;
    let list = friends(&mut bob).await;
    assert_eq!((list[0].link, list[0].online, list[0].map.as_str(), list[0].channel), (0, true, "Town", 1));
    assert_eq!(note(&mut bob).await, (note::ACCEPTED, ann_id, "fr_ann_char".into()));
    // Ann hears it with her list: Bob is online, in Town on Hub-1.
    let list = friends(&mut ann).await;
    assert_eq!((list[0].account, list[0].link, list[0].online, list[0].map.as_str(), list[0].channel), (bob_id, 0, true, "Town", 1));
    assert_eq!(note(&mut ann).await, (note::ACCEPTED, bob_id, "fr_bob_char".into()));
    send(&mut ann, ClientMsg::Friend { op: FriendOp::Add { entity_id: bob_char } }).await;
    assert_eq!(error(&mut ann).await, "friend_already");

    // Bob leaves: Ann's list goes offline.
    bob.close(None).await.unwrap();
    drop(bob);
    let list = friends(&mut ann).await;
    assert!(!list[0].online, "{list:?}");
    assert_eq!(list[0].map, "");

    // Bob is back (the friendship is in the database): Ann gets the toast.
    let (mut bob, _, list) = enter(addr, &bob_token).await;
    assert_eq!((list[0].account, list[0].link, list[0].online), (ann_id, 0, true));
    assert_eq!(note(&mut ann).await, (note::ONLINE, bob_id, "fr_bob_char".into()));

    // Bob removes Ann: both lists are empty.
    send(&mut bob, ClientMsg::Friend { op: FriendOp::Remove { account: ann_id } }).await;
    assert!(friends(&mut bob).await.is_empty());
    assert_eq!(note(&mut bob).await.0, note::REMOVED);
    assert!(friends(&mut ann).await.is_empty());

    // Asking each other is a yes: by name this time.
    send(&mut bob, ClientMsg::Friend { op: FriendOp::AddName { name: "FR_ANN_CHAR".into() } }).await;
    assert_eq!(note(&mut bob).await.0, note::SENT);
    assert_eq!(note(&mut ann).await, (note::REQUEST, bob_id, "fr_bob_char".into()));
    send(&mut ann, ClientMsg::Friend { op: FriendOp::Add { entity_id: bob_char } }).await;
    assert_eq!(note(&mut ann).await.0, note::ACCEPTED);
    assert_eq!(note(&mut bob).await.0, note::ACCEPTED);
    send(&mut ann, ClientMsg::Friend { op: FriendOp::List }).await;
    let list = friends(&mut ann).await;
    assert_eq!((list[0].link, list[0].online), (0, true));
}
