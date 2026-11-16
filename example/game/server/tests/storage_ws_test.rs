//! The account storage end to end: two characters of one account over
//! HTTP + WebSocket, the storage moves, the database rows, the revision
//! fence against a stale session.

use futures_util::{SinkExt, StreamExt};
use game_server::db::character_repo::CharacterRepo;
use game_server::db::StorageRepo;
use game_server::session::StateFlusher;
use game_server::world::combat::vault::Dir;
use game_server::world::combat::{Bag, Class};
use game_server::world::map::{
    CameraConstraint, Dimensions, HeightBounds, Heightmap, LocationKind, LocationManifest, MapData, WalkGrid,
};
use game_server::world::protocol::StorageDir;
use game_server::world::{ClientMsg, HubWorld, ServerMsg, Snapshot};
use game_server::{api::AppState, config::AppConfig, create_app_with_world};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use std::assert_matches;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn town() -> HubWorld {
    let walk = WalkGrid::from_ascii(&["############", "#..........#", "#..........#", "#..........#", "#..........#", "#..........#", "############"]);
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

async fn start() -> (SocketAddr, AppState) {
    let (app, state) = create_app_with_world(AppConfig::test_config().await, Some(town())).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.into_make_service()).await.unwrap() });
    (addr, state)
}

/// Registers, creates and selects a mage (100 zeny, five Red Potions).
/// Returns (account id, token, character id = entity id).
async fn player(addr: SocketAddr, name: &str) -> (u32, String, u32) {
    let http = reqwest::Client::new();
    let reg: Value = http
        .post(format!("http://{addr}/api/auth/register"))
        .json(&json!({ "username": name, "password": "TradePassword123!" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = reg["access_token"].as_str().unwrap().to_string();
    let user = reg["user"]["id"].as_u64().unwrap() as u32;
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
    (user, token, id)
}

async fn connect(addr: SocketAddr, token: &str) -> Ws {
    let (mut ws, _) = connect_async(format!("ws://{addr}/api/ws?token={token}&build={}", game_server::version::client_build())).await.unwrap();
    assert_matches!(next_msg(&mut ws).await, ServerMsg::Connected { .. });
    send(&mut ws, ClientMsg::HubJoin { channel: None }).await;
    next_where(&mut ws, "inventory", |m| matches!(m, ServerMsg::Inventory { .. })).await;
    ws
}

async fn send(ws: &mut Ws, m: ClientMsg) {
    ws.send(Message::Binary(m.encode())).await.unwrap();
}


/// Next control message (snapshots skipped) before `deadline`.
async fn next_msg_by(ws: &mut Ws, what: &str, deadline: tokio::time::Instant) -> ServerMsg {
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next()).await.unwrap_or_else(|_| panic!("no {what} in time")).unwrap().unwrap();
        match msg {
            Message::Binary(b) if Snapshot::decode(&b).is_some() => continue,
            Message::Binary(b) => match ServerMsg::decode(&b) {
                Some(m) => return m,
                None => continue,
            },
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

async fn next_msg(ws: &mut Ws) -> ServerMsg {
    next_msg_by(ws, "message", tokio::time::Instant::now() + Duration::from_secs(5)).await
}

/// Next control message matching `pred` within 5 s (others are skipped).
async fn next_where(ws: &mut Ws, what: &str, pred: impl Fn(&ServerMsg) -> bool) -> ServerMsg {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let m = next_msg_by(ws, what, deadline).await;
        if pred(&m) {
            return m;
        }
    }
}





async fn inventory(ws: &mut Ws) -> (u64, Vec<(String, u32)>) {
    match next_where(ws, "inventory", |m| matches!(m, ServerMsg::Inventory { .. })).await {
        ServerMsg::Inventory { zeny, items, .. } => (zeny, items),
        _ => unreachable!(),
    }
}

/// The stored bag of a character.
async fn stored(state: &AppState, id: u32) -> Bag {
    let r = CharacterRepo::find_by_id(&state.pool, id).await.unwrap().unwrap();
    r.bag(Class::Mage)
}


/// A second mage on the same account (not selected). Its id.
async fn alt(addr: SocketAddr, token: &str, name: &str) -> u32 {
    let ch: Value = reqwest::Client::new()
        .post(format!("http://{addr}/api/characters"))
        .bearer_auth(token)
        .json(&json!({ "name": name, "archetype": "mage" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    ch["id"].as_u64().unwrap() as u32
}

async fn select(addr: SocketAddr, token: &str, id: u32) {
    let resp = reqwest::Client::new().post(format!("http://{addr}/api/characters/{id}/select")).bearer_auth(token).send().await.unwrap();
    assert!(resp.status().is_success());
}

async fn storage(ws: &mut Ws) -> (u64, Vec<(String, u32)>) {
    match next_where(ws, "storage or error", |m| matches!(m, ServerMsg::Storage { .. } | ServerMsg::HubError { .. })).await {
        ServerMsg::Storage { zeny, items, .. } => (zeny, items),
        ServerMsg::HubError { code, .. } => panic!("storage refused: {code}"),
        _ => unreachable!(),
    }
}

async fn refused(ws: &mut Ws) -> String {
    match next_where(ws, "storage or error", |m| matches!(m, ServerMsg::Storage { .. } | ServerMsg::HubError { .. })).await {
        ServerMsg::HubError { code, .. } => code,
        m => panic!("expected a refusal, got {m:?}"),
    }
}

fn mv(dir: StorageDir, items: &[(&str, u32)], zeny: u64) -> ClientMsg {
    ClientMsg::StorageMove { dir, items: items.iter().map(|(k, n)| (k.to_string(), *n)).collect(), zeny }
}

#[tokio::test]
async fn storage_moves_between_characters_of_an_account_and_stores_at_once() {
    let (addr, state) = start().await;
    let (user, token, a) = player(addr, "keeper").await;
    let b = alt(addr, &token, "keeper_alt").await;
    let mut ws = connect(addr, &token).await;
    send(&mut ws, ClientMsg::StorageOpen).await;
    assert_eq!(storage(&mut ws).await, (0, Vec::new()), "a new account's storage is empty");

    // A deposits three potions and 40 zeny: both rows change in one go.
    send(&mut ws, mv(StorageDir::Put, &[("red_potion", 3)], 40)).await;
    assert_eq!(storage(&mut ws).await, (40, vec![("red_potion".into(), 3)]));
    let (zeny, items) = inventory(&mut ws).await;
    assert_eq!((zeny, items), (60, vec![("red_potion".into(), 2)]));
    let bag = stored(&state, a).await;
    assert_eq!((bag.zeny, bag.count("red_potion")), (60, 2), "the bag is stored with the move, not later");
    let vault = StorageRepo::load(&state.pool, user).await.unwrap();
    assert_eq!((vault.zeny, vault.count("red_potion")), (40, 3));

    // All or nothing: more than stored moves nothing.
    send(&mut ws, mv(StorageDir::Take, &[("red_potion", 1), ("red_potion", 5)], 0)).await;
    assert_eq!(refused(&mut ws).await, "not_stored");
    send(&mut ws, mv(StorageDir::Put, &[], 1_000)).await;
    assert_eq!(refused(&mut ws).await, "not_enough_zeny");
    assert_eq!(StorageRepo::load(&state.pool, user).await.unwrap().count("red_potion"), 3);

    // The other character takes them out.
    drop(ws);
    tokio::time::sleep(Duration::from_millis(200)).await;
    select(addr, &token, b).await;
    let mut ws = connect(addr, &token).await;
    send(&mut ws, mv(StorageDir::Take, &[("red_potion", 3)], 40)).await;
    assert_eq!(storage(&mut ws).await, (0, Vec::new()));
    let bag = stored(&state, b).await;
    assert_eq!((bag.zeny, bag.count("red_potion")), (140, 8));
    let bag = stored(&state, a).await;
    assert_eq!((bag.zeny, bag.count("red_potion")), (60, 2), "A keeps what it kept");
}

#[tokio::test]
async fn a_stale_revision_is_refused_by_storage_and_by_the_flusher() {
    let (addr, state) = start().await;
    let (user, token, a) = player(addr, "fence").await;
    let mut ws = connect(addr, &token).await;
    send(&mut ws, mv(StorageDir::Put, &[("red_potion", 1)], 0)).await;
    storage(&mut ws).await;
    let rec = CharacterRepo::find_by_id(&state.pool, a).await.unwrap().unwrap();
    let bag = rec.bag(Class::Mage);
    // A memory loaded before that move (one revision behind) may not move
    // anything: its bag still holds the potion that went to storage.
    let stale = StorageRepo::transfer(&state.pool, user, a, rec.rev - 1, &bag, Dir::Put, &[("red_potion".into(), 1)], 0).await;
    assert!(matches!(stale, Err(game_server::db::StorageFail::Stale)), "{stale:?}");
    // Nor may the flusher write it.
    let session = state.session_registry.get(user).unwrap();
    {
        let mut g = session.character.write().await;
        let ch = g.as_mut().unwrap();
        ch.rev -= 1;
        ch.add_exp(1);
        assert!(!StateFlusher::flush_character(&state.pool, user, ch).await.unwrap());
        assert!(ch.fenced && !ch.is_dirty);
    }
    let after = CharacterRepo::find_by_id(&state.pool, a).await.unwrap().unwrap();
    assert_eq!(after.rev, rec.rev, "nothing written");
    // The fenced session moves nothing more.
    send(&mut ws, mv(StorageDir::Take, &[("red_potion", 1)], 0)).await;
    assert_eq!(refused(&mut ws).await, "no_character");
}
