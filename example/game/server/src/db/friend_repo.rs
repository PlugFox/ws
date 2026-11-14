//! Account-level friends (`friends` table): one row per direction.
//!
//! A request is a single `pending` row `(asker, asked)`. Accepting it turns
//! the pair into two `friend` rows, so "my friends" is one indexed lookup.
//! Asking someone who already asked you accepts their request (a mutual
//! request is a friendship). Removing deletes both directions in any state,
//! which also withdraws a request you sent.

use crate::db::Db;

/// Accepted friends plus requests sent, per account.
pub const MAX_FRIENDS: i64 = 50;
/// Requests waiting on one account (spam guard).
pub const MAX_INCOMING: i64 = 30;

const FRIEND: i16 = 0;
const PENDING: i16 = 1;

/// How an account relates to another (from the first one's side).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Friend,
    /// They asked you.
    Incoming,
    /// You asked them.
    Outgoing,
}

impl Link {
    /// Wire value (`FRIENDS` state byte).
    pub fn code(self) -> u8 {
        match self {
            Link::Friend => 0,
            Link::Incoming => 1,
            Link::Outgoing => 2,
        }
    }
}

/// What [`FriendRepo::request`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requested {
    /// A new request is waiting on the other account.
    Sent,
    /// They had asked first: now friends.
    Accepted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FriendError {
    #[error("You cannot add yourself as a friend.")]
    SelfAdd,
    #[error("That player is already your friend.")]
    AlreadyFriends,
    #[error("Your friend request is already waiting on them.")]
    AlreadyAsked,
    #[error("Your friend list is full.")]
    Full,
    #[error("That player cannot take more friend requests right now.")]
    TheirsFull,
    #[error("No friend request from that player.")]
    NoRequest,
    #[error("No such player.")]
    Unknown,
    #[error("Friends are unavailable right now.")]
    Db,
}

impl FriendError {
    pub fn code(self) -> &'static str {
        match self {
            FriendError::SelfAdd => "friend_self",
            FriendError::AlreadyFriends => "friend_already",
            FriendError::AlreadyAsked => "friend_pending",
            FriendError::Full => "friend_full",
            FriendError::TheirsFull => "friend_their_full",
            FriendError::NoRequest => "friend_no_request",
            FriendError::Unknown => "friend_unknown",
            FriendError::Db => "friend_unavailable",
        }
    }
}

impl From<sqlx::Error> for FriendError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!("friends: {e}");
        FriendError::Db
    }
}

pub struct FriendRepo;

impl FriendRepo {
    async fn state(pool: &Db, from: u32, to: u32) -> Result<Option<i16>, sqlx::Error> {
        sqlx::query_scalar("SELECT state FROM friends WHERE user_id = $1 AND friend_id = $2")
            .bind(from as i64)
            .bind(to as i64)
            .fetch_optional(pool)
            .await
    }

