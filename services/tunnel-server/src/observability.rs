use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::params;
use serde::Serialize;

use crate::database::DatabaseWriter;

const LOG_CAPACITY: usize = 2_000;
const ACTIVITY_CAPACITY: usize = 500;

#[derive(Clone)]
pub struct Observability {
    inner: Arc<Inner>,
}

struct Inner {
    started_at: Instant,
    started_at_unix_ms: u64,
    logs: Mutex<VecDeque<LogEntry>>,
    database: Option<DatabaseWriter>,
    activities: Mutex<VecDeque<ActivityEntry>>,
    workers: Mutex<HashMap<String, WorkerSnapshot>>,
    total_requests: AtomicU64,
    successful_requests: AtomicU64,
    failed_requests: AtomicU64,
    in_flight_requests: AtomicUsize,
    queued_requests: AtomicUsize,
    peak_queued_requests: AtomicUsize,
    capacity_rejections: AtomicU64,
    worker_acquire_timeouts: AtomicU64,
    assigned_requests: AtomicU64,
    queue_wait_total_ms: AtomicU64,
    max_queue_wait_ms: AtomicU64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    pub id: u64,
    pub timestamp_unix_ms: u64,
    pub level: String,
    pub category: String,
    pub message: String,
    pub client_id: Option<String>,
    pub service: Option<String>,
    pub worker_id: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ActivityEntry {
    pub id: u64,
    pub timestamp_unix_ms: u64,
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub level: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkerSnapshot {
    pub worker_id: String,
    pub device_id: String,
    pub client_id: String,
    pub service: String,
    pub state: String,
    pub connected_at_unix_ms: u64,
    pub last_seen_at_unix_ms: u64,
    pub requests_completed: u64,
    pub last_error: Option<String>,
    pub agent_version: Option<String>,
    pub client_compat_version: Option<String>,
    pub build_git_sha: Option<String>,
    pub update_id: Option<String>,
    pub update_target_version: Option<String>,
    pub update_state: Option<String>,
    pub update_message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DashboardSnapshot {
    pub status: String,
    pub started_at_unix_ms: u64,
    pub uptime_seconds: u64,
    pub connected_workers: usize,
    pub idle_workers: usize,
    pub busy_workers: usize,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub in_flight_requests: usize,
    pub queued_requests: usize,
    pub peak_queued_requests: usize,
    pub capacity_rejections: u64,
    pub worker_acquire_timeouts: u64,
    pub average_queue_wait_ms: f64,
    pub max_queue_wait_ms: u64,
    pub error_rate_percent: f64,
    pub retained_logs: usize,
    pub retained_activities: usize,
}

#[derive(Default)]
pub struct LogFilter<'a> {
    pub query: Option<&'a str>,
    pub level: Option<&'a str>,
    pub service: Option<&'a str>,
    pub client_id: Option<&'a str>,
    pub scope: Option<&'a str>,
    pub limit: usize,
}

pub struct RequestGuard {
    observability: Observability,
    request_id: String,
    method: String,
    path: String,
    client_id: String,
    service: String,
    started_at: Instant,
    finished: bool,
}

pub struct WorkerGuard {
    observability: Observability,
    worker_id: String,
    disconnected: bool,
}

impl Default for Observability {
    fn default() -> Self {
        Self::new()
    }
}

impl Observability {
    pub fn new() -> Self {
        Self::from_parts(VecDeque::with_capacity(LOG_CAPACITY), None)
    }

    pub fn from_database(database: DatabaseWriter) -> Result<Self, String> {
        let logs = database
            .call(|connection| -> rusqlite::Result<VecDeque<LogEntry>> {
                connection.execute_batch(
                    "CREATE TABLE IF NOT EXISTS observability_logs (
                        id INTEGER PRIMARY KEY,
                        timestamp_unix_ms INTEGER NOT NULL,
                        level TEXT NOT NULL,
                        category TEXT NOT NULL,
                        message TEXT NOT NULL,
                        client_id TEXT,
                        service TEXT,
                        worker_id TEXT,
                        request_id TEXT
                    );
                    CREATE INDEX IF NOT EXISTS observability_logs_timestamp
                        ON observability_logs(timestamp_unix_ms DESC);",
                )?;
                let mut statement = connection.prepare(
                    "SELECT id, timestamp_unix_ms, level, category, message,
                            client_id, service, worker_id, request_id
                     FROM observability_logs
                     ORDER BY id DESC
                     LIMIT ?1",
                )?;
                let entries = statement
                    .query_map([LOG_CAPACITY as i64], |row| {
                        Ok(LogEntry {
                            id: row.get::<_, i64>(0)?.max(0) as u64,
                            timestamp_unix_ms: row.get::<_, i64>(1)?.max(0) as u64,
                            level: row.get(2)?,
                            category: row.get(3)?,
                            message: row.get(4)?,
                            client_id: row.get(5)?,
                            service: row.get(6)?,
                            worker_id: row.get(7)?,
                            request_id: row.get(8)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(entries.into_iter().rev().collect())
            })
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        Ok(Self::from_parts(logs, Some(database)))
    }

    fn from_parts(logs: VecDeque<LogEntry>, database: Option<DatabaseWriter>) -> Self {
        Self {
            inner: Arc::new(Inner {
                started_at: Instant::now(),
                started_at_unix_ms: now_unix_ms(),
                logs: Mutex::new(logs),
                database,
                activities: Mutex::new(VecDeque::with_capacity(ACTIVITY_CAPACITY)),
                workers: Mutex::new(HashMap::new()),
                total_requests: AtomicU64::new(0),
                successful_requests: AtomicU64::new(0),
                failed_requests: AtomicU64::new(0),
                in_flight_requests: AtomicUsize::new(0),
                queued_requests: AtomicUsize::new(0),
                peak_queued_requests: AtomicUsize::new(0),
                capacity_rejections: AtomicU64::new(0),
                worker_acquire_timeouts: AtomicU64::new(0),
                assigned_requests: AtomicU64::new(0),
                queue_wait_total_ms: AtomicU64::new(0),
                max_queue_wait_ms: AtomicU64::new(0),
            }),
        }
    }

    pub fn dashboard(&self) -> DashboardSnapshot {
        let workers = self
            .inner
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let connected_workers = workers.len();
        let idle_workers = workers
            .values()
            .filter(|worker| worker.state == "idle")
            .count();
        let busy_workers = workers
            .values()
            .filter(|worker| worker.state == "busy")
            .count();
        let total_requests = self.inner.total_requests.load(Ordering::Relaxed);
        let failed_requests = self.inner.failed_requests.load(Ordering::Relaxed);
        let assigned_requests = self.inner.assigned_requests.load(Ordering::Relaxed);
        let queue_wait_total_ms = self.inner.queue_wait_total_ms.load(Ordering::Relaxed);
        DashboardSnapshot {
            status: if connected_workers > 0 || total_requests == 0 {
                "healthy"
            } else {
                "degraded"
            }
            .into(),
            started_at_unix_ms: self.inner.started_at_unix_ms,
            uptime_seconds: self.inner.started_at.elapsed().as_secs(),
            connected_workers,
            idle_workers,
            busy_workers,
            total_requests,
            successful_requests: self.inner.successful_requests.load(Ordering::Relaxed),
            failed_requests,
            in_flight_requests: self.inner.in_flight_requests.load(Ordering::Relaxed),
            queued_requests: self.inner.queued_requests.load(Ordering::Relaxed),
            peak_queued_requests: self.inner.peak_queued_requests.load(Ordering::Relaxed),
            capacity_rejections: self.inner.capacity_rejections.load(Ordering::Relaxed),
            worker_acquire_timeouts: self.inner.worker_acquire_timeouts.load(Ordering::Relaxed),
            average_queue_wait_ms: if assigned_requests == 0 {
                0.0
            } else {
                queue_wait_total_ms as f64 / assigned_requests as f64
            },
            max_queue_wait_ms: self.inner.max_queue_wait_ms.load(Ordering::Relaxed),
            error_rate_percent: if total_requests == 0 {
                0.0
            } else {
                failed_requests as f64 * 100.0 / total_requests as f64
            },
            retained_logs: self
                .inner
                .logs
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            retained_activities: self
                .inner
                .activities
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
        }
    }

    pub fn workers(&self) -> Vec<WorkerSnapshot> {
        let mut workers = self
            .inner
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        workers.sort_by_key(|worker| std::cmp::Reverse(worker.connected_at_unix_ms));
        workers
    }

    pub fn activities(&self, limit: usize) -> Vec<ActivityEntry> {
        self.inner
            .activities
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .rev()
            .take(limit.clamp(1, 200))
            .cloned()
            .collect()
    }

    pub fn logs(&self, filter: LogFilter<'_>) -> Vec<LogEntry> {
        let query = filter
            .query
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_lowercase);
        let level = filter
            .level
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "all")
            .map(str::to_lowercase);
        let service = filter
            .service
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "all")
            .map(str::to_lowercase);
        let client_id = filter
            .client_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_lowercase);
        let scope = filter
            .scope
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "all")
            .map(str::to_lowercase);
        self.inner
            .logs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .rev()
            .filter(|entry| {
                level
                    .as_ref()
                    .is_none_or(|value| entry.level.eq_ignore_ascii_case(value))
                    && service.as_ref().is_none_or(|value| {
                        entry
                            .service
                            .as_deref()
                            .is_some_and(|entry_value| entry_value.eq_ignore_ascii_case(value))
                    })
                    && match scope.as_deref() {
                        Some("system") => entry.client_id.is_none(),
                        Some("client") => client_id.as_ref().is_some_and(|value| {
                            entry
                                .client_id
                                .as_deref()
                                .is_some_and(|entry_value| entry_value.eq_ignore_ascii_case(value))
                        }),
                        _ => client_id.as_ref().is_none_or(|value| {
                            entry.client_id.as_deref().is_some_and(|entry_value| {
                                entry_value.to_lowercase().contains(value)
                            })
                        }),
                    }
                    && query.as_ref().is_none_or(|value| {
                        entry.message.to_lowercase().contains(value)
                            || entry.category.to_lowercase().contains(value)
                            || entry
                                .client_id
                                .as_deref()
                                .is_some_and(|field| field.to_lowercase().contains(value))
                            || entry
                                .worker_id
                                .as_deref()
                                .is_some_and(|field| field.to_lowercase().contains(value))
                            || entry
                                .request_id
                                .as_deref()
                                .is_some_and(|field| field.to_lowercase().contains(value))
                    })
            })
            .take(filter.limit.clamp(1, 500))
            .cloned()
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn log(
        &self,
        level: &str,
        category: &str,
        message: impl Into<String>,
        client_id: Option<&str>,
        service: Option<&str>,
        worker_id: Option<&str>,
        request_id: Option<&str>,
    ) {
        let timestamp = now_unix_ms();
        let mut logs = self
            .inner
            .logs
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let id = logs.back().map_or(1, |entry| entry.id.saturating_add(1));
        let entry = LogEntry {
            id,
            timestamp_unix_ms: timestamp,
            level: level.to_lowercase(),
            category: category.into(),
            message: message.into(),
            client_id: client_id.map(str::to_owned),
            service: service.map(str::to_owned),
            worker_id: worker_id.map(str::to_owned),
            request_id: request_id.map(str::to_owned),
        };
        push_bounded(&mut logs, LOG_CAPACITY, entry.clone());
        drop(logs);

        if let Some(database) = self.inner.database.as_ref() {
            if let Err(error) = database.enqueue(move |connection| persist_log(connection, &entry))
            {
                tracing::warn!(%error, "could not enqueue observability log persistence");
            }
        }
    }

    pub fn activity(
        &self,
        kind: &str,
        level: &str,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) {
        let mut activities = self
            .inner
            .activities
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let id = activities
            .back()
            .map_or(1, |entry| entry.id.saturating_add(1));
        push_bounded(
            &mut activities,
            ACTIVITY_CAPACITY,
            ActivityEntry {
                id,
                timestamp_unix_ms: now_unix_ms(),
                kind: kind.into(),
                title: title.into(),
                detail: detail.into(),
                level: level.into(),
            },
        );
    }

    pub fn connect_worker(
        &self,
        worker_id: &str,
        device_id: &str,
        client_id: &str,
        service: &str,
    ) -> WorkerGuard {
        let now = now_unix_ms();
        self.inner
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                worker_id.into(),
                WorkerSnapshot {
                    worker_id: worker_id.into(),
                    device_id: device_id.into(),
                    client_id: client_id.into(),
                    service: service.into(),
                    state: "connecting".into(),
                    connected_at_unix_ms: now,
                    last_seen_at_unix_ms: now,
                    requests_completed: 0,
                    last_error: None,
                    agent_version: None,
                    client_compat_version: None,
                    build_git_sha: None,
                    update_id: None,
                    update_target_version: None,
                    update_state: None,
                    update_message: None,
                },
            );
        self.log(
            "info",
            "worker",
            "worker connected",
            Some(client_id),
            Some(service),
            Some(worker_id),
            None,
        );
        self.activity(
            "worker_connected",
            "info",
            "Worker 已連線",
            format!("{client_id} / {service} / {worker_id}"),
        );
        WorkerGuard {
            observability: self.clone(),
            worker_id: worker_id.into(),
            disconnected: false,
        }
    }

