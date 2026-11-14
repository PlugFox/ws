//! Player-to-player trade end to end: two accounts over HTTP + WebSocket,
//! the hub's table, the session vault and the database.

use futures_util::{SinkExt, StreamExt};
use game_server::db::character_repo::CharacterRepo;
use game_server::session::trade::SessionVault;
use game_server::world::combat::{Bag, Class};
use game_server::world::map::{
    CameraConstraint, Dimensions, HeightBounds, Heightmap, LocationKind, LocationManifest, MapData, WalkGrid,
};
use game_server::world::trade::{Deal, Offer, Party, TradeError, TradeOp, TradeVault};
use game_server::world::{ClientMsg, HubWorld, ServerMsg, Snapshot};
use game_server::{api::AppState, config::AppConfig, create_app_with_world};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

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
    let (mut ws, _) = connect_async(format!("ws://{addr}/api/ws?token={token}")).await.unwrap();
    assert!(matches!(next_msg(&mut ws).await, ServerMsg::Connected { .. }));
    send(&mut ws, ClientMsg::HubJoin { channel: None }).await;
    next_where(&mut ws, "inventory", |m| matches!(m, ServerMsg::Inventory { .. })).await;
    ws
}

async fn send(ws: &mut Ws, m: ClientMsg) {
    ws.send(Message::Binary(m.encode())).await.unwrap();
}

async fn trade(ws: &mut Ws, op: TradeOp) {
    send(ws, ClientMsg::Trade { op }).await;
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

/// The next `trade_state` flags.
async fn flags(ws: &mut Ws) -> u8 {
    match next_where(ws, "trade_state", |m| matches!(m, ServerMsg::TradeState { .. })).await {
        ServerMsg::TradeState { flags, .. } => flags,
        _ => unreachable!(),
    }
}

/// Reads `trade_state`s until the partner's offer holds `count` of `item`.
async fn until_theirs(ws: &mut Ws, item: &str, count: u32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let ServerMsg::TradeState { theirs, .. } = next_msg_by(ws, "trade_state", deadline).await
            && theirs.count(item) == count
        {
            return;
        }
    }
}

/// Reads `trade_state`s until the flags are `want` (the sockets' commands
/// reach the hub in no fixed order between players: sync on the state).
async fn until_flags(ws: &mut Ws, want: u8) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let ServerMsg::TradeState { flags, .. } = next_msg_by(ws, "trade_state", deadline).await
            && flags == want
        {
            return;
        }
    }
}

