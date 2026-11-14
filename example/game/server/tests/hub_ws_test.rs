//! End-to-end hub flow over HTTP + WebSocket with synthetic maps.

use futures_util::{SinkExt, StreamExt};
use game_server::world::map::{
    CameraConstraint, Dimensions, HeightBounds, Heightmap, LocationKind, LocationManifest, MapData, NpcDef, PortalDef,
    WalkGrid,
};
use game_server::world::protocol::{kind, MapInfo};
use game_server::world::{ClientMsg, HubWorld, ServerMsg, Snapshot};
use game_server::world::horde::catalog::{MonsterCatalog, MonsterDef};
use game_server::{config::AppConfig, create_app_with_monsters, api::AppState};
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
            light_radius: 0.0,
            residents: Default::default(),
            spawn: None,
            return_point: None,
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
        shop: String::new(),
        x: 5,
        y: 5,
        facing: 4,
        interact_radius: 3.0,
    });
    HubWorld::new(vec![town, house], "town").unwrap()
}

/// Stage world for `poring_island_survival` (map id `prt_fild08`): a small
/// field whose west exit leads to the town at (3,3).
fn synthetic_stage() -> HubWorld {
    let walk = WalkGrid::from_ascii(&["##########", "#........#", "#........#", "#........#", "##########"]);
    let (w, h) = (walk.width(), walk.height());
    let manifest = LocationManifest {
        id: "prt_fild08".into(),
        name: "Field".into(),
        kind: LocationKind::Field,
        dimensions: Dimensions { width: w, height: h, cell_size: 0.5 },
        height_bounds: HeightBounds { min: 0.0, max: 0.0 },
        camera: CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 50.0, default_zoom: 28.0 },
        portals: vec![PortalDef { name: "to_town".into(), trigger: [1, 2, 1], target_map: "town".into(), target_pos: [3, 3] }],
        bgm: None,
        npcs: Vec::new(),
        light_radius: 0.0,
        residents: Default::default(),
        spawn: None,
        return_point: None,
    };
    HubWorld::new(vec![MapData::new(manifest, walk, Heightmap::flat(w, h)).unwrap()], "prt_fild08").unwrap()
}

async fn start() -> (SocketAddr, AppState) {
    start_with(Vec::new()).await
}

async fn start_with(stages: Vec<HubWorld>) -> (SocketAddr, AppState) {
    let poring = MonsterDef {
        id: "poring".into(),
        name: "Poring".into(),
        radius: 0.35,
        speed: 2.5,
        level: 1,
        archetype: game_server::world::combat::Archetype::Swarm,
        element: game_server::world::combat::Element::Water,
        size: game_server::world::combat::Size::Small,
        spawn_cost: 1,
        unlock_secs: 0.0,
        weight: 100,
        xp: 5,
        boss: false,
        pack: (6, 12),
        skills: Vec::new(),
        evil: false,
        drops: Vec::new(),
        hits: (0, 0),
    };
    let monsters = MonsterCatalog::new(vec![poring]).unwrap();
    let (app, state) =
        create_app_with_monsters(AppConfig::test_config().await, Some(synthetic_world()), stages, Some(monsters))
            .await
            .unwrap();
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
        .json(&json!({ "name": format!("{name}_char"), "archetype": "mage" }))
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
    assert!(matches!(next_msg(&mut ws).await, ServerMsg::Connected { .. }));
    ws
}

async fn send(ws: &mut Ws, m: ClientMsg) {
    ws.send(Message::Binary(m.encode())).await.unwrap();
}

/// Next control message, skipping snapshots.
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

/// Next control message matching `pred`; fails after 5 s overall
/// (snapshots keep arriving, so a per-frame timeout alone would never fire).
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

macro_rules! next_of {
    ($ws:expr, $variant:ident) => {
        match next_where($ws, stringify!($variant), |m| matches!(m, ServerMsg::$variant { .. })).await {
            ServerMsg::$variant { .. } => {}
            _ => unreachable!(),
        }
    };
    ($ws:expr, $variant:ident { $($field:ident),* }) => {
        match next_where($ws, stringify!($variant), |m| matches!(m, ServerMsg::$variant { .. })).await {
            ServerMsg::$variant { $($field,)* .. } => ($($field,)*),
            _ => unreachable!(),
        }
    };
}

