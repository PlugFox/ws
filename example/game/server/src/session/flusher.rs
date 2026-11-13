use super::registry::{ActiveSession, SessionRegistry};
use crate::db::character_repo::CharacterRepo;
use sqlx::{Pool, Sqlite};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info};

pub struct StateFlusher;

impl StateFlusher {
    /// Immediately flushes the dirty character state of an active session to the database.
    pub async fn flush_session(
        pool: &Pool<Sqlite>,
        session: &ActiveSession,
    ) -> Result<bool, sqlx::Error> {
        let mut char_lock = session.character.write().await;
        if let Some(ref mut character) = *char_lock {
            if character.is_dirty {
                let stats_str = character.stats.to_string();
                let skills_str = character.unlocked_skills.to_string();
                let inv_str = character.inventory.to_string();

                CharacterRepo::update_dirty_state(
                    pool,
                    character.id,
                    character.level,
                    character.base_exp,
                    &stats_str,
                    &skills_str,
                    &inv_str,
                    &character.last_location_id,
                    character.look,
                )
                .await?;

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

    /// Spawns a background asynchronous task that periodically scans all active sessions
    /// and persists dirty character state to the database.
    pub fn spawn_periodic_worker(
        pool: Pool<Sqlite>,
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
                let sessions = registry.all_sessions();
                let mut flushed_count = 0;

                for session in sessions {
                    match Self::flush_session(&pool, &session).await {
                        Ok(true) => {
                            flushed_count += 1;
                        }
                        Ok(false) => {}
                        Err(err) => {
                            error!(
                                "Failed to flush dirty state for account {}: {}",
                                session.user_id, err
                            );
                        }
                    }
                }

                if flushed_count > 0 {
                    debug!("Periodic flusher saved {} dirty characters", flushed_count);
                }
            }
        })
    }
}
