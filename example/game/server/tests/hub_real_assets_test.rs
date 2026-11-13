//! Runs the hub simulation on the real baked maps (`example/assets/dist`).
//! The asset submodule is private and absent in public CI, so every test
//! here skips (passes with a note) when the pack is missing.

use game_server::world::hub::{HubEvent, HubInstance, HubSimConfig, HubWorld, MapIdx, PlayerProfile, PortalLink};
use game_server::world::map::MapData;
use game_server::world::{CameraConstraint, ServerMsg};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

fn dist() -> Option<PathBuf> {
    let dir = std::env::var("GAME_ASSETS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist"));
    if dir.join("server/locations/izlude/walkability.bin").exists() {
        Some(dir)
    } else {
        eprintln!("SKIP: asset pack not found at {}", dir.display());
        None
    }
}

fn load_world() -> Option<HubWorld> {
    let dir = dist()?;
    Some(HubWorld::load(&dir, &["izlude".into(), "izlude_in".into()], "izlude").expect("hub maps load"))
}

/// 4-connected BFS from a position to any cell inside `goal`; returns cell
/// centres to walk through.
fn bfs(map: &MapData, from: (f32, f32), goal: impl Fn(u32, u32) -> bool) -> Option<Vec<(f32, f32)>> {
    let (w, h) = (map.walk.width() as i32, map.walk.height() as i32);
    let start = (from.0.floor() as i32, from.1.floor() as i32);
    let mut prev = vec![u32::MAX; (w * h) as usize];
    let idx = |x: i32, y: i32| (y * w + x) as usize;
    let mut q = VecDeque::from([start]);
    prev[idx(start.0, start.1)] = idx(start.0, start.1) as u32;
    while let Some((x, y)) = q.pop_front() {
        if goal(x as u32, y as u32) {
            let mut path = vec![(x as f32 + 0.5, y as f32 + 0.5)];
            let mut cur = idx(x, y);
            while prev[cur] as usize != cur {
                cur = prev[cur] as usize;
                path.push(((cur as i32 % w) as f32 + 0.5, (cur as i32 / w) as f32 + 0.5));
            }
            path.reverse();
            return Some(path);
        }
        for (nx, ny) in [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)] {
            if map.walk.is_walkable_cell(nx, ny) && prev[idx(nx, ny)] == u32::MAX {
                prev[idx(nx, ny)] = idx(x, y) as u32;
                q.push_back((nx, ny));
            }
        }
    }
    None
}

/// Steers player `user` along `path` with intents only, the way a client
/// would. Stops early when the player changes map or leaves the instance.
/// Returns the events.
fn follow(hub: &mut HubInstance, user: u32, seq: &mut u32, path: &[(f32, f32)]) -> Vec<(u32, HubEvent)> {
    let mut out = Vec::new();
    let start_map = hub.position_of(user).unwrap().0.to_string();
    for &(tx, ty) in path {
        for _ in 0..40 {
            let Some((map, x, y)) = hub.position_of(user) else { return out };
            if map != start_map {
                return out;
            }
            let (dx, dy) = (tx - x, ty - y);
            if dx.abs() < 0.2 && dy.abs() < 0.2 {
                break;
            }
            *seq += 1;
            hub.input(user, *seq, dx, dy);
            hub.tick(&mut out);
        }
    }
    // Stand still a moment in case the goal was reached on the last step.
    for _ in 0..3 {
        hub.tick(&mut out);
    }
    out
}

