//! Public status endpoints: `/health` (a real check: the database answers,
//! uptime, version) and `/api/online` (how many players are in game, for
//! the login screen; recounted at most every [`ONLINE_TTL`]).

use super::state::AppState;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a counted `/api/online` answer is served before a recount.
pub const ONLINE_TTL: Duration = Duration::from_secs(2);
/// A `/health` database ping slower than this fails the check.
const DB_PING_TIMEOUT: Duration = Duration::from_secs(2);

/// The last online count and when it was taken.
#[derive(Clone, Default)]
pub struct OnlineCache {
    last: Arc<Mutex<Option<(Instant, usize)>>>,
}

impl OnlineCache {
    /// The cached count while fresh, else `count()` (then cached).
    pub fn get_or(&self, now: Instant, count: impl FnOnce() -> usize) -> usize {
        let mut last = self.last.lock().unwrap();
        match *last {
            Some((at, n)) if now.duration_since(at) < ONLINE_TTL => n,
            _ => {
                let n = count();
                *last = Some((now, n));
                n
            }
        }
    }
}

/// Players in game now (on any map), cached for [`ONLINE_TTL`].
pub fn online_now(state: &AppState) -> usize {
    state.online.get_or(Instant::now(), || state.friends.online_count())
}

/// Whether the database answers a trivial query in time.
pub async fn db_ok(state: &AppState) -> bool {
    let ping = sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.pool);
    matches!(tokio::time::timeout(DB_PING_TIMEOUT, ping).await, Ok(Ok(1)))
}

/// `GET /health`: 200 `{"status": "healthy", ...}` when the database
/// answers, else 503 `"unhealthy"`. Also the version, the uptime and
/// whether the hub maps loaded.
pub async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let db = db_ok(&state).await;
    let status = if db { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    let body = json!({
        "status": if db { "healthy" } else { "unhealthy" },
        "service": "game_server",
        "version": crate::version::version(),
        "uptime_secs": state.started.elapsed().as_secs(),
        "checks": {
            "database": if db { "ok" } else { "error" },
            "hub": if state.hub.is_some() { "ok" } else { "disabled" },
        },
        "online": online_now(&state),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    (status, Json(body))
}

/// `GET /api/online`: `{"online": n, "server_time": unix ms}`; cheap and
/// public (the login screen polls it): shared caches may keep it for the
/// [`ONLINE_TTL`] the count is cached here anyway.
pub async fn online(State(state): State<AppState>) -> impl IntoResponse {
    let body = Json(json!({
        "online": online_now(&state),
        "server_time": chrono::Utc::now().timestamp_millis(),
    }));
    let cache = format!("public, max-age={}", ONLINE_TTL.as_secs());
    ([(axum::http::header::CACHE_CONTROL, cache)], body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_cache_recounts_only_after_the_ttl() {
        let cache = OnlineCache::default();
        let t0 = Instant::now();
        assert_eq!(cache.get_or(t0, || 3), 3);
        assert_eq!(cache.get_or(t0 + Duration::from_millis(500), || 9), 3, "fresh: cached");
        assert_eq!(cache.get_or(t0 + ONLINE_TTL, || 9), 9, "stale: recounted");
    }
}