    pub fn worker_client_status(
        &self,
        worker_id: &str,
        agent_version: &str,
        client_compat_version: &str,
        build_git_sha: &str,
    ) {
        if let Some(worker) = self
            .inner
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(worker_id)
        {
            worker.agent_version = Some(agent_version.into());
            worker.client_compat_version = Some(client_compat_version.into());
            worker.build_git_sha = Some(build_git_sha.into());
            worker.last_seen_at_unix_ms = now_unix_ms();
        }
    }

    pub fn worker_update_status(
        &self,
        worker_id: &str,
        update_id: &str,
        version: &str,
        state: &str,
        message: Option<&str>,
    ) {
        if let Some(worker) = self
            .inner
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(worker_id)
        {
            worker.update_id = Some(update_id.into());
            worker.update_target_version = Some(version.into());
            worker.update_state = Some(state.into());
            worker.update_message = message.map(str::to_owned);
            worker.last_seen_at_unix_ms = now_unix_ms();
        }
    }

    pub fn worker_state(&self, worker_id: &str, state: &str) {
        if let Some(worker) = self
            .inner
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(worker_id)
        {
            worker.state = state.into();
            worker.last_seen_at_unix_ms = now_unix_ms();
        }
    }

    pub fn worker_error(&self, worker_id: &str, message: &str) {
        if let Some(worker) = self
            .inner
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(worker_id)
        {
            worker.last_error = Some(message.into());
            worker.last_seen_at_unix_ms = now_unix_ms();
        }
    }

