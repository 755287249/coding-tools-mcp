use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::location::ExecutionTarget;
use super::model::{
    default_oauth_token_ttl_seconds, ActionsConfig, AuthConfig, RuntimeConfig, SandboxConfig,
    SandboxPathAccess, SandboxPathGrant, SecurityPolicy, TunnelConfig, WorkspaceFolder,
    WorkspaceProfile,
};

pub const CANONICAL_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalFolder {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalBind {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalAuth {
    #[serde(rename = "type")]
    pub auth_type: String,
    pub oauth_client_id: String,
    #[serde(default = "default_oauth_token_ttl_seconds")]
    pub oauth_token_ttl_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalPolicy {
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    #[serde(default = "default_true")]
    pub workspace_local_entries: bool,
    #[serde(default)]
    pub workspace_script_extensions: Vec<String>,
    #[serde(default = "default_max_patch_bytes")]
    pub max_patch_bytes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalSandboxPath {
    pub path: String,
    #[serde(default = "default_read_only")]
    pub access: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalSandbox {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_sandbox_backend")]
    pub backend: String,
    #[serde(default)]
    pub external_paths: Vec<CanonicalSandboxPath>,
    #[serde(default)]
    pub options: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalLimits {
    #[serde(default = "default_blocking")]
    pub blocking_concurrency: u16,
    #[serde(default = "default_process")]
    pub process_concurrency: u16,
    #[serde(default = "default_global_blocking")]
    pub global_blocking_concurrency: u16,
    #[serde(default = "default_global_process")]
    pub global_process_concurrency: u16,
    #[serde(default = "default_sessions")]
    pub active_session_limit: u16,
    #[serde(default = "default_output")]
    pub max_output_bytes: u32,
    #[serde(default)]
    pub command_timeout_max_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalBuiltinTunnel {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub public_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalTunnel {
    #[serde(default)]
    pub builtin: CanonicalBuiltinTunnel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalToggle {
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub enabled: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalExtensions {
    #[serde(default)]
    pub hooks: CanonicalToggle,
    #[serde(default)]
    pub mcp: CanonicalToggle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalHost {
    #[serde(default)]
    pub desktop: Value,
    #[serde(default)]
    pub node: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalWorkspace {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub folders: Vec<CanonicalFolder>,
    #[serde(default)]
    pub active_folder_id: String,
    pub bind: CanonicalBind,
    #[serde(default)]
    pub public_base_url: String,
    pub auth: CanonicalAuth,
    #[serde(default = "default_tool_profile")]
    pub tool_profile: String,
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    #[serde(default)]
    pub security_policy: Map<String, Value>,
    #[serde(default)]
    pub policy: CanonicalPolicy,
    #[serde(default)]
    pub sandbox: CanonicalSandbox,
    #[serde(default)]
    pub limits: CanonicalLimits,
    #[serde(default)]
    pub tunnel: CanonicalTunnel,
    #[serde(default)]
    pub skills: CanonicalToggle,
    #[serde(default)]
    pub extensions: CanonicalExtensions,
    #[serde(default)]
    pub host: CanonicalHost,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn default_true() -> bool {
    true
}
fn default_max_patch_bytes() -> u32 {
    1024 * 1024
}
fn default_read_only() -> String {
    "read_only".into()
}
fn default_sandbox_backend() -> String {
    "appcontainer".into()
}
fn default_blocking() -> u16 {
    128
}
fn default_process() -> u16 {
    64
}
fn default_global_blocking() -> u16 {
    1024
}
fn default_global_process() -> u16 {
    512
}
fn default_sessions() -> u16 {
    512
}
fn default_output() -> u32 {
    1024 * 1024
}
fn default_tool_profile() -> String {
    "core".into()
}
fn default_permission_mode() -> String {
    "trusted".into()
}

impl Default for CanonicalPolicy {
    fn default() -> Self {
        Self {
            allowed_commands: Vec::new(),
            workspace_local_entries: true,
            workspace_script_extensions: Vec::new(),
            max_patch_bytes: default_max_patch_bytes(),
        }
    }
}

impl Default for CanonicalSandbox {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: default_sandbox_backend(),
            external_paths: Vec::new(),
            options: Map::new(),
        }
    }
}

impl Default for CanonicalLimits {
    fn default() -> Self {
        Self {
            blocking_concurrency: default_blocking(),
            process_concurrency: default_process(),
            global_blocking_concurrency: default_global_blocking(),
            global_process_concurrency: default_global_process(),
            active_session_limit: default_sessions(),
            max_output_bytes: default_output(),
            command_timeout_max_ms: 0,
        }
    }
}

impl Default for CanonicalBuiltinTunnel {
    fn default() -> Self {
        Self {
            enabled: true,
            public_url: String::new(),
        }
    }
}

impl Default for CanonicalTunnel {
    fn default() -> Self {
        Self {
            builtin: CanonicalBuiltinTunnel::default(),
        }
    }
}

impl Default for CanonicalToggle {
    fn default() -> Self {
        Self {
            active: true,
            disabled: Vec::new(),
            enabled: Vec::new(),
        }
    }
}

impl Default for CanonicalExtensions {
    fn default() -> Self {
        Self {
            hooks: CanonicalToggle::default(),
            mcp: CanonicalToggle::default(),
        }
    }
}

const SECRET_KEYS: &[&str] = &[
    "password",
    "clientSecret",
    "tokenSecret",
    "enrollmentUrl",
    "cloudflareToken",
    "oauthPassword",
    "oauthClientSecret",
    "oauthTokenSecret",
    "tunnelEnrollmentUrl",
];

fn strip_secrets(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(strip_secrets).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, entry) in map {
                if SECRET_KEYS.contains(&key.as_str()) {
                    continue;
                }
                out.insert(key, strip_secrets(entry));
            }
            Value::Object(out)
        }
        other => other,
    }
}

fn ensure_folders(document: &CanonicalWorkspace) -> Result<(), String> {
    if document.schema_version != CANONICAL_SCHEMA_VERSION {
        return Err(format!(
            "Unsupported workspace schemaVersion {}",
            document.schema_version
        ));
    }
    if document.folders.is_empty() {
        return Err("workspace document must contain at least one folder".into());
    }
    Ok(())
}

pub fn parse_canonical_workspace(value: &Value) -> Result<CanonicalWorkspace, String> {
    let stripped = strip_secrets(value.clone());
    let document: CanonicalWorkspace =
        serde_json::from_value(stripped).map_err(|error| error.to_string())?;
    ensure_folders(&document)?;
    Ok(document)
}

pub fn serialize_canonical_workspace(document: &CanonicalWorkspace) -> Result<Value, String> {
    let value = serde_json::to_value(document).map_err(|error| error.to_string())?;
    Ok(strip_secrets(value))
}

pub fn looks_like_canonical_workspace(value: &Value) -> bool {
    let schema = value
        .get("schemaVersion")
        .or_else(|| value.get("schema_version"))
        .and_then(Value::as_u64);
    if schema == Some(CANONICAL_SCHEMA_VERSION as u64) && value.get("schemaVersion").is_some() {
        return true;
    }
    schema == Some(CANONICAL_SCHEMA_VERSION as u64)
        && (value.get("bind").is_some()
            || value.get("auth").is_some()
            || value.get("host").is_some())
}

pub fn decode_stored_profile(value: &Value) -> Result<WorkspaceProfile, String> {
    if looks_like_canonical_workspace(value) {
        return Ok(desktop_profile_from_canonical(&parse_canonical_workspace(
            value,
        )?));
    }
    serde_json::from_value(value.clone()).map_err(|error| error.to_string())
}

pub fn encode_stored_profile(profile: &WorkspaceProfile) -> Result<Value, String> {
    serialize_canonical_workspace(&canonical_from_desktop_profile(profile))
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn desktop_map(document: &CanonicalWorkspace) -> Map<String, Value> {
    document
        .host
        .desktop
        .as_object()
        .cloned()
        .unwrap_or_default()
}

fn map_str(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn map_bool(map: &Map<String, Value>, key: &str, fallback: bool) -> bool {
    map.get(key).and_then(Value::as_bool).unwrap_or(fallback)
}

fn map_u64(map: &Map<String, Value>, key: &str, fallback: u64) -> u64 {
    map.get(key).and_then(Value::as_u64).unwrap_or(fallback)
}

fn policy_from_map(map: &Map<String, Value>) -> SecurityPolicy {
    let defaults = SecurityPolicy::default();
    let flag =
        |key: &str, fallback: bool| map.get(key).and_then(Value::as_bool).unwrap_or(fallback);
    SecurityPolicy {
        restrict_tool_catalog: flag("restrictToolCatalog", defaults.restrict_tool_catalog),
        enforce_command_allowlist: flag(
            "enforceCommandAllowlist",
            defaults.enforce_command_allowlist,
        ),
        require_dangerous_confirmation: flag(
            "requireDangerousConfirmation",
            defaults.require_dangerous_confirmation,
        ),
        require_shell_confirmation: flag(
            "requireShellConfirmation",
            defaults.require_shell_confirmation,
        ),
        block_network_commands: flag("blockNetworkCommands", defaults.block_network_commands),
        enforce_workspace_boundary: flag(
            "enforceWorkspaceBoundary",
            defaults.enforce_workspace_boundary,
        ),
        protect_repository_metadata: flag(
            "protectRepositoryMetadata",
            defaults.protect_repository_metadata,
        ),
        block_symlink_escape: flag("blockSymlinkEscape", defaults.block_symlink_escape),
        protect_environment_variables: flag(
            "protectEnvironmentVariables",
            defaults.protect_environment_variables,
        ),
        enforce_harness_baseline: flag("enforceHarnessBaseline", defaults.enforce_harness_baseline),
        require_write_confirmation: flag(
            "requireWriteConfirmation",
            defaults.require_write_confirmation,
        ),
        verify_write_conflicts: flag("verifyWriteConflicts", defaults.verify_write_conflicts),
        enforce_resource_limits: flag("enforceResourceLimits", defaults.enforce_resource_limits),
        redact_sensitive_output: flag("redactSensitiveOutput", defaults.redact_sensitive_output),
        withhold_sensitive_source_output: flag(
            "withholdSensitiveSourceOutput",
            defaults.withhold_sensitive_source_output,
        ),
        redact_telemetry: flag("redactTelemetry", defaults.redact_telemetry),
        redact_history: flag("redactHistory", defaults.redact_history),
    }
}

fn folder_from_canonical(folder: &CanonicalFolder) -> WorkspaceFolder {
    WorkspaceFolder {
        id: folder.id.clone(),
        name: folder.name.clone(),
        path: folder.path.clone(),
        execution: folder
            .extra
            .get("execution")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
    }
}

fn policy_to_map(policy: &SecurityPolicy) -> Map<String, Value> {
    let mut map = Map::new();
    let fields = [
        ("restrictToolCatalog", policy.restrict_tool_catalog),
        ("enforceCommandAllowlist", policy.enforce_command_allowlist),
        (
            "requireDangerousConfirmation",
            policy.require_dangerous_confirmation,
        ),
        (
            "requireShellConfirmation",
            policy.require_shell_confirmation,
        ),
        ("blockNetworkCommands", policy.block_network_commands),
        (
            "enforceWorkspaceBoundary",
            policy.enforce_workspace_boundary,
        ),
        (
            "protectRepositoryMetadata",
            policy.protect_repository_metadata,
        ),
        ("blockSymlinkEscape", policy.block_symlink_escape),
        (
            "protectEnvironmentVariables",
            policy.protect_environment_variables,
        ),
        ("enforceHarnessBaseline", policy.enforce_harness_baseline),
        (
            "requireWriteConfirmation",
            policy.require_write_confirmation,
        ),
        ("verifyWriteConflicts", policy.verify_write_conflicts),
        ("enforceResourceLimits", policy.enforce_resource_limits),
        ("redactSensitiveOutput", policy.redact_sensitive_output),
        (
            "withholdSensitiveSourceOutput",
            policy.withhold_sensitive_source_output,
        ),
        ("redactTelemetry", policy.redact_telemetry),
        ("redactHistory", policy.redact_history),
    ];
    for (key, value) in fields {
        map.insert(key.to_string(), Value::Bool(value));
    }
    map
}

pub fn canonical_from_desktop_profile(profile: &WorkspaceProfile) -> CanonicalWorkspace {
    let folders = if profile.folders.is_empty() {
        vec![CanonicalFolder {
            id: "legacy".into(),
            name: profile.name.clone(),
            path: profile.path.clone(),
            extra: Map::new(),
        }]
    } else {
        profile
            .folders
            .iter()
            .map(|folder| {
                let mut extra = Map::new();
                if folder.execution != ExecutionTarget::default() {
                    if let Ok(value) = serde_json::to_value(&folder.execution) {
                        extra.insert("execution".into(), value);
                    }
                }
                CanonicalFolder {
                    id: folder.id.clone(),
                    name: folder.name.clone(),
                    path: folder.path.clone(),
                    extra,
                }
            })
            .collect()
    };
    let builtin = profile.tunnel.tunnel_type == "builtin";
    let mut desktop = Map::new();
    if profile.auth.auth_type != "oauth" {
        desktop.insert(
            "authType".into(),
            Value::String(profile.auth.auth_type.clone()),
        );
    }
    if profile.auth.use_shared_secrets {
        desktop.insert("useSharedSecrets".into(), Value::Bool(true));
    }
    if profile.runtime.transport_mode != "streamable-http" {
        desktop.insert(
            "transportMode".into(),
            Value::String(profile.runtime.transport_mode.clone()),
        );
    }
    if !profile.runtime.runtime_command.is_empty() {
        desktop.insert(
            "runtimeCommand".into(),
            Value::String(profile.runtime.runtime_command.clone()),
        );
    }
    if profile.tunnel.tunnel_type != "builtin" && profile.tunnel.tunnel_type != "none" {
        desktop.insert(
            "tunnel".into(),
            serde_json::json!({
                "type": profile.tunnel.tunnel_type,
                "frpServer": profile.tunnel.frp_server,
                "frpSubdomain": profile.tunnel.frp_subdomain,
                "frpProfileId": profile.tunnel.frp_profile_id,
                "frpServerPort": profile.tunnel.frp_server_port,
                "cloudflareMode": profile.tunnel.cloudflare_mode,
                "publicUrl": profile.tunnel.public_url,
                "useProxy": profile.tunnel.use_proxy,
            }),
        );
    } else if !profile.tunnel.use_proxy {
        desktop.insert("useProxy".into(), Value::Bool(false));
    }
    if let Ok(actions) = serde_json::to_value(&profile.actions) {
        desktop.insert("actions".into(), strip_secrets(actions));
    }
    CanonicalWorkspace {
        schema_version: CANONICAL_SCHEMA_VERSION,
        id: profile.id.clone(),
        name: profile.name.clone(),
        active_folder_id: if profile.active_folder_id.is_empty() {
            folders
                .first()
                .map(|folder| folder.id.clone())
                .unwrap_or_default()
        } else {
            profile.active_folder_id.clone()
        },
        folders,
        bind: CanonicalBind {
            host: if profile.runtime.bind_address.is_empty() {
                "127.0.0.1".into()
            } else {
                profile.runtime.bind_address.clone()
            },
            port: profile.runtime.local_port,
        },
        public_base_url: if builtin {
            profile
                .tunnel
                .public_url
                .trim_end_matches('/')
                .trim_end_matches("/mcp")
                .to_string()
        } else {
            String::new()
        },
        auth: CanonicalAuth {
            auth_type: "oauth".into(),
            oauth_client_id: profile.auth.oauth_client_id.clone(),
            oauth_token_ttl_seconds: profile.auth.oauth_token_ttl_seconds,
        },
        tool_profile: profile.runtime.tool_profile.clone(),
        permission_mode: profile.runtime.permission_mode.clone(),
        security_policy: profile
            .runtime
            .security_policy
            .as_ref()
            .map(policy_to_map)
            .unwrap_or_default(),
        policy: CanonicalPolicy {
            allowed_commands: split_csv(&profile.runtime.allowed_commands),
            workspace_local_entries: profile.runtime.workspace_local_entries,
            workspace_script_extensions: split_csv(&profile.runtime.workspace_script_extensions),
            max_patch_bytes: profile.actions.max_patch_bytes,
        },
        sandbox: CanonicalSandbox {
            enabled: profile.runtime.sandbox.enabled,
            backend: profile.runtime.sandbox.backend.clone(),
            external_paths: profile
                .runtime
                .sandbox
                .external_paths
                .iter()
                .map(|entry| CanonicalSandboxPath {
                    path: entry.path.clone(),
                    access: match entry.access {
                        SandboxPathAccess::Modify => "modify".into(),
                        SandboxPathAccess::ReadOnly => "read_only".into(),
                    },
                })
                .collect(),
            options: profile
                .runtime
                .sandbox
                .options
                .iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect(),
        },
        limits: CanonicalLimits {
            blocking_concurrency: profile.runtime.blocking_admission_limit,
            process_concurrency: profile.runtime.process_admission_limit,
            global_blocking_concurrency: profile.runtime.global_blocking_admission_limit,
            global_process_concurrency: profile.runtime.global_process_admission_limit,
            active_session_limit: profile.runtime.active_session_limit,
            max_output_bytes: default_output(),
            command_timeout_max_ms: 0,
        },
        tunnel: CanonicalTunnel {
            builtin: CanonicalBuiltinTunnel {
                enabled: builtin,
                public_url: if builtin {
                    profile.tunnel.public_url.clone()
                } else {
                    String::new()
                },
            },
        },
        skills: CanonicalToggle::default(),
        extensions: CanonicalExtensions::default(),
        host: CanonicalHost {
            desktop: Value::Object(desktop),
            node: Value::Object(Map::new()),
        },
        extra: Map::new(),
    }
}

pub fn canonical_from_node_v1(
    value: &Value,
    id: &str,
    name: Option<&str>,
) -> Result<CanonicalWorkspace, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "Node v1 config must be an object".to_string())?;
    let folders = object
        .get("folders")
        .cloned()
        .unwrap_or(Value::Array(vec![]));
    let oauth = object
        .get("oauth")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let tunnel = object
        .get("tunnel")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let management = object
        .get("management")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let canonical = serde_json::json!({
        "schemaVersion": CANONICAL_SCHEMA_VERSION,
        "id": id,
        "name": name.unwrap_or("Workspace"),
        "folders": folders,
        "bind": {
            "host": object.get("host").and_then(Value::as_str).unwrap_or("127.0.0.1"),
            "port": object.get("port").and_then(Value::as_u64).unwrap_or(3789)
        },
        "publicBaseUrl": object.get("publicBaseUrl").and_then(Value::as_str).unwrap_or(""),
        "auth": {
            "type": "oauth",
            "oauthClientId": oauth.get("clientId").and_then(Value::as_str).unwrap_or(""),
            "oauthTokenTtlSeconds": oauth.get("tokenTtlSeconds").and_then(Value::as_u64).unwrap_or_else(default_oauth_token_ttl_seconds)
        },
        "toolProfile": object.get("toolProfile").and_then(Value::as_str).unwrap_or("core"),
        "permissionMode": object.get("permissionMode").and_then(Value::as_str).unwrap_or("trusted"),
        "securityPolicy": object.get("securityPolicy").cloned().unwrap_or(Value::Object(Map::new())),
        "policy": object.get("policy").cloned().unwrap_or(Value::Object(Map::new())),
        "sandbox": object.get("sandbox").cloned().unwrap_or(Value::Object(Map::new())),
        "limits": object.get("limits").cloned().unwrap_or(Value::Object(Map::new())),
        "tunnel": {
            "builtin": {
                "enabled": tunnel.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                "publicUrl": tunnel.get("publicUrl").and_then(Value::as_str).unwrap_or("")
            }
        },
        "skills": object.get("skills").cloned().unwrap_or(Value::Object(Map::new())),
        "extensions": object.get("extensions").cloned().unwrap_or(Value::Object(Map::new())),
        "host": {
            "node": {
                "dataDir": object.get("dataDir").and_then(Value::as_str).unwrap_or(""),
                "management": {
                    "enabled": management.get("enabled").and_then(Value::as_bool).unwrap_or(true)
                }
            }
        }
    });
    parse_canonical_workspace(&canonical)
}

pub fn desktop_profile_from_canonical(document: &CanonicalWorkspace) -> WorkspaceProfile {
    let folders: Vec<WorkspaceFolder> =
        document.folders.iter().map(folder_from_canonical).collect();
    let path = folders
        .first()
        .map(|folder| folder.path.clone())
        .unwrap_or_default();
    let desktop = desktop_map(document);
    let desktop_tunnel = desktop
        .get("tunnel")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let tunnel_type = if document.tunnel.builtin.enabled {
        "builtin".to_string()
    } else {
        map_str(&desktop_tunnel, "type").unwrap_or_else(|| "none".into())
    };
    let public_url = if document.tunnel.builtin.enabled {
        document.tunnel.builtin.public_url.clone()
    } else {
        map_str(&desktop_tunnel, "publicUrl").unwrap_or_default()
    };
    let use_proxy = if desktop_tunnel.contains_key("useProxy") {
        map_bool(&desktop_tunnel, "useProxy", true)
    } else {
        map_bool(&desktop, "useProxy", true)
    };
    let mut actions = desktop
        .get("actions")
        .cloned()
        .and_then(|value| serde_json::from_value::<ActionsConfig>(strip_secrets(value)).ok())
        .unwrap_or(ActionsConfig {
            public_url: String::new(),
            tunnel_type: "none".into(),
            frp_server: String::new(),
            frp_subdomain: String::new(),
            frp_profile_id: String::new(),
            frp_server_port: 443,
            cloudflare_mode: "quick".into(),
            cloudflare_token: String::new(),
            use_proxy: true,
            local_port: document.bind.port.saturating_add(1),
            bind_address: document.bind.host.clone(),
            permission_mode: document.permission_mode.clone(),
            runtime_command: String::new(),
            auth_type: "oauth".into(),
            oauth_client_id: document.auth.oauth_client_id.clone(),
            oauth_scopes: String::new(),
            allowed_commands: document.policy.allowed_commands.join(","),
            max_patch_bytes: document.policy.max_patch_bytes,
            use_shared_secrets: false,
        });
    actions.max_patch_bytes = document.policy.max_patch_bytes;
    actions.cloudflare_token.clear();
    WorkspaceProfile {
        id: document.id.clone(),
        name: document.name.clone(),
        path,
        folders,
        active_folder_id: document.active_folder_id.clone(),
        tunnel: TunnelConfig {
            tunnel_type,
            public_url,
            frp_server: map_str(&desktop_tunnel, "frpServer").unwrap_or_default(),
            frp_subdomain: map_str(&desktop_tunnel, "frpSubdomain").unwrap_or_default(),
            frp_profile_id: map_str(&desktop_tunnel, "frpProfileId").unwrap_or_default(),
            frp_server_port: map_u64(&desktop_tunnel, "frpServerPort", 443) as u16,
            cloudflare_mode: map_str(&desktop_tunnel, "cloudflareMode")
                .unwrap_or_else(|| "quick".into()),
            use_proxy,
        },
        auth: AuthConfig {
            auth_type: map_str(&desktop, "authType")
                .unwrap_or_else(|| document.auth.auth_type.clone()),
            oauth_client_id: document.auth.oauth_client_id.clone(),
            oauth_token_ttl_seconds: document.auth.oauth_token_ttl_seconds,
            use_shared_secrets: map_bool(&desktop, "useSharedSecrets", false),
        },
        runtime: RuntimeConfig {
            local_port: document.bind.port,
            bind_address: document.bind.host.clone(),
            transport_mode: map_str(&desktop, "transportMode")
                .unwrap_or_else(|| "streamable-http".into()),
            tool_profile: document.tool_profile.clone(),
            permission_mode: document.permission_mode.clone(),
            security_policy: if document.security_policy.is_empty() {
                None
            } else {
                Some(policy_from_map(&document.security_policy))
            },
            runtime_command: map_str(&desktop, "runtimeCommand").unwrap_or_default(),
            allowed_commands: document.policy.allowed_commands.join(","),
            workspace_local_entries: document.policy.workspace_local_entries,
            workspace_script_extensions: document.policy.workspace_script_extensions.join(","),
            blocking_admission_limit: document.limits.blocking_concurrency,
            process_admission_limit: document.limits.process_concurrency,
            global_blocking_admission_limit: document.limits.global_blocking_concurrency,
            global_process_admission_limit: document.limits.global_process_concurrency,
            active_session_limit: document.limits.active_session_limit,
            sandbox: SandboxConfig {
                enabled: document.sandbox.enabled,
                backend: document.sandbox.backend.clone(),
                external_paths: document
                    .sandbox
                    .external_paths
                    .iter()
                    .map(|entry| SandboxPathGrant {
                        path: entry.path.clone(),
                        access: if entry.access == "modify" {
                            SandboxPathAccess::Modify
                        } else {
                            SandboxPathAccess::ReadOnly
                        },
                    })
                    .collect(),
                options: document
                    .sandbox
                    .options
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|text| (key.clone(), text.to_string()))
                    })
                    .collect(),
            },
        },
        actions,
    }
}

pub fn merge_desktop_profile_on_canonical(
    existing: Option<&CanonicalWorkspace>,
    profile: &WorkspaceProfile,
) -> CanonicalWorkspace {
    let mut next = canonical_from_desktop_profile(profile);
    let Some(existing) = existing else {
        return next;
    };

    next.skills = existing.skills.clone();
    next.extensions = existing.extensions.clone();
    next.host.node = existing.host.node.clone();
    next.extra = existing.extra.clone();

    if let (Some(existing_desktop), Some(next_desktop)) = (
        existing.host.desktop.as_object(),
        next.host.desktop.as_object_mut(),
    ) {
        let mut merged = existing_desktop.clone();
        for owned in [
            "authType",
            "useSharedSecrets",
            "transportMode",
            "runtimeCommand",
            "tunnel",
            "useProxy",
            "actions",
        ] {
            merged.remove(owned);
        }
        for (key, value) in next_desktop.iter() {
            merged.insert(key.clone(), value.clone());
        }
        *next_desktop = merged;
    }

    for folder in &mut next.folders {
        if let Some(previous) = existing.folders.iter().find(|item| item.id == folder.id) {
            let mut extra = previous.extra.clone();
            for (key, value) in folder.extra.iter() {
                extra.insert(key.clone(), value.clone());
            }
            folder.extra = extra;
        }
    }
    next
}

pub fn roundtrip_desktop_profile(profile: WorkspaceProfile) -> WorkspaceProfile {
    desktop_profile_from_canonical(&canonical_from_desktop_profile(&profile))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(name: &str) -> Value {
        let root = format!(
            "{}/../docs/specs/shared-workspace-config/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(root).expect("fixture"))
            .expect("fixture json")
    }

    fn shared(document: &CanonicalWorkspace) -> Value {
        let mut value = serialize_canonical_workspace(document).expect("serialize");
        if let Some(object) = value.as_object_mut() {
            object.remove("host");
            object.remove("extra");
        }
        value
    }

    #[test]
    fn parses_minimal_and_full_fixtures() {
        let minimal = parse_canonical_workspace(&fixture("minimal.json")).unwrap();
        assert_eq!(minimal.id, "ws-minimal");
        assert_eq!(minimal.bind.port, 3789);
        let full = parse_canonical_workspace(&fixture("full.json")).unwrap();
        assert_eq!(full.folders.len(), 2);
        assert_eq!(full.policy.allowed_commands, vec!["pytest", "cargo"]);
        let roundtrip =
            parse_canonical_workspace(&serialize_canonical_workspace(&full).unwrap()).unwrap();
        assert_eq!(shared(&full), shared(&roundtrip));
    }

    #[test]
    fn preserves_host_extensions_and_strips_secrets() {
        let desktop = parse_canonical_workspace(&fixture("desktop-extras.json")).unwrap();
        assert_eq!(desktop.host.desktop["tunnel"]["type"].as_str(), Some("frp"));
        let node = parse_canonical_workspace(&fixture("node-extras.json")).unwrap();
        assert_eq!(
            node.host.node["management"]["enabled"].as_bool(),
            Some(true)
        );
        let migrated =
            canonical_from_node_v1(&fixture("from-node-v1.json"), "ws-v1", Some("V1")).unwrap();
        let serialized = serialize_canonical_workspace(&migrated).unwrap();
        let text = serialized.to_string();
        assert!(!text.contains("must-not-survive-migrate"));
        assert!(!text.contains("enroll/SECRET"));
        assert_eq!(migrated.auth.oauth_client_id, "chatgpt-v1");
        assert_eq!(migrated.host.node["dataDir"], json!("C:\\data\\node-v1"));
    }

    #[test]
    fn migrates_desktop_profile_fixture() {
        let profile: WorkspaceProfile =
            serde_json::from_value(fixture("from-desktop-profile.json")).unwrap();
        let canonical = canonical_from_desktop_profile(&profile);
        assert_eq!(canonical.id, "desktop-1");
        assert!(canonical.tunnel.builtin.enabled);
        assert_eq!(canonical.bind.port, 18790);
        assert_eq!(canonical.policy.allowed_commands, vec!["pytest", "cargo"]);
        let restored = desktop_profile_from_canonical(&canonical);
        assert_eq!(restored.runtime.local_port, 18790);
        assert_eq!(restored.auth.oauth_client_id, "chatgpt-desktop");
        assert_eq!(restored.actions.oauth_client_id, "actions-desktop");
        assert_eq!(restored.actions.local_port, 18791);
        assert_eq!(restored.tunnel.tunnel_type, "builtin");
    }

    #[test]
    fn desktop_roundtrip_keeps_host_only_fields() {
        let mut profile: WorkspaceProfile =
            serde_json::from_value(fixture("from-desktop-profile.json")).unwrap();
        profile.auth.auth_type = "bearer".into();
        profile.auth.use_shared_secrets = true;
        profile.runtime.transport_mode = "legacy-json".into();
        profile.runtime.runtime_command = "custom-runtime".into();
        profile.tunnel.tunnel_type = "frp".into();
        profile.tunnel.public_url = "https://dev.example.test/mcp".into();
        profile.tunnel.frp_server = "frp.example".into();
        profile.tunnel.frp_subdomain = "dev".into();
        profile.tunnel.frp_profile_id = "profile-1".into();
        profile.tunnel.frp_server_port = 7000;
        profile.tunnel.use_proxy = false;
        profile.folders[0].execution = ExecutionTarget::Wsl {
            distro: "Ubuntu".into(),
            linux_path: "/home/repo".into(),
        };
        let restored = roundtrip_desktop_profile(profile.clone());
        assert_eq!(restored.auth.auth_type, "bearer");
        assert!(restored.auth.use_shared_secrets);
        assert_eq!(restored.runtime.transport_mode, "legacy-json");
        assert_eq!(restored.runtime.runtime_command, "custom-runtime");
        assert_eq!(restored.tunnel.tunnel_type, "frp");
        assert_eq!(restored.tunnel.public_url, "https://dev.example.test/mcp");
        assert_eq!(restored.tunnel.frp_server, "frp.example");
        assert_eq!(restored.tunnel.frp_subdomain, "dev");
        assert_eq!(restored.tunnel.frp_profile_id, "profile-1");
        assert_eq!(restored.tunnel.frp_server_port, 7000);
        assert!(!restored.tunnel.use_proxy);
        assert_eq!(
            restored.folders[0].execution,
            ExecutionTarget::Wsl {
                distro: "Ubuntu".into(),
                linux_path: "/home/repo".into(),
            }
        );
        assert_eq!(restored.actions.oauth_client_id, "actions-desktop");
        assert_eq!(restored.runtime.local_port, 18790);
    }

    #[test]
    fn desktop_live_merge_preserves_node_owned_fields() {
        let mut existing = parse_canonical_workspace(&fixture("node-extras.json")).unwrap();
        existing.skills.active = true;
        existing.skills.disabled = vec!["skill-a".into()];
        existing.extensions.hooks.active = true;
        existing
            .extra
            .insert("futureTopLevel".into(), json!({"keep": true}));
        existing.host.desktop = json!({
            "runtimeCommand": "stale-runtime",
            "futureDesktop": {"keep": true}
        });

        let mut profile = desktop_profile_from_canonical(&existing);
        profile.name = "Desktop renamed".into();
        profile.runtime.runtime_command.clear();
        let merged = merge_desktop_profile_on_canonical(Some(&existing), &profile);

        assert_eq!(merged.name, "Desktop renamed");
        assert!(merged.skills.active);
        assert_eq!(merged.skills.disabled, vec!["skill-a"]);
        assert!(merged.extensions.hooks.active);
        assert_eq!(merged.host.node, existing.host.node);
        assert_eq!(merged.extra["futureTopLevel"]["keep"], json!(true));
        assert_eq!(merged.host.desktop["futureDesktop"]["keep"], json!(true));
        assert!(merged.host.desktop.get("runtimeCommand").is_none());
    }
}
