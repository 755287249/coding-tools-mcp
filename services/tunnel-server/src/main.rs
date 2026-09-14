mod admin;
mod database;
mod device_auth;
mod observability;
mod server_identity;
mod worker_policy;

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use bytes::Bytes;
use coding_tools_tunnel_protocol::{
    expected_routes, is_hop_by_hop_header, is_retry_safe_mcp_request, is_retry_safe_tool_name,
    route_matches, server_ack_signing_payload, server_challenge_signing_payload, valid_client_id,
    ClientHello, ControlMessage, EnrollmentRequest, HeaderPair, NodeUpdateOffer, NodeUpdateState,
    TunnelService, WorkerDemand, WorkerPolicy, CLIENT_ID_HEADER, DEVICE_ID_HEADER,
    ENROLL_PATH_PREFIX, MAX_REQUEST_BODY_BYTES, PROTOCOL_VERSION, SERVICE_HEADER, WORKER_ID_HEADER,
    WS_PATH, WS_SUBPROTOCOL,
};
use database::DatabaseWriter;
use device_auth::{unix_ms, AllowedServices, DeviceAuthError, DeviceRegistry};
use observability::Observability;
use server_identity::ServerIdentity;
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::{interval, sleep, timeout, timeout_at, Instant, MissedTickBehavior};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{info, warn};
use tracing_subscriber::{prelude::*, EnvFilter};
use uuid::Uuid;
use worker_policy::WorkerPolicyStore;

const REQUEST_QUEUE_CAPACITY: usize = 128;
const CONTROL_REQUEST_QUEUE_CAPACITY: usize = 32;
const CONTROL_PENDING_REQUEST_CAPACITY: usize = 32;
const AVAILABLE_WORKER_CAPACITY: usize = 128;
const RESPONSE_BODY_CAPACITY: usize = 16;
const RESPONSE_HEAD_TIMEOUT: Duration = Duration::from_secs(30);
const RECONNECT_GRACE_TIMEOUT: Duration = Duration::from_secs(2);
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(25);
const MAX_SAFE_REQUEST_ATTEMPTS: u8 = 2;
const AUTH_CHALLENGE_TIMEOUT: Duration = Duration::from_secs(10);
const WORKER_LIVENESS_TIMEOUT: Duration = Duration::from_secs(45);
const DEFAULT_PUBLIC_ORIGIN: &str = "http://127.0.0.1:8088";

#[derive(Clone)]
struct AppState {
    registry: Registry,
    devices: DeviceRegistry,
    policies: WorkerPolicyStore,
    observability: Observability,
    server_identity: ServerIdentity,
    max_request_body_bytes: usize,
    response_head_timeout: Duration,
    reconnect_grace_timeout: Duration,
    worker_liveness_timeout: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ClientKey {
    client_id: String,
    service: TunnelService,
}

#[derive(Clone)]
struct ClientPool {
    control_request_tx: mpsc::Sender<ProxyJob>,
    execution_request_tx: mpsc::Sender<ProxyJob>,
    available_tx: mpsc::Sender<AvailableWorker>,
    update_tx: watch::Sender<Option<NodeUpdateOffer>>,
    active_workers: Arc<AtomicUsize>,
    control_pending_requests: Arc<AtomicUsize>,
    execution_pending_requests: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct RouteEntry {
    prefix: String,
    key: ClientKey,
    pool: ClientPool,
}

#[derive(Clone)]
struct RouteMatch {
    key: ClientKey,
    pool: ClientPool,
}

#[derive(Default)]
struct RegistryInner {
    pools: HashMap<ClientKey, ClientPool>,
    routes: Vec<RouteEntry>,
}

#[derive(Clone, Default)]
struct Registry {
    inner: Arc<Mutex<RegistryInner>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestLane {
    Control,
    Execution,
}

impl RequestLane {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Execution => "execution",
        }
    }
}

const CONTROL_WORKER_RESERVE: usize = 1;

struct ProxyJob {
    request_id: String,
    method: String,
    path_and_query: String,
    headers: Vec<HeaderPair>,
    body: Bytes,
    lane: RequestLane,
    attempt: u8,
    enqueued_at: Instant,
    policy: WorkerPolicy,
    active_workers: Arc<AtomicUsize>,
    demand: Option<WorkerDemand>,
    assigned: Option<oneshot::Sender<AssignmentInfo>>,
    pending_slot: Option<PendingRequestSlot>,
    response_head: oneshot::Sender<Result<ResponseHeadData, String>>,
    response_body: mpsc::Sender<Result<Bytes, io::Error>>,
    cancelled: watch::Receiver<bool>,
}

impl ProxyJob {
    fn abandoned(&self) -> bool {
        self.response_head.is_closed()
            || self
                .assigned
                .as_ref()
                .is_none_or(oneshot::Sender::is_closed)
            || *self.cancelled.borrow()
    }
}

#[derive(Debug, Clone, Copy)]
struct AssignmentInfo {
    queue_wait_ms: u64,
}

struct PendingRequestSlot {
    pending_requests: Arc<AtomicUsize>,
    observability: Observability,
    released: bool,
}

impl PendingRequestSlot {
    fn try_new(
        pending_requests: Arc<AtomicUsize>,
        maximum: usize,
        observability: Observability,
    ) -> Option<Self> {
        pending_requests
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < maximum).then_some(current + 1)
            })
            .ok()?;
        observability.queue_enter();
        Some(Self {
            pending_requests,
            observability,
            released: false,
        })
    }

    fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        self.pending_requests.fetch_sub(1, Ordering::AcqRel);
        self.observability.queue_exit();
    }
}

impl Drop for PendingRequestSlot {
    fn drop(&mut self) {
        self.release();
    }
}

struct ResponseHeadData {
    status: u16,
    headers: Vec<HeaderPair>,
}

struct AvailableWorker {
    assign: oneshot::Sender<ProxyJob>,
}

fn discard_closed_workers(workers: &mut VecDeque<AvailableWorker>) -> usize {
    let before = workers.len();
    workers.retain(|worker| !worker.assign.is_closed());
    before.saturating_sub(workers.len())
}

struct ActiveWorkerGuard {
    active_workers: Arc<AtomicUsize>,
}

impl ActiveWorkerGuard {
    fn try_new(active_workers: Arc<AtomicUsize>, maximum: usize) -> Option<Self> {
        active_workers
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < maximum).then_some(current + 1)
            })
            .ok()?;
        Some(Self { active_workers })
    }
}

