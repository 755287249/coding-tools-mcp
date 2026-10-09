use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use serde::{Deserialize, Serialize};

use super::location::{parse_wsl_unc_path, ExecutionTarget};
use crate::settings::AppSettings;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceProfile {
    pub id: String,
    pub name: String,
    /// Legacy-compatible active folder path. New code should use `folders` and
    /// `active_folder_id`; this value is kept in sync for the desktop UI and upgrades.
    pub path: String,
    #[serde(default)]
    pub folders: Vec<WorkspaceFolder>,
    /// Desktop UI selection and MCP bootstrap context only. Runtime tool routing
    /// must never treat this as an authorization default or fallback.
    #[serde(default)]
    pub active_folder_id: String,
    pub tunnel: TunnelConfig,
    pub auth: AuthConfig,
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub actions: ActionsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceFolder {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub execution: ExecutionTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelConfig {
    #[serde(rename = "type", default = "legacy_tunnel_type")]
    pub tunnel_type: String,
    #[serde(default)]
    pub public_url: String,
    #[serde(default)]
    pub frp_server: String,
    #[serde(default)]
    pub frp_subdomain: String,
    #[serde(default)]
    pub frp_profile_id: String,
    #[serde(default = "default_frp_server_port")]
    pub frp_server_port: u16,
    #[serde(default = "default_cloudflare_mode")]
    pub cloudflare_mode: String,
    /// When true, apply global proxy from Settings → General when starting the tunnel.
    #[serde(default = "default_use_proxy")]
    pub use_proxy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPublicEndpoint {
    pub endpoint_url: String,
    pub base_url: String,
    pub server_host: String,
    pub server_port: u16,
    pub route_prefix: String,
}

impl McpPublicEndpoint {
    pub fn authorization_server_metadata_path(&self) -> String {
        format!(
            "/.well-known/oauth-authorization-server{}",
            self.route_prefix
        )
    }

    pub fn protected_resource_metadata_path(&self) -> String {
        format!(
            "/.well-known/oauth-protected-resource{}/mcp",
            self.route_prefix
        )
    }

    pub fn frp_locations(&self) -> Vec<String> {
        vec![
            self.route_prefix.clone(),
            self.authorization_server_metadata_path(),
            self.protected_resource_metadata_path(),
        ]
    }
}

pub fn parse_mcp_public_endpoint(value: &str) -> Result<McpPublicEndpoint, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("公開 MCP 網址不可為空。".into());
    }

    let mut endpoint =
        reqwest::Url::parse(value).map_err(|error| format!("公開 MCP 網址格式無效：{error}"))?;
    if endpoint.scheme() != "https" {
        return Err("公開 MCP 網址必須使用 HTTPS。".into());
    }
    if !endpoint.username().is_empty() || endpoint.password().is_some() {
        return Err("公開 MCP 網址不可包含使用者名稱或密碼。".into());
    }
    if endpoint.query().is_some() || endpoint.fragment().is_some() {
        return Err("公開 MCP 網址不可包含查詢參數或片段。".into());
    }

    let server_host = endpoint
        .host_str()
        .filter(|host| !host.trim().is_empty())
        .ok_or_else(|| "公開 MCP 網址缺少網域。".to_string())?
        .to_string();
    let server_port = endpoint.port_or_known_default().unwrap_or(443);
    let mut endpoint_path = endpoint.path().trim_end_matches('/').to_string();
    if !endpoint_path.ends_with("/mcp") && !endpoint_path.is_empty() && endpoint_path != "/" {
        endpoint_path.push_str("/mcp");
    }
    let route_prefix = endpoint_path
        .strip_suffix("/mcp")
        .filter(|prefix| !prefix.is_empty() && *prefix != "/")
        .ok_or_else(|| {
            "公開 MCP 網址必須使用例如 /clients/<client-id>/mcp 的獨立路徑。".to_string()
        })?
        .to_string();
    endpoint.set_path(&endpoint_path);
    let endpoint_url = endpoint.as_str().trim_end_matches('/').to_string();
    let mut base = endpoint.clone();
    base.set_path(&route_prefix);
    let base_url = base.as_str().trim_end_matches('/').to_string();

    Ok(McpPublicEndpoint {
        endpoint_url,
        base_url,
        server_host,
        server_port,
        route_prefix,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(rename = "type", default = "default_auth_type")]
    pub auth_type: String,
    #[serde(default = "default_oauth_client_id")]
    pub oauth_client_id: String,
    #[serde(default = "default_oauth_token_ttl_seconds")]
    pub oauth_token_ttl_seconds: u64,
    #[serde(default)]
    pub use_shared_secrets: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPathAccess {
    #[default]
    ReadOnly,
    Modify,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxPathGrant {
    pub path: String,
    #[serde(default)]
    pub access: SandboxPathAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxConfig {
    #[serde(default = "default_sandbox_enabled")]
    pub enabled: bool,
    #[serde(default = "default_sandbox_backend")]
    pub backend: String,
    #[serde(default)]
    pub external_paths: Vec<SandboxPathGrant>,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecurityPolicy {
    #[serde(default = "default_true")]
    pub restrict_tool_catalog: bool,
    #[serde(default = "default_true")]
    pub enforce_command_allowlist: bool,
    #[serde(default = "default_true")]
    pub require_dangerous_confirmation: bool,
    #[serde(default = "default_true")]
    pub require_shell_confirmation: bool,
    #[serde(default)]
    pub block_network_commands: bool,
    #[serde(default = "default_true")]
    pub enforce_workspace_boundary: bool,
    #[serde(default = "default_true")]
    pub protect_repository_metadata: bool,
    #[serde(default = "default_true")]
    pub block_symlink_escape: bool,
    #[serde(default = "default_true")]
    pub protect_environment_variables: bool,
    #[serde(default = "default_true")]
    pub enforce_harness_baseline: bool,
    #[serde(default = "default_true")]
    pub require_write_confirmation: bool,
    #[serde(default = "default_true")]
    pub verify_write_conflicts: bool,
    #[serde(default = "default_true")]
    pub enforce_resource_limits: bool,
    #[serde(default = "default_true")]
    pub redact_sensitive_output: bool,
    #[serde(default = "default_true")]
    pub withhold_sensitive_source_output: bool,
    #[serde(default = "default_true")]
    pub redact_telemetry: bool,
    #[serde(default = "default_true")]
    pub redact_history: bool,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            restrict_tool_catalog: true,
            enforce_command_allowlist: true,
            require_dangerous_confirmation: true,
            require_shell_confirmation: true,
            block_network_commands: false,
            enforce_workspace_boundary: true,
            protect_repository_metadata: true,
            block_symlink_escape: true,
            protect_environment_variables: true,
            enforce_harness_baseline: true,
            require_write_confirmation: true,
            verify_write_conflicts: true,
            enforce_resource_limits: true,
            redact_sensitive_output: true,
            withhold_sensitive_source_output: true,
            redact_telemetry: true,
            redact_history: true,
        }
    }
}

impl SecurityPolicy {
    pub fn legacy(permission_mode: &str, tool_profile: &str) -> Self {
        Self {
            restrict_tool_catalog: !matches!(tool_profile, "advanced" | "compat-readonly-all"),
            require_dangerous_confirmation: permission_mode != "dangerous",
            require_shell_confirmation: permission_mode != "dangerous",
            require_write_confirmation: permission_mode != "dangerous",
            block_network_commands: !matches!(permission_mode, "trusted" | "dangerous"),
            ..Self::default()
        }
    }

    pub fn compatibility_permission_mode(&self) -> &'static str {
        if !self.require_dangerous_confirmation
            && !self.require_shell_confirmation
            && !self.require_write_confirmation
        {
            "dangerous"
        } else if self.block_network_commands {
            "safe"
        } else {
            "trusted"
        }
    }

    pub fn compatibility_tool_profile(&self) -> &'static str {
        if self.restrict_tool_catalog {
            "trusted-core"
        } else {
            "advanced"
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_mcp_port")]
    pub local_port: u16,
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    #[serde(default = "default_transport_mode")]
    pub transport_mode: String,
    #[serde(default = "default_tool_profile")]
    pub tool_profile: String,
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    #[serde(default)]
    pub security_policy: Option<SecurityPolicy>,
    #[serde(default)]
    pub runtime_command: String,
    /// Workspace execution policy shared by MCP clients.
    #[serde(default = "default_allowed_commands")]
    pub allowed_commands: String,
    #[serde(default = "default_workspace_local_entries")]
    pub workspace_local_entries: bool,
    #[serde(default = "default_workspace_script_extensions")]
    pub workspace_script_extensions: String,
    #[serde(default = "default_blocking_admission_limit")]
    pub blocking_admission_limit: u16,
    #[serde(default = "default_process_admission_limit")]
    pub process_admission_limit: u16,
    #[serde(default = "default_global_blocking_admission_limit")]
    pub global_blocking_admission_limit: u16,
    #[serde(default = "default_global_process_admission_limit")]
    pub global_process_admission_limit: u16,
    #[serde(default = "default_active_session_limit")]
    pub active_session_limit: u16,
    #[serde(default)]
    pub sandbox: SandboxConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionsConfig {
    #[serde(default)]
    pub public_url: String,
    #[serde(default = "legacy_tunnel_type")]
    pub tunnel_type: String,
    #[serde(default)]
    pub frp_server: String,
    #[serde(default)]
    pub frp_subdomain: String,
    #[serde(default)]
    pub frp_profile_id: String,
    #[serde(default = "default_frp_server_port")]
    pub frp_server_port: u16,
    #[serde(default = "default_cloudflare_mode")]
    pub cloudflare_mode: String,
    #[serde(default)]
    pub cloudflare_token: String,
    #[serde(default = "default_use_proxy")]
    pub use_proxy: bool,
    #[serde(default = "default_actions_port")]
    pub local_port: u16,
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    #[serde(default)]
    pub runtime_command: String,
    #[serde(default = "default_actions_auth_type")]
    pub auth_type: String,
    #[serde(default = "default_actions_oauth_client_id")]
    pub oauth_client_id: String,
    #[serde(default)]
    pub oauth_scopes: String,
    #[serde(default = "default_allowed_commands")]
    pub allowed_commands: String,
    #[serde(default = "default_max_patch_bytes")]
    pub max_patch_bytes: u32,
    #[serde(default)]
    pub use_shared_secrets: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatusDto {
    pub state: String,
    pub pid: Option<u32>,
    pub local_message: String,
    pub public_message: String,
    pub local_endpoint: String,
    pub public_endpoint: String,
}

fn legacy_tunnel_type() -> String {
    "frp".to_string()
}

const DEFAULT_BUILTIN_TUNNEL_ORIGIN: &str = "http://127.0.0.1:8088";

fn default_builtin_mcp_url(workspace_id: &str) -> String {
    format!("{DEFAULT_BUILTIN_TUNNEL_ORIGIN}/builtin/clients/{workspace_id}/mcp")
}

fn default_builtin_actions_url(workspace_id: &str) -> String {
    format!("{DEFAULT_BUILTIN_TUNNEL_ORIGIN}/builtin/actions/{workspace_id}")
}

fn default_cloudflare_mode() -> String {
    "quick".to_string()
}

fn default_use_proxy() -> bool {
    true
}

fn default_auth_type() -> String {
    "oauth".to_string()
}

fn default_frp_server_port() -> u16 {
    443
}

fn default_actions_auth_type() -> String {
    "api_key".to_string()
}

fn default_actions_oauth_client_id() -> String {
    format!(
        "chatgpt-actions-{}",
        &uuid::Uuid::new_v4().to_string()[..12]
    )
}

fn default_oauth_client_id() -> String {
    format!("chatgpt-client-{}", &uuid::Uuid::new_v4().to_string()[..12])
}

pub const DEFAULT_OAUTH_TOKEN_TTL_SECONDS: u64 = 3650 * 24 * 60 * 60;

pub fn default_oauth_token_ttl_seconds() -> u64 {
    DEFAULT_OAUTH_TOKEN_TTL_SECONDS
}

fn default_mcp_port() -> u16 {
    28766
}

pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1";

fn default_bind_address() -> String {
    DEFAULT_BIND_ADDRESS.to_string()
}

pub fn parse_bind_address(value: &str) -> Result<IpAddr, String> {
    let value = value.trim();
    let value = if value.is_empty() {
        DEFAULT_BIND_ADDRESS
    } else {
        value
    };
    value.parse::<IpAddr>().map_err(|_| {
        format!(
            "監聽位址「{value}」無效，請輸入 IPv4 或 IPv6 位址，例如 127.0.0.1、0.0.0.0 或 ::1。"
        )
    })
}

pub fn socket_addr_for_bind(value: &str, port: u16) -> Result<SocketAddr, String> {
    parse_bind_address(value).map(|address| SocketAddr::new(address, port))
}

pub fn connect_address_for_bind(value: &str) -> String {
    match parse_bind_address(value).unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)) {
        IpAddr::V4(address) if address.is_unspecified() => Ipv4Addr::LOCALHOST.to_string(),
        IpAddr::V6(address) if address.is_unspecified() => Ipv6Addr::LOCALHOST.to_string(),
        address => address.to_string(),
    }
}

pub fn url_host_for_bind(value: &str) -> String {
    let address = connect_address_for_bind(value);
    if address.contains(':') {
        format!("[{address}]")
    } else {
        address
    }
}

fn default_transport_mode() -> String {
    "streamable-http".to_string()
}

fn default_actions_port() -> u16 {
    8787
}

fn default_tool_profile() -> String {
    "core".to_string()
}

fn default_permission_mode() -> String {
    "trusted".to_string()
}

fn default_sandbox_backend() -> String {
    "appcontainer".to_string()
}

fn default_sandbox_enabled() -> bool {
    false
}

fn default_allowed_commands() -> String {
    "pytest,python,python3,npm,npx,node,pnpm,yarn,make,mvn,mvnw,gradle,gradlew,cargo,go,ruff,mypy,eslint,tsc,git,cmd,powershell,pwsh".to_string()
}

fn default_workspace_local_entries() -> bool {
    true
}

fn default_workspace_script_extensions() -> String {
    ".exe,.bat,.cmd,.ps1".to_string()
}

fn default_blocking_admission_limit() -> u16 {
    128
}

fn default_process_admission_limit() -> u16 {
    64
}

fn default_global_blocking_admission_limit() -> u16 {
    1024
}

fn default_global_process_admission_limit() -> u16 {
    512
}

fn default_active_session_limit() -> u16 {
    512
}

fn default_max_patch_bytes() -> u32 {
    200_000
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            tunnel_type: legacy_tunnel_type(),
            public_url: String::new(),
            frp_server: String::new(),
            frp_subdomain: String::new(),
            frp_profile_id: String::new(),
            frp_server_port: default_frp_server_port(),
            cloudflare_mode: default_cloudflare_mode(),
            use_proxy: default_use_proxy(),
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            auth_type: default_auth_type(),
            oauth_client_id: default_oauth_client_id(),
            oauth_token_ttl_seconds: default_oauth_token_ttl_seconds(),
            use_shared_secrets: false,
        }
    }
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: default_sandbox_backend(),
            external_paths: Vec::new(),
            options: BTreeMap::new(),
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            local_port: default_mcp_port(),
            bind_address: default_bind_address(),
            transport_mode: default_transport_mode(),
            tool_profile: default_tool_profile(),
            permission_mode: default_permission_mode(),
            security_policy: None,
            runtime_command: String::new(),
            allowed_commands: default_allowed_commands(),
            workspace_local_entries: default_workspace_local_entries(),
            workspace_script_extensions: default_workspace_script_extensions(),
            blocking_admission_limit: default_blocking_admission_limit(),
            process_admission_limit: default_process_admission_limit(),
            global_blocking_admission_limit: default_global_blocking_admission_limit(),
            global_process_admission_limit: default_global_process_admission_limit(),
            active_session_limit: default_active_session_limit(),
            sandbox: SandboxConfig::default(),
        }
    }
}