fn input(seq: u32, dx: f32, dy: f32) -> ClientMsg {
    ClientMsg::Input { seq, dx, dy, aim: [0, 0], slots: 0 }
}

fn mission_select(public: bool) -> ClientMsg {
    ClientMsg::MissionSelect { mission: "poring_island_survival".into(), public }
}

async fn next_snapshot(ws: &mut Ws, pred: impl Fn(&Snapshot) -> bool) -> Snapshot {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(3), ws.next()).await.expect("timeout").unwrap().unwrap();
        if let Message::Binary(b) = msg
            && let Some(s) = Snapshot::decode(&b)
            && pred(&s)
        {
            return s;
        }
    }
}

#[tokio::test]
async fn join_requires_selected_character() {
    let (addr, _state) = start().await;
    let (token, _) = player(addr, "nochar", false).await;
    let mut ws = connect(addr, &token).await;
    send(&mut ws, ClientMsg::HubJoin { channel: None }).await;
    let (code,) = next_of!(&mut ws, HubError { code });
    assert_eq!(code, "no_character");

    // Ping still works next to the hub protocol.
    send(&mut ws, ClientMsg::Ping).await;
    next_of!(&mut ws, Pong);

    // A malformed known message is reported; unknown kinds and text frames
    // are not part of the protocol.
    ws.send(Message::Binary(vec![kind::LOBBY_READY, 7].into())).await.unwrap();
    assert_eq!(next_of!(&mut ws, HubError { code }).0, "bad_message");
    ws.send(Message::Binary(vec![0x7F, 1, 2].into())).await.unwrap();
    ws.send(Message::Text("{\"type\":\"ping\"}".into())).await.unwrap();
    assert_eq!(next_of!(&mut ws, HubError { code }).0, "bad_message", "the unknown kind was ignored");
}

#[tokio::test]
async fn hub_flow_join_move_portal_and_peers() {
    let (addr, state) = start().await;
    let (token_a, char_a) = player(addr, "alice", true).await;
    let (token_b, char_b) = player(addr, "bob", true).await;
    let mut a = connect(addr, &token_a).await;
    let mut b = connect(addr, &token_b).await;

    send(&mut a, ClientMsg::HubJoin { channel: None }).await;
    let ServerMsg::HubJoined { channel_name, entity_id, .. } = next_msg(&mut a).await else { panic!("hub_joined first") };
    assert_eq!(channel_name, "Hub-1");
    assert_eq!(entity_id as u64, char_a);
    let ServerMsg::MapEnter { map, x: x0, .. } = next_msg(&mut a).await else { panic!("then map_enter") };
    assert_eq!(map.id, "town");
    assert!(matches!(map.camera, CameraConstraint::Free { max_zoom: 40.0, .. }), "{:?}", map.camera);

    // Bob joins: Alice sees him appear, Bob's map_enter lists Alice.
    send(&mut b, ClientMsg::HubJoin { channel: Some(1) }).await;
    let (players,) = next_of!(&mut b, MapEnter { players });
    assert_eq!(players[0].entity_id as u64, char_a);
    let (player,) = next_of!(&mut a, PlayerAppeared { player });
    assert_eq!(player.entity_id as u64, char_b);

    send(&mut a, ClientMsg::HubChannels).await;
    let (channels,) = next_of!(&mut a, HubChannels { channels });
    assert_eq!(channels[0].population, 2);

    // Alice walks east; the snapshot acks her input and shows her moving.
    for seq in 1..=3u32 {
        send(&mut a, input(seq, 1.0, 0.0)).await;
    }
    let snap = next_snapshot(&mut a, |s| s.ack_seq == 3).await;
    let me = snap.entities.iter().find(|e| e.0 == char_a as u32).unwrap();
    assert!(me.1 > x0, "moved east: {x0} -> {}", me.1);
    assert_eq!(snap.entities.len(), 2);

    // Keep walking into the door: fixed indoor camera arrives over the wire.
    let mut seq = 3u32;
    let (indoor, via, epoch): (MapInfo, Option<String>, u16) = loop {
        seq += 1;
        send(&mut a, input(seq, 1.0, 0.0)).await;
        let msg = tokio::time::timeout(Duration::from_secs(3), a.next()).await.unwrap().unwrap().unwrap();
        if let Message::Binary(b) = msg
            && let Some(ServerMsg::MapEnter { map, via_portal, map_epoch, .. }) = ServerMsg::decode(&b)
        {
            break (map, via_portal, map_epoch);
        }
        assert!(seq < 500, "never reached the door");
    };
    assert_eq!(indoor.id, "house");
    assert_eq!(via.as_deref(), Some("to_house"));
    assert_eq!(epoch, 1);
    assert_eq!(indoor.camera, CameraConstraint::Fixed { yaw: 0.0, pitch: -45.0, zoom: 18.0 });
    assert_eq!(next_of!(&mut b, PlayerVanished { entity_id }).0 as u64, char_a);

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
    send(&mut ws1, ClientMsg::HubJoin { channel: None }).await;
    next_of!(&mut ws1, MapEnter);

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
    next_of!(&mut ws1, Displaced);

    let mut ws2 = connect(addr, &token_2).await;
    send(&mut ws2, ClientMsg::HubJoin { channel: None }).await;
    next_of!(&mut ws2, MapEnter);
    // Snapshots keep flowing to the new socket after the old one is gone.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let snap = next_snapshot(&mut ws2, |_| true).await;
    assert_eq!(snap.entities.len(), 1);
    assert_eq!(state.hub.as_ref().unwrap().channels()[0].population, 1);
}

