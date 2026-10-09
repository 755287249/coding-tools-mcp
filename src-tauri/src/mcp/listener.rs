mod lifecycle;
mod routes;

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Form, Query, State};
use axum::http::{
    header::{ALLOW, CACHE_CONTROL, CONTENT_TYPE, ORIGIN, WWW_AUTHENTICATE},
    HeaderMap, HeaderValue, StatusCode,
};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::stream::unfold;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};

use crate::auth::{
    authorization_server_metadata, authorize_get, authorize_post, external_base_url,
    protected_resource_metadata, protected_resource_metadata_url, token_exchange,
    verify_bearer_header, verify_oauth_bearer_header, AuthorizeForm, AuthorizeParams, OAuthRuntime,
    TokenForm,
};
use crate::mcp::activity;
use crate::mcp::server::{
    handle_request, handle_request_async, is_supported_protocol_version, permission_input_required,
    SharedState, LATEST_LEGACY_PROTOCOL_VERSION, LATEST_PROTOCOL_VERSION, MODERN_PROTOCOL_VERSION,
    SUPPORTED_PROTOCOL_VERSIONS,
};
use crate::mcp::session_activity::{self, SessionActivityGuard};
use crate::mcp::telemetry::{
    begin_tool_request, format_request_log_value, record_tool_usage, ToolRequestTiming,
    ToolUsageInput,
};
use crate::tunnel::append_profile_log_buffered;
use crate::workspace::{parse_bind_address, AuthConfig};

pub use lifecycle::{spawn_listener, ShutdownSender};

#[cfg(test)]
use lifecycle::bind_listener;
#[cfg(test)]
use routes::{
    authorization_metadata_path, build_router, configured_route_prefix, prefixed_route,
    protected_resource_metadata_path,
};

const MCP_STREAM_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const MCP_STREAM_CHANNEL_CAPACITY: usize = 2;

pub(crate) fn behavioral_parity_fixture() -> Value {
    json!({
        "latest_protocol_version": LATEST_PROTOCOL_VERSION,
        "supported_protocol_versions": SUPPORTED_PROTOCOL_VERSIONS,
        "stream_heartbeat_interval_ms": MCP_STREAM_HEARTBEAT_INTERVAL.as_millis(),
        "stream_channel_capacity": MCP_STREAM_CHANNEL_CAPACITY
    })
}

#[derive(Clone)]
struct ListenerState {
    mcp: SharedState,
    auth: AuthConfig,
    workspace_id: String,
    bind_address: String,
    bind_port: u16,
    configured_public_url: String,
    bearer_token: Option<String>,
    oauth: Option<Arc<OAuthRuntime>>,
    oauth_client_secret: Option<String>,
    transport_mode: String,
    redact_telemetry: bool,
}

struct RpcExecutionContext {
    state: ListenerState,
    method: String,
    request_id: Value,
    tool_name: String,
    argument_value: Value,
    started_ts_ms: u128,
    started_at: Instant,
    session_activity: Option<SessionActivityGuard>,
    request_timing: Option<ToolRequestTiming>,
    request_json_bytes: usize,
    protocol_version: String,
    host_session_key: Option<String>,
}

