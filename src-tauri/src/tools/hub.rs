use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};

use crate::knowledge::sync_tool_evolution_lifecycle_for_workspace;

use crate::tools::{
    ExecutionLimits, SharedRuntimeToolConfig, SharedToolContext, ToolContext, Workspace,
};
use crate::workspace::{
    compare_wsl_paths, AuthConfig, RuntimeConfig, WorkspaceFolder, WorkspaceProfile,
};

const MAX_CONVERSATION_CONTEXTS: usize = 128;

pub(crate) fn behavioral_parity_fixture() -> Value {
    json!({ "max_conversation_contexts": MAX_CONVERSATION_CONTEXTS })
}

#[derive(Clone)]
pub struct HubConfig {
    pub auth: AuthConfig,
    pub runtime_config: SharedRuntimeToolConfig,
    pub limits: ExecutionLimits,
    pub execution_resource_namespace: String,
}

struct RoutingState {
    folders: Vec<WorkspaceFolder>,
    session_folders: HashMap<String, String>,
    action_session_folders: HashMap<String, String>,
    action_resume_folders: HashMap<String, String>,
    contexts: HashMap<String, SharedToolContext>,
    saved_cwds: HashMap<String, PathBuf>,
    context_last_used: HashMap<String, u64>,
    context_access_clock: u64,
}

#[derive(Clone)]
pub struct RoutedContext {
    pub folder_id: String,
    pub folder: WorkspaceFolder,
    pub context: SharedToolContext,
}

#[derive(Clone)]
pub struct McpRoutedContext {
    pub folder_id: String,
    pub selected_folder_id: Option<String>,
    pub route_source: &'static str,
    pub context: SharedToolContext,
}

pub struct HubRouter {
    profile_id: String,
    config: HubConfig,
    state: Mutex<RoutingState>,
}

static LIVE_HUBS: OnceLock<Mutex<HashMap<String, Arc<HubRouter>>>> = OnceLock::new();

fn live_hubs() -> &'static Mutex<HashMap<String, Arc<HubRouter>>> {
    LIVE_HUBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_hubs() -> std::sync::MutexGuard<'static, HashMap<String, Arc<HubRouter>>> {
    live_hubs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_state(router: &HubRouter) -> std::sync::MutexGuard<'_, RoutingState> {
    router
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn register(
    profile_id: String,
    folders: Vec<WorkspaceFolder>,
    bootstrap_folder_id: String,
    initial_context: SharedToolContext,
    config: HubConfig,
) -> Result<(), String> {
    for folder in &folders {
        let _ = sync_tool_evolution_lifecycle_for_workspace(Path::new(&folder.path));
    }
    let router = HubRouter::new(profile_id.clone(), folders, config)?;
    let bootstrap_folder_id = {
        let state = lock_state(&router);
        folder_id_if_allowed(&state.folders, &bootstrap_folder_id)?
    };
    {
        let mut state = lock_state(&router);
        let cache_key = context_cache_key(&bootstrap_folder_id, None);
        state.contexts.insert(cache_key.clone(), initial_context);
        touch_context_locked(&mut state, &cache_key);
    }
    lock_hubs().insert(profile_id, router);
    Ok(())
}

pub fn remove_live_hub(profile_id: &str) {
    lock_hubs().remove(profile_id);
}

pub fn sync_live_hub(profile: &WorkspaceProfile) -> Result<bool, String> {
    let router = lock_hubs().get(&profile.id).cloned();
    let Some(router) = router else {
        return Ok(false);
    };
    router.sync(profile.folders.clone(), &profile.runtime)?;
    Ok(true)
}

pub fn preflight_live_hub(profile: &WorkspaceProfile) -> Result<bool, String> {
    let router = lock_hubs().get(&profile.id).cloned();
    let Some(router) = router else {
        return Ok(false);
    };
    router.preflight(profile.folders.as_slice(), &profile.runtime)?;
    Ok(true)
}

pub fn sync_live_hub_after_preflight(profile: &WorkspaceProfile) -> Result<bool, String> {
    let router = lock_hubs().get(&profile.id).cloned();
    let Some(router) = router else {
        return Ok(false);
    };
    router.sync_preflighted(profile.folders.clone(), &profile.runtime)?;
    Ok(true)
}

pub fn resolve_context(
    fallback: SharedToolContext,
    host_session_key: Option<&str>,
) -> Result<SharedToolContext, String> {
    let router = lock_hubs().get(&fallback.profile_id).cloned();
    let Some(router) = router else {
        #[cfg(test)]
        {
            return Ok(fallback);
        }
        #[cfg(not(test))]
        {
            return Err("工具區 routing 尚未初始化；未明確選取資料夾前不得存取專案內容。".into());
        }
    };
    router.resolve(host_session_key)
}

pub(crate) fn resolve_profile_folder_context(
    profile_id: &str,
    folder_id: &str,
) -> Result<SharedToolContext, String> {
    let router = lock_hubs()
        .get(profile_id)
        .cloned()
        .ok_or_else(|| "Knowledge ingestion workspace router is not active.".to_string())?;
    router.context_for_folder(folder_id, None)
}

pub fn resolve_tool_context(
    fallback: SharedToolContext,
    host_session_key: Option<&str>,
    explicit_folder_id: Option<&str>,
    tool_name: &str,
    arguments: &Value,
) -> Result<McpRoutedContext, String> {
    let router = lock_hubs().get(&fallback.profile_id).cloned();
    let Some(router) = router else {
        #[cfg(test)]
        {
            return Ok(McpRoutedContext {
                folder_id: "legacy".into(),
                selected_folder_id: None,
                route_source: "legacy",
                context: fallback,
            });
        }
        #[cfg(not(test))]
        {
            return Err("工具區 routing 尚未初始化；未明確選取資料夾前不得存取專案內容。".into());
        }
    };
    router.resolve_tool_context(host_session_key, explicit_folder_id, tool_name, arguments)
}

