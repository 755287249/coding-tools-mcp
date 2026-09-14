use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 4;
pub const WS_PATH: &str = "/_tunnel/v1";
pub const ENROLL_PATH_PREFIX: &str = "/_tunnel/enroll";
pub const WS_SUBPROTOCOL: &str = "coding-tools-tunnel-v4";
pub const CLIENT_ID_HEADER: &str = "x-coding-tools-client-id";
pub const SERVICE_HEADER: &str = "x-coding-tools-service";
pub const DEVICE_ID_HEADER: &str = "x-coding-tools-device-id";
pub const WORKER_ID_HEADER: &str = "x-coding-tools-worker-id";
pub const BUILTIN_MCP_PREFIX: &str = "/builtin/clients";
pub const BUILTIN_ACTIONS_PREFIX: &str = "/builtin/actions";
pub const MAX_REQUEST_BODY_BYTES: usize = 8 * 1024 * 1024;

/// MCP tools whose contracts are read-only and therefore safe to replay after a
/// transport failure that occurs before any response headers are received.
///
/// Keep this as the single source of truth for both MCP annotations and tunnel
/// retry decisions. Tools that change workspace, Git, process, routing, task,
/// permission, or history state must never be added here.
pub const MCP_READ_ONLY_TOOL_NAMES: &[&str] = &[
    "list_workspace_folders",
    "harness_status",
    "operation_log",
    "server_info",
    "query_tool_usage",
    "exec_health_check",
    "read_file",
    "read_many",
    "project_map",
    "list_files",
    "search_text",
    "wait_command",
    "resolve_operation",
    "list_sessions",
    "read_output",
    "git_status",
    "git_diff",
    "git_log",
    "git_show",
    "git_blame",
    "view_image",
    "patch_check",
    "project_state",
    "task_context",
    "list_task_events",
    "change_summary",
];

pub fn is_retry_safe_tool_name(name: &str) -> bool {
    MCP_READ_ONLY_TOOL_NAMES.contains(&name.trim())
}

