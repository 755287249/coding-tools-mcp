use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::tools::workspace::WorkspaceError;

use super::lifecycle::prune_finalized_sessions;
use super::{ExecSession, SessionRegistry, SessionStore, DEFAULT_ACTIVE_SESSION_LIMIT};

impl Default for SessionStore {
    fn default() -> Self {
        Self {
            registry: Mutex::new(SessionRegistry::default()),
            active_slots: Arc::new(Semaphore::new(DEFAULT_ACTIVE_SESSION_LIMIT)),
            active_session_limit: DEFAULT_ACTIVE_SESSION_LIMIT,
        }
    }
}

impl SessionStore {
    pub fn close_admission(&self) {
        self.active_slots.close();
    }
    pub fn is_closed(&self) -> bool {
        self.active_slots.is_closed()
    }
    pub async fn shutdown(&self) {
        self.close_admission();
        for session in self.list(false, usize::MAX) {
            session.mark_termination_reason("project_stopped");
            session.kill_and_wait().await;
        }
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_active_session_limit(limit: usize) -> Self {
        let limit = limit.clamp(1, u16::MAX as usize);
        Self {
            registry: Mutex::new(SessionRegistry::default()),
            active_slots: Arc::new(Semaphore::new(limit)),
            active_session_limit: limit,
        }
    }

    pub fn insert(&self, session: ExecSession) -> Arc<ExecSession> {
        let arc = Arc::new(session);
        let mut registry = self.registry.lock().expect("sessions registry lock");
        prune_finalized_sessions(&mut registry);
        if let Some(operation_id) = arc.operation_id.as_ref() {
            registry
                .operation_index
                .insert(operation_id.clone(), arc.session_id.clone());
        }
        if let Some(fingerprint) = arc.command_fingerprint.as_ref() {
            registry
                .fingerprint_index
                .insert(fingerprint.clone(), arc.session_id.clone());
        }
        registry
            .sessions
            .insert(arc.session_id.clone(), arc.clone());
        if self.is_closed() {
            let stopped = arc.clone();
            tokio::spawn(async move {
                stopped.mark_termination_reason("project_stopped");
                stopped.kill_and_wait().await;
            });
        }
        arc
    }

    pub async fn acquire_active_slot(&self) -> Result<OwnedSemaphorePermit, WorkspaceError> {
        match tokio::time::timeout(
            Duration::from_secs(1),
            self.active_slots.clone().acquire_owned(),
        )
        .await
        {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(error)) => Err(WorkspaceError::ToolDetails {
                code: "SESSION_ADMISSION_CLOSED",
                message: format!("Session admission closed: {error}"),
                category: "runtime",
                retryable: true,
                details: json!({
                    "stage": "session_admission",
                    "active_session_limit": self.active_session_limit,
                    "suggestion": "重启 MCP 运行时后重试"
                }),
            }),
            Err(_) => Err(WorkspaceError::ToolDetails {
                code: "SESSION_LIMIT_REACHED",
                message: format!(
                    "Active command session limit reached ({}).",
                    self.active_session_limit
                ),
                category: "runtime",
                retryable: true,
                details: json!({
                    "stage": "session_admission",
                    "active_session_limit": self.active_session_limit,
                    "active_session_slots_available": self.active_slots.available_permits(),
                    "suggestion": "等待现有命令完成，或使用 kill_session 终止不再需要的长任务"
                }),
            }),
        }
    }

    pub fn active_session_limit(&self) -> usize {
        self.active_session_limit
    }

    pub fn active_slots_available(&self) -> usize {
        self.active_slots.available_permits()
    }

    pub fn get(&self, session_id: &str) -> Result<Arc<ExecSession>, WorkspaceError> {
        self.get_with_metrics(session_id)
            .map(|(session, _)| session)
    }

    pub fn get_with_metrics(
        &self,
        session_id: &str,
    ) -> Result<(Arc<ExecSession>, u128), WorkspaceError> {
        let started = Instant::now();
        let registry = self.registry.lock().expect("sessions registry lock");
        let lock_wait_ms = started.elapsed().as_millis();
        registry
            .sessions
            .get(session_id)
            .cloned()
            .map(|session| (session, lock_wait_ms))
            .ok_or_else(|| WorkspaceError::Tool {
                code: "SESSION_NOT_FOUND",
                message: format!("Session not found: {session_id}"),
                category: "not_found",
                retryable: false,
            })
    }

    pub fn get_by_operation(&self, operation_id: &str) -> Option<Arc<ExecSession>> {
        let mut registry = self.registry.lock().expect("sessions registry lock");
        prune_finalized_sessions(&mut registry);
        let session_id = registry.operation_index.get(operation_id)?.clone();
        registry.sessions.get(&session_id).cloned()
    }

    pub fn get_by_fingerprint(&self, fingerprint: &str) -> Option<Arc<ExecSession>> {
        let mut registry = self.registry.lock().expect("sessions registry lock");
        prune_finalized_sessions(&mut registry);
        let session_id = registry.fingerprint_index.get(fingerprint)?.clone();
        registry.sessions.get(&session_id).cloned()
    }

    pub fn list(&self, include_finalized: bool, limit: usize) -> Vec<Arc<ExecSession>> {
        let mut registry = self.registry.lock().expect("sessions registry lock");
        prune_finalized_sessions(&mut registry);
        let mut sessions = registry
            .sessions
            .values()
            .filter(|session| include_finalized || !session.is_finalized())
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.started_ts_ms));
        sessions.truncate(limit);
        sessions
    }

    pub fn contains(&self, session_id: &str) -> bool {
        let registry = self.registry.lock().expect("sessions registry lock");
        registry.sessions.contains_key(session_id)
    }

    pub fn remove(&self, session_id: &str) {
        let mut registry = self.registry.lock().expect("sessions registry lock");
        registry.sessions.remove(session_id);
        registry
            .operation_index
            .retain(|_, indexed_session_id| indexed_session_id != session_id);
        registry
            .fingerprint_index
            .retain(|_, indexed_session_id| indexed_session_id != session_id);
    }
}
