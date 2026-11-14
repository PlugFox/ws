pub mod admin_handlers;
pub mod auth_handlers;
pub mod character_handlers;
pub mod class_handlers;
pub mod leaderboard_handlers;
pub mod middleware;
pub mod mcp;
pub mod state;
pub mod static_files;
pub mod status_handlers;
pub mod ws_handlers;

use axum::{
    http::{header, Method},
    routing::{get, patch, post},
    Router,
};
pub use state::AppState;
use tower_http::cors::{Any, CorsLayer};

pub use status_handlers::health_check;

/// Builds the complete Axum HTTP & WebSocket router.
pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT]);

    let mut router = Router::new()
        .route("/health", get(health_check))
        .route("/api/online", get(status_handlers::online))
        .route("/api/leaderboard", get(leaderboard_handlers::leaderboard))
        .route("/api/classes", get(class_handlers::classes))
        .route("/api/auth/register", post(auth_handlers::register))
        .route("/api/auth/login", post(auth_handlers::login))
        .route("/api/auth/refresh", post(auth_handlers::refresh))
        .route("/api/auth/logout", post(auth_handlers::logout))
        .route("/api/auth/me", get(auth_handlers::me))
        .route("/api/auth/contacts", patch(auth_handlers::update_contacts))
        .route(
            "/api/characters",
            get(character_handlers::list_characters).post(character_handlers::create_character),
        )
        .route(
            "/api/characters/{id}",
            get(character_handlers::get_character),
        )
        .route(
            "/api/characters/{id}/select",
            post(character_handlers::select_character),
        )
        .route("/api/ws", get(ws_handlers::ws_handler))
        // Operators only: `Authorization: Bearer $ADMIN_TOKEN`.
        .route("/admin/broadcast", post(admin_handlers::broadcast))
        .route(
            "/admin/restart-warning",
            post(admin_handlers::restart_warning)
                .get(admin_handlers::restart_status)
                .delete(admin_handlers::restart_cancel),
        )
        .route("/admin/save", post(admin_handlers::save))
        // MCP (Streamable HTTP) for coding agents, same admin token.
        .route("/mcp", post(mcp::handle).get(mcp::get_not_allowed))
        // Dynamic answers are personal (tokens, characters) unless a handler
        // says otherwise (`/api/online`, `/api/leaderboard`): `no-store`.
        .layer(axum::middleware::map_response(no_store_by_default));
    // One origin for the web client, the engine bundle and the baked pack
    // (`/assets/client/...`, `/assets/engine.bundle.js`; the client also
    // reads `server/locations/*/walkability.bin` for prediction), with
    // precompressed variants and build-versioned caching (`static_files`).
    let build = state.config.assets_version.as_deref();
    if let Some(dir) = &state.config.assets_dir {
        router = router.nest_service("/assets", static_files::pack(dir, build));
    }
    if let Some(dir) = &state.config.client_dir {
        router = router.fallback_service(static_files::client(dir, build));
    }
    router.layer(cors).with_state(state)
}

/// `Cache-Control: no-store` on a response that sets no cache policy.
async fn no_store_by_default(mut res: axum::response::Response) -> axum::response::Response {
    res.headers_mut().entry(header::CACHE_CONTROL).or_insert(header::HeaderValue::from_static("no-store"));
    res
}