#[tokio::test]
async fn binary_input_and_mission_board() {
    let (addr, _state) = start().await;
    let (token, char_id) = player(addr, "dave", true).await;
    let mut ws = connect(addr, &token).await;

    send(&mut ws, ClientMsg::HubJoin { channel: None }).await;
    let (x0,) = next_of!(&mut ws, MapEnter { x });

    // Raw input frame: u8 kind, u32 seq, i16 dx, i16 dy.
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

    send(&mut ws, ClientMsg::NpcInteract { npc: "board".into() }).await;
    let (npc_kind, missions) = next_of!(&mut ws, NpcDialog { kind, missions });
    assert_eq!(npc_kind, "mission_board");
    assert_eq!(missions[0].id, "poring_island_survival");

    // This server has no stage maps loaded.
    send(&mut ws, mission_select(true)).await;
    assert_eq!(next_of!(&mut ws, LobbyError { code }).0, "mission_unavailable");
}

#[tokio::test]
async fn lobby_ready_check_stage_launch_and_return() {
    let (addr, state) = start_with(vec![synthetic_stage()]).await;
    let (ta, a_id) = player(addr, "erin", true).await;
    let (tb, b_id) = player(addr, "finn", true).await;
    let mut a = connect(addr, &ta).await;
    let mut b = connect(addr, &tb).await;
    for ws in [&mut a, &mut b] {
        send(ws, ClientMsg::HubJoin { channel: None }).await;
        next_of!(ws, MapEnter);
    }

    // Lobby messages only from the hub, and missions only at a board.
    send(&mut a, ClientMsg::LobbyReady { ready: true }).await;
    assert_eq!(next_of!(&mut a, LobbyError { code }).0, "not_in_lobby");

    // Both pick the public mission: they land in one lobby, erin leads.
    send(&mut a, mission_select(true)).await;
    assert_eq!(next_of!(&mut a, LobbyState { lobby }).0.members.len(), 1);
    send(&mut b, mission_select(true)).await;
    let (lobby,) = next_of!(&mut b, LobbyState { lobby });
    assert_eq!(lobby.members.len(), 2);
    assert_eq!(lobby.members[0].entity_id as u64, a_id);
    assert!(lobby.members[0].leader);

    // Ready check: finn is not ready yet.
    send(&mut a, ClientMsg::LobbyStart).await;
    assert_eq!(next_of!(&mut a, LobbyError { code }).0, "not_ready");
    send(&mut b, ClientMsg::LobbyReady { ready: true }).await;
    loop {
        let (l,) = next_of!(&mut a, LobbyState { lobby });
        if l.members[1].ready {
            break;
        }
    }
    send(&mut a, ClientMsg::LobbyStart).await;

    // Both move to the same stage; epochs keep rising across instances.
    let mut stage_ids = Vec::new();
    for ws in [&mut a, &mut b] {
        assert_eq!(next_of!(ws, LobbyLeft { reason }).0, "launched");
        let (stage_id, mission, monsters, light_radius) =
            next_of!(ws, StageJoined { stage_id, mission, monsters, light_radius });
        assert_eq!(mission, "poring_island_survival");
        assert_eq!(monsters, ["poring"], "kind index → id table");
        assert!(light_radius > 0.0, "the client draws the light");
        stage_ids.push(stage_id);
        let (map, map_epoch) = next_of!(ws, MapEnter { map, map_epoch });
        assert_eq!(map.id, "prt_fild08");
        assert_eq!(map.kind, LocationKind::Field);
        assert!(map_epoch >= 1, "epoch continues after the hub's 0");
    }
    assert_eq!(stage_ids[0], stage_ids[1]);
    let lobbies = state.lobbies.as_ref().unwrap();
    assert_eq!(lobbies.stages().active(), 1);
    assert_eq!(state.hub.as_ref().unwrap().channels()[0].population, 0, "the party left the hub");
    // Stage snapshots carry the monster section (empty here: the synthetic
    // field has no monsters yet); the first one is a keyframe.
    let snap = next_snapshot(&mut a, |s| s.entities.len() == 2).await;
    assert!(snap.entities.iter().any(|e| e.0 == b_id as u32));
    assert!(snap.monsters.is_some(), "stage snapshot kind");
    // A resync request is accepted (the next delta is a keyframe).
    send(&mut a, ClientMsg::Resync).await;
    next_snapshot(&mut a, |s| s.monsters.as_ref().is_some_and(|m| m.baseline == 0)).await;

    // finn walks west into the exit and returns to the town at (3,3).
    // Paced like a client (one input per tick), so the held direction never
    // times out on the way.
    let mut seq = 0u32;
    let channel = loop {
        seq += 1;
        assert!(seq < 100, "never reached the exit");
        send(&mut b, input(seq, -1.0, 0.0)).await;
        if let Ok(Some(Ok(Message::Binary(f)))) = tokio::time::timeout(Duration::from_millis(50), b.next()).await
            && let Some(ServerMsg::HubJoined { channel, .. }) = ServerMsg::decode(&f)
        {
            break channel;
        }
    };
    assert_eq!(channel, 1, "back to the channel they came from");
    let (map, x, y) = next_of!(&mut b, MapEnter { map, x, y });
    assert_eq!(map.id, "town");
    // Back from a mission: scattered around the town's return point (no
    // `returnPoint` here: the map's spawn), not the exit portal's arrival.
    let world = synthetic_world();
    let spawn = world.map(world.entry()).spawn;
    assert!((x - spawn.0).abs() <= 3.5 && (y - spawn.1).abs() <= 3.5, "({x}, {y}) near {spawn:?}");
    assert_eq!(next_of!(&mut a, PlayerVanished { entity_id }).0 as u64, b_id);

    // erin abandons the stage from the panel: the stage ends.
    send(&mut a, ClientMsg::StageLeave).await;
    next_of!(&mut a, HubJoined);
    assert_eq!(next_of!(&mut a, MapEnter { map }).0.id, "town");
    assert_eq!(lobbies.stages().active(), 0, "the stage ended with its last player");
    assert_eq!(state.hub.as_ref().unwrap().channels()[0].population, 2);
}

