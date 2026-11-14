//! Player feedback (`feedback` table, `crate::feedback`): stored from the
//! game, listed, read and closed by operators (`api::mcp`).

use crate::feedback::{Draft, Place, CLOSED, OPEN};
use serde::Serialize;
use crate::db::Db;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, Postgres, QueryBuilder};

/// One report as stored.
#[derive(Clone, Debug, Serialize, FromRow, PartialEq)]
pub struct FeedbackRecord {
    pub id: i64,
    pub user_id: Option<i64>,
    pub character_id: Option<i64>,
    pub character_name: String,
    pub kind: String,
    pub priority: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub resolution: Option<String>,
    pub map: String,
    pub pos_x: Option<f64>,
    pub pos_y: Option<f64>,
    pub level: Option<i32>,
    pub class: String,
    pub client_version: String,
    pub server_version: String,
    pub user_agent: String,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// Who sent a report.
#[derive(Clone, Debug)]
pub struct Author {
    pub user_id: u32,
    pub character_id: u32,
    pub character_name: String,
}

/// [`FeedbackRepo::list`] filters; `None` = any. Newest first; `before`
/// is the cursor (ids below it).
#[derive(Clone, Debug, Default)]
pub struct FeedbackFilter {
    pub status: Option<String>,
    pub kind: Option<String>,
    pub priority: Option<String>,
    pub before: Option<i64>,
    pub limit: i64,
}

const COLUMNS: &str = "id, user_id, character_id, character_name, kind, priority, title, body, status, resolution, \
     map, pos_x, pos_y, level, class, client_version, server_version, user_agent, created_at, closed_at";

pub struct FeedbackRepo;

impl FeedbackRepo {
    /// Stores a validated report; returns its id.
    pub async fn insert(pool: &Db, author: &Author, draft: &Draft, place: Option<&Place>) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(
            "INSERT INTO feedback (user_id, character_id, character_name, kind, priority, title, body, \
                 map, pos_x, pos_y, level, class, client_version, server_version, user_agent) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) RETURNING id",
        )
        .bind(author.user_id as i64)
        .bind(author.character_id as i64)
        .bind(&author.character_name)
        .bind(&draft.kind)
        .bind(&draft.priority)
        .bind(&draft.title)
        .bind(&draft.body)
        .bind(place.map_or("", |p| p.map.as_str()))
        .bind(place.map(|p| p.x as f64))
        .bind(place.map(|p| p.y as f64))
        .bind(place.map(|p| p.level.min(i32::MAX as u32) as i32))
        .bind(place.map_or("", |p| p.class.as_str()))
        .bind(&draft.client_version)
        .bind(crate::version::version())
        .bind(&draft.user_agent)
        .fetch_one(pool)
        .await
    }

    /// The account's reports of the last 24 hours, and how many seconds
    /// ago the latest of them was sent (for `feedback::check_rate`).
    pub async fn recent(pool: &Db, user_id: u32) -> Result<(i64, Option<i64>), sqlx::Error> {
        sqlx::query_as::<_, (i64, Option<i64>)>(
            "SELECT COUNT(*), EXTRACT(EPOCH FROM now() - MAX(created_at))::BIGINT \
             FROM feedback WHERE user_id = $1 AND created_at > now() - INTERVAL '1 day'",
        )
        .bind(user_id as i64)
        .fetch_one(pool)
        .await
    }