impl Default for ActionsConfig {
    fn default() -> Self {
        Self {
            public_url: String::new(),
            tunnel_type: legacy_tunnel_type(),
            frp_server: String::new(),
            frp_subdomain: String::new(),
            frp_profile_id: String::new(),
            frp_server_port: default_frp_server_port(),
            cloudflare_mode: default_cloudflare_mode(),
            cloudflare_token: String::new(),
            use_proxy: default_use_proxy(),
            local_port: default_actions_port(),
            bind_address: default_bind_address(),
            permission_mode: default_permission_mode(),
            runtime_command: String::new(),
            auth_type: default_actions_auth_type(),
            oauth_client_id: default_actions_oauth_client_id(),
            oauth_scopes: String::new(),
            allowed_commands: default_allowed_commands(),
            max_patch_bytes: default_max_patch_bytes(),
            use_shared_secrets: false,
        }
    }
}

impl WorkspaceFolder {
    pub fn new(path: String, name: Option<String>) -> Self {
        let mut execution = ExecutionTarget::from_host_path(&path);
        let path = execution
            .normalize_for_host_path(&path)
            .unwrap_or_else(|| clean_folder_path(&path));
        Self {
            id: new_folder_id(),
            name: name
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| folder_name_from_path(&path)),
            path,
            execution,
        }
    }

    fn normalize_execution(&mut self) {
        if let Some(path) = self.execution.normalize_for_host_path(&self.path) {
            self.path = path;
        }
    }
}