impl Drop for ActiveWorkerGuard {
    fn drop(&mut self) {
        self.active_workers.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Registry {
    async fn register(&self, key: ClientKey) -> ClientPool {
        let mut inner = self.inner.lock().await;
        if let Some(pool) = inner.pools.get(&key) {
            return pool.clone();
        }

        let (control_request_tx, control_request_rx) =
            mpsc::channel(CONTROL_REQUEST_QUEUE_CAPACITY);
        let (execution_request_tx, execution_request_rx) = mpsc::channel(REQUEST_QUEUE_CAPACITY);
        let (available_tx, available_rx) = mpsc::channel(AVAILABLE_WORKER_CAPACITY);
        let (update_tx, _) = watch::channel(None);
        let pool = ClientPool {
            control_request_tx,
            execution_request_tx,
            available_tx,
            update_tx,
            active_workers: Arc::new(AtomicUsize::new(0)),
            control_pending_requests: Arc::new(AtomicUsize::new(0)),
            execution_pending_requests: Arc::new(AtomicUsize::new(0)),
        };
        tokio::spawn(dispatch_requests(
            control_request_rx,
            execution_request_rx,
            available_rx,
        ));

        for prefix in expected_routes(&key.client_id, key.service) {
            inner.routes.push(RouteEntry {
                prefix,
                key: key.clone(),
                pool: pool.clone(),
            });
        }
        inner
            .routes
            .sort_by_key(|route| std::cmp::Reverse(route.prefix.len()));
        inner.pools.insert(key, pool.clone());
        pool
    }

    async fn dispatch_node_update(&self, client_id: &str, offer: NodeUpdateOffer) -> usize {
        let inner = self.inner.lock().await;
        let key = ClientKey {
            client_id: client_id.to_string(),
            service: TunnelService::Mcp,
        };
        let Some(pool) = inner.pools.get(&key) else {
            return 0;
        };
        let targets = pool.active_workers.load(Ordering::Acquire);
        pool.update_tx.send_replace(Some(offer));
        targets
    }

    async fn clear_node_update(&self, client_id: &str, update_id: &str) -> bool {
        let inner = self.inner.lock().await;
        let key = ClientKey {
            client_id: client_id.to_string(),
            service: TunnelService::Mcp,
        };
        let Some(pool) = inner.pools.get(&key) else {
            return false;
        };
        let matches_current = pool
            .update_tx
            .borrow()
            .as_ref()
            .is_some_and(|offer| offer.update_id == update_id);
        if matches_current {
            pool.update_tx.send_replace(None);
        }
        matches_current
    }

    async fn lookup(&self, path: &str) -> Option<RouteMatch> {
        let inner = self.inner.lock().await;
        inner
            .routes
            .iter()
            .find(|entry| route_matches(&entry.prefix, path))
            .map(|entry| RouteMatch {
                key: entry.key.clone(),
                pool: entry.pool.clone(),
            })
    }
}

async fn dispatch_requests(
    mut control_request_rx: mpsc::Receiver<ProxyJob>,
    mut execution_request_rx: mpsc::Receiver<ProxyJob>,
    mut available_rx: mpsc::Receiver<AvailableWorker>,
) {
    let mut control_jobs: VecDeque<ProxyJob> = VecDeque::new();
    let mut execution_jobs: VecDeque<ProxyJob> = VecDeque::new();
    let mut workers: VecDeque<AvailableWorker> = VecDeque::new();
    let mut cleanup_tick = interval(Duration::from_millis(100));
    cleanup_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        control_jobs.retain(|job| !job.abandoned());
        execution_jobs.retain(|job| !job.abandoned());
        discard_closed_workers(&mut workers);
        while !workers.is_empty() && (!control_jobs.is_empty() || !execution_jobs.is_empty()) {
            let execution_can_run = execution_jobs.front().is_some_and(|job| {
                let connected = job.active_workers.load(Ordering::Acquire);
                let reserve =
                    usize::from(connected > CONTROL_WORKER_RESERVE) * CONTROL_WORKER_RESERVE;
                workers.len() > reserve
            });
            let mut job = if let Some(job) = control_jobs.pop_front() {
                job
            } else if execution_can_run {
                execution_jobs.pop_front().expect("execution queue checked")
            } else {
                break;
            };
            let worker = workers.pop_front().expect("worker queue checked");
            if job.abandoned() {
                workers.push_front(worker);
                continue;
            }

            let queued_requests = job.pending_slot.as_ref().map_or(1, |slot| {
                slot.pending_requests.load(Ordering::Acquire).max(1)
            });
            let connected_workers = job.active_workers.load(Ordering::Acquire);
            let idle_workers = workers.len().saturating_add(1).min(connected_workers);
            let busy_workers = connected_workers.saturating_sub(idle_workers);
            let desired_workers = connected_workers
                .max(
                    busy_workers
                        .saturating_add(queued_requests)
                        .saturating_add(usize::from(job.policy.min_idle_workers)),
                )
                .min(usize::from(job.policy.max_workers));
            let queue_wait_ms = job
                .enqueued_at
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            job.demand = Some(WorkerDemand {
                queued_requests: queued_requests.min(usize::from(u16::MAX)) as u16,
                oldest_queue_wait_ms: queue_wait_ms,
                desired_workers: desired_workers.min(usize::from(u16::MAX)) as u16,
            });
            let assigned = job.assigned.take();
            let mut pending_slot = job.pending_slot.take();
            match worker.assign.send(job) {
                Ok(()) => {
                    if let Some(slot) = pending_slot.as_mut() {
                        slot.release();
                    }
                    if let Some(assigned) = assigned {
                        let _ = assigned.send(AssignmentInfo { queue_wait_ms });
                    }
                }
                Err(mut job) => {
                    job.assigned = assigned;
                    job.pending_slot = pending_slot;
                    match job.lane {
                        RequestLane::Control => control_jobs.push_front(job),
                        RequestLane::Execution => execution_jobs.push_front(job),
                    }
                    discard_closed_workers(&mut workers);
                }
            }
        }

        tokio::select! {
            biased;
            job = control_request_rx.recv(), if !control_request_rx.is_closed() => match job {
                Some(job) => control_jobs.push_back(job),
                None => {}
            },
            job = execution_request_rx.recv(), if !execution_request_rx.is_closed() => match job {
                Some(job) => execution_jobs.push_back(job),
                None => {}
            },
            worker = available_rx.recv() => match worker {
                Some(worker) => workers.push_back(worker),
                None => break,
            },
            _ = cleanup_tick.tick() => {}
        }
        if control_request_rx.is_closed()
            && execution_request_rx.is_closed()
            && control_jobs.is_empty()
            && execution_jobs.is_empty()
        {
            break;
        }
    }

    for job in control_jobs.into_iter().chain(execution_jobs) {
        let _ = job
            .response_head
            .send(Err("內建隧道 dispatcher 已停止。".into()));
    }
}

#[tokio::main]
async fn main() {
    let database_path = std::env::var_os("CODING_TOOLS_TUNNEL_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tunnel-data/tunnel.db"));
    let log_directory = std::env::var_os("CODING_TOOLS_TUNNEL_LOG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            database_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| FsPath::new("."))
                .join("logs")
        });
    let _trace_guard = init_tracing(&log_directory).expect("failed to initialize file logging");
    let database = DatabaseWriter::open(&database_path).expect("failed to open tunnel database");
    let server_identity_path = std::env::var_os("CODING_TOOLS_TUNNEL_SERVER_IDENTITY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            database_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| FsPath::new("."))
                .join("server-identity.json")
        });
    let server_identity = ServerIdentity::load_or_create(&server_identity_path)
        .expect("failed to load tunnel server identity");
    let devices = DeviceRegistry::from_writer(database.clone())
        .expect("failed to open tunnel device registry");
    let policies = WorkerPolicyStore::from_writer(database.clone())
        .expect("failed to open worker policy store");
    match handle_cli(&devices) {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }

    let bind = std::env::var("CODING_TOOLS_TUNNEL_BIND")
        .unwrap_or_else(|_| "0.0.0.0:8088".to_string())
        .parse::<SocketAddr>()
        .expect("CODING_TOOLS_TUNNEL_BIND must be a socket address");
    let admin_bind = std::env::var("CODING_TOOLS_TUNNEL_ADMIN_BIND")
        .ok()
        .map(|value| {
            value
                .parse::<SocketAddr>()
                .expect("CODING_TOOLS_TUNNEL_ADMIN_BIND must be a socket address")
        });
    let admin_config = if admin_bind.is_some() {
        Some(
            admin::load_admin_config()
                .expect("failed to load tunnel admin login configuration")
                .expect(
                    "CODING_TOOLS_TUNNEL_ADMIN_USERNAME and CODING_TOOLS_TUNNEL_ADMIN_PASSWORD_FILE or CODING_TOOLS_TUNNEL_ADMIN_PASSWORD are required when the admin listener is enabled",
                ),
        )
    } else {
        None
    };
    let public_origin = std::env::var("CODING_TOOLS_TUNNEL_PUBLIC_ORIGIN")
        .unwrap_or_else(|_| DEFAULT_PUBLIC_ORIGIN.into());
    let observability = Observability::from_database(database.clone())
        .expect("failed to initialize persistent observability logs");
    observability.log(
        "info",
        "server",
        "tunnel server starting",
        None,
        None,
        None,
        None,
    );
    let state = AppState {
        registry: Registry::default(),
        devices: devices.clone(),
        policies: policies.clone(),
        observability: observability.clone(),
        server_identity,
        max_request_body_bytes: std::env::var("CODING_TOOLS_TUNNEL_MAX_BODY_BYTES")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(MAX_REQUEST_BODY_BYTES),
        response_head_timeout: std::env::var("CODING_TOOLS_TUNNEL_RESPONSE_HEAD_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(RESPONSE_HEAD_TIMEOUT),
        reconnect_grace_timeout: std::env::var("CODING_TOOLS_TUNNEL_RECONNECT_GRACE_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(RECONNECT_GRACE_TIMEOUT),
        worker_liveness_timeout: WORKER_LIVENESS_TIMEOUT,
    };

    let admin_registry = state.registry.clone();
    let app = build_app(state);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .expect("failed to bind tunnel server");
    info!(%bind, "built-in WSS tunnel server listening");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });
    let public_server =
        axum::serve(listener, app).with_graceful_shutdown(wait_for_shutdown(shutdown_rx.clone()));

    if let Some(admin_bind) = admin_bind {
        let admin_config = admin_config.expect("admin config checked with admin bind");
        let admin_app = admin::build_admin_app(
            devices,
            policies,
            observability,
            admin_registry,
            public_origin,
            admin_config,
        )
        .expect("failed to initialize tunnel admin login");
        let admin_listener = tokio::net::TcpListener::bind(admin_bind)
            .await
            .expect("failed to bind tunnel admin server");
        info!(%admin_bind, "built-in WSS tunnel admin server listening");
        let admin_server = axum::serve(admin_listener, admin_app)
            .with_graceful_shutdown(wait_for_shutdown(shutdown_rx));
        tokio::select! {
            result = public_server => result.expect("tunnel server failed"),
            result = admin_server => result.expect("tunnel admin server failed"),
        }
    } else {
        public_server.await.expect("tunnel server failed");
    }
}

fn init_tracing(
    log_directory: &FsPath,
) -> Result<tracing_appender::non_blocking::WorkerGuard, String> {
    std::fs::create_dir_all(log_directory).map_err(|error| {
        format!(
            "could not create log directory {}: {error}",
            log_directory.display()
        )
    })?;
    let file_appender = tracing_appender::rolling::daily(log_directory, "tunnel-server.log");
    let (file_writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .lossy(false)
        .finish(file_appender);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("coding_tools_tunnel_server=info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer),
        )
        .init();
    Ok(guard)
}

async fn wait_for_shutdown(mut receiver: watch::Receiver<bool>) {
    while !*receiver.borrow() {
        if receiver.changed().await.is_err() {
            break;
        }
    }
}

fn build_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route(WS_PATH, get(websocket_upgrade))
        .route("/_tunnel/enroll/{code}", post(enroll_device))
        .fallback(proxy_request)
        .with_state(state)
}

fn handle_cli(devices: &DeviceRegistry) -> Result<bool, String> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.is_empty() {
        return Ok(false);
    }
    match arguments.as_slice() {
        [group, command, rest @ ..] if group == "enroll" && command == "create" => {
            let client_id = argument_value(rest, "--client-id")
                .ok_or_else(|| "missing --client-id".to_string())?;
            let service_value =
                argument_value(rest, "--service").unwrap_or_else(|| "both".into());
            let services = AllowedServices::parse(&service_value)
                .ok_or_else(|| "--service must be mcp, actions, or both".to_string())?;
            let ttl_seconds = argument_value(rest, "--ttl-seconds")
                .map(|value| value.parse::<u64>())
                .transpose()
                .map_err(|_| "--ttl-seconds must be an integer".to_string())?;
            let grant = devices
                .create_enrollment(&client_id, services, ttl_seconds)
                .map_err(|error| error.to_string())?;
            let origin = std::env::var("CODING_TOOLS_TUNNEL_PUBLIC_ORIGIN")
                .unwrap_or_else(|_| DEFAULT_PUBLIC_ORIGIN.into());
            println!(
                "{}/{}/{}",
                origin.trim_end_matches('/'),
                ENROLL_PATH_PREFIX.trim_start_matches('/'),
                grant.code
            );
            println!("client_id={}", grant.client_id);
            println!("expires_at_unix_ms={}", grant.expires_at_unix_ms);
            println!("services={:?}", grant.services);
            Ok(true)
        }
        [group, command] if group == "devices" && command == "list" => {
            for device in devices.list_devices().map_err(|error| error.to_string())? {
                println!(
                    "{}\t{}\t{}\tmcp={}\tactions={}\tcreated={}\tlast_seen={}\trevoked={}",
                    device.device_id,
                    device.client_id,
                    device.device_name,
                    device.allow_mcp,
                    device.allow_actions,
                    device.created_at_unix_ms,
                    device
                        .last_seen_at_unix_ms
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".into()),
                    device
                        .revoked_at_unix_ms
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".into())
                );
            }
            Ok(true)
        }
        [group, command, rest @ ..] if group == "devices" && command == "revoke" => {
            let device_id = argument_value(rest, "--device-id")
                .ok_or_else(|| "missing --device-id".to_string())?;
            if !devices
                .revoke_device(&device_id)
                .map_err(|error| error.to_string())?
            {
                return Err(format!("active device not found: {device_id}"));
            }
            println!("revoked {device_id}");
            Ok(true)
        }
        _ => Err(
            "usage: coding-tools-tunnel-server [enroll create --client-id ID [--service both|mcp|actions] [--ttl-seconds 600] | devices list | devices revoke --device-id ID]"
                .into(),
        ),
    }
}

fn argument_value(arguments: &[String], name: &str) -> Option<String> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

async fn health() -> &'static str {
    "ok"
}