#[tokio::test]
async fn private_lobby_by_code_and_disconnect_promotes() {
    let (addr, state) = start_with(vec![synthetic_stage()]).await;
    let (ta, _) = player(addr, "gwen", true).await;
    let (tb, b_id) = player(addr, "hugo", true).await;
    let mut a = connect(addr, &ta).await;
    let mut b = connect(addr, &tb).await;
    for ws in [&mut a, &mut b] {
        send(ws, ClientMsg::HubJoin { channel: None }).await;
        next_of!(ws, MapEnter);
    }
    send(&mut a, mission_select(false)).await;
    let (lobby,) = next_of!(&mut a, LobbyState { lobby });
    assert!(!lobby.public);

    // Public matchmaking does not find it; the code does.
    send(&mut b, ClientMsg::LobbyJoin { code: "NOPE1".into() }).await;
    assert_eq!(next_of!(&mut b, LobbyError { code }).0, "no_such_lobby");
    send(&mut b, ClientMsg::LobbyJoin { code: lobby.code.to_lowercase() }).await;
    assert_eq!(next_of!(&mut b, LobbyState { lobby }).0.members.len(), 2);

    // The leader's socket drops: hugo leads the lobby now.
    a.close(None).await.unwrap();
    let (lobby,) = next_of!(&mut b, LobbyState { lobby });
    assert_eq!(lobby.members.len(), 1);
    assert_eq!((lobby.members[0].entity_id as u64, lobby.members[0].leader), (b_id, true));
    send(&mut b, ClientMsg::LobbyLeave).await;
    assert_eq!(next_of!(&mut b, LobbyLeft { reason }).0, "left");
    assert_eq!(state.lobbies.as_ref().unwrap().open_lobbies(), 0);
}