fn new_folder_id() -> String {
    uuid::Uuid::new_v4().to_string().replace('-', "")
}

fn clean_folder_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed == "/" {
        return trimmed.to_string();
    }
    #[cfg(windows)]
    if trimmed.len() == 3 && trimmed.as_bytes().get(1) == Some(&b':') {
        return trimmed.to_string();
    }
    trimmed.trim_end_matches(['\\', '/']).to_string()
}

fn comparable_folder_path(path: &str) -> String {
    if let Some(location) = parse_wsl_unc_path(path) {
        return format!(
            "//wsl.localhost/{}/{}",
            location.distro.to_ascii_lowercase(),
            location.linux_path.trim_start_matches('/')
        );
    }
    let normalized = clean_folder_path(path).replace('\\', "/");
    #[cfg(windows)]
    {
        return normalized.to_lowercase();
    }
    #[cfg(not(windows))]
    normalized
}

fn folder_name_from_path(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty())
        .next_back()
        .unwrap_or("資料夾")
        .to_string()
}

#[allow(dead_code)]
impl WorkspaceProfile {
    pub fn new(path: String, name: Option<String>) -> Self {
        let cleaned = path.trim_end_matches(['\\', '/']).to_string();
        let label = name.unwrap_or_else(|| {
            cleaned
                .replace('\\', "/")
                .split('/')
                .next_back()
                .unwrap_or("工作区")
                .to_string()
        });
        let folder = WorkspaceFolder::new(cleaned.clone(), None);
        let cleaned = folder.path.clone();
        let id = uuid::Uuid::new_v4().to_string().replace('-', "");
        let mut tunnel = TunnelConfig::default();
        tunnel.tunnel_type = "builtin".into();
        tunnel.public_url = default_builtin_mcp_url(&id);
        tunnel.use_proxy = false;
        let mut actions = ActionsConfig::default();
        actions.tunnel_type = "builtin".into();
        actions.public_url = default_builtin_actions_url(&id);
        actions.use_proxy = false;
        Self {
            id,
            name: label,
            path: cleaned,
            active_folder_id: folder.id.clone(),
            folders: vec![folder],
            tunnel,
            auth: AuthConfig::default(),
            runtime: RuntimeConfig::default(),
            actions,
        }
    }