fn map_enters(out: &[(u32, HubEvent)]) -> Vec<ServerMsg> {
    out.iter()
        .filter_map(|(_, e)| match e {
            HubEvent::Control(m @ ServerMsg::MapEnter { .. }) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn real_maps_load_and_portals_resolve() {
    let Some(world) = load_world() else { return };
    let izlude = world.map_index("izlude").unwrap();
    let izlude_in = world.map_index("izlude_in").unwrap();
    let city = world.map(izlude);
    assert_eq!((city.walk.width(), city.walk.height()), (268, 268));
    assert!(matches!(city.manifest.camera, CameraConstraint::Free { .. }));
    assert!(matches!(world.map(izlude_in).manifest.camera, CameraConstraint::Fixed { .. }));

    // Every city portal must be reachable from the spawn.
    for p in &city.manifest.portals {
        assert!(city.is_reachable_cell(p.trigger[0], p.trigger[1]), "portal {} unreachable from spawn", p.name);
    }
    let links: Vec<_> = (0..city.manifest.portals.len()).map(|i| world.link(izlude, i).clone()).collect();
    assert!(matches!(&links[0], PortalLink::External { target_map } if target_map == "prt_fild08"));
    assert!(links[1..].iter().all(|l| matches!(l, PortalLink::Internal { map, .. } if *map == izlude_in)));

    // Heights decode within the manifest bounds.
    let b = city.manifest.height_bounds;
    let h = city.heights.height_at(city.spawn.0, city.spawn.1);
    assert!(h >= b.min && h <= b.max, "{h} not in [{}, {}]", b.min, b.max);
}

#[test]
fn walk_through_every_house_door_and_back() {
    let Some(world) = load_world() else { return };
    let world = Arc::new(world);
    let city_idx: MapIdx = world.map_index("izlude").unwrap();
    let mut hub = HubInstance::new(1, Arc::clone(&world), HubSimConfig::default());
    let mut out = Vec::new();
    hub.join(
        PlayerProfile { user_id: 1, character_id: 1, name: "walker".into(), archetype: "novice".into(), look: game_server::world::protocol::Look::default() },
        Uuid::nil(),
        None,
        0,
        &mut out,
    );
    let mut seq = 0;

    let city = world.map(city_idx);
    for (i, door) in city.manifest.portals.iter().enumerate() {
        let PortalLink::Internal { map: inside_idx, arrival } = world.link(city_idx, i).clone() else {
            continue;
        };
        // City → house.
        let (_, x, y) = hub.position_of(1).unwrap();
        let path = bfs(city, (x, y), |cx, cy| door.contains(cx as f32 + 0.5, cy as f32 + 0.5))
            .unwrap_or_else(|| panic!("no path to {}", door.name));
        let events = follow(&mut hub, 1, &mut seq, &path);
        let (map, ax, ay) = hub.position_of(1).unwrap();
        assert_eq!(map, "izlude_in", "entered via {}", door.name);
        assert_eq!((ax, ay), arrival);
        match &map_enters(&events)[..] {
            [ServerMsg::MapEnter { map, via_portal, .. }] => {
                assert!(matches!(map.camera, CameraConstraint::Fixed { .. }));
                assert_eq!(via_portal.as_deref(), Some(door.name.as_str()));
            }
            other => panic!("expected one map_enter, got {other:?}"),
        }

        // Idle on the arrival point: the guard must hold us inside.
        for _ in 0..40 {
            hub.tick(&mut out);
        }
        assert_eq!(hub.position_of(1).unwrap().0, "izlude_in");

        // House → city through the nearest exit leading back to the city.
        let house = world.map(inside_idx);
        let (exit_idx, exit) = house
            .manifest
            .portals
            .iter()
            .enumerate()
            .filter(|(_, p)| p.target_map == "izlude")
            .min_by(|(_, a), (_, b)| {
                let d = |p: &game_server::world::map::PortalDef| {
                    let (cx, cy) = p.center();
                    (cx - ax).powi(2) + (cy - ay).powi(2)
                };
                d(a).total_cmp(&d(b))
            })
            .expect("house has an exit");
        // Leave the trigger first, then walk back in.
        let away = bfs(house, (ax, ay), |cx, cy| {
            let (px, py) = (cx as f32 + 0.5, cy as f32 + 0.5);
            !exit.contains(px, py) && (px - ax).hypot(py - ay) > exit.radius() + 1.0
        })
        .expect("room to step away from the exit");
        follow(&mut hub, 1, &mut seq, &away);
        assert_eq!(hub.position_of(1).unwrap().0, "izlude_in", "stepping away must not trigger");
        let (_, x, y) = hub.position_of(1).unwrap();
        let back = bfs(house, (x, y), |cx, cy| exit.contains(cx as f32 + 0.5, cy as f32 + 0.5)).unwrap();
        follow(&mut hub, 1, &mut seq, &back);
        let (map, bx, by) = hub.position_of(1).unwrap();
        assert_eq!(map, "izlude", "left via {}", exit.name);
        let PortalLink::Internal { arrival: back_arrival, .. } = world.link(inside_idx, exit_idx).clone() else {
            unreachable!()
        };
        assert_eq!((bx, by), back_arrival);

        // Step off the city arrival so the next door starts clean.
        for _ in 0..40 {
            hub.tick(&mut out);
        }
    }
}

#[test]
fn field_portal_is_denied_in_the_hub() {
    let Some(world) = load_world() else { return };
    let world = Arc::new(world);
    let city_idx = world.map_index("izlude").unwrap();
    let mut hub = HubInstance::new(1, Arc::clone(&world), HubSimConfig::default());
    let mut out = Vec::new();
    hub.join(
        PlayerProfile { user_id: 1, character_id: 1, name: "walker".into(), archetype: "novice".into(), look: game_server::world::protocol::Look::default() },
        Uuid::nil(),
        None,
        0,
        &mut out,
    );
    let city = world.map(city_idx);
    let field = city.manifest.portals.iter().find(|p| p.target_map == "prt_fild08").unwrap();
    let path = bfs(city, city.spawn, |cx, cy| field.contains(cx as f32 + 0.5, cy as f32 + 0.5)).unwrap();
    let mut seq = 0;
    let events = follow(&mut hub, 1, &mut seq, &path);
    assert!(events.iter().any(|(_, e)| matches!(e, HubEvent::Control(ServerMsg::PortalDenied { target_map, .. }) if target_map == "prt_fild08")));
    assert_eq!(hub.position_of(1).unwrap().0, "izlude");
}

#[test]
fn stage_on_prt_fild08_exits_to_izlude_arrival() {
    let Some(dir) = dist() else { return };
    let hub = load_world().unwrap();
    let stage_world = Arc::new(HubWorld::load(&dir, &["prt_fild08".into()], "prt_fild08").expect("stage map loads"));
    let field = stage_world.map(stage_world.entry());
    let exit = field.manifest.portals.iter().find(|p| p.target_map == "izlude").expect("field has an exit to izlude");
    assert!(
        matches!(stage_world.link(stage_world.entry(), 0), PortalLink::External { .. }),
        "the hub map is outside the stage world"
    );
    // The hub side of the exit must be a walkable spot in izlude.
    let izlude = hub.map(hub.map_index("izlude").unwrap());
    let [ax, ay] = exit.target_pos;
    assert!(izlude.walk.is_walkable_cell(ax as i32, ay as i32), "arrival ({ax},{ay}) is walkable in izlude");

    let mut stage = HubInstance::stage(1, "poring_island_survival", Arc::clone(&stage_world), HubSimConfig::default());
    let mut out = Vec::new();
    let walker = PlayerProfile { user_id: 1, character_id: 1, name: "walker".into(), archetype: "novice".into(), look: game_server::world::protocol::Look::default() };
    stage.join(walker, Uuid::nil(), None, 0, &mut out);
    let path = bfs(field, field.spawn, |cx, cy| exit.contains(cx as f32 + 0.5, cy as f32 + 0.5))
        .expect("the exit is reachable from the stage spawn");
    let mut seq = 0;
    let events = follow(&mut stage, 1, &mut seq, &path);
    assert!(
        events.iter().any(|(_, e)| matches!(e, HubEvent::Transfer { target_map, arrival }
            if target_map == "izlude" && *arrival == (ax as f32 + 0.5, ay as f32 + 0.5))),
        "walking into the exit transfers to izlude"
    );
    assert_eq!(stage.population(), 0);
}

#[test]
fn horde_on_prt_fild08_spawns_chases_and_stays_on_walkable_ground() {
    use game_server::world::horde::catalog::MonsterCatalog;
    use game_server::world::horde::{Horde, HordeConfig};
    let Some(dir) = dist() else { return };
    let world = Arc::new(HubWorld::load(&dir, &["prt_fild08".into()], "prt_fild08").unwrap());
    let catalog = Arc::new(MonsterCatalog::load(&dir).expect("monsters.json loads"));
    assert!(catalog.kind_of("poring").is_some() && catalog.kind_of("baphomet").is_some());
    let cfg = HordeConfig { seed: Some(5), ..HordeConfig::default() };
    let mut horde = Horde::new(Arc::clone(&world), Arc::clone(&catalog), cfg);
    let map = world.map(world.entry());
    let (sx, sy) = map.spawn;
    let players = [(sx, sy), (sx + 3.0, sy + 1.0)];
    assert!(players.iter().all(|&(x, y)| map.walk.is_walkable(x, y)));

    let hz = 20;
    let mut nearest_seen = f32::INFINITY;
    for t in 0..(75 * hz) {
        horde.tick(t, hz, &players);
        for (_, _, _, x, y) in horde.monsters() {
            assert!(map.walk.is_walkable(x, y), "monster on a blocked cell at {x},{y} (tick {t})");
        }
    }
    let stats = horde.stats();
    assert!(stats.alive > 30, "director kept spawning: {stats:?}");
    assert!(stats.field_builds >= 1);
    for (_, _, kind, x, y) in horde.monsters() {
        let d = players.iter().map(|&(px, py)| (x - px).hypot(y - py)).fold(f32::INFINITY, f32::min);
        nearest_seen = nearest_seen.min(d);
        assert!(!catalog.get(kind).boss, "baphomet unlocks at 300 s");
    }
    // The first waves have reached the party (spawned outside the view, so
    // anything close walked there).
    assert!(nearest_seen < 3.0, "closest monster is {nearest_seen} cells away");
    let mut buf = bytes::BytesMut::new();
    let viewer = game_server::world::horde::PlayerView { x: sx, y: sy, light: horde.light_radius() };
    let mut view = game_server::world::horde::view::MonsterView::new(horde.capacity());
    let stats = horde.write_delta(&mut view, &viewer, 75 * hz, &mut buf);
    assert!(stats.spawns > 0 && stats.spawns as usize <= horde.len(), "{stats:?}");
}
