use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, PasswordHasher, SaltString};
use argon2::{Argon2, PasswordVerifier};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, REFERRER_POLICY, SET_COOKIE,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, Semaphore};

use crate::device_auth::{AllowedServices, DeviceAuthError, DeviceRegistry, DeviceSummary};
use crate::observability::{LogFilter, Observability};
use crate::worker_policy::{WorkerPolicyError, WorkerPolicyStore};
use crate::Registry;
use coding_tools_tunnel_protocol::{valid_client_id, NodeUpdateOffer, TunnelService, WorkerPolicy};

const ADMIN_HTML: &str = include_str!("admin.html");
const ADMIN_LOGIN_HTML: &str = include_str!("admin_login.html");
const ADMIN_USERNAME_ENV: &str = "CODING_TOOLS_TUNNEL_ADMIN_USERNAME";
const ADMIN_PASSWORD_ENV: &str = "CODING_TOOLS_TUNNEL_ADMIN_PASSWORD";
const ADMIN_PASSWORD_FILE_ENV: &str = "CODING_TOOLS_TUNNEL_ADMIN_PASSWORD_FILE";
const ADMIN_SESSION_SECONDS_ENV: &str = "CODING_TOOLS_TUNNEL_ADMIN_SESSION_SECONDS";
const ADMIN_SESSION_COOKIE: &str = "__Host-coding_tools_admin_session";
const CSRF_HEADER: HeaderName = HeaderName::from_static("x-csrf-token");
const DEFAULT_SESSION_SECONDS: u64 = 8 * 60 * 60;
const MIN_PASSWORD_BYTES: usize = 12;
const MAX_USERNAME_BYTES: usize = 128;
const MAX_PASSWORD_BYTES: usize = 4096;
const MAX_CONCURRENT_PASSWORD_CHECKS: usize = 2;
const MIN_SESSION_SECONDS: u64 = 5 * 60;
const MAX_SESSION_SECONDS: u64 = 7 * 24 * 60 * 60;

#[derive(Clone)]
pub struct AdminConfig {
    username: String,
    password: String,
    session_ttl: Duration,
}

#[derive(Clone)]
struct AdminState {
    devices: DeviceRegistry,
    policies: WorkerPolicyStore,
    observability: Observability,
    registry: Registry,
    public_origin: String,
    username: String,
    username_digest: [u8; 32],
    password_hash: Arc<String>,
    sessions: SessionStore,
    login_limit: Arc<Semaphore>,
}

#[derive(Clone)]
struct SessionStore {
    entries: Arc<Mutex<HashMap<[u8; 32], AdminSession>>>,
    ttl: Duration,
}

#[derive(Clone)]
struct AdminSession {
    username: String,
    csrf_token: String,
    expires_at: Instant,
}