fn unix_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn json_byte_len(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

fn json_rpc_response_message(body: &Value) -> bool {
    body.get("method").is_none()
        && body.get("id").is_some()
        && (body.get("result").is_some() || body.get("error").is_some())
}

fn modern_request_method_supported(method: &str) -> bool {
    matches!(
        method,
        "server/discover"
            | "tools/list"
            | "tools/call"
            | "tasks/get"
            | "tasks/update"
            | "tasks/cancel"
            | "subscriptions/listen"
    )
}

async fn mcp_info() -> Response {
    ([(CACHE_CONTROL, "no-store")], Json(mcp_discovery_payload())).into_response()
}

async fn mcp_get(State(state): State<ListenerState>, headers: HeaderMap) -> Response {
    if state.transport_mode == "legacy-json" {
        return mcp_info().await;
    }
    if let Some(response) = validate_standard_connection(&state, &headers, false) {
        record_transport_response(&state.workspace_id, &headers, &response, None);
        return response;
    }
    method_not_allowed("POST")
}

async fn mcp_delete(State(state): State<ListenerState>, headers: HeaderMap) -> Response {
    if state.transport_mode == "legacy-json" {
        return method_not_allowed("GET, POST");
    }
    if let Some(response) = validate_standard_connection(&state, &headers, false) {
        record_transport_response(&state.workspace_id, &headers, &response, None);
        return response;
    }
    method_not_allowed("POST")
}

fn mcp_discovery_payload() -> Value {
    json!({
        "name": "coding-tools-mcp",
        "version": env!("CARGO_PKG_VERSION"),
        "protocolVersion": LATEST_PROTOCOL_VERSION,
        "supportedProtocolVersions": SUPPORTED_PROTOCOL_VERSIONS,
        "transport": "streamable-http"
    })
}

/// Metadata key the tool router uses as the per-conversation identity.
const HOST_SESSION_META_KEY: &str = "openai/session";

/// ChatGPT sends `_meta["openai/session"]` with every tool call, but standard
/// MCP clients (Claude, Cursor, VS Code, custom agents…) do not. Without an
/// identity the router refuses to bind a folder, so derive a stable
/// per-client key and inject it when the client did not provide one.
fn ensure_host_session_identity(headers: &HeaderMap, body: &mut Value) {
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    if !(method.starts_with("tools/")
        || method.starts_with("prompts/")
        || method.starts_with("resources/"))
    {
        return;
    }
    let Some(object) = body.as_object_mut() else {
        return;
    };
    let params = object
        .entry("params")
        .or_insert_with(|| Value::Object(Default::default()));
    let Some(params) = params.as_object_mut() else {
        return;
    };
    let meta = params
        .entry("_meta")
        .or_insert_with(|| Value::Object(Default::default()));
    let Some(meta) = meta.as_object_mut() else {
        return;
    };
    let present = meta
        .get(HOST_SESSION_META_KEY)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    if !present {
        meta.insert(
            HOST_SESSION_META_KEY.into(),
            Value::String(derive_client_session_key(headers)),
        );
    }
}

/// Stable identity for clients without a platform conversation id:
/// 1. an explicit `x-openai-session` header,
/// 2. the transport `Mcp-Session-Id` header,
/// 3. a hash of the Authorization header (one identity per OAuth grant / key),
/// 4. a hash of the User-Agent for unauthenticated local clients.
fn derive_client_session_key(headers: &HeaderMap) -> String {
    use sha2::{Digest, Sha256};
    fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
    if let Some(value) = header(headers, "x-openai-session") {
        return value.chars().take(200).collect();
    }
    if let Some(value) = header(headers, "mcp-session-id") {
        return format!(
            "mcp-session:{}",
            value.chars().take(200).collect::<String>()
        );
    }
    let (kind, seed) = match header(headers, "authorization") {
        Some(value) => ("client", value),
        None => (
            "anonymous",
            header(headers, "user-agent").unwrap_or("unknown"),
        ),
    };
    let digest = Sha256::digest(seed.as_bytes());
    let short: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("mcp-{kind}:{short}")
}

fn resolve_oauth_base(state: &ListenerState, headers: &HeaderMap) -> String {
    external_base_url(headers, state.bind_port, &state.configured_public_url)
}

async fn mcp_post(
    State(state): State<ListenerState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let standard_transport = state.transport_mode != "legacy-json";
    let diagnostic_workspace_id = state.workspace_id.clone();
    let diagnostic_method = body.get("method").and_then(Value::as_str);
    if standard_transport {
        if let Some(response) = validate_standard_connection(&state, &headers, true) {
            record_transport_response(
                &diagnostic_workspace_id,
                &headers,
                &response,
                diagnostic_method,
            );
            return response;
        }
        if let Some(response) = validate_json_rpc_message(&body) {
            record_transport_response(
                &diagnostic_workspace_id,
                &headers,
                &response,
                diagnostic_method,
            );
            return response;
        }
        let modern_header = headers
            .get("mcp-protocol-version")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.trim() == MODERN_PROTOCOL_VERSION);
        if modern_header && json_rpc_response_message(&body) {
            let response = transport_error_with_id(
                StatusCode::BAD_REQUEST,
                -32600,
                body.get("id").cloned().unwrap_or(Value::Null),
                "Streamable HTTP accepts only JSON-RPC requests or notifications from clients",
            );
            record_transport_response(
                &diagnostic_workspace_id,
                &headers,
                &response,
                diagnostic_method,
            );
            return response;
        }
        if let Some(response) = validate_modern_request(&headers, &body) {
            record_transport_response(
                &diagnostic_workspace_id,
                &headers,
                &response,
                diagnostic_method,
            );
            return response;
        }
        let runtime = state.mcp.runtime_config();
        if let Some(response) = validate_modern_tool_headers(&headers, &body, &runtime.tool_profile)
        {
            record_transport_response(
                &diagnostic_workspace_id,
                &headers,
                &response,
                diagnostic_method,
            );
            return response;
        }
    } else if let Some(response) = require_mcp_auth(&state, &headers) {
        return response;
    }

    let mut body = body;
    ensure_host_session_identity(&headers, &mut body);

    let method = body
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let request_id = body.get("id").cloned().unwrap_or(Value::Null);
    let tool_name = body
        .get("params")
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let argument_value = body
        .get("params")
        .and_then(|params| params.get("arguments"))
        .cloned()
        .unwrap_or(Value::Null);
    let host_session_key = body
        .get("params")
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get("openai/session"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let host_session_key_owned = host_session_key.map(str::to_string);
    let arguments = format_request_log_value(&argument_value);
    let is_notification = body.get("method").is_some() && body.get("id").is_none();
    let is_response = json_rpc_response_message(&body);
    let started_ts_ms = unix_timestamp_ms();
    let started_at = Instant::now();
    let session_activity = session_activity::begin(
        &state.workspace_id,
        host_session_key,
        &tool_name,
        &argument_value,
        started_ts_ms,
    );
    let request_timing =
        (!tool_name.is_empty()).then(|| begin_tool_request(&state.workspace_id, started_ts_ms));
    let orchestration_gap_log = request_timing
        .as_ref()
        .and_then(|timing| timing.orchestration_gap_ms)
        .map(|gap| gap.to_string())
        .unwrap_or_else(|| "null".to_string());
    let burst_id_log = request_timing
        .as_ref()
        .map(|timing| timing.activity_burst_id.to_string())
        .unwrap_or_else(|| "null".to_string());
    let burst_sequence_log = request_timing
        .as_ref()
        .map(|timing| timing.activity_burst_sequence.to_string())
        .unwrap_or_else(|| "null".to_string());
    let concurrent_request = request_timing
        .as_ref()
        .is_some_and(|timing| timing.concurrent_request);
    let request_json_bytes = json_byte_len(&body);
    let protocol_version = headers
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok())
        .unwrap_or(LATEST_LEGACY_PROTOCOL_VERSION)
        .to_string();
    if standard_transport
        && protocol_version == MODERN_PROTOCOL_VERSION
        && !is_notification
        && !is_response
        && !modern_request_method_supported(&method)
    {
        return transport_error_with_id(
            StatusCode::NOT_FOUND,
            -32601,
            request_id,
            &format!("Method not found: {method}"),
        );
    }
    if standard_transport
        && protocol_version == MODERN_PROTOCOL_VERSION
        && method == "subscriptions/listen"
    {
        if request_id.is_null() {
            let response = transport_error_with_id(
                StatusCode::BAD_REQUEST,
                -32600,
                request_id,
                "subscriptions/listen requires a JSON-RPC request id",
            );
            record_transport_response(&diagnostic_workspace_id, &headers, &response, Some(&method));
            return response;
        }
        if modern_subscription_notifications(&body).is_none() {
            return json_no_store(json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {
                    "code": -32602,
                    "message": "Invalid params: notifications is required and must be a valid subscription filter"
                }
            }));
        }
        // This server currently advertises tools.listChanged=false and no prompt/resource
        // notification capabilities, so the honored subset is intentionally empty.
        return subscription_sse_response(request_id, json!({}), MCP_STREAM_HEARTBEAT_INTERVAL);
    }
    append_profile_log_buffered(
        &state.workspace_id,
        "mcp-requests.log",
        &format!(
            "[rpc] received ts={} id={} method={} tool={} orchestration_gap_ms={} burst_id={} burst_sequence={} concurrent_request={} arguments={}",
            started_ts_ms,
            request_id,
            method,
            tool_name,
            orchestration_gap_log,
            burst_id_log,
            burst_sequence_log,
            concurrent_request,
            arguments
        ),
    );

    if standard_transport && is_response {
        append_profile_log_buffered(
            &state.workspace_id,
            "mcp-requests.log",
            &format!(
                "[rpc] accepted_response id={} duration_ms={}",
                request_id,
                started_at.elapsed().as_millis()
            ),
        );
        return StatusCode::ACCEPTED.into_response();
    }

    let fast_path = matches!(
        method.as_str(),
        "initialize" | "server/discover" | "ping" | "tools/list"
    ) || method.starts_with("notifications/");
    let missing_elicitation_check = standard_transport
        && protocol_version == MODERN_PROTOCOL_VERSION
        && method == "tools/call"
        && !modern_client_supports_elicitation(&body)
        && !modern_request_has_input_responses(&body);
    let response_id = request_id.clone();
    let diagnostic_method_name = method.clone();
    let missing_elicitation_tool = tool_name.clone();
    let execution = execute_rpc(
        RpcExecutionContext {
            state,
            method,
            request_id,
            tool_name,
            argument_value,
            started_ts_ms,
            started_at,
            session_activity,
            request_timing,
            request_json_bytes,
            protocol_version,
            host_session_key: host_session_key_owned,
        },
        body,
        fast_path,
    );
    if !fast_path && !is_notification && !missing_elicitation_check {
        return streaming_json_no_store(execution);
    }
    let response = execution.await;
    if missing_elicitation_check
        && response
            .get("result")
            .and_then(|result| result.get("resultType"))
            .and_then(Value::as_str)
            == Some("input_required")
    {
        let transport_response = transport_error_with_data(
            StatusCode::BAD_REQUEST,
            -32021,
            response_id,
            &format!(
                "Client capability elicitation is required to approve {missing_elicitation_tool}"
            ),
            json!({ "requiredCapabilities": { "elicitation": {} } }),
        );
        record_transport_response(
            &diagnostic_workspace_id,
            &headers,
            &transport_response,
            Some(&diagnostic_method_name),
        );
        return transport_response;
    }
    if standard_transport && is_notification {
        StatusCode::ACCEPTED.into_response()
    } else {
        json_no_store(response)
    }
}