    pub async fn get(pool: &Db, id: i64) -> Result<Option<FeedbackRecord>, sqlx::Error> {
        sqlx::query_as::<_, FeedbackRecord>(&format!("SELECT {COLUMNS} FROM feedback WHERE id = $1"))
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// Reports matching `f`, newest first, at most `f.limit`.
    pub async fn list(pool: &Db, f: &FeedbackFilter) -> Result<Vec<FeedbackRecord>, sqlx::Error> {
        let mut q = QueryBuilder::<Postgres>::new(format!("SELECT {COLUMNS} FROM feedback WHERE 1 = 1"));
        if let Some(v) = &f.status {
            q.push(" AND status = ").push_bind(v.clone());
        }
        if let Some(v) = &f.kind {
            q.push(" AND kind = ").push_bind(v.clone());
        }
        if let Some(v) = &f.priority {
            q.push(" AND priority = ").push_bind(v.clone());
        }
        if let Some(v) = f.before {
            q.push(" AND id < ").push_bind(v);
        }
        q.push(" ORDER BY id DESC LIMIT ").push_bind(f.limit.max(1));
        q.build_query_as::<FeedbackRecord>().fetch_all(pool).await
    }

    /// Closes (with a resolution note) or reopens a report; `None` when
    /// there is no such report.
    pub async fn set_status(pool: &Db, id: i64, closed: bool, resolution: Option<&str>) -> Result<Option<FeedbackRecord>, sqlx::Error> {
        let done = if closed {
            sqlx::query("UPDATE feedback SET status = $1, resolution = $2, closed_at = now() WHERE id = $3")
                .bind(CLOSED)
                .bind(resolution)
                .bind(id)
                .execute(pool)
                .await?
        } else {
            // Reopening keeps the old note (history) unless a new one is given.
            sqlx::query("UPDATE feedback SET status = $1, resolution = COALESCE($2, resolution), closed_at = NULL WHERE id = $3")
                .bind(OPEN)
                .bind(resolution)
                .bind(id)
                .execute(pool)
                .await?
        };
        if done.rows_affected() == 0 {
            return Ok(None);
        }
        Self::get(pool, id).await
    }

    /// Report counts per status (`open`, `closed`).
    pub async fn count_by_status(pool: &Db) -> Result<Vec<(String, i64)>, sqlx::Error> {
        sqlx::query_as::<_, (String, i64)>("SELECT status, COUNT(*) FROM feedback GROUP BY status ORDER BY status")
            .fetch_all(pool)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing;

    /// A pool with account 1 and its character 5.
    async fn pool() -> Db {
        let pool = testing::pool().await;
        sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (1, 'ann', 'x')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO characters (id, user_id, name, archetype) VALUES (5, 1, 'Ann', 'mage')").execute(&pool).await.unwrap();
        pool
    }

    fn draft(kind: &str, priority: &str) -> Draft {
        Draft {
            kind: kind.into(),
            priority: priority.into(),
            title: "A title".into(),
            body: "A body long enough to pass.".into(),
            client_version: "0.1".into(),
            user_agent: "test".into(),
        }
    }

    #[tokio::test]
    async fn store_list_close_and_reopen() {
        let pool = pool().await;
        let author = Author { user_id: 1, character_id: 5, character_name: "Ann".into() };
        let place = Place { map: "izlude".into(), x: 12.5, y: 7.0, level: 9, class: "mage".into() };
        let a = FeedbackRepo::insert(&pool, &author, &draft("bug", "high"), Some(&place)).await.unwrap();
        let b = FeedbackRepo::insert(&pool, &author, &draft("suggestion", "low"), None).await.unwrap();
        assert!(b > a);

        let (n, ago) = FeedbackRepo::recent(&pool, 1).await.unwrap();
        assert_eq!(n, 2);
        assert!(ago.is_some_and(|s| (0..5).contains(&s)), "{ago:?}");
        assert_eq!(FeedbackRepo::recent(&pool, 2).await.unwrap(), (0, None));

        let got = FeedbackRepo::get(&pool, a).await.unwrap().unwrap();
        assert_eq!((got.map.as_str(), got.pos_x, got.level, got.status.as_str()), ("izlude", Some(12.5), Some(9), OPEN));
        assert_eq!(got.server_version, crate::version::version());

        let all = FeedbackRepo::list(&pool, &FeedbackFilter { limit: 10, ..Default::default() }).await.unwrap();
        assert_eq!(all.iter().map(|r| r.id).collect::<Vec<_>>(), vec![b, a], "newest first");
        let page = FeedbackRepo::list(&pool, &FeedbackFilter { limit: 10, before: Some(b), ..Default::default() }).await.unwrap();
        assert_eq!(page.iter().map(|r| r.id).collect::<Vec<_>>(), vec![a]);
        let bugs = FeedbackRepo::list(&pool, &FeedbackFilter { limit: 10, kind: Some("bug".into()), ..Default::default() }).await.unwrap();
        assert_eq!(bugs.len(), 1);

        let closed = FeedbackRepo::set_status(&pool, a, true, Some("Fixed in 0.2")).await.unwrap().unwrap();
        assert_eq!((closed.status.as_str(), closed.resolution.as_deref()), (CLOSED, Some("Fixed in 0.2")));
        assert!(closed.closed_at.is_some());
        let open = FeedbackRepo::list(&pool, &FeedbackFilter { limit: 10, status: Some(OPEN.into()), ..Default::default() }).await.unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(FeedbackRepo::count_by_status(&pool).await.unwrap(), vec![("closed".into(), 1), ("open".into(), 1)]);

        let reopened = FeedbackRepo::set_status(&pool, a, false, None).await.unwrap().unwrap();
        assert_eq!((reopened.status.as_str(), reopened.closed_at), (OPEN, None));
        assert_eq!(reopened.resolution.as_deref(), Some("Fixed in 0.2"), "the note stays as history");
        assert!(FeedbackRepo::set_status(&pool, 999, true, None).await.unwrap().is_none());
    }
}
