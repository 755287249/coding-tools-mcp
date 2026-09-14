use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::tasks::{
    client_supports_tasks, conversation_key as task_conversation_key, create_process_task,
    detailed_task, mark_cancelled, require_task, task_error, update_from_snapshot, TASKS_EXTENSION,
};
use crate::tools::{
    call_tool, call_tool_async, call_tool_async_with_canary_sample_key, list_tools_for_profile,
    wrap_mcp_tool_result, ExecutionLimits, SharedRuntimeToolConfig, SharedToolContext, ToolContext,
    Workspace,
};
use crate::workspace::{AuthConfig, SandboxConfig, WorkspaceFolder};

pub type SharedState = SharedToolContext;

pub const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
pub const LATEST_LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";
pub const LATEST_PROTOCOL_VERSION: &str = MODERN_PROTOCOL_VERSION;
pub const LEGACY_PROTOCOL_VERSIONS: &[&str] =
    &[LATEST_LEGACY_PROTOCOL_VERSION, "2025-06-18", "2025-03-26"];
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    LATEST_PROTOCOL_VERSION,
    LATEST_LEGACY_PROTOCOL_VERSION,
    "2025-06-18",
    "2025-03-26",
];

pub fn is_supported_protocol_version(version: &str) -> bool {
    SUPPORTED_PROTOCOL_VERSIONS.contains(&version)
}

pub fn handle_request(state: &SharedState, body: &Value) -> Value {
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let params = body.get("params").cloned().unwrap_or(Value::Null);

    if id.is_null() && method.starts_with("notifications/") {
        return Value::Null;
    }

    let runtime = state.runtime_config();
    let result = match method {
        "initialize" => Ok(initialize_result(
            params.get("protocolVersion").and_then(Value::as_str),
            &runtime.tool_profile,
        )),
        "server/discover" => Ok(discover_result()),
        "tasks/get" | "tasks/update" | "tasks/cancel" => {
            handle_task_request(state, method, &params)
        }
        "ping" => Ok(serde_json::json!({})),
        "tools/list" => {
            let tools = list_tools_for_profile(&runtime.tool_profile);
            Ok(serde_json::json!({
                "tools": tools,
                "toolsetRevision": crate::tools::registry::toolset_revision(&runtime.tool_profile)
            }))
        }
        "prompts/list" => crate::workspace_features::list_skill_prompts(&state.profile_id)
            .map_err(|message| crate::workspace_features::skill_rpc_error(method, message)),
        "prompts/get" => params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| json!({ "code": -32602, "message": "Missing prompt name" }))
            .and_then(|name| {
                crate::workspace_features::get_skill_prompt_for_session(
                    &state.profile_id,
                    name,
                    host_session_key(&params),
                )
                .map_err(|message| crate::workspace_features::skill_rpc_error(method, message))
            }),
        "resources/list" => list_resources(state),
        "resources/read" => params
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| json!({ "code": -32602, "message": "Missing resource URI" }))
            .and_then(|uri| read_resource(state, uri, host_session_key(&params))),
        "tools/call" => handle_tools_call(state, &params),
        _ => Err(serde_json::json!({
            "code": -32601,
            "message": format!("Method not found: {method}")
        })),
    };

    match result {
        Ok(result) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    }
}

pub async fn handle_request_async(state: SharedState, body: Value) -> Value {
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    if !matches!(
        method,
        "tools/call" | "tools/list" | "tasks/get" | "tasks/cancel"
    ) {
        return handle_request(&state, &body);
    }

    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let params = body.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "tools/call" => handle_tools_call_async(state, params).await,
        "tools/list" => Ok(list_tools_dynamic(&state).await),
        _ => handle_task_request_async(state, method, params).await,
    };
    match result {
        Ok(result) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    }
}

fn list_resources(state: &SharedState) -> Result<Value, Value> {
    let skills = crate::workspace_features::list_skill_resources(&state.profile_id)
        .map_err(|message| crate::workspace_features::skill_rpc_error("resources/list", message))?;
    let evolution = crate::knowledge::list_tool_evolution_resources(state)?;
    Ok(crate::knowledge::merge_resource_lists(skills, evolution))
}

fn read_resource(
    state: &SharedState,
    uri: &str,
    session_key: Option<&str>,
) -> Result<Value, Value> {
    if crate::knowledge::is_tool_evolution_resource_uri(uri) {
        crate::knowledge::read_tool_evolution_resource(state, uri)
    } else {
        crate::workspace_features::read_skill_resource_for_session(
            &state.profile_id,
            uri,
            session_key,
        )
        .map_err(|message| crate::workspace_features::skill_rpc_error("resources/read", message))
    }
}

async fn list_tools_dynamic(state: &SharedState) -> Value {
    let runtime = state.runtime_config();
    let mut tools = list_tools_for_profile(&runtime.tool_profile);
    let external = crate::workspace_features::list_external_tools(&state.profile_id).await;
    for tool in &external {
        tools.push(tool.definition.clone());
    }
    let revision_material = json!({
        "base": crate::tools::registry::toolset_revision(&runtime.tool_profile),
        "external": external.iter().map(|tool| &tool.definition).collect::<Vec<_>>()
    });
    let revision = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&revision_material).unwrap_or_default())
    );
    json!({ "tools": tools, "toolsetRevision": revision })
}

fn discover_result() -> Value {
    let mut result = serde_json::json!({
        "supportedVersions": [MODERN_PROTOCOL_VERSION],
        "capabilities": {
            "tools": { "listChanged": false },
            "prompts": { "listChanged": false },
            "resources": { "subscribe": false, "listChanged": false },
            "extensions": {}
        },
        "instructions": "Use these tools only for local coding operations inside the configured tool hub. Call conversation_bootstrap before project tools. Workspace and enabled Codex/Claude user-level Skills are exposed through standard MCP prompts and resources. Enabled Hooks may block or rewrite tool calls, and enabled external MCP servers contribute proxied tools to tools/list."
    });
    result["capabilities"]["extensions"][TASKS_EXTENSION] = json!({});
    result
}

fn initialize_result(requested_version: Option<&str>, tool_profile: &str) -> Value {
    let protocol_version = requested_version
        .filter(|version| LEGACY_PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(LATEST_LEGACY_PROTOCOL_VERSION);
    serde_json::json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": { "listChanged": false },
            "prompts": { "listChanged": false },
            "resources": { "subscribe": false, "listChanged": false },
            "logging": {}
        },
        "serverInfo": {
            "name": "coding-tools-mcp",
            "title": "Coding Tools MCP",
            "version": env!("CARGO_PKG_VERSION"),
            "toolsetRevision": crate::tools::registry::toolset_revision(tool_profile)
        },
        "instructions": "Use these tools only for local coding operations inside the configured tool hub. A tool hub may contain multiple allowed folders while sharing one MCP endpoint. At the start of every new ChatGPT conversation, before accessing project content, call conversation_bootstrap. It reuses an existing conversation folder, auto-binds the only configured folder, or returns available folder choices when multiple folders are unselected; in the ambiguous case retry conversation_bootstrap with folder_id. It also performs compact history_session_bootstrap, so the normal startup path is one tool call. The legacy list_workspace_folders, switch_workspace_folder, then history_session_bootstrap sequence remains available for manual recovery. There is no history-based folder fallback, and ambiguous multi-folder conversations remain unselected until explicitly bound. The selected folder and default cwd are remembered for the same runtime session without affecting other conversations. Workspace and enabled Codex/Claude user-level Skills are exposed through standard MCP prompts and resources. After workspace selection, use the lightweight skill summaries returned by conversation_bootstrap to identify a clearly relevant Skill, then load only that Skill through prompts/get or resources/read. Skills are workflow guidance and never grant permissions or weaken tool, sandbox, or workspace policy. Enabled Hooks may block or rewrite tool calls, and enabled external MCP servers contribute proxied tools to tools/list. Tools whose schema exposes workspace_folder_id may route one call to another allowed folder without changing the conversation selection; control calls can also recover their original folder from session_id, output_ref, or resume_id. Preserve session_key and current_path returned by bootstrap, then pass them unchanged as session_key and expected_path to every history_session_checkpoint call. After completing each user-requested task in the conversation, call history_session_checkpoint before the final response. Only state that progress was saved after checkpoint returns ok=true with the same session_key and path. Prefer exec_many(mode=auto) over sequential exec_command calls when two or more independent commands are known in the same reasoning step. Persistence requires a successful tool call and is not automatic background persistence."
    })
}

