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
    let mut response = match result {
        Ok(value) => json_no_store(value),
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
pub(super) async fn provision(
    State(state): State<ListenerState>,
    Path(folder_id): Path<String>,
    headers: HeaderMap,
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
        let token = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        let mut result = seeds::provision(&root, token, &body).map_err(|e| e.message())?;
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
    if method != "initialize" && protocol != Some("2025-03-26") {
        return Err("MCP-Protocol-Version must match negotiated 2025-03-26".into());
    }
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
                json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{},"resources":{}},"serverInfo":{"name":"coding-tools-seed","version":"1"},"instructions":seeds::skill()["text"]}),
            ));
        }
        let access = seeds::initialize(&root, seed_id, token).map_err(|e| e.message())?;
        return Ok(Some(
            json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{},"resources":{}},"serverInfo":{"name":"coding-tools-seed","version":"1"},"instructions":seeds::skill()["text"],"_meta":{"seed_access_token":access}}),
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
                    .filter(|t| t["name"].as_str().is_some_and(|name| TOOLS.contains(&name)))
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
            if args.get("workspace_folder_id").is_some() && args["workspace_folder_id"] != folder_id
            {
                return Err("Seed workspace mismatch".into());
            }
            if name == "seed_connect" {
                let mut status = seeds::poll(&root, seed_id, token).map_err(|e| e.message())?;
                status["registered"] = json!(status["status"] != "retired");
                status["seed_id"] = json!(seed_id);
                status["workspace_folder_id"] = json!(folder_id);
                status["workspace"] = folder.clone();
                status["skill"] = seeds::skill();
                status["instruction"] =
                    json!("Read skill.text, then immediately chat_wait(timeout_ms:25000).");
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
                let status = seeds::poll(&root, seed_id, token).map_err(|e| e.message())?;
                if status["status"] == "retired" {
                    return Ok(Some(wrapped(
                        json!({"status":"retired","registered":false,"instruction":"Seed retired. Stop waiting and do not switch identities."}),
                    )));
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
                    let result = seeds::poll(&root, seed_id, token).map_err(|e| e.message())?;
                    if result["status"] != "idle" || tokio::time::Instant::now() >= deadline {
                        if chat_wait && result["status"] == "assigned" {
                            let binding = seeds::begin(
                                &root,
                                seed_id,
                                token,
                                "chat_open",
                                &args,
                                &body["id"].to_string(),
                            )
                            .map_err(|e| e.message())?;
                            let open_args = json!({"chat_id":binding["chat_id"],"attachment_id":binding["attachment_id"],"workspace_folder_id":folder_id});
                            let response = handle_request_async(state.mcp.clone(),json!({"jsonrpc":"2.0","id":body["id"],"method":"tools/call","params":{"name":"chat_open","arguments":open_args,"_meta":{"openai/session":format!("seed:{seed_id}")}}})).await;
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
                            actual["workspace_folder_id"] = json!(folder_id);
                            break wrapped(actual);
                        }
                        let mut result = result;
                        if chat_wait {
                            result["registered"] = json!(result["status"] != "retired");
                            result["instruction"] = json!(if result["status"] == "retired" {
                                "Seed retired. Stop waiting and do not switch identities."
                            } else {
                                "Immediately call chat_wait(timeout_ms:25000) again. Idle is normal."
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
                if !TOOLS.contains(&name) {
                    return Err("Tool not available to seeds".into());
                }
                if args.get("workspace_folder_id").is_some()
                    && args["workspace_folder_id"] != folder_id
                {
                    return Err("Seed workspace mismatch".into());
                }
                let request_id = body["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| body["id"].to_string());
                let binding = seeds::begin(&root, seed_id, token, name, &args, &request_id)
                    .map_err(|e| e.message())?;
                if auto_open {
                    args.as_object_mut().unwrap().remove("timeout_ms");
                }
                args["workspace_folder_id"] = json!(folder_id);
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
                let response=handle_request_async(state.mcp,json!({"jsonrpc":"2.0","id":body["id"],"method":"tools/call","params":{"name":name,"arguments":args,"_meta":{"openai/session":format!("seed:{seed_id}")}}})).await;
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
                    actual["workspace_folder_id"] = json!(folder_id);
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
        let paired = format!("{base}/seed-library/legacy");
        let state = client
            .post(&paired)
            .bearer_auth(&pairing)
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
                    .bearer_auth(&pairing)
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
                .post(&paired)
                .bearer_auth("wrong")
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
                .bearer_auth(&pairing)
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
                .bearer_auth(&pairing)
                .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::UNAUTHORIZED
        );
        assert!(!client
            .post(format!("{paired}/extra"))
            .bearer_auth(&pairing)
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
        let landing = client.get(&url_endpoint).send().await.unwrap();
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
        let denied=rpc(token.clone(),"tools/call",json!({"name":"read_file","arguments":{"path":"sample.txt","workspace_folder_id":"other"}}),6).await.unwrap().json::<Value>().await.unwrap();
        assert!(denied.get("error").is_some());
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
