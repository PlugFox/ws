use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Sqlite};

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct CharacterRecord {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub archetype: String,
    pub level: i64,
    pub base_exp: i64,
    pub stats_json: String,
    pub unlocked_skills_json: String,
    pub inventory_json: String,
    pub last_location_id: String,
    pub created_at: String,
    pub updated_at: String,
}

pub struct CharacterRepo;

impl CharacterRepo {
    pub async fn create(
        pool: &Pool<Sqlite>,
        user_id: u32,
        name: &str,
        archetype: &str,
        stats_json: &str,
        unlocked_skills_json: &str,
        inventory_json: &str,
        last_location_id: &str,
    ) -> Result<CharacterRecord, sqlx::Error> {
        let user_id_i64 = user_id as i64;

        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO characters (
                user_id, name, archetype, level, base_exp,
                stats_json, unlocked_skills_json, inventory_json, last_location_id
            )
            VALUES (?, ?, ?, 1, 0, ?, ?, ?, ?)
            RETURNING id
            "#,
        )
        .bind(user_id_i64)
        .bind(name)
        .bind(archetype)
        .bind(stats_json)
        .bind(unlocked_skills_json)
        .bind(inventory_json)
        .bind(last_location_id)
        .fetch_one(pool)
        .await?;

        Self::find_by_id(pool, id as u32)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn list_by_user_id(
        pool: &Pool<Sqlite>,
        user_id: u32,
    ) -> Result<Vec<CharacterRecord>, sqlx::Error> {
        let user_id_i64 = user_id as i64;
        sqlx::query_as::<_, CharacterRecord>(
            r#"
            SELECT id, user_id, name, archetype, level, base_exp,
                   stats_json, unlocked_skills_json, inventory_json, last_location_id,
                   created_at, updated_at
            FROM characters
            WHERE user_id = ?
            ORDER BY id ASC
            "#,
        )
        .bind(user_id_i64)
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_id(
        pool: &Pool<Sqlite>,
        id: u32,
    ) -> Result<Option<CharacterRecord>, sqlx::Error> {
        let id_i64 = id as i64;
        sqlx::query_as::<_, CharacterRecord>(
            r#"
            SELECT id, user_id, name, archetype, level, base_exp,
                   stats_json, unlocked_skills_json, inventory_json, last_location_id,
                   created_at, updated_at
            FROM characters
            WHERE id = ?
            "#,
        )
        .bind(id_i64)
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_user_and_id(
        pool: &Pool<Sqlite>,
        user_id: u32,
        id: u32,
    ) -> Result<Option<CharacterRecord>, sqlx::Error> {
        let user_id_i64 = user_id as i64;
        let id_i64 = id as i64;
        sqlx::query_as::<_, CharacterRecord>(
            r#"
            SELECT id, user_id, name, archetype, level, base_exp,
                   stats_json, unlocked_skills_json, inventory_json, last_location_id,
                   created_at, updated_at
            FROM characters
            WHERE user_id = ? AND id = ?
            "#,
        )
        .bind(user_id_i64)
        .bind(id_i64)
        .fetch_optional(pool)
        .await
    }

    pub async fn update_dirty_state(
        pool: &Pool<Sqlite>,
        character_id: u32,
        level: u32,
        base_exp: u64,
        stats_json: &str,
        unlocked_skills_json: &str,
        inventory_json: &str,
        last_location_id: &str,
    ) -> Result<(), sqlx::Error> {
        let char_id_i64 = character_id as i64;
        let level_i64 = level as i64;
        let exp_i64 = base_exp as i64;

        sqlx::query(
            r#"
            UPDATE characters
            SET level = ?,
                base_exp = ?,
                stats_json = ?,
                unlocked_skills_json = ?,
                inventory_json = ?,
                last_location_id = ?,
                updated_at = datetime('now')
            WHERE id = ?
            "#,
        )
        .bind(level_i64)
        .bind(exp_i64)
        .bind(stats_json)
        .bind(unlocked_skills_json)
        .bind(inventory_json)
        .bind(last_location_id)
        .bind(char_id_i64)
        .execute(pool)
        .await?;

        Ok(())
    }
}
