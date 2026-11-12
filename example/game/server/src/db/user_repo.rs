use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Sqlite};

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct UserRecord {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

pub struct UserRepo;

impl UserRepo {
    pub async fn create(
        pool: &Pool<Sqlite>,
        username: &str,
        password_hash: &str,
    ) -> Result<UserRecord, sqlx::Error> {
        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO users (username, password_hash)
            VALUES (?, ?)
            RETURNING id
            "#,
        )
        .bind(username)
        .bind(password_hash)
        .fetch_one(pool)
        .await?;

        Self::find_by_id(pool, id as u32)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn find_by_username(
        pool: &Pool<Sqlite>,
        username: &str,
    ) -> Result<Option<UserRecord>, sqlx::Error> {
        sqlx::query_as::<_, UserRecord>(
            r#"
            SELECT id, username, password_hash, status, created_at, updated_at
            FROM users
            WHERE username = ? COLLATE NOCASE
            "#,
        )
        .bind(username)
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_id(
        pool: &Pool<Sqlite>,
        id: u32,
    ) -> Result<Option<UserRecord>, sqlx::Error> {
        let id_i64 = id as i64;
        sqlx::query_as::<_, UserRecord>(
            r#"
            SELECT id, username, password_hash, status, created_at, updated_at
            FROM users
            WHERE id = ?
            "#,
        )
        .bind(id_i64)
        .fetch_optional(pool)
        .await
    }
}
