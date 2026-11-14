use serde::{Deserialize, Serialize};
use crate::db::Db;
use chrono::{DateTime, Utc};
use sqlx::FromRow;

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct UserRecord {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Optional contacts (`auth::contacts`, normalised), unverified until
    /// the verification flows exist (`*_verified_at` NULL).
    pub email: Option<String>,
    pub email_verified_at: Option<DateTime<Utc>>,
    pub telegram: Option<String>,
    pub telegram_verified_at: Option<DateTime<Utc>>,
}

/// The columns of [`UserRecord`].
const COLUMNS: &str = "id, username, password_hash, status, created_at, updated_at, \
    email, email_verified_at, telegram, telegram_verified_at";

/// A change of the contacts: `None` keeps one, `Some(None)` clears it,
/// `Some(Some(v))` sets it (already normalised by `auth::contacts`).
#[derive(Clone, Debug, Default)]
pub struct ContactsUpdate<'a> {
    pub email: Option<Option<&'a str>>,
    pub telegram: Option<Option<&'a str>>,
}

pub struct UserRepo;

impl UserRepo {
    pub async fn create(
        pool: &Db,
        username: &str,
        password_hash: &str,
    ) -> Result<UserRecord, sqlx::Error> {
        Self::create_with_contacts(pool, username, password_hash, None, None).await
    }

    /// A new account with its optional contacts (normalised). A taken
    /// name, email or Telegram is a unique violation naming its index.
    pub async fn create_with_contacts(
        pool: &Db,
        username: &str,
        password_hash: &str,
        email: Option<&str>,
        telegram: Option<&str>,
    ) -> Result<UserRecord, sqlx::Error> {
        sqlx::query_as::<_, UserRecord>(&format!(
            "INSERT INTO users (username, password_hash, email, telegram) VALUES ($1, $2, $3, $4) RETURNING {COLUMNS}"
        ))
        .bind(username)
        .bind(password_hash)
        .bind(email)
        .bind(telegram)
        .fetch_one(pool)
        .await
    }

    /// The account (if any) using `email`, in any case.
    pub async fn find_by_email(pool: &Db, email: &str) -> Result<Option<UserRecord>, sqlx::Error> {
        sqlx::query_as::<_, UserRecord>(&format!("SELECT {COLUMNS} FROM users WHERE lower(email) = lower($1)"))
            .bind(email)
            .fetch_optional(pool)
            .await
    }

    /// The account (if any) using Telegram `name`, in any case.
    pub async fn find_by_telegram(pool: &Db, name: &str) -> Result<Option<UserRecord>, sqlx::Error> {
        sqlx::query_as::<_, UserRecord>(&format!("SELECT {COLUMNS} FROM users WHERE lower(telegram) = lower($1)"))
            .bind(name)
            .fetch_optional(pool)
            .await
    }