async fn enroll_device(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Json(request): Json<EnrollmentRequest>,
) -> Response<Body> {
    match state.devices.enroll(code, request).await {
        Ok(mut response) => {
            response.server_id = state.server_identity.server_id().to_string();
            response.server_public_key = state.server_identity.public_key().to_string();
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(error) => {
            if matches!(&error, DeviceAuthError::Storage(_)) {
                warn!(%error, "device enrollment storage failed");
            }
            let status = match &error {
                DeviceAuthError::EnrollmentUsed | DeviceAuthError::DeviceIdConflict => {
                    StatusCode::CONFLICT
                }
                DeviceAuthError::EnrollmentExpired => StatusCode::GONE,
                DeviceAuthError::Storage(_) => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::BAD_REQUEST,
            };
            status_response(status, error.public_message())
        }
    }
}

#[derive(Clone)]
struct WorkerSocketAuth {
    server_identity: ServerIdentity,
    key: ClientKey,
    device_id: String,
    worker_id: String,
}

async fn websocket_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response<Body> {
    let Some(client_id) = header_text(&headers, CLIENT_ID_HEADER) else {
        return status_response(StatusCode::BAD_REQUEST, "missing client id header");
    };
    if !valid_client_id(client_id) {
        return status_response(StatusCode::BAD_REQUEST, "invalid client id header");
    }
    let Some(service) = header_text(&headers, SERVICE_HEADER).and_then(TunnelService::parse) else {
        return status_response(StatusCode::BAD_REQUEST, "invalid service header");
    };
    let Some(device_id) = header_text(&headers, DEVICE_ID_HEADER)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .map(str::to_string)
    else {
        return status_response(StatusCode::BAD_REQUEST, "invalid device id header");
    };
    let Some(worker_id) = header_text(&headers, WORKER_ID_HEADER)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .map(str::to_string)
    else {
        return status_response(StatusCode::BAD_REQUEST, "invalid worker id header");
    };
    let key = ClientKey {
        client_id: client_id.to_string(),
        service,
    };
    ws.protocols([WS_SUBPROTOCOL])
        .on_upgrade(move |socket| {
            run_worker_socket(
                socket,
                state.registry,
                state.devices,
                state.policies,
                state.observability,
                WorkerSocketAuth {
                    server_identity: state.server_identity,
                    key,
                    device_id,
                    worker_id,
                },
                state.worker_liveness_timeout,
            )
        })
        .into_response()
}

async fn run_worker_socket(
    mut socket: WebSocket,
    registry: Registry,
    devices: DeviceRegistry,
    policies: WorkerPolicyStore,
    observability: Observability,
    auth: WorkerSocketAuth,
    worker_liveness_timeout: Duration,
) {
    let WorkerSocketAuth {
        server_identity,
        key,
        device_id: expected_device_id,
        worker_id: expected_worker_id,
    } = auth;
    let nonce = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let expires_at_unix_ms = unix_ms().saturating_add(AUTH_CHALLENGE_TIMEOUT.as_millis() as u64);
    let server_signature = server_identity.sign(&server_challenge_signing_payload(
        &nonce,
        expires_at_unix_ms,
        server_identity.server_id(),
        &expected_device_id,
        &key.client_id,
        key.service,
        &expected_worker_id,
    ));
    if send_control(
        &mut socket,
        &ControlMessage::Challenge {
            nonce: nonce.clone(),
            expires_at_unix_ms,
            server_id: server_identity.server_id().to_string(),
            server_signature,
        },
    )
    .await
    .is_err()
    {
        return;
    }
    let proof = match timeout(AUTH_CHALLENGE_TIMEOUT, receive_control(&mut socket)).await {
        Ok(Ok(ControlMessage::Authenticate(proof))) => proof,
        Ok(Ok(_)) => {
            let _ = send_error(&mut socket, None, "expected device authentication proof").await;
            return;
        }
        Ok(Err(error)) => {
            warn!(client_id = %key.client_id, service = key.service.as_str(), %error, "worker authentication failed");
            return;
        }
        Err(_) => {
            let _ = send_error(&mut socket, None, "authentication challenge timed out").await;
            return;
        }
    };

    if let Err(message) = validate_hello(&proof.hello, &key) {
        let _ = send_error(&mut socket, None, &message).await;
        return;
    }
    if proof.device_id != expected_device_id || proof.hello.worker_id != expected_worker_id {
        let _ = send_error(
            &mut socket,
            None,
            "authentication proof does not match connection identity",
        )
        .await;
        return;
    }
    if let Err(error) = devices
        .verify(nonce.clone(), expires_at_unix_ms, proof.clone())
        .await
    {
        warn!(client_id = %key.client_id, service = key.service.as_str(), device_id = %proof.device_id, %error, "device authentication rejected");
        let _ = send_error(&mut socket, None, error.public_message()).await;
        return;
    }

    let hello = proof.hello.clone();
    let pool = registry.register(key.clone()).await;
    let mut policy_updates = policies.subscribe(key.service);
    let worker_policy = policy_updates.borrow().clone();
    let server_signature = server_identity.sign(&server_ack_signing_payload(
        &nonce,
        server_identity.server_id(),
        &proof,
        &worker_policy,
    ));
    let Some(_active_worker) = ActiveWorkerGuard::try_new(
        pool.active_workers.clone(),
        usize::from(worker_policy.max_workers),
    ) else {
        let _ = send_error(&mut socket, None, "worker limit reached for this route").await;
        return;
    };
    if send_control(
        &mut socket,
        &ControlMessage::HelloAck {
            protocol_version: PROTOCOL_VERSION,
            worker_policy,
            server_id: server_identity.server_id().to_string(),
            server_signature,
        },
    )
    .await
    .is_err()
    {
        return;
    }
    info!(client_id = %key.client_id, service = key.service.as_str(), worker_id = %hello.worker_id, device_id = %proof.device_id, "worker connected");
    let worker_id = hello.worker_id.clone();
    let _worker_guard = observability.connect_worker(
        &worker_id,
        &proof.device_id,
        &key.client_id,
        key.service.as_str(),
    );
    let mut update_offers = pool.update_tx.subscribe();

    loop {
        match timeout(worker_liveness_timeout, receive_control(&mut socket)).await {
            Ok(Ok(ControlMessage::Ready)) => {
                observability.worker_state(&worker_id, "idle");
            }
            Ok(Ok(ControlMessage::ClientStatus {
                agent_version,
                client_compat_version,
                build_git_sha,
            })) => {
                observability.worker_client_status(
                    &worker_id,
                    &agent_version,
                    &client_compat_version,
                    &build_git_sha,
                );
                continue;
            }
            Ok(Ok(ControlMessage::UpdateStatus {
                update_id,
                version,
                state,
                message,
            })) => {
                observability.worker_update_status(
                    &worker_id,
                    &update_id,
                    &version,
                    node_update_state_name(&state),
                    message.as_deref(),
                );
                if node_update_state_terminal(&state) {
                    registry.clear_node_update(&key.client_id, &update_id).await;
                }
                continue;
            }
            Ok(Ok(ControlMessage::Error { message, .. })) => {
                warn!(client_id = %key.client_id, service = key.service.as_str(), %message, "worker reported an error while idle");
                observability.worker_error(&worker_id, &message);
                observability.log(
                    "warn",
                    "worker",
                    format!("worker reported an error while idle: {message}"),
                    Some(&key.client_id),
                    Some(key.service.as_str()),
                    Some(&worker_id),
                    None,
                );
                continue;
            }
            Ok(Ok(_)) => {
                let _ = send_error(&mut socket, None, "expected ready message").await;
                return;
            }
            Ok(Err(_)) | Err(_) => return,
        }

        let (assign, mut assigned) = oneshot::channel();
        if pool
            .available_tx
            .send(AvailableWorker { assign })
            .await
            .is_err()
        {
            return;
        }

        let job = loop {
            tokio::select! {
                job = &mut assigned => match job {
                    Ok(job) => break job,
                    Err(_) => return,
                },
                incoming = timeout(worker_liveness_timeout, socket.recv()) => {
                    match incoming {
                        Ok(Some(Ok(Message::Ping(payload)))) => {
                            if socket.send(Message::Pong(payload)).await.is_err() {
                                return;
                            }
                        }
                        Ok(Some(Ok(Message::Text(text)))) => {
                            let Ok(control) = serde_json::from_str::<ControlMessage>(text.as_str()) else {
                                return;
                            };
                            match control {
                                ControlMessage::ClientStatus { agent_version, client_compat_version, build_git_sha } => {
                                    observability.worker_client_status(&worker_id, &agent_version, &client_compat_version, &build_git_sha);
                                }
                                ControlMessage::UpdateStatus { update_id, version, state, message } => {
                                    observability.worker_update_status(&worker_id, &update_id, &version, node_update_state_name(&state), message.as_deref());
                                    if node_update_state_terminal(&state) {
                                        registry.clear_node_update(&key.client_id, &update_id).await;
                                    }
                                }
                                ControlMessage::Error { message, .. } => observability.worker_error(&worker_id, &message),
                                _ => return,
                            }
                        }
                        _ => return,
                    }
                }
                changed = update_offers.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let offer = { update_offers.borrow_and_update().clone() };
                    if let Some(offer) = offer {
                        if send_control(&mut socket, &ControlMessage::UpdateOffer(offer)).await.is_err() {
                            return;
                        }
                    }
                }
                changed = policy_updates.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let worker_policy = policy_updates.borrow_and_update().clone();
                    if send_control(
                        &mut socket,
                        &ControlMessage::PolicyUpdate { worker_policy },
                    )
                    .await
                    .is_err()
                    {
                        return;
                    }
                }
            }
        };

        observability.worker_state(&worker_id, "busy");
        let request_id = job.request_id.clone();
        let attempt = job.attempt;
        if let Err(error) = proxy_job_over_socket(&mut socket, job, worker_liveness_timeout).await {
            warn!(client_id = %key.client_id, service = key.service.as_str(), worker_id = %worker_id, request_id = %request_id, attempt, %error, "worker transaction failed");
            observability.worker_error(&worker_id, &error);
            observability.log(
                "error",
                "worker",
                format!("worker transaction failed on attempt {attempt}: {error}"),
                Some(&key.client_id),
                Some(key.service.as_str()),
                Some(&worker_id),
                Some(&request_id),
            );
            return;
        }
        observability.worker_completed_request(&worker_id);
        observability.worker_state(&worker_id, "idle");
    }
}

fn validate_hello(hello: &ClientHello, key: &ClientKey) -> Result<(), String> {
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(format!(
            "unsupported protocol version {}; expected {}",
            hello.protocol_version, PROTOCOL_VERSION
        ));
    }
    if hello.client_id != key.client_id || hello.service != key.service {
        return Err("hello identity does not match authenticated headers".into());
    }
    if hello.worker_id.trim().is_empty() || hello.worker_id.len() > 128 {
        return Err("invalid worker id".into());
    }
    Ok(())
}