#[derive(Debug, Deserialize)]
struct LoginInput {
    username: String,
    password: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionOutput {
    authenticated: bool,
    username: String,
    csrf_token: String,
}

#[derive(Debug, Deserialize)]
struct CreateEnrollmentInput {
    client_id: String,
    service: String,
    ttl_seconds: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CreateEnrollmentOutput {
    enrollment_url: String,
    client_id: String,
    services: String,
    expires_at_unix_ms: u64,
}

#[derive(Debug, Serialize)]
struct DevicesOutput {
    devices: Vec<DeviceSummary>,
}

#[derive(Debug, Serialize)]
struct RevokeOutput {
    device_id: String,
    revoked: bool,
}

#[derive(Debug, Serialize)]
struct DeleteDeviceOutput {
    device_id: String,
    deleted: bool,
}

#[derive(Debug, Serialize)]
struct PurgeRevokedOutput {
    deleted: usize,
}

#[derive(Debug, Serialize)]
struct WorkerPoliciesOutput {
    mcp: WorkerPolicy,
    actions: WorkerPolicy,
}

#[derive(Debug, Deserialize)]
struct DispatchNodeUpdateInput {
    version: String,
    url: String,
    sha256: String,
    signature: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DispatchNodeUpdateOutput {
    client_id: String,
    update_id: String,
    version: String,
    target_workers: usize,
}

#[derive(Debug, Deserialize)]
struct LogsQuery {
    q: Option<String>,
    level: Option<String>,
    service: Option<String>,
    client_id: Option<String>,
    scope: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ActivityQuery {
    limit: Option<usize>,
}

pub fn load_admin_config() -> Result<Option<AdminConfig>, String> {
    let username = std::env::var(ADMIN_USERNAME_ENV).ok();
    let password = if let Ok(path) = std::env::var(ADMIN_PASSWORD_FILE_ENV) {
        Some(std::fs::read_to_string(&path).map_err(|error| {
            format!("failed to read {ADMIN_PASSWORD_FILE_ENV} path {path}: {error}")
        })?)
    } else {
        std::env::var(ADMIN_PASSWORD_ENV).ok()
    };

    if username.is_none() && password.is_none() {
        return Ok(None);
    }
    let username = username
        .ok_or_else(|| format!("{ADMIN_USERNAME_ENV} is required when admin login is enabled"))?;
    let password = password.ok_or_else(|| {
        format!(
            "{ADMIN_PASSWORD_FILE_ENV} or {ADMIN_PASSWORD_ENV} is required when admin login is enabled"
        )
    })?;
    let username = username.trim().to_string();
    let password = password.trim().to_string();
    if username.is_empty() || username.len() > MAX_USERNAME_BYTES {
        return Err(format!(
            "admin username must contain 1 to {MAX_USERNAME_BYTES} bytes"
        ));
    }
    if password.len() < MIN_PASSWORD_BYTES || password.len() > MAX_PASSWORD_BYTES {
        return Err(format!(
            "admin password must contain {MIN_PASSWORD_BYTES} to {MAX_PASSWORD_BYTES} bytes"
        ));
    }
    let session_seconds = std::env::var(ADMIN_SESSION_SECONDS_ENV)
        .ok()
        .map(|value| {
            value.parse::<u64>().map_err(|_| {
                format!("{ADMIN_SESSION_SECONDS_ENV} must be an integer number of seconds")
            })
        })
        .transpose()?
        .unwrap_or(DEFAULT_SESSION_SECONDS);
    if !(MIN_SESSION_SECONDS..=MAX_SESSION_SECONDS).contains(&session_seconds) {
        return Err(format!(
            "{ADMIN_SESSION_SECONDS_ENV} must be between {MIN_SESSION_SECONDS} and {MAX_SESSION_SECONDS}"
        ));
    }
    Ok(Some(AdminConfig {
        username,
        password,
        session_ttl: Duration::from_secs(session_seconds),
    }))
}

pub fn build_admin_app(
    devices: DeviceRegistry,
    policies: WorkerPolicyStore,
    observability: Observability,
    registry: Registry,
    public_origin: String,
    config: AdminConfig,
) -> Result<Router, String> {
    let username_digest: [u8; 32] = Sha256::digest(config.username.as_bytes()).into();
    let salt = SaltString::generate(&mut OsRng);
    let password_hash = Argon2::default()
        .hash_password(config.password.as_bytes(), &salt)
        .map_err(|error| format!("failed to prepare admin password verifier: {error}"))?
        .to_string();
    let state = AdminState {
        devices,
        policies,
        observability,
        registry,
        public_origin: public_origin.trim_end_matches('/').to_string(),
        username: config.username,
        username_digest,
        password_hash: Arc::new(password_hash),
        sessions: SessionStore {
            entries: Arc::new(Mutex::new(HashMap::new())),
            ttl: config.session_ttl,
        },
        login_limit: Arc::new(Semaphore::new(MAX_CONCURRENT_PASSWORD_CHECKS)),
    };
    Ok(Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/api/health", get(health))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/session", get(session_status))
        .route("/api/devices", get(list_devices))
        .route("/api/dashboard", get(dashboard))
        .route("/api/workers", get(list_workers))
        .route(
            "/api/clients/{client_id}/update",
            post(dispatch_node_update),
        )
        .route("/api/activity", get(list_activity))
        .route("/api/logs", get(list_logs))
        .route("/api/worker-policies", get(list_worker_policies))
        .route("/api/worker-policies/{service}", put(update_worker_policy))
        .route("/api/enrollments", post(create_enrollment))
        .route("/api/devices/revoked/purge", post(purge_revoked_devices))
        .route("/api/devices/{device_id}", delete(delete_revoked_device))
        .route("/api/devices/{device_id}/revoke", post(revoke_device))
        .with_state(state))
}

async fn index(State(state): State<AdminState>, headers: HeaderMap) -> Response<Body> {
    let body = if state.sessions.get(&headers).await.is_some() {
        ADMIN_HTML
    } else {
        ADMIN_LOGIN_HTML
    };
    html_response(body)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

async fn login(State(state): State<AdminState>, Json(input): Json<LoginInput>) -> Response<Body> {
    if input.username.len() > MAX_USERNAME_BYTES || input.password.len() > MAX_PASSWORD_BYTES {
        tokio::time::sleep(Duration::from_millis(400)).await;
        return json_error(StatusCode::UNAUTHORIZED, "帳號或密碼錯誤");
    }
    let Ok(_permit) = state.login_limit.acquire().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "登入服務暫時不可用");
    };
    let supplied_username: [u8; 32] = Sha256::digest(input.username.trim().as_bytes()).into();
    let username_valid = bool::from(supplied_username.ct_eq(&state.username_digest));
    let password_hash = state.password_hash.clone();
    let password = input.password;
    let password_valid = tokio::task::spawn_blocking(move || {
        PasswordHash::new(&password_hash)
            .ok()
            .is_some_and(|parsed| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok()
            })
    })
    .await
    .unwrap_or(false);
    if !(username_valid && password_valid) {
        tokio::time::sleep(Duration::from_millis(400)).await;
        return json_error(StatusCode::UNAUTHORIZED, "帳號或密碼錯誤");
    }

    let (token, _session) = state.sessions.create(&state.username).await;
    let mut response = Json(json!({ "ok": true, "username": state.username })).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&session_cookie(&token, state.sessions.ttl.as_secs()))
            .expect("session cookie contains only valid header characters"),
    );
    secure_json_response(response)
}

