//! Operator actions behind the admin HTTP API (`api::admin_handlers`):
//! server-wide announcements, the restart countdown, saving every session.

use crate::session::{FlushReport, SessionRegistry, SessionSignal, StateFlusher};
use crate::world::chat;
use crate::db::Db;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::info;

/// Longest announcement, in characters.
pub const ANNOUNCE_MAX_CHARS: usize = 300;
/// Longest restart countdown (minutes).
pub const MAX_COUNTDOWN_MINUTES: u32 = 24 * 60;
/// Minutes before the restart that get an announcement (besides the first).
const MARKS: [u32; 10] = [120, 60, 30, 15, 10, 5, 4, 3, 2, 1];

/// Minutes left at each announcement of a countdown from `minutes`, then
/// 0 (the restart itself): `minutes` right away, every mark below it.
/// `countdown_marks(30)` = 30, 15, 10, 5, 4, 3, 2, 1, 0.
pub fn countdown_marks(minutes: u32) -> Vec<u32> {
    let mut marks = vec![minutes];
    marks.extend(MARKS.iter().copied().filter(|&m| m < minutes));
    marks.push(0);
    marks
}

/// The countdown announcement with `left` minutes to go.
pub fn countdown_text(left: u32, note: Option<&str>) -> String {
    let base = match left {
        0 => "The server is restarting now.".to_string(),
        1 => "Server restart in 1 minute.".to_string(),
        n => format!("Server restart in {n} minutes."),
    };
    match note {
        Some(note) if left > 0 => format!("{base} {note}"),
        _ => base,
    }
}

/// Shared operator services (cheap to clone).
#[derive(Clone)]
pub struct Admin {
    inner: Arc<Inner>,
}

struct Inner {
    registry: SessionRegistry,
    pool: Db,
    /// Length of a countdown minute in ns (tests shorten it).
    minute_ns: AtomicU64,
    countdown: Mutex<Option<Countdown>>,
    next_id: AtomicU64,
}

struct Countdown {
    id: u64,
    minutes: u32,
    restart_at: chrono::DateTime<chrono::Utc>,
    /// Minutes left at each announcement still to come.
    pending: Arc<Mutex<Vec<u32>>>,
    task: JoinHandle<()>,
}

/// A scheduled restart countdown, as the API reports it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CountdownStatus {
    pub id: u64,
    pub minutes: u32,
    /// When the restart is due (UTC, RFC 3339).
    pub restart_at: String,
    /// Minutes left at each announcement still to come (0 = the restart).
    pub pending: Vec<u32>,
}

impl Admin {
    pub fn new(registry: SessionRegistry, pool: Db) -> Self {
        Self {
            inner: Arc::new(Inner {
                registry,
                pool,
                minute_ns: AtomicU64::new(60_000_000_000),
                countdown: Mutex::new(None),
                next_id: AtomicU64::new(1),
            }),
        }
    }

    /// Changes how long a countdown minute lasts (tests).
    pub fn set_minute(&self, minute: Duration) {
        self.inner.minute_ns.store(minute.as_nanos() as u64, Ordering::Relaxed);
    }

    fn minute(&self) -> Duration {
        Duration::from_nanos(self.inner.minute_ns.load(Ordering::Relaxed))
    }

    /// Sends an announcement to every connected socket; returns how many
    /// got it. `None` when the text is empty after cleaning.
    pub fn broadcast(&self, text: &str) -> Option<usize> {
        let text = chat::clean(text, ANNOUNCE_MAX_CHARS)?;
        Some(announce(&self.inner.registry, &text))
    }

    /// Writes every active session's unsaved character to the database now.
    pub async fn save_all(&self) -> FlushReport {
        let report = StateFlusher::flush_all(&self.inner.pool, &self.inner.registry).await;
        info!("Admin save: {} saved, {} failed, {} sessions", report.saved, report.failed, report.sessions);
        report
    }