    /// Applies `update` and returns the account as it is now (`None`: no
    /// such account). A contact that really changes (not just its case)
    /// loses its verification mark. A taken one is a unique violation.
    pub async fn set_contacts(pool: &Db, id: u32, update: &ContactsUpdate<'_>) -> Result<Option<UserRecord>, sqlx::Error> {
        // On the right of SET, columns are the old values.
        sqlx::query_as::<_, UserRecord>(&format!(
            "UPDATE users SET \
                email = CASE WHEN $2 THEN $3 ELSE email END, \
                email_verified_at = CASE WHEN $2 AND lower($3) IS DISTINCT FROM lower(email) THEN NULL ELSE email_verified_at END, \
                telegram = CASE WHEN $4 THEN $5 ELSE telegram END, \
                telegram_verified_at = CASE WHEN $4 AND lower($5) IS DISTINCT FROM lower(telegram) THEN NULL ELSE telegram_verified_at END, \
                updated_at = CASE WHEN $2 OR $4 THEN now() ELSE updated_at END \
             WHERE id = $1 RETURNING {COLUMNS}"
        ))
        .bind(i64::from(id))
        .bind(update.email.is_some())
        .bind(update.email.flatten())
        .bind(update.telegram.is_some())
        .bind(update.telegram.flatten())
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_username(
        pool: &Db,
        username: &str,
    ) -> Result<Option<UserRecord>, sqlx::Error> {
        sqlx::query_as::<_, UserRecord>(&format!("SELECT {COLUMNS} FROM users WHERE lower(username) = lower($1)"))
        .bind(username)
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_id(
        pool: &Db,
        id: u32,
    ) -> Result<Option<UserRecord>, sqlx::Error> {
        let id_i64 = id as i64;
        sqlx::query_as::<_, UserRecord>(&format!("SELECT {COLUMNS} FROM users WHERE id = $1"))
        .bind(id_i64)
        .fetch_optional(pool)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{testing, CharacterRepo};
    use crate::world::combat::{Bag, Class};

    /// Account and character names are unique and found in any case
    /// (the `lower(...)` unique indexes).
    #[tokio::test]
    async fn names_are_unique_in_any_case() {
        let pool = testing::pool().await;
        let ann = UserRepo::create(&pool, "Ann", "x").await.unwrap();
        assert_eq!(UserRepo::find_by_username(&pool, "aNN").await.unwrap().map(|u| u.id), Some(ann.id));
        let dup = UserRepo::create(&pool, "ANN", "x").await.unwrap_err();
        assert!(dup.as_database_error().is_some_and(|e| e.is_unique_violation()), "{dup}");

        let bag = Bag::starting(Class::Mage, "");
        let look = Default::default();
        let c = CharacterRepo::create(&pool, ann.id as u32, "Lumi", "mage", "izlude", look, &bag, 1).await.unwrap();
        assert_eq!(c.bag(Class::Mage), bag, "the kit is stored with the row");
        let dup = CharacterRepo::create(&pool, ann.id as u32, "LUMI", "mage", "izlude", look, &bag, 1).await.unwrap_err();
        assert!(dup.as_database_error().is_some_and(|e| e.is_unique_violation()), "{dup}");
    }

    fn unique(err: &sqlx::Error, index: &str) -> bool {
        err.as_database_error().is_some_and(|e| e.is_unique_violation() && e.constraint() == Some(index))
    }

    /// Contacts are optional (any number of accounts have none), unique in
    /// any case when set, found in any case, and lose their verification
    /// mark when they change.
    #[tokio::test]
    async fn contacts_are_optional_unique_in_any_case_and_updatable() {
        let pool = testing::pool().await;
        let a = UserRepo::create(&pool, "NoContactsA", "x").await.unwrap();
        let b = UserRepo::create(&pool, "NoContactsB", "x").await.unwrap();
        assert_eq!((a.email.as_deref(), a.telegram.as_deref(), b.email.as_deref()), (None, None, None));

        let ann = UserRepo::create_with_contacts(&pool, "Ann", "x", Some("Ann@example.com"), Some("Ann_tg")).await.unwrap();
        assert_eq!((ann.email.as_deref(), ann.telegram.as_deref()), (Some("Ann@example.com"), Some("Ann_tg")));
        assert!(ann.email_verified_at.is_none() && ann.telegram_verified_at.is_none());
        assert_eq!(UserRepo::find_by_email(&pool, "ann@EXAMPLE.com").await.unwrap().map(|u| u.id), Some(ann.id));
        assert_eq!(UserRepo::find_by_telegram(&pool, "ANN_TG").await.unwrap().map(|u| u.id), Some(ann.id));

        let dup = UserRepo::create_with_contacts(&pool, "Bob", "x", Some("ANN@example.com"), None).await.unwrap_err();
        assert!(unique(&dup, "users_email_lower_key"), "{dup}");
        let dup = UserRepo::create_with_contacts(&pool, "Bob", "x", None, Some("ann_TG")).await.unwrap_err();
        assert!(unique(&dup, "users_telegram_lower_key"), "{dup}");
        let dup = UserRepo::set_contacts(&pool, a.id as u32, &ContactsUpdate { email: Some(Some("ann@example.com")), telegram: None }).await.unwrap_err();
        assert!(unique(&dup, "users_email_lower_key"), "{dup}");

        // Verified (by hand: no flow yet); a case-only change keeps the
        // mark, a real change and a clear drop it; omitted fields stay.
        sqlx::query("UPDATE users SET email_verified_at = now(), telegram_verified_at = now() WHERE id = $1").bind(ann.id).execute(&pool).await.unwrap();
        let u = UserRepo::set_contacts(&pool, ann.id as u32, &ContactsUpdate { email: Some(Some("ANN@example.com")), telegram: None }).await.unwrap().unwrap();
        assert_eq!(u.email.as_deref(), Some("ANN@example.com"));
        assert!(u.email_verified_at.is_some() && u.telegram_verified_at.is_some());
        let u = UserRepo::set_contacts(&pool, ann.id as u32, &ContactsUpdate { email: Some(Some("ann@other.org")), telegram: Some(None) }).await.unwrap().unwrap();
        assert_eq!((u.email.as_deref(), u.telegram.as_deref()), (Some("ann@other.org"), None));
        assert!(u.email_verified_at.is_none() && u.telegram_verified_at.is_none());
        // The old values are free again.
        UserRepo::set_contacts(&pool, b.id as u32, &ContactsUpdate { email: Some(Some("ann@example.com")), telegram: Some(Some("Ann_tg")) }).await.unwrap().unwrap();
        let same = UserRepo::set_contacts(&pool, ann.id as u32, &ContactsUpdate::default()).await.unwrap().unwrap();
        assert_eq!((same.email.as_deref(), same.updated_at), (u.email.as_deref(), u.updated_at), "nothing to change");
        assert!(UserRepo::set_contacts(&pool, 999_999, &ContactsUpdate::default()).await.unwrap().is_none());
    }
}