async fn logout(State(state): State<AdminState>, headers: HeaderMap) -> Response<Body> {
    let Some(session) = state.sessions.get(&headers).await else {
        return unauthorized_response();
    };
    if !csrf_authorized(&session, &headers) {
        return json_error(StatusCode::FORBIDDEN, "CSRF 驗證失敗");
    }
    state.sessions.remove(&headers).await;
    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_static(
            "__Host-coding_tools_admin_session=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Strict",
        ),
    );
    secure_json_response(response)
}

async fn session_status(State(state): State<AdminState>, headers: HeaderMap) -> Response<Body> {
    let Some(session) = state.sessions.get(&headers).await else {
        return unauthorized_response();
    };
    secure_json_response(
        Json(SessionOutput {
            authenticated: true,
            username: session.username,
            csrf_token: session.csrf_token,
        })
        .into_response(),
    )
}

async fn list_devices(State(state): State<AdminState>, headers: HeaderMap) -> Response<Body> {
    if state.sessions.get(&headers).await.is_none() {
        return unauthorized_response();
    }
    let devices = state.devices.clone();
    match tokio::task::spawn_blocking(move || devices.list_devices()).await {
        Ok(Ok(devices)) => secure_json_response(Json(DevicesOutput { devices }).into_response()),
        Ok(Err(error)) => device_error_response(error),
        Err(error) => json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("device registry task failed: {error}"),
        ),
    }
}

async fn dashboard(State(state): State<AdminState>, headers: HeaderMap) -> Response<Body> {
    if state.sessions.get(&headers).await.is_none() {
        return unauthorized_response();
    }
    secure_json_response(Json(state.observability.dashboard()).into_response())
}

async fn list_workers(State(state): State<AdminState>, headers: HeaderMap) -> Response<Body> {
    if state.sessions.get(&headers).await.is_none() {
        return unauthorized_response();
    }
    secure_json_response(Json(json!({ "workers": state.observability.workers() })).into_response())
}

async fn dispatch_node_update(
    State(state): State<AdminState>,
    Path(client_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<DispatchNodeUpdateInput>,
) -> Response<Body> {
    let Some(session) = state.sessions.get(&headers).await else {
        return unauthorized_response();
    };
    if !csrf_authorized(&session, &headers) {
        return json_error(StatusCode::FORBIDDEN, "CSRF validation failed");
    }
    let client_id = client_id.trim().to_string();
    if !valid_client_id(&client_id) {
        return json_error(StatusCode::BAD_REQUEST, "invalid client id");
    }
    let version = input.version.trim().to_string();
    if version.is_empty()
        || version.len() > 64
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
    {
        return json_error(StatusCode::BAD_REQUEST, "invalid update version");
    }
    let url = input.url.trim().to_string();
    if url.len() > 2048 || !url.starts_with("https://") {
        return json_error(StatusCode::BAD_REQUEST, "update URL must use https");
    }
    let sha256 = input.sha256.trim().to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return json_error(
            StatusCode::BAD_REQUEST,
            "sha256 must be 64 hexadecimal characters",
        );
    }
    let signature = input.signature.trim().to_string();
    let Ok(signature_bytes) = URL_SAFE_NO_PAD.decode(&signature) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "signature must be base64url without padding",
        );
    };
    if signature_bytes.len() != 64 {
        return json_error(
            StatusCode::BAD_REQUEST,
            "signature must contain a 64-byte Ed25519 signature",
        );
    }

    let update_id = uuid::Uuid::new_v4().to_string();
    let offer = NodeUpdateOffer {
        update_id: update_id.clone(),
        version: version.clone(),
        url,
        sha256,
        signature,
    };
    let target_workers = state.registry.dispatch_node_update(&client_id, offer).await;
    if target_workers == 0 {
        return json_error(StatusCode::CONFLICT, "client has no connected MCP workers");
    }
    state.observability.log(
        "info",
        "update",
        format!("dispatched Node Agent update {version} to {target_workers} worker(s)"),
        Some(&client_id),
        Some("mcp"),
        None,
        None,
    );
    secure_json_response(
        Json(DispatchNodeUpdateOutput {
            client_id,
            update_id,
            version,
            target_workers,
        })
        .into_response(),
    )
}

async fn list_activity(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Query(query): Query<ActivityQuery>,
) -> Response<Body> {
    if state.sessions.get(&headers).await.is_none() {
        return unauthorized_response();
    }
    let activities = state.observability.activities(query.limit.unwrap_or(50));
    secure_json_response(Json(json!({ "activities": activities })).into_response())
}

