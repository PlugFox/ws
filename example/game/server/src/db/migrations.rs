//! The schema: ordered SQL files in `migrations/` (`NNNN_<what>.sql`),
//! embedded at compile time and applied at boot. sqlx keeps the applied
//! versions and their checksums in `_sqlx_migrations`, so each file runs
//! once, in order, inside a transaction, and an edited file that already
//! ran is refused instead of silently diverging. A schema change is a new
//! file, never an edit of an applied one.

use sqlx::migrate::Migrator;
use sqlx::PgPool;

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Applies the migrations the database has not seen yet.
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::Error> {
    MIGRATOR.run(pool).await.map_err(Into::into)
}