fn conversation_bootstrap(
    state: &SharedState,
    args: &Value,
    host_session_key: Option<&str>,
) -> Result<Value, Value> {
    let listing = crate::tools::hub::list_workspace_folders(state, host_session_key);
    if listing.get("ok").and_then(Value::as_bool) == Some(false) {
        return Ok(listing);
    }
    let requested = args
        .get("folder_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let selected = listing
        .get("selected_folder_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let only_folder = listing
        .get("folders")
        .and_then(Value::as_array)
        .filter(|folders| folders.len() == 1)
        .and_then(|folders| folders.first())
        .and_then(|folder| folder.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(folder_id) = requested.or(selected).or(only_folder) else {
        let mut response = listing;
        if let Some(object) = response.as_object_mut() {
            object.insert("needs_folder_selection".into(), Value::Bool(true));
            object.insert(
                "next_action".into(),
                serde_json::json!({
                    "tool": "conversation_bootstrap",
                    "required_arguments": ["folder_id"],
                    "suggestion": "Choose one folders entry and retry conversation_bootstrap with its id."
                }),
            );
        }
        return Ok(response);
    };

    let routing = crate::tools::hub::switch_workspace_folder(state, folder_id, host_session_key);
    if routing.get("ok").and_then(Value::as_bool) == Some(false) {
        return Ok(routing);
    }
    let selected_context = crate::tools::hub::resolve_context(state.clone(), host_session_key)
        .map_err(|message| workspace_routing_error(state, host_session_key, message))?;
    let mut history_args = args.clone();
    if let Some(object) = history_args.as_object_mut() {
        object.remove("folder_id");
    }
    let mut history = call_tool(
        selected_context.as_ref(),
        "history_session_bootstrap",
        &history_args,
    );
    if let (Some(history), Some(routing)) = (history.as_object_mut(), routing.as_object()) {
        for field in [
            "selected_folder_id",
            "selected_folder",
            "selection_scope",
            "conversation_isolated",
            "history_dir",
        ] {
            if let Some(value) = routing.get(field) {
                history.insert(field.into(), value.clone());
            }
        }
        history.insert("needs_folder_selection".into(), Value::Bool(false));
        history.insert(
            "startup_flow".into(),
            Value::String("workspace_and_history_bootstrapped".into()),
        );
        history.insert(
            "project_skills".into(),
            crate::workspace_features::skill_bootstrap_summary(&state.profile_id, folder_id)
                .unwrap_or_else(|_| serde_json::json!({ "count": 0, "skills": [] })),
        );
        history.insert(
            "learning".into(),
            crate::knowledge::learning_bootstrap_summary(selected_context.as_ref()).unwrap_or_else(
                |_| {
                    serde_json::json!({
                        "state": "metadata_unavailable",
                        "runtime_policy": "validated_canary_rust_parity",
                        "knowledge_loading_policy": "Persistent knowledge is not injected into inference context."
                    })
                },
            ),
        );
        history.insert(
            "legacy_startup_fallback".into(),
            serde_json::json!([
                "list_workspace_folders",
                "switch_workspace_folder",
                "history_session_bootstrap"
            ]),
        );
    }
    Ok(history)
}

const PERMISSION_MRTR_RESPONSE_KEY: &str = "permission_approval";
const PERMISSION_MRTR_STATE_PREFIX: &str = "permission:";

#[derive(Debug)]
struct PermissionMrtrRetry {
    resume_id: String,
    approved: bool,
}

fn permission_mrtr_retry(params: &Value) -> Result<Option<PermissionMrtrRetry>, Value> {
    let Some(request_state) = params.get("requestState").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(resume_id) = request_state.strip_prefix(PERMISSION_MRTR_STATE_PREFIX) else {
        return Ok(None);
    };
    let resume_id = resume_id.trim();
    if resume_id.is_empty() {
        return Err(
            json!({ "code": -32602, "message": "MRTR permission requestState is missing its resume identifier" }),
        );
    }
    let response = params
        .get("inputResponses")
        .and_then(Value::as_object)
        .and_then(|responses| responses.get(PERMISSION_MRTR_RESPONSE_KEY))
        .and_then(Value::as_object)
        .ok_or_else(|| json!({
            "code": -32602,
            "message": format!("MRTR permission response '{PERMISSION_MRTR_RESPONSE_KEY}' is required")
        }))?;
    let action = response
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            json!({
                "code": -32602,
                "message": "MRTR permission response action must be accept, decline, or cancel"
            })
        })?;
    let approved = match action {
        "decline" | "cancel" => false,
        "accept" => response
            .get("content")
            .and_then(|content| content.get("approve"))
            .and_then(Value::as_bool)
            .ok_or_else(|| json!({
                "code": -32602,
                "message": "Accepted MRTR permission response must include boolean content.approve"
            }))?,
        _ => return Err(json!({
            "code": -32602,
            "message": "MRTR permission response action must be accept, decline, or cancel"
        })),
    };
    Ok(Some(PermissionMrtrRetry {
        resume_id: resume_id.to_string(),
        approved,
    }))
}

pub(crate) fn permission_input_required(result: &Value) -> Option<Value> {
    let structured = result.get("structuredContent")?;
    if structured.get("ok").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    let error = structured.get("error")?;
    let permission = error.get("details")?.get("permission_request")?;
    let resume_id = permission.get("resume_id")?.as_str()?.trim();
    if resume_id.is_empty() {
        return None;
    }
    let tool_name = permission
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("operation");
    let permission_kind = permission
        .get("permission")
        .and_then(Value::as_str)
        .unwrap_or("permission");
    let reason = permission
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{tool_name} requires approval."));
    Some(json!({
        "resultType": "input_required",
        "inputRequests": {
            PERMISSION_MRTR_RESPONSE_KEY: {
                "method": "elicitation/create",
                "params": {
                    "mode": "form",
                    "message": format!("{reason} Approve this {permission_kind} request?"),
                    "requestedSchema": {
                        "type": "object",
                        "properties": {
                            "approve": {
                                "type": "boolean",
                                "title": "Approve operation",
                                "description": format!("Allow {tool_name} to continue once.")
                            }
                        },
                        "required": ["approve"],
                        "additionalProperties": false
                    }
                }
            }
        },
        "requestState": format!("{PERMISSION_MRTR_STATE_PREFIX}{resume_id}")
    }))
}

fn modern_task_client(params: &Value) -> bool {
    client_supports_tasks(params)
        && params
            .get("_meta")
            .and_then(|meta| meta.get("io.modelcontextprotocol/protocolVersion"))
            .and_then(Value::as_str)
            == Some(MODERN_PROTOCOL_VERSION)
}

fn task_id(params: &Value) -> Result<&str, Value> {
    params
        .get("taskId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            task_error(
                "taskId is required",
                "TASK_NOT_FOUND",
                "task_not_found",
                None,
            )
        })
}

fn handle_task_request(state: &SharedState, method: &str, params: &Value) -> Result<Value, Value> {
    let _ = state;
    if !modern_task_client(params) {
        return Err(json!({ "code": -32601, "message": format!("Method not found: {method}") }));
    }
    let task = require_task(&task_conversation_key(params), task_id(params)?)?;
    match method {
        "tasks/update" => Err(task_error(
            "Task is not waiting for input",
            "TASK_NOT_INPUT_REQUIRED",
            "task_not_input_required",
            Some(&task),
        )),
        "tasks/cancel" => {
            if task.status != "working" {
                return Err(task_error(
                    &format!("Task is already terminal: {}", task.status),
                    "TASK_ALREADY_TERMINAL",
                    "task_already_terminal",
                    Some(&task),
                ));
            }
            let structured = call_tool(
                task.context.as_ref(),
                "kill_session",
                &json!({ "session_id": task.session_id, "wait_ms": 0 }),
            );
            if structured.get("ok").and_then(Value::as_bool) == Some(false) {
                return Err(task_error(
                    "Unable to cancel task",
                    "TASK_CANCEL_FAILED",
                    "task_cancel_failed",
                    Some(&task),
                ));
            }
            mark_cancelled(&task.task_id)?;
            Ok(json!({}))
        }
        "tasks/get" => {
            if task.status != "working" {
                return Ok(detailed_task(&task));
            }
            let structured = call_tool(
                task.context.as_ref(),
                "wait_command",
                &json!({
                    "session_id": task.session_id,
                    "timeout_ms": 50,
                    "until": "exit",
                    "output_mode": "tail",
                    "max_output_bytes": 65_536
                }),
            );
            if structured.get("ok").and_then(Value::as_bool) == Some(false) {
                return Err(task_error(
                    "Unable to resolve task state",
                    "TASK_STATE_UNAVAILABLE",
                    "task_state_unavailable",
                    Some(&task),
                ));
            }
            update_from_snapshot(&task.task_id, &structured)
        }
        _ => Err(json!({ "code": -32601, "message": format!("Method not found: {method}") })),
    }
}

async fn handle_task_request_async(
    _state: SharedState,
    method: &str,
    params: Value,
) -> Result<Value, Value> {
    if !modern_task_client(&params) {
        return Err(json!({ "code": -32601, "message": format!("Method not found: {method}") }));
    }
    let task = require_task(&task_conversation_key(&params), task_id(&params)?)?;
    match method {
        "tasks/cancel" => {
            if task.status != "working" {
                return Err(task_error(
                    &format!("Task is already terminal: {}", task.status),
                    "TASK_ALREADY_TERMINAL",
                    "task_already_terminal",
                    Some(&task),
                ));
            }
            let structured = call_tool_async(
                task.context.clone(),
                "kill_session".into(),
                json!({ "session_id": task.session_id, "wait_ms": 0 }),
            )
            .await;
            if structured.get("ok").and_then(Value::as_bool) == Some(false) {
                return Err(task_error(
                    "Unable to cancel task",
                    "TASK_CANCEL_FAILED",
                    "task_cancel_failed",
                    Some(&task),
                ));
            }
            mark_cancelled(&task.task_id)?;
            Ok(json!({}))
        }
        "tasks/get" => {
            if task.status != "working" {
                return Ok(detailed_task(&task));
            }
            let structured = call_tool_async(
                task.context.clone(),
                "wait_command".into(),
                json!({
                    "session_id": task.session_id,
                    "timeout_ms": 50,
                    "until": "exit",
                    "output_mode": "tail",
                    "max_output_bytes": 65_536
                }),
            )
            .await;
            if structured.get("ok").and_then(Value::as_bool) == Some(false) {
                return Err(task_error(
                    "Unable to resolve task state",
                    "TASK_STATE_UNAVAILABLE",
                    "task_state_unavailable",
                    Some(&task),
                ));
            }
            update_from_snapshot(&task.task_id, &structured)
        }
        _ => handle_task_request(&_state, method, &params),
    }
}

fn handle_tools_call(state: &SharedState, params: &Value) -> Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| serde_json::json!({ "code": -32602, "message": "Missing tool name" }))?;
    let mut args = tool_arguments(name, params);

    let canonical_name = crate::tools::registry::canonical_tool_name(name);
    let recovery = take_recovery_context(&mut args)?;
    let requested_folder_id = take_workspace_folder_id(canonical_name, &mut args)?;
    let runtime = state.runtime_config();
    let known = crate::tools::registry::exposed_tool_names(&runtime.tool_profile);
    if !known.iter().any(|n| n == &canonical_name) {
        return Err(unknown_tool_error(name, &known, &runtime.tool_profile));
    }

    let host_session_key = host_session_key(params);
    let mut task_context = None;
    let structured = match canonical_name {
        "list_workspace_folders" => {
            crate::tools::hub::list_workspace_folders(state, host_session_key)
        }
        "conversation_bootstrap" => conversation_bootstrap(state, &args, host_session_key)?,
        "switch_workspace_folder" => {
            let folder_id = args
                .get("folder_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            crate::tools::hub::switch_workspace_folder(state, folder_id, host_session_key)
        }
        _ => {
            let routed = crate::tools::hub::resolve_tool_context(
                state.clone(),
                host_session_key,
                requested_folder_id.as_deref(),
                canonical_name,
                &args,
            )
            .map_err(|message| workspace_routing_error(state, host_session_key, message))?;
            task_context = Some(routed.context.clone());
            let structured = if let Some(retry) = permission_mrtr_retry(params)? {
                if let Some(pending_name) = routed
                    .context
                    .pending_operations
                    .tool_name(&retry.resume_id)
                {
                    if pending_name != canonical_name {
                        return Err(json!({
                            "code": -32602,
                            "message": "MRTR requestState does not match the retried tool",
                            "data": { "reason": "mrtr_request_state_mismatch", "requested_tool": canonical_name, "pending_tool": pending_name }
                        }));
                    }
                }
                call_tool(
                    routed.context.as_ref(),
                    "request_permissions",
                    &json!({ "resume_id": retry.resume_id, "approve": retry.approved, "confirm": retry.approved, "scope": "once" }),
                )
            } else {
                call_tool(routed.context.as_ref(), canonical_name, &args)
            };
            let structured = enrich_alternate_workspace_recovery(
                state,
                host_session_key,
                canonical_name,
                &args,
                requested_folder_id.as_deref(),
                &routed,
                structured,
            );
            attach_workspace_routing(structured, requested_folder_id.as_deref(), &routed)
        }
    };
    let structured = attach_recovery_metadata(structured, canonical_name, &args, &recovery);
    if modern_task_client(params) {
        if let Some(context) = task_context {
            if let Some(task) = create_process_task(
                context,
                task_conversation_key(params),
                canonical_name,
                &args,
                &structured,
            ) {
                return Ok(task);
            }
        }
    }
    Ok(wrap_mcp_tool_result(canonical_name, &args, structured))
}

