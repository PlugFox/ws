use crate::auth::JwtManager;
use crate::config::AppConfig;
use crate::geo::GeolocationService;
use crate::session::SessionRegistry;
use crate::world::HubDirectory;
use sqlx::{Pool, Sqlite};

/// Shared application state injected into Axum route handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub pool: Pool<Sqlite>,
    pub jwt_manager: JwtManager,
    pub geo_service: GeolocationService,
    pub session_registry: SessionRegistry,
    /// `None` when the asset pack is missing or invalid.
    pub hub: Option<HubDirectory>,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        pool: Pool<Sqlite>,
        jwt_manager: JwtManager,
        geo_service: GeolocationService,
        session_registry: SessionRegistry,
        hub: Option<HubDirectory>,
    ) -> Self {
        Self {
            config,
            pool,
            jwt_manager,
            geo_service,
            session_registry,
            hub,
        }
    }
}
