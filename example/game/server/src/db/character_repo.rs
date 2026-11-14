use crate::db::Db;
use crate::world::combat::items::Stack;
use crate::world::combat::{Bag, Loadout};
use crate::world::protocol::Look;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::FromRow;

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct CharacterRecord {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub archetype: String,
    pub level: i32,
    pub base_exp: i64,
    pub stats: Json<serde_json::Value>,
    pub unlocked_skills: Json<serde_json::Value>,
    /// Bag stacks (`combat::Bag::items`).
    pub inventory: Json<Vec<Stack>>,
    pub last_location_id: String,
    pub gender: i16,
    pub hair_style: i16,
    pub hair_color: i16,
    pub body_color: i16,
    /// Held weapon id (`combat::weapon`); empty = the class's starting weapon.
    pub weapon: String,
    /// Learned skills and the hotbar.
    pub loadout: Json<Loadout>,
    /// The ten equipment slots (`combat::Bag::equip`); `None` = never set.
    pub equipment: Option<Json<Vec<String>>>,
    pub zeny: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Columns of [`CharacterRecord`].
const COLUMNS: &str = "id, user_id, name, archetype, level, base_exp, stats, unlocked_skills, inventory, last_location_id, \
     gender, hair_style, hair_color, body_color, weapon, loadout, equipment, zeny, created_at, updated_at";

impl CharacterRecord {
    /// Stored appearance; out-of-range values (hand-edited rows) fall back
    /// to the default look.
    pub fn look(&self) -> Look {
        let byte = |v: i16| u8::try_from(v).unwrap_or(u8::MAX);
        let look = Look {
            gender: byte(self.gender),
            hair: byte(self.hair_style),
            hair_color: byte(self.hair_color),
            body_color: byte(self.body_color),
        };
        if look.is_valid() { look } else { Look { gender: look.gender.min(1), ..Look::default() } }
    }

    /// The stored belongings (the starting kit when never set).
    pub fn bag(&self, class: crate::world::combat::Class) -> Bag {
        Bag::load(class, self.inventory.0.clone(), self.equipment.as_ref().map(|e| e.0.clone()), self.zeny, &self.weapon)
    }

    /// Mirrors a bag just stored for this row.
    pub fn set_bag(&mut self, bag: &Bag) {
        self.inventory = Json(bag.items.clone());
        self.equipment = Some(Json(bag.equip.to_vec()));
        self.zeny = bag.zeny as i64;
        self.weapon = bag.weapon().into();
    }
}

pub struct CharacterRepo;

impl CharacterRepo {
    /// A new character at `level` with `bag` (its starting kit); nothing
    /// learned yet. Fails with a unique violation when the name is taken
    /// in any case.
    pub async fn create(
        pool: &Db,
        user_id: u32,
        name: &str,
        archetype: &str,
        last_location_id: &str,
        look: Look,
        bag: &Bag,
        level: u32,
    ) -> Result<CharacterRecord, sqlx::Error> {
        sqlx::query_as::<_, CharacterRecord>(&format!(
            "INSERT INTO characters (user_id, name, archetype, level, last_location_id, \
                 gender, hair_style, hair_color, body_color, inventory, equipment, zeny, weapon) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
             RETURNING {COLUMNS}"
        ))
        .bind(i64::from(user_id))
        .bind(name)
        .bind(archetype)
        .bind(level as i32)
        .bind(last_location_id)
        .bind(i16::from(look.gender))
        .bind(i16::from(look.hair))
        .bind(i16::from(look.hair_color))
        .bind(i16::from(look.body_color))
        .bind(Json(&bag.items))
        .bind(Json(&bag.equip))
        .bind(bag.zeny as i64)
        .bind(bag.weapon())
        .fetch_one(pool)
        .await
    }

    pub async fn count_by_user_id(pool: &Db, user_id: u32) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM characters WHERE user_id = $1")
            .bind(i64::from(user_id))
            .fetch_one(pool)
            .await
    }

    pub async fn list_by_user_id(pool: &Db, user_id: u32) -> Result<Vec<CharacterRecord>, sqlx::Error> {
        sqlx::query_as::<_, CharacterRecord>(&format!("SELECT {COLUMNS} FROM characters WHERE user_id = $1 ORDER BY id"))
            .bind(i64::from(user_id))
            .fetch_all(pool)
            .await
    }

    pub async fn find_by_id(pool: &Db, id: u32) -> Result<Option<CharacterRecord>, sqlx::Error> {
        sqlx::query_as::<_, CharacterRecord>(&format!("SELECT {COLUMNS} FROM characters WHERE id = $1"))
            .bind(i64::from(id))
            .fetch_optional(pool)
            .await
    }

    pub async fn find_by_user_and_id(pool: &Db, user_id: u32, id: u32) -> Result<Option<CharacterRecord>, sqlx::Error> {
        sqlx::query_as::<_, CharacterRecord>(&format!("SELECT {COLUMNS} FROM characters WHERE user_id = $1 AND id = $2"))
            .bind(i64::from(user_id))
            .bind(i64::from(id))
            .fetch_optional(pool)
            .await
    }

    /// Replaces a character's belongings.
    pub async fn set_bag(pool: &Db, character_id: u32, bag: &Bag) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE characters SET inventory = $1, equipment = $2, zeny = $3, weapon = $4, updated_at = now() WHERE id = $5")
            .bind(Json(&bag.items))
            .bind(Json(&bag.equip))
            .bind(bag.zeny as i64)
            .bind(bag.weapon())
            .bind(i64::from(character_id))
            .execute(pool)
            .await
            .map(|_| ())
    }

    /// Both sides of a player trade (`session::trade`): their bags in one
    /// transaction, so either both rows change or neither does. The rows
    /// are locked lower id first, so two trades sharing a character can
    /// never deadlock in the database either (`session::trade` already
    /// serializes them on the characters' locks).
    pub async fn store_trade(pool: &Db, a: (u32, &Bag), b: (u32, &Bag)) -> Result<(), sqlx::Error> {
        let mut sides = [a, b];
        sides.sort_by_key(|(id, _)| *id);
        let mut tx = pool.begin().await?;
        let ids = [i64::from(sides[0].0), i64::from(sides[1].0)];
        let locked: Vec<i64> = sqlx::query_scalar("SELECT id FROM characters WHERE id = ANY($1) ORDER BY id FOR UPDATE")
            .bind(&ids[..])
            .fetch_all(&mut *tx)
            .await?;
        if locked.len() != 2 {
            return Err(sqlx::Error::RowNotFound);
        }
        for (id, bag) in sides {
            sqlx::query("UPDATE characters SET inventory = $1, equipment = $2, zeny = $3, weapon = $4, updated_at = now() WHERE id = $5")
                .bind(Json(&bag.items))
                .bind(Json(&bag.equip))
                .bind(bag.zeny as i64)
                .bind(bag.weapon())
                .bind(i64::from(id))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }

    /// Writes a session's changed character (`session::flusher`).
    pub async fn update_dirty_state(
        pool: &Db,
        character_id: u32,
        level: u32,
        base_exp: u64,
        stats: &serde_json::Value,
        unlocked_skills: &serde_json::Value,
        last_location_id: &str,
        look: Look,
        loadout: &Loadout,
        bag: &Bag,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE characters SET level = $1, base_exp = $2, stats = $3, unlocked_skills = $4, inventory = $5, \
                 last_location_id = $6, gender = $7, hair_style = $8, hair_color = $9, body_color = $10, \
                 loadout = $11, equipment = $12, zeny = $13, weapon = $14, updated_at = now() \
             WHERE id = $15",
        )
        .bind(level.min(i32::MAX as u32) as i32)
        .bind(base_exp.min(i64::MAX as u64) as i64)
        .bind(Json(stats))
        .bind(Json(unlocked_skills))
        .bind(Json(&bag.items))
        .bind(last_location_id)
        .bind(i16::from(look.gender))
        .bind(i16::from(look.hair))
        .bind(i16::from(look.hair_color))
        .bind(i16::from(look.body_color))
        .bind(Json(loadout))
        .bind(Json(&bag.equip))
        .bind(bag.zeny as i64)
        .bind(bag.weapon())
        .bind(i64::from(character_id))
        .execute(pool)
        .await
        .map(|_| ())
    }
}
