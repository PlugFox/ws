//! The account storage table (`account_storage`, `storage_log`).
//!
//! A move is one transaction: the character's row is locked first and
//! must still be at the revision the session's memory stands on (else
//! another session stored it since: refused), then the storage row; the
//! move is applied to both in memory, both rows and a log line are
//! written, and only after the commit does the caller change its memory.
//! Every path locks the character row before the storage row, so two
//! moves never deadlock.

use super::character_repo::write_bag;
use super::Db;
use crate::world::combat::items::Stack;
use crate::world::combat::vault::{transfer, Dir, Vault, VaultError};
use crate::world::combat::Bag;
use sqlx::types::Json;

pub struct StorageRepo;

/// Why a move did not happen.
#[derive(Debug)]
pub enum StorageFail {
    /// The rules refused it (nothing changed).
    Rule(VaultError),
    /// The character's row moved on: this session's memory is stale.
    Stale,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for StorageFail {
    fn from(e: sqlx::Error) -> Self {
        StorageFail::Db(e)
    }
}

/// A committed move: the character's bag and revision, the storage.
#[derive(Debug)]
pub struct Moved {
    pub bag: Bag,
    pub rev: i64,
    pub vault: Vault,
}

impl StorageRepo {
    /// The account's storage (empty when never used).
    pub async fn load(pool: &Db, user_id: u32) -> Result<Vault, sqlx::Error> {
        let row: Option<(i64, Json<Vec<Stack>>)> = sqlx::query_as("SELECT zeny, items FROM account_storage WHERE user_id = $1")
            .bind(i64::from(user_id))
            .fetch_optional(pool)
            .await?;
        Ok(row.map_or_else(Vault::default, |(zeny, items)| Vault::load(items.0, zeny)))
    }

    /// Moves `items` and `zeny` between character `character_id` (its
    /// memory: `bag` at revision `rev`) and the account's storage.
    pub async fn transfer(
        pool: &Db,
        user_id: u32,
        character_id: u32,
        rev: i64,
        bag: &Bag,
        dir: Dir,
        items: &[(String, u32)],
        zeny: u64,
    ) -> Result<Moved, StorageFail> {
        let mut tx = pool.begin().await?;
        let stored: Option<i64> = sqlx::query_scalar("SELECT rev FROM characters WHERE id = $1 AND user_id = $2 FOR UPDATE")
            .bind(i64::from(character_id))
            .bind(i64::from(user_id))
            .fetch_optional(&mut *tx)
            .await?;
        if stored != Some(rev) {
            return Err(StorageFail::Stale);
        }
        sqlx::query("INSERT INTO account_storage (user_id) VALUES ($1) ON CONFLICT (user_id) DO NOTHING")
            .bind(i64::from(user_id))
            .execute(&mut *tx)
            .await?;
        let (vault_zeny, vault_items): (i64, Json<Vec<Stack>>) = sqlx::query_as("SELECT zeny, items FROM account_storage WHERE user_id = $1 FOR UPDATE")
            .bind(i64::from(user_id))
            .fetch_one(&mut *tx)
            .await?;
        let mut vault = Vault::load(vault_items.0, vault_zeny);
        let mut bag = bag.clone();
        transfer(&mut bag, &mut vault, dir, items, zeny).map_err(StorageFail::Rule)?;
        write_bag(&mut tx, character_id, &bag).await?;
        sqlx::query("UPDATE account_storage SET zeny = $1, items = $2, rev = rev + 1, updated_at = now() WHERE user_id = $3")
            .bind(vault.zeny as i64)
            .bind(Json(&vault.items))
            .bind(i64::from(user_id))
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO storage_log (user_id, character_id, dir, items, zeny) VALUES ($1, $2, $3, $4, $5)")
            .bind(i64::from(user_id))
            .bind(i64::from(character_id))
            .bind(i16::from(dir == Dir::Take))
            .bind(Json(items))
            .bind(zeny as i64)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Moved { bag, rev: rev + 1, vault })
    }
}