/// Two sockets with the same access token (a reconnect over a half-dead
/// connection): the stale socket's cleanup must not remove the new one from
/// the hub or its lobby.
#[tokio::test]
async fn stale_socket_with_same_token_does_not_evict_successor() {
    let (addr, state) = start_with(vec![synthetic_stage()]).await;
    let (token, _) = player(addr, "ivy", true).await;
    let mut old = connect(addr, &token).await;
    send(&mut old, ClientMsg::HubJoin { channel: None }).await;
    next_of!(&mut old, MapEnter);

    let mut new = connect(addr, &token).await;
    send(&mut new, ClientMsg::HubJoin { channel: None }).await;
    next_of!(&mut new, MapEnter);
    send(&mut new, mission_select(false)).await;
    next_of!(&mut new, LobbyState);

    old.close(None).await.unwrap();
    drop(old);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(state.hub.as_ref().unwrap().channels()[0].population, 1, "successor still in the hub");
    assert_eq!(state.lobbies.as_ref().unwrap().open_lobbies(), 1, "successor's lobby survives");
    // And the new socket still gets snapshots.
    next_snapshot(&mut new, |_| true).await;
}

/// Starts a private mission alone; returns the stage id.
async fn solo_stage(ws: &mut Ws) -> u32 {
    send(ws, ClientMsg::HubJoin { channel: None }).await;
    next_of!(ws, MapEnter);
    send(ws, mission_select(false)).await;
    next_of!(ws, LobbyState);
    send(ws, ClientMsg::LobbyStart).await;
    let (stage_id,) = next_of!(ws, StageJoined { stage_id });
    next_of!(ws, MapEnter);
    stage_id
}

async fn until(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..100 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{what}");
}