    /// Schedules the restart countdown (replacing one already running):
    /// announcements at [`countdown_marks`], and at 0 a last one and a
    /// save of every session. `note` is appended to each announcement.
    pub fn schedule_restart(&self, minutes: u32, note: Option<&str>) -> Result<CountdownStatus, &'static str> {
        if minutes == 0 || minutes > MAX_COUNTDOWN_MINUTES {
            return Err("minutes must be between 1 and 1440");
        }
        let note = note.and_then(|n| chat::clean(n, ANNOUNCE_MAX_CHARS / 2));
        let marks = countdown_marks(minutes);
        let pending = Arc::new(Mutex::new(marks.clone()));
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let minute = self.minute();
        let start = Instant::now();
        let restart_at = chrono::Utc::now() + chrono::Duration::from_std(minute * minutes).unwrap_or_default();
        // Held until the countdown is in place, so a countdown that ends
        // right away still clears its own slot.
        let mut slot = self.inner.countdown.lock().unwrap();
        let task = {
            let admin = self.clone();
            let pending = Arc::clone(&pending);
            tokio::spawn(async move {
                for left in marks {
                    tokio::time::sleep_until(start + minute * (minutes - left)).await;
                    pending.lock().unwrap().retain(|&m| m != left);
                    let n = announce(&admin.inner.registry, &countdown_text(left, note.as_deref()));
                    info!("Restart countdown: {left} min left, announced to {n} sockets");
                    if left == 0 {
                        admin.save_all().await;
                    }
                }
                let mut slot = admin.inner.countdown.lock().unwrap();
                if slot.as_ref().is_some_and(|c| c.id == id) {
                    *slot = None;
                }
            })
        };
        let countdown = Countdown { id, minutes, restart_at, pending, task };
        let status = countdown.status();
        if let Some(old) = slot.replace(countdown) {
            old.task.abort();
        }
        info!("Restart countdown scheduled: {minutes} min");
        Ok(status)
    }

    /// Stops the running countdown and tells everyone; `false` when none
    /// was running.
    pub fn cancel_restart(&self) -> bool {
        let Some(old) = self.inner.countdown.lock().unwrap().take() else { return false };
        old.task.abort();
        announce(&self.inner.registry, "The server restart was cancelled.");
        info!("Restart countdown cancelled");
        true
    }

    /// The running countdown, if any.
    pub fn restart_status(&self) -> Option<CountdownStatus> {
        self.inner.countdown.lock().unwrap().as_ref().map(Countdown::status)
    }
}

impl Countdown {
    fn status(&self) -> CountdownStatus {
        CountdownStatus {
            id: self.id,
            minutes: self.minutes,
            restart_at: self.restart_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            pending: self.pending.lock().unwrap().clone(),
        }
    }
}

/// Signals an announcement to every session; counts the sockets reached.
fn announce(registry: &SessionRegistry, text: &str) -> usize {
    registry
        .all_sessions()
        .iter()
        .filter(|s| s.signal_tx.send(SessionSignal::Announce { text: text.to_string() }).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_marks_follow_the_usual_schedule() {
        assert_eq!(countdown_marks(30), vec![30, 15, 10, 5, 4, 3, 2, 1, 0]);
        assert_eq!(countdown_marks(7), vec![7, 5, 4, 3, 2, 1, 0]);
        assert_eq!(countdown_marks(1), vec![1, 0]);
        assert_eq!(countdown_marks(90), vec![90, 60, 30, 15, 10, 5, 4, 3, 2, 1, 0]);
        assert_eq!(countdown_marks(240), vec![240, 120, 60, 30, 15, 10, 5, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn countdown_texts() {
        assert_eq!(countdown_text(30, None), "Server restart in 30 minutes.");
        assert_eq!(countdown_text(1, Some("Patch 1.2.")), "Server restart in 1 minute. Patch 1.2.");
        assert_eq!(countdown_text(0, Some("Patch 1.2.")), "The server is restarting now.");
    }
}
