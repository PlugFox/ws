use crate::auth::JwtManager;
use crate::config::AppConfig;
use crate::geo::GeolocationService;
use crate::session::SessionRegistry;
use crate::world::combat::SkillBook;
use crate::world::{HubDirectory, LobbyDirectory};
use sqlx::{Pool, Sqlite};
use std::sync::Arc;

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
    /// Mission lobbies and stage instances; `None` without a hub.
    pub lobbies: Option<LobbyDirectory>,
    /// The skill table (empty without assets): learning and casting.
    pub skills: Arc<SkillBook>,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        pool: Pool<Sqlite>,
        jwt_manager: JwtManager,
        geo_service: GeolocationService,
        session_registry: SessionRegistry,
        hub: Option<HubDirectory>,
        lobbies: Option<LobbyDirectory>,
    ) -> Self {
        Self {
            config,
            pool,
            jwt_manager,
            geo_service,
            session_registry,
            hub,
            lobbies,
            skills: Arc::new(SkillBook::default()),
        }
    }
}
