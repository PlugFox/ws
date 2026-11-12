pub mod auth_handlers;
pub mod character_handlers;
pub mod middleware;
pub mod state;
pub mod ws_handlers;

use axum::{
    http::{header, Method},
    response::Json,
    routing::{get, post},
    Router,
};
use serde_json::json;
pub use state::AppState;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

pub async fn health_check() -> Json<serde_json::Value> {
    Json(json!({
        "status": "healthy",
        "service": "game_server",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

/// Builds the complete Axum HTTP & WebSocket router.
pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT]);

    let mut router = Router::new()
        .route("/health", get(health_check))
        .route("/api/auth/register", post(auth_handlers::register))
        .route("/api/auth/login", post(auth_handlers::login))
        .route("/api/auth/refresh", post(auth_handlers::refresh))
        .route("/api/auth/logout", post(auth_handlers::logout))
        .route("/api/auth/me", get(auth_handlers::me))
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
        .route("/api/ws", get(ws_handlers::ws_handler));
    // Dev convenience: one origin for the web client, the engine bundle and
    // the baked pack (`/assets/client/...`, `/assets/engine.bundle.js`; the
    // client also reads `server/locations/*/walkability.bin` for prediction).
    if let Some(dir) = &state.config.assets_dir {
        // Revalidate on every load: baked files change in place between bakes.
        let assets = tower::ServiceBuilder::new()
            .layer(SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-cache"),
            ))
            .service(ServeDir::new(dir));
        router = router.nest_service("/assets", assets);
    }
    if let Some(dir) = &state.config.client_dir {
        router = router.fallback_service(ServeDir::new(dir));
    }
    router.layer(cors).with_state(state)
}
