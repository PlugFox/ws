use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Sqlite};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct SessionRecord {
    pub id: String,
    pub user_id: i64,
    pub device_id: Option<String>,
    pub refresh_hash: Vec<u8>,
    pub refresh_generation: i64,
    pub prev_refresh_hash: Option<Vec<u8>>,
    pub rotated_at: Option<String>,
    pub ip_address: Option<String>,
    pub country: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: String,
    pub last_seen_at: String,
    pub revoked_at: Option<String>,
    pub revoked_reason: Option<String>,
}

pub struct SessionRepo;

impl SessionRepo {
    pub async fn create(
        pool: &Pool<Sqlite>,
        session_id: &Uuid,
        user_id: u32,
        device_id: Option<&str>,
        refresh_hash: &[u8],
        ip_address: Option<&str>,
        country: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<SessionRecord, sqlx::Error> {
        let sid_str = session_id.to_string();
        let user_id_i64 = user_id as i64;

        sqlx::query(
            r#"
            INSERT INTO sessions (
                id, user_id, device_id, refresh_hash, refresh_generation,
                ip_address, country, user_agent
            )
            VALUES (?, ?, ?, ?, 0, ?, ?, ?)
            "#,
        )
        .bind(&sid_str)
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
        pool: &Pool<Sqlite>,
        session_id: &Uuid,
    ) -> Result<Option<SessionRecord>, sqlx::Error> {
        let sid_str = session_id.to_string();
        sqlx::query_as::<_, SessionRecord>(
            r#"
            SELECT id, user_id, device_id, refresh_hash, refresh_generation,
                   prev_refresh_hash, rotated_at, ip_address, country, user_agent,
                   created_at, last_seen_at, revoked_at, revoked_reason
            FROM sessions
            WHERE id = ?
            "#,
        )
        .bind(sid_str)
        .fetch_optional(pool)
        .await
    }

    pub async fn find_live_by_user_id(
        pool: &Pool<Sqlite>,
        user_id: u32,
    ) -> Result<Option<SessionRecord>, sqlx::Error> {
        let user_id_i64 = user_id as i64;
        sqlx::query_as::<_, SessionRecord>(
            r#"
            SELECT id, user_id, device_id, refresh_hash, refresh_generation,
                   prev_refresh_hash, rotated_at, ip_address, country, user_agent,
                   created_at, last_seen_at, revoked_at, revoked_reason
            FROM sessions
            WHERE user_id = ? AND revoked_at IS NULL
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(user_id_i64)
        .fetch_optional(pool)
        .await
    }

    pub async fn rotate_refresh_token(
        pool: &Pool<Sqlite>,
        session_id: &Uuid,
        prev_hash: &[u8],
        new_hash: &[u8],
        new_generation: u32,
    ) -> Result<(), sqlx::Error> {
        let sid_str = session_id.to_string();
        let gen_i64 = new_generation as i64;

        sqlx::query(
            r#"
            UPDATE sessions
            SET prev_refresh_hash = ?,
                refresh_hash = ?,
                refresh_generation = ?,
                rotated_at = datetime('now'),
                last_seen_at = datetime('now')
            WHERE id = ? AND revoked_at IS NULL
            "#,
        )
        .bind(prev_hash)
        .bind(new_hash)
        .bind(gen_i64)
        .bind(sid_str)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn revoke_session(
        pool: &Pool<Sqlite>,
        session_id: &Uuid,
        reason: &str,
    ) -> Result<(), sqlx::Error> {
        let sid_str = session_id.to_string();
        sqlx::query(
            r#"
            UPDATE sessions
            SET revoked_at = datetime('now'),
                revoked_reason = ?
            WHERE id = ? AND revoked_at IS NULL
            "#,
        )
        .bind(reason)
        .bind(sid_str)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn revoke_all_user_sessions(
        pool: &Pool<Sqlite>,
        user_id: u32,
        reason: &str,
    ) -> Result<u64, sqlx::Error> {
        let user_id_i64 = user_id as i64;
        let result = sqlx::query(
            r#"
            UPDATE sessions
            SET revoked_at = datetime('now'),
                revoked_reason = ?
            WHERE user_id = ? AND revoked_at IS NULL
            "#,
        )
        .bind(reason)
        .bind(user_id_i64)
        .execute(pool)
        .await?;

        Ok(result.rows_affected())
    }

    pub async fn update_last_seen(
        pool: &Pool<Sqlite>,
        session_id: &Uuid,
    ) -> Result<(), sqlx::Error> {
        let sid_str = session_id.to_string();
        sqlx::query(
            r#"
            UPDATE sessions
            SET last_seen_at = datetime('now')
            WHERE id = ?
            "#,
        )
        .bind(sid_str)
        .execute(pool)
        .await?;

        Ok(())
    }
}
