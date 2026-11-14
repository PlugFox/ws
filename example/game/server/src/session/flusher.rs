use super::registry::{ActiveSession, SessionRegistry};
use crate::db::character_repo::CharacterRepo;
use crate::db::leaderboard_repo::LeaderboardRepo;
use crate::db::Db;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info};

pub struct StateFlusher;

/// What [`StateFlusher::flush_all`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct FlushReport {
    /// Active sessions looked at.
    pub sessions: usize,
    /// Dirty characters written.
    pub saved: usize,
    /// Writes that failed (the character stays dirty for the next pass).
    pub failed: usize,
}

impl StateFlusher {
    /// Immediately flushes the dirty character state of an active session to the database.
    pub async fn flush_session(
        pool: &Db,
        session: &ActiveSession,
    ) -> Result<bool, sqlx::Error> {
        let mut char_lock = session.character.write().await;
        if let Some(ref mut character) = *char_lock {
            if character.is_dirty {
                CharacterRepo::update_dirty_state(
                    pool,
                    character.id,
                    character.level,
                    character.base_exp,
                    &character.stats,
                    &character.unlocked_skills,
                    &character.last_location_id,
                    character.look,
                    &character.loadout,
                    &character.bag,
                )
                .await?;
                LeaderboardRepo::store_record(pool, character.id, &character.record).await?;

                character.mark_clean();
                debug!(
                    "Flushed dirty character {} (account {}) to database",
                    character.id, session.user_id
                );
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Flushes every active session's dirty character now (the periodic
    /// worker, the admin `save` endpoint). Safe to run concurrently with
    /// itself and with per-session flushes: each character is written under
    /// its write lock, so a character is saved once and the late caller
    /// finds it clean.
    pub async fn flush_all(pool: &Db, registry: &SessionRegistry) -> FlushReport {
        let sessions = registry.all_sessions();
        let mut report = FlushReport { sessions: sessions.len(), ..FlushReport::default() };
        for session in sessions {
            match Self::flush_session(pool, &session).await {
                Ok(true) => report.saved += 1,
                Ok(false) => {}
                Err(err) => {
                    report.failed += 1;
                    error!("Failed to flush dirty state for account {}: {}", session.user_id, err);
                }
            }
        }
        report
    }

    /// Spawns a background asynchronous task that periodically scans all active sessions
    /// and persists dirty character state to the database.
    pub fn spawn_periodic_worker(
        pool: Db,
        registry: SessionRegistry,
        interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!(
                "Starting background persistence flusher with interval {:?}",
                interval
            );
            loop {
                sleep(interval).await;
                let report = Self::flush_all(&pool, &registry).await;
                if report.saved > 0 {
                    debug!("Periodic flusher saved {} dirty characters", report.saved);
                }
            }
        })
    }
}