    /// `me` asks `other` to be friends (or accepts their request).
    pub async fn request(pool: &Db, me: u32, other: u32) -> Result<Requested, FriendError> {
        if me == other {
            return Err(FriendError::SelfAdd);
        }
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1").bind(other as i64).fetch_optional(pool).await?;
        if exists.is_none() {
            return Err(FriendError::Unknown);
        }
        match Self::state(pool, me, other).await? {
            Some(FRIEND) => return Err(FriendError::AlreadyFriends),
            Some(_) => return Err(FriendError::AlreadyAsked),
            None => {}
        }
        if Self::state(pool, other, me).await? == Some(PENDING) {
            Self::accept(pool, me, other).await?;
            return Ok(Requested::Accepted);
        }
        let mine: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM friends WHERE user_id = $1").bind(me as i64).fetch_one(pool).await?;
        if mine >= MAX_FRIENDS {
            return Err(FriendError::Full);
        }
        let waiting: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM friends WHERE friend_id = $1 AND state = $2")
            .bind(other as i64)
            .bind(PENDING)
            .fetch_one(pool)
            .await?;
        if waiting >= MAX_INCOMING {
            return Err(FriendError::TheirsFull);
        }
        sqlx::query("INSERT INTO friends (user_id, friend_id, state) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
            .bind(me as i64)
            .bind(other as i64)
            .bind(PENDING)
            .execute(pool)
            .await?;
        Ok(Requested::Sent)
    }

    /// `me` accepts the request `other` sent.
    pub async fn accept(pool: &Db, me: u32, other: u32) -> Result<(), FriendError> {
        if Self::state(pool, other, me).await? != Some(PENDING) {
            return Err(FriendError::NoRequest);
        }
        let mine: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM friends WHERE user_id = $1 AND state = $2")
            .bind(me as i64)
            .bind(FRIEND)
            .fetch_one(pool)
            .await?;
        if mine >= MAX_FRIENDS {
            return Err(FriendError::Full);
        }
        let mut tx = pool.begin().await?;
        for (a, b) in [(me, other), (other, me)] {
            sqlx::query(
                "INSERT INTO friends (user_id, friend_id, state) VALUES ($1, $2, $3) \
                 ON CONFLICT(user_id, friend_id) DO UPDATE SET state = excluded.state",
            )
            .bind(a as i64)
            .bind(b as i64)
            .bind(FRIEND)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// `me` turns down the request `other` sent.
    pub async fn decline(pool: &Db, me: u32, other: u32) -> Result<(), FriendError> {
        let done = sqlx::query("DELETE FROM friends WHERE user_id = $1 AND friend_id = $2 AND state = $3")
            .bind(other as i64)
            .bind(me as i64)
            .bind(PENDING)
            .execute(pool)
            .await?;
        if done.rows_affected() == 0 {
            return Err(FriendError::NoRequest);
        }
        Ok(())
    }

    /// Ends a friendship, or withdraws / declines a request, either way.
    /// Returns whether there was anything to remove.
    pub async fn remove(pool: &Db, me: u32, other: u32) -> Result<bool, FriendError> {
        let done = sqlx::query("DELETE FROM friends WHERE (user_id = $1 AND friend_id = $2) OR (user_id = $3 AND friend_id = $4)")
            .bind(me as i64)
            .bind(other as i64)
            .bind(other as i64)
            .bind(me as i64)
            .execute(pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Everyone `me` relates to: friends, requests to them, their requests.
    pub async fn list(pool: &Db, me: u32) -> Result<Vec<(u32, Link)>, sqlx::Error> {
        let rows: Vec<(i64, i16, i32)> = sqlx::query_as(
            "SELECT friend_id, state, 0 FROM friends WHERE user_id = $1 \
             UNION ALL SELECT user_id, state, 1 FROM friends WHERE friend_id = $2 AND state = $3 \
             ORDER BY 1",
        )
        .bind(me as i64)
        .bind(me as i64)
        .bind(PENDING)
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, state, theirs)| {
                let link = match (state, theirs) {
                    (FRIEND, _) => Link::Friend,
                    (_, 1) => Link::Incoming,
                    _ => Link::Outgoing,
                };
                (id as u32, link)
            })
            .collect())
    }

    /// Accepted friends of `me` (who hears that `me` came online).
    pub async fn friends_of(pool: &Db, me: u32) -> Result<Vec<u32>, sqlx::Error> {
        let ids: Vec<i64> = sqlx::query_scalar("SELECT friend_id FROM friends WHERE user_id = $1 AND state = $2")
            .bind(me as i64)
            .bind(FRIEND)
            .fetch_all(pool)
            .await?;
        Ok(ids.into_iter().map(|id| id as u32).collect())
    }

    /// An account's characters, last played first: `(id, name, class, level)`.
    pub async fn characters(pool: &Db, account: u32) -> Result<Vec<(u32, String, String, u32)>, sqlx::Error> {
        let rows: Vec<(i64, String, String, i32)> =
            sqlx::query_as("SELECT id, name, archetype, level FROM characters WHERE user_id = $1 ORDER BY updated_at DESC, id DESC")
                .bind(account as i64)
                .fetch_all(pool)
                .await?;
        Ok(rows.into_iter().map(|(id, n, a, l)| (id as u32, n, a, l as u32)).collect())
    }

    /// The account playing character `id`.
    pub async fn account_of_character(pool: &Db, id: u32) -> Result<Option<u32>, sqlx::Error> {
        let row: Option<i64> =
            sqlx::query_scalar("SELECT user_id FROM characters WHERE id = $1").bind(id as i64).fetch_optional(pool).await?;
        Ok(row.map(|v| v as u32))
    }

    /// The account owning the character called `name` (any case).
    pub async fn account_of_name(pool: &Db, name: &str) -> Result<Option<u32>, sqlx::Error> {
        let row: Option<i64> = sqlx::query_scalar("SELECT user_id FROM characters WHERE lower(name) = lower($1)")
            .bind(name)
            .fetch_optional(pool)
            .await?;
        Ok(row.map(|v| v as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{init_db, testing, DbConfig};

    async fn pool() -> Db {
        testing::pool().await
    }

    async fn user(pool: &Db, name: &str) -> u32 {
        let id: i64 = sqlx::query_scalar("INSERT INTO users (username, password_hash) VALUES ($1, 'x') RETURNING id")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO characters (user_id, name, archetype) VALUES ($1, $2, 'mage')")
            .bind(id)
            .bind(format!("{name}_char"))
            .execute(pool)
            .await
            .unwrap();
        id as u32
    }

    #[tokio::test]
    async fn request_accept_and_remove() {
        let pool = pool().await;
        let (a, b, c) = (user(&pool, "a").await, user(&pool, "b").await, user(&pool, "c").await);

        assert_eq!(FriendRepo::request(&pool, a, b).await, Ok(Requested::Sent));
        assert_eq!(FriendRepo::list(&pool, a).await.unwrap(), vec![(b, Link::Outgoing)]);
        assert_eq!(FriendRepo::list(&pool, b).await.unwrap(), vec![(a, Link::Incoming)]);
        assert!(FriendRepo::friends_of(&pool, a).await.unwrap().is_empty(), "a request is not a friendship");

        // Only the asked side accepts.
        assert_eq!(FriendRepo::accept(&pool, a, b).await, Err(FriendError::NoRequest));
        FriendRepo::accept(&pool, b, a).await.unwrap();
        assert_eq!(FriendRepo::list(&pool, a).await.unwrap(), vec![(b, Link::Friend)]);
        assert_eq!(FriendRepo::list(&pool, b).await.unwrap(), vec![(a, Link::Friend)]);
        assert_eq!(FriendRepo::friends_of(&pool, b).await.unwrap(), vec![a]);

        // Declining.
        FriendRepo::request(&pool, c, a).await.unwrap();
        FriendRepo::decline(&pool, a, c).await.unwrap();
        assert_eq!(FriendRepo::list(&pool, a).await.unwrap(), vec![(b, Link::Friend)]);
        assert_eq!(FriendRepo::decline(&pool, a, c).await, Err(FriendError::NoRequest));

        // Either side removes, both lose it.
        assert!(FriendRepo::remove(&pool, b, a).await.unwrap());
        assert!(FriendRepo::list(&pool, a).await.unwrap().is_empty());
        assert!(FriendRepo::list(&pool, b).await.unwrap().is_empty());
        assert!(!FriendRepo::remove(&pool, b, a).await.unwrap());

        // Removing withdraws a request too.
        FriendRepo::request(&pool, a, c).await.unwrap();
        assert!(FriendRepo::remove(&pool, a, c).await.unwrap());
        assert!(FriendRepo::list(&pool, c).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn duplicates_self_and_mutual_requests() {
        let pool = pool().await;
        let (a, b) = (user(&pool, "a").await, user(&pool, "b").await);
        assert_eq!(FriendRepo::request(&pool, a, a).await, Err(FriendError::SelfAdd));
        assert_eq!(FriendRepo::request(&pool, a, 999).await, Err(FriendError::Unknown));
        FriendRepo::request(&pool, a, b).await.unwrap();
        assert_eq!(FriendRepo::request(&pool, a, b).await, Err(FriendError::AlreadyAsked));
        // b asks back: that is a yes.
        assert_eq!(FriendRepo::request(&pool, b, a).await, Ok(Requested::Accepted));
        assert_eq!(FriendRepo::request(&pool, a, b).await, Err(FriendError::AlreadyFriends));
        assert_eq!(FriendRepo::request(&pool, b, a).await, Err(FriendError::AlreadyFriends));
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM friends").fetch_one(&pool).await.unwrap();
        assert_eq!(rows, 2, "one row per direction");
    }

    #[tokio::test]
    async fn limits() {
        let pool = pool().await;
        let a = user(&pool, "a").await;
        for i in 0..MAX_FRIENDS {
            let o = user(&pool, &format!("o{i}")).await;
            FriendRepo::request(&pool, a, o).await.unwrap();
        }
        let extra = user(&pool, "extra").await;
        assert_eq!(FriendRepo::request(&pool, a, extra).await, Err(FriendError::Full));

        let popular = user(&pool, "popular").await;
        for i in 0..MAX_INCOMING {
            let o = user(&pool, &format!("f{i}")).await;
            FriendRepo::request(&pool, o, popular).await.unwrap();
        }
        assert_eq!(FriendRepo::request(&pool, extra, popular).await, Err(FriendError::TheirsFull));
    }

    #[tokio::test]
    async fn friendships_persist_and_follow_the_account() {
        let cfg: DbConfig = testing::fresh_config().await;
        let (a, b) = {
            let pool = init_db(&cfg).await.unwrap();
            let (a, b) = (user(&pool, "a").await, user(&pool, "b").await);
            FriendRepo::request(&pool, a, b).await.unwrap();
            FriendRepo::accept(&pool, b, a).await.unwrap();
            pool.close().await;
            (a, b)
        };
        // Reopened (migrations run again): still friends.
        let pool = init_db(&cfg).await.unwrap();
        assert_eq!(FriendRepo::list(&pool, a).await.unwrap(), vec![(b, Link::Friend)]);
        // Account level: any of b's characters resolves to b.
        let alt: i64 = sqlx::query_scalar("INSERT INTO characters (user_id, name, archetype) VALUES ($1, 'b_alt', 'archer') RETURNING id")
            .bind(b as i64)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(FriendRepo::account_of_character(&pool, alt as u32).await.unwrap(), Some(b));
        assert_eq!(FriendRepo::account_of_name(&pool, "B_ALT").await.unwrap(), Some(b));
        assert_eq!(FriendRepo::characters(&pool, b).await.unwrap().len(), 2);
        pool.close().await;
    }
}
