//! `GET /api/leaderboard?sort=level|kills|mvp|zeny&page=1&limit=20&me=<character id>`:
//! one page of a board (`db::leaderboard_repo`), public.
//!
//! ```json
//! {"sort": "kills", "page": 1, "limit": 20, "pages": 3, "total": 47,
//!  "entries": [{"rank": 1, "id": 7, "name": "Ann", "class": "swordsman",
//!               "title": "Crusader", "level": 41, "kills": 5120, "mvps": 12,
//!               "zeny": 90210, "deaths": 33, "missions": 40, "best_combo": 212}],
//!  "me": {...same fields...} | null}
//! ```
//!
//! `me` is that character's own row and rank (shown even when it is on
//! another page). Pages are cached for [`PAGE_TTL`] (the counters reach the
//! database with the periodic save anyway); `limit` is capped at
//! [`MAX_LIMIT`].

use super::state::AppState;
use crate::db::leaderboard_repo::{Board, Entry, LeaderboardRepo, MAX_LIMIT};
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a page is served from memory.
pub const PAGE_TTL: Duration = Duration::from_secs(10);
/// Page size when the request names none.
pub const DEFAULT_LIMIT: u32 = 20;
/// Cached pages before the cache starts over (deep pages are rare).
const CACHE_PAGES: usize = 256;

#[derive(Debug, Default, Deserialize)]
pub struct LeaderboardQuery {
    sort: Option<String>,
    page: Option<u32>,
    limit: Option<u32>,
    me: Option<i64>,
}

/// A cached page: (entries, total) by (board, page, limit).
type Page = Arc<(Vec<Entry>, i64)>;

/// Recently served pages.
#[derive(Clone, Default)]
pub struct LeaderboardCache {
    pages: Arc<Mutex<HashMap<(Board, u32, u32), (Instant, Page)>>>,
}

impl LeaderboardCache {
    fn get(&self, key: (Board, u32, u32), now: Instant) -> Option<Page> {
        let pages = self.pages.lock().unwrap();
        pages.get(&key).filter(|(at, _)| now.duration_since(*at) < PAGE_TTL).map(|(_, p)| Arc::clone(p))
    }

    fn put(&self, key: (Board, u32, u32), now: Instant, page: Page) {
        let mut pages = self.pages.lock().unwrap();
        if pages.len() >= CACHE_PAGES {
            pages.retain(|_, (at, _)| now.duration_since(*at) < PAGE_TTL);
            if pages.len() >= CACHE_PAGES {
                pages.clear();
            }
        }
        pages.insert(key, (now, page));
    }
}

pub async fn leaderboard(State(state): State<AppState>, Query(q): Query<LeaderboardQuery>) -> Response {
    let board = match q.sort.as_deref() {
        None | Some("") => Board::default(),
        Some(s) => match Board::parse(s) {
            Some(b) => b,
            None => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": "sort must be level, kills, mvp or zeny" }))).into_response();
            }
        },
    };
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let page = q.page.unwrap_or(1).max(1);
    let key = (board, page, limit);
    let now = Instant::now();
    let cached = match state.leaderboard.get(key, now) {
        Some(p) => p,
        None => {
            let offset = (page - 1).saturating_mul(limit);
            let read = async {
                let entries = LeaderboardRepo::page(&state.pool, board, offset, limit).await?;
                let total = LeaderboardRepo::total(&state.pool).await?;
                Ok::<_, sqlx::Error>(Arc::new((entries, total)))
            };
            match read.await {
                Ok(p) => {
                    state.leaderboard.put(key, now, Arc::clone(&p));
                    p
                }
                Err(err) => return db_error(err),
            }
        }
    };
    let me = match q.me {
        Some(id) => match LeaderboardRepo::entry(&state.pool, board, id).await {
            Ok(e) => e,
            Err(err) => return db_error(err),
        },
        None => None,
    };
    let (entries, total) = &*cached;
    let pages = ((*total).max(0) as u64).div_ceil(u64::from(limit)).max(1);
    let body = json!({
        "sort": board.id(),
        "page": page,
        "limit": limit,
        "pages": pages,
        "total": total,
        "entries": entries,
        "me": me,
    });
    let cache = header::HeaderValue::from_str(&format!("public, max-age={}", PAGE_TTL.as_secs())).expect("ascii");
    ([(header::CACHE_CONTROL, cache)], Json(body)).into_response()
}

fn db_error(err: sqlx::Error) -> Response {
    tracing::error!("leaderboard: {err}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "leaderboard unavailable" }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_pages_expire_and_the_cache_stays_bounded() {
        let cache = LeaderboardCache::default();
        let t0 = Instant::now();
        let page: Page = Arc::new((Vec::new(), 3));
        cache.put((Board::Kills, 1, 20), t0, Arc::clone(&page));
        assert!(cache.get((Board::Kills, 1, 20), t0 + Duration::from_secs(1)).is_some());
        assert!(cache.get((Board::Level, 1, 20), t0).is_none(), "another board");
        assert!(cache.get((Board::Kills, 1, 20), t0 + PAGE_TTL).is_none(), "stale");
        for p in 0..(CACHE_PAGES as u32 + 10) {
            cache.put((Board::Zeny, p, 20), t0, Arc::clone(&page));
        }
        assert!(cache.pages.lock().unwrap().len() <= CACHE_PAGES);
    }
}
