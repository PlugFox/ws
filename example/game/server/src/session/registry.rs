use super::state::{CharacterState, SessionSignal};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};
use uuid::Uuid;

/// In-memory representation of an active connected player session.
pub struct ActiveSession {
    pub session_id: Uuid,
    pub user_id: u32,
    pub signal_tx: broadcast::Sender<SessionSignal>,
    pub character: Arc<RwLock<Option<CharacterState>>>,
    pub ip_address: Option<String>,
    pub country: Option<String>,
    pub connected_at: Instant,
    /// Where the character stood in town when its last socket closed: a
    /// reconnect of this session lands there again (`ws_handlers`).
    pub town_spot: std::sync::Mutex<Option<TownSpot>>,
}

/// A remembered spot in town (in memory: short drops, not relogs).
#[derive(Clone, Debug, PartialEq)]
pub struct TownSpot {
    pub character_id: u32,
    pub channel: u16,
    pub arrival: crate::world::Arrival,
    pub at: Instant,
}

impl ActiveSession {
    pub fn new(
        session_id: Uuid,
        user_id: u32,
        ip_address: Option<String>,
        country: Option<String>,
    ) -> (Arc<Self>, broadcast::Receiver<SessionSignal>) {
        let (signal_tx, signal_rx) = broadcast::channel(16);
        let session = Arc::new(Self {
            session_id,
            user_id,
            signal_tx,
            character: Arc::new(RwLock::new(None)),
            ip_address,
            country,
            connected_at: Instant::now(),
            town_spot: std::sync::Mutex::new(None),
        });
        (session, signal_rx)
    }

    /// Dispatches a displacement signal to connected sockets.
    pub fn displace(&self, reason: &str) {
        let signal = SessionSignal::Displaced {
            reason: reason.to_string(),
        };
        let _ = self.signal_tx.send(signal);
    }
}

/// Thread-safe registry mapping `account_id` (u32) to `ActiveSession`.
/// Enforces single active session per account across HTTP, WebSocket, and WebTransport.
#[derive(Clone, Default)]
pub struct SessionRegistry {
    sessions: Arc<DashMap<u32, Arc<ActiveSession>>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
        }
    }

    /// Registers a session for a user. If an active session already exists for this account,
    /// it triggers displacement on the old session and returns it for state flushing and DB revocation.
    pub fn register_or_displace(
        &self,
        new_session: Arc<ActiveSession>,
    ) -> Option<Arc<ActiveSession>> {
        let user_id = new_session.user_id;
        let old_session = self.sessions.insert(user_id, new_session);

        if let Some(ref old) = old_session {
            warn!(
                "Account {} already had an active session ({}); displacing for new session",
                user_id, old.session_id
            );
            old.displace("New session established from another connection or tab");
        } else {
            info!("Account {} registered active session", user_id);
        }

        old_session
    }

    /// Retrieves the active session for a given user if online.
    pub fn get(&self, user_id: u32) -> Option<Arc<ActiveSession>> {
        self.sessions.get(&user_id).map(|entry| entry.value().clone())
    }

    /// Removes an active session by user_id if it matches the specified session_id.
    pub fn remove_if_match(&self, user_id: u32, session_id: &Uuid) -> Option<Arc<ActiveSession>> {
        if let Some(entry) = self.sessions.get(&user_id) {
            if entry.value().session_id == *session_id {
                drop(entry);
                return self.sessions.remove(&user_id).map(|(_, s)| s);
            }
        }
        None
    }

    /// Removes an active session by user_id unconditionally.
    pub fn remove(&self, user_id: u32) -> Option<Arc<ActiveSession>> {
        self.sessions.remove(&user_id).map(|(_, s)| s)
    }

    /// Returns the number of currently active sessions.
    pub fn active_count(&self) -> usize {
        self.sessions.len()
    }

    /// Provides access to all active sessions (e.g. for background persistence flushing).
    pub fn all_sessions(&self) -> Vec<Arc<ActiveSession>> {
        self.sessions.iter().map(|entry| entry.value().clone()).collect()
    }
}