async fn list_logs(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Query(query): Query<LogsQuery>,
) -> Response<Body> {
    if state.sessions.get(&headers).await.is_none() {
        return unauthorized_response();
    }
    if query.q.as_ref().is_some_and(|value| value.len() > 256)
        || query
            .client_id
            .as_ref()
            .is_some_and(|value| value.len() > 128)
    {
        return json_error(StatusCode::BAD_REQUEST, "log query is too long");
    }
    if query.scope.as_deref().is_some_and(|scope| {
        !matches!(
            scope.trim().to_ascii_lowercase().as_str(),
            "all" | "system" | "client"
        )
    }) {
        return json_error(
            StatusCode::BAD_REQUEST,
            "log scope must be all, system, or client",
        );
    }
    if query
        .scope
        .as_deref()
        .is_some_and(|scope| scope.eq_ignore_ascii_case("client"))
        && query
            .client_id
            .as_deref()
            .is_none_or(|client_id| client_id.trim().is_empty())
    {
        return json_error(
            StatusCode::BAD_REQUEST,
            "client_id is required for client log scope",
        );
    }
    let logs = state.observability.logs(LogFilter {
        query: query.q.as_deref(),
        level: query.level.as_deref(),
        service: query.service.as_deref(),
        client_id: query.client_id.as_deref(),
        scope: query.scope.as_deref(),
        limit: query.limit.unwrap_or(100),
    });
    secure_json_response(Json(json!({ "logs": logs })).into_response())
}

async fn list_worker_policies(
    State(state): State<AdminState>,
    headers: HeaderMap,
) -> Response<Body> {
    if state.sessions.get(&headers).await.is_none() {
        return unauthorized_response();
    }
    secure_json_response(
        Json(WorkerPoliciesOutput {
            mcp: state.policies.current(TunnelService::Mcp),
            actions: state.policies.current(TunnelService::Actions),
        })
        .into_response(),
    )
}

async fn update_worker_policy(
    State(state): State<AdminState>,
    Path(service): Path<String>,
    headers: HeaderMap,
    Json(policy): Json<WorkerPolicy>,
) -> Response<Body> {
    let Some(session) = state.sessions.get(&headers).await else {
        return unauthorized_response();
    };
    if !csrf_authorized(&session, &headers) {
        return json_error(StatusCode::FORBIDDEN, "CSRF 驗證失敗");
    }
    let Some(service) = TunnelService::parse(&service) else {
        return json_error(StatusCode::BAD_REQUEST, "service must be mcp or actions");
    };
    let policies = state.policies.clone();
    match tokio::task::spawn_blocking(move || policies.update(service, policy)).await {
        Ok(Ok(saved)) => secure_json_response(Json(saved).into_response()),
        Ok(Err(WorkerPolicyError::Invalid(message))) => {
            json_error(StatusCode::BAD_REQUEST, message)
        }
        Ok(Err(WorkerPolicyError::Storage(message))) => {
            json_error(StatusCode::SERVICE_UNAVAILABLE, message)
        }
        Err(error) => json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("worker policy task failed: {error}"),
        ),
    }
}

async fn create_enrollment(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Json(input): Json<CreateEnrollmentInput>,
) -> Response<Body> {
    let Some(session) = state.sessions.get(&headers).await else {
        return unauthorized_response();
    };
    if !csrf_authorized(&session, &headers) {
        return json_error(StatusCode::FORBIDDEN, "CSRF 驗證失敗");
    }
    let Some(services) = AllowedServices::parse(&input.service) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "service must be mcp, actions, or both",
        );
    };
    let client_id = input.client_id.trim().to_string();
    let ttl_seconds = input.ttl_seconds;
    let devices = state.devices.clone();
    let grant = match tokio::task::spawn_blocking(move || {
        devices.create_enrollment(&client_id, services, ttl_seconds)
    })
    .await
    {
        Ok(Ok(grant)) => grant,
        Ok(Err(error)) => return device_error_response(error),
        Err(error) => {
            return json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("device registry task failed: {error}"),
            )
        }
    };
    secure_json_response(
        Json(CreateEnrollmentOutput {
            enrollment_url: format!("{}/_tunnel/enroll/{}", state.public_origin, grant.code),
            client_id: grant.client_id,
            services: grant.services.as_str().to_string(),
            expires_at_unix_ms: grant.expires_at_unix_ms,
        })
        .into_response(),
    )
}

async fn revoke_device(
    State(state): State<AdminState>,
    Path(device_id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(session) = state.sessions.get(&headers).await else {
        return unauthorized_response();
    };
    if !csrf_authorized(&session, &headers) {
        return json_error(StatusCode::FORBIDDEN, "CSRF 驗證失敗");
    }
    let devices = state.devices.clone();
    let requested_id = device_id.clone();
    match tokio::task::spawn_blocking(move || devices.revoke_device(&device_id)).await {
        Ok(Ok(true)) => secure_json_response(
            Json(RevokeOutput {
                device_id: requested_id,
                revoked: true,
            })
            .into_response(),
        ),
        Ok(Ok(false)) => json_error(StatusCode::NOT_FOUND, "active device not found"),
        Ok(Err(error)) => device_error_response(error),
        Err(error) => json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("device registry task failed: {error}"),
        ),
    }
}

