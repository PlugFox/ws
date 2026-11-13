use sqlx::{Pool, Sqlite};

/// Executes embedded database schema migrations.
pub async fn run_migrations(pool: &Pool<Sqlite>) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE COLLATE NOCASE,
            password_hash TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_users_username ON users(username);

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            device_id TEXT,
            refresh_hash BLOB NOT NULL,
            refresh_generation INTEGER NOT NULL DEFAULT 0,
            prev_refresh_hash BLOB,
            rotated_at TEXT,
            ip_address TEXT,
            country TEXT,
            user_agent TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            revoked_at TEXT,
            revoked_reason TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_user_live ON sessions(user_id) WHERE revoked_at IS NULL;

        CREATE TABLE IF NOT EXISTS characters (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            name TEXT NOT NULL UNIQUE COLLATE NOCASE,
            archetype TEXT NOT NULL,
            level INTEGER NOT NULL DEFAULT 1,
            base_exp INTEGER NOT NULL DEFAULT 0,
            stats_json TEXT NOT NULL DEFAULT '{}',
            unlocked_skills_json TEXT NOT NULL DEFAULT '[]',
            inventory_json TEXT NOT NULL DEFAULT '[]',
            last_location_id TEXT NOT NULL DEFAULT 'hub_central',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_characters_user ON characters(user_id);

        CREATE TABLE IF NOT EXISTS audit_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
            event_type TEXT NOT NULL,
            ip_address TEXT,
            country TEXT,
            details_json TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_audit_user ON audit_events(user_id);
        "#,
    )
    .execute(pool)
    .await?;

    // Appearance columns (added after the first schema): ALTER only when
    // missing, so existing databases upgrade in place.
    let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('characters')")
        .fetch_all(pool)
        .await?;
    for (name, default) in [("gender", 0), ("hair_style", 1), ("hair_color", 0), ("body_color", 0)] {
        if !columns.iter().any(|c| c == name) {
            sqlx::query(&format!("ALTER TABLE characters ADD COLUMN {name} INTEGER NOT NULL DEFAULT {default}"))
                .execute(pool)
                .await?;
        }
    }

    if !columns.iter().any(|c| c == "weapon") {
        sqlx::query("ALTER TABLE characters ADD COLUMN weapon TEXT NOT NULL DEFAULT ''").execute(pool).await?;
    }

    // Class rework (COMBAT.md §1.2): the first archetypes map to the nearest
    // playable line. Idempotent.
    sqlx::query(
        "UPDATE characters SET archetype = CASE archetype \
             WHEN 'warrior' THEN 'swordsman' WHEN 'novice' THEN 'swordsman' \
             WHEN 'ranger' THEN 'archer' WHEN 'rogue' THEN 'archer' ELSE archetype END \
         WHERE archetype IN ('warrior', 'novice', 'ranger', 'rogue')",
    )
    .execute(pool)
    .await?;

    Ok(())
}