async fn handle_tools_call_async(state: SharedState, params: Value) -> Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| serde_json::json!({ "code": -32602, "message": "Missing tool name" }))?
        .to_string();

    if name.starts_with("mcp__") {
        let external = crate::workspace_features::list_external_tools(&state.profile_id).await;
        if let Some(tool) = external
            .iter()
            .find(|tool| tool.name == name || tool.logical_name == name)
            .cloned()
        {
            return call_external_tool_with_hooks(&state, &params, &tool).await;
        }
    }

    let mut args = tool_arguments(&name, &params);
    let canonical_name = crate::tools::registry::canonical_tool_name(&name).to_string();
    let recovery = take_recovery_context(&mut args)?;
    let requested_folder_id = take_workspace_folder_id(&canonical_name, &mut args)?;
    let runtime = state.runtime_config();
    let known = crate::tools::registry::exposed_tool_names(&runtime.tool_profile);
    if !known.iter().any(|known_name| known_name == &canonical_name) {
        return Err(unknown_tool_error(&name, &known, &runtime.tool_profile));
    }

    let host_session_key = host_session_key(&params);
    let hook_session_id = host_session_key.unwrap_or("mcp").to_string();
    let mut task_context = None;
    let is_control_tool = matches!(
        canonical_name.as_str(),
        "list_workspace_folders" | "conversation_bootstrap" | "switch_workspace_folder"
    );
    let structured = if is_control_tool {
        let listing = crate::tools::hub::list_workspace_folders(&state, host_session_key);
        let hook_folder_id = listing
            .get("selected_folder_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let hook_cwd = hook_folder_id
            .as_deref()
            .and_then(|folder_id| {
                crate::workspace_features::runtime(&state.profile_id).and_then(|runtime| {
                    runtime
                        .folders
                        .iter()
                        .find(|folder| folder.id == folder_id)
                        .map(|folder| folder.path.clone())
                })
            })
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|_| ".".into())
            });
        let pre = crate::workspace_features::run_pre_tool_hooks(
            &state.profile_id,
            hook_folder_id.as_deref(),
            &hook_cwd,
            &hook_session_id,
            &canonical_name,
            args.clone(),
        )
        .await;
        if let Some(blocked) = pre.blocked {
            hook_blocked_structured(blocked.message, blocked.hook_key)
        } else {
            args = pre.input;
            let mut structured = match canonical_name.as_str() {
                "list_workspace_folders" => {
                    crate::tools::hub::list_workspace_folders(&state, host_session_key)
                }
                "conversation_bootstrap" => {
                    conversation_bootstrap(&state, &args, host_session_key)?
                }
                "switch_workspace_folder" => {
                    let folder_id = args
                        .get("folder_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    crate::tools::hub::switch_workspace_folder(&state, folder_id, host_session_key)
                }
                _ => unreachable!("control tool classification"),
            };
            let post = crate::workspace_features::run_post_tool_hooks(
                &state.profile_id,
                hook_folder_id.as_deref(),
                &hook_cwd,
                &hook_session_id,
                &canonical_name,
                &args,
                &structured,
                structured.get("ok").and_then(Value::as_bool) != Some(false),
            )
            .await;
            attach_hook_metadata(&mut structured, pre.context, post.feedback);
            structured
        }
    } else {
        let routed = crate::tools::hub::resolve_tool_context(
            state.clone(),
            host_session_key,
            requested_folder_id.as_deref(),
            &canonical_name,
            &args,
        )
        .map_err(|message| workspace_routing_error(&state, host_session_key, message))?;
        task_context = Some(routed.context.clone());
        let hook_cwd = routed.context.workspace.root().display().to_string();
        let pre = crate::workspace_features::run_pre_tool_hooks(
            &state.profile_id,
            Some(&routed.folder_id),
            &hook_cwd,
            &hook_session_id,
            &canonical_name,
            args.clone(),
        )
        .await;
        if let Some(blocked) = pre.blocked {
            hook_blocked_structured(blocked.message, blocked.hook_key)
        } else {
            args = pre.input;
            let mut structured = if let Some(retry) = permission_mrtr_retry(&params)? {
                if let Some(pending_name) = routed
                    .context
                    .pending_operations
                    .tool_name(&retry.resume_id)
                {
                    if pending_name != canonical_name {
                        return Err(json!({
                            "code": -32602,
                            "message": "MRTR requestState does not match the retried tool",
                            "data": { "reason": "mrtr_request_state_mismatch", "requested_tool": canonical_name, "pending_tool": pending_name }
                        }));
                    }
                }
                call_tool(
                    routed.context.as_ref(),
                    "request_permissions",
                    &json!({ "resume_id": retry.resume_id, "approve": retry.approved, "confirm": retry.approved, "scope": "once" }),
                )
            } else {
                call_tool_async_with_canary_sample_key(
                    routed.context.clone(),
                    canonical_name.clone(),
                    args.clone(),
                    host_session_key.map(str::to_owned),
                )
                .await
            };
            structured = enrich_alternate_workspace_recovery(
                &state,
                host_session_key,
                &canonical_name,
                &args,
                requested_folder_id.as_deref(),
                &routed,
                structured,
            );
            let post = crate::workspace_features::run_post_tool_hooks(
                &state.profile_id,
                Some(&routed.folder_id),
                &hook_cwd,
                &hook_session_id,
                &canonical_name,
                &args,
                &structured,
                structured.get("ok").and_then(Value::as_bool) != Some(false),
            )
            .await;
            attach_hook_metadata(&mut structured, pre.context, post.feedback);
            attach_workspace_routing(structured, requested_folder_id.as_deref(), &routed)
        }
    };
    let structured = attach_recovery_metadata(structured, &canonical_name, &args, &recovery);
    if modern_task_client(&params) {
        if let Some(context) = task_context {
            if let Some(task) = create_process_task(
                context,
                task_conversation_key(&params),
                &canonical_name,
                &args,
                &structured,
            ) {
                return Ok(task);
            }
        }
    }
    Ok(wrap_mcp_tool_result(&canonical_name, &args, structured))
}

async fn call_external_tool_with_hooks(
    state: &SharedState,
    params: &Value,
    tool: &crate::workspace_features::ExternalTool,
) -> Result<Value, Value> {
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let runtime = crate::workspace_features::runtime(&state.profile_id).ok_or_else(
        || json!({ "code": -32603, "message": "Workspace feature runtime is not active" }),
    )?;
    let folder = runtime
        .folders
        .iter()
        .find(|folder| folder.id == tool.folder_id)
        .ok_or_else(
            || json!({ "code": -32602, "message": "External MCP workspace folder is unavailable" }),
        )?;
    let cwd = folder.path.clone();
    let session_id = host_session_key(params).unwrap_or("mcp");
    let pre = crate::workspace_features::run_pre_tool_hooks(
        &state.profile_id,
        Some(&tool.folder_id),
        &cwd,
        session_id,
        &tool.logical_name,
        args,
    )
    .await;
    if let Some(blocked) = pre.blocked {
        return Ok(json!({
            "ok": false,
            "isError": true,
            "content": [{ "type": "text", "text": blocked.message }],
            "error": { "code": "HOOK_BLOCKED", "message": blocked.message, "hook_key": blocked.hook_key }
        }));
    }
    let mut result = match crate::workspace_features::call_external_tool(
        &state.profile_id,
        tool,
        pre.input.clone(),
    )
    .await
    {
        Ok(result) => result,
        Err(message) => json!({
            "isError": true,
            "content": [{ "type": "text", "text": message }]
        }),
    };
    let post = crate::workspace_features::run_post_tool_hooks(
        &state.profile_id,
        Some(&tool.folder_id),
        &cwd,
        session_id,
        &tool.logical_name,
        &pre.input,
        &result,
        result.get("isError").and_then(Value::as_bool) != Some(true),
    )
    .await;
    attach_hook_metadata(&mut result, pre.context, post.feedback);
    Ok(result)
}

fn hook_blocked_structured(message: String, hook_key: String) -> Value {
    json!({
        "ok": false,
        "error": {
            "code": "HOOK_BLOCKED",
            "message": message,
            "category": "policy",
            "retryable": false,
            "details": { "hook_key": hook_key }
        }
    })
}

fn attach_hook_metadata(structured: &mut Value, context: Vec<String>, feedback: Vec<String>) {
    if let Some(object) = structured.as_object_mut() {
        if !context.is_empty() {
            object.insert("hook_context".into(), json!(context));
        }
        if !feedback.is_empty() {
            object.insert("hook_feedback".into(), json!(feedback));
        }
    }
}

#[derive(Default)]
struct RecoveryContext {
    retry_of_call_sequence: Option<u64>,
    recovery_of_operation_id: Option<String>,
    recovery_action_id: Option<String>,
}

impl RecoveryContext {
    fn requested(&self) -> bool {
        self.retry_of_call_sequence.is_some()
            || self.recovery_of_operation_id.is_some()
            || self.recovery_action_id.is_some()
    }
}

fn take_recovery_context(args: &mut Value) -> Result<RecoveryContext, Value> {
    let Some(object) = args.as_object_mut() else {
        return Ok(RecoveryContext::default());
    };
    let retry_of_call_sequence = match object.remove("retry_of_call_sequence") {
        Some(value) => value.as_u64().filter(|value| *value > 0).ok_or_else(|| {
            serde_json::json!({ "code": -32602, "message": "retry_of_call_sequence must be a positive integer" })
        })?.into(),
        None => None,
    };
    let recovery_of_operation_id = take_recovery_string(object, "recovery_of_operation_id", false)?;
    let recovery_action_id = take_recovery_string(object, "recovery_action_id", true)?;
    Ok(RecoveryContext {
        retry_of_call_sequence,
        recovery_of_operation_id,
        recovery_action_id,
    })
}

fn take_recovery_string(
    object: &mut serde_json::Map<String, Value>,
    field: &str,
    token_only: bool,
) -> Result<Option<String>, Value> {
    let Some(value) = object.remove(field) else {
        return Ok(None);
    };
    let Some(value) = value.as_str().map(str::trim) else {
        return Err(
            serde_json::json!({ "code": -32602, "message": format!("{field} must be a string") }),
        );
    };
    if value.is_empty() || value.len() > 128 {
        return Err(
            serde_json::json!({ "code": -32602, "message": format!("{field} must contain 1-128 characters") }),
        );
    }
    if token_only
        && !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(serde_json::json!({
            "code": -32602,
            "message": format!("{field} must be a stable ASCII token")
        }));
    }
    Ok(Some(value.to_string()))
}

fn attach_recovery_metadata(
    mut structured: Value,
    tool_name: &str,
    semantic_args: &Value,
    recovery: &RecoveryContext,
) -> Value {
    let failed = structured.get("ok").and_then(Value::as_bool) == Some(false);
    if let Some(object) = structured.as_object_mut() {
        if let Some(sequence) = recovery.retry_of_call_sequence {
            object.insert("retry_of_call_sequence".into(), Value::from(sequence));
        }
        if let Some(operation_id) = recovery.recovery_of_operation_id.as_deref() {
            object.insert(
                "recovery_of_operation_id_hash".into(),
                Value::String(format!("{:x}", Sha256::digest(operation_id.as_bytes()))),
            );
        }
        if let Some(action_id) = recovery.recovery_action_id.as_deref() {
            object.insert(
                "recovery_action_id".into(),
                Value::String(action_id.to_string()),
            );
        }
        if recovery.requested() {
            object.insert("recovery_attempt".into(), Value::Bool(true));
            object.insert("recovery_succeeded".into(), Value::Bool(!failed));
        }
    }
    if failed {
        let failure_id = stable_failure_id(tool_name, semantic_args, &structured);
        if let Some(object) = structured.as_object_mut() {
            object.insert("failure_id".into(), Value::String(failure_id));
        }
    }
    structured
}