async fn delete_revoked_device(
    State(state): State<AdminState>,
    Path(device_id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(session) = state.sessions.get(&headers).await else {
        return unauthorized_response();
    };
    if !csrf_authorized(&session, &headers) {
        return json_error(StatusCode::FORBIDDEN, "CSRF 驗證失敗");
    }
    let devices = state.devices.clone();
    let requested_id = device_id.clone();
    match tokio::task::spawn_blocking(move || devices.delete_revoked_device(&device_id)).await {
        Ok(Ok(())) => {
            state.observability.log(
                "info",
                "admin",
                format!("permanently deleted revoked device {requested_id}"),
                None,
                None,
                None,
                None,
            );
            secure_json_response(
                Json(DeleteDeviceOutput {
                    device_id: requested_id,
                    deleted: true,
                })
                .into_response(),
            )
        }
        Ok(Err(error)) => device_error_response(error),
        Err(error) => json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("device registry task failed: {error}"),
        ),
    }
}

async fn purge_revoked_devices(
    State(state): State<AdminState>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(session) = state.sessions.get(&headers).await else {
        return unauthorized_response();
    };
    if !csrf_authorized(&session, &headers) {
        return json_error(StatusCode::FORBIDDEN, "CSRF 驗證失敗");
    }
    let devices = state.devices.clone();
    match tokio::task::spawn_blocking(move || devices.purge_revoked_devices()).await {
        Ok(Ok(deleted)) => {
            state.observability.log(
                "info",
                "admin",
                format!("purged {deleted} revoked device(s)"),
                None,
                None,
                None,
                None,
            );
            secure_json_response(Json(PurgeRevokedOutput { deleted }).into_response())
        }
        Ok(Err(error)) => device_error_response(error),
        Err(error) => json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("device registry task failed: {error}"),
        ),
    }
}

impl SessionStore {
    async fn create(&self, username: &str) -> (String, AdminSession) {
        let token = random_token();
        let session = AdminSession {
            username: username.to_string(),
            csrf_token: random_token(),
            expires_at: Instant::now() + self.ttl,
        };
        self.entries
            .lock()
            .await
            .insert(token_digest(&token), session.clone());
        (token, session)
    }

    async fn get(&self, headers: &HeaderMap) -> Option<AdminSession> {
        let token = session_token(headers)?;
        let digest = token_digest(token);
        let now = Instant::now();
        let mut entries = self.entries.lock().await;
        entries.retain(|_, session| session.expires_at > now);
        entries.get(&digest).cloned()
    }

    async fn remove(&self, headers: &HeaderMap) {
        let Some(token) = session_token(headers) else {
            return;
        };
        self.entries.lock().await.remove(&token_digest(token));
    }
}

fn session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let (name, value) = cookie.trim().split_once('=')?;
                (name == ADMIN_SESSION_COOKIE && !value.is_empty()).then_some(value)
            })
        })
}

fn csrf_authorized(session: &AdminSession, headers: &HeaderMap) -> bool {
    headers
        .get(&CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let supplied: [u8; 32] = Sha256::digest(value.as_bytes()).into();
            let expected: [u8; 32] = Sha256::digest(session.csrf_token.as_bytes()).into();
            bool::from(supplied.ct_eq(&expected))
        })
}

fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn token_digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn session_cookie(token: &str, max_age_seconds: u64) -> String {
    format!(
        "{ADMIN_SESSION_COOKIE}={token}; Path=/; Max-Age={max_age_seconds}; HttpOnly; Secure; SameSite=Strict"
    )
}