async fn execute_rpc(mut context: RpcExecutionContext, body: Value, fast_path: bool) -> Value {
    let profile_id = context.state.workspace_id.clone();
    let activity_seq = (!context.tool_name.is_empty()).then(|| {
        activity::begin(
            &profile_id,
            &context.tool_name,
            &context.argument_value,
            context.request_json_bytes,
        )
    });
    let mcp = context.state.mcp.clone();
    let era_method_mismatch = (context.protocol_version == MODERN_PROTOCOL_VERSION
        && matches!(context.method.as_str(), "initialize" | "ping"))
        || (context.protocol_version != MODERN_PROTOCOL_VERSION
            && context.method == "server/discover");
    let result: Result<Value, String> = if era_method_mismatch {
        Ok(json!({
            "jsonrpc": "2.0",
            "id": context.request_id.clone(),
            "error": {
                "code": -32601,
                "message": format!("Method not found: {}", context.method)
            }
        }))
    } else if fast_path {
        Ok(handle_request(&mcp, &body))
    } else {
        Ok(handle_request_async(mcp, body).await)
    };
    match result {
        Ok(response) => {
            let runtime = context.state.mcp.runtime_config();
            let response = if context.protocol_version == MODERN_PROTOCOL_VERSION {
                decorate_modern_response(response, &context.method, &runtime.tool_profile)
            } else {
                response
            };
            let duration_ms = context.started_at.elapsed().as_millis();
            append_profile_log_buffered(
                &profile_id,
                "mcp-requests.log",
                &format!(
                    "[rpc] completed id={} method={} tool={} duration_ms={}",
                    context.request_id, context.method, context.tool_name, duration_ms
                ),
            );
            if !context.tool_name.is_empty() {
                let result_is_error = response
                    .get("result")
                    .and_then(|result| result.get("isError"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let structured_ok = response
                    .get("result")
                    .and_then(|result| result.get("structuredContent"))
                    .and_then(|structured| structured.get("ok"))
                    .and_then(Value::as_bool);
                let outcome = if response.get("error").is_some() {
                    "rpc_error"
                } else if result_is_error || structured_ok == Some(false) {
                    "tool_error"
                } else {
                    "success"
                };
                if let Some(activity) = context.session_activity.as_mut() {
                    activity.complete(outcome, context.started_ts_ms.saturating_add(duration_ms));
                }
                if let Some(seq) = activity_seq {
                    activity::finish(
                        &profile_id,
                        seq,
                        outcome,
                        duration_ms,
                        Some(&response),
                        None,
                    );
                }
                record_tool_usage(ToolUsageInput {
                    profile_id: &profile_id,
                    transport_mode: &context.state.transport_mode,
                    protocol_version: &context.protocol_version,
                    request_id: &context.request_id,
                    method: &context.method,
                    tool_name: &context.tool_name,
                    arguments: &context.argument_value,
                    request_json_bytes: context.request_json_bytes,
                    rpc_fast_path: fast_path,
                    request_timing: context
                        .request_timing
                        .as_ref()
                        .expect("tool request timing"),
                    started_ts_ms: context.started_ts_ms,
                    duration_ms,
                    outcome,
                    response: Some(&response),
                    worker_error: None,
                    host_session_key: context.host_session_key.as_deref(),
                    redact_telemetry: context.state.redact_telemetry,
                });
            }
            if context.tool_name == "exec_command" || context.tool_name == "exec_health_check" {
                let structured = response
                    .get("result")
                    .and_then(|result| result.get("structuredContent"));
                let status = structured
                    .and_then(|value| value.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let termination_reason = structured
                    .and_then(|value| value.get("termination_reason"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let exit_code = structured
                    .and_then(|value| value.get("exit_code"))
                    .map(Value::to_string)
                    .unwrap_or_default();
                let is_error = response
                    .get("result")
                    .and_then(|result| result.get("isError"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                append_profile_log_buffered(
                    &profile_id,
                    "mcp-requests.log",
                    &format!(
                        "[exec] id={} tool={} is_error={} status={} termination_reason={} exit_code={}",
                        context.request_id,
                        context.tool_name,
                        is_error,
                        status,
                        termination_reason,
                        exit_code
                    ),
                );
            }
            response
        }
        Err(error) => {
            let duration_ms = context.started_at.elapsed().as_millis();
            append_profile_log_buffered(
                &profile_id,
                "mcp-requests.log",
                &format!(
                    "[rpc] worker_failed id={} method={} tool={} error={error}",
                    context.request_id, context.method, context.tool_name
                ),
            );
            if !context.tool_name.is_empty() {
                let worker_error = error.to_string();
                if let Some(seq) = activity_seq {
                    activity::finish(
                        &profile_id,
                        seq,
                        "worker_failed",
                        duration_ms,
                        None,
                        Some(&worker_error),
                    );
                }
                if let Some(activity) = context.session_activity.as_mut() {
                    activity.complete(
                        "worker_failed",
                        context.started_ts_ms.saturating_add(duration_ms),
                    );
                }
                record_tool_usage(ToolUsageInput {
                    profile_id: &profile_id,
                    transport_mode: &context.state.transport_mode,
                    protocol_version: &context.protocol_version,
                    request_id: &context.request_id,
                    method: &context.method,
                    tool_name: &context.tool_name,
                    arguments: &context.argument_value,
                    request_json_bytes: context.request_json_bytes,
                    rpc_fast_path: fast_path,
                    request_timing: context
                        .request_timing
                        .as_ref()
                        .expect("tool request timing"),
                    started_ts_ms: context.started_ts_ms,
                    duration_ms,
                    outcome: "worker_failed",
                    response: None,
                    worker_error: Some(&worker_error),
                    host_session_key: context.host_session_key.as_deref(),
                    redact_telemetry: context.state.redact_telemetry,
                });
            }
            let error_body = json!({
                "jsonrpc": "2.0",
                "id": context.request_id,
                "error": {
                    "code": -32603,
                    "message": "Exec RPC worker failed",
                    "data": {
                        "stage": "rpc_worker",
                        "reason": "worker_failed",
                        "retryable": true,
                        "suggestion": "重试请求或重启 MCP 运行时"
                    }
                }
            });
            error_body
        }
    }
}

fn validate_standard_connection(
    state: &ListenerState,
    headers: &HeaderMap,
    _post_request: bool,
) -> Option<Response> {
    if !origin_is_allowed(state, headers) {
        return Some(transport_error(
            StatusCode::FORBIDDEN,
            -32000,
            "Invalid Origin header",
        ));
    }

    if let Some(version) = headers.get("mcp-protocol-version") {
        let Ok(version) = version.to_str() else {
            return Some(transport_error(
                StatusCode::BAD_REQUEST,
                -32600,
                "Invalid MCP-Protocol-Version header",
            ));
        };
        if !is_supported_protocol_version(version.trim()) {
            let requested = version.trim();
            return Some(transport_error_with_data(
                StatusCode::BAD_REQUEST,
                -32022,
                Value::Null,
                &format!("Unsupported MCP protocol version: {requested}"),
                json!({
                    "supported": SUPPORTED_PROTOCOL_VERSIONS,
                    "requested": requested
                }),
            ));
        }
    }

    require_mcp_auth(state, headers)
        .map(|response| with_authenticate_challenge(state, headers, response))
}

fn modern_request_meta(body: &Value) -> Option<&serde_json::Map<String, Value>> {
    body.get("params")?.get("_meta")?.as_object()
}

fn modern_client_supports_elicitation(body: &Value) -> bool {
    modern_request_meta(body)
        .and_then(|meta| meta.get("io.modelcontextprotocol/clientCapabilities"))
        .and_then(Value::as_object)
        .and_then(|capabilities| capabilities.get("elicitation"))
        .is_some_and(Value::is_object)
}

fn modern_request_has_input_responses(body: &Value) -> bool {
    body.get("params")
        .and_then(|params| params.get("inputResponses"))
        .is_some_and(Value::is_object)
}

fn modern_subscription_notifications(body: &Value) -> Option<&serde_json::Map<String, Value>> {
    let notifications = body.get("params")?.get("notifications")?.as_object()?;
    for key in [
        "toolsListChanged",
        "promptsListChanged",
        "resourcesListChanged",
    ] {
        if notifications
            .get(key)
            .is_some_and(|value| !value.is_boolean())
        {
            return None;
        }
    }
    if let Some(resources) = notifications.get("resourceSubscriptions") {
        let Some(resources) = resources.as_array() else {
            return None;
        };
        if resources.iter().any(|value| !value.is_string()) {
            return None;
        }
    }
    Some(notifications)
}

fn modern_request_detected(headers: &HeaderMap, body: &Value) -> bool {
    let header_modern = headers
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim() == MODERN_PROTOCOL_VERSION);
    let body_modern = modern_request_meta(body)
        .and_then(|meta| meta.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str)
        .is_some_and(|value| value == MODERN_PROTOCOL_VERSION);
    header_modern || body_modern
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TransportDiagnostic {
    status: u16,
    code: i64,
}

fn attach_transport_diagnostic(mut response: Response, status: StatusCode, code: i64) -> Response {
    response.extensions_mut().insert(TransportDiagnostic {
        status: status.as_u16(),
        code,
    });
    response
}

fn record_transport_response(
    workspace_id: &str,
    headers: &HeaderMap,
    response: &Response,
    method: Option<&str>,
) {
    let Some(diagnostic) = response.extensions().get::<TransportDiagnostic>() else {
        return;
    };
    let mut fields = json!({
        "rpc_error_code": diagnostic.code.to_string(),
        "http_status": diagnostic.status,
        "transport_mode": "streamable-http"
    });
    if let Some(fields) = fields.as_object_mut() {
        if let Some(protocol_version) = headers
            .get("mcp-protocol-version")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            fields.insert("protocol_version".into(), json!(protocol_version));
        }
        if let Some(method) = method.filter(|value| !value.is_empty()) {
            fields.insert("method".into(), json!(method));
        }
    }
    crate::mcp::record_diagnostic_event(
        workspace_id,
        "transport_event",
        "mcp_transport_error",
        "error",
        "transport",
        fields,
    );
}

fn transport_error_with_data(
    status: StatusCode,
    code: i64,
    id: Value,
    message: &str,
    data: Value,
) -> Response {
    attach_transport_diagnostic(
        (
            status,
            [(CACHE_CONTROL, "no-store")],
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message, "data": data }
            })),
        )
            .into_response(),
        status,
        code,
    )
}

fn transport_error_with_id(status: StatusCode, code: i64, id: Value, message: &str) -> Response {
    attach_transport_diagnostic(
        (
            status,
            [(CACHE_CONTROL, "no-store")],
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message }
            })),
        )
            .into_response(),
        status,
        code,
    )
}