    /// Keep the backend usable for legacy profiles without changing the user's
    /// explicit sandbox enablement choice.
    pub fn normalize_sandbox(&mut self) -> bool {
        let mut changed = false;
        if self.runtime.sandbox.backend.trim().is_empty() {
            self.runtime.sandbox.backend = default_sandbox_backend();
            changed = true;
        }
        changed
    }

    /// Migrates legacy single-path profiles and keeps the compatibility path
    /// synchronized with the selected folder. Returns true when data changed.
    pub fn normalize_folders(&mut self) -> bool {
        let original_path = self.path.clone();
        let original_active = self.active_folder_id.clone();
        let original_folders = self.folders.clone();

        self.path = clean_folder_path(&self.path);
        if self.folders.is_empty() && !self.path.is_empty() {
            self.folders
                .push(WorkspaceFolder::new(self.path.clone(), None));
        }

        let mut normalized = Vec::with_capacity(self.folders.len());
        for mut folder in std::mem::take(&mut self.folders) {
            folder.normalize_execution();
            folder.path = clean_folder_path(&folder.path);
            if folder.path.is_empty()
                || normalized.iter().any(|item: &WorkspaceFolder| {
                    comparable_folder_path(&item.path) == comparable_folder_path(&folder.path)
                })
            {
                continue;
            }
            if folder.id.trim().is_empty() {
                folder.id = new_folder_id();
            }
            if folder.name.trim().is_empty() {
                folder.name = folder_name_from_path(&folder.path);
            }
            normalized.push(folder);
        }
        self.folders = normalized;

        if self.folders.is_empty() {
            self.active_folder_id.clear();
            self.path.clear();
        } else {
            let active_index = self
                .folders
                .iter()
                .position(|folder| folder.id == self.active_folder_id)
                .or_else(|| {
                    self.folders.iter().position(|folder| {
                        comparable_folder_path(&folder.path) == comparable_folder_path(&self.path)
                    })
                })
                .unwrap_or(0);
            self.active_folder_id = self.folders[active_index].id.clone();

            // Existing UI/API clients edit `path`; treat that as editing the
            // selected folder instead of silently discarding the update.
            if !self.path.is_empty()
                && comparable_folder_path(&self.path)
                    != comparable_folder_path(&self.folders[active_index].path)
            {
                self.folders[active_index].path = self.path.clone();
                if self.folders[active_index].name.trim().is_empty() {
                    self.folders[active_index].name = folder_name_from_path(&self.path);
                }
            }
            self.path = self.folders[active_index].path.clone();
        }

        self.path != original_path
            || self.active_folder_id != original_active
            || self.folders != original_folders
    }