pub fn list_workspace_folders(
    fallback: &SharedToolContext,
    host_session_key: Option<&str>,
) -> Value {
    let router = lock_hubs().get(&fallback.profile_id).cloned();
    let Some(router) = router else {
        #[cfg(test)]
        {
            return json!({
                "ok": true,
                "multi_folder": false,
                "selected_folder_id": Value::Null,
                "selection_scope": "unselected",
                "folders": [{
                    "id": "legacy",
                    "name": fallback.workspace.root().file_name().and_then(|value| value.to_str()).unwrap_or("workspace"),
                    "path": fallback.workspace_path(),
                    "selected": false
                }]
            });
        }
        #[cfg(not(test))]
        {
            return hub_error(
                "WORKSPACE_ROUTER_NOT_CONFIGURED",
                "工具區 routing 尚未初始化；無法列出可用資料夾。",
                false,
            );
        }
    };
    match router.folder_listing(host_session_key) {
        Ok(value) => value,
        Err(message) => hub_error("WORKSPACE_FOLDER_ROUTING_FAILED", &message, true),
    }
}

pub fn switch_workspace_folder(
    fallback: &SharedToolContext,
    folder_id: &str,
    host_session_key: Option<&str>,
) -> Value {
    let router = lock_hubs().get(&fallback.profile_id).cloned();
    let Some(router) = router else {
        return hub_error(
            "MULTI_FOLDER_NOT_CONFIGURED",
            "目前 MCP 沒有設定多資料夾工具區。",
            false,
        );
    };
    match router.switch(folder_id, host_session_key) {
        Ok(value) => value,
        Err(message) => {
            let (code, message) = routing_error_parts(&message, "WORKSPACE_FOLDER_SWITCH_FAILED");
            hub_error(code, message, false)
        }
    }
}

impl HubRouter {
    pub fn new(
        profile_id: String,
        folders: Vec<WorkspaceFolder>,
        config: HubConfig,
    ) -> Result<Arc<Self>, String> {
        if folders.is_empty() {
            return Err("工具區至少需要一個資料夾。".into());
        }
        validate_unique_folder_paths(&folders)?;
        Ok(Arc::new(Self {
            profile_id,
            config,
            state: Mutex::new(RoutingState {
                folders,
                session_folders: HashMap::new(),
                action_session_folders: HashMap::new(),
                action_resume_folders: HashMap::new(),
                contexts: HashMap::new(),
                saved_cwds: HashMap::new(),
                context_last_used: HashMap::new(),
                context_access_clock: 0,
            }),
        }))
    }

