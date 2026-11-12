pub mod character_repo;
pub mod migrations;
pub mod session_repo;
pub mod user_repo;

use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::time::Duration;
use tracing::info;

pub use character_repo::{CharacterRecord, CharacterRepo};
pub use migrations::run_migrations;
pub use session_repo::{SessionRecord, SessionRepo};
pub use user_repo::{UserRecord, UserRepo};

/// Initializes the SQLite connection pool with WAL mode and runs embedded schema migrations.
pub async fn init_db(database_url: &str) -> Result<Pool<Sqlite>, sqlx::Error> {
    info!("Initializing database connection pool to '{}'", database_url);
    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(database_url)
        .await?;

    // Enable WAL mode and foreign keys for high concurrent read performance and integrity
    sqlx::query("PRAGMA journal_mode = WAL;")
        .execute(&pool)
        .await?;
    sqlx::query("PRAGMA foreign_keys = ON;")
        .execute(&pool)
        .await?;
    sqlx::query("PRAGMA synchronous = NORMAL;")
        .execute(&pool)
        .await?;

    info!("Running database schema migrations...");
    run_migrations(&pool).await?;
    info!("Database migrations applied successfully.");

    Ok(pool)
}