fn decode_mcp_header_value(value: &str) -> Option<String> {
    let prefix_len = "=?base64?".len();
    if value.len() < prefix_len + 2
        || !value[..prefix_len].eq_ignore_ascii_case("=?base64?")
        || !value.ends_with("?=")
    {
        return Some(value.to_owned());
    }
    let encoded = &value[prefix_len..value.len() - 2];
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    String::from_utf8(decoded).ok()
}

fn mirrored_primitive_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn validate_modern_tool_headers(
    headers: &HeaderMap,
    body: &Value,
    tool_profile: &str,
) -> Option<Response> {
    if !modern_request_detected(headers, body)
        || body.get("method").and_then(Value::as_str) != Some("tools/call")
    {
        return None;
    }
    let params = body.get("params")?.as_object()?;
    let tool_name = params.get("name")?.as_str()?;
    let arguments = params.get("arguments").and_then(Value::as_object);
    let tool = crate::tools::list_tools_for_profile(tool_profile)
        .into_iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some(tool_name))?;
    let properties = tool
        .get("inputSchema")
        .and_then(|schema| schema.get("properties"))
        .and_then(Value::as_object)?;
    for (property_name, property_schema) in properties {
        let Some(header_suffix) = property_schema
            .get("x-mcp-header")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(body_value) = arguments.and_then(|values| values.get(property_name)) else {
            continue;
        };
        if body_value.is_null() {
            continue;
        }
        let Some(expected) = mirrored_primitive_value(body_value) else {
            continue;
        };
        let header_name = format!("mcp-param-{}", header_suffix.to_ascii_lowercase());
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let Some(raw_header) = headers
            .get(header_name.as_str())
            .and_then(|value| value.to_str().ok())
        else {
            return Some(transport_error_with_id(
                StatusCode::BAD_REQUEST,
                -32020,
                id,
                &format!("Header mismatch: Mcp-Param-{header_suffix} header is required"),
            ));
        };
        let Some(decoded) = decode_mcp_header_value(raw_header) else {
            return Some(transport_error_with_id(
                StatusCode::BAD_REQUEST,
                -32020,
                id,
                &format!("Header mismatch: Mcp-Param-{header_suffix} header is malformed"),
            ));
        };
        if decoded != expected {
            return Some(transport_error_with_id(
                StatusCode::BAD_REQUEST,
                -32020,
                id,
                &format!(
                    "Header mismatch: Mcp-Param-{header_suffix} header value '{decoded}' does not match body value '{expected}'"
                ),
            ));
        }
    }
    None
}

fn validate_modern_request(headers: &HeaderMap, body: &Value) -> Option<Response> {
    if !modern_request_detected(headers, body) {
        return None;
    }
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let protocol_header = headers
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok())
        .map(str::trim);
    let meta = modern_request_meta(body);
    let body_protocol = meta
        .and_then(|value| value.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str);
    if protocol_header != Some(MODERN_PROTOCOL_VERSION) || body_protocol != protocol_header {
        return Some(transport_error_with_id(
            StatusCode::BAD_REQUEST,
            -32020,
            id,
            "Header mismatch: MCP-Protocol-Version must match request _meta protocol version",
        ));
    }
    let method_header = headers
        .get("mcp-method")
        .and_then(|value| value.to_str().ok());
    if method_header != Some(method) {
        return Some(transport_error_with_id(
            StatusCode::BAD_REQUEST,
            -32020,
            id,
            "Header mismatch: Mcp-Method must match the JSON-RPC method",
        ));
    }
    let params = body.get("params").and_then(Value::as_object);
    let expected_name = match method {
        "tools/call" | "prompts/get" => params
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str),
        "resources/read" => params
            .and_then(|value| value.get("uri"))
            .and_then(Value::as_str),
        _ => None,
    };
    if matches!(method, "tools/call" | "prompts/get" | "resources/read") {
        let name_header = headers
            .get("mcp-name")
            .and_then(|value| value.to_str().ok())
            .and_then(decode_mcp_header_value);
        if name_header.as_deref() != expected_name {
            return Some(transport_error_with_id(
                StatusCode::BAD_REQUEST,
                -32020,
                id,
                "Header mismatch: Mcp-Name must match the request target",
            ));
        }
    }
    let capabilities =
        meta.and_then(|value| value.get("io.modelcontextprotocol/clientCapabilities"));
    if !capabilities.is_some_and(Value::is_object) {
        return Some(transport_error_with_id(
            StatusCode::BAD_REQUEST,
            -32602,
            id,
            "Invalid request metadata: io.modelcontextprotocol/clientCapabilities is required",
        ));
    }
    if let Some(client_info) =
        meta.and_then(|value| value.get("io.modelcontextprotocol/clientInfo"))
    {
        let valid = client_info.as_object().is_some_and(|info| {
            info.get("name").and_then(Value::as_str).is_some()
                && info.get("version").and_then(Value::as_str).is_some()
        });
        if !valid {
            return Some(transport_error_with_id(
                StatusCode::BAD_REQUEST,
                -32602,
                id,
                "Invalid request metadata: clientInfo requires name and version",
            ));
        }
    }
    None
}

fn decorate_modern_response(mut response: Value, method: &str, tool_profile: &str) -> Value {
    if method == "tools/call" {
        if let Some(input_required) = response.get("result").and_then(permission_input_required) {
            response["result"] = input_required;
        }
    }
    let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) else {
        return response;
    };
    result
        .entry("resultType")
        .or_insert_with(|| Value::String("complete".into()));
    if matches!(
        method,
        "server/discover" | "tools/list" | "prompts/list" | "resources/list" | "resources/read"
    ) {
        result.entry("ttlMs").or_insert_with(|| json!(0));
        result
            .entry("cacheScope")
            .or_insert_with(|| Value::String("private".into()));
    }
    let meta = result.entry("_meta").or_insert_with(|| json!({}));
    if let Some(meta) = meta.as_object_mut() {
        meta.insert(
            "io.modelcontextprotocol/serverInfo".into(),
            json!({
                "name": "coding-tools-mcp",
                "title": "Coding Tools MCP",
                "version": env!("CARGO_PKG_VERSION"),
                "toolsetRevision": crate::tools::registry::toolset_revision(tool_profile)
            }),
        );
    }
    response
}

fn validate_json_rpc_message(body: &Value) -> Option<Response> {
    let Some(object) = body.as_object() else {
        return Some(transport_error(
            StatusCode::BAD_REQUEST,
            -32600,
            "The request body must be one JSON-RPC message",
        ));
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(transport_error(
            StatusCode::BAD_REQUEST,
            -32600,
            "jsonrpc must be '2.0'",
        ));
    }

    let has_method = object.get("method").and_then(Value::as_str).is_some();
    let has_id = object.contains_key("id");
    let is_response =
        !has_method && has_id && (object.contains_key("result") || object.contains_key("error"));
    if !has_method && !is_response {
        return Some(transport_error(
            StatusCode::BAD_REQUEST,
            -32600,
            "Invalid JSON-RPC request, notification, or response",
        ));
    }
    None
}