    fn resolve_tool_context(
        &self,
        host_session_key: Option<&str>,
        explicit_folder_id: Option<&str>,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<McpRoutedContext, String> {
        let session_key = host_session_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "WORKSPACE_FOLDER_NOT_SELECTED: 缺少 MCP conversation/session identity；無法建立單次資料夾路由。"
                    .to_string()
            })?;
        let (folder_id, selected_folder_id, route_source) = {
            let state = lock_state(self);
            let selected = state.session_folders.get(session_key).cloned();
            let explicit = explicit_folder_id
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(folder_id) = explicit {
                (
                    folder_id_if_allowed(&state.folders, folder_id)?,
                    selected,
                    "explicit",
                )
            } else if let Some(session_id) = action_session_id(tool_name, arguments) {
                let folder_id = unique_conversation_context_match(
                    &state,
                    session_key,
                    |context| context.sessions.contains(session_id),
                    "session_id",
                )?
                .ok_or_else(|| {
                    format!(
                        "WORKSPACE_FOLDER_NOT_SELECTED: 找不到目前 conversation 中 session_id 对应的资料夹上下文：{session_id}。"
                    )
                })?;
                (folder_id, selected, "session_id")
            } else if tool_name == "request_permissions" {
                let resume_id = arguments
                    .get("resume_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if let Some(resume_id) = resume_id {
                    let folder_id = unique_conversation_context_match(
                        &state,
                        session_key,
                        |context| context.pending_operations.contains(resume_id),
                        "resume_id",
                    )?
                    .ok_or_else(|| {
                        format!(
                            "WORKSPACE_FOLDER_NOT_SELECTED: 找不到目前 conversation 中 resume_id 对应的资料夹上下文：{resume_id}。"
                        )
                    })?;
                    (folder_id, selected, "resume_id")
                } else {
                    let folder_id = selected.clone().ok_or_else(|| {
                        "WORKSPACE_FOLDER_NOT_SELECTED: request_permissions 没有可推导的 resume_id，且 conversation 尚未绑定资料夹。"
                            .to_string()
                    })?;
                    (folder_id, selected, "conversation")
                }
            } else {
                let folder_id = selected.clone().ok_or_else(|| {
                    "WORKSPACE_FOLDER_NOT_SELECTED: 此 session 尚未选取资料夹；请先呼叫 conversation_bootstrap，或为支援的工具提供 workspace_folder_id。"
                        .to_string()
                })?;
                (folder_id, selected, "conversation")
            }
        };
        let context = self.context_for_folder(&folder_id, Some(session_key))?;
        Ok(McpRoutedContext {
            folder_id,
            selected_folder_id,
            route_source,
            context,
        })
    }

    pub fn action_folder_listing(&self) -> Value {
        match self.folder_listing(None) {
            Ok(value) => value,
            Err(message) => hub_error("WORKSPACE_FOLDER_ROUTING_FAILED", &message, true),
        }
    }

    pub fn resolve_action_context(
        &self,
        explicit_folder_id: Option<&str>,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<RoutedContext, String> {
        let mut state = lock_state(self);
        let explicit = explicit_folder_id
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let folder_id = if let Some(folder_id) = explicit {
            folder_id_if_allowed(&state.folders, folder_id)?
        } else if let Some(session_id) = action_session_id(tool_name, arguments) {
            self.folder_for_session_locked(&mut state, session_id)?
                .ok_or_else(|| {
                    format!(
                        "找不到 session_id 对应的资料夹上下文：{session_id}；请确认 ID 或明确提供 workspace_folder_id。"
                    )
                })?
        } else if tool_name == "request_permissions" {
            let resume_id = arguments
                .get("resume_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            match resume_id {
                Some(resume_id) => self
                    .folder_for_resume_locked(&mut state, resume_id)?
                    .ok_or_else(|| {
                        format!(
                            "找不到 resume_id 对应的资料夹上下文：{resume_id}；请确认 ID 或明确提供 workspace_folder_id。"
                        )
                    })?,
                None => {
                    return Err(
                        "WORKSPACE_FOLDER_NOT_SELECTED: request_permissions 必須提供可識別資料夾的 resume_id，或明確提供 workspace_folder_id。"
                            .into(),
                    )
                }
            }
        } else {
            return Err(
                "WORKSPACE_FOLDER_NOT_SELECTED: 此 Actions 請求必須明確提供 workspace_folder_id。"
                    .into(),
            );
        };
        let folder = state
            .folders
            .iter()
            .find(|folder| folder.id == folder_id)
            .cloned()
            .ok_or_else(|| format!("資料夾不在允許清單內：{folder_id}"))?;
        drop(state);
        let context = self.context_for_folder(&folder_id, None)?;
        Ok(RoutedContext {
            folder_id,
            folder,
            context,
        })
    }

    fn folder_for_session_locked(
        &self,
        state: &mut RoutingState,
        session_id: &str,
    ) -> Result<Option<String>, String> {
        if let Some(folder_id) = state.action_session_folders.get(session_id) {
            return Ok(Some(folder_id.clone()));
        }
        let matched = unique_context_match(
            state,
            |context| context.sessions.contains(session_id),
            "session_id",
        )?;
        if let Some(folder_id) = &matched {
            state
                .action_session_folders
                .insert(session_id.to_string(), folder_id.clone());
        }
        Ok(matched)
    }

    fn folder_for_resume_locked(
        &self,
        state: &mut RoutingState,
        resume_id: &str,
    ) -> Result<Option<String>, String> {
        if let Some(folder_id) = state.action_resume_folders.get(resume_id) {
            return Ok(Some(folder_id.clone()));
        }
        let matched = unique_context_match(
            state,
            |context| context.pending_operations.contains(resume_id),
            "resume_id",
        )?;
        if let Some(folder_id) = &matched {
            state
                .action_resume_folders
                .insert(resume_id.to_string(), folder_id.clone());
        }
        Ok(matched)
    }

    pub fn record_action_result(&self, folder_id: &str, result: &Value) {
        let mut state = lock_state(self);
        if !state.folders.iter().any(|folder| folder.id == folder_id) {
            return;
        }
        for session_id in action_result_session_ids(result) {
            state
                .action_session_folders
                .insert(session_id, folder_id.to_string());
        }
        if let Some(resume_id) = result
            .get("resume_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            state
                .action_resume_folders
                .insert(resume_id.to_string(), folder_id.to_string());
        }
    }

    fn live_preflight_folders(&self, folders: &[WorkspaceFolder]) -> Vec<WorkspaceFolder> {
        let live_paths = {
            let state = lock_state(self);
            state
                .contexts
                .iter()
                // The bootstrap context has a cache key without a conversation suffix and
                // must be ready before the generation is published. Conversation contexts
                // are prewarmed only while an external owner still holds them; cache-only
                // historical entries may prepare lazily on their next fail-closed use.
                .filter(|(cache_key, context)| {
                    !cache_key.contains('\u{0}') || Arc::strong_count(context) > 1
                })
                .map(|(_, context)| context.workspace_path())
                .collect::<Vec<_>>()
        };
        folders
            .iter()
            .filter(|folder| live_paths.iter().any(|path| same_path(&folder.path, path)))
            .cloned()
            .collect()
    }

    fn preflight(
        &self,
        folders: &[WorkspaceFolder],
        runtime: &RuntimeConfig,
    ) -> Result<(), String> {
        if folders.is_empty() {
            return Err("工具區至少需要一個資料夾。".into());
        }
        validate_unique_folder_paths(folders)?;
        let current_sandbox = self.config.runtime_config.snapshot().sandbox;
        if !runtime.sandbox.enabled || current_sandbox == runtime.sandbox {
            return Ok(());
        }
        // Prewarm only folders that already own a live ToolContext. Inactive folders
        // remain fail-closed: their first command prepares the enabled backend on demand.
        // An unrelated configured folder must not block a global sandbox generation swap.
        for folder in self.live_preflight_folders(folders) {
            let workspace = Workspace::new_with_execution(
                PathBuf::from(&folder.path),
                folder.execution.clone(),
            )
            .map_err(|error| error.message())?;
            let context =
                ToolContext::from_workspace_with_shared_runtime_config_and_resource_ids_and_limits(
                    workspace,
                    self.config.auth.clone(),
                    self.config.runtime_config.clone(),
                    self.profile_id.clone(),
                    format!(
                        "{}--{}--{}",
                        self.profile_id, self.config.execution_resource_namespace, folder.id
                    ),
                    format!(
                        "{}--{}",
                        self.profile_id, self.config.execution_resource_namespace
                    ),
                    self.config.limits,
                );
            crate::tools::exec::prewarm_sandbox_backend(&context, &runtime.sandbox)
                .map_err(|error| error.message())?;
        }
        Ok(())
    }

    fn sync(&self, folders: Vec<WorkspaceFolder>, runtime: &RuntimeConfig) -> Result<(), String> {
        self.preflight(&folders, runtime)?;
        self.sync_preflighted(folders, runtime)
    }

    fn sync_preflighted(
        &self,
        folders: Vec<WorkspaceFolder>,
        runtime: &RuntimeConfig,
    ) -> Result<(), String> {
        if folders.is_empty() {
            return Err("工具區至少需要一個資料夾。".into());
        }
        validate_unique_folder_paths(&folders)?;
        self.config.runtime_config.update_from_runtime(runtime);
        let mut state = lock_state(self);
        let previous_folders = state.folders.clone();
        state.contexts.retain(|_, context| {
            folders
                .iter()
                .any(|folder| same_path(&folder.path, &context.workspace_path()))
        });
        let retained_contexts = state
            .contexts
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        state
            .context_last_used
            .retain(|cache_key, _| retained_contexts.contains(cache_key));
        state.saved_cwds.retain(|cache_key, _| {
            let folder_id = context_folder_id(cache_key);
            folders.iter().any(|folder| folder.id == folder_id)
        });
        state
            .session_folders
            .retain(|_, folder_id| folders.iter().any(|folder| folder.id == *folder_id));
        state
            .action_session_folders
            .retain(|_, folder_id| folders.iter().any(|folder| folder.id == *folder_id));
        state
            .action_resume_folders
            .retain(|_, folder_id| folders.iter().any(|folder| folder.id == *folder_id));
        let lifecycle_folders = folders
            .iter()
            .filter(|folder| {
                !previous_folders.iter().any(|previous| {
                    previous.id == folder.id && same_path(&previous.path, &folder.path)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        state.folders = folders;
        drop(state);
        for folder in lifecycle_folders {
            let _ = sync_tool_evolution_lifecycle_for_workspace(Path::new(&folder.path));
        }
        Ok(())
    }

    fn resolve(&self, host_session_key: Option<&str>) -> Result<SharedToolContext, String> {
        let folder_id = {
            let mut state = lock_state(self);
            self.resolve_folder_id_locked(&mut state, host_session_key)?
        };
        self.context_for_folder(&folder_id, host_session_key)
    }

    fn folder_listing(&self, host_session_key: Option<&str>) -> Result<Value, String> {
        let state = lock_state(self);
        let selected = host_session_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|session_key| state.session_folders.get(session_key).cloned());
        let selection_scope = if selected.is_some() {
            "conversation"
        } else {
            "unselected"
        };
        let folders = state
            .folders
            .iter()
            .map(|folder| {
                json!({
                    "id": folder.id,
                    "name": folder.name,
                    "path": folder.path,
                    "selected": selected.as_deref() == Some(folder.id.as_str()),
                    "history_dir": Path::new(&folder.path).join("docs/history-session").display().to_string()
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "ok": true,
            "multi_folder": state.folders.len() > 1,
            "profile_id": self.profile_id,
            "selected_folder_id": selected,
            "selection_scope": selection_scope,
            "conversation_isolated": host_session_key.is_some(),
            "folders": folders
        }))
    }

    fn switch(&self, folder_id: &str, host_session_key: Option<&str>) -> Result<Value, String> {
        let folder_id = folder_id.trim();
        if folder_id.is_empty() {
            return Err("folder_id 不可為空。".into());
        }
        let session_key = host_session_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "WORKSPACE_FOLDER_NOT_SELECTED: 缺少 MCP conversation/session identity；無法建立資料夾綁定。"
                    .to_string()
            })?;
        let folder = {
            let state = lock_state(self);
            state
                .folders
                .iter()
                .find(|folder| folder.id == folder_id)
                .cloned()
                .ok_or_else(|| format!("資料夾不在允許清單內：{folder_id}"))?
        };
        let _ = self.context_for_folder(folder_id, Some(session_key))?;
        let mut state = lock_state(self);
        if !state
            .folders
            .iter()
            .any(|candidate| candidate.id == folder_id)
        {
            return Err(format!("資料夾不在允許清單內：{folder_id}"));
        }
        state
            .session_folders
            .insert(session_key.to_string(), folder_id.to_string());
        Ok(json!({
            "ok": true,
            "selected_folder": folder,
            "selected_folder_id": folder_id,
            "selection_scope": "conversation",
            "conversation_isolated": true,
            "history_dir": Path::new(&folder.path).join("docs/history-session").display().to_string(),
            "next_action": "Call history_session_bootstrap after selecting a folder for a new conversation."
        }))
    }

    fn resolve_folder_id_locked(
        &self,
        state: &mut RoutingState,
        host_session_key: Option<&str>,
    ) -> Result<String, String> {
        let Some(session_key) = host_session_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Err(
                "WORKSPACE_FOLDER_NOT_SELECTED: 缺少 MCP conversation/session identity；未明確選取資料夾前不得存取專案內容。"
                    .into(),
            );
        };
        state
            .session_folders
            .get(session_key)
            .cloned()
            .ok_or_else(|| {
                "WORKSPACE_FOLDER_NOT_SELECTED: 此 session 尚未選取資料夾；請先呼叫 list_workspace_folders，再呼叫 switch_workspace_folder。"
                    .to_string()
            })
    }

    fn context_for_folder(
        &self,
        folder_id: &str,
        host_session_key: Option<&str>,
    ) -> Result<SharedToolContext, String> {
        let cache_key = context_cache_key(folder_id, host_session_key);
        let (folder, saved_cwd) = {
            let mut state = lock_state(self);
            let folder = state
                .folders
                .iter()
                .find(|folder| folder.id == folder_id)
                .cloned()
                .ok_or_else(|| format!("資料夾不在允許清單內：{folder_id}"))?;
            if let Some(context) = state.contexts.get(&cache_key).cloned() {
                if same_path(&folder.path, &context.workspace_path()) {
                    touch_context_locked(&mut state, &cache_key);
                    return Ok(context);
                }
            }
            let saved_cwd = state.saved_cwds.get(&cache_key).cloned();
            (folder, saved_cwd)
        };

        let workspace =
            Workspace::new_with_execution(PathBuf::from(&folder.path), folder.execution.clone())
                .map_err(|error| error.message())?;
        let created = Arc::new(
            ToolContext::from_workspace_with_shared_runtime_config_and_resource_ids_and_limits(
                workspace,
                self.config.auth.clone(),
                self.config.runtime_config.clone(),
                self.profile_id.clone(),
                format!(
                    "{}--{}--{}",
                    self.profile_id, self.config.execution_resource_namespace, folder.id
                ),
                format!(
                    "{}--{}",
                    self.profile_id, self.config.execution_resource_namespace
                ),
                self.config.limits,
            ),
        );
        if let Some(saved_cwd) = saved_cwd {
            if saved_cwd.is_dir() && saved_cwd.starts_with(created.workspace.root()) {
                created.set_default_cwd(saved_cwd);
            }
        }

        let mut state = lock_state(self);
        let current_folder = state
            .folders
            .iter()
            .find(|candidate| candidate.id == folder_id)
            .cloned()
            .ok_or_else(|| format!("資料夾不在允許清單內：{folder_id}"))?;
        if !same_path(&folder.path, &current_folder.path) {
            return Err(format!("資料夾設定已變更，請重試：{folder_id}"));
        }
        if let Some(context) = state.contexts.get(&cache_key).cloned() {
            if same_path(&current_folder.path, &context.workspace_path()) {
                touch_context_locked(&mut state, &cache_key);
                return Ok(context);
            }
        }
        state.contexts.insert(cache_key.clone(), created.clone());
        touch_context_locked(&mut state, &cache_key);
        prune_conversation_contexts_locked(&mut state);
        Ok(created)
    }
}

fn touch_context_locked(state: &mut RoutingState, cache_key: &str) {
    state.context_access_clock = state.context_access_clock.wrapping_add(1);
    state
        .context_last_used
        .insert(cache_key.to_string(), state.context_access_clock);
}

fn prune_conversation_contexts_locked(state: &mut RoutingState) {
    let conversation_count = state
        .contexts
        .keys()
        .filter(|cache_key| cache_key.contains('\u{0}'))
        .count();
    let remove_count = conversation_count.saturating_sub(MAX_CONVERSATION_CONTEXTS);
    if remove_count == 0 {
        return;
    }
    let mut candidates = state
        .contexts
        .keys()
        .filter(|cache_key| cache_key.contains('\u{0}'))
        .map(|cache_key| {
            (
                cache_key.clone(),
                state.context_last_used.get(cache_key).copied().unwrap_or(0),
            )
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(_, last_used)| *last_used);
    for (cache_key, _) in candidates.into_iter().take(remove_count) {
        if let Some(context) = state.contexts.remove(&cache_key) {
            state
                .saved_cwds
                .insert(cache_key.clone(), context.default_cwd_path());
        }
        state.context_last_used.remove(&cache_key);
    }
}

fn context_cache_key(folder_id: &str, host_session_key: Option<&str>) -> String {
    match host_session_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(session_key) => format!("{folder_id}\u{0}{session_key}"),
        None => folder_id.to_string(),
    }
}

fn context_folder_id(cache_key: &str) -> &str {
    cache_key.split('\u{0}').next().unwrap_or(cache_key)
}

fn validate_unique_folder_paths(folders: &[WorkspaceFolder]) -> Result<(), String> {
    for (index, folder) in folders.iter().enumerate() {
        for other in folders.iter().skip(index + 1) {
            if same_path(&folder.path, &other.path) {
                return Err(format!(
                    "同一實體資料夾不可使用多個 folder_id：{} 與 {} 都指向 {}",
                    folder.id, other.id, folder.path
                ));
            }
        }
    }
    Ok(())
}

fn folder_id_if_allowed(folders: &[WorkspaceFolder], requested: &str) -> Result<String, String> {
    folders
        .iter()
        .find(|folder| folder.id == requested)
        .map(|folder| folder.id.clone())
        .ok_or_else(|| format!("資料夾不在允許清單內：{requested}"))
}

fn action_session_id<'a>(tool_name: &str, arguments: &'a Value) -> Option<&'a str> {
    if matches!(tool_name, "wait_command" | "send_input" | "kill_session") {
        return arguments.get("session_id").and_then(Value::as_str);
    }
    if tool_name == "read_output" {
        return arguments
            .get("output_ref")
            .and_then(Value::as_str)
            .and_then(parse_output_session_id);
    }
    None
}