async fn proxy_job_over_socket(
    socket: &mut WebSocket,
    job: ProxyJob,
    worker_liveness_timeout: Duration,
) -> Result<(), String> {
    let ProxyJob {
        request_id,
        method,
        path_and_query,
        headers,
        body,
        demand,
        response_head,
        response_body,
        mut cancelled,
        ..
    } = job;
    let mut response_head = Some(response_head);

    if let Err(error) = send_control(
        socket,
        &ControlMessage::RequestHead {
            request_id: request_id.clone(),
            method,
            path_and_query,
            headers,
            demand,
        },
    )
    .await
    {
        fail_response_head(&mut response_head, error.clone());
        return Err(error);
    }
    if !body.is_empty() {
        if let Err(error) = socket.send(Message::Binary(body)).await {
            let message = error.to_string();
            fail_response_head(&mut response_head, message.clone());
            return Err(message);
        }
    }
    send_control(
        socket,
        &ControlMessage::RequestEnd {
            request_id: request_id.clone(),
        },
    )
    .await
    .inspect_err(|error| fail_response_head(&mut response_head, error.clone()))?;

    loop {
        let incoming = tokio::select! {
            _ = wait_for_cancellation(&mut cancelled) => {
                let _ = send_control(
                    socket,
                    &ControlMessage::Cancel {
                        request_id: request_id.clone(),
                    },
                )
                .await;
                return Ok(());
            }
            incoming = timeout(worker_liveness_timeout, socket.recv()) => incoming,
        };
        match incoming {
            Ok(Some(Ok(Message::Text(text)))) => {
                let control: ControlMessage =
                    serde_json::from_str(text.as_str()).map_err(|error| error.to_string())?;
                match control {
                    ControlMessage::ResponseHead {
                        request_id: response_id,
                        status,
                        headers,
                    } if response_id == request_id => {
                        let Some(sender) = response_head.take() else {
                            return Err("duplicate response head".into());
                        };
                        let _ = sender.send(Ok(ResponseHeadData { status, headers }));
                    }
                    ControlMessage::ResponseEnd {
                        request_id: response_id,
                    } if response_id == request_id => {
                        if response_head.is_some() {
                            fail_response_head(
                                &mut response_head,
                                "response ended before headers".into(),
                            );
                        }
                        return Ok(());
                    }
                    ControlMessage::Error {
                        request_id: response_id,
                        message,
                    } if response_id
                        .as_deref()
                        .is_none_or(|value| value == request_id) =>
                    {
                        if response_head.is_some() {
                            fail_response_head(&mut response_head, message.clone());
                        } else {
                            let _ = response_body
                                .send(Err(io::Error::other(message.clone())))
                                .await;
                        }
                        return Err(message);
                    }
                    _ => return Err("unexpected control message during response".into()),
                }
            }
            Ok(Some(Ok(Message::Binary(chunk)))) => {
                if response_head.is_some() {
                    fail_response_head(
                        &mut response_head,
                        "response body arrived before headers".into(),
                    );
                    return Err("response body arrived before headers".into());
                }
                if response_body.send(Ok(chunk)).await.is_err() {
                    let _ = send_control(
                        socket,
                        &ControlMessage::Cancel {
                            request_id: request_id.clone(),
                        },
                    )
                    .await;
                    return Err("public response consumer disconnected".into());
                }
            }
            Ok(Some(Ok(Message::Ping(payload)))) => socket
                .send(Message::Pong(payload))
                .await
                .map_err(|error| error.to_string())?,
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                fail_response_head(&mut response_head, "worker disconnected".into());
                return Err("worker disconnected".into());
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(error))) => {
                let message = error.to_string();
                fail_response_head(&mut response_head, message.clone());
                return Err(message);
            }
            Err(_) => {
                fail_response_head(&mut response_head, "worker heartbeat timed out".into());
                return Err("worker heartbeat timed out".into());
            }
        }
    }
}