    /// Upgrades only the exact legacy concurrency default tuple. Any user-customized
    /// value keeps the entire tuple untouched so loading cannot overwrite tuning.
    pub fn normalize_runtime_concurrency_defaults(&mut self) -> bool {
        let legacy_defaults = (
            self.runtime.blocking_admission_limit,
            self.runtime.process_admission_limit,
            self.runtime.global_blocking_admission_limit,
            self.runtime.global_process_admission_limit,
            self.runtime.active_session_limit,
        ) == (8, 4, 16, 8, 16);
        if !legacy_defaults {
            return false;
        }
        self.runtime.blocking_admission_limit = default_blocking_admission_limit();
        self.runtime.process_admission_limit = default_process_admission_limit();
        self.runtime.global_blocking_admission_limit = default_global_blocking_admission_limit();
        self.runtime.global_process_admission_limit = default_global_process_admission_limit();
        self.runtime.active_session_limit = default_active_session_limit();
        true
    }

    pub fn normalize_bind_addresses(&mut self) -> bool {
        let runtime = normalized_bind_address(&self.runtime.bind_address);
        let actions = normalized_bind_address(&self.actions.bind_address);
        let changed = runtime != self.runtime.bind_address || actions != self.actions.bind_address;
        self.runtime.bind_address = runtime;
        self.actions.bind_address = actions;
        changed
    }

    pub fn validate_bind_addresses(&self) -> Result<(), String> {
        parse_bind_address(&self.runtime.bind_address).map_err(|error| format!("MCP {error}"))?;
        parse_bind_address(&self.actions.bind_address)
            .map_err(|error| format!("Actions {error}"))?;
        Ok(())
    }

    pub fn active_folder(&self) -> Option<&WorkspaceFolder> {
        self.folders
            .iter()
            .find(|folder| folder.id == self.active_folder_id)
            .or_else(|| self.folders.first())
    }

    pub fn local_endpoint(&self) -> String {
        format!(
            "http://{}:{}/mcp",
            url_host_for_bind(&self.runtime.bind_address),
            self.runtime.local_port
        )
    }

    pub fn effective_public_url(&self) -> String {
        self.effective_public_url_with(&AppSettings::load_or_default())
    }

    pub fn effective_public_url_with(&self, settings: &AppSettings) -> String {
        computed_public_url(
            &self.tunnel.tunnel_type,
            &self.tunnel.frp_server,
            &self.tunnel.frp_subdomain,
            &self.tunnel.public_url,
            &self.tunnel.frp_profile_id,
            settings,
        )
    }

    pub fn public_endpoint(&self) -> String {
        let base = self.effective_public_url();
        if base.is_empty() {
            return String::new();
        }
        format!("{}/mcp", base.trim_end_matches('/'))
    }