fn parse_output_session_id(output_ref: &str) -> Option<&str> {
    output_ref
        .strip_prefix("output://")
        .and_then(|value| value.split('/').next())
        .filter(|value| !value.is_empty())
}

fn action_result_session_ids(result: &Value) -> Vec<String> {
    let mut session_ids = Vec::new();
    if let Some(session_id) = result
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        session_ids.push(session_id.to_string());
    }
    if let Some(output_refs) = result.get("output_refs").and_then(Value::as_object) {
        for output_ref in output_refs.values().filter_map(Value::as_str) {
            if let Some(session_id) = parse_output_session_id(output_ref) {
                session_ids.push(session_id.to_string());
            }
        }
    }
    session_ids.sort();
    session_ids.dedup();
    session_ids
}

fn unique_context_match(
    state: &RoutingState,
    predicate: impl Fn(&SharedToolContext) -> bool,
    identifier_name: &str,
) -> Result<Option<String>, String> {
    let mut matches = state
        .contexts
        .iter()
        .filter(|(_, context)| predicate(context))
        .map(|(cache_key, _)| context_folder_id(cache_key).to_string())
        .collect::<Vec<_>>();
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [] => Ok(None),
        [folder_id] => Ok(Some(folder_id.clone())),
        _ => Err(format!(
            "同一個 {identifier_name} 出現在多個資料夾上下文，請明確提供 workspace_folder_id。"
        )),
    }
}

