use crate::world::protocol::Look;
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
    pub gender: i64,
    pub hair_style: i64,
    pub hair_color: i64,
    pub body_color: i64,
    /// Held weapon id (`combat::weapon`); empty = the class's starting weapon.
    pub weapon: String,
    /// Learned skills and the hotbar (`combat::Loadout` as JSON).
    pub loadout_json: String,
    pub created_at: String,
    pub updated_at: String,
}

impl CharacterRecord {
    /// Stored appearance; out-of-range values (hand-edited rows) fall back
    /// to the default look.
    pub fn look(&self) -> Look {
        let byte = |v: i64| u8::try_from(v).unwrap_or(u8::MAX);
        let look = Look {
            gender: byte(self.gender),
            hair: byte(self.hair_style),
            hair_color: byte(self.hair_color),
            body_color: byte(self.body_color),
        };
        if look.is_valid() { look } else { Look { gender: look.gender.min(1), ..Look::default() } }
    }
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
        look: Look,
    ) -> Result<CharacterRecord, sqlx::Error> {
        let user_id_i64 = user_id as i64;

        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO characters (
                user_id, name, archetype, level, base_exp,
                stats_json, unlocked_skills_json, inventory_json, last_location_id,
                gender, hair_style, hair_color, body_color
            )
            VALUES (?, ?, ?, 1, 0, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(i64::from(look.gender))
        .bind(i64::from(look.hair))
        .bind(i64::from(look.hair_color))
        .bind(i64::from(look.body_color))
        .fetch_one(pool)
        .await?;

        Self::find_by_id(pool, id as u32)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn count_by_user_id(pool: &Pool<Sqlite>, user_id: u32) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM characters WHERE user_id = ?")
            .bind(user_id as i64)
            .fetch_one(pool)
            .await
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
                   gender, hair_style, hair_color, body_color, weapon, loadout_json,
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
                   gender, hair_style, hair_color, body_color, weapon, loadout_json,
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
                   gender, hair_style, hair_color, body_color, weapon, loadout_json,
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

    /// Sets a fresh character's level (test hook, `GAME_DEV_LEVEL`).
    pub async fn set_level(pool: &Pool<Sqlite>, character_id: u32, level: u32) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE characters SET level = ?, base_exp = 0 WHERE id = ?")
            .bind(level as i64)
            .bind(character_id as i64)
            .execute(pool)
            .await
            .map(|_| ())
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
        look: Look,
        loadout_json: &str,
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
                gender = ?,
                hair_style = ?,
                hair_color = ?,
                body_color = ?,
                loadout_json = ?,
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
        .bind(i64::from(look.gender))
        .bind(i64::from(look.hair))
        .bind(i64::from(look.hair_color))
        .bind(i64::from(look.body_color))
        .bind(loadout_json)
        .bind(char_id_i64)
        .execute(pool)
        .await?;

        Ok(())
    }
}
