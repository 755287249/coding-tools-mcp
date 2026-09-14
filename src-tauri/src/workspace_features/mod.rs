mod discovery;
mod evolved_skills;
mod external_mcp;
mod hooks;
mod skills;
mod skills_mcp;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use serde::Serialize;
use tokio::sync::Mutex as AsyncMutex;

use crate::data::DataStore;
use crate::workspace::canonical::{CanonicalExtensions, CanonicalToggle};
use crate::workspace::WorkspaceFolder;

pub use discovery::{
    discover_extensions, ExtensionDiagnostic, HookDescriptor, McpServerDescriptor,
};
pub use evolved_skills::{apply_evolved_skill_record, apply_promoted_evolved_skills};
pub use external_mcp::{call_external_tool, list_external_tools, ExternalTool};
pub use hooks::{run_post_tool_hooks, run_pre_tool_hooks};
pub use skills::{discover_skills, SkillDescriptor, SkillDiagnostic, SkillEvolution};
pub(crate) use skills_mcp::selected_skill_learning_attribution;
pub use skills_mcp::{
    bootstrap_summary as skill_bootstrap_summary,
    get_prompt_for_session as get_skill_prompt_for_session, list_prompts as list_skill_prompts,
    list_resources as list_skill_resources,
    read_resource_for_session as read_skill_resource_for_session, rpc_error as skill_rpc_error,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInventoryItem {
    pub key: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub scope: String,
    pub relative_path: String,
    pub root_relative_path: String,
    pub version: Option<String>,
    pub selected: bool,
    pub enabled: bool,
    pub folder_id: Option<String>,
    pub folder_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInventoryPayload {
    pub ok: bool,
    pub workspace_id: String,
    pub active: bool,
    pub skills: Vec<SkillInventoryItem>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookInventoryItem {
    pub key: String,
    pub provider: String,
    pub scope: String,
    pub folder_id: Option<String>,
    pub event: String,
    pub matcher: Option<String>,
    pub handler_type: String,
    pub source_path: String,
    pub source_enabled: bool,
    pub supported: bool,
    pub selected: bool,
    pub enabled: bool,
    pub command: Option<String>,
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInventoryItem {
    pub key: String,
    pub provider: String,
    pub scope: String,
    pub folder_id: Option<String>,
    pub name: String,
    pub transport: String,
    pub source_path: String,
    pub source_enabled: bool,
    pub supported: bool,
    pub selected: bool,
    pub enabled: bool,
    pub connected: bool,
    pub tool_count: usize,
    pub command: Option<String>,
    pub endpoint: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionInventoryPayload {
    pub ok: bool,
    pub workspace_id: String,
    pub hooks_active: bool,
    pub mcp_active: bool,
    pub hooks: Vec<HookInventoryItem>,
    pub mcp_servers: Vec<McpServerInventoryItem>,
    pub diagnostics: Vec<ExtensionDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillMasterToggleResult {
    pub ok: bool,
    pub workspace_id: String,
    pub active: bool,
    pub restart_required: bool,
    pub applied_immediately: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillToggleResult {
    pub ok: bool,
    pub workspace_id: String,
    pub skill_key: String,
    pub enabled: bool,
    pub restart_required: bool,
    pub applied_immediately: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionMasterToggleResult {
    pub ok: bool,
    pub workspace_id: String,
    pub extension_kind: String,
    pub active: bool,
    pub restart_required: bool,
    pub applied_immediately: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionToggleResult {
    pub ok: bool,
    pub workspace_id: String,
    pub extension_kind: String,
    pub extension_key: String,
    pub enabled: bool,
    pub restart_required: bool,
    pub applied_immediately: Vec<String>,
}

#[derive(Clone)]
pub struct FeatureConfig {
    pub skills: CanonicalToggle,
    pub extensions: CanonicalExtensions,
}

pub struct RuntimeFeatures {
    pub folders: Vec<WorkspaceFolder>,
    config: RwLock<FeatureConfig>,
    pub(crate) connections: AsyncMutex<HashMap<String, external_mcp::ExternalMcpConnection>>,
    pub(crate) sessions: AsyncMutex<HashMap<(String, String), String>>,
}

static RUNTIMES: OnceLock<Mutex<HashMap<String, Arc<RuntimeFeatures>>>> = OnceLock::new();

fn runtimes() -> &'static Mutex<HashMap<String, Arc<RuntimeFeatures>>> {
    RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_runtimes() -> std::sync::MutexGuard<'static, HashMap<String, Arc<RuntimeFeatures>>> {
    runtimes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl RuntimeFeatures {
    pub(crate) fn config(&self) -> FeatureConfig {
        self.config
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn update_config(&self, config: FeatureConfig) {
        *self
            .config
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = config;
    }
}

pub fn register_runtime(workspace_id: &str, folders: Vec<WorkspaceFolder>) {
    let config = DataStore::load()
        .ok()
        .and_then(|store| store.feature_document(workspace_id).ok())
        .map(|document| FeatureConfig {
            skills: document.skills,
            extensions: document.extensions,
        })
        .unwrap_or_else(|| FeatureConfig {
            skills: CanonicalToggle::default(),
            extensions: CanonicalExtensions::default(),
        });
    lock_runtimes().insert(
        workspace_id.to_string(),
        Arc::new(RuntimeFeatures {
            folders,
            config: RwLock::new(config),
            connections: AsyncMutex::new(HashMap::new()),
            sessions: AsyncMutex::new(HashMap::new()),
        }),
    );
}

pub fn unregister_runtime(workspace_id: &str) {
    let runtime = lock_runtimes().remove(workspace_id);
    if let Some(runtime) = runtime {
        crate::task_runtime::spawn(async move {
            hooks::run_session_end_hooks(&runtime, "shutdown").await;
            let mut connections = runtime.connections.lock().await;
            let drained = connections
                .drain()
                .map(|(_, connection)| connection)
                .collect::<Vec<_>>();
            drop(connections);
            for mut connection in drained {
                connection.close().await;
            }
        });
    }
}

pub fn update_runtime_config(workspace_id: &str, config: FeatureConfig) -> bool {
    let runtime = lock_runtimes().get(workspace_id).cloned();
    if let Some(runtime) = runtime {
        runtime.update_config(config);
        true
    } else {
        false
    }
}

pub fn runtime(workspace_id: &str) -> Option<Arc<RuntimeFeatures>> {
    lock_runtimes().get(workspace_id).cloned()
}

pub fn skill_inventory(
    workspace_id: &str,
    folders: &[WorkspaceFolder],
    toggle: &CanonicalToggle,
) -> SkillInventoryPayload {
    let discovered = discover_skills(folders);
    let disabled = toggle.disabled.iter().cloned().collect::<HashSet<_>>();
    let skills = discovered
        .skills
        .into_iter()
        .map(|skill| {
            let selected = !disabled.contains(&skill.key);
            SkillInventoryItem {
                key: skill.key,
                name: skill.name,
                description: skill.description,
                source: skill.source,
                scope: skill.scope,
                relative_path: skill.relative_path,
                root_relative_path: skill.root_relative_path,
                version: skill.version,
                selected,
                enabled: toggle.active && selected,
                folder_id: skill.folder_id,
                folder_name: skill.folder_name,
            }
        })
        .collect();
    SkillInventoryPayload {
        ok: true,
        workspace_id: workspace_id.to_string(),
        active: toggle.active,
        skills,
        diagnostics: discovered.diagnostics,
    }
}

pub async fn extension_inventory(
    workspace_id: &str,
    folders: &[WorkspaceFolder],
    extensions: &CanonicalExtensions,
) -> ExtensionInventoryPayload {
    let discovered = discover_extensions(folders).await;
    let enabled_hooks = extensions
        .hooks
        .enabled
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let enabled_mcp = extensions
        .mcp
        .enabled
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let hooks = discovered
        .hooks
        .into_iter()
        .map(|hook| {
            let selected = enabled_hooks.contains(&hook.key);
            HookInventoryItem {
                key: hook.key,
                provider: hook.provider,
                scope: hook.scope,
                folder_id: hook.folder_id,
                event: hook.event,
                matcher: hook.matcher,
                handler_type: hook.handler_type,
                source_path: hook.source_path,
                source_enabled: hook.source_enabled,
                supported: hook.supported,
                selected,
                enabled: extensions.hooks.active
                    && selected
                    && hook.source_enabled
                    && hook.supported,
                command: hook.command,
                endpoint: hook.url,
            }
        })
        .collect();
    let mut mcp_servers = Vec::new();
    for server in discovered.mcp_servers {
        let selected = enabled_mcp.contains(&server.key);
        let (connected, tool_count, error) = external_mcp::status(workspace_id, &server.key).await;
        mcp_servers.push(McpServerInventoryItem {
            key: server.key,
            provider: server.provider,
            scope: server.scope,
            folder_id: Some(server.folder_id),
            name: server.name,
            transport: server.transport,
            source_path: server.source_path,
            source_enabled: server.source_enabled,
            supported: server.supported,
            selected,
            enabled: extensions.mcp.active && selected && server.source_enabled && server.supported,
            connected,
            tool_count,
            command: server.command,
            endpoint: server.url,
            error,
        });
    }
    ExtensionInventoryPayload {
        ok: true,
        workspace_id: workspace_id.to_string(),
        hooks_active: extensions.hooks.active,
        mcp_active: extensions.mcp.active,
        hooks,
        mcp_servers,
        diagnostics: discovered.diagnostics,
    }
}
