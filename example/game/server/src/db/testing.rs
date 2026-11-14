//! Throwaway databases for tests (unit and `tests/*.rs`).
//!
//! Every test gets its own database on the test server, created empty and
//! migrated by the code under test, so tests run in parallel without
//! seeing each other's rows. The server is `TEST_DATABASE_URL`, else
//! `DATABASE_URL`, else the dev container (`mise run game:db:up`); its role
//! needs CREATEDB. Databases are named `gs_test_<unix secs>_<random>`
//! (about 8 MB each); each test process drops the ones older than
//! [`STALE_AFTER`] left by earlier runs, and e2e databases
//! (`gs_e2e_<unix secs>`, `e2e/playwright.config.ts`) older than
//! [`E2E_STALE_AFTER`], so they do not pile up.

use super::{redacted, Db, DbConfig};
use sqlx::migrate::MigrateDatabase;
use sqlx::{Connection, PgConnection, Postgres};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::OnceCell;

/// The dev container of `example/game/compose.dev.yaml`.
pub const DEV_DATABASE_URL: &str = "postgres://mobbed:mobbed@127.0.0.1:55432/mobbed";
/// Test databases older than this are dropped (a test run takes seconds;
/// the margin covers a concurrent run elsewhere).
pub const STALE_AFTER: Duration = Duration::from_secs(15 * 60);
/// E2E databases older than this are dropped.
pub const E2E_STALE_AFTER: Duration = Duration::from_secs(6 * 3600);
const PREFIX: &str = "gs_test_";
const E2E_PREFIX: &str = "gs_e2e_";

/// The server tests run against (a database on it to connect to first).
pub fn server_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .ok()
        .filter(|u| u.starts_with("postgres"))
        .unwrap_or_else(|| DEV_DATABASE_URL.to_string())
}

/// `url` pointing at database `name` instead (query parameters kept).
pub fn with_database(url: &str, name: &str) -> String {
    let (base, query) = url.split_once('?').map_or((url, None), |(b, q)| (b, Some(q)));
    let scheme_end = base.find("://").map_or(0, |i| i + 3);
    let host_end = base[scheme_end..].find('/').map_or(base.len(), |i| scheme_end + i);
    let mut out = format!("{}/{name}", &base[..host_end]);
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    out
}

fn unreachable(url: &str, err: sqlx::Error) -> ! {
    panic!(
        "\n\nPostgreSQL for tests is not reachable at {} ({err}).\n\
         Start the dev database:  mise run game:db:up\n\
         (or point TEST_DATABASE_URL at a server whose role may CREATE DATABASE)\n\n",
        redacted(url)
    )
}

/// Drops leftovers of earlier runs, once per test process.
async fn sweep(url: &str) {
    static SWEPT: OnceCell<()> = OnceCell::const_new();
    SWEPT
        .get_or_init(|| async {
            let Ok(mut conn) = PgConnection::connect(url).await else { return };
            let names: Vec<String> = sqlx::query_scalar("SELECT datname FROM pg_database WHERE datname LIKE 'gs\\_%'")
                .fetch_all(&mut conn)
                .await
                .unwrap_or_default();
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
            for name in names {
                let (rest, keep) = if let Some(rest) = name.strip_prefix(PREFIX) {
                    (rest, STALE_AFTER)
                } else if let Some(rest) = name.strip_prefix(E2E_PREFIX) {
                    (rest, E2E_STALE_AFTER)
                } else {
                    continue;
                };
                let born = rest.split('_').next().and_then(|s| s.parse::<u64>().ok());
                if born.is_some_and(|b| now.saturating_sub(b) > keep.as_secs()) {
                    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)")).execute(&mut conn).await;
                }
            }
            let _ = conn.close().await;
        })
        .await;
}

/// A new, empty database on the test server; returns its URL. Panics with
/// how to start the database when the server is not reachable.
pub async fn fresh_database_url() -> String {
    let server = server_url();
    sweep(&server).await;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let name = format!("{PREFIX}{now}_{}", uuid::Uuid::new_v4().simple());
    let url = with_database(&server, &name);
    if let Err(err) = Postgres::create_database(&url).await {
        unreachable(&server, err);
    }
    url
}

/// Pool settings of a test database: small, fail fast.
pub async fn fresh_config() -> DbConfig {
    DbConfig {
        url: fresh_database_url().await,
        max_connections: 5,
        acquire_timeout: Duration::from_secs(10),
        statement_timeout: Duration::from_secs(10),
        connect_retry: Duration::ZERO,
        create_if_missing: false,
    }
}

/// A migrated pool on a fresh database (repository unit tests).
pub async fn pool() -> Db {
    super::init_db(&fresh_config().await).await.expect("test database")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swaps_the_database_of_a_url() {
        assert_eq!(with_database("postgres://u:p@h:1/mobbed", "x"), "postgres://u:p@h:1/x");
        assert_eq!(with_database("postgres://u:p@h:1/mobbed?sslmode=disable", "x"), "postgres://u:p@h:1/x?sslmode=disable");
        assert_eq!(with_database("postgres://h", "x"), "postgres://h/x");
    }
}
