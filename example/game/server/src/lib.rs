pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod geo;
pub mod session;
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
use world::{HubDirectory, HubWorld};

/// Initializes application components and returns the configured Axum router and AppState.
///
/// Loads the hub maps from `config.assets_dir`. A missing or invalid asset
/// pack is not fatal: auth and characters keep working and `hub_join`
/// answers `hub_unavailable`.
pub async fn create_app(config: AppConfig) -> Result<(Router, AppState), Box<dyn std::error::Error + Send + Sync>> {
    let world = match &config.assets_dir {
        Some(dir) => match HubWorld::load(dir, &config.hub_maps, &config.hub_entry) {
            Ok(world) => {
                info!("Loaded hub maps {:?} from {}", config.hub_maps, dir.display());
                Some(world)
            }
            Err(err) => {
                warn!("Hub disabled: cannot load maps from {}: {err}", dir.display());
                None
            }
        },
        None => None,
    };
    create_app_with_world(config, world).await
}

/// Same as [`create_app`] with an already-built hub world (tests use
/// synthetic maps so they do not depend on the private asset submodule).
pub async fn create_app_with_world(
    config: AppConfig,
    world: Option<HubWorld>,
) -> Result<(Router, AppState), Box<dyn std::error::Error + Send + Sync>> {
    let pool = init_db(&config.database_url).await?;

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
        let dir = HubDirectory::new(Arc::new(w), config.hub.clone());
        let _reaper = dir.spawn_reaper(Duration::from_secs(10));
        dir
    });

    let state = AppState::new(
        config,
        pool,
        jwt_manager,
        geo_service,
        session_registry,
        hub,
    );

    let router = build_router(state.clone());
    info!("Game server initialized successfully.");

    Ok((router, state))
}
