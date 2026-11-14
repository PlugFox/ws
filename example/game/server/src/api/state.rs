use crate::admin::Admin;
use crate::auth::JwtManager;
use crate::config::AppConfig;
use crate::geo::GeolocationService;
use crate::session::SessionRegistry;
use crate::world::combat::SkillBook;
use crate::world::{HubDirectory, LobbyDirectory};
use crate::db::Db;
use std::sync::Arc;

/// Shared application state injected into Axum route handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub pool: Db,
    pub jwt_manager: JwtManager,
    pub geo_service: GeolocationService,
    pub session_registry: SessionRegistry,
    /// `None` when the asset pack is missing or invalid.
    pub hub: Option<HubDirectory>,
    /// Mission lobbies and stage instances; `None` without a hub.
    pub lobbies: Option<LobbyDirectory>,
    /// The skill table (empty without assets): learning and casting.
    pub skills: Arc<SkillBook>,
    /// Announcements, the restart countdown, save-all (the admin API).
    pub admin: Admin,
    /// Friends lists and who is online where (`world::friends`).
    pub friends: crate::world::friends::Friends,
    /// When the server started (uptime in `/health` and the MCP status).
    pub started: std::time::Instant,
    /// `/api/online`: the online count, recounted at most every few seconds.
    pub online: super::status_handlers::OnlineCache,
    /// `/api/leaderboard`: recently served pages.
    pub leaderboard: super::leaderboard_handlers::LeaderboardCache,
}

impl AppState {
    /// Display name of a location (`last_location_id` and the like): the
    /// manifest name of a hub map, else the place of the mission played on
    /// that map.
    pub fn place_name(&self, map: &str) -> Option<String> {
        let world = self.hub.as_ref().map(|h| h.world());
        if let Some(w) = world
            && let Some(i) = w.map_index(map)
        {
            return Some(w.map(i).manifest.name.clone());
        }
        crate::world::missions::MISSIONS.iter().find(|m| m.map == map).map(|m| m.place.to_string())
    }

    pub fn new(
        config: AppConfig,
        pool: Db,
        jwt_manager: JwtManager,
        geo_service: GeolocationService,
        session_registry: SessionRegistry,
        hub: Option<HubDirectory>,
        lobbies: Option<LobbyDirectory>,
    ) -> Self {
        let admin = Admin::new(session_registry.clone(), pool.clone());
        let friends = crate::world::friends::Friends::new(pool.clone());
        Self {
            config,
            pool,
            jwt_manager,
            geo_service,
            session_registry,
            hub,
            lobbies,
            skills: Arc::new(SkillBook::default()),
            admin,
            friends,
            started: std::time::Instant::now(),
            online: Default::default(),
            leaderboard: Default::default(),
        }
    }
}