fn html_response(body: &'static str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(REFERRER_POLICY, "no-referrer")
        .header("x-frame-options", "DENY")
        .header(
            CONTENT_SECURITY_POLICY,
            "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        )
        .body(Body::from(body))
        .expect("admin HTML response headers are valid")
}

fn unauthorized_response() -> Response<Body> {
    json_error(StatusCode::UNAUTHORIZED, "請先登入管理介面")
}

fn device_error_response(error: DeviceAuthError) -> Response<Body> {
    let status = match error {
        DeviceAuthError::InvalidClientId
        | DeviceAuthError::InvalidDeviceName
        | DeviceAuthError::InvalidPublicKey => StatusCode::BAD_REQUEST,
        DeviceAuthError::UnknownDevice => StatusCode::NOT_FOUND,
        DeviceAuthError::Storage(_) => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::CONFLICT,
    };
    json_error(status, error.public_message())
}

fn secure_json_response(mut response: Response<Body>) -> Response<Body> {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

fn json_error(status: StatusCode, message: impl Into<String>) -> Response<Body> {
    secure_json_response((status, Json(json!({ "error": message.into() }))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use ed25519_dalek::SigningKey;
    use tempfile::TempDir;
    use tower::ServiceExt;

    const TEST_USERNAME: &str = "administrator";
    const TEST_PASSWORD: &str = "correct horse battery staple";

    fn test_app() -> (TempDir, Router) {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("devices.db");
        let devices = DeviceRegistry::open(&database_path).expect("device registry");
        let policies =
            WorkerPolicyStore::from_writer(devices.database_writer()).expect("worker policy store");
        let app = build_admin_app(
            devices,
            policies,
            Observability::new(),
            Registry::default(),
            "https://tunnel.example.com/".into(),
            AdminConfig {
                username: TEST_USERNAME.into(),
                password: TEST_PASSWORD.into(),
                session_ttl: Duration::from_secs(600),
            },
        )
        .expect("admin app");
        (directory, app)
    }

    fn test_app_with_devices() -> (TempDir, DeviceRegistry, Router) {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("devices.db");
        let devices = DeviceRegistry::open(&database_path).expect("device registry");
        let policies =
            WorkerPolicyStore::from_writer(devices.database_writer()).expect("worker policy store");
        let app = build_admin_app(
            devices.clone(),
            policies,
            Observability::new(),
            Registry::default(),
            "https://tunnel.example.com/".into(),
            AdminConfig {
                username: TEST_USERNAME.into(),
                password: TEST_PASSWORD.into(),
                session_ttl: Duration::from_secs(600),
            },
        )
        .expect("admin app");
        (directory, devices, app)
    }

    async fn enroll_test_device(
        devices: &DeviceRegistry,
        device_id: &str,
        client_id: &str,
        key_seed: u8,
    ) {
        let grant = devices
            .create_enrollment(client_id, AllowedServices::Both, Some(60))
            .expect("create enrollment");
        let signing_key = SigningKey::from_bytes(&[key_seed; 32]);
        devices
            .enroll(
                grant.code,
                coding_tools_tunnel_protocol::EnrollmentRequest {
                    device_id: device_id.into(),
                    client_id: client_id.into(),
                    device_name: format!("{client_id} admin test"),
                    public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes()),
                },
            )
            .await
            .expect("enroll device");
    }

    async fn authenticated(app: &Router) -> (String, String) {
        let login = app
            .clone()
            .oneshot(
                Request::post("/api/login")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        r#"{{"username":"{TEST_USERNAME}","password":"{TEST_PASSWORD}"}}"#
                    )))
                    .expect("login request"),
            )
            .await
            .expect("login response");
        assert_eq!(login.status(), StatusCode::OK);
        let cookie = login
            .headers()
            .get(SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("set-cookie")
            .split(';')
            .next()
            .expect("cookie pair")
            .to_string();
        assert!(login
            .headers()
            .get(SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("HttpOnly")
                && value.contains("Secure")
                && value.contains("SameSite=Strict")));

        let session = app
            .clone()
            .oneshot(
                Request::get("/api/session")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("session request"),
            )
            .await
            .expect("session response");
        assert_eq!(session.status(), StatusCode::OK);
        let body = to_bytes(session.into_body(), 64 * 1024)
            .await
            .expect("session body");
        let value: SessionOutput = serde_json::from_slice(&body).expect("session json");
        (cookie, value.csrf_token)
    }

    #[tokio::test]
    async fn unauthenticated_browser_receives_only_the_login_page() {
        let (_directory, app) = test_app();
        let response = app
            .oneshot(Request::get("/").body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let csp = response
            .headers()
            .get(CONTENT_SECURITY_POLICY)
            .and_then(|value| value.to_str().ok())
            .expect("CSP");
        assert!(csp.contains("default-src 'none'"));
        assert_eq!(
            response
                .headers()
                .get("x-frame-options")
                .and_then(|value| value.to_str().ok()),
            Some("DENY")
        );
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        let html = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(html.contains("管理帳號"));
        assert!(!html.contains("已註冊裝置"));
        assert!(!html.contains("Admin Key"));
    }

    #[tokio::test]
    async fn correct_login_unlocks_the_dashboard() {
        let (_directory, app) = test_app();
        let (cookie, _csrf) = authenticated(&app).await;
        let response = app
            .oneshot(
                Request::get("/")
                    .header(COOKIE, cookie)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        let html = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(html.contains("已註冊裝置"));
        assert!(!html.contains("管理帳號"));
    }

    #[tokio::test]
    async fn dashboard_exposes_fpm_worker_policy_controls() {
        let (_directory, app) = test_app();
        let (cookie, _csrf) = authenticated(&app).await;
        let response = app
            .oneshot(
                Request::get("/")
                    .header(COOKIE, cookie)
                    .body(Body::empty())
                    .expect("dashboard request"),
            )
            .await
            .expect("dashboard response");
        let body = to_bytes(response.into_body(), 128 * 1024)
            .await
            .expect("dashboard body");
        let html = String::from_utf8(body.to_vec()).expect("dashboard utf8");
        for marker in [
            "worker-policy-form",
            "mcp-policy-tab",
            "actions-policy-tab",
            "role=\"tabpanel\"",
            "mcp-start-workers",
            "mcp-min-idle-workers",
            "mcp-max-idle-workers",
            "mcp-max-workers",
            "mcp-max-requests-per-worker",
            "mcp-max-pending-requests",
            "mcp-worker-acquire-timeout-ms",
            "mcp-max-connecting-workers",
            "mcp-connecting-capacity-grace-ms",
            "mcp-scale-down-step",
            "mcp-burst-warm-workers",
            "mcp-burst-warm-seconds",
            "actions-max-workers",
            "actions-max-pending-requests",
            "actions-worker-acquire-timeout-ms",
            "actions-scale-down-step",
            "actions-burst-warm-seconds",
            "/api/worker-policies",
            "primary-tabs",
            "clients-panel",
            "client-log-search",
            "client-update-panel",
            "client-update-form",
            "client-agent-version",
            "client-update-state",
            "/api/clients/${encodeURIComponent(client.clientId)}/update",
            "system-log-search",
            "purge-revoked",
            "/api/devices/revoked/purge",
            "health-status",
            "queue-balance",
            "queue-wait",
            "capacity-errors",
            "worker-count",
            "/api/dashboard",
            "/api/workers",
            "/api/activity",
            "/api/logs",
        ] {
            assert!(
                html.contains(marker),
                "missing worker policy marker: {marker}"
            );
        }
    }

    #[tokio::test]
    async fn observability_apis_require_login_and_return_dashboard_data() {
        let (_directory, app) = test_app();
        let unauthenticated = app
            .clone()
            .oneshot(
                Request::get("/api/dashboard")
                    .body(Body::empty())
                    .expect("dashboard request"),
            )
            .await
            .expect("dashboard response");
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let (cookie, _csrf) = authenticated(&app).await;
        for path in [
            "/api/dashboard",
            "/api/workers",
            "/api/activity?limit=10",
            "/api/logs?q=server&level=info&scope=system&limit=10",
            "/api/logs?client_id=pc-a&scope=client&limit=10",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::get(path)
                        .header(COOKIE, &cookie)
                        .body(Body::empty())
                        .expect("observability request"),
                )
                .await
                .expect("observability response");
            assert_eq!(response.status(), StatusCode::OK, "path: {path}");
            assert_eq!(
                response
                    .headers()
                    .get(CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok()),
                Some("no-store")
            );
        }

        for path in ["/api/logs?scope=invalid", "/api/logs?scope=client"] {
            let response = app
                .clone()
                .oneshot(
                    Request::get(path)
                        .header(COOKIE, &cookie)
                        .body(Body::empty())
                        .expect("invalid log scope request"),
                )
                .await
                .expect("invalid log scope response");
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "path: {path}");
        }
    }

    #[tokio::test]
    async fn creates_an_enrollment_link_and_lists_devices_with_session_and_csrf() {
        let (_directory, app) = test_app();
        let (cookie, csrf) = authenticated(&app).await;
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/enrollments")
                    .header(COOKIE, &cookie)
                    .header(&CSRF_HEADER, &csrf)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"client_id":"pc-a","service":"both","ttl_seconds":600}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        let value: CreateEnrollmentOutput = serde_json::from_slice(&body).expect("json");
        assert!(value
            .enrollment_url
            .starts_with("https://tunnel.example.com/_tunnel/enroll/"));

        let response = app
            .oneshot(
                Request::get("/api/devices")
                    .header(COOKIE, cookie)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn manually_cleans_up_only_revoked_devices() {
        let (_directory, devices, app) = test_app_with_devices();
        enroll_test_device(&devices, "admin-active", "admin-active-client", 31).await;
        enroll_test_device(
            &devices,
            "admin-revoked-one",
            "admin-revoked-client-one",
            32,
        )
        .await;
        enroll_test_device(
            &devices,
            "admin-revoked-two",
            "admin-revoked-client-two",
            33,
        )
        .await;
        assert!(devices
            .revoke_device("admin-revoked-one")
            .expect("revoke first device"));
        assert!(devices
            .revoke_device("admin-revoked-two")
            .expect("revoke second device"));

        let (cookie, csrf) = authenticated(&app).await;
        let active_delete = app
            .clone()
            .oneshot(
                Request::delete("/api/devices/admin-active")
                    .header(COOKIE, &cookie)
                    .header(&CSRF_HEADER, &csrf)
                    .body(Body::empty())
                    .expect("active delete request"),
            )
            .await
            .expect("active delete response");
        assert_eq!(active_delete.status(), StatusCode::CONFLICT);

        let revoked_delete = app
            .clone()
            .oneshot(
                Request::delete("/api/devices/admin-revoked-one")
                    .header(COOKIE, &cookie)
                    .header(&CSRF_HEADER, &csrf)
                    .body(Body::empty())
                    .expect("revoked delete request"),
            )
            .await
            .expect("revoked delete response");
        assert_eq!(revoked_delete.status(), StatusCode::OK);
        let body = to_bytes(revoked_delete.into_body(), 64 * 1024)
            .await
            .expect("delete body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("delete json");
        assert_eq!(value["deleted"], true);

        let purge = app
            .clone()
            .oneshot(
                Request::post("/api/devices/revoked/purge")
                    .header(COOKIE, &cookie)
                    .header(&CSRF_HEADER, &csrf)
                    .body(Body::empty())
                    .expect("purge request"),
            )
            .await
            .expect("purge response");
        assert_eq!(purge.status(), StatusCode::OK);
        let body = to_bytes(purge.into_body(), 64 * 1024)
            .await
            .expect("purge body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("purge json");
        assert_eq!(value["deleted"], 1);

        let remaining = devices.list_devices().expect("remaining devices");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].device_id, "admin-active");
    }

    #[tokio::test]
    async fn rejects_bad_credentials_and_missing_csrf() {
        let (_directory, app) = test_app();
        let bad_login = app
            .clone()
            .oneshot(
                Request::post("/api/login")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"administrator","password":"wrong password"}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(bad_login.status(), StatusCode::UNAUTHORIZED);

        let (cookie, _csrf) = authenticated(&app).await;
        let no_csrf = app
            .oneshot(
                Request::post("/api/enrollments")
                    .header(COOKIE, cookie)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"client_id":"pc-a","service":"both","ttl_seconds":600}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(no_csrf.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn node_update_dispatch_validates_csrf_https_hash_and_signature() {
        let (_directory, app) = test_app();
        let (cookie, csrf) = authenticated(&app).await;
        let signature = URL_SAFE_NO_PAD.encode([0_u8; 64]);
        let valid = json!({
            "version": "0.29.34",
            "url": "https://updates.example.test/ctnode.zip",
            "sha256": "a".repeat(64),
            "signature": signature,
        })
        .to_string();

        let no_csrf = app
            .clone()
            .oneshot(
                Request::post("/api/clients/pc-a/update")
                    .header(COOKIE, &cookie)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(valid.clone()))
                    .expect("missing csrf update request"),
            )
            .await
            .expect("missing csrf update response");
        assert_eq!(no_csrf.status(), StatusCode::FORBIDDEN);

        let insecure = json!({
            "version": "0.29.34",
            "url": "http://updates.example.test/ctnode.zip",
            "sha256": "a".repeat(64),
            "signature": URL_SAFE_NO_PAD.encode([0_u8; 64]),
        })
        .to_string();
        let insecure = app
            .clone()
            .oneshot(
                Request::post("/api/clients/pc-a/update")
                    .header(COOKIE, &cookie)
                    .header(&CSRF_HEADER, &csrf)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(insecure))
                    .expect("insecure update request"),
            )
            .await
            .expect("insecure update response");
        assert_eq!(insecure.status(), StatusCode::BAD_REQUEST);

        let malformed_signature = json!({
            "version": "0.29.34",
            "url": "https://updates.example.test/ctnode.zip",
            "sha256": "a".repeat(64),
            "signature": "short",
        })
        .to_string();
        let malformed_signature = app
            .clone()
            .oneshot(
                Request::post("/api/clients/pc-a/update")
                    .header(COOKIE, &cookie)
                    .header(&CSRF_HEADER, &csrf)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(malformed_signature))
                    .expect("bad signature request"),
            )
            .await
            .expect("bad signature response");
        assert_eq!(malformed_signature.status(), StatusCode::BAD_REQUEST);

        let accepted_metadata = app
            .oneshot(
                Request::post("/api/clients/pc-a/update")
                    .header(COOKIE, &cookie)
                    .header(&CSRF_HEADER, &csrf)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(valid))
                    .expect("valid update request"),
            )
            .await
            .expect("valid update response");
        assert_eq!(accepted_metadata.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn manages_mcp_and_actions_worker_policies_with_csrf() {
        let (_directory, app) = test_app();
        let (cookie, csrf) = authenticated(&app).await;

        let response = app
            .clone()
            .oneshot(
                Request::get("/api/worker-policies")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("policy request"),
            )
            .await
            .expect("policy response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("policy body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("policy json");
        assert_eq!(value["mcp"]["max_workers"], 16);
        assert_eq!(value["actions"]["min_idle_workers"], 2);
        assert_eq!(value["mcp"]["max_pending_requests"], 32);
        assert_eq!(value["mcp"]["worker_acquire_timeout_ms"], 10_000);
        assert_eq!(value["mcp"]["scale_down_step"], 4);

        let update = r#"{
            "start_workers":6,
            "min_idle_workers":3,
            "max_idle_workers":8,
            "max_workers":24,
            "max_requests_per_worker":900,
            "max_lifetime_seconds":1800,
            "scale_down_delay_seconds":30,
            "recycle_jitter_percent":15,
            "revision":1
        }"#;
        let response = app
            .clone()
            .oneshot(
                Request::put("/api/worker-policies/mcp")
                    .header(COOKIE, &cookie)
                    .header(&CSRF_HEADER, &csrf)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(update))
                    .expect("policy update request"),
            )
            .await
            .expect("policy update response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("updated policy body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("updated policy json");
        assert_eq!(value["max_workers"], 24);
        assert_eq!(value["revision"], 2);
        assert_eq!(value["max_pending_requests"], 32);
        assert_eq!(value["worker_acquire_timeout_ms"], 10_000);
        assert_eq!(value["burst_warm_seconds"], 120);

        let no_csrf = app
            .oneshot(
                Request::put("/api/worker-policies/actions")
                    .header(COOKIE, cookie)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(update))
                    .expect("missing csrf request"),
            )
            .await
            .expect("missing csrf response");
        assert_eq!(no_csrf.status(), StatusCode::FORBIDDEN);
    }
}