fn origin_is_allowed(state: &ListenerState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Some(origin) = normalized_origin(origin) else {
        return false;
    };

    if origin_matches_listener(state, &origin) {
        return true;
    }

    if matches!(
        origin.as_str(),
        "https://chatgpt.com" | "https://chat.openai.com"
    ) {
        return true;
    }

    normalized_origin(&state.configured_public_url).is_some_and(|allowed| allowed == origin)
}

fn origin_matches_listener(state: &ListenerState, origin: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(origin) else {
        return false;
    };
    if url.scheme() != "http" || url.port_or_known_default() != Some(state.bind_port) {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let Ok(bind_address) = parse_bind_address(&state.bind_address) else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return bind_address.is_loopback() || bind_address.is_unspecified();
    }
    let Ok(origin_address) = host.parse::<std::net::IpAddr>() else {
        return false;
    };
    bind_address.is_unspecified()
        || bind_address == origin_address
        || (bind_address.is_loopback() && origin_address.is_loopback())
}

fn normalized_origin(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

fn with_authenticate_challenge(
    state: &ListenerState,
    headers: &HeaderMap,
    mut response: Response,
) -> Response {
    if response.status() != StatusCode::UNAUTHORIZED {
        return response;
    }
    let challenge = if state.auth.oauth_enabled() {
        format!(
            "Bearer resource_metadata=\"{}\"",
            protected_resource_metadata_url(&resolve_oauth_base(state, headers))
        )
    } else {
        "Bearer".to_string()
    };
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response.headers_mut().insert(WWW_AUTHENTICATE, value);
    }
    response
}

fn transport_error(status: StatusCode, code: i64, message: &str) -> Response {
    attach_transport_diagnostic(
        (
            status,
            [(CACHE_CONTROL, "no-store")],
            Json(json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {
                    "code": code,
                    "message": message
                }
            })),
        )
            .into_response(),
        status,
        code,
    )
}

fn json_no_store(value: Value) -> Response {
    ([(CACHE_CONTROL, "no-store")], Json(value)).into_response()
}

fn subscription_sse_response(
    subscription_id: Value,
    notifications: Value,
    heartbeat_interval: Duration,
) -> Response {
    let (sender, receiver) =
        mpsc::channel::<Result<Vec<u8>, Infallible>>(MCP_STREAM_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        let acknowledged = json!({
            "jsonrpc": "2.0",
            "method": "notifications/subscriptions/acknowledged",
            "params": {
                "notifications": notifications,
                "_meta": {
                    "io.modelcontextprotocol/subscriptionId": subscription_id
                }
            }
        });
        let payload = format!("data: {}\n\n", acknowledged);
        if sender.send(Ok(payload.into_bytes())).await.is_err() {
            return;
        }

        let mut heartbeat = interval(heartbeat_interval);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        heartbeat.tick().await;
        loop {
            tokio::select! {
                _ = sender.closed() => return,
                _ = heartbeat.tick() => {
                    let _ = sender.try_send(Ok(b":\n\n".to_vec()));
                }
            }
        }
    });

    let stream = unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .header(CACHE_CONTROL, "no-store")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| {
            transport_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                -32603,
                "Failed to create subscription stream",
            )
        })
}

fn streaming_json_no_store<F>(execution: F) -> Response
where
    F: Future<Output = Value> + Send + 'static,
{
    let (sender, receiver) =
        mpsc::channel::<Result<Vec<u8>, Infallible>>(MCP_STREAM_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        let mut heartbeat = interval(MCP_STREAM_HEARTBEAT_INTERVAL);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        heartbeat.tick().await;
        tokio::pin!(execution);
        loop {
            tokio::select! {
                _ = sender.closed() => return,
                response = &mut execution => {
                    let payload = serde_json::to_vec(&response).unwrap_or_else(|_| {
                        br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Failed to serialize RPC response"}}"#.to_vec()
                    });
                    let _ = sender.send(Ok(payload)).await;
                    return;
                }
                _ = heartbeat.tick() => {
                    // Leading JSON whitespace keeps every proxy layer active while
                    // preserving a standards-compatible application/json body.
                    let _ = sender.try_send(Ok(b"\n".to_vec()));
                }
            }
        }
    });

    let stream = unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .header(CACHE_CONTROL, "no-store")
        .header("x-accel-buffering", "no")
        .header("x-coding-tools-streaming", "1")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| {
            transport_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                -32603,
                "Failed to create streaming RPC response",
            )
        })
}

fn method_not_allowed(allow: &'static str) -> Response {
    let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static(allow));
    response
}

fn require_mcp_auth(state: &ListenerState, headers: &HeaderMap) -> Option<Response> {
    if state.auth.bearer_enabled() {
        let expected = state.bearer_token.as_deref().unwrap_or("");
        return verify_bearer_header(headers, expected);
    }
    if state.auth.oauth_enabled() {
        if let Some(oauth) = state.oauth.as_ref() {
            let server_url = resolve_oauth_base(state, headers);
            return verify_oauth_bearer_header(headers, oauth, &server_url);
        }
    }
    None
}

async fn oauth_authorization_server_metadata(
    State(state): State<ListenerState>,
    headers: HeaderMap,
) -> Response {
    if !state.auth.oauth_enabled() {
        return oauth_not_configured();
    }
    let base = resolve_oauth_base(&state, &headers);
    let mut metadata = authorization_server_metadata(&base, state.oauth_client_secret.as_deref());
    if state.oauth_client_secret.is_none() {
        if let Some(object) = metadata.as_object_mut() {
            object.insert(
                "registration_endpoint".into(),
                Value::String(format!("{}/oauth/register", base.trim_end_matches('/'))),
            );
        }
    }
    Json(metadata).into_response()
}

/// Minimal RFC 7591 Dynamic Client Registration for public clients.
///
/// Claude, Claude Code, Cursor, VS Code and most MCP clients register
/// themselves automatically instead of asking the user for a Client ID. This
/// server has a single public client per workspace, so registration simply
/// hands out that client id after validating the redirect URIs. Access is
/// still gated by the one-time authorization password and PKCE.
async fn oauth_register_post(
    State(state): State<ListenerState>,
    Json(body): Json<Value>,
) -> Response {
    let Some(oauth) = state.oauth.as_ref() else {
        return oauth_not_configured();
    };
    if state.oauth_client_secret.is_some() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_client_metadata",
                "error_description": "This server uses a confidential client; configure the Client ID and secret manually."
            })),
        )
            .into_response();
    }
    let redirect_uris: Vec<String> = body
        .get("redirect_uris")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if redirect_uris.is_empty()
        || !redirect_uris
            .iter()
            .all(|uri| crate::auth::redirect_uri_allowed(uri))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_redirect_uri",
                "error_description": "redirect_uris must use an allowed MCP client callback (ChatGPT, Claude, VS Code, Cursor, or a loopback address)."
            })),
        )
            .into_response();
    }
    let client_id = if oauth.client_id.trim().is_empty() {
        format!("mcp-client-{}", uuid::Uuid::new_v4().simple())
    } else {
        oauth.client_id.clone()
    };
    let issued_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default();
    let mut response = json!({
        "client_id": client_id,
        "client_id_issued_at": issued_at,
        "redirect_uris": redirect_uris,
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "scope": "mcp",
    });
    if let Some(name) = body.get("client_name").and_then(Value::as_str) {
        response["client_name"] = Value::String(name.chars().take(200).collect());
    }
    (StatusCode::CREATED, Json(response)).into_response()
}

async fn oauth_protected_resource_metadata(
    State(state): State<ListenerState>,
    headers: HeaderMap,
) -> Response {
    if !state.auth.oauth_enabled() {
        return oauth_not_configured();
    }
    Json(protected_resource_metadata(&resolve_oauth_base(
        &state, &headers,
    )))
    .into_response()
}

async fn oauth_authorize_get(
    State(state): State<ListenerState>,
    Query(params): Query<AuthorizeParams>,
) -> Response {
    let Some(oauth) = state.oauth.as_ref() else {
        return oauth_not_configured();
    };
    authorize_get(oauth, params, None)
}