fn stable_failure_id(tool_name: &str, semantic_args: &Value, structured: &Value) -> String {
    let argument_bytes = serde_json::to_vec(semantic_args).unwrap_or_default();
    let error = structured.get("error").unwrap_or(&Value::Null);
    let details = error.get("details").unwrap_or(&Value::Null);
    let identity = serde_json::json!({
        "version": "tool-failure-v2",
        "tool": tool_name,
        "arguments_sha256": format!("{:x}", Sha256::digest(&argument_bytes)),
        "resolved_workspace_id": structured.get("resolved_workspace_id"),
        "error_code": error.get("code").or_else(|| structured.get("error_code")),
        "error_category": error.get("category").or_else(|| structured.get("error_category")),
        "stage": details.get("stage"),
        "reason": details.get("reason"),
        "path": details.get("path"),
        "file_index": details.get("file_index"),
        "edit_index": details.get("edit_index"),
        "expected_sha256": details.get("expected_sha256"),
        "actual_sha256": details.get("actual_sha256")
    });
    let bytes = serde_json::to_vec(&identity).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

fn take_workspace_folder_id(name: &str, args: &mut Value) -> Result<Option<String>, Value> {
    let Some(object) = args.as_object_mut() else {
        return Ok(None);
    };
    let Some(value) = object.remove("workspace_folder_id") else {
        return Ok(None);
    };
    if !crate::tools::tool_runtime::descriptor(name).workspace_selector {
        return Err(serde_json::json!({
            "code": -32602,
            "message": format!("workspace_folder_id is not supported by {name}"),
            "data": {
                "reason": "workspace_selector_not_supported",
                "error_code": "INVALID_ARGUMENT",
                "error_category": "validation",
                "retryable": false
            }
        }));
    }
    let Some(folder_id) = value.as_str().map(str::trim) else {
        return Err(
            serde_json::json!({ "code": -32602, "message": "workspace_folder_id must be a string" }),
        );
    };
    if folder_id.is_empty() {
        return Err(
            serde_json::json!({ "code": -32602, "message": "workspace_folder_id must not be empty" }),
        );
    }
    Ok(Some(folder_id.to_string()))
}

fn alternate_workspace_path_recovery_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file" | "view_image" | "list_files" | "search_text" | "project_map" | "git_status"
    )
}

fn enrich_alternate_workspace_recovery(
    state: &SharedState,
    host_session_key: Option<&str>,
    tool_name: &str,
    args: &Value,
    requested_folder_id: Option<&str>,
    routed: &crate::tools::hub::McpRoutedContext,
    mut structured: Value,
) -> Value {
    if requested_folder_id.is_some()
        || routed.route_source != "conversation"
        || structured.get("ok").and_then(Value::as_bool) != Some(false)
        || !alternate_workspace_path_recovery_tool(tool_name)
    {
        return structured;
    }
    let Some(error_code) = structured
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
    else {
        return structured;
    };
    if error_code != "NOT_FOUND" {
        return structured;
    }
    let Some(raw_path) = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty() && *path != ".")
    else {
        return structured;
    };

    let listing = crate::tools::hub::list_workspace_folders(state, host_session_key);
    let Some(folders) = listing.get("folders").and_then(Value::as_array) else {
        return structured;
    };
    let mut matches = Vec::<Value>::new();
    let mut recovery_actions = Vec::<Value>::new();
    for folder in folders {
        let Some(folder_id) = folder.get("id").and_then(Value::as_str) else {
            continue;
        };
        if folder_id == routed.folder_id {
            continue;
        }
        let Ok(alternate) = crate::tools::hub::resolve_tool_context(
            state.clone(),
            host_session_key,
            Some(folder_id),
            tool_name,
            args,
        ) else {
            continue;
        };
        if alternate
            .context
            .workspace
            .resolve_read_path(raw_path)
            .is_err()
        {
            continue;
        }
        let folder_name = folder
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(folder_id);
        matches.push(json!({
            "folder_id": folder_id,
            "folder_name": folder_name
        }));
        if recovery_actions.len() < 8 {
            let mut retry_args = args.as_object().cloned().unwrap_or_default();
            retry_args.insert(
                "workspace_folder_id".into(),
                Value::String(folder_id.to_string()),
            );
            recovery_actions.push(json!({
                "action": "retry_in_workspace",
                "action_id": format!("workspace-retry-{folder_id}"),
                "tool": tool_name,
                "required_arguments": [],
                "arguments": Value::Object(retry_args),
                "reason": "alternate_workspace_match"
            }));
        }
    }
    if matches.is_empty() {
        return structured;
    }

    let match_count = matches.len();
    let Some(error) = structured.get_mut("error").and_then(Value::as_object_mut) else {
        return structured;
    };
    error.insert("retryable".into(), Value::Bool(true));
    let details = error.entry("details").or_insert_with(|| json!({}));
    if !details.is_object() {
        *details = json!({});
    }
    let details = details
        .as_object_mut()
        .expect("details normalized to object");
    details.insert("alternate_workspace_match_count".into(), json!(match_count));
    details.insert("alternate_workspace_matches".into(), Value::Array(matches));
    details.insert("recovery_actions".into(), Value::Array(recovery_actions));
    details.insert(
        "suggestion".into(),
        Value::String(if match_count == 1 {
            "Retry this call with the provided workspace_folder_id; the conversation selection will not change."
                .into()
        } else {
            "Choose one matching workspace recovery action; the conversation selection will not change."
                .into()
        }),
    );
    structured
}

fn attach_workspace_routing(
    mut structured: Value,
    requested_folder_id: Option<&str>,
    routed: &crate::tools::hub::McpRoutedContext,
) -> Value {
    if let Some(object) = structured.as_object_mut() {
        object.insert(
            "requested_workspace_id".into(),
            requested_folder_id
                .map(|value| Value::String(value.to_string()))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "resolved_workspace_id".into(),
            Value::String(routed.folder_id.clone()),
        );
        object.insert(
            "workspace_route_source".into(),
            Value::String(routed.route_source.to_string()),
        );
        object.insert(
            "workspace_route_changed".into(),
            Value::Bool(
                routed.route_source != "conversation"
                    && routed.selected_folder_id.as_deref() != Some(routed.folder_id.as_str()),
            ),
        );
        object.insert("conversation_selection_changed".into(), Value::Bool(false));
    }
    structured
}

fn workspace_routing_error(
    state: &SharedState,
    host_session_key: Option<&str>,
    message: String,
) -> Value {
    let (error_code, message) =
        crate::tools::hub::routing_error_parts(&message, "WORKSPACE_FOLDER_ROUTING_FAILED");
    let mut data = serde_json::json!({
        "reason": "workspace_folder_routing_failed",
        "error_code": error_code,
        "error_category": "workspace_routing",
        "retryable": true,
        "suggestion": "Call conversation_bootstrap, or retry a tool that supports workspace_folder_id with an allowed folder id."
    });
    if error_code == "WORKSPACE_FOLDER_NOT_SELECTED" {
        let listing = crate::tools::hub::list_workspace_folders(state, host_session_key);
        data["available_folders"] = crate::tools::hub::routing_folder_options(&listing);
        data["selected_folder_id"] = listing
            .get("selected_folder_id")
            .cloned()
            .unwrap_or(Value::Null);
        data["next_action"] = Value::String(
            "Choose one available_folders entry and call conversation_bootstrap with its id, or pass that id as workspace_folder_id to a supported project tool."
                .into(),
        );
        data["suggestion"] = Value::String(
            "Choose an available folder and call conversation_bootstrap, or use workspace_folder_id for a one-call route; ambiguous multi-folder sessions are not auto-selected."
                .into(),
        );
    }
    serde_json::json!({
        "code": -32602,
        "message": message,
        "data": data
    })
}

