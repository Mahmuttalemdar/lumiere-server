use dashmap::DashMap;
use lumiere_models::snowflake::Snowflake;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::mpsc;

use crate::protocol::GatewayMessage;

/// Manages all active gateway sessions on this instance
pub struct SessionManager {
    /// session_id → active session
    pub sessions: DashMap<String, Arc<GatewaySession>>,
    /// user_id → list of session_ids (multi-device)
    pub user_sessions: DashMap<u64, Vec<String>>,
}

pub struct GatewaySession {
    pub session_id: String,
    pub user_id: Snowflake,
    pub sequence: AtomicU64,
    pub sender: mpsc::UnboundedSender<GatewayMessage>,
    pub last_heartbeat: AtomicU64,
    pub connected_at: std::time::Instant,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            user_sessions: DashMap::new(),
        }
    }

    pub fn register(&self, session: Arc<GatewaySession>) {
        let session_id = session.session_id.clone();
        let user_id = session.user_id.value();

        self.sessions.insert(session_id.clone(), session);
        self.user_sessions
            .entry(user_id)
            .or_default()
            .push(session_id);
    }

    pub fn remove(&self, session_id: &str) -> Option<Arc<GatewaySession>> {
        if let Some((_, session)) = self.sessions.remove(session_id) {
            let user_id = session.user_id.value();
            if let Some(mut sessions) = self.user_sessions.get_mut(&user_id) {
                sessions.retain(|s| s != session_id);
                if sessions.is_empty() {
                    drop(sessions);
                    self.user_sessions.remove(&user_id);
                }
            }
            Some(session)
        } else {
            None
        }
    }

    pub fn get(&self, session_id: &str) -> Option<Arc<GatewaySession>> {
        self.sessions.get(session_id).map(|s| Arc::clone(&s))
    }

    /// Send event to all sessions of a user
    pub fn dispatch_to_user(&self, user_id: u64, msg: GatewayMessage) {
        if let Some(session_ids) = self.user_sessions.get(&user_id) {
            for sid in session_ids.iter() {
                if let Some(session) = self.sessions.get(sid) {
                    let _ = session.sender.send(msg.clone());
                }
            }
        }
    }

    pub fn next_sequence(session: &GatewaySession) -> u64 {
        session.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