async fn oauth_authorize_post(
    State(state): State<ListenerState>,
    headers: HeaderMap,
    Form(form): Form<AuthorizeForm>,
) -> Response {
    let Some(oauth) = state.oauth.as_ref() else {
        return oauth_not_configured();
    };
    authorize_post(oauth, form, &resolve_oauth_base(&state, &headers))
}

async fn oauth_token_post(
    State(state): State<ListenerState>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    let Some(oauth) = state.oauth.as_ref() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unsupported_grant_type" })),
        )
            .into_response();
    };
    token_exchange(oauth, &headers, form, &resolve_oauth_base(&state, &headers))
}

fn oauth_not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "OAuth not configured" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    #[test]
    fn injects_stable_session_identity_for_generic_clients() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            axum::http::HeaderValue::from_static("Bearer abc"),
        );
        let mut body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "read_file", "arguments": {}}
        });
        super::ensure_host_session_identity(&headers, &mut body);
        let first = body["params"]["_meta"]["openai/session"]
            .as_str()
            .expect("injected")
            .to_string();
        assert!(first.starts_with("mcp-client:"));

        let mut again =
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "x"}});
        super::ensure_host_session_identity(&headers, &mut again);
        assert_eq!(again["params"]["_meta"]["openai/session"], first);

        let mut chatgpt = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "x", "_meta": {"openai/session": "conv-1"}}
        });
        super::ensure_host_session_identity(&headers, &mut chatgpt);
        assert_eq!(chatgpt["params"]["_meta"]["openai/session"], "conv-1");

        let mut init = json!({"jsonrpc": "2.0", "id": 4, "method": "initialize", "params": {}});
        super::ensure_host_session_identity(&headers, &mut init);
        assert!(init["params"].get("_meta").is_none());
    }

    use std::sync::Arc;
    use std::time::Duration;

    use axum::extract::State;
    use axum::http::{header::CACHE_CONTROL, HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::Json;
    use serde_json::{json, Value};
    use tempfile::TempDir;

    use crate::mcp::telemetry::format_log_value;
    use crate::tools::ToolContext;
    use crate::workspace::AuthConfig;

    use super::{
        authorization_metadata_path, bind_listener, build_router, configured_route_prefix,
        decode_mcp_header_value, mcp_discovery_payload, mcp_get, mcp_info, mcp_post,
        origin_matches_listener, prefixed_route, protected_resource_metadata_path,
        validate_modern_tool_headers, ListenerState, TransportDiagnostic,
    };

    fn test_state(transport_mode: &str) -> (TempDir, TempDir, ListenerState) {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let mcp = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        let auth = AuthConfig {
            auth_type: "noauth".into(),
            ..AuthConfig::default()
        };
        let state = ListenerState {
            mcp,
            auth,
            workspace_id: "test-workspace".into(),
            bind_address: "127.0.0.1".into(),
            bind_port: 28_766,
            configured_public_url: "https://mcp.example.com".into(),
            bearer_token: None,
            oauth: None,
            oauth_client_secret: None,
            transport_mode: transport_mode.into(),
            redact_telemetry: true,
        };
        (workspace, harness, state)
    }

    #[test]
    fn bind_listener_reports_port_conflict_synchronously() {
        let occupied = std::net::TcpListener::bind(("0.0.0.0", 0)).expect("占用测试端口");
        let port = occupied.local_addr().expect("读取测试端口").port();

        assert!(bind_listener("0.0.0.0", port).is_err());
    }

    #[test]
    fn public_base_url_creates_path_scoped_routes() {
        assert_eq!(
            configured_route_prefix("https://mcp.example.com/clients/pc-a"),
            "/clients/pc-a"
        );
        assert_eq!(prefixed_route("/clients/pc-a", "/mcp"), "/clients/pc-a/mcp");
        assert_eq!(prefixed_route("", "/mcp"), "/mcp");
        assert_eq!(
            authorization_metadata_path("/clients/pc-a"),
            "/.well-known/oauth-authorization-server/clients/pc-a"
        );
        assert_eq!(
            protected_resource_metadata_path("/clients/pc-a"),
            "/.well-known/oauth-protected-resource/clients/pc-a/mcp"
        );
    }

    #[tokio::test]
    async fn path_scoped_mcp_endpoint_accepts_requests() {
        let (_workspace, _harness, mut state) = test_state("streamable-http");
        state.configured_public_url = "https://mcp.example.com/clients/pc-a".into();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("test listener address");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, build_router(state)).await;
        });

        let response = reqwest::Client::new()
            .post(format!("http://{address}/clients/pc-a/mcp"))
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": {}
            }))
            .send()
            .await
            .expect("path-scoped MCP request");

        assert_eq!(response.status(), StatusCode::OK);
        server.abort();
    }

    #[tokio::test]
    async fn modern_mcp_permission_mrtr_requires_elicitation_capability() {
        let (_workspace, _harness, state) = test_state("streamable-http");
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("test listener address");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, build_router(state)).await;
        });
        let client = reqwest::Client::new();
        #[cfg(windows)]
        let (shell, command) = ("powershell", "Write-Output mrtr-permission");
        #[cfg(unix)]
        let (shell, command) = ("sh", "printf mrtr-permission");
        let request = |id: i64, capabilities: Value| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": "exec_command",
                    "arguments": {
                        "cmd": command,
                        "shell": shell,
                        "timeout_ms": 5000,
                        "yield_time_ms": 5000
                    },
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                        "io.modelcontextprotocol/clientCapabilities": capabilities
                    }
                }
            })
        };

        let missing = client
            .post(format!("http://{address}/mcp"))
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/call")
            .header("mcp-name", "exec_command")
            .json(&request(51, json!({})))
            .send()
            .await
            .expect("missing elicitation request");
        assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
        let missing_body: Value = missing.json().await.expect("missing elicitation json");
        assert_eq!(missing_body["id"], 51);
        assert_eq!(missing_body["error"]["code"], -32021);
        assert_eq!(
            missing_body["error"]["data"]["requiredCapabilities"],
            json!({"elicitation": {}})
        );

        let capable = client
            .post(format!("http://{address}/mcp"))
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/call")
            .header("mcp-name", "exec_command")
            .json(&request(52, json!({"elicitation": {}})))
            .send()
            .await
            .expect("elicitation capable request");
        assert_eq!(capable.status(), StatusCode::OK);
        let capable_body: Value = capable.json().await.expect("input required json");
        assert_eq!(capable_body["result"]["resultType"], "input_required");
        assert_eq!(
            capable_body["result"]["inputRequests"]["permission_approval"]["method"],
            "elicitation/create"
        );
        assert!(capable_body["result"]["requestState"]
            .as_str()
            .is_some_and(|state| state.starts_with("permission:")));
        server.abort();
    }
    #[tokio::test]
    async fn modern_mcp_http_rejects_client_responses_and_returns_not_found_for_unknown_methods() {
        let (_workspace, _harness, state) = test_state("streamable-http");
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("test listener address");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, build_router(state)).await;
        });
        let client = reqwest::Client::new();

        let response = client
            .post(format!("http://{address}/mcp"))
            .header("mcp-protocol-version", "2026-07-28")
            .json(&json!({"jsonrpc": "2.0", "id": 18, "result": {}}))
            .send()
            .await
            .expect("modern client response POST");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response_body: Value = response.json().await.expect("modern response error json");
        assert_eq!(response_body["id"], 18);
        assert_eq!(response_body["error"]["code"], -32600);

        let unknown = client
            .post(format!("http://{address}/mcp"))
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "unknown/method")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 19,
                "method": "unknown/method",
                "params": {
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                        "io.modelcontextprotocol/clientCapabilities": {}
                    }
                }
            }))
            .send()
            .await
            .expect("unknown modern request");
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        let unknown_body: Value = unknown.json().await.expect("unknown method error json");
        assert_eq!(unknown_body["id"], 19);
        assert_eq!(unknown_body["error"]["code"], -32601);

        let legacy = client
            .post(format!("http://{address}/mcp"))
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({"jsonrpc": "2.0", "id": 20, "result": {}}))
            .send()
            .await
            .expect("legacy client response POST");
        assert_eq!(legacy.status(), StatusCode::ACCEPTED);
        server.abort();
    }

    #[tokio::test]
    async fn modern_mcp_subscriptions_listen_opens_sse_and_acknowledges_honored_filter() {
        let (_workspace, _harness, state) = test_state("streamable-http");
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("test listener address");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, build_router(state)).await;
        });
        let client = reqwest::Client::new();
        let mut response = client
            .post(format!("http://{address}/mcp"))
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "subscriptions/listen")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 61,
                "method": "subscriptions/listen",
                "params": {
                    "notifications": {
                        "toolsListChanged": true,
                        "promptsListChanged": true,
                        "resourcesListChanged": true,
                        "resourceSubscriptions": ["file:///unsupported.txt"]
                    },
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                        "io.modelcontextprotocol/clientCapabilities": {}
                    }
                }
            }))
            .send()
            .await
            .expect("subscription listen request");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        assert_eq!(
            response
                .headers()
                .get("x-accel-buffering")
                .and_then(|value| value.to_str().ok()),
            Some("no")
        );

        let first = tokio::time::timeout(Duration::from_secs(2), response.chunk())
            .await
            .expect("subscription acknowledgement timed out")
            .expect("subscription chunk read")
            .expect("subscription stream ended before acknowledgement");
        let first = String::from_utf8(first.to_vec()).expect("utf8 subscription acknowledgement");
        let data = first
            .strip_prefix("data: ")
            .and_then(|value| value.strip_suffix("\n\n"))
            .expect("SSE data frame");
        let acknowledged: Value = serde_json::from_str(data).expect("acknowledgement json");
        assert_eq!(
            acknowledged["method"],
            "notifications/subscriptions/acknowledged"
        );
        assert_eq!(acknowledged["params"]["notifications"], json!({}));
        assert_eq!(
            acknowledged["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
            61
        );

        assert!(
            tokio::time::timeout(Duration::from_millis(100), response.chunk())
                .await
                .is_err(),
            "listen stream should remain open after acknowledgement"
        );
        drop(response);
        server.abort();
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn modern_mcp_tasks_round_trip_over_streamable_http() {
        let (_workspace, _harness, state) = test_state("streamable-http");
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("test listener address");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, build_router(state)).await;
        });
        let client = reqwest::Client::new();
        let session = format!("http-tasks-{}", uuid::Uuid::new_v4());
        let meta = json!({
            "openai/session": session,
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {
                "extensions": {"io.modelcontextprotocol/tasks": {}}
            }
        });
        #[cfg(windows)]
        let command = "powershell -NoProfile -Command \"Start-Sleep -Milliseconds 200; Write-Output http-task-complete\"";
        #[cfg(unix)]
        let command = "sh -c \"sleep 0.2; printf http-task-complete\"";

        let created = client
            .post(format!("http://{address}/mcp"))
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/call")
            .header("mcp-name", "exec_command")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 71,
                "method": "tools/call",
                "params": {
                    "name": "exec_command",
                    "arguments": {
                        "cmd": command,
                        "yield_time_ms": 0,
                        "timeout_ms": 5000,
                        "output_mode": "tail"
                    },
                    "_meta": meta.clone()
                }
            }))
            .send()
            .await
            .expect("create task request");
        assert_eq!(created.status(), StatusCode::OK);
        let created: Value = created.json().await.expect("create task json");
        assert_eq!(created["result"]["resultType"], "task", "{created}");
        let task_id = created["result"]["taskId"]
            .as_str()
            .expect("task id")
            .to_string();

        let mut completed = None;
        for attempt in 0..40 {
            let response = client
                .post(format!("http://{address}/mcp"))
                .header("mcp-protocol-version", "2026-07-28")
                .header("mcp-method", "tasks/get")
                .json(&json!({
                    "jsonrpc": "2.0",
                    "id": 72 + attempt,
                    "method": "tasks/get",
                    "params": {"taskId": task_id, "_meta": meta.clone()}
                }))
                .send()
                .await
                .expect("get task request");
            assert_eq!(response.status(), StatusCode::OK);
            let body: Value = response.json().await.expect("get task json");
            assert_eq!(body["result"]["resultType"], "complete", "{body}");
            if body["result"]["status"] == "completed" {
                completed = Some(body["result"].clone());
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let completed = completed.expect("HTTP task should complete");
        assert_eq!(completed["result"]["isError"], false);
        assert!(completed["result"]["structuredContent"]["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("http-task-complete"));
        server.abort();
    }

    #[tokio::test]
    async fn modern_mcp_discover_requires_routing_metadata_and_returns_modern_envelope() {
        let (_workspace, _harness, state) = test_state("streamable-http");
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("test listener address");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, build_router(state)).await;
        });
        let client = reqwest::Client::new();
        let body = json!({
            "jsonrpc": "2.0",
            "id": 41,
            "method": "server/discover",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        });

        let missing_method = client
            .post(format!("http://{address}/mcp"))
            .header("mcp-protocol-version", "2026-07-28")
            .json(&body)
            .send()
            .await
            .expect("modern mismatch request");
        assert_eq!(missing_method.status(), StatusCode::BAD_REQUEST);
        let mismatch: serde_json::Value = missing_method.json().await.expect("mismatch json");
        assert_eq!(mismatch["error"]["code"], -32020);
        assert_eq!(mismatch["id"], 41);

        let response = client
            .post(format!("http://{address}/mcp"))
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "server/discover")
            .json(&body)
            .send()
            .await
            .expect("modern discover request");
        assert_eq!(response.status(), StatusCode::OK);
        let discovered: serde_json::Value = response.json().await.expect("modern discover json");
        assert_eq!(
            discovered["result"]["supportedVersions"],
            json!(["2026-07-28"])
        );
        assert_eq!(
            discovered["result"]["capabilities"],
            json!({
                "tools": {"listChanged": false},
                "prompts": {"listChanged": false},
                "resources": {"subscribe": false, "listChanged": false},
                "extensions": {"io.modelcontextprotocol/tasks": {}}
            })
        );
        assert!(discovered["result"].get("protocolVersion").is_none());
        assert_eq!(discovered["result"]["resultType"], "complete");
        assert_eq!(discovered["result"]["ttlMs"], 0);
        assert_eq!(discovered["result"]["cacheScope"], "private");
        assert_eq!(
            discovered["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "coding-tools-mcp"
        );
        server.abort();
    }

    #[test]
    fn modern_mcp_param_headers_mirror_workspace_selector_and_accept_case_insensitive_base64() {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "tools/call",
            "params": {
                "name": "read_file",
                "arguments": {"path": "README.md", "workspace_folder_id": "folder-a"},
                "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}
            }
        });
        let mut headers = HeaderMap::new();
        headers.insert("mcp-protocol-version", "2026-07-28".parse().unwrap());
        assert_eq!(
            validate_modern_tool_headers(&headers, &body, "core")
                .expect("missing mirrored header")
                .status(),
            StatusCode::BAD_REQUEST
        );
        headers.insert("mcp-param-workspace", "folder-a".parse().unwrap());
        assert!(validate_modern_tool_headers(&headers, &body, "core").is_none());
        assert_eq!(
            decode_mcp_header_value("=?BASE64?IGZvbGRlci1h?=").as_deref(),
            Some(" folder-a")
        );
    }

    #[tokio::test]
    async fn path_scoped_oauth_metadata_uses_standard_well_known_routes() {
        let (_workspace, _harness, mut state) = test_state("streamable-http");
        state.auth.auth_type = "oauth".into();
        state.configured_public_url = "https://mcp.example.com/clients/pc-a".into();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("test listener address");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, build_router(state)).await;
        });
        let client = reqwest::Client::new();

        let authorization: serde_json::Value = client
            .get(format!(
                "http://{address}/.well-known/oauth-authorization-server/clients/pc-a"
            ))
            .send()
            .await
            .expect("authorization metadata request")
            .json()
            .await
            .expect("authorization metadata response");
        assert_eq!(
            authorization["issuer"],
            "https://mcp.example.com/clients/pc-a"
        );

        let protected: serde_json::Value = client
            .get(format!(
                "http://{address}/.well-known/oauth-protected-resource/clients/pc-a/mcp"
            ))
            .send()
            .await
            .expect("protected resource metadata request")
            .json()
            .await
            .expect("protected resource metadata response");
        assert_eq!(
            protected["resource"],
            "https://mcp.example.com/clients/pc-a/mcp"
        );
        assert_eq!(
            protected["authorization_servers"],
            json!(["https://mcp.example.com/clients/pc-a"])
        );
        server.abort();
    }

    #[test]
    fn configured_listener_origin_is_allowed() {
        let (_workspace, _harness, mut state) = test_state("streamable-http");
        state.bind_address = "192.168.1.20".into();

        assert!(origin_matches_listener(&state, "http://192.168.1.20:28766"));
        assert!(!origin_matches_listener(
            &state,
            "http://192.168.1.21:28766"
        ));
    }

    #[test]
    fn wildcard_listener_accepts_numeric_interface_origins() {
        let (_workspace, _harness, mut state) = test_state("streamable-http");
        state.bind_address = "0.0.0.0".into();

        assert!(origin_matches_listener(&state, "http://192.168.1.20:28766"));
        assert!(!origin_matches_listener(&state, "http://example.com:28766"));
    }

    #[tokio::test]
    async fn discovery_reports_the_current_package_version() {
        let discovery = mcp_discovery_payload();

        assert_eq!(discovery["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn discovery_prevents_stale_tool_catalog_caching() {
        let response = mcp_info().await.into_response();

        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
    }

    #[tokio::test]
    async fn standard_get_returns_method_not_allowed() {
        let (_workspace, _harness, state) = test_state("streamable-http");
        let response = mcp_get(State(state), HeaderMap::new()).await;

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn legacy_get_keeps_discovery_fallback() {
        let (_workspace, _harness, state) = test_state("legacy-json");
        let response = mcp_get(State(state), HeaderMap::new()).await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn standard_notification_returns_accepted_without_rpc_body() {
        let (_workspace, _harness, state) = test_state("streamable-http");
        let response = mcp_post(
            State(state),
            HeaderMap::new(),
            Json(json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            })),
        )
        .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn real_http_disconnect_cancels_exec_and_releases_session_capacity() {
        let (_workspace, _harness, state) = test_state("streamable-http");
        let context = state.mcp.clone();
        let active_session_limit = context.sessions.active_session_limit();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("test listener address");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, build_router(state)).await;
        });

        #[cfg(windows)]
        let command = "powershell -NoProfile -Command \"Start-Sleep -Seconds 30\"";
        #[cfg(unix)]
        let command = "sh -c \"sleep 30\"";
        let response = tokio::time::timeout(Duration::from_secs(2), async move {
            reqwest::Client::new()
                .post(format!("http://{address}/mcp"))
                .header("mcp-protocol-version", "2025-11-25")
                .json(&json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "exec_command",
                        "arguments": {
                            "cmd": command,
                            "timeout_ms": 60_000,
                            "yield_time_ms": 30_000,
                            "output_mode": "none"
                        }
                    }
                }))
                .send()
                .await
        })
        .await
        .expect("slow tool did not return response headers promptly")
        .expect("slow tool response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-coding-tools-streaming")
                .and_then(|value| value.to_str().ok()),
            Some("1")
        );

        tokio::time::timeout(Duration::from_secs(5), async {
            while context.sessions.list(false, 1).is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("HTTP request did not register an exec session");
        assert_eq!(
            context.sessions.active_slots_available(),
            active_session_limit - 1
        );

        drop(response);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            context.sessions.active_slots_available(),
            active_session_limit - 1,
            "HTTP disconnect should preserve the session during the reconnect grace period"
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            while context.sessions.active_slots_available()
                != context.sessions.active_session_limit()
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("HTTP disconnect did not release exec session capacity");
        server.abort();
    }

    #[tokio::test]
    async fn raw_http_can_discover_and_call_new_tools_without_client_schema_cache() {
        let (_workspace, _harness, state) = test_state("streamable-http");
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("test listener address");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, build_router(state)).await;
        });
        let client = reqwest::Client::new();
        let endpoint = format!("http://{address}/mcp");

        let listed: serde_json::Value = client
            .post(&endpoint)
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": {}
            }))
            .send()
            .await
            .expect("tools/list request")
            .json()
            .await
            .expect("tools/list response");
        let tools = listed["result"]["tools"].as_array().expect("tool list");
        let names = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"exec_many"));
        assert!(names.contains(&"wait_command"));
        assert!(names.contains(&"query_tool_usage"));
        assert!(!names.contains(&"write_stdin"));
        let search = tools
            .iter()
            .find(|tool| tool["name"] == "search_text")
            .expect("search_text schema");
        assert_eq!(
            search["inputSchema"]["properties"]["context_lines"]["maximum"],
            1000
        );

        let executed: serde_json::Value = client
            .post(&endpoint)
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "exec_many",
                    "arguments": {
                        "commands": [{"program": "cargo", "args": ["--version"]}]
                    }
                }
            }))
            .send()
            .await
            .expect("exec_many request")
            .json()
            .await
            .expect("exec_many response");
        assert_eq!(
            executed["result"]["structuredContent"]["command_ok"], true,
            "{executed}"
        );
        assert_eq!(
            executed["result"]["structuredContent"]["commands_executed"], 1,
            "{executed}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn standard_initialize_negotiates_chatgpt_compatible_version() {
        let (_workspace, _harness, state) = test_state("streamable-http");
        let response = mcp_post(
            State(state),
            HeaderMap::new(),
            Json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "1"}
                }
            })),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
    }

    #[tokio::test]
    async fn standard_transport_rejects_bad_version_and_origin() {
        let (_workspace, _harness, state) = test_state("streamable-http");
        let mut headers = HeaderMap::new();
        headers.insert("mcp-protocol-version", "unsupported".parse().unwrap());
        let response = mcp_get(State(state.clone()), headers).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.extensions().get::<TransportDiagnostic>(),
            Some(&TransportDiagnostic {
                status: 400,
                code: -32022
            })
        );

        let mut headers = HeaderMap::new();
        headers.insert("origin", "https://attacker.example".parse().unwrap());
        let response = mcp_get(State(state), headers).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.extensions().get::<TransportDiagnostic>(),
            Some(&TransportDiagnostic {
                status: 403,
                code: -32000
            })
        );
    }

    #[tokio::test]
    async fn standard_transport_accepts_chatgpt_origin() {
        let (_workspace, _harness, state) = test_state("streamable-http");
        let mut headers = HeaderMap::new();
        headers.insert("origin", "https://chatgpt.com".parse().unwrap());

        let response = mcp_get(State(state), headers).await;

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn request_log_keeps_command_and_redacts_sensitive_fields() {
        let value = json!({
            "cmd": "git status",
            "reason": "inspect workspace",
            "api_key": "should-not-be-logged",
            "nested": {
                "access-token": "also-secret"
            }
        });

        let logged = format_log_value(&value);

        assert!(logged.contains("git status"));
        assert!(logged.contains("inspect workspace"));
        assert!(!logged.contains("should-not-be-logged"));
        assert!(!logged.contains("also-secret"));
        assert_eq!(logged.matches("[REDACTED]").count(), 2);
    }

    #[test]
    fn request_log_truncates_multibyte_text_safely() {
        let value = json!({
            "items": vec!["指令內容"; 5_000]
        });

        let logged = format_log_value(&value);

        assert!(logged.ends_with("...[TRUNCATED]"));
        assert!(logged.is_char_boundary(logged.len()));
    }

    #[test]
    fn request_log_redacts_secrets_embedded_in_command_text() {
        let value = json!({
            "cmd": "curl --token super-secret -H 'Authorization: Bearer abc.def.ghi' API_KEY=hidden"
        });

        let logged = format_log_value(&value);

        assert!(logged.contains("--token [REDACTED]"));
        assert!(logged.contains("Authorization: [REDACTED]"));
        assert!(logged.contains("API_KEY=[REDACTED]"));
        assert!(!logged.contains("super-secret"));
        assert!(!logged.contains("abc.def.ghi"));
        assert!(!logged.contains("hidden"));
    }
}