/// A stage socket drops: the fighter waits link-dead; the next socket is
/// offered the stage on `hub_join` and re-attaches to it; a stale socket
/// still holding the slot is closed with 4009 when a newer one takes over.
#[tokio::test]
async fn a_dropped_stage_socket_comes_back_to_its_fighter() {
    let (addr, state) = start_with(vec![synthetic_stage()]).await;
    let (token, char_id) = player(addr, "jade", true).await;
    let mut ws = connect(addr, &token).await;
    let stage_id = solo_stage(&mut ws).await;
    let stages = state.lobbies.as_ref().unwrap().stages();

    ws.close(None).await.unwrap();
    drop(ws);
    until("link-dead", || stages.summary().first().is_some_and(|s| s.link_dead == 1)).await;
    assert_eq!(stages.active(), 1, "the stage waits for them");

    // A fresh start asks first.
    let mut ws = connect(addr, &token).await;
    send(&mut ws, ClientMsg::HubJoin { channel: None }).await;
    let (sid, left_ms) = next_of!(&mut ws, Resumable { stage_id, left_ms });
    assert_eq!(sid, stage_id);
    assert!(left_ms > 50_000 && left_ms <= 60_000, "{left_ms}");
    send(&mut ws, ClientMsg::StageResume { accept: true }).await;
    let (sid, entity_id) = next_of!(&mut ws, StageJoined { stage_id, entity_id });
    assert_eq!((sid, entity_id as u64), (stage_id, char_id));
    assert_eq!(next_of!(&mut ws, MapEnter { map }).0.id, "prt_fild08");
    next_of!(&mut ws, Inventory);
    assert_eq!(stages.summary()[0].link_dead, 0);

    // A second socket (a reconnect while the first looks alive) takes over;
    // the first is told to stop.
    let mut ws2 = connect(addr, &token).await;
    send(&mut ws2, ClientMsg::StageResume { accept: true }).await;
    assert_eq!(next_of!(&mut ws2, StageJoined { stage_id }).0, stage_id);
    let code = loop {
        match tokio::time::timeout(Duration::from_secs(3), ws.next()).await.expect("closed in time") {
            Some(Ok(Message::Close(Some(f)))) => break u16::from(f.code),
            Some(Ok(_)) => continue,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(code, 4009);
    drop(ws);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(stages.summary()[0].link_dead, 0, "the stale socket's cleanup left the successor alone");
    next_snapshot(&mut ws2, |s| s.entities.len() == 1).await;
}

/// Declining "Return to battle" forfeits the stage and lands in town.
#[tokio::test]
async fn declining_the_return_forfeits_the_stage() {
    let (addr, state) = start_with(vec![synthetic_stage()]).await;
    let (token, _) = player(addr, "kent", true).await;
    let mut ws = connect(addr, &token).await;
    solo_stage(&mut ws).await;
    let stages = state.lobbies.as_ref().unwrap().stages();
    ws.close(None).await.unwrap();
    drop(ws);
    until("link-dead", || stages.summary().first().is_some_and(|s| s.link_dead == 1)).await;

    let mut ws = connect(addr, &token).await;
    send(&mut ws, ClientMsg::HubJoin { channel: None }).await;
    next_of!(&mut ws, Resumable);
    send(&mut ws, ClientMsg::StageResume { accept: false }).await;
    next_of!(&mut ws, HubJoined);
    assert_eq!(next_of!(&mut ws, MapEnter { map }).0.id, "town");
    assert_eq!(stages.active(), 0, "forfeited: the stage ended at once");
    // Nothing to resume any more: a late accept lands in town too.
    let mut again = connect(addr, &token).await;
    send(&mut again, ClientMsg::StageResume { accept: true }).await;
    assert_eq!(next_of!(&mut again, LobbyError { code }).0, "stage_gone");
}

/// A short drop in town: back on the same spot and channel.
#[tokio::test]
async fn a_town_reconnect_lands_on_the_same_spot() {
    let (addr, _state) = start().await;
    let (token, _) = player(addr, "lena", true).await;
    let mut ws = connect(addr, &token).await;
    send(&mut ws, ClientMsg::HubJoin { channel: None }).await;
    let (channel,) = next_of!(&mut ws, HubJoined { channel });
    let (x0, y0) = next_of!(&mut ws, MapEnter { x, y });
    for seq in 1..=6u32 {
        send(&mut ws, input(seq, 0.0, 1.0)).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let snap = next_snapshot(&mut ws, |s| s.ack_seq == 6).await;
    let (_, x1, y1, _) = snap.entities[0];
    assert!(y1 > y0 + 0.5, "moved: {y0} -> {y1}");
    ws.close(None).await.unwrap();
    drop(ws);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut ws = connect(addr, &token).await;
    send(&mut ws, ClientMsg::HubJoin { channel: None }).await;
    assert_eq!(next_of!(&mut ws, HubJoined { channel }).0, channel);
    let (x, y) = next_of!(&mut ws, MapEnter { x, y });
    assert!((x - x1).abs() < 0.6 && (y - y1).abs() < 0.6, "({x}, {y}) is not where they stood ({x1}, {y1}); spawn ({x0}, {y0})");
}