async fn wait_for_cancellation(cancelled: &mut watch::Receiver<bool>) {
    loop {
        if *cancelled.borrow() {
            return;
        }
        if cancelled.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

fn classify_request_lane(service: TunnelService, body: &[u8]) -> RequestLane {
    if service != TunnelService::Mcp {
        return RequestLane::Execution;
    }
    let Ok(message) = serde_json::from_slice::<serde_json::Value>(body) else {
        return RequestLane::Execution;
    };
    let Some(method) = message.get("method").and_then(serde_json::Value::as_str) else {
        return RequestLane::Execution;
    };
    if matches!(method, "initialize" | "ping" | "tools/list") {
        return RequestLane::Control;
    }
    if method != "tools/call" {
        return RequestLane::Execution;
    }
    let Some(params) = message.get("params") else {
        return RequestLane::Execution;
    };
    let Some(name) = params.get("name").and_then(serde_json::Value::as_str) else {
        return RequestLane::Execution;
    };
    if matches!(
        name,
        "exec_health_check"
            | "wait_command"
            | "resolve_operation"
            | "list_sessions"
            | "send_input"
            | "kill_session"
            | "read_output"
            | "request_permissions"
    ) {
        return RequestLane::Control;
    }
    if name == "exec_many" {
        let arguments = params
            .get("arguments")
            .and_then(serde_json::Value::as_object);
        let action = arguments
            .and_then(|value| value.get("action"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("run");
        let has_commands = arguments
            .and_then(|value| value.get("commands"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|commands| !commands.is_empty());
        let has_operation_id = arguments
            .and_then(|value| value.get("operation_id"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        if matches!(action, "status" | "cancel" | "forget")
            || (action == "run" && has_operation_id && !has_commands)
        {
            return RequestLane::Control;
        }
    }
    RequestLane::Execution
}

fn request_operation_hash(service: TunnelService, body: &[u8]) -> Option<String> {
    if service != TunnelService::Mcp {
        return None;
    }
    let message = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    let operation_id = message
        .get("params")?
        .get("arguments")?
        .get("operation_id")?
        .as_str()?
        .trim();
    if operation_id.is_empty() {
        return None;
    }
    let digest = Sha256::digest(operation_id.as_bytes());
    Some(
        digest
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn with_tunnel_request_context(
    mut response: Response<Body>,
    request_id: &str,
    lane: RequestLane,
    operation_hash: Option<&str>,
) -> Response<Body> {
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert("x-tunnel-request-id", value);
    }
    response.headers_mut().insert(
        "x-tunnel-request-lane",
        HeaderValue::from_static(lane.as_str()),
    );
    if let Some(operation_hash) = operation_hash {
        if let Ok(value) = HeaderValue::from_str(operation_hash) {
            response
                .headers_mut()
                .insert("x-tunnel-operation-hash", value);
        }
    }
    response
}

async fn proxy_request(State(state): State<AppState>, request: Request<Body>) -> Response<Body> {
    let path = request.uri().path().to_string();
    let Some(route) = state.registry.lookup(&path).await else {
        return status_response(StatusCode::NOT_FOUND, "no built-in tunnel route");
    };
    let pool = route.pool;
    let client_id = route.key.client_id;
    let service_kind = route.key.service;
    let service = service_kind.as_str().to_string();

    let (parts, body) = request.into_parts();
    let request_id = Uuid::new_v4().to_string();
    let request_method = parts.method.as_str().to_string();
    let request_path = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/")
        .to_string();
    let request_started = std::time::Instant::now();

    if pool.active_workers.load(Ordering::Acquire) == 0 {
        state.observability.log(
            "warn",
            "gateway",
            format!(
                "reconnect_grace_started; grace_ms={}",
                state.reconnect_grace_timeout.as_millis()
            ),
            Some(&client_id),
            Some(&service),
            None,
            Some(&request_id),
        );
        if !wait_for_active_worker(&pool, state.reconnect_grace_timeout).await {
            state.observability.log(
                "error",
                "gateway",
                format!(
                    "reconnect_grace_expired after {} ms",
                    request_started.elapsed().as_millis()
                ),
                Some(&client_id),
                Some(&service),
                None,
                Some(&request_id),
            );
            return status_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "no built-in tunnel worker is connected",
            );
        }
        state.observability.log(
            "info",
            "gateway",
            format!(
                "reconnect_grace_recovered after {} ms; active_workers={}",
                request_started.elapsed().as_millis(),
                pool.active_workers.load(Ordering::Acquire)
            ),
            Some(&client_id),
            Some(&service),
            None,
            Some(&request_id),
        );
    }

    let body = match to_bytes(body, state.max_request_body_bytes).await {
        Ok(body) => body,
        Err(_) => return status_response(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"),
    };
    let request_headers = encode_headers(&parts.headers);
    let retry_class =
        automatic_retry_class(&request_method, service_kind, &request_path, body.as_ref());
    let request_lane = classify_request_lane(service_kind, body.as_ref());
    let operation_hash = request_operation_hash(service_kind, body.as_ref());
    let max_attempts = if retry_class.is_some() {
        MAX_SAFE_REQUEST_ATTEMPTS
    } else {
        1
    };
    let mut request_guard = state.observability.begin_request(
        &request_id,
        &request_method,
        &request_path,
        &client_id,
        &service,
    );
    if let Some(operation_hash) = operation_hash.as_deref() {
        state.observability.log(
            "debug",
            "request",
            format!(
                "request_context lane={} operation_hash={operation_hash}",
                request_lane.as_str()
            ),
            Some(&client_id),
            Some(&service),
            None,
            Some(&request_id),
        );
    }
    let mut attempt = 1_u8;

    let (head, body_rx, queue_wait_ms) = loop {
        let policy = state.policies.current(service_kind);
        let (pending_requests, pending_limit) = match request_lane {
            RequestLane::Control => (
                pool.control_pending_requests.clone(),
                CONTROL_PENDING_REQUEST_CAPACITY,
            ),
            RequestLane::Execution => (
                pool.execution_pending_requests.clone(),
                usize::from(policy.max_pending_requests),
            ),
        };
        let Some(pending_slot) = PendingRequestSlot::try_new(
            pending_requests.clone(),
            pending_limit,
            state.observability.clone(),
        ) else {
            state.observability.record_capacity_rejection();
            state.observability.log(
                "warn",
                "capacity",
                format!(
                    "worker_capacity_exhausted lane={} pending={} limit={} active_workers={} control_reserve={}",
                    request_lane.as_str(),
                    pending_requests.load(Ordering::Acquire),
                    pending_limit,
                    pool.active_workers.load(Ordering::Acquire),
                    CONTROL_WORKER_RESERVE
                ),
                Some(&client_id),
                Some(&service),
                None,
                Some(&request_id),
            );
            request_guard.finish(StatusCode::SERVICE_UNAVAILABLE.as_u16());
            return with_tunnel_request_context(
                capacity_response(
                    "worker_capacity_exhausted",
                    "tunnel worker capacity is exhausted",
                ),
                &request_id,
                request_lane,
                operation_hash.as_deref(),
            );
        };

        let acquire_deadline =
            Instant::now() + Duration::from_millis(policy.worker_acquire_timeout_ms);
        let (head_tx, head_rx) = oneshot::channel();
        let (body_tx, body_rx) = mpsc::channel(RESPONSE_BODY_CAPACITY);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (assigned_tx, assigned_rx) = oneshot::channel();
        let job = ProxyJob {
            request_id: request_id.clone(),
            method: request_method.clone(),
            path_and_query: request_path.clone(),
            headers: request_headers.clone(),
            body: body.clone(),
            lane: request_lane,
            attempt,
            enqueued_at: Instant::now(),
            policy: policy.clone(),
            active_workers: pool.active_workers.clone(),
            demand: None,
            assigned: Some(assigned_tx),
            pending_slot: Some(pending_slot),
            response_head: head_tx,
            response_body: body_tx,
            cancelled: cancel_rx,
        };

        let request_tx = match request_lane {
            RequestLane::Control => &pool.control_request_tx,
            RequestLane::Execution => &pool.execution_request_tx,
        };
        match timeout_at(acquire_deadline, request_tx.send(job)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                state.observability.log(
                    "error",
                    "gateway",
                    format!(
                        "dispatch_failed on attempt {attempt}/{max_attempts} after {} ms: request queue closed",
                        request_started.elapsed().as_millis()
                    ),
                    Some(&client_id),
                    Some(&service),
                    None,
                    Some(&request_id),
                );
                request_guard.finish(StatusCode::SERVICE_UNAVAILABLE.as_u16());
                return status_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "tunnel route is unavailable",
                );
            }
            Err(_) => {
                let _ = cancel_tx.send(true);
                state.observability.record_worker_acquire_timeout();
                state.observability.log(
                    "warn",
                    "capacity",
                    format!(
                        "worker_acquire_timeout while entering dispatcher after {} ms; timeout_ms={}; pending={}; active_workers={}",
                        request_started.elapsed().as_millis(),
                        policy.worker_acquire_timeout_ms,
                        pending_requests.load(Ordering::Acquire),
                        pool.active_workers.load(Ordering::Acquire)
                    ),
                    Some(&client_id),
                    Some(&service),
                    None,
                    Some(&request_id),
                );
                request_guard.finish(StatusCode::SERVICE_UNAVAILABLE.as_u16());
                return with_tunnel_request_context(
                    capacity_response(
                        "worker_acquire_timeout",
                        "the tunnel dispatcher did not accept the request before the queue deadline",
                    ),
                    &request_id,
                    request_lane,
                    operation_hash.as_deref(),
                );
            }
        }

        let assignment = match timeout_at(acquire_deadline, assigned_rx).await {
            Ok(Ok(assignment)) => assignment,
            Ok(Err(_)) => {
                let _ = cancel_tx.send(true);
                state.observability.log(
                    "error",
                    "gateway",
                    "worker assignment channel closed before dispatch",
                    Some(&client_id),
                    Some(&service),
                    None,
                    Some(&request_id),
                );
                request_guard.finish(StatusCode::SERVICE_UNAVAILABLE.as_u16());
                return status_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "tunnel route is unavailable",
                );
            }
            Err(_) => {
                let _ = cancel_tx.send(true);
                state.observability.record_worker_acquire_timeout();
                state.observability.log(
                    "warn",
                    "capacity",
                    format!(
                        "worker_acquire_timeout lane={} after {} ms; timeout_ms={}; pending={}; active_workers={}; control_reserve={}",
                        request_lane.as_str(),
                        request_started.elapsed().as_millis(),
                        policy.worker_acquire_timeout_ms,
                        pending_requests.load(Ordering::Acquire),
                        pool.active_workers.load(Ordering::Acquire),
                        CONTROL_WORKER_RESERVE
                    ),
                    Some(&client_id),
                    Some(&service),
                    None,
                    Some(&request_id),
                );
                request_guard.finish(StatusCode::SERVICE_UNAVAILABLE.as_u16());
                return with_tunnel_request_context(
                    capacity_response(
                        "worker_acquire_timeout",
                        "no tunnel worker became available before the queue deadline",
                    ),
                    &request_id,
                    request_lane,
                    operation_hash.as_deref(),
                );
            }
        };
        state
            .observability
            .record_worker_assignment(assignment.queue_wait_ms);

        match timeout(state.response_head_timeout, head_rx).await {
            Ok(Ok(Ok(head))) => break (head, body_rx, assignment.queue_wait_ms),
            Ok(Ok(Err(message))) => {
                let _ = cancel_tx.send(true);
                state.observability.log(
                    "error",
                    "gateway",
                    format!(
                        "response_head_error on attempt {attempt}/{max_attempts} after assignment; queue_wait_ms={}; elapsed_ms={}: {message}",
                        assignment.queue_wait_ms,
                        request_started.elapsed().as_millis()
                    ),
                    Some(&client_id),
                    Some(&service),
                    None,
                    Some(&request_id),
                );
                if attempt < max_attempts {
                    attempt += 1;
                    state.observability.log(
                        "warn",
                        "gateway",
                        format!(
                            "retrying_request attempt={attempt}/{max_attempts}; reason=response_head_error; retry_class={}; active_workers={}",
                            retry_class.unwrap_or("disabled"),
                            pool.active_workers.load(Ordering::Acquire)
                        ),
                        Some(&client_id),
                        Some(&service),
                        None,
                        Some(&request_id),
                    );
                    continue;
                }
                request_guard.finish(StatusCode::BAD_GATEWAY.as_u16());
                return with_tunnel_request_context(
                    status_response(StatusCode::BAD_GATEWAY, &message),
                    &request_id,
                    request_lane,
                    operation_hash.as_deref(),
                );
            }
            Ok(Err(_)) => {
                let _ = cancel_tx.send(true);
                state.observability.log(
                    "error",
                    "gateway",
                    format!(
                        "response_head_channel_closed on attempt {attempt}/{max_attempts} after assignment; queue_wait_ms={}; elapsed_ms={}",
                        assignment.queue_wait_ms,
                        request_started.elapsed().as_millis()
                    ),
                    Some(&client_id),
                    Some(&service),
                    None,
                    Some(&request_id),
                );
                if attempt < max_attempts {
                    attempt += 1;
                    state.observability.log(
                        "warn",
                        "gateway",
                        format!(
                            "retrying_request attempt={attempt}/{max_attempts}; reason=response_head_channel_closed; retry_class={}; active_workers={}",
                            retry_class.unwrap_or("disabled"),
                            pool.active_workers.load(Ordering::Acquire)
                        ),
                        Some(&client_id),
                        Some(&service),
                        None,
                        Some(&request_id),
                    );
                    continue;
                }
                request_guard.finish(StatusCode::BAD_GATEWAY.as_u16());
                return with_tunnel_request_context(
                    status_response(StatusCode::BAD_GATEWAY, "tunnel worker disconnected"),
                    &request_id,
                    request_lane,
                    operation_hash.as_deref(),
                );
            }
            Err(_) => {
                let _ = cancel_tx.send(true);
                state.observability.log(
                    "error",
                    "gateway",
                    format!(
                        "response_head_timeout on attempt {attempt}/{max_attempts} after worker assignment; queue_wait_ms={}; response_timeout_ms={}; active_workers={}",
                        assignment.queue_wait_ms,
                        state.response_head_timeout.as_millis(),
                        pool.active_workers.load(Ordering::Acquire)
                    ),
                    Some(&client_id),
                    Some(&service),
                    None,
                    Some(&request_id),
                );
                request_guard.finish(StatusCode::GATEWAY_TIMEOUT.as_u16());
                return with_tunnel_request_context(
                    status_response(
                        StatusCode::GATEWAY_TIMEOUT,
                        "assigned tunnel worker did not provide response headers before the deadline",
                    ),
                    &request_id,
                    request_lane,
                    operation_hash.as_deref(),
                );
            }
        }
    };

    let status = match StatusCode::from_u16(head.status) {
        Ok(status) => status,
        Err(error) => {
            state.observability.log(
                "error",
                "gateway",
                format!(
                    "invalid_response_status after {} ms: status={} error={error}",
                    request_started.elapsed().as_millis(),
                    head.status
                ),
                Some(&client_id),
                Some(&service),
                None,
                Some(&request_id),
            );
            StatusCode::BAD_GATEWAY
        }
    };
    request_guard.finish(status.as_u16());
    let mut builder = Response::builder()
        .status(status)
        .header("x-tunnel-queue-wait-ms", queue_wait_ms.to_string())
        .header("x-tunnel-request-id", &request_id)
        .header("x-tunnel-request-lane", request_lane.as_str());
    if let Some(operation_hash) = operation_hash.as_deref() {
        builder = builder.header("x-tunnel-operation-hash", operation_hash);
    }
    for header in head.headers {
        if is_hop_by_hop_header(&header.name) {
            continue;
        }
        let Ok(name) = HeaderName::from_bytes(header.name.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(&header.value) else {
            continue;
        };
        builder = builder.header(name, value);
    }
    builder
        .body(Body::from_stream(ReceiverStream::new(body_rx)))
        .unwrap_or_else(|_| status_response(StatusCode::BAD_GATEWAY, "invalid tunnel response"))
}

async fn wait_for_active_worker(pool: &ClientPool, grace: Duration) -> bool {
    if pool.active_workers.load(Ordering::Acquire) > 0 {
        return true;
    }
    let deadline = Instant::now() + grace;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        sleep(remaining.min(WORKER_POLL_INTERVAL)).await;
        if pool.active_workers.load(Ordering::Acquire) > 0 {
            return true;
        }
    }
}

fn automatic_retry_class(
    method: &str,
    service: TunnelService,
    path_and_query: &str,
    body: &[u8],
) -> Option<&'static str> {
    if matches!(method, "GET" | "HEAD" | "OPTIONS") {
        return Some("safe_http_method");
    }
    if method != "POST" {
        return None;
    }
    match service {
        TunnelService::Mcp if is_retry_safe_mcp_request(body) => Some("mcp_read_only"),
        TunnelService::Actions => action_tool_name(path_and_query)
            .filter(|name| is_retry_safe_tool_name(name))
            .map(|_| "actions_read_only"),
        _ => None,
    }
}

fn action_tool_name(path_and_query: &str) -> Option<&str> {
    let path = path_and_query.split('?').next().unwrap_or(path_and_query);
    let marker = "/actions/";
    let start = path.rfind(marker)? + marker.len();
    let name = &path[start..];
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

fn encode_headers(headers: &HeaderMap) -> Vec<HeaderPair> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            if is_hop_by_hop_header(name.as_str()) {
                return None;
            }
            value.to_str().ok().map(|value| HeaderPair {
                name: name.as_str().to_string(),
                value: value.to_string(),
            })
        })
        .collect()
}

fn node_update_state_name(state: &NodeUpdateState) -> &'static str {
    match state {
        NodeUpdateState::Downloading => "downloading",
        NodeUpdateState::Verified => "verified",
        NodeUpdateState::Draining => "draining",
        NodeUpdateState::Scheduled => "scheduled",
        NodeUpdateState::Failed => "failed",
    }
}

fn node_update_state_terminal(state: &NodeUpdateState) -> bool {
    matches!(state, NodeUpdateState::Scheduled | NodeUpdateState::Failed)
}

async fn receive_control(socket: &mut WebSocket) -> Result<ControlMessage, String> {
    loop {
        match socket.recv().await {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(text.as_str()).map_err(|error| error.to_string());
            }
            Some(Ok(Message::Ping(payload))) => socket
                .send(Message::Pong(payload))
                .await
                .map_err(|error| error.to_string())?,
            Some(Ok(Message::Close(_))) | None => return Err("websocket closed".into()),
            Some(Ok(_)) => return Err("expected a text control message".into()),
            Some(Err(error)) => return Err(error.to_string()),
        }
    }
}

async fn send_control(socket: &mut WebSocket, message: &ControlMessage) -> Result<(), String> {
    let encoded = serde_json::to_string(message).map_err(|error| error.to_string())?;
    socket
        .send(Message::Text(encoded.into()))
        .await
        .map_err(|error| error.to_string())
}

async fn send_error(
    socket: &mut WebSocket,
    request_id: Option<String>,
    message: &str,
) -> Result<(), String> {
    send_control(
        socket,
        &ControlMessage::Error {
            request_id,
            message: message.to_string(),
        },
    )
    .await
}

fn fail_response_head(
    sender: &mut Option<oneshot::Sender<Result<ResponseHeadData, String>>>,
    message: String,
) {
    if let Some(sender) = sender.take() {
        let _ = sender.send(Err(message));
    }
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().map(str::trim)
}

fn capacity_response(reason: &str, message: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header("content-type", "text/plain; charset=utf-8")
        .header("retry-after", "1")
        .header("x-tunnel-error", reason)
        .body(Body::from(message.to_string()))
        .expect("static capacity response")
}

