//! Dedicated capability endpoint. Ordinary OAuth and its tool exposure are unchanged.
use super::*;
use crate::tools::chat::seeds;
use axum::extract::Path;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
static WAITERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static PROBES: OnceLock<Mutex<std::collections::HashMap<std::path::PathBuf, Instant>>> =
    OnceLock::new();
const TOOLS: &[&str] = &[
    "chat_open",
    "chat_wait",
    "chat_reply",
    "chat_upload",
    "chat_close",
    "set_todos",
    "update_plan",
    "report_progress",
    "read_file",
    "list_files",
    "list_directory",
    "search_files",
    "search_text",
    "search_code",
    "grep",
    "glob",
    "view_image",
    "apply_patch",
    "edit",
    "read_many",
    "project_map",
    "patch_check",
    "format_files",
    "edit_file",
    "write_file",
    "file_ops",
    "exec_command",
    "wait_command",
    "send_input",
    "kill_session",
    "read_output",
    "git_status",
    "git_diff",
    "git_log",
];
/// Same tool set as an OAuth project connection: both channels are equal.
fn seed_tool_allowed(name: &str) -> bool {
    name != "switch_workspace_folder"
        && (TOOLS.contains(&name) || crate::tools::project_scope::allowed(name))
}
const SUPPORTED_PROTOCOLS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];
fn wrapped(value: Value) -> Value {
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false})
}
fn wait_tool() -> Value {
    json!({"name":"seed_wait","description":"Wait for a conversation assignment. One independent wait at a time; idle is normal. Then chat_open with returned chat_id/attachment_id.","inputSchema":{"type":"object","properties":{"timeout_ms":{"type":"integer","minimum":0,"maximum":25000}},"required":["timeout_ms"],"additionalProperties":false}})
}
fn connect_tool() -> Value {
    json!({"name":"seed_connect","description":"Register this seed as ready; return its only workspace, full connection Skill and assignment state. Then chat_wait(timeout_ms:25000).","inputSchema":{"type":"object","properties":{},"additionalProperties":false}})
}
fn scoped_tool(mut tool: Value) -> Value {
    if let Some(required) = tool["inputSchema"]["required"].as_array_mut() {
        required.retain(|key| {
            !matches!(
                key.as_str(),
                Some("chat_id" | "attachment_id" | "workspace_folder_id")
            )
        });
    }
    if tool["name"] == "chat_wait" {
        tool["inputSchema"]["properties"]["timeout_ms"]["maximum"] = json!(25000);
        tool["description"] = json!("Wait for seed assignment or a chat message. First assignment returns status=assigned with full Skill/session; read it then wait again. Identity is bound by this endpoint.");
    }
    tool
}
pub(super) async fn get(
    State(state): State<ListenerState>,
    Path((folder_id, key)): Path<(String, String)>,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    headers: HeaderMap,
) -> Response {
    if headers.get("accept").and_then(|v| v.to_str().ok()) == Some("text/event-stream") {
        return (StatusCode::METHOD_NOT_ALLOWED, [("allow", "POST")]).into_response();
    }
    let result = (|| -> Result<Value, String> {
        let state = super::gateway::project_state(&state, &folder_id)?;
        if !crate::tools::registry::exposed_tool_names(&state.mcp.runtime_config().tool_profile)
            .contains(&"chat_open")
        {
            return Err("Seed unavailable".into());
        }
        let folders = crate::tools::hub::list_workspace_folders(&state.mcp, None);
        let folder = folders["folders"]
            .as_array()
            .and_then(|rows| rows.iter().find(|f| f["id"] == folder_id))
            .ok_or("Seed unavailable")?;
        let root = std::path::PathBuf::from(folder["path"].as_str().ok_or("Seed unavailable")?);
        seeds::url(&root, &key, false).map_err(|e| e.message())?;
        Ok(
            json!({"name":"coding-tools-seed","transport":"streamable-http","instructions":seeds::skill()["text"]}),
        )
    })();
    let wants_json = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json"));
    let mut response = match result {
        Ok(value) if wants_json => json_no_store(value),
        Ok(_) => {
            let base = base_url(&headers, &uri, None);
            page(&headers, guide_text(&base), &base, None)
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            json_no_store(json!({"error":"Seed unavailable"})),
        )
            .into_response(),
    };
    response.headers_mut().insert(
        "referrer-policy",
        axum::http::HeaderValue::from_static("no-referrer"),
    );
    response
}
struct WaitGuard(String);
/// The Bearer-header pairing form was removed in 0.1.147: the token is now the
/// last segment of one webhook URL.
pub(super) async fn provision_removed() -> Response {
    (
        StatusCode::GONE,
        json_no_store(json!({"error":"Use the seed webhook URL from the app (address and token are now one URL)"})),
    )
        .into_response()
}
pub(super) async fn provision(
    State(state): State<ListenerState>,
    Path((folder_id, token)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Response {
    let result = (|| -> Result<Value, String> {
        let state = super::gateway::project_state(&state, &folder_id)?;
        if !crate::tools::registry::exposed_tool_names(&state.mcp.runtime_config().tool_profile)
            .contains(&"chat_open")
        {
            return Err("Seed library unavailable".into());
        }
        if !body.is_object() || body.to_string().len() > 16384 {
            return Err("Invalid pairing request".into());
        }
        let folders = crate::tools::hub::list_workspace_folders(&state.mcp, None);
        let folder = folders["folders"]
            .as_array()
            .and_then(|v| v.iter().find(|f| f["id"] == folder_id))
            .ok_or("Seed library unavailable")?;
        let root =
            std::path::PathBuf::from(folder["path"].as_str().ok_or("Seed library unavailable")?);
        let mut result = seeds::provision(&root, &token, &body).map_err(|e| e.message())?;
        result["workspace_folder_id"] = json!(folder_id);
        Ok(result)
    })();
    match result {
        Ok(value) => json_no_store(value),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            json_no_store(json!({"error":error})),
        )
            .into_response(),
    }
}
impl Drop for WaitGuard {
    fn drop(&mut self) {
        if let Ok(mut waiters) = WAITERS.get_or_init(Default::default).lock() {
            waiters.remove(&self.0);
        }
    }
}
pub(super) async fn post(
    State(state): State<ListenerState>,
    Path((folder_id, seed_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    match execute(state, &folder_id, &seed_id, &headers, body).await {
        Ok(None) => StatusCode::ACCEPTED.into_response(),
        Ok(Some(result)) => json_no_store(json!({"jsonrpc":"2.0","id":id,"result":result})),
        Err(message) => json_no_store(
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":message}}),
        ),
    }
}
async fn execute(
    state: ListenerState,
    folder_id: &str,
    seed_id: &str,
    headers: &HeaderMap,
    body: Value,
) -> Result<Option<Value>, String> {
    let state = super::gateway::project_state(&state, folder_id)?;
    if body["jsonrpc"] != "2.0" || !body["method"].is_string() {
        return Err("Invalid JSON-RPC request".into());
    }
    let folders = crate::tools::hub::list_workspace_folders(&state.mcp, None);
    let folder = folders["folders"]
        .as_array()
        .and_then(|folders| folders.iter().find(|f| f["id"] == folder_id))
        .ok_or("Seed unavailable")?;
    let root = std::path::PathBuf::from(folder["path"].as_str().ok_or("Seed unavailable")?);
    let token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    let method = body["method"].as_str().unwrap_or("");
    let params = &body["params"];
    let protocol = headers
        .get("mcp-protocol-version")
        .and_then(|h| h.to_str().ok());
    // Accept a missing header (curl users, older clients) and every
    // published revision; the seed protocol surface is identical for them.
    if method != "initialize" && protocol.is_some_and(|p| !SUPPORTED_PROTOCOLS.contains(&p)) {
        return Err("Unsupported MCP-Protocol-Version".into());
    }
    let negotiated = params["protocolVersion"]
        .as_str()
        .filter(|v| SUPPORTED_PROTOCOLS.contains(v))
        .unwrap_or("2025-03-26");
    if !crate::tools::registry::exposed_tool_names(&state.mcp.runtime_config().tool_profile)
        .contains(&"chat_open")
    {
        return Err("Seed access requires a tool profile with local chat enabled".into());
    }
    let url_mode = seed_id.len() == 64
        && seed_id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
    let resolved;
    let (seed_id, token) = if url_mode {
        resolved = seeds::url(&root, seed_id, method == "initialize").map_err(|e| e.message())?;
        (resolved.as_str(), seed_id)
    } else {
        (seed_id, token)
    };
    if method == "initialize" {
        if url_mode {
            return Ok(Some(
                json!({"protocolVersion":negotiated,"capabilities":{"tools":{},"resources":{}},"serverInfo":{"name":"coding-tools-seed","version":"1"},"instructions":seeds::skill()["text"]}),
            ));
        }
        let access = seeds::initialize(&root, seed_id, token).map_err(|e| e.message())?;
        return Ok(Some(
            json!({"protocolVersion":negotiated,"capabilities":{"tools":{},"resources":{}},"serverInfo":{"name":"coding-tools-seed","version":"1"},"instructions":seeds::skill()["text"],"_meta":{"seed_access_token":access}}),
        ));
    }
    if method == "seed/created" {
        seeds::created(
            &root,
            seed_id,
            token,
            params["task_id"].as_str().unwrap_or(""),
        )
        .map_err(|e| e.message())?;
        return Ok(Some(json!({"ok":true})));
    }
    seeds::authenticate(&root, seed_id, token).map_err(|e| e.message())?;
    if method == "notifications/initialized" && body.get("id").is_none() {
        return Ok(None);
    }
    if body.get("id").is_none() {
        return Err("Request ID required".into());
    }
    let result = match method {
        "tools/list" => {
            let response = handle_request_async(
                state.mcp.clone(),
                json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            )
            .await;
            let mut tools = vec![
                connect_tool(),
                json!({"name":"list_workspace_folders","description":"Show the single project authorized for this seed.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}),
                wait_tool(),
            ];
            tools.extend(
                response["result"]["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|t| t["name"].as_str().is_some_and(seed_tool_allowed))
                    .cloned()
                    .map(scoped_tool),
            );
            json!({"tools":tools})
        }
        "resources/read" if params["uri"] == seeds::skill()["uri"] => {
            json!({"contents":[seeds::skill()]})
        }
        "resources/read" if params["uri"] == crate::tools::chat::LOCAL_CHAT_SKILL_URI => {
            json!({"contents":[crate::tools::chat::local_chat_skill()]})
        }
        "resources/list" => {
            json!({"resources":[{"uri":"coding-tools://skills/seed-connect","name":"seed-connect","mimeType":"text/markdown"},crate::tools::chat::local_chat_skill_resource()]})
        }
        "ping" => json!({}),
        "tools/call" => {
            let mut name = params["name"].as_str().unwrap_or("");
            let mut args = params.get("arguments").cloned().unwrap_or(json!({}));
            if !args.is_object() {
                return Err("Arguments must be an object".into());
            }
            if name == "seed_connect" {
                let mut status = seeds::poll_scoped(
                    &root,
                    seed_id,
                    token,
                    folder_id,
                    state.workspace_id == crate::data::GLOBAL_MCP_ID,
                )
                .map_err(|e| e.message())?;
                status["registered"] = json!(status["status"] != "retired");
                status["seed_id"] = json!(seed_id);
                status["workspace_folder_id"] = json!(folder_id);
                status["workspace"] = folder.clone();
                status["skill"] = seeds::skill();
                status["instruction"] =
                    json!("Read skill.text, then immediately chat_wait(timeout_ms:25000). Keep calling chat_wait one at a time until assigned. The user writes in the app at any time; idle only means no new message yet, so just wait again.");
                return Ok(Some(wrapped(status)));
            }
            let chat_wait = name == "chat_wait";
            let _chat_guard = if chat_wait {
                let key = format!("{}:{seed_id}", root.display());
                let mut waiters = WAITERS
                    .get_or_init(Default::default)
                    .lock()
                    .map_err(|_| "Seed wait lock unavailable")?;
                if !waiters.insert(key.clone()) {
                    return Err("A seed_wait is already active".into());
                }
                Some(WaitGuard(key))
            } else {
                None
            };
            let mut auto_open = false;
            if chat_wait {
                args["timeout_ms"]
                    .as_u64()
                    .filter(|n| *n <= 25000)
                    .ok_or("timeout_ms must be 0–25000")?;
                let status = seeds::poll_scoped(
                    &root,
                    seed_id,
                    token,
                    folder_id,
                    state.workspace_id == crate::data::GLOBAL_MCP_ID,
                )
                .map_err(|e| e.message())?;
                if status["status"] == "retired" {
                    return Ok(Some(wrapped(
                        json!({"status":"retired","registered":false,"instruction":"Seed retired. Stop waiting and do not switch identities."}),
                    )));
                }
                if crate::tools::chat::standby_handoff(&args).is_some() {
                    // Returned to standby: the old chat IDs no longer apply, even when this
                    // same poll already assigned the seed to another chat.
                    if let Some(map) = args.as_object_mut() {
                        map.remove("chat_id");
                        map.remove("attachment_id");
                    }
                }
                if args.get("chat_id").is_some() && args["chat_id"] != status["chat_id"] {
                    return Err("Seed may only access its assigned chat".into());
                }
                if args.get("attachment_id").is_some()
                    && args["attachment_id"] != status["attachment_id"]
                {
                    return Err("Seed attachment mismatch".into());
                }
                if status["status"] != "assigned" {
                    name = "seed_wait";
                } else if status["needs_open"] == true {
                    name = "chat_open";
                    auto_open = true;
                }
            }
            if name == "list_workspace_folders" {
                wrapped(json!({"ok":true,"folders":[folder],"selected_folder_id":folder_id}))
            } else if name == "seed_wait" {
                let timeout = args["timeout_ms"]
                    .as_u64()
                    .filter(|n| *n <= 25000)
                    .ok_or("timeout_ms must be 0–25000")?;
                let probe_due = {
                    let mut times = PROBES
                        .get_or_init(Default::default)
                        .lock()
                        .map_err(|_| "Seed probe lock unavailable")?;
                    let due = times
                        .get(&root)
                        .is_none_or(|last| last.elapsed() >= Duration::from_secs(5));
                    if due {
                        times.insert(root.clone(), Instant::now());
                    }
                    due
                };
                let pending = if probe_due {
                    seeds::pending_processes(&root).map_err(|e| e.message())?
                } else {
                    vec![]
                };
                for (owner, session) in pending {
                    let check=handle_request_async(state.mcp.clone(),json!({"jsonrpc":"2.0","id":"seed-recovery-probe","method":"tools/call","params":{"name":"wait_command","arguments":{"workspace_folder_id":folder_id,"session_id":session,"timeout_ms":0},"_meta":{"openai/session":format!("seed:{owner}")}}})).await;
                    let result = &check["result"]["structuredContent"];
                    if result["ok"] != false && result["process_still_running"] == false {
                        seeds::process_settled(&root, &owner, &session).map_err(|e| e.message())?;
                    }
                }
                let key = format!("{}:{seed_id}", root.display());
                {
                    let mut waiters = WAITERS
                        .get_or_init(Default::default)
                        .lock()
                        .map_err(|_| "Seed wait lock unavailable")?;
                    if !chat_wait && !waiters.insert(key.clone()) {
                        return Err("A seed_wait is already active".into());
                    }
                }
                let _guard = WaitGuard(key);
                let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
                loop {
                    let result = seeds::poll_scoped(
                        &root,
                        seed_id,
                        token,
                        folder_id,
                        state.workspace_id == crate::data::GLOBAL_MCP_ID,
                    )
                    .map_err(|e| e.message())?;
                    if result["status"] != "idle" || tokio::time::Instant::now() >= deadline {
                        if chat_wait && result["status"] == "assigned" {
                            let binding = seeds::begin_for_folder(
                                &root,
                                seed_id,
                                token,
                                "chat_open",
                                &args,
                                &body["id"].to_string(),
                                folder_id,
                            )
                            .map_err(|e| e.message())?;
                            let routed_id = binding["workspace_folder_id"]
                                .as_str()
                                .ok_or("Assigned project is unavailable")?;
                            let target_id = if routed_id == root.to_string_lossy() {
                                folder_id
                            } else {
                                routed_id
                            };
                            let target_state = super::gateway::project_state(&state, target_id)?;
                            let open_args = json!({"chat_id":binding["chat_id"],"attachment_id":binding["attachment_id"],"workspace_folder_id":target_id});
                            let response = handle_request_async(target_state.mcp.clone(),json!({"jsonrpc":"2.0","id":body["id"],"method":"tools/call","params":{"name":"chat_open","arguments":open_args,"_meta":{"openai/session":format!("seed:{seed_id}")}}})).await;
                            let mut actual = response["result"]["structuredContent"].clone();
                            seeds::finish(
                                &root,
                                seed_id,
                                binding["call_id"].as_str().unwrap_or(""),
                                &open_args,
                                &actual,
                            )
                            .map_err(|e| e.message())?;
                            if actual["ok"] != true {
                                return Err("Could not open assigned chat; retry chat_wait".into());
                            }
                            actual["status"] = json!("assigned");
                            actual["chat_id"] = binding["chat_id"].clone();
                            actual["workspace_folder_id"] = json!(target_id);
                            break wrapped(actual);
                        }
                        let mut result = result;
                        if chat_wait {
                            result["registered"] = json!(result["status"] != "retired");
                            result["instruction"] = json!(if result["status"] == "retired" {
                                "Seed retired. Stop waiting and do not switch identities."
                            } else {
                                "Immediately call chat_wait(timeout_ms:25000) again. Idle is normal. The user writes in the app at any time; idle only means no new message yet, so just wait again."
                            });
                        }
                        break wrapped(result);
                    }
                    tokio::time::sleep(
                        Duration::from_secs(5)
                            .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
                    )
                    .await;
                }
            } else {
                if !seed_tool_allowed(name) {
                    return Err("Tool not available to seeds".into());
                }
                let request_id = body["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| body["id"].to_string());
                let binding = seeds::begin_for_folder(
                    &root,
                    seed_id,
                    token,
                    name,
                    &args,
                    &request_id,
                    folder_id,
                )
                .map_err(|e| e.message())?;
                if auto_open {
                    args.as_object_mut().unwrap().remove("timeout_ms");
                }
                let routed_id = binding["workspace_folder_id"]
                    .as_str()
                    .ok_or("Assigned project is unavailable")?;
                let target_id = if routed_id == root.to_string_lossy() {
                    folder_id
                } else {
                    routed_id
                };
                let target_state = super::gateway::project_state(&state, target_id)?;
                args["workspace_folder_id"] = json!(target_id);
                if name.starts_with("chat_")
                    || matches!(name, "set_todos" | "update_plan" | "report_progress")
                {
                    args["chat_id"] = binding["chat_id"].clone();
                    args["attachment_id"] = binding["attachment_id"].clone();
                }
                if name == "exec_command" && args.get("operation_id").is_none() {
                    args["operation_id"] = json!(format!(
                        "seed-{seed_id}-{}",
                        binding["call_id"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(20)
                            .collect::<String>()
                    ));
                }
                // The chat's authorization mode (完全/一般/严格) applies to project tools.
                if !name.starts_with("chat_")
                    && !matches!(name, "set_todos" | "update_plan" | "report_progress")
                {
                    if let Some(chat_id) = binding["chat_id"]
                        .as_str()
                        .filter(|c| !c.is_empty())
                        .map(str::to_owned)
                    {
                        let target_root =
                            crate::tools::hub::list_workspace_folders(&target_state.mcp, None)
                                ["folders"]
                                .as_array()
                                .and_then(|folders| {
                                    folders.iter().find(|folder| folder["id"] == target_id)
                                })
                                .and_then(|folder| folder["path"].as_str())
                                .map(std::path::PathBuf::from)
                                .ok_or("Assigned project is unavailable")?;
                        // Recall/pause/correction: the next project tool call tells the AI to stop.
                        if let Some(notice) =
                            crate::tools::chat::control::stop_notice_at(&target_root, &chat_id)
                        {
                            seeds::finish(
                                &root,
                                seed_id,
                                binding["call_id"].as_str().unwrap_or(""),
                                &args,
                                &notice,
                            )
                            .map_err(|e| e.message())?;
                            let mut result = wrapped(notice);
                            result["isError"] = json!(true);
                            return Ok(Some(result));
                        }
                        match apply_access_mode(&target_root, &chat_id, name, &args).await {
                            Ok(next) => args = next,
                            Err(blocked) => {
                                seeds::finish(
                                    &root,
                                    seed_id,
                                    binding["call_id"].as_str().unwrap_or(""),
                                    &args,
                                    &blocked,
                                )
                                .map_err(|e| e.message())?;
                                return Ok(Some(wrapped(blocked)));
                            }
                        }
                    }
                }
                let response=handle_request_async(target_state.mcp,json!({"jsonrpc":"2.0","id":body["id"],"method":"tools/call","params":{"name":name,"arguments":args,"_meta":{"openai/session":format!("seed:{seed_id}")}}})).await;
                let actual = response["result"]["structuredContent"].clone();
                seeds::finish(
                    &root,
                    seed_id,
                    binding["call_id"].as_str().unwrap_or(""),
                    &args,
                    &actual,
                )
                .map_err(|e| e.message())?;
                if response.get("error").is_some() {
                    return Err(response["error"]["message"]
                        .as_str()
                        .unwrap_or("Tool failed")
                        .into());
                }
                if auto_open && actual["ok"] == true {
                    let mut actual = actual;
                    actual["status"] = json!("assigned");
                    actual["chat_id"] = binding["chat_id"].clone();
                    actual["workspace_folder_id"] = json!(target_id);
                    wrapped(actual)
                } else {
                    response["result"].clone()
                }
            }
        }
        _ => return Err("Method not available to seeds".into()),
    };
    Ok(Some(result))
}

/// Strict mode waits this long per call for the user's decision; the AI then
/// retries with `approval_id`. `CTMCP_APPROVAL_WAIT_MS` overrides it (tests).
const APPROVAL_WAIT_MS: u64 = 20_000;
async fn apply_access_mode(
    root: &std::path::Path,
    chat_id: &str,
    name: &str,
    args: &Value,
) -> std::result::Result<Value, Value> {
    use crate::tools::chat::access::{self, Gate};
    let read_only = coding_tools_tunnel_protocol::is_retry_safe_tool_name(name);
    let accepts_confirm = crate::tools::registry::input_schema(name)["properties"]
        .get("confirm")
        .is_some();
    let wait_ms = std::env::var("CTMCP_APPROVAL_WAIT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(APPROVAL_WAIT_MS);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    let mut current = args.clone();
    loop {
        let gate = access::gate(root, chat_id, name, &current, read_only, accepts_confirm)
            .map_err(|e| json!({"ok":false,"error":{"code":"ACCESS_CHECK_FAILED","category":"permission","retryable":true,"message":e.message()}}))?;
        match gate {
            Gate::Run(next) => return Ok(next),
            Gate::Blocked(result) => return Err(result),
            Gate::Wait(id) => {
                current["approval_id"] = json!(id);
                while tokio::time::Instant::now() < deadline
                    && access::status(root, chat_id, &id) == "pending"
                {
                    tokio::time::sleep(
                        Duration::from_secs(1)
                            .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
                    )
                    .await;
                }
                if access::status(root, chat_id, &id) != "pending" {
                    continue;
                }
                return Err(access::required_result(name, &id));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn seed_http_gateway_preserves_oauth_and_scopes_project_tools() {
        let root = tempfile::tempdir().unwrap();
        let harness = tempfile::tempdir().unwrap();
        let mcp = Arc::new(
            crate::tools::ToolContext::for_test(
                root.path().to_path_buf(),
                harness.path().to_path_buf(),
            )
            .unwrap(),
        );
        let state = ListenerState {
            gateway_epoch: super::gateway::current_epoch(),
            mcp,
            auth: crate::workspace::AuthConfig {
                auth_type: "bearer".into(),
                ..Default::default()
            },
            workspace_id: "seed-gateway-test".into(),
            bind_address: "127.0.0.1".into(),
            bind_port: 28766,
            configured_public_url: "https://example.test/clients/seed".into(),
            bearer_token: Some("ordinary-secret".into()),
            oauth: None,
            oauth_client_secret: None,
            transport_mode: "streamable-http".into(),
            redact_telemetry: true,
        };
        let ui = |args: Value| crate::tools::chat::ui(root.path(), &args).unwrap();
        ui(json!({"action":"seed_settings","enabled":true}));
        let seed=ui(json!({"action":"seed_batch","count":1,"account":"fixture","repo_id":"repo","branch":"main"}))["batch"][0].clone();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, super::super::routes::build_router(state)).await;
        });
        let client = reqwest::Client::new();
        let base = format!("http://{address}/clients/seed/mcp");
        let pairing = "d".repeat(64);
        ui(json!({"action":"seed_pairing","token":pairing}));
        let legacy = format!("{base}/seed-library/legacy");
        // The removed Bearer form now answers 410 Gone, even with a valid token.
        assert_eq!(
            client
                .post(&legacy)
                .json(&json!({"action":"status"}))
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::GONE
        );
        let paired = format!("{legacy}/{pairing}");
        let state = client
            .post(&paired)
            .json(&json!({"action":"status"}))
            .send()
            .await
            .unwrap();
        assert_eq!(state.status(), reqwest::StatusCode::OK);
        assert!(state.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("no-store"));
        assert_eq!(
            state.json::<Value>().await.unwrap()["workspace_folder_id"],
            "legacy"
        );
        for action in ["read_file", "tools/call"] {
            assert_eq!(
                client
                    .post(&paired)
                    .json(&json!({"action":action}))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                reqwest::StatusCode::BAD_REQUEST
            );
        }
        assert_eq!(
            client
                .post(format!("{legacy}/{}", "e".repeat(64)))
                .json(&json!({"action":"status"}))
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            client
                .post(&paired)
                .header("content-type", "application/json")
                .body("x".repeat(20000))
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            client
                .post(&base)
                .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::UNAUTHORIZED
        );
        assert!(!client
            .post(format!("{paired}/extra"))
            .json(&json!({"action":"status"}))
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
        // A newly issued URL alone is a standard MCP connection and keeps chat ownership server-side.
        let url_seed =
            ui(json!({"action":"seed_manual","account":"url-fixture"}))["batch"][0].clone();
        let url_endpoint = format!(
            "{base}/seeds/legacy/{}",
            url_seed["url_key"].as_str().unwrap()
        );
        let url_rpc = |method: &str, params: Value| {
            client
                .post(&url_endpoint)
                .header("MCP-Protocol-Version", "2025-03-26")
                .json(&json!({"jsonrpc":"2.0","id":"url-test","method":method,"params":params}))
                .send()
        };
        let guide = client.get(&url_endpoint).send().await.unwrap();
        assert!(guide.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain"));
        let guide = guide.text().await.unwrap();
        assert!(
            guide.contains("/connect?n=") && guide.contains("Streamable HTTP"),
            "{guide}"
        );
        let landing = client
            .get(&url_endpoint)
            .header("Accept", "application/json")
            .send()
            .await
            .unwrap();
        assert_eq!(landing.status(), 200);
        assert_eq!(landing.headers()["referrer-policy"], "no-referrer");
        assert!(landing.json::<Value>().await.unwrap()["instructions"]
            .as_str()
            .unwrap()
            .contains("seed_connect"));
        assert!(url_rpc("tools/call", json!({"name":"seed_connect"}))
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
            .get("error")
            .is_some());
        let init = url_rpc("initialize", json!({}))
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert!(init["result"]["_meta"].is_null());
        assert!(init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("chat_wait"));
        let connected = url_rpc("tools/call", json!({"name":"seed_connect","arguments":{}}))
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(
            connected["result"]["structuredContent"]["registered"], true,
            "{connected}"
        );
        // GET-only compatibility mode reaches the same seed with plain text.
        let page = |path: &str| client.get(format!("{url_endpoint}/{path}")).send();
        let tools = page("tools?n=a").await.unwrap().text().await.unwrap();
        assert!(
            tools.contains("chat_reply") && tools.contains("exec_command"),
            "{tools}"
        );
        let waited = page("wait?n=b&wait=0").await.unwrap().text().await.unwrap();
        assert!(
            waited.contains("status:") && waited.contains("/wait?n="),
            "{waited}"
        );
        // Next-step URLs come before the truncated JSON: fetch tools often cut long pages.
        assert!(
            waited.find("/wait?n=").unwrap() < waited.find("result:").unwrap(),
            "{waited}"
        );
        assert!(guide.contains("不需要自己有 MCP 客户端"), "{guide}");
        let html = client
            .get(format!("{url_endpoint}/wait?n=h&wait=0"))
            .header("Accept", "text/html,application/xhtml+xml")
            .send()
            .await
            .unwrap();
        assert!(html.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html"));
        let html = html.text().await.unwrap();
        assert!(
            html.contains(&format!("<a href=\"{url_endpoint}/wait?n="))
                && !html.contains("<script"),
            "{html}"
        );
        let robots = client
            .get(format!("http://{address}/robots.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(robots.status(), 200);
        assert!(robots.text().await.unwrap().contains("Allow: /"));
        let folders = page("call?n=c&tool=list_workspace_folders")
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(folders.contains("selected_folder_id"), "{folders}");
        let idle = url_rpc(
            "tools/call",
            json!({"name":"chat_wait","arguments":{"timeout_ms":0}}),
        )
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
        assert_eq!(idle["result"]["structuredContent"]["status"], "idle");
        let url_chat = ui(json!({"action":"create"}))["session"].clone();
        ui(
            json!({"action":"send","chat_id":url_chat["id"],"message_id":"url-user","text":"Continue"}),
        );
        let opened = url_rpc(
            "tools/call",
            json!({"name":"chat_wait","arguments":{"timeout_ms":0}}),
        )
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
        assert_eq!(
            opened["result"]["structuredContent"]["status"], "assigned",
            "{opened}"
        );
        assert!(opened["result"]["structuredContent"]["skill"]["text"].is_string());
        assert_eq!(
            opened["result"]["structuredContent"]["chat_id"],
            url_chat["id"]
        );
        let delivered = url_rpc(
            "tools/call",
            json!({"name":"chat_wait","arguments":{"timeout_ms":0}}),
        )
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
        assert_eq!(
            delivered["result"]["structuredContent"]["message"]["id"], "url-user",
            "{delivered}"
        );
        let replied = url_rpc("tools/call",json!({"name":"chat_reply","arguments":{"message_id":"url-answer","reply_to":"url-user","text":"Done","final":true}})).await.unwrap().json::<Value>().await.unwrap();
        assert_eq!(
            replied["result"]["structuredContent"]["persisted"], true,
            "{replied}"
        );
        // Disconnect sends the live seed back as standby. Its next wait with
        // the old IDs keeps waiting instead of failing, and it can be
        // assigned again through the standby pool.
        let old_attachment = opened["result"]["structuredContent"]["attachment_id"].clone();
        let moved = ui(json!({"action":"detach","chat_id":url_chat["id"],"standby":true}));
        assert_eq!(moved["standby"], "seed", "{moved}");
        let standby = url_rpc("tools/call",json!({"name":"chat_wait","arguments":{"timeout_ms":0,"chat_id":url_chat["id"],"attachment_id":old_attachment}})).await.unwrap().json::<Value>().await.unwrap();
        assert_eq!(
            standby["result"]["structuredContent"]["status"], "idle",
            "{standby}"
        );
        let standby_chat =
            ui(json!({"action":"create","seed_filter":{"source":"","keyword":"","standby":true}}))
                ["session"]
                .clone();
        let reassigned = url_rpc(
            "tools/call",
            json!({"name":"chat_wait","arguments":{"timeout_ms":3000}}),
        )
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
        assert_eq!(
            reassigned["result"]["structuredContent"]["chat_id"], standby_chat["id"],
            "{reassigned}"
        );
        assert!(url_rpc("initialize", json!({}))
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
            .get("result")
            .is_some());
        let denied = url_rpc(
            "tools/call",
            json!({"name":"chat_wait","arguments":{"timeout_ms":0,"chat_id":"other"}}),
        )
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
        assert!(denied.get("error").is_some());
        let closed = url_rpc("tools/call", json!({"name":"chat_close","arguments":{}}))
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(
            closed["result"]["structuredContent"]["ok"], true,
            "{closed}"
        );
        let retired = url_rpc(
            "tools/call",
            json!({"name":"chat_wait","arguments":{"timeout_ms":0}}),
        )
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
        assert_eq!(
            retired["result"]["structuredContent"]["status"], "retired",
            "{retired}"
        );
        assert!(retired["result"]["structuredContent"]["instruction"]
            .as_str()
            .unwrap()
            .contains("Stop waiting"));
        ui(json!({"action":"seed_retire","seed_id":url_seed["seed_id"]}));
        assert_eq!(
            client.get(&url_endpoint).send().await.unwrap().status(),
            404
        );
        assert!(url_rpc("initialize", json!({}))
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
            .get("error")
            .is_some());
        let endpoint = format!("{base}/seeds/legacy/{}", seed["seed_id"].as_str().unwrap());
        let rpc = |token: String, method: &str, params: Value, id: u64| {
            client
                .post(&endpoint)
                .header("Authorization", format!("Bearer {token}"))
                .header("MCP-Protocol-Version", "2025-03-26")
                .json(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .send()
        };
        let initialized = rpc(
            seed["ticket"].as_str().unwrap().into(),
            "initialize",
            json!({"protocolVersion":"2025-03-26"}),
            1,
        )
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
        let token = initialized["result"]["_meta"]["seed_access_token"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            client
                .post(&base)
                .header("Authorization", format!("Bearer {token}"))
                .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        let denied = rpc(
            token.clone(),
            "tools/call",
            json!({"name":"read_file","arguments":{"path":"sample.txt"}}),
            2,
        )
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
        assert!(denied.get("error").is_some());
        let chat = ui(json!({"action":"create"}))["session"].clone();
        let assigned = rpc(
            token.clone(),
            "tools/call",
            json!({"name":"seed_wait","arguments":{"timeout_ms":0}}),
            3,
        )
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
        let assignment = &assigned["result"]["structuredContent"];
        assert_eq!(assignment["chat_id"], chat["id"], "{assigned}");
        let opened=rpc(token.clone(),"tools/call",json!({"name":"chat_open","arguments":{"chat_id":chat["id"],"attachment_id":assignment["attachment_id"]}}),4).await.unwrap().json::<Value>().await.unwrap();
        assert_eq!(
            opened["result"]["structuredContent"]["ok"], true,
            "{opened}"
        );
        std::fs::write(root.path().join("sample.txt"), "seed project").unwrap();
        let result = rpc(
            token.clone(),
            "tools/call",
            json!({"name":"read_file","arguments":{"path":"sample.txt"}}),
            5,
        )
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
        assert_eq!(
            result["result"]["structuredContent"]["content"], "seed project",
            "{result}"
        );
        let rerouted=rpc(token.clone(),"tools/call",json!({"name":"read_file","arguments":{"path":"sample.txt","workspace_folder_id":"other"}}),6).await.unwrap().json::<Value>().await.unwrap();
        assert_eq!(
            rerouted["result"]["structuredContent"]["content"], "seed project",
            "{rerouted}"
        );
        ui(json!({"action":"seed_retire","seed_id":seed["seed_id"]}));
        let denied = rpc(
            token,
            "tools/call",
            json!({"name":"read_file","arguments":{"path":"sample.txt"}}),
            7,
        )
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
        assert!(denied.get("error").is_some());
        server.abort();
    }
}

// ---------------------------------------------------------------------------
// GET compatibility mode: AIs that can only read web pages (no MCP client, no
// HTTP POST) follow plain-text pages. Every page lists the complete next URL
// with a fresh `n` so fetch caches never replay an old answer. The URL key is
// the credential, exactly as for JSON-RPC.
// ---------------------------------------------------------------------------
/// Fetch tools usually time out after 10–30 s; keep GET waits short by default.
const GET_WAIT_DEFAULT_MS: u64 = 8_000;
const PAGE_JSON_LIMIT: usize = 6_000;
/// Seed URLs are capabilities, not public pages; fetch tools that honor
/// robots.txt may open them.
pub(super) const ROBOTS_TXT: &str = "User-agent: *\nAllow: /\n";
pub(super) async fn robots() -> Response {
    (
        [
            (CACHE_CONTROL, "no-store"),
            (CONTENT_TYPE, "text/plain; charset=utf-8"),
        ],
        ROBOTS_TXT,
    )
        .into_response()
}
fn page_headers(content_type: &'static str) -> [(axum::http::HeaderName, &'static str); 5] {
    [
        (CACHE_CONTROL, "no-store"),
        (CONTENT_TYPE, content_type),
        (axum::http::header::REFERRER_POLICY, "no-referrer"),
        (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (
            axum::http::HeaderName::from_static("x-robots-tag"),
            "noindex, nofollow",
        ),
    ]
}
fn text_no_store(text: String) -> Response {
    (page_headers("text/plain; charset=utf-8"), text).into_response()
}
fn wants_html(headers: &HeaderMap) -> bool {
    let accept = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    accept.contains("text/html") && !accept.starts_with("text/plain")
}
fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}
/// Linkify whole-token URLs only; templates with `<placeholders>` stay text.
fn linkify(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 256);
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if token.is_empty() {
            return;
        }
        let is_url = (token.starts_with("http://") || token.starts_with("https://"))
            && !token.contains(['<', '>', '"']);
        if is_url {
            let escaped = escape_html(token);
            out.push_str(&format!("<a href=\"{escaped}\">{escaped}</a>"));
        } else {
            out.push_str(&escape_html(token));
        }
        token.clear();
    };
    for c in text.chars() {
        if c.is_whitespace() {
            flush(&mut token, &mut out);
            out.push(c);
        } else {
            token.push(c);
        }
    }
    flush(&mut token, &mut out);
    out
}
pub(super) fn html_page(text: &str, base: &str, reply_to: Option<&str>) -> String {
    let form = reply_to
        .map(|to| {
            format!(
                "<form method=\"get\" action=\"{base}/reply\"><input type=\"hidden\" name=\"n\" value=\"{n}\"><input type=\"hidden\" name=\"to\" value=\"{to}\"><input type=\"hidden\" name=\"final\" value=\"1\"><textarea name=\"text\" rows=\"6\" cols=\"80\" placeholder=\"回复 Reply\"></textarea><br><button type=\"submit\">发送回复 Send reply</button></form>",
                base = escape_html(base),
                n = nonce(),
                to = escape_html(to)
            )
        })
        .unwrap_or_default();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"robots\" content=\"noindex\"><title>Coding Tools MCP</title></head><body><pre style=\"white-space:pre-wrap\">{}</pre>{form}</body></html>",
        linkify(text)
    )
}
/// Plain text for fetch tools; HTML with links and a reply form for browsers.
fn page(headers: &HeaderMap, text: String, base: &str, reply_to: Option<&str>) -> Response {
    if wants_html(headers) {
        (
            page_headers("text/html; charset=utf-8"),
            html_page(&text, base, reply_to),
        )
            .into_response()
    } else {
        text_no_store(text)
    }
}
fn nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
/// Absolute URL of this seed endpoint as the AI reached it (tunnel or local).
fn base_url(headers: &HeaderMap, uri: &axum::http::Uri, action: Option<&str>) -> String {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1");
    let loopback =
        host.starts_with("127.") || host.starts_with("localhost") || host.starts_with("[::1]");
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or(if loopback { "http" } else { "https" });
    let mut path = uri.path().trim_end_matches('/').to_owned();
    if let Some(action) = action {
        if let Some(stripped) = path.strip_suffix(&format!("/{action}")) {
            path = stripped.to_owned();
        }
    }
    format!("{proto}://{host}{path}")
}
pub(super) fn next_links(base: &str, reply_to: Option<&str>) -> String {
    let n = nonce();
    let to = reply_to
        .map(percent_encode)
        .unwrap_or_else(|| "<message.id>".to_owned());
    format!(
        "下一步 / Next (open the complete URL with your web/fetch tool):\n\
         - 等待用户消息 Wait for the next message: {base}/wait?n={n}\n\
         - 回复用户 Reply (append your URL-encoded text): {base}/reply?n={n}&to={to}&final=1&text=\n\
         - 进度回复（未完成）Progress: same as Reply with final=0；长回复可分几次 final=0，最后一次 final=1\n\
         - 调用项目工具 Call a tool: {base}/call?n={n}&tool=<name>&args=<URL-encoded JSON>  (e.g. {base}/call?n={n}&tool=read_file&args=%7B%22path%22%3A%22README.md%22%7D)\n\
         - 工具列表 Tool list: {base}/tools?n={n}"
    )
}
pub(super) fn guide_text(base: &str) -> String {
    format!(
        "Coding Tools MCP — 用户本人的本机项目连接 (the user's own local coding tool)\n\n\
         这个网址由用户在自己的 Coding Tools MCP 应用里生成，用来让你访问用户指定的项目并在应用的对话里和用户交流。\n\
         你不需要自己有 MCP 客户端或 seed_connect / chat_wait / chat_reply 工具：它们都在这个服务器上。\n\
         You do NOT need an MCP client or these tools yourself — they run on this server.\n\n\
         最简单 Easiest (web page reading is enough): 打开 Open {base}/connect?n={n}\n\
         之后每个页面都会给出下一步的完整网址，照着打开即可。Each page gives the complete next URL.\n\n\
         其他方式 Other ways:\n\
         - MCP 客户端 MCP client: 把 {base} 添加为 Streamable HTTP MCP 服务器（不需要额外认证），然后调用 seed_connect，再 chat_wait。\n\
         - HTTP POST (curl/python): POST JSON-RPC 到 {base}，Content-Type: application/json，Accept: application/json, text/event-stream；\n\
           先 initialize {{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{{}},\"clientInfo\":{{\"name\":\"ai\",\"version\":\"1\"}}}}，\n\
           再 notifications/initialized，之后请求加 MCP-Protocol-Version: 2025-03-26；tools/call seed_connect，然后 chat_wait {{\"timeout_ms\":25000}}。\n\n\
         收到消息后按说明完成工作并回复；回复后继续等待下一条消息。等待返回 idle 只表示用户还没发消息，再等一次即可。\n",
        n = nonce()
    )
}
/// Page text plus the user message id when the result carries one. The
/// next-step URLs come first: fetch tools often truncate long pages.
pub(super) fn summarize(action: &str, result: &Value, base: &str) -> (String, Option<String>) {
    let data = if result["structuredContent"].is_null() {
        result.clone()
    } else {
        result["structuredContent"].clone()
    };
    let mut out = format!(
        "action: {action}\nstatus: {}\n",
        data["status"]
            .as_str()
            .unwrap_or(if data["ok"] == true { "ok" } else { "-" })
    );
    let mut reply_to = None;
    if data["status"] == "message" {
        let message = &data["message"];
        let id = message["id"].as_str().unwrap_or("");
        if !id.is_empty() {
            reply_to = Some(id.to_owned());
        }
        out.push_str(&format!(
            "\n用户消息 User message (id={id}):\n{}\n",
            message["text"].as_str().unwrap_or("")
        ));
    }
    if let Some(instruction) = data["instruction"].as_str() {
        out.push_str(&format!("\ninstruction: {instruction}\n"));
    }
    if data["status"] == "idle" {
        out.push_str("\n用户还没发新消息，打开下面的「等待」网址继续等。No new message yet — open the Wait URL again.\n");
    }
    let links = next_links(base, reply_to.as_deref());
    let mut body = serde_json::to_string_pretty(&data).unwrap_or_default();
    if body.len() > PAGE_JSON_LIMIT {
        let mut cut = PAGE_JSON_LIMIT;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
        body.push_str("\n…(truncated; use /call for full tool output)");
    }
    (
        format!("{out}\n{links}\n\nresult:\n{body}\n\n{links}\n"),
        reply_to,
    )
}
pub(super) async fn get_action(
    State(state): State<ListenerState>,
    Path((folder_id, key, action)): Path<(String, String, String)>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    headers: HeaderMap,
) -> Response {
    let base = base_url(&headers, &uri, Some(&action));
    let url_mode = key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
    if !url_mode {
        return (
            StatusCode::NOT_FOUND,
            text_no_store("Seed unavailable".into()),
        )
            .into_response();
    }
    let mut rpc_headers = HeaderMap::new();
    rpc_headers.insert(
        "mcp-protocol-version",
        axum::http::HeaderValue::from_static("2025-03-26"),
    );
    let n = query.get("n").cloned().unwrap_or_default();
    let call = |name: &str, args: Value| json!({"jsonrpc":"2.0","id":format!("get-{n}"),"method":"tools/call","params":{"name":name,"arguments":args}});
    let outcome: Result<Value, String> = async {
        // Every page works as the first page: redeeming is idempotent.
        execute(state.clone(), &folder_id, &key, &rpc_headers, json!({"jsonrpc":"2.0","id":"get-init","method":"initialize","params":{"protocolVersion":"2025-03-26"}})).await?;
        match action.as_str() {
            "connect" => {
                execute(state.clone(), &folder_id, &key, &rpc_headers, call("seed_connect", json!({}))).await.map(|v| v.unwrap_or(Value::Null))
            }
            "wait" => {
                let timeout = query.get("wait").and_then(|v| v.parse::<u64>().ok()).unwrap_or(GET_WAIT_DEFAULT_MS).min(25_000);
                execute(state.clone(), &folder_id, &key, &rpc_headers, call("chat_wait", json!({"timeout_ms":timeout}))).await.map(|v| v.unwrap_or(Value::Null))
            }
            "reply" => {
                let to = query.get("to").cloned().unwrap_or_default();
                let text = query.get("text").cloned().unwrap_or_default();
                if n.is_empty() || to.is_empty() || text.trim().is_empty() {
                    return Err("reply needs n, to and text".into());
                }
                let message_id = format!("get-{}", &crate::tools::chat::digest_text(&format!("{n}\0{to}"))[..24]);
                let final_reply = query.get("final").is_none_or(|v| v != "0");
                let mut args = json!({"reply_to":to,"message_id":message_id,"text":text,"final":final_reply});
                if query.get("ask").is_some_and(|v| v == "1") {
                    args["awaiting_user"] = json!(true);
                }
                execute(state.clone(), &folder_id, &key, &rpc_headers, call("chat_reply", args)).await.map(|v| v.unwrap_or(Value::Null))
            }
            "call" => {
                let tool = query.get("tool").cloned().unwrap_or_default();
                if tool.is_empty() {
                    return Err("call needs tool".into());
                }
                let args: Value = match query.get("args") {
                    Some(raw) if !raw.trim().is_empty() => serde_json::from_str(raw).map_err(|e| format!("args must be JSON: {e}"))?,
                    _ => json!({}),
                };
                execute(state.clone(), &folder_id, &key, &rpc_headers, call(&tool, args)).await.map(|v| v.unwrap_or(Value::Null))
            }
            "tools" => {
                let listed = execute(state.clone(), &folder_id, &key, &rpc_headers, json!({"jsonrpc":"2.0","id":"get-tools","method":"tools/list"})).await?.unwrap_or(Value::Null);
                let names: Vec<_> = listed["tools"].as_array().into_iter().flatten().map(|t| format!("- {}: {}", t["name"].as_str().unwrap_or(""), t["description"].as_str().unwrap_or("").chars().take(160).collect::<String>())).collect();
                Ok(json!({"ok":true,"status":"tools","tools":names.join("\n")}))
            }
            _ => Err("Unknown action. Use connect, wait, reply, call or tools.".into()),
        }
    }
    .await;
    match outcome {
        Ok(value) => {
            let (text, reply_to) = if action == "tools" {
                (
                    format!(
                        "{}\n\ntools:\n{}\n",
                        next_links(&base, None),
                        value["tools"].as_str().unwrap_or("")
                    ),
                    None,
                )
            } else {
                summarize(&action, &value, &base)
            };
            page(&headers, text, &base, reply_to.as_deref())
        }
        Err(message) => page(
            &headers,
            format!(
                "error: {message}\n\n{}",
                next_links(&base, query.get("to").map(String::as_str))
            ),
            &base,
            None,
        ),
    }
}

#[cfg(test)]
mod get_page_tests {
    use super::*;

    #[test]
    fn links_come_first_and_reply_template_carries_message_id() {
        let base = "https://example.test/mcp/seeds/a/key";
        let result = json!({"structuredContent":{"status":"message","message":{"id":"m 1","text":"hi"},"blob":"x".repeat(20_000)}});
        let (text, reply_to) = summarize("wait", &result, base);
        assert_eq!(reply_to.as_deref(), Some("m 1"));
        assert!(text.find("/wait?n=").unwrap() < text.find("result:").unwrap());
        assert!(text.contains("&to=m%201&final=1&text="), "{text}");
        assert!(text.len() < 12_000, "{}", text.len());
    }

    #[test]
    fn html_page_escapes_and_links_only_complete_urls() {
        let html = html_page(
            "a <b> https://example.test/x?n=1 https://example.test/r?to=<message.id>",
            "https://example.test",
            Some("m\"1"),
        );
        assert!(html.contains("&lt;b&gt;"));
        assert!(html.contains("<a href=\"https://example.test/x?n=1\">"));
        assert!(!html.contains("<a href=\"https://example.test/r?to="));
        assert!(html.contains("value=\"m&quot;1\""));
        assert!(!html.contains("<script"));
    }
}