fn unique_conversation_context_match(
    state: &RoutingState,
    session_key: &str,
    predicate: impl Fn(&SharedToolContext) -> bool,
    identifier_name: &str,
) -> Result<Option<String>, String> {
    let suffix = format!("\u{0}{session_key}");
    let mut matches = state
        .contexts
        .iter()
        .filter(|(cache_key, context)| cache_key.ends_with(&suffix) && predicate(context))
        .map(|(cache_key, _)| context_folder_id(cache_key).to_string())
        .collect::<Vec<_>>();
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [] => Ok(None),
        [folder_id] => Ok(Some(folder_id.clone())),
        _ => Err(format!(
            "同一個 {identifier_name} 出現在目前 conversation 的多個資料夾上下文，請明確提供 workspace_folder_id。"
        )),
    }
}

fn same_path(left: &str, right: &str) -> bool {
    if let Some(equal) = compare_wsl_paths(left, right) {
        return equal;
    }
    let left = normalized_path(left);
    let right = normalized_path(right);
    #[cfg(windows)]
    {
        left.eq_ignore_ascii_case(&right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn normalized_path(path: &str) -> String {
    let normalized = path.trim_end_matches(['\\', '/']).replace('\\', "/");
    #[cfg(windows)]
    {
        normalized
            .strip_prefix("//?/")
            .unwrap_or(&normalized)
            .to_string()
    }
    #[cfg(not(windows))]
    {
        normalized
    }
}

pub fn routing_folder_options(listing: &Value) -> Value {
    let folders = listing
        .get("folders")
        .and_then(Value::as_array)
        .map(|folders| {
            folders
                .iter()
                .map(|folder| {
                    json!({
                        "id": folder.get("id").cloned().unwrap_or(Value::Null),
                        "name": folder.get("name").cloned().unwrap_or(Value::Null),
                        "path": folder.get("path").cloned().unwrap_or(Value::Null)
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Value::Array(folders)
}

pub fn routing_error_parts<'a>(
    message: &'a str,
    fallback_code: &'static str,
) -> (&'static str, &'a str) {
    const NOT_SELECTED_PREFIX: &str = "WORKSPACE_FOLDER_NOT_SELECTED: ";
    match message.strip_prefix(NOT_SELECTED_PREFIX) {
        Some(message) => ("WORKSPACE_FOLDER_NOT_SELECTED", message),
        None => (fallback_code, message),
    }
}

fn hub_error(code: &str, message: &str, retryable: bool) -> Value {
    json!({
        "ok": false,
        "error": {
            "code": code,
            "message": message,
            "category": "workspace_routing",
            "retryable": retryable
        }
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::tools::tool_runtime::MutationLockGroup;

    fn test_config(namespace: &str) -> HubConfig {
        HubConfig {
            auth: AuthConfig::default(),
            runtime_config: SharedRuntimeToolConfig::new(
                crate::tools::policy::PolicySettings::default(),
                "full".into(),
                "trusted".into(),
            ),
            limits: ExecutionLimits::default(),
            execution_resource_namespace: namespace.into(),
        }
    }

    #[test]
    fn wsl_context_paths_preserve_linux_case() {
        assert!(same_path(
            r"\\wsl$\Ubuntu-24.04\opt\src\SampleProject",
            r"\\wsl.localhost\ubuntu-24.04\opt\src\SampleProject"
        ));
        assert!(!same_path(
            r"\\wsl.localhost\Ubuntu-24.04\opt\src\SampleProject",
            r"\\wsl.localhost\Ubuntu-24.04\opt\src\sampleproject"
        ));
    }

    #[tokio::test]
    async fn action_session_controls_follow_the_origin_folder() {
        let first = tempfile::tempdir().expect("first folder");
        let second = tempfile::tempdir().expect("second folder");
        let folders = vec![
            WorkspaceFolder {
                id: "first".into(),
                name: "First".into(),
                path: first.path().display().to_string(),
                execution: Default::default(),
            },
            WorkspaceFolder {
                id: "second".into(),
                name: "Second".into(),
                path: second.path().display().to_string(),
                execution: Default::default(),
            },
        ];
        let router = HubRouter::new(
            "actions-routing-test".into(),
            folders,
            test_config("actions-test"),
        )
        .expect("router");

        assert!(router
            .resolve_action_context(None, "read_file", &json!({"path": "README.md"}))
            .is_err());
        let routed = router
            .resolve_action_context(Some("second"), "exec_command", &json!({}))
            .expect("explicit second folder");

        #[cfg(windows)]
        let child = tokio::process::Command::new("cmd")
            .args(["/C", "exit", "0"])
            .spawn()
            .expect("spawn test child");
        #[cfg(unix)]
        let child = tokio::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn test child");
        let session = routed
            .context
            .sessions
            .insert(crate::tools::session::ExecSession::new(child));
        let session_id = session.session_id.clone();

        let wait_route = router
            .resolve_action_context(None, "wait_command", &json!({"session_id": session_id}))
            .expect("wait route");
        assert_eq!(wait_route.folder_id, "second");
        let output_route = router
            .resolve_action_context(
                None,
                "read_output",
                &json!({"output_ref": format!("output://{}/stdout", session.session_id)}),
            )
            .expect("output route");
        assert_eq!(output_route.folder_id, "second");
        assert!(router
            .resolve_action_context(
                None,
                "kill_session",
                &json!({"session_id": "missing-session"}),
            )
            .is_err());
    }

    #[test]
    fn concurrent_context_creation_publishes_one_shared_context() {
        let workspace = tempfile::tempdir().expect("workspace");
        let router = HubRouter::new(
            "context-race-test".into(),
            vec![WorkspaceFolder {
                id: "workspace".into(),
                name: "Workspace".into(),
                path: workspace.path().display().to_string(),
                execution: Default::default(),
            }],
            test_config("race-test"),
        )
        .expect("router");
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let contexts = std::thread::scope(|scope| {
            let handles = (0..16)
                .map(|_| {
                    let router = router.clone();
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        router
                            .context_for_folder("workspace", Some("shared-conversation"))
                            .expect("context")
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("join"))
                .collect::<Vec<_>>()
        });
        for context in contexts.iter().skip(1) {
            assert!(Arc::ptr_eq(&contexts[0], context));
        }
    }

    #[test]
    fn sync_hot_applies_runtime_config_to_cached_and_new_contexts() {
        let workspace = tempfile::tempdir().expect("workspace");
        let folders = vec![WorkspaceFolder {
            id: "workspace".into(),
            name: "Workspace".into(),
            path: workspace.path().display().to_string(),
            execution: Default::default(),
        }];
        let router = HubRouter::new(
            "runtime-sync-test".into(),
            folders.clone(),
            test_config("runtime-sync"),
        )
        .expect("router");
        let cached = router
            .context_for_folder("workspace", Some("cached-conversation"))
            .expect("cached context");
        assert_eq!(cached.runtime_config().tool_profile, "trusted-core");

        let mut runtime = crate::workspace::RuntimeConfig::default();
        runtime.tool_profile = "advanced".into();
        runtime.permission_mode = "dangerous".into();
        runtime.allowed_commands = "node,git".into();
        runtime.workspace_local_entries = false;
        runtime.workspace_script_extensions = ".js,.ts".into();
        runtime.sandbox.enabled = false;
        runtime.sandbox.backend = "docker".into();
        router
            .sync(folders.clone(), &runtime)
            .expect("hot apply runtime");

        let updated = cached.runtime_config();
        assert_eq!(updated.tool_profile, "advanced");
        assert_eq!(updated.permission_mode, "dangerous");
        assert_eq!(updated.policy.permission_mode, "dangerous");
        assert!(!updated.policy.workspace_local_entries);
        assert!(updated.policy.allowed_commands.contains("node"));
        assert!(updated.policy.workspace_script_extensions.contains(".ts"));
        assert!(!updated.sandbox.enabled);
        assert_eq!(updated.sandbox.backend, "docker");

        let cached_again = router
            .context_for_folder("workspace", Some("cached-conversation"))
            .expect("same cached context");
        assert!(Arc::ptr_eq(&cached, &cached_again));
        let created_after_update = router
            .context_for_folder("workspace", Some("new-conversation"))
            .expect("new context");
        assert_eq!(
            created_after_update.runtime_config().tool_profile,
            "advanced"
        );
        assert_eq!(
            created_after_update.runtime_config().sandbox.backend,
            "docker"
        );

        runtime.tool_profile = "read-only".into();
        runtime.permission_mode = "read-only".into();
        runtime.allowed_commands = "python".into();
        runtime.sandbox.backend = "podman".into();
        router.sync(folders, &runtime).expect("second hot apply");
        for context in [&cached, &created_after_update] {
            let current = context.runtime_config();
            assert_eq!(current.tool_profile, "read-only");
            assert_eq!(current.permission_mode, "read-only");
            assert!(current.policy.allowed_commands.contains("python"));
            assert_eq!(current.sandbox.backend, "podman");
        }
    }

    #[test]
    fn sync_hot_applies_sandbox_enablement_after_preflight() {
        let workspace = tempfile::tempdir().expect("workspace");
        let folders = vec![WorkspaceFolder {
            id: "workspace".into(),
            name: "Workspace".into(),
            path: workspace.path().display().to_string(),
            execution: Default::default(),
        }];
        let router = HubRouter::new(
            "sandbox-enable-hot-apply-test".into(),
            folders.clone(),
            test_config("sandbox-enable-hot-apply"),
        )
        .expect("router");
        let cached = router
            .context_for_folder("workspace", Some("cached-conversation"))
            .expect("cached context");
        let before = cached.runtime_config().sandbox;
        assert!(!before.enabled);

        let mut runtime = crate::workspace::RuntimeConfig::default();
        runtime.sandbox.enabled = true;
        runtime.sandbox.backend = "appcontainer".into();
        router
            .sync_preflighted(folders, &runtime)
            .expect("sync preflighted runtime");

        let after = cached.runtime_config().sandbox;
        assert!(after.enabled);
        assert_eq!(after.backend, "appcontainer");
        assert_ne!(after, before);
    }

    #[test]
    fn sandbox_preflight_only_targets_folders_with_live_contexts() {
        let first = tempfile::tempdir().expect("first workspace");
        let second = tempfile::tempdir().expect("second workspace");
        let folders = vec![
            WorkspaceFolder {
                id: "first".into(),
                name: "First".into(),
                path: first.path().display().to_string(),
                execution: Default::default(),
            },
            WorkspaceFolder {
                id: "second".into(),
                name: "Second".into(),
                path: second.path().display().to_string(),
                execution: Default::default(),
            },
        ];
        let router = HubRouter::new(
            "sandbox-live-preflight-test".into(),
            folders.clone(),
            test_config("sandbox-live-preflight"),
        )
        .expect("router");

        assert!(router.live_preflight_folders(&folders).is_empty());
        let first_context = router
            .context_for_folder("first", Some("first-conversation"))
            .expect("first context");
        assert_eq!(
            router
                .live_preflight_folders(&folders)
                .into_iter()
                .map(|folder| folder.id)
                .collect::<Vec<_>>(),
            vec!["first"]
        );
        drop(first_context);
        assert!(
            router.live_preflight_folders(&folders).is_empty(),
            "a conversation context retained only by the routing cache must not block preflight"
        );

        let bootstrap = router
            .context_for_folder("first", None)
            .expect("bootstrap context");
        drop(bootstrap);
        assert_eq!(
            router
                .live_preflight_folders(&folders)
                .into_iter()
                .map(|folder| folder.id)
                .collect::<Vec<_>>(),
            vec!["first"],
            "bootstrap context must be preflighted even when only the routing cache holds it"
        );

        let second_context = router
            .context_for_folder("second", Some("second-conversation"))
            .expect("second context");
        let mut live_ids = router
            .live_preflight_folders(&folders)
            .into_iter()
            .map(|folder| folder.id)
            .collect::<Vec<_>>();
        live_ids.sort();
        assert_eq!(live_ids, vec!["first", "second"]);
        drop(second_context);
    }

    #[test]
    fn sandbox_preflight_does_not_touch_stale_inactive_contexts() {
        let root = tempfile::tempdir().expect("workspace root");
        let live = root.path().join("live");
        let inactive = root.path().join("inactive");
        fs::create_dir_all(&live).expect("live workspace");
        fs::create_dir_all(&inactive).expect("inactive workspace");
        let folders = vec![
            WorkspaceFolder {
                id: "inactive".into(),
                name: "Inactive".into(),
                path: inactive.display().to_string(),
                execution: Default::default(),
            },
            WorkspaceFolder {
                id: "live".into(),
                name: "Live".into(),
                path: live.display().to_string(),
                execution: Default::default(),
            },
        ];
        let router = HubRouter::new(
            "sandbox-inactive-preflight-test".into(),
            folders.clone(),
            test_config("sandbox-inactive-preflight"),
        )
        .expect("router");
        let stale_context = router
            .context_for_folder("inactive", Some("stale-conversation"))
            .expect("stale context");
        drop(stale_context);
        fs::remove_dir_all(&inactive).expect("remove inactive workspace");

        let _live_context = router
            .context_for_folder("live", Some("live-conversation"))
            .expect("live context");

        let mut runtime = crate::workspace::RuntimeConfig::default();
        runtime.sandbox.enabled = true;
        runtime.sandbox.backend = "missing-backend".into();
        let error = router
            .preflight(&folders, &runtime)
            .expect_err("live backend must still be preflighted");
        assert!(
            error.contains("Sandbox backend is not registered: missing-backend"),
            "stale inactive context was touched before the live backend: {error}"
        );
    }

    #[test]
    fn sandbox_preflight_failure_keeps_live_generation_unchanged() {
        let workspace = tempfile::tempdir().expect("workspace");
        let folders = vec![WorkspaceFolder {
            id: "workspace".into(),
            name: "Workspace".into(),
            path: workspace.path().display().to_string(),
            execution: Default::default(),
        }];
        let router = HubRouter::new(
            "sandbox-preflight-failure-test".into(),
            folders.clone(),
            test_config("sandbox-preflight-failure"),
        )
        .expect("router");
        let cached = router
            .context_for_folder("workspace", Some("cached-conversation"))
            .expect("cached context");
        let before = cached.runtime_config().sandbox;

        let mut runtime = crate::workspace::RuntimeConfig::default();
        runtime.sandbox.enabled = true;
        runtime.sandbox.backend = "unknown-hot-toggle-backend".into();
        assert!(router.preflight(&folders, &runtime).is_err());
        assert_eq!(cached.runtime_config().sandbox, before);
    }

    #[test]
    fn sessions_require_explicit_selection_and_remember_cwd() {
        let first = tempfile::tempdir().expect("first folder");
        let second = tempfile::tempdir().expect("second folder");
        fs::create_dir_all(second.path().join("docs/history-session")).expect("history dir");
        fs::write(
            second.path().join("docs/history-session/index.json"),
            r#"{"sessions":{"history-only-session":{"number":1,"path":"docs/history-session/1.md"}}}"#,
        )
        .expect("history index");

        let folders = vec![
            WorkspaceFolder {
                id: "first".into(),
                name: "First".into(),
                path: first.path().display().to_string(),
                execution: Default::default(),
            },
            WorkspaceFolder {
                id: "second".into(),
                name: "Second".into(),
                path: second.path().display().to_string(),
                execution: Default::default(),
            },
        ];
        let router = HubRouter::new(
            "explicit-routing-test".into(),
            folders,
            test_config("explicit-test"),
        )
        .expect("router");

        let listing = router
            .folder_listing(Some("new-conversation"))
            .expect("folder listing");
        assert!(listing.get("default_folder_id").is_none());
        assert!(listing["selected_folder_id"].is_null());
        assert_eq!(listing["selection_scope"], "unselected");
        assert!(listing["folders"]
            .as_array()
            .expect("folders")
            .iter()
            .all(|folder| folder["selected"] == false));

        let history_error = match router.resolve(Some("history-only-session")) {
            Ok(_) => panic!("history must not select a folder"),
            Err(error) => error,
        };
        assert!(history_error.starts_with("WORKSPACE_FOLDER_NOT_SELECTED:"));
        assert!(router.resolve(None).is_err());
        assert!(router.switch("first", None).is_err());
        assert!(router
            .resolve_action_context(None, "read_file", &json!({"path": "README.md"}))
            .is_err());
        assert!(router
            .resolve_action_context(None, "request_permissions", &json!({}))
            .is_err());

        let explicit_action = router
            .resolve_action_context(Some("second"), "read_file", &json!({"path": "README.md"}))
            .expect("explicit action");
        assert_eq!(explicit_action.folder_id, "second");
        let pending = explicit_action.context.pending_operations.insert(
            "exec_command",
            &json!({"cmd": "echo test"}),
            "shell_expansion",
            "test",
            std::time::Duration::from_secs(60),
        );
        let resumed_action = router
            .resolve_action_context(
                None,
                "request_permissions",
                &json!({"resume_id": pending.resume_id}),
            )
            .expect("resume action routing");
        assert_eq!(resumed_action.folder_id, "second");

        router.switch("first", Some("cwd-a")).expect("bind cwd a");
        router.switch("first", Some("cwd-b")).expect("bind cwd b");
        let conversation_a = router.resolve(Some("cwd-a")).expect("cwd a");
        let conversation_b = router.resolve(Some("cwd-b")).expect("cwd b");
        assert!(!Arc::ptr_eq(&conversation_a, &conversation_b));
        assert!(Arc::ptr_eq(
            &conversation_a.mutation_lock_for(MutationLockGroup::WorkspaceContent),
            &conversation_b.mutation_lock_for(MutationLockGroup::WorkspaceContent)
        ));
        let scoped_subdir = first.path().join("scoped-subdir");
        fs::create_dir_all(&scoped_subdir).expect("scoped cwd");
        conversation_a.set_default_cwd(scoped_subdir.canonicalize().expect("canonical scoped cwd"));
        assert_eq!(conversation_a.default_cwd_display(), "scoped-subdir");
        assert_eq!(
            conversation_b.default_cwd_path(),
            conversation_b.workspace.root()
        );

        for index in 0..(MAX_CONVERSATION_CONTEXTS + 16) {
            let session_key = format!("lru-conversation-{index}");
            router
                .switch("first", Some(&session_key))
                .expect("bind lru session");
        }
        {
            let state = lock_state(&router);
            assert_eq!(
                state
                    .contexts
                    .keys()
                    .filter(|cache_key| cache_key.contains('\u{0}'))
                    .count(),
                MAX_CONVERSATION_CONTEXTS
            );
            assert!(state
                .saved_cwds
                .contains_key(&context_cache_key("first", Some("cwd-a"))));
        }

        let restored_cwd = router.resolve(Some("cwd-a")).expect("restore cwd a");
        assert_eq!(restored_cwd.default_cwd_display(), "scoped-subdir");

        router.record_action_result(
            "second",
            &json!({
                "session_id": "indexed-session",
                "resume_id": "indexed-resume",
                "output_refs": {"stdout": "output://indexed-output/stdout"}
            }),
        );
        assert_eq!(
            router
                .resolve_action_context(
                    None,
                    "wait_command",
                    &json!({"session_id": "indexed-session"}),
                )
                .expect("indexed session route")
                .folder_id,
            "second"
        );
        assert_eq!(
            router
                .resolve_action_context(
                    None,
                    "read_output",
                    &json!({"output_ref": "output://indexed-output/stdout"}),
                )
                .expect("indexed output route")
                .folder_id,
            "second"
        );
        assert_eq!(
            router
                .resolve_action_context(
                    None,
                    "request_permissions",
                    &json!({"resume_id": "indexed-resume"}),
                )
                .expect("indexed resume route")
                .folder_id,
            "second"
        );

        router
            .switch("second", Some("new-conversation"))
            .expect("switch");
        let switched = router.resolve(Some("new-conversation")).expect("switched");
        assert_eq!(
            switched.workspace.root(),
            second.path().canonicalize().unwrap()
        );
    }
}