/// Returns true only for JSON-RPC requests that are safe to replay.
/// Notifications and malformed messages are intentionally not retried.
pub fn is_retry_safe_mcp_request(body: &[u8]) -> bool {
    let Ok(message) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    let Some(object) = message.as_object() else {
        return false;
    };
    if object.get("id").is_none() || object.get("id").is_some_and(serde_json::Value::is_null) {
        return false;
    }
    match object.get("method").and_then(serde_json::Value::as_str) {
        Some("initialize" | "ping" | "tools/list") => true,
        Some("tools/call") => object
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_retry_safe_tool_name),
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelService {
    Mcp,
    Actions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerPolicy {
    pub start_workers: u16,
    pub min_idle_workers: u16,
    pub max_idle_workers: u16,
    pub max_workers: u16,
    pub max_requests_per_worker: u64,
    pub max_lifetime_seconds: u64,
    pub scale_down_delay_seconds: u64,
    pub recycle_jitter_percent: u8,
    #[serde(default = "default_max_pending_requests")]
    pub max_pending_requests: u16,
    #[serde(default = "default_worker_acquire_timeout_ms")]
    pub worker_acquire_timeout_ms: u64,
    #[serde(default)]
    pub max_connecting_workers: u16,
    #[serde(default = "default_connecting_capacity_grace_ms")]
    pub connecting_capacity_grace_ms: u64,
    #[serde(default = "default_scale_down_step")]
    pub scale_down_step: u16,
    #[serde(default)]
    pub burst_warm_workers: u16,
    #[serde(default = "default_burst_warm_seconds")]
    pub burst_warm_seconds: u64,
    pub revision: u64,
}

const fn default_max_pending_requests() -> u16 {
    32
}

const fn default_worker_acquire_timeout_ms() -> u64 {
    10_000
}

const fn default_connecting_capacity_grace_ms() -> u64 {
    1_000
}

const fn default_scale_down_step() -> u16 {
    4
}

const fn default_burst_warm_seconds() -> u64 {
    120
}

impl WorkerPolicy {
    pub fn default_for(_service: TunnelService) -> Self {
        Self {
            start_workers: 4,
            min_idle_workers: 2,
            max_idle_workers: 4,
            max_workers: 16,
            max_requests_per_worker: 500,
            max_lifetime_seconds: 3_600,
            scale_down_delay_seconds: 60,
            recycle_jitter_percent: 10,
            max_pending_requests: default_max_pending_requests(),
            worker_acquire_timeout_ms: default_worker_acquire_timeout_ms(),
            max_connecting_workers: 0,
            connecting_capacity_grace_ms: default_connecting_capacity_grace_ms(),
            scale_down_step: default_scale_down_step(),
            burst_warm_workers: 0,
            burst_warm_seconds: default_burst_warm_seconds(),
            revision: 1,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.min_idle_workers == 0
            || self.min_idle_workers > self.start_workers
            || self.start_workers > self.max_idle_workers
            || self.max_idle_workers > self.max_workers
            || self.max_workers > 256
        {
            return Err(
                "worker counts must satisfy 1 <= min idle <= start <= max idle <= max workers <= 256"
                    .into(),
            );
        }
        if self.max_requests_per_worker > 1_000_000 {
            return Err("max requests per worker must be between 0 and 1000000".into());
        }
        if self.max_lifetime_seconds != 0
            && !(60..=7 * 24 * 60 * 60).contains(&self.max_lifetime_seconds)
        {
            return Err("max lifetime must be 0 or between 60 and 604800 seconds".into());
        }
        if self.scale_down_delay_seconds > 3_600 {
            return Err("scale-down delay must be between 0 and 3600 seconds".into());
        }
        if self.recycle_jitter_percent > 50 {
            return Err("recycle jitter must be between 0 and 50 percent".into());
        }
        if self.max_pending_requests == 0 || self.max_pending_requests > 4_096 {
            return Err("max pending requests must be between 1 and 4096".into());
        }
        if !(100..=60_000).contains(&self.worker_acquire_timeout_ms) {
            return Err("worker acquire timeout must be between 100 and 60000 milliseconds".into());
        }
        if self.max_connecting_workers > self.max_workers {
            return Err(
                "max connecting workers must be 0 (automatic) or no greater than max workers"
                    .into(),
            );
        }
        if self.connecting_capacity_grace_ms > 30_000 {
            return Err(
                "connecting capacity grace must be between 0 and 30000 milliseconds".into(),
            );
        }
        if self.scale_down_step == 0 || self.scale_down_step > 256 {
            return Err("scale-down step must be between 1 and 256".into());
        }
        if self.burst_warm_workers != 0
            && (self.burst_warm_workers < self.max_idle_workers
                || self.burst_warm_workers > self.max_workers)
        {
            return Err(
                "burst warm workers must be 0 (automatic) or between max idle and max workers"
                    .into(),
            );
        }
        if self.burst_warm_seconds > 3_600 {
            return Err("burst warm duration must be between 0 and 3600 seconds".into());
        }
        if self.revision == 0 {
            return Err("worker policy revision must be positive".into());
        }
        Ok(())
    }
}

impl TunnelService {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Actions => "actions",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mcp" => Some(Self::Mcp),
            "actions" => Some(Self::Actions),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderPair {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    pub protocol_version: u16,
    pub client_id: String,
    pub service: TunnelService,
    pub worker_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAuthProof {
    pub hello: ClientHello,
    pub device_id: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollmentRequest {
    pub device_id: String,
    pub client_id: String,
    pub device_name: String,
    pub public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollmentResponse {
    pub device_id: String,
    #[serde(default)]
    pub client_id: String,
    pub server_id: String,
    pub server_public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerDemand {
    pub queued_requests: u16,
    pub oldest_queue_wait_ms: u64,
    pub desired_workers: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeUpdateOffer {
    pub update_id: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeUpdateState {
    Downloading,
    Verified,
    Draining,
    Scheduled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlMessage {
    Challenge {
        nonce: String,
        expires_at_unix_ms: u64,
        server_id: String,
        server_signature: String,
    },
    Authenticate(DeviceAuthProof),
    HelloAck {
        protocol_version: u16,
        worker_policy: WorkerPolicy,
        server_id: String,
        server_signature: String,
    },
    PolicyUpdate {
        worker_policy: WorkerPolicy,
    },
    ClientStatus {
        agent_version: String,
        client_compat_version: String,
        build_git_sha: String,
    },
    UpdateOffer(NodeUpdateOffer),
    UpdateStatus {
        update_id: String,
        version: String,
        state: NodeUpdateState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Ready,
    RequestHead {
        request_id: String,
        method: String,
        path_and_query: String,
        headers: Vec<HeaderPair>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        demand: Option<WorkerDemand>,
    },
    RequestEnd {
        request_id: String,
    },
    ResponseHead {
        request_id: String,
        status: u16,
        headers: Vec<HeaderPair>,
    },
    ResponseEnd {
        request_id: String,
    },
    Cancel {
        request_id: String,
    },
    Error {
        request_id: Option<String>,
        message: String,
    },
}

#[derive(Serialize)]
struct AuthSigningPayload<'a> {
    protocol_version: u16,
    nonce: &'a str,
    device_id: &'a str,
    client_id: &'a str,
    service: TunnelService,
    worker_id: &'a str,
}

pub fn auth_signing_payload(nonce: &str, proof: &DeviceAuthProof) -> Vec<u8> {
    serde_json::to_vec(&AuthSigningPayload {
        protocol_version: proof.hello.protocol_version,
        nonce,
        device_id: &proof.device_id,
        client_id: &proof.hello.client_id,
        service: proof.hello.service,
        worker_id: &proof.hello.worker_id,
    })
    .expect("auth signing payload is serializable")
}

#[derive(Serialize)]
struct ServerChallengeSigningPayload<'a> {
    protocol_version: u16,
    nonce: &'a str,
    expires_at_unix_ms: u64,
    server_id: &'a str,
    device_id: &'a str,
    client_id: &'a str,
    service: TunnelService,
    worker_id: &'a str,
}

pub fn server_challenge_signing_payload(
    nonce: &str,
    expires_at_unix_ms: u64,
    server_id: &str,
    device_id: &str,
    client_id: &str,
    service: TunnelService,
    worker_id: &str,
) -> Vec<u8> {
    serde_json::to_vec(&ServerChallengeSigningPayload {
        protocol_version: PROTOCOL_VERSION,
        nonce,
        expires_at_unix_ms,
        server_id,
        device_id,
        client_id,
        service,
        worker_id,
    })
    .expect("server challenge signing payload is serializable")
}

#[derive(Serialize)]
struct ServerAckSigningPayload<'a> {
    protocol_version: u16,
    nonce: &'a str,
    server_id: &'a str,
    device_id: &'a str,
    client_id: &'a str,
    service: TunnelService,
    worker_id: &'a str,
    worker_policy: &'a WorkerPolicy,
}

pub fn server_ack_signing_payload(
    nonce: &str,
    server_id: &str,
    proof: &DeviceAuthProof,
    worker_policy: &WorkerPolicy,
) -> Vec<u8> {
    serde_json::to_vec(&ServerAckSigningPayload {
        protocol_version: PROTOCOL_VERSION,
        nonce,
        server_id,
        device_id: &proof.device_id,
        client_id: &proof.hello.client_id,
        service: proof.hello.service,
        worker_id: &proof.hello.worker_id,
        worker_policy,
    })
    .expect("server ack signing payload is serializable")
}

pub fn valid_client_id(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub fn expected_routes(client_id: &str, service: TunnelService) -> Vec<String> {
    match service {
        TunnelService::Mcp => {
            let route_prefix = format!("{BUILTIN_MCP_PREFIX}/{client_id}");
            vec![
                route_prefix.clone(),
                format!("/.well-known/oauth-authorization-server{route_prefix}"),
                format!("/.well-known/oauth-protected-resource{route_prefix}/mcp"),
            ]
        }
        TunnelService::Actions => vec![format!("{BUILTIN_ACTIONS_PREFIX}/{client_id}")],
    }
}

pub fn route_matches(prefix: &str, path: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "content-length"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ids_are_path_safe() {
        assert!(valid_client_id("pc-a_01"));
        assert!(!valid_client_id(""));
        assert!(!valid_client_id("../pc"));
        assert!(!valid_client_id("pc/a"));
    }

    #[test]
    fn mcp_routes_include_standard_well_known_paths() {
        assert_eq!(
            expected_routes("pc-a", TunnelService::Mcp),
            vec![
                "/builtin/clients/pc-a",
                "/.well-known/oauth-authorization-server/builtin/clients/pc-a",
                "/.well-known/oauth-protected-resource/builtin/clients/pc-a/mcp",
            ]
        );
    }

    #[test]
    fn route_matching_respects_segment_boundaries() {
        assert!(route_matches(
            "/builtin/actions/pc-a",
            "/builtin/actions/pc-a/openapi.json"
        ));
        assert!(!route_matches(
            "/builtin/actions/pc-a",
            "/builtin/actions/pc-ab"
        ));
    }

    #[test]
    fn control_messages_round_trip() {
        let message = ControlMessage::Authenticate(DeviceAuthProof {
            hello: ClientHello {
                protocol_version: PROTOCOL_VERSION,
                client_id: "pc-a".into(),
                service: TunnelService::Mcp,
                worker_id: "worker-1".into(),
            },
            device_id: "device-1".into(),
            signature: "signature".into(),
        });
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<ControlMessage>(&encoded).unwrap(),
            message
        );

        for message in [
            ControlMessage::ClientStatus {
                agent_version: "0.29.33".into(),
                client_compat_version: "0.1.58".into(),
                build_git_sha: "0123456789012345678901234567890123456789".into(),
            },
            ControlMessage::UpdateOffer(NodeUpdateOffer {
                update_id: "update-1".into(),
                version: "0.29.34".into(),
                url: "https://updates.example.test/ctnode.zip".into(),
                sha256: "a".repeat(64),
                signature: "signature".into(),
            }),
            ControlMessage::UpdateStatus {
                update_id: "update-1".into(),
                version: "0.29.34".into(),
                state: NodeUpdateState::Verified,
                message: None,
            },
        ] {
            let encoded = serde_json::to_string(&message).unwrap();
            assert_eq!(
                serde_json::from_str::<ControlMessage>(&encoded).unwrap(),
                message
            );
        }
    }

    #[test]
    fn signing_payload_is_stable_and_binds_the_worker_identity() {
        let proof = DeviceAuthProof {
            hello: ClientHello {
                protocol_version: PROTOCOL_VERSION,
                client_id: "pc-a".into(),
                service: TunnelService::Mcp,
                worker_id: "worker-1".into(),
            },
            device_id: "device-1".into(),
            signature: String::new(),
        };
        assert_eq!(
            String::from_utf8(auth_signing_payload("nonce-1", &proof)).unwrap(),
            r#"{"protocol_version":4,"nonce":"nonce-1","device_id":"device-1","client_id":"pc-a","service":"mcp","worker_id":"worker-1"}"#
        );
    }

    #[test]
    fn protocol_v4_requires_mutual_server_and_device_authentication() {
        assert_eq!(PROTOCOL_VERSION, 4);
        assert_eq!(WS_SUBPROTOCOL, "coding-tools-tunnel-v4");
        assert_eq!(DEVICE_ID_HEADER, "x-coding-tools-device-id");
        assert_eq!(WORKER_ID_HEADER, "x-coding-tools-worker-id");

        let challenge_payload = server_challenge_signing_payload(
            "nonce-1",
            1234,
            "server-1",
            "device-1",
            "pc-a",
            TunnelService::Mcp,
            "worker-1",
        );
        assert_eq!(
            String::from_utf8(challenge_payload).unwrap(),
            r#"{"protocol_version":4,"nonce":"nonce-1","expires_at_unix_ms":1234,"server_id":"server-1","device_id":"device-1","client_id":"pc-a","service":"mcp","worker_id":"worker-1"}"#
        );

        let policy = WorkerPolicy::default_for(TunnelService::Mcp);
        assert_eq!(policy.start_workers, 4);
        assert_eq!(policy.min_idle_workers, 2);
        assert_eq!(policy.max_idle_workers, 4);
        assert_eq!(policy.max_workers, 16);
        assert_eq!(policy.max_requests_per_worker, 500);
        assert_eq!(policy.max_lifetime_seconds, 3_600);
        assert_eq!(policy.scale_down_delay_seconds, 60);
        assert_eq!(policy.recycle_jitter_percent, 10);
        assert_eq!(policy.max_pending_requests, 32);
        assert_eq!(policy.worker_acquire_timeout_ms, 10_000);
        assert_eq!(policy.max_connecting_workers, 0);
        assert_eq!(policy.connecting_capacity_grace_ms, 1_000);
        assert_eq!(policy.scale_down_step, 4);
        assert_eq!(policy.burst_warm_workers, 0);
        assert_eq!(policy.burst_warm_seconds, 120);
        assert_eq!(policy.revision, 1);

        let message = ControlMessage::HelloAck {
            protocol_version: PROTOCOL_VERSION,
            worker_policy: policy.clone(),
            server_id: "server-1".into(),
            server_signature: "server-signature".into(),
        };
        let encoded = serde_json::to_string(&message).expect("policy hello ack");
        assert_eq!(
            serde_json::from_str::<ControlMessage>(&encoded).unwrap(),
            message
        );
        assert!(matches!(
            ControlMessage::PolicyUpdate {
                worker_policy: policy,
            },
            ControlMessage::PolicyUpdate { .. }
        ));
    }

    #[test]
    fn older_worker_policy_json_receives_capacity_defaults() {
        let policy: WorkerPolicy = serde_json::from_value(serde_json::json!({
            "start_workers": 1,
            "min_idle_workers": 1,
            "max_idle_workers": 1,
            "max_workers": 1,
            "max_requests_per_worker": 500,
            "max_lifetime_seconds": 3600,
            "scale_down_delay_seconds": 60,
            "recycle_jitter_percent": 10,
            "revision": 1
        }))
        .expect("legacy policy");
        assert_eq!(policy.max_pending_requests, 32);
        assert_eq!(policy.worker_acquire_timeout_ms, 10_000);
        assert_eq!(policy.max_connecting_workers, 0);
        assert_eq!(policy.burst_warm_workers, 0);
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn worker_policy_rejects_invalid_fpm_relationships_and_bounds() {
        let valid = WorkerPolicy::default_for(TunnelService::Actions);
        assert!(valid.validate().is_ok());

        let mut invalid = valid.clone();
        invalid.min_idle_workers = invalid.start_workers + 1;
        assert!(invalid.validate().is_err());

        invalid = valid.clone();
        invalid.max_idle_workers = invalid.max_workers + 1;
        assert!(invalid.validate().is_err());

        invalid = valid;
        invalid.recycle_jitter_percent = 51;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn retry_contract_accepts_only_read_only_mcp_requests() {
        let read = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "read_file", "arguments": {"path": "README.md"}}
        });
        let write = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "apply_patch", "arguments": {"patch": "test"}}
        });
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {"name": "read_file", "arguments": {"path": "README.md"}}
        });

        assert!(is_retry_safe_mcp_request(
            &serde_json::to_vec(&read).unwrap()
        ));
        assert!(!is_retry_safe_mcp_request(
            &serde_json::to_vec(&write).unwrap()
        ));
        assert!(!is_retry_safe_mcp_request(
            &serde_json::to_vec(&notification).unwrap()
        ));
        assert!(!is_retry_safe_mcp_request(b"not-json"));
    }

    #[test]
    fn retry_contract_accepts_safe_protocol_requests() {
        for method in ["initialize", "ping", "tools/list"] {
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": method,
                "method": method,
                "params": {}
            });
            assert!(is_retry_safe_mcp_request(
                &serde_json::to_vec(&request).unwrap()
            ));
        }
    }
}
