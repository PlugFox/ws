use serde::{Deserialize, Serialize};
use crate::db::Db;
use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct SessionRecord {
    pub id: Uuid,
    pub user_id: i64,
    pub device_id: Option<String>,
    pub refresh_hash: Vec<u8>,
    pub refresh_generation: i32,
    pub prev_refresh_hash: Option<Vec<u8>>,
    pub rotated_at: Option<DateTime<Utc>>,
    pub ip_address: Option<String>,
    pub country: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_reason: Option<String>,
}

pub struct SessionRepo;

impl SessionRepo {
    pub async fn create(
        pool: &Db,
        session_id: &Uuid,
        user_id: u32,
        device_id: Option<&str>,
        refresh_hash: &[u8],
        ip_address: Option<&str>,
        country: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<SessionRecord, sqlx::Error> {
        let user_id_i64 = user_id as i64;

        sqlx::query(
            r#"
            INSERT INTO sessions (
                id, user_id, device_id, refresh_hash, refresh_generation,
                ip_address, country, user_agent
            )
            VALUES ($1, $2, $3, $4, 0, $5, $6, $7)
            "#,
        )
        .bind(session_id)
        .bind(user_id_i64)
        .bind(device_id)
        .bind(refresh_hash)
        .bind(ip_address)
        .bind(country)
        .bind(user_agent)
        .execute(pool)
        .await?;

        Self::find_by_id(pool, session_id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn find_by_id(
        pool: &Db,
        session_id: &Uuid,
    ) -> Result<Option<SessionRecord>, sqlx::Error> {
        sqlx::query_as::<_, SessionRecord>(
            r#"
            SELECT id, user_id, device_id, refresh_hash, refresh_generation,
                   prev_refresh_hash, rotated_at, ip_address, country, user_agent,
                   created_at, last_seen_at, revoked_at, revoked_reason
            FROM sessions
            WHERE id = $1
            "#,
        )
        .bind(session_id)
        .fetch_optional(pool)
        .await
    }

    pub async fn find_live_by_user_id(
        pool: &Db,
        user_id: u32,
    ) -> Result<Option<SessionRecord>, sqlx::Error> {
        let user_id_i64 = user_id as i64;
        sqlx::query_as::<_, SessionRecord>(
            r#"
            SELECT id, user_id, device_id, refresh_hash, refresh_generation,
                   prev_refresh_hash, rotated_at, ip_address, country, user_agent,
                   created_at, last_seen_at, revoked_at, revoked_reason
            FROM sessions
            WHERE user_id = $1 AND revoked_at IS NULL
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(user_id_i64)
        .fetch_optional(pool)
        .await
    }

    /// Replaces the current refresh token `prev_hash` with `new_hash`.
    /// A compare-and-swap: it only applies while `prev_hash` is still the
    /// current token of a live session, so of two concurrent refreshes
    /// with the same token exactly one wins. Returns whether it applied.
    pub async fn rotate_refresh_token(
        pool: &Db,
        session_id: &Uuid,
        prev_hash: &[u8],
        new_hash: &[u8],
        new_generation: u32,
    ) -> Result<bool, sqlx::Error> {
        let done = sqlx::query(
            r#"
            UPDATE sessions
            SET prev_refresh_hash = refresh_hash,
                refresh_hash = $2,
                refresh_generation = $3,
                rotated_at = now(),
                last_seen_at = now()
            WHERE id = $4 AND revoked_at IS NULL AND refresh_hash = $1
            "#,
        )
        .bind(prev_hash)
        .bind(new_hash)
        .bind(new_generation.min(i32::MAX as u32) as i32)
        .bind(session_id)
        .execute(pool)
        .await?;

        Ok(done.rows_affected() == 1)
    }

    pub async fn revoke_session(
        pool: &Db,
        session_id: &Uuid,
        reason: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE sessions
            SET revoked_at = now(),
                revoked_reason = $1
            WHERE id = $2 AND revoked_at IS NULL
            "#,
        )
        .bind(reason)
        .bind(session_id)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn revoke_all_user_sessions(
        pool: &Db,
        user_id: u32,
        reason: &str,
    ) -> Result<u64, sqlx::Error> {
        let user_id_i64 = user_id as i64;
        let result = sqlx::query(
            r#"
            UPDATE sessions
            SET revoked_at = now(),
                revoked_reason = $1
            WHERE user_id = $2 AND revoked_at IS NULL
            "#,
        )
        .bind(reason)
        .bind(user_id_i64)
        .execute(pool)
        .await?;

        Ok(result.rows_affected())
    }

    pub async fn update_last_seen(
        pool: &Db,
        session_id: &Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE sessions
            SET last_seen_at = now()
            WHERE id = $1
            "#,
        )
        .bind(session_id)
        .execute(pool)
        .await?;

        Ok(())
    }
}
