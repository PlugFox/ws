pub mod admin;
pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod feedback;
pub mod geo;
pub mod session;
pub mod version;
pub mod world;

use api::{build_router, AppState};
use auth::JwtManager;
use axum::Router;
use config::AppConfig;
use db::init_db;
use geo::GeolocationService;
use session::{SessionRegistry, StateFlusher};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};
use world::{missions, HubDirectory, HubWorld, LobbyDirectory, StageDirectory};

/// Initializes application components and returns the configured Axum router and AppState.
///
/// Loads the hub maps from `config.assets_dir`. A missing or invalid asset
/// pack is not fatal: auth and characters keep working and `hub_join`
/// answers `hub_unavailable`.
pub async fn create_app(config: AppConfig) -> Result<(Router, AppState), Box<dyn std::error::Error + Send + Sync>> {
    if config.hub.sim.missions_anywhere {
        warn!("GAME_DEV test hooks are on: missions anywhere, new characters at level {:?}", config.dev_level);
    }
    let world = match &config.assets_dir {
        Some(dir) => match HubWorld::load(dir, &config.hub_maps, &config.hub_entry) {
            Ok(world) => {
                info!("Loaded hub maps {:?} from {}", config.hub_maps, dir.display());
                let has_board = (0..world.map_count()).any(|i| {
                    world.map(i as world::hub::MapIdx).manifest.npcs.iter().any(|n| world::hub::gives_missions(&n.kind))
                });
                if !has_board {
                    warn!(
                        "No NPC with kind \"{}\" or \"{}\" in the hub maps: missions cannot be picked (check the recipe's npcs)",
                        world::hub::KAFRA,
                        world::hub::MISSION_BOARD
                    );
                }
                Some(world)
            }
            Err(err) => {
                warn!("Hub disabled: cannot load maps from {}: {err}", dir.display());
                None
            }
        },
        None => None,
    };
    // One stage world per distinct mission map; a map that fails to load
    // only disables the missions played on it.
    let mut stages = Vec::new();
    if let (Some(dir), Some(_)) = (&config.assets_dir, &world) {
        let mut maps: Vec<&str> = missions::MISSIONS.iter().map(|m| m.map).collect();
        maps.sort_unstable();
        maps.dedup();
        for id in maps {
            match HubWorld::load(dir, &[id.to_string()], id) {
                Ok(w) => {
                    info!("Loaded stage map {id}");
                    stages.push(w);
                }
                Err(err) => warn!("Missions on {id} disabled: {err}"),
            }
        }
    }
    let monsters = match (&config.assets_dir, stages.is_empty()) {
        (Some(dir), false) => match world::horde::catalog::MonsterCatalog::load(dir) {
            Ok(c) => {
                info!("Loaded {} monster kinds", c.defs().len());
                Some(c)
            }
            Err(err) => {
                warn!("Stages run without monsters: {err}");
                None
            }
        },
        _ => None,
    };
    let skills = match &config.assets_dir {
        Some(dir) => match world::combat::SkillBook::load(dir) {
            Ok(b) => {
                info!("Loaded {} skills", b.len());
                b
            }
            Err(err) => {
                warn!("No skills: {err}");
                world::combat::SkillBook::default()
            }
        },
        None => world::combat::SkillBook::default(),
    };
    create_app_with_skills(config, world, stages, monsters, skills).await
}

/// Same as [`create_app`] with an already-built hub world and no stage maps
/// (tests use synthetic maps so they do not depend on the private asset
/// submodule).
pub async fn create_app_with_world(
    config: AppConfig,
    world: Option<HubWorld>,
) -> Result<(Router, AppState), Box<dyn std::error::Error + Send + Sync>> {
    create_app_with_worlds(config, world, Vec::new()).await
}

/// Same as [`create_app`] with already-built hub and stage worlds (stages
/// without monsters).
pub async fn create_app_with_worlds(
    config: AppConfig,
    world: Option<HubWorld>,
    stages: Vec<HubWorld>,
) -> Result<(Router, AppState), Box<dyn std::error::Error + Send + Sync>> {
    create_app_with_monsters(config, world, stages, None).await
}

/// Same as [`create_app`] with already-built worlds and monster catalog.
pub async fn create_app_with_monsters(
    config: AppConfig,
    world: Option<HubWorld>,
    stages: Vec<HubWorld>,
    monsters: Option<world::horde::catalog::MonsterCatalog>,
) -> Result<(Router, AppState), Box<dyn std::error::Error + Send + Sync>> {
    create_app_with_skills(config, world, stages, monsters, world::combat::SkillBook::default()).await
}

/// Same as [`create_app_with_monsters`] with a skill table.
pub async fn create_app_with_skills(
    config: AppConfig,
    world: Option<HubWorld>,
    stages: Vec<HubWorld>,
    monsters: Option<world::horde::catalog::MonsterCatalog>,
    skills: world::combat::SkillBook,
) -> Result<(Router, AppState), Box<dyn std::error::Error + Send + Sync>> {
    let skills = Arc::new(skills);
    match &config.admin_token {
        Some(t) if t.len() < 16 => warn!("ADMIN_TOKEN is shorter than 16 characters: use a long random token"),
        Some(_) => info!("Admin API enabled at /admin/*"),
        None => info!("Admin API disabled (set ADMIN_TOKEN to enable /admin/*)"),
    }
    let pool = init_db(&config.db).await?;

    let jwt_manager = JwtManager::new(&config.jwt_secret, config.access_token_ttl);
    let geo_service = GeolocationService::new(config.geoip_db_path.as_deref());
    let session_registry = SessionRegistry::new();

    // Start background persistence flusher worker
    let _flusher_handle = StateFlusher::spawn_periodic_worker(
        pool.clone(),
        session_registry.clone(),
        config.flush_interval,
    );

    let hub = world.map(|w| {
        // Player trades settle against the sessions' characters and the database.
        let vault: Arc<dyn world::trade::TradeVault> = Arc::new(session::trade::SessionVault::new(pool.clone(), session_registry.clone()));
        let dir = HubDirectory::with_vault(Arc::new(w), config.hub.clone(), Some(vault));
        let _reaper = dir.spawn_reaper(Duration::from_secs(10));
        dir
    });
    let lobbies = match hub.as_ref() {
        Some(_) => {
            let mut stages = StageDirectory::new(stages, config.stages.clone()).with_skills(Arc::clone(&skills));
            if let Some(m) = monsters {
                stages = stages.with_monsters(m);
            }
            let _reaper = stages.spawn_reaper(Duration::from_secs(5));
            // Parties belong to accounts and are kept in the database.
            let lobbies = LobbyDirectory::persistent(stages, pool.clone()).await?;
            let _party_reaper = lobbies.spawn_reaper(Duration::from_secs(30));
            Some(lobbies)
        }
        None => None,
    };

    let mut state = AppState::new(
        config,
        pool,
        jwt_manager,
        geo_service,
        session_registry,
        hub,
        lobbies,
    );
    state.skills = skills;

    let router = build_router(state.clone());
    info!("Game server initialized successfully.");

    Ok((router, state))
}