    pub fn worker_completed_request(&self, worker_id: &str) {
        if let Some(worker) = self
            .inner
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(worker_id)
        {
            worker.requests_completed = worker.requests_completed.saturating_add(1);
            worker.last_seen_at_unix_ms = now_unix_ms();
        }
    }

    pub fn queue_enter(&self) {
        let current = self
            .inner
            .queued_requests
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        self.inner
            .peak_queued_requests
            .fetch_max(current, Ordering::Relaxed);
    }

    pub fn queue_exit(&self) {
        let _ = self.inner.queued_requests.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |current| Some(current.saturating_sub(1)),
        );
    }

    pub fn record_capacity_rejection(&self) {
        self.inner
            .capacity_rejections
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_worker_acquire_timeout(&self) {
        self.inner
            .worker_acquire_timeouts
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_worker_assignment(&self, queue_wait_ms: u64) {
        self.inner.assigned_requests.fetch_add(1, Ordering::Relaxed);
        self.inner
            .queue_wait_total_ms
            .fetch_add(queue_wait_ms, Ordering::Relaxed);
        self.inner
            .max_queue_wait_ms
            .fetch_max(queue_wait_ms, Ordering::Relaxed);
    }

    pub fn begin_request(
        &self,
        request_id: &str,
        method: &str,
        path: &str,
        client_id: &str,
        service: &str,
    ) -> RequestGuard {
        self.inner.total_requests.fetch_add(1, Ordering::Relaxed);
        self.inner
            .in_flight_requests
            .fetch_add(1, Ordering::Relaxed);
        self.log(
            "info",
            "request",
            format!("{method} {path}"),
            Some(client_id),
            Some(service),
            None,
            Some(request_id),
        );
        RequestGuard {
            observability: self.clone(),
            request_id: request_id.into(),
            method: method.into(),
            path: path.into(),
            client_id: client_id.into(),
            service: service.into(),
            started_at: Instant::now(),
            finished: false,
        }
    }

    fn disconnect_worker(&self, worker_id: &str) {
        if let Some(worker) = self
            .inner
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(worker_id)
        {
            self.log(
                "info",
                "worker",
                "worker disconnected",
                Some(&worker.client_id),
                Some(&worker.service),
                Some(worker_id),
                None,
            );
            self.activity(
                "worker_disconnected",
                "warning",
                "Worker 已離線",
                format!("{} / {} / {}", worker.client_id, worker.service, worker_id),
            );
        }
    }
}

impl RequestGuard {
    pub fn finish(&mut self, status: u16) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.observability
            .inner
            .in_flight_requests
            .fetch_sub(1, Ordering::Relaxed);
        let successful = status < 500;
        if successful {
            self.observability
                .inner
                .successful_requests
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.observability
                .inner
                .failed_requests
                .fetch_add(1, Ordering::Relaxed);
        }
        let duration_ms = self.started_at.elapsed().as_millis();
        self.observability.log(
            if successful { "info" } else { "error" },
            "request",
            format!(
                "{} {} -> {} ({} ms)",
                self.method, self.path, status, duration_ms
            ),
            Some(&self.client_id),
            Some(&self.service),
            None,
            Some(&self.request_id),
        );
        self.observability.activity(
            "request",
            if successful { "info" } else { "error" },
            format!("{} {}", self.method, self.path),
            format!(
                "{} / {} · HTTP {status} · {duration_ms} ms",
                self.client_id, self.service
            ),
        );
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.observability
                .inner
                .in_flight_requests
                .fetch_sub(1, Ordering::Relaxed);
            self.observability
                .inner
                .failed_requests
                .fetch_add(1, Ordering::Relaxed);
            self.observability.log(
                "error",
                "request",
                format!("{} {} ended without a response", self.method, self.path),
                Some(&self.client_id),
                Some(&self.service),
                None,
                Some(&self.request_id),
            );
        }
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if !self.disconnected {
            self.observability.disconnect_worker(&self.worker_id);
        }
    }
}