fn host_session_key(params: &Value) -> Option<&str> {
    params
        .get("_meta")
        .and_then(|meta| meta.get("openai/session"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn unknown_tool_error(name: &str, known: &[&str], tool_profile: &str) -> Value {
    serde_json::json!({
        "code": -32602,
        "message": format!("Unknown tool: {name}"),
        "data": {
            "reason": "unknown_tool",
            "error_code": "UNKNOWN_TOOL",
            "error_category": "catalog",
            "retryable": true,
            "suggestion": "Refresh tools/list and retry with the current tool catalog.",
            "toolset_revision": crate::tools::registry::toolset_revision(tool_profile),
            "available_tools": known
        }
    })
}

fn legacy_edit_file_arguments(args: Value) -> Value {
    let Some(source) = args.as_object() else {
        return args;
    };
    let mut file = serde_json::Map::new();
    for field in ["path", "expected_sha256", "edits", "apply_proposal"] {
        if let Some(value) = source.get(field) {
            file.insert(field.to_string(), value.clone());
        }
    }
    let mut converted = serde_json::Map::new();
    converted.insert("files".into(), Value::Array(vec![Value::Object(file)]));
    for field in ["dry_run", "reason", "workspace_folder_id"] {
        if let Some(value) = source.get(field) {
            converted.insert(field.to_string(), value.clone());
        }
    }
    Value::Object(converted)
}

fn tool_arguments(name: &str, params: &Value) -> Value {
    let mut args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if name == "edit_file" {
        args = legacy_edit_file_arguments(args);
    }
    if name.starts_with("history_session_") || name == "conversation_bootstrap" {
        if let Some(session_key) = host_session_key(params) {
            if !args.is_object() {
                args = serde_json::json!({});
            }
            args["_host_session_key"] = Value::String(session_key.to_string());
        }
    }
    args
}

pub fn new_state(
    folders: Vec<WorkspaceFolder>,
    bootstrap_folder_id: String,
    profile_id: String,
    auth: AuthConfig,
    policy: crate::tools::policy::PolicySettings,
    tool_profile: String,
    permission_mode: String,
    sandbox: SandboxConfig,
    limits: ExecutionLimits,
) -> Result<SharedState, String> {
    let bootstrap_folder = folders
        .iter()
        .find(|folder| folder.id == bootstrap_folder_id)
        .cloned()
        .ok_or_else(|| "找不到 MCP 啟動用資料夾；不會 fallback 到第一個資料夾。".to_string())?;
    let workspace = Workspace::new_with_execution(
        PathBuf::from(&bootstrap_folder.path),
        bootstrap_folder.execution.clone(),
    )
    .map_err(|error| error.message())?;
    let runtime_config =
        SharedRuntimeToolConfig::new_with_sandbox(policy, tool_profile, permission_mode, sandbox);
    let execution_resource_id = format!("{}--mcp--{}", profile_id, bootstrap_folder.id);
    let state = Arc::new(
        ToolContext::from_workspace_with_shared_runtime_config_and_resource_ids_and_limits(
            workspace,
            auth.clone(),
            runtime_config.clone(),
            profile_id.clone(),
            execution_resource_id.clone(),
            execution_resource_id,
            limits,
        ),
    );
    crate::tools::hub::register(
        profile_id.clone(),
        folders.clone(),
        bootstrap_folder.id,
        state.clone(),
        crate::tools::hub::HubConfig {
            auth,
            runtime_config,
            limits,
            execution_resource_namespace: "mcp".into(),
        },
    )?;
    crate::workspace_features::register_runtime(&profile_id, folders);
    Ok(state)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use serde_json::json;

    use crate::tools::ToolContext;

    use super::{
        attach_recovery_metadata, discover_result, handle_request, handle_request_async,
        initialize_result, new_state, permission_input_required, permission_mrtr_retry,
        take_recovery_context, tool_arguments, LATEST_LEGACY_PROTOCOL_VERSION,
        MODERN_PROTOCOL_VERSION,
    };

    #[test]
    fn permission_mrtr_contract_round_trips_approval_state() {
        let input = permission_input_required(&json!({
            "structuredContent": {"ok": false, "error": {"code": "PERMISSION_REQUIRED", "details": {
                "permission_request": {"resume_id": "resume-1", "tool_name": "exec_command", "permission": "process_execution", "reason": "approval required"}
            }}}
        })).expect("input required");
        assert_eq!(input["resultType"], "input_required");
        assert_eq!(
            input["inputRequests"]["permission_approval"]["method"],
            "elicitation/create"
        );
        assert_eq!(input["requestState"], "permission:resume-1");

        let retry = permission_mrtr_retry(&json!({
            "requestState": "permission:resume-1",
            "inputResponses": {"permission_approval": {"action": "accept", "content": {"approve": true}}}
        })).expect("valid response").expect("permission retry");
        assert_eq!(retry.resume_id, "resume-1");
        assert!(retry.approved);
    }

    #[test]
    fn permission_mrtr_decline_is_not_approval() {
        let retry = permission_mrtr_retry(&json!({
            "requestState": "permission:resume-2",
            "inputResponses": {"permission_approval": {"action": "decline"}}
        }))
        .expect("valid response")
        .expect("permission retry");
        assert_eq!(retry.resume_id, "resume-2");
        assert!(!retry.approved);
    }
    #[test]
    fn initialize_instructions_define_the_history_persistence_workflow() {
        let initialized = initialize_result(Some("2025-06-18"), "core");
        let instructions = initialized["instructions"].as_str().expect("instructions");
        assert!(instructions.contains("conversation_bootstrap"));
        assert!(instructions.contains("history_session_bootstrap"));
        assert!(instructions.contains("list_workspace_folders"));
        assert!(instructions.contains("switch_workspace_folder"));
        assert!(instructions.contains("without affecting other conversations"));
        assert!(instructions.contains("At the start of every new ChatGPT conversation"));
        assert!(instructions.contains("before accessing project content"));
        assert!(instructions.contains("normal startup path is one tool call"));
        assert!(instructions.contains("ambiguous multi-folder conversations remain unselected"));
        assert!(instructions.contains("no history-based folder fallback"));
        assert!(instructions.contains("default cwd are remembered for the same runtime session"));
        assert!(instructions.contains("standard MCP prompts and resources"));
        assert!(instructions.contains("prompts/get or resources/read"));
        assert!(instructions.contains("Skills are workflow guidance"));
        assert!(instructions.contains("Hooks may block or rewrite tool calls"));
        assert!(instructions.contains("external MCP servers contribute proxied tools"));
        assert!(instructions.contains("workspace_folder_id"));
        assert!(instructions.contains("without changing the conversation selection"));
        assert!(instructions.contains("exec_many(mode=auto)"));
        assert!(instructions.contains("history_session_checkpoint"));
        assert!(instructions.contains("session_key and current_path returned by bootstrap"));
        assert!(instructions.contains("session_key and expected_path"));
        assert!(instructions.contains("After completing each user-requested task"));
        assert!(instructions.contains("before the final response"));
        assert!(instructions.contains("checkpoint returns ok=true"));
        assert!(instructions.contains("not automatic background persistence"));
    }

    #[tokio::test]
    async fn conversation_bootstrap_returns_enabled_project_skill_summaries() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let skill_name = format!("rust-parity-{}", uuid::Uuid::new_v4());
        let skill_dir = workspace.path().join(".agents/skills").join(&skill_name);
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: {skill_name}\ndescription: Rust parity bootstrap skill\n---\nUse this skill only for the dispatcher test.\n"
            ),
        )
        .expect("write skill");
        let profile_id = format!("skill-bootstrap-{}", uuid::Uuid::new_v4());
        let state = new_state(
            vec![crate::workspace::WorkspaceFolder {
                id: "folder-a".into(),
                name: "Folder A".into(),
                path: workspace.path().display().to_string(),
                execution: Default::default(),
            }],
            "folder-a".into(),
            profile_id.clone(),
            crate::workspace::AuthConfig::default(),
            crate::tools::policy::PolicySettings::default(),
            "full".into(),
            "trusted".into(),
            crate::workspace::SandboxConfig::default(),
            crate::tools::ExecutionLimits::default(),
        )
        .expect("mcp state");

        let response = handle_request_async(
            state,
            json!({
                "jsonrpc": "2.0",
                "id": 77,
                "method": "tools/call",
                "params": {
                    "name": "conversation_bootstrap",
                    "arguments": {
                        "create_if_missing": true,
                        "title": "Skill bootstrap parity"
                    },
                    "_meta": {"openai/session": "skill-bootstrap-session"}
                }
            }),
        )
        .await;
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["ok"], true, "{response}");
        let project_skills = &structured["project_skills"];
        assert!(project_skills["count"]
            .as_u64()
            .is_some_and(|count| count >= 1));
        assert_eq!(
            project_skills["skillset_revision"]
                .as_str()
                .expect("skillset revision")
                .len(),
            64
        );
        let summary = project_skills["skills"]
            .as_array()
            .expect("skill summaries")
            .iter()
            .find(|skill| skill["name"] == skill_name)
            .expect("workspace skill summary");
        assert_eq!(summary["description"], "Rust parity bootstrap skill");
        assert_eq!(summary["source"], "agents");
        assert_eq!(summary["scope"], "workspace");
        assert_eq!(
            summary["relative_path"],
            format!(".agents/skills/{skill_name}/SKILL.md")
        );
        assert_eq!(
            summary["content_sha256"]
                .as_str()
                .expect("content hash")
                .len(),
            64
        );
        assert_eq!(
            project_skills["mcp_surfaces"],
            json!([
                "prompts/list",
                "prompts/get",
                "resources/list",
                "resources/read"
            ])
        );
        let learning = &structured["learning"];
        assert_eq!(learning["runtime_policy"], "validated_canary_rust_parity");
        assert_eq!(learning["ingestion"]["enabled"], true);
        assert_eq!(
            learning["knowledge_revision"]
                .as_str()
                .expect("knowledge revision")
                .len(),
            64
        );
        assert!(learning.get("records").is_none());
        assert!(learning.get("hypothesis").is_none());
        assert!(learning.get("recommended_action").is_none());

        crate::tools::hub::remove_live_hub(&profile_id);
        crate::workspace_features::unregister_runtime(&profile_id);
    }

    #[test]
    fn validated_evolved_skill_is_only_consumed_by_session_canary_surfaces() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let skill_name = format!("evolved-canary-{}", uuid::Uuid::new_v4());
        let skill_dir = workspace.path().join(".agents/skills").join(&skill_name);
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: {skill_name}\ndescription: Evolved Skill canary parity\n---\nBase workflow only.\n"
            ),
        )
        .expect("write skill");
        let profile_id = format!("evolved-canary-{}", uuid::Uuid::new_v4());
        let state = new_state(
            vec![crate::workspace::WorkspaceFolder {
                id: "folder-a".into(),
                name: "Folder A".into(),
                path: workspace.path().display().to_string(),
                execution: Default::default(),
            }],
            "folder-a".into(),
            profile_id.clone(),
            crate::workspace::AuthConfig::default(),
            crate::tools::policy::PolicySettings::default(),
            "full".into(),
            "trusted".into(),
            crate::workspace::SandboxConfig::default(),
            crate::tools::ExecutionLimits::default(),
        )
        .expect("mcp state");

        let bootstrap_before =
            crate::workspace_features::skill_bootstrap_summary(&profile_id, "folder-a")
                .expect("skill bootstrap");
        let base = bootstrap_before["skills"]
            .as_array()
            .expect("skill summaries")
            .iter()
            .find(|skill| skill["name"] == skill_name)
            .expect("base skill");
        let base_source = base["source"].as_str().expect("base source").to_string();
        let base_sha = base["content_sha256"]
            .as_str()
            .expect("base sha")
            .to_string();
        let prompt_name = format!("project-skill/folder-a/{skill_name}");
        let resource_uri = format!("skill://coding-tools/folder-a/{skill_name}");
        let prompts_before = handle_request(
            &state,
            &json!({"jsonrpc":"2.0","id":1,"method":"prompts/list","params":{}}),
        );
        let resources_before = handle_request(
            &state,
            &json!({"jsonrpc":"2.0","id":2,"method":"resources/list","params":{}}),
        );

        let store = crate::knowledge::knowledge_store_for_context(state.as_ref());
        let record = crate::knowledge::KnowledgeRecord {
            id: String::new(),
            schema_version: 1,
            scope: crate::knowledge::KnowledgeScope::Workspace,
            target: crate::knowledge::KnowledgeArtifactTarget::EvolvedSkill,
            status: crate::knowledge::KnowledgeStatus::Validated,
            trigger: json!({"tool":"edit","pattern":"stale_guarded_edit_recovery"})
                .as_object()
                .unwrap()
                .clone(),
            hypothesis: "Serve validated local Skill guidance to a deterministic session canary."
                .into(),
            recommended_action: json!({
                "append_guidance": ["Re-read the current file before rebuilding a stale guarded edit."]
            })
            .as_object()
            .unwrap()
            .clone(),
            evidence: crate::knowledge::KnowledgeEvidence {
                observations: 15,
                successes: 0,
                failures: 15,
                verified_successes: None,
                verification_failures: None,
                recovery_successes: None,
                recovery_failures: None,
                total_duration_ms: 1_000,
                total_request_bytes: 0,
                total_response_bytes: 0,
                first_seen_at_ms: 1,
                last_seen_at_ms: 15,
                task_context_ids: None,
                conversation_context_ids: Some(vec!["a".repeat(64), "b".repeat(64)]),
                runtime_boot_context_ids: None,
            },
            confidence: 0.9,
            compatibility: None,
            source_event_ids: (0..15).map(|index| format!("seed-{index}")).collect(),
            counterexample_event_ids: vec![],
            supersedes: None,
            superseded_by: None,
            created_at_ms: 1,
            updated_at_ms: 15,
            metadata: Some(
                json!({
                    "name": skill_name,
                    "base": {
                        "source": base_source,
                        "name": skill_name,
                        "contentSha256": base_sha
                    },
                    "generation": 1
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        };
        let stored = store.upsert(record).expect("store evolved Skill");
        let canary_store = crate::knowledge::CanaryImpactStore::new(store.root());
        let engine = crate::knowledge::EvolvedSkillCanaryEngine::new(store, canary_store);
        let mut intervention_session = None;
        let mut control_session = None;
        for index in 0..100 {
            let session = format!("evolved-session-{index}");
            let decision = engine
                .decide(&base_source, &skill_name, &base_sha, None, &session)
                .expect("canary decision")
                .expect("eligible evolved Skill");
            if decision.applied && intervention_session.is_none() {
                intervention_session = Some(session.clone());
            }
            if !decision.applied && control_session.is_none() {
                control_session = Some(session);
            }
            if intervention_session.is_some() && control_session.is_some() {
                break;
            }
        }
        let intervention_session = intervention_session.expect("intervention session");
        let control_session = control_session.expect("control session");

        let intervention_prompt = handle_request(
            &state,
            &json!({
                "jsonrpc":"2.0","id":3,"method":"prompts/get",
                "params":{"name":prompt_name,"_meta":{"openai/session":intervention_session}}
            }),
        );
        let control_prompt = handle_request(
            &state,
            &json!({
                "jsonrpc":"2.0","id":4,"method":"prompts/get",
                "params":{"name":prompt_name,"_meta":{"openai/session":control_session}}
            }),
        );
        let intervention_text = intervention_prompt["result"]["messages"][0]["content"]["text"]
            .as_str()
            .expect("intervention prompt text");
        let control_text = control_prompt["result"]["messages"][0]["content"]["text"]
            .as_str()
            .expect("control prompt text");
        assert!(intervention_text.contains("## Learned guidance"));
        assert!(intervention_text.contains("Re-read the current file"));
        assert!(!control_text.contains("## Learned guidance"));

        let intervention_resource = handle_request(
            &state,
            &json!({
                "jsonrpc":"2.0","id":5,"method":"resources/read",
                "params":{"uri":resource_uri,"_meta":{"openai/session":intervention_session}}
            }),
        );
        assert!(intervention_resource["result"]["contents"][0]["text"]
            .as_str()
            .expect("resource text")
            .contains("## Learned guidance"));

        let intervention_attribution =
            crate::workspace_features::selected_skill_learning_attribution(
                &profile_id,
                Some(&intervention_session),
                "folder-a",
            )
            .expect("intervention attribution");
        let control_attribution = crate::workspace_features::selected_skill_learning_attribution(
            &profile_id,
            Some(&control_session),
            "folder-a",
        )
        .expect("control attribution");
        assert_eq!(
            intervention_attribution
                .canary
                .as_ref()
                .expect("intervention canary")
                .knowledge_id,
            stored.id
        );
        assert!(intervention_attribution.canary.unwrap().applied);
        assert!(!control_attribution.canary.unwrap().applied);

        assert_eq!(
            crate::workspace_features::skill_bootstrap_summary(&profile_id, "folder-a")
                .expect("bootstrap after validated canary"),
            bootstrap_before,
            "validated canary guidance must not change bootstrap snapshots"
        );
        assert_eq!(
            handle_request(
                &state,
                &json!({"jsonrpc":"2.0","id":1,"method":"prompts/list","params":{}}),
            ),
            prompts_before,
            "validated canary guidance must not change prompts/list"
        );
        let resources_after = handle_request(
            &state,
            &json!({"jsonrpc":"2.0","id":2,"method":"resources/list","params":{}}),
        );
        assert_eq!(
            resources_after["result"]["resources"], resources_before["result"]["resources"],
            "validated canary guidance must not change the listed Skill resources"
        );
        assert_eq!(
            resources_after["result"]["_meta"]["coding-tools/skillset-revision"],
            resources_before["result"]["_meta"]["coding-tools/skillset-revision"],
            "validated canary guidance must not change the Skill resource revision"
        );

        crate::tools::hub::remove_live_hub(&profile_id);
        crate::workspace_features::unregister_runtime(&profile_id);
    }

    #[test]
    fn recovery_metadata_is_stripped_and_failure_id_is_stable() {
        let mut first_args = json!({
            "path": "missing.txt",
            "retry_of_call_sequence": 41,
            "recovery_of_operation_id": "operation-41",
            "recovery_action_id": "read_current_file"
        });
        let first_recovery = take_recovery_context(&mut first_args).expect("recovery context");
        assert_eq!(first_args, json!({"path": "missing.txt"}));
        let failure = json!({
            "ok": false,
            "resolved_workspace_id": "folder-a",
            "error": {
                "code": "NOT_FOUND",
                "category": "not_found",
                "retryable": false,
                "details": {"path": "missing.txt", "reason": "not_found"}
            }
        });
        let first =
            attach_recovery_metadata(failure.clone(), "read_file", &first_args, &first_recovery);
        assert_eq!(first["retry_of_call_sequence"], 41);
        assert_eq!(first["recovery_action_id"], "read_current_file");
        assert_eq!(first["recovery_attempt"], true);
        assert_eq!(first["recovery_succeeded"], false);
        assert_eq!(
            first["recovery_of_operation_id_hash"]
                .as_str()
                .expect("operation hash")
                .len(),
            64
        );
        let failure_id = first["failure_id"]
            .as_str()
            .expect("failure id")
            .to_string();
        assert_eq!(failure_id.len(), 64);

        let mut second_args = json!({
            "path": "missing.txt",
            "retry_of_call_sequence": 42,
            "recovery_action_id": "refresh_target"
        });
        let second_recovery = take_recovery_context(&mut second_args).expect("second recovery");
        let second = attach_recovery_metadata(failure, "read_file", &second_args, &second_recovery);
        assert_eq!(second["failure_id"], failure_id);
    }

    #[test]
    fn mcp_session_cannot_access_project_before_explicit_folder_selection() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        fs::write(workspace.path().join("README.md"), "explicit routing").expect("write readme");
        let profile_id = format!("explicit-routing-{}", uuid::Uuid::new_v4());
        let state = new_state(
            vec![crate::workspace::WorkspaceFolder {
                id: "folder-a".into(),
                name: "Folder A".into(),
                path: workspace.path().display().to_string(),
                execution: Default::default(),
            }],
            "folder-a".into(),
            profile_id.clone(),
            crate::workspace::AuthConfig::default(),
            crate::tools::policy::PolicySettings::default(),
            "full".into(),
            "trusted".into(),
            crate::workspace::SandboxConfig::default(),
            crate::tools::ExecutionLimits::default(),
        )
        .expect("mcp state");

        let unselected = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "read_file",
                    "arguments": {"path": "README.md"},
                    "_meta": {"openai/session": "session-a"}
                }
            }),
        );
        assert_eq!(
            unselected["error"]["data"]["error_code"],
            "WORKSPACE_FOLDER_NOT_SELECTED"
        );
        assert_eq!(
            unselected["error"]["data"]["available_folders"],
            json!([{
                "id": "folder-a",
                "name": "Folder A",
                "path": workspace.path().display().to_string()
            }])
        );
        assert!(unselected["error"]["data"]["selected_folder_id"].is_null());
        assert!(unselected["error"]["data"]["next_action"]
            .as_str()
            .expect("next action")
            .contains("conversation_bootstrap"));

        let one_call = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "tools/call",
                "params": {
                    "name": "read_file",
                    "arguments": {"path": "README.md", "workspace_folder_id": "folder-a"},
                    "_meta": {"openai/session": "session-a"}
                }
            }),
        );
        let routed = &one_call["result"]["structuredContent"];
        assert_eq!(routed["ok"], true);
        assert_eq!(routed["content"], "explicit routing");
        assert_eq!(routed["requested_workspace_id"], "folder-a");
        assert_eq!(routed["resolved_workspace_id"], "folder-a");
        assert_eq!(routed["workspace_route_source"], "explicit");
        assert_eq!(routed["workspace_route_changed"], true);
        assert_eq!(routed["conversation_selection_changed"], false);

        let listing = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "list_workspace_folders",
                    "arguments": {},
                    "_meta": {"openai/session": "session-a"}
                }
            }),
        );
        let listed = &listing["result"]["structuredContent"];
        assert!(listed.get("default_folder_id").is_none());
        assert!(listed["selected_folder_id"].is_null());
        assert!(listed["folders"][0]["selected"].as_bool() == Some(false));

        let switched = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "switch_workspace_folder",
                    "arguments": {"folder_id": "folder-a"},
                    "_meta": {"openai/session": "session-a"}
                }
            }),
        );
        assert_eq!(switched["result"]["structuredContent"]["ok"], true);

        let selected = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "read_file",
                    "arguments": {"path": "README.md"},
                    "_meta": {"openai/session": "session-a"}
                }
            }),
        );
        assert_eq!(selected["result"]["structuredContent"]["ok"], true);

        crate::tools::hub::remove_live_hub(&profile_id);
    }

    #[test]
    fn not_found_returns_one_call_retry_for_unique_alternate_workspace_match() {
        let first = tempfile::tempdir().expect("first workspace");
        let second = tempfile::tempdir().expect("second workspace");
        fs::write(
            second.path().join("alternate-only.txt"),
            "alternate workspace only",
        )
        .expect("write alternate file");
        let profile_id = format!("alternate-routing-{}", uuid::Uuid::new_v4());
        let state = new_state(
            vec![
                crate::workspace::WorkspaceFolder {
                    id: "folder-a".into(),
                    name: "Folder A".into(),
                    path: first.path().display().to_string(),
                    execution: Default::default(),
                },
                crate::workspace::WorkspaceFolder {
                    id: "folder-b".into(),
                    name: "Folder B".into(),
                    path: second.path().display().to_string(),
                    execution: Default::default(),
                },
            ],
            "folder-a".into(),
            profile_id.clone(),
            crate::workspace::AuthConfig::default(),
            crate::tools::policy::PolicySettings::default(),
            "full".into(),
            "trusted".into(),
            crate::workspace::SandboxConfig::default(),
            crate::tools::ExecutionLimits::default(),
        )
        .expect("mcp state");

        let switched = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "switch_workspace_folder",
                    "arguments": {"folder_id": "folder-a"},
                    "_meta": {"openai/session": "session-a"}
                }
            }),
        );
        assert_eq!(switched["result"]["structuredContent"]["ok"], true);

        let missing = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "read_file",
                    "arguments": {"path": "alternate-only.txt"},
                    "_meta": {"openai/session": "session-a"}
                }
            }),
        );
        let structured = &missing["result"]["structuredContent"];
        assert_eq!(structured["ok"], false);
        assert_eq!(structured["error"]["code"], "NOT_FOUND");
        assert_eq!(structured["error"]["retryable"], true);
        assert_eq!(
            structured["error"]["details"]["alternate_workspace_match_count"],
            1
        );
        assert_eq!(
            structured["error"]["details"]["alternate_workspace_matches"],
            json!([{"folder_id": "folder-b", "folder_name": "Folder B"}])
        );
        let action = &structured["error"]["details"]["recovery_actions"][0];
        assert_eq!(action["action"], "retry_in_workspace");
        assert_eq!(action["action_id"], "workspace-retry-folder-b");
        assert_eq!(action["tool"], "read_file");
        assert_eq!(action["required_arguments"], json!([]));
        assert_eq!(
            action["arguments"],
            json!({"path": "alternate-only.txt", "workspace_folder_id": "folder-b"})
        );

        let recovered = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "read_file",
                    "arguments": action["arguments"].clone(),
                    "_meta": {"openai/session": "session-a"}
                }
            }),
        );
        let recovered = &recovered["result"]["structuredContent"];
        assert_eq!(recovered["ok"], true);
        assert_eq!(recovered["content"], "alternate workspace only");
        assert_eq!(recovered["workspace_route_source"], "explicit");

        let listing = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "list_workspace_folders",
                    "arguments": {},
                    "_meta": {"openai/session": "session-a"}
                }
            }),
        );
        assert_eq!(
            listing["result"]["structuredContent"]["selected_folder_id"],
            "folder-a"
        );

        crate::tools::hub::remove_live_hub(&profile_id);
    }

    #[test]
    fn initialize_does_not_claim_tool_catalog_notifications_without_a_stream() {
        let initialized = initialize_result(Some("2025-06-18"), "core");

        assert_eq!(initialized["capabilities"]["tools"]["listChanged"], false);
        assert_eq!(
            initialized["serverInfo"]["toolsetRevision"]
                .as_str()
                .expect("toolset revision")
                .len(),
            16
        );
    }

    #[test]
    fn runtime_tool_profile_updates_catalog_without_listener_restart() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        let initial = handle_request(
            &state,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}),
        );
        let initial_names = initial["result"]["tools"]
            .as_array()
            .expect("initial tools")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        let initial_revision = initial["result"]["toolsetRevision"]
            .as_str()
            .expect("initial revision")
            .to_string();
        assert!(!initial_names.contains(&"start_task"));

        let runtime = state.runtime_config();
        state.update_runtime_config(runtime.policy, "advanced".into(), runtime.permission_mode);

        let advanced = handle_request(
            &state,
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        );
        let advanced_names = advanced["result"]["tools"]
            .as_array()
            .expect("advanced tools")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        let advanced_revision = advanced["result"]["toolsetRevision"]
            .as_str()
            .expect("advanced revision")
            .to_string();
        assert!(advanced_names.contains(&"start_task"));
        assert_ne!(advanced_revision, initial_revision);

        let initialized = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "initialize",
                "params": {"protocolVersion": "2025-06-18"}
            }),
        );
        assert_eq!(
            initialized["result"]["serverInfo"]["toolsetRevision"],
            advanced_revision
        );

        let runtime = state.runtime_config();
        state.update_runtime_config(runtime.policy, "guarded-core".into(), "guarded".into());
        let hidden = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {"name": "start_task", "arguments": {"objective": "hidden"}}
            }),
        );
        let guarded = state.runtime_config();
        assert_eq!(hidden["error"]["data"]["error_code"], "UNKNOWN_TOOL");
        assert_eq!(
            hidden["error"]["data"]["toolset_revision"],
            crate::tools::registry::toolset_revision(&guarded.tool_profile)
        );
    }

    #[test]
    fn initialize_negotiates_legacy_versions_and_never_upgrades_to_stateless() {
        assert_eq!(
            initialize_result(Some("2025-06-18"), "core")["protocolVersion"],
            "2025-06-18"
        );
        assert_eq!(
            initialize_result(Some(MODERN_PROTOCOL_VERSION), "core")["protocolVersion"],
            LATEST_LEGACY_PROTOCOL_VERSION
        );
        assert_eq!(
            initialize_result(Some("unsupported"), "core")["protocolVersion"],
            LATEST_LEGACY_PROTOCOL_VERSION
        );
    }

    #[test]
    fn discover_uses_final_modern_schema() {
        let discovered = discover_result();
        assert_eq!(
            discovered["supportedVersions"],
            serde_json::json!([MODERN_PROTOCOL_VERSION])
        );
        assert_eq!(
            discovered["capabilities"]["tools"],
            serde_json::json!({"listChanged": false})
        );
        assert_eq!(
            discovered["capabilities"]["extensions"]["io.modelcontextprotocol/tasks"],
            serde_json::json!({})
        );
        assert!(discovered.get("protocolVersion").is_none());
        assert!(discovered.get("protocolVersions").is_none());
        assert!(discovered.get("serverInfo").is_none());
    }

    #[test]
    fn workspace_prompt_initializes_or_restores_a_chatgpt_session() {
        let component = include_str!("../../../src/lib/components/ChatGptSessionPrompt.svelte");
        let catalog = include_str!("../../../src/lib/i18n/catalog.ts");

        assert!(component.contains("ChatGPT new-session prompt"));
        assert!(component.contains("Session bootstrap prompt"));
        assert!(catalog.contains("请初始化或恢复项目会话"));
        assert!(catalog.contains("先调用 conversation_bootstrap"));
        assert!(catalog.contains("多文件夹尚未选择时返回候选列表"));
        assert!(catalog.contains("同一次调用内完成 compact history bootstrap"));
        assert!(!catalog.contains("如果目标不是当前文件夹"));
        assert!(catalog.contains("all_history_summary"));
        assert!(catalog.contains("history_session_checkpoint"));
        assert!(!component.contains("打开连接器设置"));
    }

    #[test]
    fn chatgpt_session_metadata_is_injected_for_history_and_conversation_bootstrap_tools() {
        let params = json!({
            "arguments": {"session_key": "explicit"},
            "_meta": {"openai/session": "chatgpt-conversation"}
        });
        let history = tool_arguments("history_session_bootstrap", &params);
        assert_eq!(history["session_key"], "explicit");
        assert_eq!(history["_host_session_key"], "chatgpt-conversation");

        let conversation = tool_arguments("conversation_bootstrap", &params);
        assert_eq!(conversation["session_key"], "explicit");
        assert_eq!(conversation["_host_session_key"], "chatgpt-conversation");

        let existing = tool_arguments("read_file", &params);
        assert_eq!(existing["session_key"], "explicit");
        assert!(existing.get("_host_session_key").is_none());
    }

    #[test]
    fn legacy_edit_names_are_canonicalized_before_catalog_validation() {
        let params = json!({
            "arguments": {
                "path": "main.rs",
                "expected_sha256": "a".repeat(64),
                "edits": [{
                    "type": "replace",
                    "old_text": "old",
                    "new_text": "new"
                }],
                "dry_run": true,
                "reason": "legacy compatibility"
            }
        });
        let arguments = tool_arguments("edit_file", &params);
        assert_eq!(
            crate::tools::registry::canonical_tool_name("edit_file"),
            "edit"
        );
        assert_eq!(
            crate::tools::registry::canonical_tool_name("edit_many"),
            "edit"
        );
        assert_eq!(arguments["files"].as_array().unwrap().len(), 1);
        assert_eq!(arguments["files"][0]["path"], "main.rs");
        assert_eq!(arguments["files"][0]["expected_sha256"], "a".repeat(64));
        assert_eq!(arguments["files"][0]["edits"].as_array().unwrap().len(), 1);
        assert_eq!(arguments["dry_run"], true);
        assert_eq!(arguments["reason"], "legacy compatibility");
        assert!(arguments.get("path").is_none());
    }

    #[test]
    fn explicit_session_key_prevents_changed_chatgpt_metadata_from_redirecting_history() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        let response = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "history_session_bootstrap",
                    "arguments": {"session_key": "explicit-session"},
                    "_meta": {"openai/session": "chatgpt-session"}
                }
            }),
        );
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["ok"], true);
        assert_eq!(structured["session_key_source"], "explicit_session_key");
        assert_eq!(structured["session_key"], "explicit-session");
        assert_eq!(structured["host_session_key_mismatch"], true);
        let content = fs::read_to_string(workspace.path().join("docs/history-session/1.md"))
            .expect("read history file");
        assert!(content.contains("**Session key:** explicit-session"));
        assert!(!content.contains("**Session key:** chatgpt-session"));
    }

    #[test]
    fn removed_tool_names_return_refreshable_catalog_errors() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        let response = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "grep_text",
                    "arguments": {"query": "needle", "path": "."}
                }
            }),
        );

        assert_eq!(response["error"]["data"]["error_code"], "UNKNOWN_TOOL");
        assert_eq!(response["error"]["data"]["retryable"], true);
        assert!(response["error"]["data"]["available_tools"]
            .as_array()
            .expect("available tools")
            .iter()
            .all(|tool| tool != "grep_text"));
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn modern_tasks_project_retained_exec_sessions_through_lifecycle() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        let session_key = format!("rust-tasks-{}", uuid::Uuid::new_v4());
        let meta = json!({
            "openai/session": session_key,
            "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {
                "extensions": {"io.modelcontextprotocol/tasks": {}}
            }
        });

        #[cfg(windows)]
        let complete_command = "powershell -NoProfile -Command \"Start-Sleep -Milliseconds 200; Write-Output rust-task-complete\"";
        #[cfg(unix)]
        let complete_command = "sh -c \"sleep 0.2; printf rust-task-complete\"";

        let created = handle_request_async(
            state.clone(),
            json!({
                "jsonrpc": "2.0",
                "id": 201,
                "method": "tools/call",
                "params": {
                    "name": "exec_command",
                    "arguments": {
                        "cmd": complete_command,
                        "yield_time_ms": 0,
                        "timeout_ms": 5000,
                        "output_mode": "tail"
                    },
                    "_meta": meta.clone()
                }
            }),
        )
        .await;
        assert_eq!(created["result"]["resultType"], "task", "{created}");
        assert_eq!(created["result"]["status"], "working", "{created}");
        let task_id = created["result"]["taskId"]
            .as_str()
            .expect("task id")
            .to_string();
        assert!(task_id.starts_with("exec:"));
        assert_eq!(created["result"]["ttlMs"], 60 * 60_000);
        assert_eq!(created["result"]["pollIntervalMs"], 1_000);

        let unadvertised = handle_request_async(
            state.clone(),
            json!({
                "jsonrpc": "2.0",
                "id": 202,
                "method": "tasks/get",
                "params": {
                    "taskId": task_id,
                    "_meta": {
                        "openai/session": session_key,
                        "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSION
                    }
                }
            }),
        )
        .await;
        assert_eq!(unadvertised["error"]["code"], -32601);

        let mut completed = None;
        for attempt in 0..40 {
            let response = handle_request_async(
                state.clone(),
                json!({
                    "jsonrpc": "2.0",
                    "id": 210 + attempt,
                    "method": "tasks/get",
                    "params": {"taskId": task_id, "_meta": meta.clone()}
                }),
            )
            .await;
            assert_eq!(response["result"]["resultType"], "complete", "{response}");
            if response["result"]["status"] == "completed" {
                completed = Some(response["result"].clone());
                break;
            }
            assert_eq!(response["result"]["status"], "working", "{response}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let completed = completed.expect("retained process task should complete");
        assert_eq!(completed["result"]["resultType"], "complete");
        assert_eq!(completed["result"]["isError"], false);
        assert!(completed["result"]["structuredContent"]["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("rust-task-complete"));

        #[cfg(windows)]
        let cancel_command = "powershell -NoProfile -Command \"Start-Sleep -Seconds 5\"";
        #[cfg(unix)]
        let cancel_command = "sh -c \"sleep 5\"";

        let cancellable = handle_request_async(
            state.clone(),
            json!({
                "jsonrpc": "2.0",
                "id": 260,
                "method": "tools/call",
                "params": {
                    "name": "exec_command",
                    "arguments": {
                        "cmd": cancel_command,
                        "yield_time_ms": 0,
                        "timeout_ms": 10_000,
                        "output_mode": "none"
                    },
                    "_meta": meta.clone()
                }
            }),
        )
        .await;
        assert_eq!(cancellable["result"]["resultType"], "task", "{cancellable}");
        let cancellable_id = cancellable["result"]["taskId"]
            .as_str()
            .expect("cancellable task id")
            .to_string();

        let update = handle_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 261,
                "method": "tasks/update",
                "params": {
                    "taskId": cancellable_id,
                    "inputResponses": {"unused": {"action": "accept"}},
                    "_meta": meta.clone()
                }
            }),
        );
        assert_eq!(update["error"]["code"], -32602);
        assert_eq!(
            update["error"]["data"]["error_code"],
            "TASK_NOT_INPUT_REQUIRED"
        );

        let cancelled = handle_request_async(
            state.clone(),
            json!({
                "jsonrpc": "2.0",
                "id": 262,
                "method": "tasks/cancel",
                "params": {"taskId": cancellable_id, "_meta": meta.clone()}
            }),
        )
        .await;
        assert_eq!(cancelled["result"], json!({}), "{cancelled}");

        let tombstone = handle_request_async(
            state,
            json!({
                "jsonrpc": "2.0",
                "id": 263,
                "method": "tasks/get",
                "params": {"taskId": cancellable_id, "_meta": meta}
            }),
        )
        .await;
        assert_eq!(tombstone["result"]["resultType"], "complete", "{tombstone}");
        assert_eq!(tombstone["result"]["status"], "cancelled", "{tombstone}");
        assert!(tombstone["result"].get("result").is_none());
    }

    #[tokio::test]
    async fn async_dispatch_routes_session_controls_without_blocking_workers() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        let response = handle_request_async(
            state,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "wait_command",
                    "arguments": {"session_id": "missing", "timeout_ms": 1}
                }
            }),
        )
        .await;
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["execution_lane"], "async_control");
        assert_eq!(structured["blocking_queue_wait_ms"], 0);
    }

    #[tokio::test]
    async fn async_dispatch_keeps_lightweight_control_calls_inline() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        let response = handle_request_async(
            state,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "server_info", "arguments": {}}
            }),
        )
        .await;
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["execution_lane"], "inline_fast");
        assert_eq!(structured["blocking_queue_wait_ms"], 0);
        assert_eq!(structured["admission_lane"], "fast");
        assert_eq!(structured["admission_limit"], 0);
        assert_eq!(structured["admission_queue_wait_ms"], 0);
    }

    #[tokio::test]
    async fn process_admission_queues_without_blocking_the_async_runtime() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        let (_, _, admission, _, _) = state
            .admission_for("exec_command")
            .expect("process admission");
        let process_limit = admission.available_permits();
        assert!(process_limit > 0, "process admission must have capacity");
        let permits = admission
            .acquire_many_owned(process_limit as u32)
            .await
            .expect("reserve process lane");

        #[cfg(windows)]
        let command = "cmd /d /c echo admitted";
        #[cfg(unix)]
        let command = "sh -c \"printf admitted\"";

        let queued_state = state.clone();
        let command = command.to_string();
        let queued = tokio::spawn(async move {
            handle_request_async(
                queued_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "exec_command",
                        "arguments": {
                            "cmd": command,
                            "yield_time_ms": 5000,
                            "timeout_ms": 5000
                        }
                    }
                }),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!queued.is_finished(), "process request bypassed admission");

        let fast_started = Instant::now();
        let fast = handle_request_async(
            state,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "server_info", "arguments": {}}
            }),
        )
        .await;
        assert!(
            fast_started.elapsed() < Duration::from_millis(100),
            "{fast}"
        );
        assert_eq!(
            fast["result"]["structuredContent"]["execution_lane"],
            "inline_fast"
        );

        drop(permits);
        let response = queued.await.expect("queued tool task");
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["execution_lane"], "async_process");
        assert_eq!(structured["admission_lane"], "process");
        assert_eq!(structured["admission_limit"], json!(process_limit));
        assert!(
            structured["admission_queue_wait_ms"]
                .as_u64()
                .is_some_and(|wait| wait >= 80),
            "{structured}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn async_exec_wait_does_not_block_fast_control_calls() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        #[cfg(windows)]
        let command =
            "powershell -NoProfile -Command \"Start-Sleep -Milliseconds 700; Write-Output done\"";
        #[cfg(unix)]
        let command = "sh -c \"sleep 0.7; printf done\"";

        let exec_state = state.clone();
        let exec = tokio::spawn(async move {
            handle_request_async(
                exec_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "exec_command",
                        "arguments": {
                            "cmd": command,
                            "yield_time_ms": 5000,
                            "timeout_ms": 5000,
                            "output_mode": "tail"
                        }
                    }
                }),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let fast_started = Instant::now();
        let fast = handle_request_async(
            state,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "server_info", "arguments": {}}
            }),
        )
        .await;
        assert!(
            fast_started.elapsed() < Duration::from_millis(500),
            "{fast}"
        );
        assert_eq!(
            fast["result"]["structuredContent"]["execution_lane"],
            "inline_fast"
        );

        let response = exec.await.expect("async exec task");
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["execution_lane"], "async_process");
        assert_eq!(structured["blocking_queue_wait_ms"], 0);
        assert_eq!(structured["admission_lane"], "process");
        assert!(structured["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("done"));
    }

    #[tokio::test]
    #[serial_test::serial(process_runtime)]
    async fn cancelled_exec_request_terminates_the_orphaned_session() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        let initial_slots = state.sessions.active_slots_available();

        #[cfg(windows)]
        let command = "powershell -NoProfile -Command \"Start-Sleep -Seconds 10\"";
        #[cfg(unix)]
        let command = "sh -c \"sleep 10\"";

        let request_state = state.clone();
        let request = tokio::spawn(async move {
            handle_request_async(
                request_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "exec_command",
                        "arguments": {
                            "cmd": command,
                            "yield_time_ms": 5000,
                            "timeout_ms": 15_000,
                            "output_mode": "none"
                        }
                    }
                }),
            )
            .await
        });

        tokio::time::timeout(Duration::from_secs(2), async {
            while state.sessions.list(false, 1).is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("exec session should be registered");
        assert_eq!(state.sessions.active_slots_available(), initial_slots - 1);

        request.abort();
        assert!(request
            .await
            .expect_err("request should be cancelled")
            .is_cancelled());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            state.sessions.active_slots_available(),
            initial_slots - 1,
            "detached session should remain attachable during the reconnect grace period"
        );

        tokio::time::timeout(Duration::from_secs(3), async {
            while state.sessions.active_slots_available() != initial_slots {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled request should release the active session slot");
    }

    #[tokio::test]
    async fn permission_resume_exec_uses_the_async_process_lane() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let context =
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context");
        let runtime = context.runtime_config();
        context.update_runtime_config(
            runtime.policy,
            "guarded-core".into(),
            runtime.permission_mode,
        );
        let state = Arc::new(context);

        #[cfg(windows)]
        let shell = "powershell";
        #[cfg(windows)]
        let command = "Write-Output async-permission-resumed";
        #[cfg(unix)]
        let shell = "sh";
        #[cfg(unix)]
        let command = "printf async-permission-resumed";

        let blocked = handle_request_async(
            state.clone(),
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "exec_command",
                    "arguments": {
                        "cmd": command,
                        "shell": shell,
                        "yield_time_ms": 5000,
                        "timeout_ms": 5000
                    }
                }
            }),
        )
        .await;
        let resume_id = blocked["result"]["structuredContent"]["error"]["details"]
            ["permission_request"]["resume_id"]
            .as_str()
            .expect("resume id")
            .to_string();

        let resumed = handle_request_async(
            state,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "request_permissions",
                    "arguments": {
                        "resume_id": resume_id,
                        "approve": true,
                        "confirm": true,
                        "scope": "once"
                    }
                }
            }),
        )
        .await;
        let structured = &resumed["result"]["structuredContent"];
        assert_eq!(
            structured["execution_lane"], "async_permission",
            "{structured}"
        );
        assert_eq!(structured["resumed_execution_lane"], "async_process");
        assert_eq!(structured["resumed"], true);
        assert_eq!(structured["command_ok"], true, "{structured}");
        assert!(structured["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("async-permission-resumed"));
    }

    #[tokio::test]
    async fn long_wait_does_not_block_unrelated_tool_calls() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );

        #[cfg(windows)]
        let command = "powershell -NoProfile -Command \"Start-Sleep -Milliseconds 900\"";
        #[cfg(unix)]
        let command = "sh -c \"sleep 0.9\"";

        let started = handle_request_async(
            state.clone(),
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "exec_command",
                    "arguments": {
                        "cmd": command,
                        "yield_time_ms": 0,
                        "timeout_ms": 5000,
                        "output_mode": "none"
                    }
                }
            }),
        )
        .await;
        let session_id = started["result"]["structuredContent"]["session_id"]
            .as_str()
            .expect("session id")
            .to_string();

        let wait_state = state.clone();
        let wait = tokio::spawn(async move {
            handle_request_async(
                wait_state,
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "wait_command",
                        "arguments": {
                            "session_id": session_id,
                            "timeout_ms": 700,
                            "output_mode": "none"
                        }
                    }
                }),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(25)).await;

        let unrelated_started = Instant::now();
        let unrelated = handle_request_async(
            state,
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "server_info", "arguments": {}}
            }),
        )
        .await;
        assert!(
            unrelated_started.elapsed() < Duration::from_millis(300),
            "{unrelated}"
        );
        assert_eq!(
            unrelated["result"]["structuredContent"]["execution_lane"],
            "inline_fast"
        );

        let waited = wait.await.expect("wait task");
        assert_eq!(
            waited["result"]["structuredContent"]["execution_lane"],
            "async_control"
        );
    }

    #[tokio::test]
    #[ignore = "manual runtime benchmark"]
    async fn runtime_benchmark_100_fast_lane_calls() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        let started = Instant::now();
        let mut tasks = tokio::task::JoinSet::new();
        for id in 0..100 {
            let state = state.clone();
            tasks.spawn(async move {
                handle_request_async(
                    state,
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": "tools/call",
                        "params": {"name": "server_info", "arguments": {}}
                    }),
                )
                .await
            });
        }
        let mut completed = 0usize;
        while let Some(result) = tasks.join_next().await {
            let response = result.expect("fast task");
            assert_eq!(
                response["result"]["structuredContent"]["execution_lane"],
                "inline_fast"
            );
            completed += 1;
        }
        let elapsed = started.elapsed();
        println!(
            "{}",
            json!({
                "benchmark": "fast_lane_100",
                "completed": completed,
                "duration_ms": elapsed.as_millis(),
                "calls_per_second": completed as f64 / elapsed.as_secs_f64()
            })
        );
        assert_eq!(completed, 100);
    }

    #[tokio::test]
    #[ignore = "manual runtime benchmark"]
    async fn runtime_benchmark_16_active_process_sessions() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let harness = tempfile::tempdir().expect("harness tempdir");
        let state = Arc::new(
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .expect("tool context"),
        );
        #[cfg(windows)]
        let command = "powershell -NoProfile -Command \"Start-Sleep -Seconds 30\"";
        #[cfg(unix)]
        let command = "sh -c \"sleep 30\"";

        let started = Instant::now();
        let mut sessions = Vec::new();
        for id in 0..16 {
            let response = handle_request_async(
                state.clone(),
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "tools/call",
                    "params": {
                        "name": "exec_command",
                        "arguments": {
                            "cmd": command,
                            "yield_time_ms": 0,
                            "timeout_ms": 60000,
                            "output_mode": "none"
                        }
                    }
                }),
            )
            .await;
            sessions.push(
                response["result"]["structuredContent"]["session_id"]
                    .as_str()
                    .expect("session id")
                    .to_string(),
            );
        }
        assert_eq!(state.sessions.active_slots_available(), 0);
        for (id, session_id) in sessions.into_iter().enumerate() {
            let killed = handle_request_async(
                state.clone(),
                json!({
                    "jsonrpc": "2.0",
                    "id": 100 + id,
                    "method": "tools/call",
                    "params": {
                        "name": "kill_session",
                        "arguments": {"session_id": session_id, "wait_ms": 10000}
                    }
                }),
            )
            .await;
            assert_eq!(killed["result"]["structuredContent"]["killed"], true);
        }
        println!(
            "{}",
            json!({
                "benchmark": "active_process_sessions_16",
                "duration_ms": started.elapsed().as_millis(),
                "active_slots_available": state.sessions.active_slots_available()
            })
        );
        assert_eq!(state.sessions.active_slots_available(), 16);
    }
}
