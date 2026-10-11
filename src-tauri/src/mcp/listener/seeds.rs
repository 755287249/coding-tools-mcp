//! Dedicated capability endpoint. Ordinary OAuth and its tool exposure are unchanged.
use super::*;
use crate::tools::chat::seeds;
use axum::extract::Path;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
static WAITERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
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
struct WaitGuard(String);
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
    if method == "initialize" {
        let access = seeds::initialize(&root, seed_id, token).map_err(|e| e.message())?;
        return Ok(Some(
            json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{},"resources":{}},"serverInfo":{"name":"coding-tools-seed","version":"1"},"instructions":"Keep _meta.seed_access_token in client memory and use it as Bearer on this endpoint. List tools and list_workspace_folders, then seed_wait. Assigned work uses only this endpoint and project. Never create another attachment or use another seed identity.","_meta":{"seed_access_token":access}}),
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
                json!({"name":"list_workspace_folders","description":"Show the single project authorized for this seed.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}),
                wait_tool(),
            ];
            tools.extend(
                response["result"]["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|t| t["name"].as_str().is_some_and(|name| TOOLS.contains(&name)))
                    .cloned(),
            );
            json!({"tools":tools})
        }
        "resources/read" if params["uri"] == crate::tools::chat::LOCAL_CHAT_SKILL_URI => {
            json!({"contents":[crate::tools::chat::local_chat_skill()]})
        }
        "resources/list" => json!({"resources":[crate::tools::chat::local_chat_skill_resource()]}),
        "ping" => json!({}),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let mut args = params.get("arguments").cloned().unwrap_or(json!({}));
            if !args.is_object() {
                return Err("Arguments must be an object".into());
            }
            if name == "list_workspace_folders" {
                wrapped(json!({"ok":true,"folders":[folder],"selected_folder_id":folder_id}))
            } else if name == "seed_wait" {
                let timeout = args["timeout_ms"]
                    .as_u64()
                    .filter(|n| *n <= 25000)
                    .ok_or("timeout_ms must be 0–25000")?;
                for (owner, session) in seeds::pending_processes(&root).map_err(|e| e.message())? {
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
                    if !waiters.insert(key.clone()) {
                        return Err("A seed_wait is already active".into());
                    }
                }
                let _guard = WaitGuard(key);
                let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
                loop {
                    let result = seeds::poll(&root, seed_id, token).map_err(|e| e.message())?;
                    if result["status"] != "idle" || tokio::time::Instant::now() >= deadline {
                        break wrapped(result);
                    }
                    tokio::time::sleep(Duration::from_millis(1000)).await;
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
                response["result"].clone()
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
