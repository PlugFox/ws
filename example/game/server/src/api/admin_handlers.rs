//! Admin HTTP API (`/admin/*`), for operators: announcements, the restart
//! countdown, saving every session. Every route wants
//! `Authorization: Bearer $ADMIN_TOKEN`; without `ADMIN_TOKEN` the API is
//! off (503). See the server README for curl examples.

use super::state::AppState;
use axum::{
    extract::{FromRef, FromRequestParts, State},
    http::{header, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use subtle::ConstantTimeEq;
use tracing::warn;

/// Proof that the request carries the admin token.
pub struct AdminAuth;

fn reject(status: StatusCode, code: &str, error: &str) -> Response {
    (status, Json(json!({ "error": error, "code": code }))).into_response()
}

impl<S> FromRequestParts<S> for AdminAuth
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        let Some(expected) = state.config.admin_token.as_deref() else {
            return Err(reject(StatusCode::SERVICE_UNAVAILABLE, "ADMIN_DISABLED", "admin API is disabled (set ADMIN_TOKEN)"));
        };
        let given = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        // Constant time for equal lengths; the length itself is no secret.
        if given.len() != expected.len() || !bool::from(given.as_bytes().ct_eq(expected.as_bytes())) {
            warn!("Admin API: rejected request to {}", parts.uri.path());
            return Err(reject(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "missing or wrong admin token"));
        }
        Ok(AdminAuth)
    }
}

#[derive(Deserialize)]
pub struct BroadcastBody {
    pub text: String,
}

/// `POST /admin/broadcast {"text": "..."}`: an announcement to everyone
/// connected (banner and chat log). Answers how many sockets got it.
pub async fn broadcast(_: AdminAuth, State(state): State<AppState>, Json(body): Json<BroadcastBody>) -> Response {
    match state.admin.broadcast(&body.text) {
        Some(delivered) => Json(json!({ "delivered": delivered })).into_response(),
        None => reject(StatusCode::BAD_REQUEST, "EMPTY_TEXT", "text is empty"),
    }
}

#[derive(Deserialize)]
pub struct RestartBody {
    pub minutes: u32,
    /// Appended to every announcement (e.g. "New patch: bosses.").
    #[serde(default)]
    pub message: Option<String>,
}

/// `POST /admin/restart-warning {"minutes": 30, "message": "..."}`:
/// schedules the countdown (30, 15, 10, 5, 4, 3, 2, 1 minutes, then
/// "restarting now" and a save of every session), replacing a running one.
pub async fn restart_warning(_: AdminAuth, State(state): State<AppState>, Json(body): Json<RestartBody>) -> Response {
    match state.admin.schedule_restart(body.minutes, body.message.as_deref()) {
        Ok(status) => Json(json!({ "scheduled": status })).into_response(),
        Err(e) => reject(StatusCode::BAD_REQUEST, "BAD_MINUTES", e),
    }
}

/// `GET /admin/restart-warning`: the running countdown (`null` if none).
pub async fn restart_status(_: AdminAuth, State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "scheduled": state.admin.restart_status() }))
}

/// `DELETE /admin/restart-warning`: cancels the countdown (and says so to
/// everyone).
pub async fn restart_cancel(_: AdminAuth, State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "cancelled": state.admin.cancel_restart() }))
}

/// `POST /admin/save`: writes every active session's unsaved character
/// now: `{"sessions": n, "saved": k, "failed": f}`.
pub async fn save(_: AdminAuth, State(state): State<AppState>) -> Json<Value> {
    let report = state.admin.save_all().await;
    Json(json!(report))
}