fn status_response(status: StatusCode, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(Body::from(message.to_string()))
        .expect("static response")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use coding_tools_tunnel_protocol::{DeviceAuthProof, WorkerPolicy};
    use ed25519_dalek::{Signer, SigningKey};
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    fn test_server_identity() -> ServerIdentity {
        let directory = tempfile::tempdir().expect("server identity tempdir");
        ServerIdentity::load_or_create(&directory.path().join("server-identity.json"))
            .expect("test server identity")
    }

    #[tokio::test]
    async fn terminal_update_status_clears_only_the_matching_offer() {
        let registry = Registry::default();
        let key = ClientKey {
            client_id: "pc-a".into(),
            service: TunnelService::Mcp,
        };
        let pool = registry.register(key).await;
        let offer = NodeUpdateOffer {
            update_id: "update-1".into(),
            version: "0.29.34".into(),
            url: "https://updates.example.test/ctnode.zip".into(),
            sha256: "a".repeat(64),
            signature: "b".repeat(86),
        };
        registry.dispatch_node_update("pc-a", offer.clone()).await;
        assert_eq!(pool.update_tx.borrow().as_ref(), Some(&offer));

        assert!(!registry.clear_node_update("pc-a", "older-update").await);
        assert_eq!(pool.update_tx.borrow().as_ref(), Some(&offer));

        assert!(registry.clear_node_update("pc-a", "update-1").await);
        assert!(pool.update_tx.borrow().is_none());
        assert!(!registry.clear_node_update("pc-a", "update-1").await);
    }

    #[test]
    fn terminal_update_state_is_scheduled_or_failed_only() {
        assert!(!node_update_state_terminal(&NodeUpdateState::Downloading));
        assert!(!node_update_state_terminal(&NodeUpdateState::Verified));
        assert!(!node_update_state_terminal(&NodeUpdateState::Draining));
        assert!(node_update_state_terminal(&NodeUpdateState::Scheduled));
        assert!(node_update_state_terminal(&NodeUpdateState::Failed));
    }

    #[test]
    fn dispatcher_discards_closed_worker_slots_before_assignment() {
        let (closed_assign, closed_receiver) = oneshot::channel::<ProxyJob>();
        drop(closed_receiver);
        let (live_assign, _live_receiver) = oneshot::channel::<ProxyJob>();
        let mut workers = VecDeque::from([
            AvailableWorker {
                assign: closed_assign,
            },
            AvailableWorker {
                assign: live_assign,
            },
        ]);

        assert_eq!(discard_closed_workers(&mut workers), 1);
        assert_eq!(workers.len(), 1);
        assert!(!workers.front().expect("live worker").assign.is_closed());
    }

    async fn start_test_server(
        devices: DeviceRegistry,
        registry: Registry,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let policies = WorkerPolicyStore::from_writer(devices.database_writer())
            .expect("test worker policies");
        let state = AppState {
            registry,
            devices,
            policies,
            observability: Observability::new(),
            server_identity: test_server_identity(),
            max_request_body_bytes: MAX_REQUEST_BODY_BYTES,
            response_head_timeout: RESPONSE_HEAD_TIMEOUT,
            reconnect_grace_timeout: RECONNECT_GRACE_TIMEOUT,
            worker_liveness_timeout: WORKER_LIVENESS_TIMEOUT,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test address");
        let server = tokio::spawn(async move {
            axum::serve(listener, build_app(state))
                .await
                .expect("serve");
        });
        (address, server)
    }

    async fn start_test_server_with_policies(
        devices: DeviceRegistry,
        registry: Registry,
        policies: WorkerPolicyStore,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let state = AppState {
            registry,
            devices,
            policies,
            observability: Observability::new(),
            server_identity: test_server_identity(),
            max_request_body_bytes: MAX_REQUEST_BODY_BYTES,
            response_head_timeout: RESPONSE_HEAD_TIMEOUT,
            reconnect_grace_timeout: RECONNECT_GRACE_TIMEOUT,
            worker_liveness_timeout: WORKER_LIVENESS_TIMEOUT,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test address");
        let server = tokio::spawn(async move {
            axum::serve(listener, build_app(state))
                .await
                .expect("serve");
        });
        (address, server)
    }

    async fn start_test_server_with_timeouts(
        devices: DeviceRegistry,
        registry: Registry,
        response_head_timeout: Duration,
        reconnect_grace_timeout: Duration,
        worker_liveness_timeout: Duration,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let policies = WorkerPolicyStore::from_writer(devices.database_writer())
            .expect("test worker policies");
        let state = AppState {
            registry,
            devices,
            policies,
            observability: Observability::new(),
            server_identity: test_server_identity(),
            max_request_body_bytes: MAX_REQUEST_BODY_BYTES,
            response_head_timeout,
            reconnect_grace_timeout,
            worker_liveness_timeout,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test address");
        let server = tokio::spawn(async move {
            axum::serve(listener, build_app(state))
                .await
                .expect("serve");
        });
        (address, server)
    }

    async fn enroll_test_device(
        address: SocketAddr,
        devices: &DeviceRegistry,
    ) -> (SigningKey, String) {
        let grant = devices
            .create_enrollment("pc-a", AllowedServices::Both, Some(60))
            .expect("enrollment grant");
        let signing_key = SigningKey::from_bytes(&[17_u8; 32]);
        let enrollment = reqwest::Client::new()
            .post(format!(
                "http://{address}{ENROLL_PATH_PREFIX}/{}",
                grant.code
            ))
            .json(&EnrollmentRequest {
                device_id: "device-resilience".into(),
                client_id: "pc-a".into(),
                device_name: "resilience test".into(),
                public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes()),
            })
            .send()
            .await
            .expect("enrollment request");
        assert_eq!(enrollment.status(), StatusCode::CREATED);
        let enrolled = enrollment
            .json::<coding_tools_tunnel_protocol::EnrollmentResponse>()
            .await
            .expect("enrollment response");
        (signing_key, enrolled.device_id)
    }

    async fn finish_worker_auth(
        worker: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> WorkerPolicy {
        let hello_ack = worker
            .next()
            .await
            .expect("hello ack frame")
            .expect("hello ack");
        let WsMessage::Text(hello_ack) = hello_ack else {
            panic!("expected hello ack text");
        };
        let ControlMessage::HelloAck {
            protocol_version,
            worker_policy,
            ..
        } = serde_json::from_str::<ControlMessage>(hello_ack.as_ref()).expect("hello ack json")
        else {
            panic!("expected hello ack control");
        };
        assert_eq!(protocol_version, PROTOCOL_VERSION);
        worker
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::Ready)
                    .expect("ready json")
                    .into(),
            ))
            .await
            .expect("send ready");
        worker_policy
    }

    async fn authenticate_worker(
        address: SocketAddr,
        signing_key: &SigningKey,
        device_id: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let mut request = format!("ws://{address}{WS_PATH}")
            .into_client_request()
            .expect("websocket request");
        request
            .headers_mut()
            .insert(CLIENT_ID_HEADER, "pc-a".parse().expect("client id"));
        request
            .headers_mut()
            .insert(SERVICE_HEADER, "mcp".parse().expect("service"));
        request
            .headers_mut()
            .insert(DEVICE_ID_HEADER, device_id.parse().expect("device id"));
        request
            .headers_mut()
            .insert(WORKER_ID_HEADER, "worker-1".parse().expect("worker id"));
        request.headers_mut().insert(
            SEC_WEBSOCKET_PROTOCOL,
            WS_SUBPROTOCOL.parse().expect("subprotocol"),
        );
        let (mut worker, response) = connect_async(request).await.expect("connect worker");
        assert_eq!(
            response
                .headers()
                .get(SEC_WEBSOCKET_PROTOCOL)
                .and_then(|value| value.to_str().ok()),
            Some(WS_SUBPROTOCOL)
        );

        let challenge = worker
            .next()
            .await
            .expect("challenge frame")
            .expect("challenge");
        let WsMessage::Text(challenge) = challenge else {
            panic!("expected challenge text");
        };
        let ControlMessage::Challenge {
            nonce,
            expires_at_unix_ms,
            ..
        } = serde_json::from_str::<ControlMessage>(challenge.as_ref()).expect("challenge json")
        else {
            panic!("expected challenge");
        };
        assert!(expires_at_unix_ms >= unix_ms());

        let mut proof = DeviceAuthProof {
            hello: ClientHello {
                protocol_version: PROTOCOL_VERSION,
                client_id: "pc-a".into(),
                service: TunnelService::Mcp,
                worker_id: "worker-1".into(),
            },
            device_id: device_id.into(),
            signature: String::new(),
        };
        proof.signature = URL_SAFE_NO_PAD.encode(
            signing_key
                .sign(&coding_tools_tunnel_protocol::auth_signing_payload(
                    &nonce, &proof,
                ))
                .to_bytes(),
        );
        worker
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::Authenticate(proof))
                    .expect("auth json")
                    .into(),
            ))
            .await
            .expect("send auth");
        worker
    }

    #[tokio::test]
    async fn public_listener_does_not_mount_admin_routes() {
        let directory = tempfile::tempdir().expect("tempdir");
        let devices =
            DeviceRegistry::open(directory.path().join("devices.db")).expect("device registry");
        let (address, server) = start_test_server(devices, Registry::default()).await;

        for path in ["/api/devices", "/api/enrollments", "/_tunnel/admin"] {
            let response = reqwest::get(format!("http://{address}{path}"))
                .await
                .expect("public request");
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "path={path}");
        }

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn enrolls_and_proxies_http_over_a_signed_websocket_worker() {
        let directory = tempfile::tempdir().expect("tempdir");
        let devices =
            DeviceRegistry::open(directory.path().join("devices.db")).expect("device registry");
        let grant = devices
            .create_enrollment("pc-a", AllowedServices::Both, Some(60))
            .expect("enrollment grant");
        let (address, server) = start_test_server(devices.clone(), Registry::default()).await;
        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let enrollment = reqwest::Client::new()
            .post(format!(
                "http://{address}{ENROLL_PATH_PREFIX}/{}",
                grant.code
            ))
            .json(&EnrollmentRequest {
                device_id: "device-1".into(),
                client_id: "pc-a".into(),
                device_name: "test device".into(),
                public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes()),
            })
            .send()
            .await
            .expect("enrollment request");
        assert_eq!(enrollment.status(), StatusCode::CREATED);
        let enrolled = enrollment
            .json::<coding_tools_tunnel_protocol::EnrollmentResponse>()
            .await
            .expect("enrollment response");
        assert_eq!(enrolled.device_id, "device-1");

        let mut worker = authenticate_worker(address, &signing_key, &enrolled.device_id).await;
        let hello_ack = worker
            .next()
            .await
            .expect("hello ack frame")
            .expect("hello ack");
        let WsMessage::Text(hello_ack) = hello_ack else {
            panic!("expected hello ack text");
        };
        assert!(matches!(
            serde_json::from_str::<ControlMessage>(hello_ack.as_ref()).expect("hello ack json"),
            ControlMessage::HelloAck { protocol_version: PROTOCOL_VERSION, worker_policy, .. }
                if worker_policy == WorkerPolicy::default_for(TunnelService::Mcp)
        ));
        worker
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::Ready)
                    .expect("ready json")
                    .into(),
            ))
            .await
            .expect("send ready");

        let public_request = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("http://{address}/builtin/clients/pc-a/mcp?probe=1"))
                .header("x-test", "yes")
                .body("ping")
                .send()
                .await
                .expect("public request")
        });

        let request_head = worker
            .next()
            .await
            .expect("request head frame")
            .expect("request head");
        let WsMessage::Text(request_head) = request_head else {
            panic!("expected request head text");
        };
        let ControlMessage::RequestHead {
            request_id,
            method,
            path_and_query,
            headers,
            ..
        } = serde_json::from_str::<ControlMessage>(request_head.as_ref())
            .expect("request head json")
        else {
            panic!("expected request head");
        };
        assert_eq!(method, "POST");
        assert_eq!(path_and_query, "/builtin/clients/pc-a/mcp?probe=1");
        assert!(headers
            .iter()
            .any(|header| header.name == "x-test" && header.value == "yes"));
        assert!(headers.iter().all(|header| header.name != "content-length"));

        let body = worker.next().await.expect("body frame").expect("body");
        assert_eq!(body, WsMessage::Binary(Bytes::from_static(b"ping")));
        let request_end = worker
            .next()
            .await
            .expect("request end frame")
            .expect("request end");
        let WsMessage::Text(request_end) = request_end else {
            panic!("expected request end text");
        };
        assert_eq!(
            serde_json::from_str::<ControlMessage>(request_end.as_ref()).expect("request end json"),
            ControlMessage::RequestEnd {
                request_id: request_id.clone()
            }
        );

        worker
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::ResponseHead {
                    request_id: request_id.clone(),
                    status: 201,
                    headers: vec![HeaderPair {
                        name: "content-type".into(),
                        value: "text/plain".into(),
                    }],
                })
                .expect("response head json")
                .into(),
            ))
            .await
            .expect("send response head");
        worker
            .send(WsMessage::Binary(Bytes::from_static(b"pong")))
            .await
            .expect("send response body");
        worker
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::ResponseEnd { request_id })
                    .expect("response end json")
                    .into(),
            ))
            .await
            .expect("send response end");

        let response = public_request.await.expect("public request task");
        assert_eq!(response.status(), 201);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/plain"
        );
        assert_eq!(response.text().await.expect("response text"), "pong");

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn revoked_device_is_rejected_during_websocket_authentication() {
        let directory = tempfile::tempdir().expect("tempdir");
        let devices =
            DeviceRegistry::open(directory.path().join("devices.db")).expect("device registry");
        let grant = devices
            .create_enrollment("pc-a", AllowedServices::Mcp, Some(60))
            .expect("enrollment grant");
        let signing_key = SigningKey::from_bytes(&[9_u8; 32]);
        devices
            .enroll(
                grant.code,
                EnrollmentRequest {
                    device_id: "device-2".into(),
                    client_id: "pc-a".into(),
                    device_name: "revoked device".into(),
                    public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes()),
                },
            )
            .await
            .expect("enroll device");
        assert!(devices.revoke_device("device-2").expect("revoke device"));
        let (address, server) = start_test_server(devices, Registry::default()).await;
        let mut worker = authenticate_worker(address, &signing_key, "device-2").await;
        let rejection = worker
            .next()
            .await
            .expect("rejection frame")
            .expect("rejection");
        let WsMessage::Text(rejection) = rejection else {
            panic!("expected rejection text");
        };
        assert_eq!(
            serde_json::from_str::<ControlMessage>(rejection.as_ref()).expect("rejection json"),
            ControlMessage::Error {
                request_id: None,
                message: "tunnel device has been revoked".into(),
            }
        );
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn registered_route_without_workers_waits_for_reconnect_grace() {
        let directory = tempfile::tempdir().expect("tempdir");
        let devices =
            DeviceRegistry::open(directory.path().join("devices.db")).expect("device registry");
        let registry = Registry::default();
        registry
            .register(ClientKey {
                client_id: "pc-a".into(),
                service: TunnelService::Mcp,
            })
            .await;
        let (address, server) = start_test_server_with_timeouts(
            devices,
            registry,
            Duration::from_secs(1),
            Duration::from_millis(125),
            Duration::from_secs(1),
        )
        .await;

        let started = std::time::Instant::now();
        let response = reqwest::get(format!("http://{address}/builtin/clients/pc-a/mcp"))
            .await
            .expect("public request");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            response.text().await.expect("response text"),
            "no built-in tunnel worker is connected"
        );

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn reconnect_grace_accepts_a_worker_that_returns_before_expiry() {
        let directory = tempfile::tempdir().expect("tempdir");
        let devices =
            DeviceRegistry::open(directory.path().join("devices.db")).expect("device registry");
        let registry = Registry::default();
        registry
            .register(ClientKey {
                client_id: "pc-a".into(),
                service: TunnelService::Mcp,
            })
            .await;
        let (address, server) = start_test_server_with_timeouts(
            devices.clone(),
            registry,
            Duration::from_secs(1),
            Duration::from_millis(500),
            Duration::from_secs(2),
        )
        .await;
        let (signing_key, device_id) = enroll_test_device(address, &devices).await;

        let public_request = tokio::spawn(async move {
            reqwest::get(format!("http://{address}/builtin/clients/pc-a/mcp?grace=1"))
                .await
                .expect("grace public request")
        });
        tokio::time::sleep(Duration::from_millis(75)).await;
        let mut worker = authenticate_worker(address, &signing_key, &device_id).await;
        finish_worker_auth(&mut worker).await;

        let request_head = worker
            .next()
            .await
            .expect("grace request head")
            .expect("grace request head");
        let WsMessage::Text(request_head) = request_head else {
            panic!("expected grace request head text");
        };
        let ControlMessage::RequestHead { request_id, .. } =
            serde_json::from_str::<ControlMessage>(request_head.as_ref())
                .expect("grace request head json")
        else {
            panic!("expected grace request head control");
        };
        let _request_end = worker
            .next()
            .await
            .expect("grace request end")
            .expect("grace request end");
        worker
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::ResponseHead {
                    request_id: request_id.clone(),
                    status: 204,
                    headers: vec![],
                })
                .expect("grace response head")
                .into(),
            ))
            .await
            .expect("send grace response head");
        worker
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::ResponseEnd { request_id })
                    .expect("grace response end")
                    .into(),
            ))
            .await
            .expect("send grace response end");

        assert_eq!(
            public_request.await.expect("grace request task").status(),
            StatusCode::NO_CONTENT
        );

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn safe_get_retries_after_worker_disconnects_before_response_head() {
        let directory = tempfile::tempdir().expect("tempdir");
        let devices =
            DeviceRegistry::open(directory.path().join("devices.db")).expect("device registry");
        let (address, server) = start_test_server_with_timeouts(
            devices.clone(),
            Registry::default(),
            Duration::from_secs(2),
            Duration::from_millis(500),
            Duration::from_secs(2),
        )
        .await;
        let (signing_key, device_id) = enroll_test_device(address, &devices).await;
        let mut first_worker = authenticate_worker(address, &signing_key, &device_id).await;
        finish_worker_auth(&mut first_worker).await;

        let public_request = tokio::spawn(async move {
            reqwest::get(format!("http://{address}/builtin/clients/pc-a/mcp?retry=1"))
                .await
                .expect("retry public request")
        });
        let first_head = first_worker
            .next()
            .await
            .expect("first retry head")
            .expect("first retry head");
        let WsMessage::Text(first_head) = first_head else {
            panic!("expected first retry head text");
        };
        let ControlMessage::RequestHead {
            request_id: first_request_id,
            ..
        } = serde_json::from_str::<ControlMessage>(first_head.as_ref())
            .expect("first retry head json")
        else {
            panic!("expected first retry request head");
        };
        let _first_end = first_worker
            .next()
            .await
            .expect("first retry end")
            .expect("first retry end");
        first_worker
            .send(WsMessage::Close(None))
            .await
            .expect("close first worker");
        drop(first_worker);

        let mut replacement = authenticate_worker(address, &signing_key, &device_id).await;
        finish_worker_auth(&mut replacement).await;
        let retry_head = replacement
            .next()
            .await
            .expect("replacement retry head")
            .expect("replacement retry head");
        let WsMessage::Text(retry_head) = retry_head else {
            panic!("expected replacement retry head text");
        };
        let ControlMessage::RequestHead {
            request_id: retry_request_id,
            ..
        } = serde_json::from_str::<ControlMessage>(retry_head.as_ref())
            .expect("replacement retry head json")
        else {
            panic!("expected replacement retry request head");
        };
        assert_eq!(retry_request_id, first_request_id);
        let _retry_end = replacement
            .next()
            .await
            .expect("replacement retry end")
            .expect("replacement retry end");
        replacement
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::ResponseHead {
                    request_id: retry_request_id.clone(),
                    status: 204,
                    headers: vec![],
                })
                .expect("replacement retry response head")
                .into(),
            ))
            .await
            .expect("send replacement retry response head");
        replacement
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::ResponseEnd {
                    request_id: retry_request_id,
                })
                .expect("replacement retry response end")
                .into(),
            ))
            .await
            .expect("send replacement retry response end");

        assert_eq!(
            public_request.await.expect("retry request task").status(),
            StatusCode::NO_CONTENT
        );

        server.abort();
        let _ = server.await;
    }

    #[test]
    fn automatic_retry_is_limited_to_read_only_operations() {
        assert_eq!(
            automatic_retry_class("GET", TunnelService::Mcp, "/mcp", b""),
            Some("safe_http_method")
        );

        let read = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "read_file", "arguments": {"path": "README.md"}}
        }))
        .unwrap();
        let write = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "apply_patch", "arguments": {"patch": "test"}}
        }))
        .unwrap();

        assert_eq!(
            automatic_retry_class("POST", TunnelService::Mcp, "/mcp", &read),
            Some("mcp_read_only")
        );
        assert_eq!(
            automatic_retry_class("POST", TunnelService::Mcp, "/mcp", &write),
            None
        );
        assert_eq!(
            automatic_retry_class(
                "POST",
                TunnelService::Actions,
                "/builtin/actions/pc-a/actions/read_file",
                b"{}"
            ),
            Some("actions_read_only")
        );
        assert_eq!(
            automatic_retry_class(
                "POST",
                TunnelService::Actions,
                "/builtin/actions/pc-a/actions/apply_patch",
                b"{}"
            ),
            None
        );
        assert_eq!(
            automatic_retry_class("PUT", TunnelService::Mcp, "/mcp", &read),
            None
        );
    }

    #[test]
    fn mcp_control_lane_classification_keeps_reattach_and_session_control_off_execution_lane() {
        let call = |name: &str, arguments: serde_json::Value| {
            serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments }
            }))
            .expect("request json")
        };
        for name in [
            "exec_health_check",
            "wait_command",
            "resolve_operation",
            "list_sessions",
            "send_input",
            "kill_session",
            "read_output",
            "request_permissions",
        ] {
            assert_eq!(
                classify_request_lane(TunnelService::Mcp, &call(name, serde_json::json!({}))),
                RequestLane::Control
            );
        }
        assert_eq!(
            classify_request_lane(
                TunnelService::Mcp,
                &call(
                    "exec_many",
                    serde_json::json!({ "operation_id": "graph-1" })
                )
            ),
            RequestLane::Control
        );
        assert_eq!(
            classify_request_lane(
                TunnelService::Mcp,
                &call(
                    "exec_many",
                    serde_json::json!({ "operation_id": "graph-1", "action": "cancel" })
                )
            ),
            RequestLane::Control
        );
        assert_eq!(
            classify_request_lane(
                TunnelService::Mcp,
                &call(
                    "exec_many",
                    serde_json::json!({ "operation_id": "graph-1", "commands": [{ "id": "a", "program": "echo" }] })
                )
            ),
            RequestLane::Execution
        );
        assert_eq!(
            classify_request_lane(
                TunnelService::Mcp,
                &call("exec_command", serde_json::json!({}))
            ),
            RequestLane::Execution
        );
    }

    #[tokio::test]
    async fn saturated_route_uses_bounded_queue_and_returns_explicit_503_errors() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("devices.db");
        let devices = DeviceRegistry::open(&database_path).expect("device registry");
        let policies =
            WorkerPolicyStore::from_writer(devices.database_writer()).expect("policy store");
        let mut policy = policies.current(TunnelService::Mcp);
        policy.start_workers = 1;
        policy.min_idle_workers = 1;
        policy.max_idle_workers = 1;
        policy.max_workers = 4;
        policy.max_pending_requests = 1;
        policy.worker_acquire_timeout_ms = 150;
        policies
            .update(TunnelService::Mcp, policy)
            .expect("save capacity policy");
        let (address, server) =
            start_test_server_with_policies(devices.clone(), Registry::default(), policies).await;
        let (signing_key, device_id) = enroll_test_device(address, &devices).await;
        let mut worker = authenticate_worker(address, &signing_key, &device_id).await;
        finish_worker_auth(&mut worker).await;

        let first_request = tokio::spawn(async move {
            reqwest::get(format!("http://{address}/builtin/clients/pc-a/mcp?busy=1"))
                .await
                .expect("busy public request")
        });
        let first_head = worker.next().await.expect("busy head").expect("busy head");
        let WsMessage::Text(first_head) = first_head else {
            panic!("expected busy request head");
        };
        let ControlMessage::RequestHead {
            demand: Some(demand),
            ..
        } = serde_json::from_str::<ControlMessage>(first_head.as_ref())
            .expect("busy request head json")
        else {
            panic!("expected demand hint on request head");
        };
        assert_eq!(demand.queued_requests, 1);
        assert_eq!(demand.desired_workers, 2);
        let _first_end = worker.next().await.expect("busy end").expect("busy end");

        let waiting_request = tokio::spawn(async move {
            reqwest::get(format!(
                "http://{address}/builtin/clients/pc-a/mcp?waiting=1"
            ))
            .await
            .expect("waiting public request")
        });
        tokio::time::sleep(Duration::from_millis(25)).await;

        let rejected = reqwest::get(format!(
            "http://{address}/builtin/clients/pc-a/mcp?rejected=1"
        ))
        .await
        .expect("capacity rejection");
        assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            rejected
                .headers()
                .get("x-tunnel-error")
                .and_then(|value| value.to_str().ok()),
            Some("worker_capacity_exhausted")
        );
        assert_eq!(
            rejected
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok()),
            Some("1")
        );

        let waiting = tokio::time::timeout(Duration::from_secs(1), waiting_request)
            .await
            .expect("worker acquire timeout deadline")
            .expect("waiting task");
        assert_eq!(waiting.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            waiting
                .headers()
                .get("x-tunnel-error")
                .and_then(|value| value.to_str().ok()),
            Some("worker_acquire_timeout")
        );
        assert_eq!(
            waiting
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok()),
            Some("1")
        );

        first_request.abort();
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn response_head_timeout_cancels_and_releases_the_worker() {
        let directory = tempfile::tempdir().expect("tempdir");
        let devices =
            DeviceRegistry::open(directory.path().join("devices.db")).expect("device registry");
        let (address, server) = start_test_server_with_timeouts(
            devices.clone(),
            Registry::default(),
            Duration::from_millis(150),
            Duration::from_millis(100),
            Duration::from_secs(2),
        )
        .await;
        let (signing_key, device_id) = enroll_test_device(address, &devices).await;
        let mut worker = authenticate_worker(address, &signing_key, &device_id).await;
        finish_worker_auth(&mut worker).await;

        let first_request = tokio::spawn(async move {
            reqwest::get(format!("http://{address}/builtin/clients/pc-a/mcp?first=1"))
                .await
                .expect("first public request")
        });
        let first_head = worker
            .next()
            .await
            .expect("first head")
            .expect("first head");
        let WsMessage::Text(first_head) = first_head else {
            panic!("expected first request head");
        };
        let ControlMessage::RequestHead { request_id, .. } =
            serde_json::from_str::<ControlMessage>(first_head.as_ref()).expect("first head json")
        else {
            panic!("expected first request head control");
        };
        let _first_end = worker.next().await.expect("first end").expect("first end");

        let cancelled = tokio::time::timeout(Duration::from_secs(1), worker.next())
            .await
            .expect("cancel deadline")
            .expect("cancel frame")
            .expect("cancel frame result");
        let WsMessage::Text(cancelled) = cancelled else {
            panic!("expected cancel text");
        };
        assert_eq!(
            serde_json::from_str::<ControlMessage>(cancelled.as_ref()).expect("cancel json"),
            ControlMessage::Cancel {
                request_id: request_id.clone()
            }
        );
        assert_eq!(
            first_request.await.expect("first task").status(),
            StatusCode::GATEWAY_TIMEOUT
        );

        worker
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::Ready)
                    .expect("ready json")
                    .into(),
            ))
            .await
            .expect("ready after cancel");
        let second_request = tokio::spawn(async move {
            reqwest::get(format!(
                "http://{address}/builtin/clients/pc-a/mcp?second=1"
            ))
            .await
            .expect("second public request")
        });
        let second_head = worker
            .next()
            .await
            .expect("second head")
            .expect("second head");
        let WsMessage::Text(second_head) = second_head else {
            panic!("expected second request head");
        };
        let ControlMessage::RequestHead {
            request_id: second_id,
            ..
        } = serde_json::from_str::<ControlMessage>(second_head.as_ref()).expect("second head json")
        else {
            panic!("expected second request head control");
        };
        let _second_end = worker
            .next()
            .await
            .expect("second end")
            .expect("second end");
        worker
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::ResponseHead {
                    request_id: second_id.clone(),
                    status: 204,
                    headers: vec![],
                })
                .expect("second response head")
                .into(),
            ))
            .await
            .expect("send second response head");
        worker
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::ResponseEnd {
                    request_id: second_id,
                })
                .expect("second response end")
                .into(),
            ))
            .await
            .expect("send second response end");
        assert_eq!(
            second_request.await.expect("second task").status(),
            StatusCode::NO_CONTENT
        );

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn stale_worker_expires_and_same_device_can_reconnect() {
        let directory = tempfile::tempdir().expect("tempdir");
        let devices =
            DeviceRegistry::open(directory.path().join("devices.db")).expect("device registry");
        let (address, server) = start_test_server_with_timeouts(
            devices.clone(),
            Registry::default(),
            Duration::from_secs(1),
            Duration::from_millis(100),
            Duration::from_millis(100),
        )
        .await;
        let (signing_key, device_id) = enroll_test_device(address, &devices).await;
        let mut stale_worker = authenticate_worker(address, &signing_key, &device_id).await;
        finish_worker_auth(&mut stale_worker).await;

        tokio::time::sleep(Duration::from_millis(175)).await;
        let unavailable =
            reqwest::get(format!("http://{address}/builtin/clients/pc-a/mcp?stale=1"))
                .await
                .expect("stale route request");
        assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);

        let mut replacement = authenticate_worker(address, &signing_key, &device_id).await;
        finish_worker_auth(&mut replacement).await;
        let public_request = tokio::spawn(async move {
            reqwest::get(format!(
                "http://{address}/builtin/clients/pc-a/mcp?reconnected=1"
            ))
            .await
            .expect("reconnected request")
        });
        let request_head = replacement
            .next()
            .await
            .expect("replacement head")
            .expect("replacement head");
        let WsMessage::Text(request_head) = request_head else {
            panic!("expected replacement request head");
        };
        let ControlMessage::RequestHead { request_id, .. } =
            serde_json::from_str::<ControlMessage>(request_head.as_ref())
                .expect("replacement head json")
        else {
            panic!("expected replacement request head control");
        };
        let _request_end = replacement
            .next()
            .await
            .expect("replacement end")
            .expect("replacement end");
        replacement
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::ResponseHead {
                    request_id: request_id.clone(),
                    status: 204,
                    headers: vec![],
                })
                .expect("replacement response head")
                .into(),
            ))
            .await
            .expect("send replacement response head");
        replacement
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::ResponseEnd { request_id })
                    .expect("replacement response end")
                    .into(),
            ))
            .await
            .expect("send replacement response end");
        assert_eq!(
            public_request.await.expect("reconnected task").status(),
            StatusCode::NO_CONTENT
        );

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn handshake_uses_saved_policy_and_pushes_idle_revisions() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("devices.db");
        let devices = DeviceRegistry::open(&database_path).expect("device registry");
        let policies =
            WorkerPolicyStore::from_writer(devices.database_writer()).expect("policy store");
        let mut policy = policies.current(TunnelService::Mcp);
        policy.start_workers = 6;
        policy.max_idle_workers = 8;
        policy.max_workers = 24;
        let policy = policies
            .update(TunnelService::Mcp, policy)
            .expect("save initial policy");
        let (address, server) =
            start_test_server_with_policies(devices.clone(), Registry::default(), policies.clone())
                .await;
        let (signing_key, device_id) = enroll_test_device(address, &devices).await;
        let mut worker = authenticate_worker(address, &signing_key, &device_id).await;

        let hello_ack = worker.next().await.expect("hello ack").expect("hello ack");
        let WsMessage::Text(hello_ack) = hello_ack else {
            panic!("expected hello ack text");
        };
        assert!(matches!(
            serde_json::from_str::<ControlMessage>(hello_ack.as_ref()).expect("hello ack json"),
            ControlMessage::HelloAck { protocol_version: PROTOCOL_VERSION, worker_policy, .. }
                if worker_policy == policy
        ));
        worker
            .send(WsMessage::Text(
                serde_json::to_string(&ControlMessage::Ready)
                    .expect("ready")
                    .into(),
            ))
            .await
            .expect("send ready");

        let mut changed = policy;
        changed.max_workers = 32;
        let changed = policies
            .update(TunnelService::Mcp, changed)
            .expect("save changed policy");
        let update = timeout(Duration::from_secs(1), worker.next())
            .await
            .expect("policy update timeout")
            .expect("policy update frame")
            .expect("policy update");
        let WsMessage::Text(update) = update else {
            panic!("expected policy update text");
        };
        assert_eq!(
            serde_json::from_str::<ControlMessage>(update.as_ref()).expect("policy update json"),
            ControlMessage::PolicyUpdate {
                worker_policy: changed,
            }
        );

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn route_policy_refuses_workers_above_the_server_maximum() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("devices.db");
        let devices = DeviceRegistry::open(&database_path).expect("device registry");
        let policies =
            WorkerPolicyStore::from_writer(devices.database_writer()).expect("policy store");
        let mut policy = policies.current(TunnelService::Mcp);
        policy.start_workers = 1;
        policy.min_idle_workers = 1;
        policy.max_idle_workers = 1;
        policy.max_workers = 1;
        policies
            .update(TunnelService::Mcp, policy)
            .expect("save cap policy");
        let (address, server) =
            start_test_server_with_policies(devices.clone(), Registry::default(), policies).await;
        let (signing_key, device_id) = enroll_test_device(address, &devices).await;

        let mut first = authenticate_worker(address, &signing_key, &device_id).await;
        finish_worker_auth(&mut first).await;
        let mut second = authenticate_worker(address, &signing_key, &device_id).await;
        let rejection = second
            .next()
            .await
            .expect("worker cap frame")
            .expect("worker cap response");
        let WsMessage::Text(rejection) = rejection else {
            panic!("expected worker cap text");
        };
        assert_eq!(
            serde_json::from_str::<ControlMessage>(rejection.as_ref()).expect("worker cap json"),
            ControlMessage::Error {
                request_id: None,
                message: "worker limit reached for this route".into(),
            }
        );

        server.abort();
        let _ = server.await;
    }
}
