//! PostgreSQL storage: the pool, the schema (`migrations/`, applied at
//! boot by [`run_migrations`]) and one repository per table.

pub mod character_repo;
pub mod feedback_repo;
pub mod friend_repo;
pub mod leaderboard_repo;
pub mod migrations;
pub mod party_repo;
pub mod session_repo;
pub mod testing;
pub mod user_repo;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use std::str::FromStr;
use std::time::{Duration, Instant};
use tracing::{info, warn};

pub use character_repo::{CharacterRecord, CharacterRepo};
pub use feedback_repo::FeedbackRepo;
pub use friend_repo::FriendRepo;
pub use leaderboard_repo::LeaderboardRepo;
pub use migrations::run_migrations;
pub use party_repo::{PartyRepo, PartyWriter};
pub use session_repo::{SessionRecord, SessionRepo};
pub use user_repo::{ContactsUpdate, UserRecord, UserRepo};

/// The connection pool every repository takes.
pub type Db = PgPool;

/// Where the database is and how the pool behaves (`DATABASE_*`, see
/// `config.rs` and the server README).
#[derive(Clone, Debug)]
pub struct DbConfig {
    /// `postgres://user:password@host:port/database`.
    pub url: String,
    pub max_connections: u32,
    /// How long a query waits for a free connection before failing.
    pub acquire_timeout: Duration,
    /// Server-side cap on one statement (`statement_timeout`); zero = none.
    pub statement_timeout: Duration,
    /// How long boot keeps retrying an unreachable database (it may start
    /// after the server, e.g. in compose) before giving up.
    pub connect_retry: Duration,
    /// Create the database when it does not exist (`DATABASE_CREATE=1`:
    /// development and e2e runs; production databases are provisioned).
    pub create_if_missing: bool,
}

impl DbConfig {
    /// The pool's connect options: the URL plus the statement timeout as a
    /// session parameter on every connection.
    fn connect_options(&self) -> Result<PgConnectOptions, sqlx::Error> {
        // sqlx logs every statement at debug and slow ones (> 1 s) at warn.
        let mut opts = PgConnectOptions::from_str(&self.url)?.application_name("game_server");
        if !self.statement_timeout.is_zero() {
            opts = opts.options([("statement_timeout", format!("{}ms", self.statement_timeout.as_millis()))]);
        }
        Ok(opts)
    }
}

/// `url` without its password, for logs.
pub fn redacted(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(s), Some(at)) if at > s => {
            let creds = &url[s + 3..at];
            let user = creds.split(':').next().unwrap_or("");
            format!("{}{user}:***{}", &url[..s + 3], &url[at..])
        }
        _ => url.to_string(),
    }
}

/// Opens the pool, retrying with exponential backoff (0.25 s doubling up to
/// 5 s) while the server is unreachable for at most `connect_retry`, then
/// applies pending migrations.
pub async fn init_db(cfg: &DbConfig) -> Result<Db, sqlx::Error> {
    info!("Connecting to PostgreSQL at {} (pool of {})", redacted(&cfg.url), cfg.max_connections);
    let opts = cfg.connect_options()?;
    let started = Instant::now();
    let mut delay = Duration::from_millis(250);
    let pool = loop {
        match open(cfg, opts.clone()).await {
            Ok(pool) => break pool,
            Err(err) if retryable(&err) && started.elapsed() + delay < cfg.connect_retry => {
                warn!("Database not reachable yet ({err}); retrying in {delay:?}");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(5));
            }
            Err(err) => return Err(err),
        }
    };

    info!("Running database schema migrations...");
    run_migrations(&pool).await?;
    info!("Database migrations applied successfully.");
    Ok(pool)
}

/// One connection attempt: creates the database first when asked to.
async fn open(cfg: &DbConfig, opts: PgConnectOptions) -> Result<Db, sqlx::Error> {
    if cfg.create_if_missing {
        create_if_missing(&cfg.url).await?;
    }
    PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .acquire_timeout(cfg.acquire_timeout)
        // Recycle idle connections so a restarted database or a proxy
        // dropping idle sockets does not leave dead ones in the pool.
        .idle_timeout(Duration::from_secs(300))
        .max_lifetime(Duration::from_secs(1800))
        .connect_with(opts)
        .await
}

async fn create_if_missing(url: &str) -> Result<(), sqlx::Error> {
    use sqlx::migrate::MigrateDatabase;
    if !sqlx::Postgres::database_exists(url).await? {
        info!("Creating database {}", redacted(url));
        sqlx::Postgres::create_database(url).await?;
    }
    Ok(())
}

/// Errors worth waiting out at boot: the server is not up (yet), or is
/// still starting / shutting down (`57P03` cannot_connect_now). A wrong
/// password or a missing database fails at once.
fn retryable(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Io(_) | sqlx::Error::PoolTimedOut | sqlx::Error::Tls(_) => true,
        sqlx::Error::Database(db) => db.code().as_deref() == Some("57P03"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_the_password_only() {
        assert_eq!(redacted("postgres://mobbed:secret@postgres:5432/mobbed"), "postgres://mobbed:***@postgres:5432/mobbed");
        assert_eq!(redacted("postgres://localhost/mobbed"), "postgres://localhost/mobbed");
    }
}