    pub fn actions_local_base_url(&self) -> String {
        format!(
            "http://{}:{}",
            url_host_for_bind(&self.actions.bind_address),
            self.actions.local_port
        )
    }

    pub fn actions_effective_public_url(&self) -> String {
        self.actions_effective_public_url_with(&AppSettings::load_or_default())
    }

    pub fn actions_effective_public_url_with(&self, settings: &AppSettings) -> String {
        computed_public_url(
            &self.actions.tunnel_type,
            &self.actions.frp_server,
            &self.actions.frp_subdomain,
            &self.actions.public_url,
            &self.actions.frp_profile_id,
            settings,
        )
    }

    pub fn actions_openapi_url(&self) -> String {
        let base = self.actions_public_base_url();
        if base.is_empty() {
            return String::new();
        }
        format!("{}/openapi.json", base.trim_end_matches('/'))
    }

    pub fn actions_privacy_url(&self) -> String {
        let base = self.actions_public_base_url();
        if base.is_empty() {
            return String::new();
        }
        format!("{}/privacy", base.trim_end_matches('/'))
    }

    pub fn actions_oauth_authorization_url(&self) -> String {
        let base = self.actions_public_base_url();
        if base.is_empty() {
            return String::new();
        }
        format!("{}/oauth/authorize", base.trim_end_matches('/'))
    }

    pub fn actions_oauth_token_url(&self) -> String {
        let base = self.actions_public_base_url();
        if base.is_empty() {
            return String::new();
        }
        format!("{}/oauth/token", base.trim_end_matches('/'))
    }

    /// Public URL for GPT schema import; falls back to localhost when no tunnel is configured.
    pub fn actions_public_base_url(&self) -> String {
        let public = self.actions_effective_public_url();
        if public.is_empty() {
            self.actions_local_base_url()
        } else {
            public
        }
    }
}

fn normalized_bind_address(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return default_bind_address();
    }
    trimmed
        .parse::<IpAddr>()
        .map(|address| address.to_string())
        .unwrap_or_else(|_| trimmed.to_string())
}