fn push_bounded<T>(queue: &mut VecDeque<T>, capacity: usize, value: T) {
    if queue.len() == capacity {
        queue.pop_front();
    }
    queue.push_back(value);
}

fn persist_log(connection: &mut rusqlite::Connection, entry: &LogEntry) -> Result<(), String> {
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT OR REPLACE INTO observability_logs (
                id, timestamp_unix_ms, level, category, message,
                client_id, service, worker_id, request_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                entry.id.min(i64::MAX as u64) as i64,
                entry.timestamp_unix_ms.min(i64::MAX as u64) as i64,
                &entry.level,
                &entry.category,
                &entry.message,
                entry.client_id.as_deref(),
                entry.service.as_deref(),
                entry.worker_id.as_deref(),
                entry.request_id.as_deref(),
            ],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM observability_logs
             WHERE id <= COALESCE((
                SELECT id FROM observability_logs
                ORDER BY id DESC
                LIMIT 1 OFFSET ?1
             ), 0)",
            [LOG_CAPACITY as i64],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_logs_and_tracks_request_totals() {
        let telemetry = Observability::new();
        telemetry.log(
            "warn",
            "worker",
            "connection failed",
            Some("pc-a"),
            Some("mcp"),
            Some("w1"),
            None,
        );
        telemetry.log("info", "server", "started", None, None, None, None);
        let filtered = telemetry.logs(LogFilter {
            query: Some("failed"),
            level: Some("warn"),
            service: Some("mcp"),
            client_id: Some("pc-a"),
            scope: Some("client"),
            limit: 10,
        });
        assert_eq!(filtered.len(), 1);
        let system = telemetry.logs(LogFilter {
            scope: Some("system"),
            limit: 10,
            ..LogFilter::default()
        });
        assert_eq!(system.len(), 1);
        let mut request = telemetry.begin_request("r1", "GET", "/test", "pc-a", "mcp");
        request.finish(503);
        let client_logs = telemetry.logs(LogFilter {
            client_id: Some("pc-a"),
            scope: Some("client"),
            limit: 10,
            ..LogFilter::default()
        });
        assert!(client_logs
            .iter()
            .any(|entry| entry.request_id.as_deref() == Some("r1")));
        let dashboard = telemetry.dashboard();
        assert_eq!(dashboard.total_requests, 1);
        assert_eq!(dashboard.failed_requests, 1);
        assert_eq!(dashboard.in_flight_requests, 0);
        telemetry.queue_enter();
        telemetry.queue_enter();
        telemetry.queue_exit();
        telemetry.record_capacity_rejection();
        telemetry.record_worker_acquire_timeout();
        telemetry.record_worker_assignment(25);
        telemetry.record_worker_assignment(75);
        let dashboard = telemetry.dashboard();
        assert_eq!(dashboard.queued_requests, 1);
        assert_eq!(dashboard.peak_queued_requests, 2);
        assert_eq!(dashboard.capacity_rejections, 1);
        assert_eq!(dashboard.worker_acquire_timeouts, 1);
        assert_eq!(dashboard.average_queue_wait_ms, 50.0);
        assert_eq!(dashboard.max_queue_wait_ms, 75);
    }

    #[test]
    fn worker_lifecycle_is_visible() {
        let telemetry = Observability::new();
        let worker = telemetry.connect_worker("worker-1", "device-1", "pc-a", "mcp");
        telemetry.worker_client_status(
            "worker-1",
            "0.29.33",
            "0.1.58",
            "0123456789012345678901234567890123456789",
        );
        telemetry.worker_update_status("worker-1", "update-1", "0.29.34", "verified", None);
        telemetry.worker_state("worker-1", "idle");
        let snapshot = telemetry
            .workers()
            .into_iter()
            .next()
            .expect("worker snapshot");
        assert_eq!(snapshot.agent_version.as_deref(), Some("0.29.33"));
        assert_eq!(snapshot.client_compat_version.as_deref(), Some("0.1.58"));
        assert_eq!(snapshot.update_target_version.as_deref(), Some("0.29.34"));
        assert_eq!(snapshot.update_state.as_deref(), Some("verified"));
        assert_eq!(telemetry.dashboard().idle_workers, 1);
        drop(worker);
        assert!(telemetry.workers().is_empty());
    }

    #[test]
    fn logs_survive_observability_restart() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("tunnel.db");
        let database = DatabaseWriter::open(database_path.clone()).expect("database");
        let telemetry =
            Observability::from_database(database.clone()).expect("persistent observability");
        telemetry.log(
            "warn",
            "worker",
            "connection failed",
            Some("pc-a"),
            Some("mcp"),
            Some("worker-1"),
            Some("request-1"),
        );
        drop(telemetry);

        let reloaded =
            Observability::from_database(database.clone()).expect("reloaded observability");
        let logs = reloaded.logs(LogFilter {
            client_id: Some("pc-a"),
            scope: Some("client"),
            limit: 10,
            ..LogFilter::default()
        });
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "connection failed");
        assert_eq!(logs[0].request_id.as_deref(), Some("request-1"));
        assert!(database_path.is_file());
    }

    #[test]
    fn persistent_logs_are_pruned_to_the_retention_limit() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = DatabaseWriter::open(directory.path().join("tunnel.db")).expect("database");
        let telemetry =
            Observability::from_database(database.clone()).expect("persistent observability");
        for index in 0..LOG_CAPACITY + 5 {
            telemetry.log(
                "info",
                "retention",
                format!("event-{index}"),
                None,
                None,
                None,
                None,
            );
        }
        drop(telemetry);

        let reloaded =
            Observability::from_database(database.clone()).expect("reloaded observability");
        assert_eq!(reloaded.dashboard().retained_logs, LOG_CAPACITY);
        let persisted_count = database
            .call(|connection| {
                connection.query_row("SELECT COUNT(*) FROM observability_logs", [], |row| {
                    row.get::<_, usize>(0)
                })
            })
            .expect("database queue")
            .expect("persisted count");
        assert_eq!(persisted_count, LOG_CAPACITY);
        assert_eq!(
            reloaded
                .logs(LogFilter {
                    limit: 1,
                    ..LogFilter::default()
                })
                .first()
                .map(|entry| entry.message.as_str()),
            Some("event-2004")
        );
    }
}