async fn closed(ws: &mut Ws) -> String {
    match next_where(ws, "trade_closed", |m| matches!(m, ServerMsg::TradeClosed { .. })).await {
        ServerMsg::TradeClosed { reason, .. } => reason,
        _ => unreachable!(),
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

/// A asks B, B accepts; both see the table.
async fn open(a: &mut Ws, b: &mut Ws, b_entity: u32) {
    trade(a, TradeOp::Request { entity_id: b_entity }).await;
    next_where(b, "trade_request", |m| matches!(m, ServerMsg::TradeRequest { .. })).await;
    trade(b, TradeOp::Accept).await;
    assert_eq!(flags(a).await, 0);
    assert_eq!(flags(b).await, 0);
}

#[tokio::test]
async fn a_trade_moves_items_and_zeny_and_stores_both_bags_at_once() {
    let (addr, state) = start().await;
    let (_, ta, ca) = player(addr, "trader_a").await;
    let (_, tb, cb) = player(addr, "trader_b").await;
    let mut a = connect(addr, &ta).await;
    let mut b = connect(addr, &tb).await;
    open(&mut a, &mut b, cb).await;

    trade(&mut a, TradeOp::Item { item: "red_potion".into(), count: 2 }).await;
    trade(&mut a, TradeOp::Zeny(40)).await;
    trade(&mut b, TradeOp::Zeny(1000)).await;
    match next_where(&mut b, "hub_error", |m| matches!(m, ServerMsg::HubError { .. })).await {
        ServerMsg::HubError { code, .. } => assert_eq!(code, "not_enough_zeny"),
        _ => unreachable!(),
    }
    trade(&mut b, TradeOp::Item { item: "red_potion".into(), count: 5 }).await;
    until_theirs(&mut a, "red_potion", 5).await;
    trade(&mut a, TradeOp::Lock).await;
    until_flags(&mut b, 0b10).await;
    trade(&mut b, TradeOp::Lock).await;
    until_flags(&mut a, 0b11).await;
    // Both confirm at once, one of them twice.
    trade(&mut a, TradeOp::Confirm).await;
    trade(&mut b, TradeOp::Confirm).await;
    trade(&mut a, TradeOp::Confirm).await;
    assert_eq!(closed(&mut a).await, "done");
    assert_eq!(closed(&mut b).await, "done");
    let (za, ia) = inventory(&mut a).await;
    let (zb, ib) = inventory(&mut b).await;
    assert_eq!((za, ia), (60, vec![("red_potion".to_string(), 8)]));
    assert_eq!((zb, ib), (140, vec![("red_potion".to_string(), 2)]));

    // Stored right away, not at the next flush.
    let (sa, sb) = (stored(&state, ca).await, stored(&state, cb).await);
    assert_eq!((sa.zeny, sa.count("red_potion"), sb.zeny, sb.count("red_potion")), (60, 8, 140, 2));
    assert_eq!(sa.weapon(), "rod", "worn gear never moves");
}

#[tokio::test]
async fn a_partner_who_disconnects_right_after_confirming_loses_nothing_and_dupes_nothing() {
    let (addr, state) = start().await;
    let (_, ta, ca) = player(addr, "leaver_a").await;
    let (_, tb, cb) = player(addr, "leaver_b").await;
    let mut a = connect(addr, &ta).await;
    let mut b = connect(addr, &tb).await;
    open(&mut a, &mut b, cb).await;
    trade(&mut a, TradeOp::Zeny(100)).await;
    trade(&mut b, TradeOp::Item { item: "red_potion".into(), count: 5 }).await;
    until_theirs(&mut a, "red_potion", 5).await;
    trade(&mut a, TradeOp::Lock).await;
    until_flags(&mut b, 0b10).await;
    trade(&mut b, TradeOp::Lock).await;
    trade(&mut b, TradeOp::Confirm).await;
    until_flags(&mut a, 0b1011).await;
    // A agrees last; B drops the moment the deal is struck, while the
    // vault settles it.
    trade(&mut a, TradeOp::Confirm).await;
    until_flags(&mut b, 0b1111).await;
    b.close(None).await.unwrap();
    drop(b);
    assert_eq!(closed(&mut a).await, "done");
    // Give B's socket time to flush on the way out.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (sa, sb) = (stored(&state, ca).await, stored(&state, cb).await);
    assert_eq!((sa.zeny + sb.zeny, sa.count("red_potion") + sb.count("red_potion")), (200, 10), "nothing made or lost");
    assert_eq!((sa.zeny, sa.count("red_potion")), (0, 10));
}

#[tokio::test]
async fn cancelling_or_walking_off_moves_nothing() {
    let (addr, state) = start().await;
    let (_, ta, ca) = player(addr, "cancel_a").await;
    let (_, tb, cb) = player(addr, "cancel_b").await;
    let mut a = connect(addr, &ta).await;
    let mut b = connect(addr, &tb).await;
    open(&mut a, &mut b, cb).await;
    trade(&mut a, TradeOp::Zeny(100)).await;
    trade(&mut a, TradeOp::Lock).await;
    until_flags(&mut b, 0b10).await;
    trade(&mut b, TradeOp::Lock).await;
    trade(&mut b, TradeOp::Cancel).await;
    assert_eq!(closed(&mut a).await, "cancelled");
    assert_eq!(closed(&mut b).await, "cancelled");
    // A late confirm finds no table.
    trade(&mut a, TradeOp::Confirm).await;
    match next_where(&mut a, "hub_error", |m| matches!(m, ServerMsg::HubError { .. })).await {
        ServerMsg::HubError { code, .. } => assert_eq!(code, "trade_none"),
        _ => unreachable!(),
    }
    // Leaving the hub mid-trade.
    open(&mut a, &mut b, cb).await;
    send(&mut b, ClientMsg::HubLeave).await;
    assert_eq!(closed(&mut a).await, "left");
    let (sa, sb) = (stored(&state, ca).await, stored(&state, cb).await);
    assert_eq!((sa.zeny, sb.zeny), (100, 100));
}

/// Two settlements of the same all-in deal racing each other: the second
/// finds the items gone. Locks are taken in account order, so they never
/// deadlock either.
#[tokio::test]
async fn the_vault_never_applies_an_all_in_deal_twice() {
    let (addr, state) = start().await;
    let (ua, ta, ca) = player(addr, "race_a").await;
    let (ub, tb, cb) = player(addr, "race_b").await;
    // Sessions must be live for the vault.
    let _a = connect(addr, &ta).await;
    let _b = connect(addr, &tb).await;
    let vault = SessionVault::new(state.pool.clone(), state.session_registry.clone());
    let deal = Deal {
        id: 1,
        sides: [
            Party { user_id: ua, character_id: ca, offer: Offer { items: vec![("red_potion".into(), 5)], zeny: 100 } },
            Party { user_id: ub, character_id: cb, offer: Offer { items: vec![], zeny: 1 } },
        ],
    };
    let mirrored = Deal { id: 2, sides: [deal.sides[1].clone(), deal.sides[0].clone()] };
    let (r1, r2, r3) = tokio::join!(vault.settle(deal.clone()), vault.settle(mirrored), vault.settle(deal.clone()));
    let oks = [&r1, &r2, &r3].iter().filter(|r| r.is_ok()).count();
    assert_eq!(oks, 1, "{r1:?} {r2:?} {r3:?}");
    for r in [r1, r2, r3] {
        if let Err(e) = r {
            assert!(matches!(e, TradeError::NotOwned | TradeError::NoZeny), "{e:?}");
        }
    }
    let (sa, sb) = (stored(&state, ca).await, stored(&state, cb).await);
    assert_eq!((sa.zeny + sb.zeny, sa.count("red_potion") + sb.count("red_potion")), (200, 10));
    assert_eq!((sa.zeny, sa.count("red_potion"), sb.zeny, sb.count("red_potion")), (1, 0, 199, 10));

    // A wrong character id (another character, a stale deal) moves nothing.
    let stale = Deal { id: 3, sides: [Party { character_id: cb + 100, ..deal.sides[0].clone() }, deal.sides[1].clone()] };
    assert_eq!(vault.settle(stale).await, Err(TradeError::Gone));
    // Nor does a deal with someone offline.
    let ghost = Deal { id: 4, sides: [Party { user_id: 99_999, ..deal.sides[0].clone() }, deal.sides[1].clone()] };
    assert_eq!(vault.settle(ghost).await, Err(TradeError::Gone));
}

/// Item instances (COMBAT.md §2.6): a refined copy trades by its instance
/// key, part of its stack splits off, the plain copies stay apart, and
/// both stored bags keep one stack per instance.
#[tokio::test]
async fn refined_stacks_split_through_a_trade_and_persist() {
    let (addr, state) = start().await;
    let (_, ta, ca) = player(addr, "instance_a").await;
    let (_, tb, cb) = player(addr, "instance_b").await;
    let mut bag = stored(&state, ca).await;
    bag.add("rod+4", 3).unwrap();
    bag.add("rod", 1).unwrap();
    CharacterRepo::set_bag(&state.pool, ca, &bag).await.unwrap();
    // Selecting again loads the stored bag into the session.
    let resp = reqwest::Client::new().post(format!("http://{addr}/api/characters/{ca}/select")).bearer_auth(&ta).send().await.unwrap();
    assert!(resp.status().is_success());
    let mut a = connect(addr, &ta).await;
    let mut b = connect(addr, &tb).await;
    open(&mut a, &mut b, cb).await;
    trade(&mut a, TradeOp::Item { item: "rod+4".into(), count: 2 }).await;
    until_theirs(&mut b, "rod+4", 2).await;
    trade(&mut a, TradeOp::Lock).await;
    until_flags(&mut b, 0b10).await;
    trade(&mut b, TradeOp::Lock).await;
    until_flags(&mut a, 0b11).await;
    trade(&mut a, TradeOp::Confirm).await;
    trade(&mut b, TradeOp::Confirm).await;
    assert_eq!(closed(&mut a).await, "done");
    assert_eq!(closed(&mut b).await, "done");
    let (_, ia) = inventory(&mut a).await;
    let (_, ib) = inventory(&mut b).await;
    assert!(ia.contains(&("rod+4".to_string(), 1)) && ia.contains(&("rod".to_string(), 1)), "{ia:?}");
    assert!(ib.contains(&("rod+4".to_string(), 2)), "{ib:?}");
    let (sa, sb) = (stored(&state, ca).await, stored(&state, cb).await);
    assert_eq!((sa.count("rod+4"), sa.count("rod"), sb.count("rod+4"), sb.count("rod")), (1, 1, 2, 0));
    assert_eq!(sb.items.iter().filter(|s| s.id == "rod").count(), 1, "one stack per instance");
}