fn computed_public_url(
    tunnel_type: &str,
    frp_server: &str,
    frp_subdomain: &str,
    public_url: &str,
    frp_profile_id: &str,
    settings: &AppSettings,
) -> String {
    if tunnel_type == "builtin" {
        if let Ok(endpoint) = parse_mcp_public_endpoint(public_url) {
            return endpoint.base_url;
        }
    } else if tunnel_type == "frp" {
        if let Ok(endpoint) = parse_mcp_public_endpoint(public_url) {
            return endpoint.base_url;
        }
        let server = settings
            .find_frp_profile(frp_profile_id)
            .map(|profile| profile.server.as_str())
            .unwrap_or(frp_server);
        if !server.is_empty() && !frp_subdomain.is_empty() {
            return format!("https://{frp_subdomain}.{server}");
        }
    }
    public_url.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        comparable_folder_path, parse_mcp_public_endpoint, ActionsConfig, RuntimeConfig,
        SecurityPolicy, TunnelConfig, WorkspaceFolder, WorkspaceProfile, DEFAULT_BIND_ADDRESS,
    };
    use crate::workspace::ExecutionTarget;

    #[test]
    fn legacy_wsl_folder_is_upgraded_without_schema_breakage() {
        let mut folder: WorkspaceFolder = serde_json::from_value(serde_json::json!({
            "id": "legacy-wsl",
            "name": "SampleProject",
            "path": r"\\wsl$\Ubuntu-24.04\opt\src\SampleProject"
        }))
        .expect("legacy folder");
        assert_eq!(folder.execution, ExecutionTarget::Host);

        folder.normalize_execution();

        assert_eq!(
            folder.path,
            r"\\wsl.localhost\Ubuntu-24.04\opt\src\SampleProject"
        );
        assert_eq!(
            folder.execution,
            ExecutionTarget::Wsl {
                distro: "Ubuntu-24.04".into(),
                linux_path: "/opt/src/SampleProject".into(),
            }
        );
    }

    #[test]
    fn wsl_folder_identity_preserves_linux_path_case() {
        assert_eq!(
            comparable_folder_path(r"\\wsl$\Ubuntu-24.04\opt\src\SampleProject"),
            comparable_folder_path(r"\\wsl.localhost\ubuntu-24.04\opt\src\SampleProject")
        );
        assert_ne!(
            comparable_folder_path(r"\\wsl.localhost\Ubuntu-24.04\opt\src\SampleProject"),
            comparable_folder_path(r"\\wsl.localhost\Ubuntu-24.04\opt\src\sampleproject")
        );
    }

    #[test]
    fn new_workspace_defaults_both_services_to_builtin() {
        let profile = WorkspaceProfile::new("/tmp/demo".into(), Some("demo".into()));

        assert_eq!(profile.tunnel.tunnel_type, "builtin");
        assert_eq!(profile.actions.tunnel_type, "builtin");
        assert!(profile
            .tunnel
            .public_url
            .starts_with("http://127.0.0.1:8088/builtin/clients/"));
        assert!(profile.tunnel.public_url.ends_with("/mcp"));
        assert!(profile
            .actions
            .public_url
            .starts_with("http://127.0.0.1:8088/builtin/actions/"));
        assert!(!profile.tunnel.use_proxy);
        assert!(!profile.actions.use_proxy);
        assert_eq!(profile.tunnel.frp_server_port, 443);
        assert_eq!(profile.actions.frp_server_port, 443);
    }

    #[test]
    fn missing_tunnel_type_keeps_legacy_frp_default() {
        let tunnel: TunnelConfig = serde_json::from_value(serde_json::json!({})).expect("tunnel");
        let actions: ActionsConfig =
            serde_json::from_value(serde_json::json!({})).expect("actions");

        assert_eq!(tunnel.tunnel_type, "frp");
        assert_eq!(actions.tunnel_type, "frp");
    }

    #[test]
    fn legacy_builtin_worker_count_is_ignored() {
        let tunnel: TunnelConfig = serde_json::from_value(serde_json::json!({
            "builtin_worker_count": 16
        }))
        .expect("legacy tunnel");
        let encoded = serde_json::to_value(tunnel).expect("serialize tunnel");

        assert!(encoded.get("builtin_worker_count").is_none());
    }

    #[test]
    fn missing_entire_actions_config_keeps_legacy_frp_default() {
        let profile = WorkspaceProfile::new("/tmp/demo".into(), Some("demo".into()));
        let mut encoded = serde_json::to_value(profile).expect("serialize profile");
        encoded
            .as_object_mut()
            .expect("profile object")
            .remove("actions");

        let restored: WorkspaceProfile = serde_json::from_value(encoded).expect("restore profile");

        assert_eq!(restored.actions.tunnel_type, "frp");
        assert!(restored.actions.public_url.is_empty());
    }

    #[test]
    fn legacy_concurrency_defaults_upgrade_as_one_tuple() {
        let mut profile = WorkspaceProfile::new("/tmp/demo".into(), Some("demo".into()));
        profile.runtime.blocking_admission_limit = 8;
        profile.runtime.process_admission_limit = 4;
        profile.runtime.global_blocking_admission_limit = 16;
        profile.runtime.global_process_admission_limit = 8;
        profile.runtime.active_session_limit = 16;

        assert!(profile.normalize_runtime_concurrency_defaults());
        assert_eq!(profile.runtime.blocking_admission_limit, 128);
        assert_eq!(profile.runtime.process_admission_limit, 64);
        assert_eq!(profile.runtime.global_blocking_admission_limit, 1024);
        assert_eq!(profile.runtime.global_process_admission_limit, 512);
        assert_eq!(profile.runtime.active_session_limit, 512);
    }

    #[test]
    fn customized_concurrency_tuple_is_preserved() {
        let mut profile = WorkspaceProfile::new("/tmp/demo".into(), Some("demo".into()));
        profile.runtime.blocking_admission_limit = 9;
        profile.runtime.process_admission_limit = 4;
        profile.runtime.global_blocking_admission_limit = 16;
        profile.runtime.global_process_admission_limit = 8;
        profile.runtime.active_session_limit = 16;

        assert!(!profile.normalize_runtime_concurrency_defaults());
        assert_eq!(profile.runtime.blocking_admission_limit, 9);
        assert_eq!(profile.runtime.process_admission_limit, 4);
        assert_eq!(profile.runtime.global_blocking_admission_limit, 16);
        assert_eq!(profile.runtime.global_process_admission_limit, 8);
        assert_eq!(profile.runtime.active_session_limit, 16);
    }

    #[test]
    fn parses_path_scoped_public_mcp_endpoint() {
        let endpoint =
            parse_mcp_public_endpoint("https://coding-tools.example.com/clients/pc-a/mcp/")
                .expect("public MCP endpoint");

        assert_eq!(endpoint.server_host, "coding-tools.example.com");
        assert_eq!(endpoint.server_port, 443);
        assert_eq!(endpoint.route_prefix, "/clients/pc-a");
        assert_eq!(
            endpoint.endpoint_url,
            "https://coding-tools.example.com/clients/pc-a/mcp"
        );
        assert_eq!(
            endpoint.base_url,
            "https://coding-tools.example.com/clients/pc-a"
        );
    }

    #[test]
    fn public_mcp_endpoint_requires_https_and_client_path() {
        assert!(parse_mcp_public_endpoint("http://example.com/clients/a/mcp").is_err());
        assert!(parse_mcp_public_endpoint("https://example.com/mcp").is_err());
        assert!(parse_mcp_public_endpoint("https://example.com/clients/a/mcp?x=1").is_err());
    }

    #[test]
    fn legacy_base_url_is_upgraded_to_full_mcp_endpoint() {
        let endpoint = parse_mcp_public_endpoint("https://example.com/clients/a")
            .expect("legacy base URL should be upgraded");

        assert_eq!(endpoint.route_prefix, "/clients/a");
        assert_eq!(endpoint.endpoint_url, "https://example.com/clients/a/mcp");
        assert_eq!(endpoint.base_url, "https://example.com/clients/a");
    }

    #[test]
    fn url_based_frp_public_endpoint_keeps_the_supplied_path() {
        let mut profile = WorkspaceProfile::new("/tmp/demo".into(), Some("demo".into()));
        profile.tunnel.tunnel_type = "frp".into();
        profile.tunnel.public_url = "https://example.com/clients/a/mcp".into();

        assert_eq!(
            profile.effective_public_url(),
            "https://example.com/clients/a"
        );
        assert_eq!(
            profile.public_endpoint(),
            "https://example.com/clients/a/mcp"
        );
    }
    #[test]
    fn builtin_mcp_public_endpoint_uses_namespaced_base_url() {
        let mut profile = WorkspaceProfile::new("/tmp/demo".into(), Some("demo".into()));
        profile.tunnel.tunnel_type = "builtin".into();
        profile.tunnel.public_url = "https://example.com/builtin/clients/pc-a/mcp".into();

        assert_eq!(
            profile.effective_public_url(),
            "https://example.com/builtin/clients/pc-a"
        );
        assert_eq!(
            profile.public_endpoint(),
            "https://example.com/builtin/clients/pc-a/mcp"
        );
    }

    #[test]
    fn builtin_actions_public_urls_keep_the_namespaced_base() {
        let mut profile = WorkspaceProfile::new("/tmp/demo".into(), Some("demo".into()));
        profile.actions.tunnel_type = "builtin".into();
        profile.actions.public_url = "https://example.com/builtin/actions/pc-a".into();

        assert_eq!(
            profile.actions_effective_public_url(),
            "https://example.com/builtin/actions/pc-a"
        );
        assert_eq!(
            profile.actions_openapi_url(),
            "https://example.com/builtin/actions/pc-a/openapi.json"
        );
        assert_eq!(
            profile.actions_oauth_token_url(),
            "https://example.com/builtin/actions/pc-a/oauth/token"
        );
    }

    #[test]
    fn listener_addresses_default_to_loopback() {
        let profile = WorkspaceProfile::new("/tmp/demo".into(), Some("demo".into()));

        assert_eq!(profile.runtime.bind_address, DEFAULT_BIND_ADDRESS);
        assert_eq!(profile.actions.bind_address, DEFAULT_BIND_ADDRESS);
        assert_eq!(profile.local_endpoint(), "http://127.0.0.1:28766/mcp");
        assert_eq!(profile.actions_local_base_url(), "http://127.0.0.1:8787");
    }

    #[test]
    fn legacy_configs_without_bind_address_use_loopback() {
        let runtime: RuntimeConfig = serde_json::from_str("{}").expect("runtime config");
        let actions: ActionsConfig = serde_json::from_str("{}").expect("actions config");

        assert_eq!(runtime.bind_address, DEFAULT_BIND_ADDRESS);
        assert_eq!(actions.bind_address, DEFAULT_BIND_ADDRESS);
        assert!(runtime.sandbox.external_paths.is_empty());
    }

    #[test]
    fn legacy_security_policy_requires_write_confirmation_except_dangerous() {
        assert!(SecurityPolicy::legacy("safe", "core").require_write_confirmation);
        assert!(SecurityPolicy::legacy("trusted", "advanced").require_write_confirmation);
        assert!(!SecurityPolicy::legacy("dangerous", "advanced").require_write_confirmation);
    }
    #[test]
    fn security_policy_round_trips_individual_protections_and_keeps_legacy_configs_compatible() {
        let mut runtime = RuntimeConfig::default();
        runtime.security_policy = Some(SecurityPolicy::default());
        runtime
            .security_policy
            .as_mut()
            .expect("security policy")
            .block_network_commands = true;
        let encoded = serde_json::to_value(&runtime).expect("encode runtime");
        let restored: RuntimeConfig = serde_json::from_value(encoded).expect("decode runtime");
        assert!(
            restored
                .security_policy
                .expect("security policy")
                .block_network_commands
        );

        let legacy: RuntimeConfig = serde_json::from_value(serde_json::json!({
            "permission_mode": "dangerous",
            "tool_profile": "advanced"
        }))
        .expect("legacy runtime");
        assert!(legacy.security_policy.is_none());
    }

    #[test]
    fn wildcard_listener_uses_loopback_for_local_endpoint() {
        let mut profile = WorkspaceProfile::new("/tmp/demo".into(), Some("demo".into()));
        profile.runtime.bind_address = "0.0.0.0".into();
        profile.actions.bind_address = "::".into();

        assert_eq!(profile.local_endpoint(), "http://127.0.0.1:28766/mcp");
        assert_eq!(profile.actions_local_base_url(), "http://[::1]:8787");
    }

    #[test]
    fn invalid_bind_address_is_rejected() {
        let mut profile = WorkspaceProfile::new("/tmp/demo".into(), Some("demo".into()));
        profile.runtime.bind_address = "localhost".into();

        assert!(profile.validate_bind_addresses().is_err());
    }
}
