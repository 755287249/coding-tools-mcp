//! A single authenticated transport routes to independent project contexts.
//! Every project operation requires the server-issued chat attachment, never a
//! client-selected session header alone. Seed endpoints retain their own capability.
use super::*;
use crate::workspace::WorkspaceProfile;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct ProjectCache {
    contexts: HashMap<String, (String, SharedState)>,
    blocked: std::collections::HashSet<String>,
    epoch: u64,
}
static CONTEXTS: OnceLock<Mutex<ProjectCache>> = OnceLock::new();
pub(super) fn current_epoch() -> u64 {
    CONTEXTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .epoch
}
pub(crate) fn unblock_project(id: &str) {
    CONTEXTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .blocked
        .remove(id);
}
pub(crate) async fn stop_project(id: Option<&str>) {
    let contexts = {
        let mut cache = CONTEXTS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(id) = id {
            cache.blocked.insert(id.into());
        } else {
            cache.epoch = cache.epoch.wrapping_add(1);
        }
        let ids = cache
            .contexts
            .keys()
            .filter(|key| id.is_none_or(|id| id == key.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let contexts = ids
            .into_iter()
            .filter_map(|id| cache.contexts.remove(&id).map(|(_, context)| context))
            .collect::<Vec<_>>();
        // Admission and cache invalidation are atomic with project context creation.
        for context in &contexts {
            context.sessions.close_admission();
            crate::tools::hub::remove_live_hub(&context.profile_id);
        }
        contexts
    };
    for context in contexts {
        context.sessions.shutdown().await;
    }
}
const SCOPE_FIELDS: &[&str] = &["workspace_folder_id", "chat_id", "attachment_id"];
const INSTRUCTIONS: &str="This is the global MCP connection. Projects have fixed directories. Call chat_open with the supplied chat_id and workspace_folder_id. For EVERY subsequent tool call include the returned attachment_id, chat_id and workspace_folder_id; for prompts/resources place these in params._meta. A client session header is not authorization. No project switch is allowed for an attachment. Commands require an OS sandbox; run Git and formatters via exec_command. Follow the full local-chat Skill.";
fn projects() -> Result<Vec<WorkspaceProfile>, String> {
    crate::data::DataStore::read_file(|data| Ok(data.profiles.clone())).map_err(|e| e.to_string())
}
pub(crate) fn folders() -> Result<Value, String> {
    let profiles = projects()?;
    Ok(
        json!({"ok":true,"folders":profiles.iter().flat_map(|p|p.folders.iter()).collect::<Vec<_>>()}),
    )
}
fn fail(id: Value, message: String) -> Response {
    json_no_store(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32003,"message":message}}))
}
pub(super) fn project_state(
    state: &ListenerState,
    folder_id: &str,
) -> Result<ListenerState, String> {
    if state.workspace_id != crate::data::GLOBAL_MCP_ID {
        return Ok(state.clone());
    }
    project_state_from_profiles(state, folder_id, &projects()?)
}
fn project_state_from_profiles(
    state: &ListenerState,
    folder_id: &str,
    profiles: &[WorkspaceProfile],
) -> Result<ListenerState, String> {
    let context = project_context_for_epoch(folder_id, profiles, Some(state.gateway_epoch))?;
    let mut routed = state.clone();
    routed.workspace_id = context.profile_id.clone();
    routed.mcp = context;
    Ok(routed)
}
pub(crate) fn project_context(folder_id: &str) -> Result<SharedState, String> {
    project_context_from_profiles(folder_id, &projects()?)
}
fn project_context_from_profiles(
    folder_id: &str,
    profiles: &[WorkspaceProfile],
) -> Result<SharedState, String> {
    project_context_for_epoch(folder_id, profiles, None)
}
fn project_context_for_epoch(
    folder_id: &str,
    profiles: &[WorkspaceProfile],
    expected_epoch: Option<u64>,
) -> Result<SharedState, String> {
    let mut lifecycle = CONTEXTS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Project lifecycle lock unavailable")?;
    if expected_epoch.is_some_and(|epoch| epoch != lifecycle.epoch) {
        return Err("Global connection is stopped".into());
    }
    let mut matches = profiles
        .iter()
        .filter(|p| p.folders.iter().any(|f| f.id == folder_id));
    let profile = matches.next().ok_or("Project directory unavailable")?;
    if matches.next().is_some() {
        return Err("Project directory identity is ambiguous".into());
    }
    if lifecycle.blocked.contains(&profile.id) {
        return Err("Project is being removed".into());
    }
    if profile.folders.len() != 1 {
        return Err("Migrate this project to one fixed directory before connecting".into());
    }
    // Nested or shared project directories are allowed: a parent project
    // includes its children, a child never sees its parent. Each project is
    // still rooted at its own directory.
    let root = std::path::Path::new(&profile.folders[0].path)
        .canonicalize()
        .map_err(|_| "Project directory unavailable")?;

    let fingerprint = format!(
        "{}:{}",
        serde_json::to_string(profile).map_err(|e| e.to_string())?,
        root.display()
    );
    let cache = &mut lifecycle.contexts;
    let context = if let Some((_, context)) = cache
        .get(&profile.id)
        .filter(|(version, _)| version == &fingerprint)
    {
        context.clone()
    } else {
        if let Some((_, previous)) = cache.remove(&profile.id) {
            previous.sessions.close_admission();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    previous.sessions.shutdown().await;
                });
            }
        }
        let runtime = &profile.runtime;
        let context = crate::mcp::server::new_state(
            profile.folders.clone(),
            profile.active_folder_id.clone(),
            profile.id.clone(),
            profile.auth.clone(),
            crate::tools::policy::PolicySettings::from_runtime(runtime),
            runtime.tool_profile.clone(),
            runtime.permission_mode.clone(),
            runtime.sandbox.clone(),
            crate::tools::ExecutionLimits::new_with_global(
                runtime.blocking_admission_limit as usize,
                runtime.process_admission_limit as usize,
                runtime.global_blocking_admission_limit as usize,
                runtime.global_process_admission_limit as usize,
                runtime.active_session_limit as usize,
            ),
        )?;
        context.shared_runtime_config().require_project_isolation();
        crate::workspace_features::require_project_isolation(&profile.id);
        cache.insert(profile.id.clone(), (fingerprint, context.clone()));
        context
    };
    Ok(context)
}
fn scoped_catalog(state: &SharedState) -> Value {
    let runtime = state.runtime_config();
    let tools = scoped_tools(if state.profile_id == crate::data::GLOBAL_MCP_ID {
        "advanced"
    } else {
        &runtime.tool_profile
    });
    let material = serde_json::to_vec(&tools).unwrap_or_default();
    use sha2::{Digest, Sha256};
    json!({"tools":tools,"toolsetRevision":format!("{:x}",Sha256::digest(material))})
}
pub(crate) fn scoped_tools(profile: &str) -> Vec<Value> {
    let mut tools = crate::tools::list_tools_for_profile(profile);
    tools.retain(|tool| {
        tool["name"]
            .as_str()
            .is_some_and(crate::tools::project_scope::allowed)
    });
    for tool in &mut tools {
        let name = tool["name"].as_str().unwrap_or("").to_string();
        for field in SCOPE_FIELDS {
            if tool["inputSchema"]["properties"].get(*field).is_none() {
                tool["inputSchema"]["properties"][*field] = json!({"type":"string","minLength":1,"description":"Fixed project capability returned by chat_open. Keep it unchanged."});
            }
        }
        if !matches!(name.as_str(), "chat_open" | "list_workspace_folders") {
            if tool["inputSchema"]["required"].is_null() {
                tool["inputSchema"]["required"] = json!([]);
            }
            let required = tool["inputSchema"]["required"].as_array_mut().unwrap();
            for field in SCOPE_FIELDS {
                if !required.iter().any(|v| v == field) {
                    required.push(json!(field));
                }
            }
        }
    }
    tools
}
fn capability(body: &Value) -> Value {
    if body["method"] == "tools/call" {
        body["params"]["arguments"].clone()
    } else {
        body["params"]["_meta"].clone()
    }
}
pub(super) async fn post(state: ListenerState, headers: HeaderMap, body: Value) -> Response {
    let profiles = match projects() {
        Ok(v) => v,
        Err(e) => return fail(body["id"].clone(), e),
    };
    post_with_projects(state, headers, body, &profiles).await
}
async fn post_with_projects(
    state: ListenerState,
    headers: HeaderMap,
    mut body: Value,
    profiles: &[WorkspaceProfile],
) -> Response {
    if state.gateway_epoch != current_epoch() {
        return fail(body["id"].clone(), "Global connection is stopped".into());
    }
    let id = body["id"].clone();
    let method = body["method"].as_str().unwrap_or("").to_string();
    let name = body["params"]["name"].as_str().unwrap_or("").to_string();
    let scope = capability(&body);
    let attached = scope["attachment_id"]
        .as_str()
        .is_some_and(|s| !s.is_empty());
    if !attached {
        let result=match method.as_str() {
            "initialize"|"server/discover"=>{
                let mut response=handle_request(&state.mcp,&body);
                response["result"]["instructions"]=json!(INSTRUCTIONS);
                return json_no_store(response);
            }
            "notifications/initialized"|"notifications/cancelled"=>return StatusCode::ACCEPTED.into_response(),
            "ping"=>json!({}),
            "tools/list"=>scoped_catalog(&state.mcp),
            "resources/list"=>json!({"resources":[crate::tools::chat::local_chat_skill_resource()]}),
            "resources/read" if body["params"]["uri"]==crate::tools::chat::LOCAL_CHAT_SKILL_URI=>json!({"contents":[crate::tools::chat::local_chat_skill()]}),
            "tools/call" if name=="list_workspace_folders"=>crate::tools::wrap_mcp_tool_result(&name,&scope,json!({"ok":true,"folders":profiles.iter().flat_map(|p|p.folders.iter()).collect::<Vec<_>>()})),
            "tools/call" if name=="chat_open"=>Value::Null,
            _=>return fail(id,"Open a chat first and include its project/chat/attachment capability in every project operation".into()),
        };
        if !result.is_null() {
            return json_no_store(json!({"jsonrpc":"2.0","id":id,"result":result}));
        }
    }
    let folder = scope["workspace_folder_id"].as_str().unwrap_or("");
    let routed = match project_state_from_profiles(&state, folder, profiles) {
        Ok(v) => v,
        Err(e) => return fail(id, e),
    };
    if attached && method == "tools/call" && name == "chat_wait" {
        // An AI moved to the seed library gets its standby hand-off instead of
        // an "attachment expired" error, so it keeps waiting.
        if let Some(handoff) = crate::tools::chat::standby_handoff(&scope) {
            return json_no_store(json!({"jsonrpc":"2.0","id":id,
                "result":crate::tools::wrap_mcp_tool_result(&name,&scope,handoff)}));
        }
    }
    if attached {
        if let Err(error) = crate::tools::chat::authorize_project_attachment(
            routed.mcp.workspace.root(),
            &scope,
            name == "chat_wait",
        ) {
            return fail(id, error.message());
        }
    } else if scope["chat_id"].as_str().is_none_or(str::is_empty) {
        return fail(id, "chat_id is required".into());
    }
    if method == "tools/list" {
        return json_no_store(
            json!({"jsonrpc":"2.0","id":id,"result":scoped_catalog(&routed.mcp)}),
        );
    }
    if method == "tools/call" {
        if !crate::tools::project_scope::allowed(&name) {
            return fail(
                id,
                "Use exec_command inside the project sandbox for this operation".into(),
            );
        }
        if name == "switch_workspace_folder"
            && body["params"]["arguments"]["folder_id"]
                .as_str()
                .is_some_and(|f| f != folder)
        {
            return fail(id, "Conversation project cannot change".into());
        }
        // The public gateway adds capability fields. Remove only fields absent from
        // the underlying contract before calling the existing validated dispatcher.
        if let Some(tool) =
            crate::tools::list_tools_for_profile(&routed.mcp.runtime_config().tool_profile)
                .iter()
                .find(|t| t["name"] == name)
        {
            if let Some(args) = body["params"]["arguments"].as_object_mut() {
                for field in SCOPE_FIELDS {
                    if tool["inputSchema"]["properties"].get(*field).is_none() {
                        args.remove(*field);
                    }
                }
            }
        }
    }
    if !body["params"].is_object() {
        body["params"] = json!({});
    }
    if !body["params"]["_meta"].is_object() {
        body["params"]["_meta"] = json!({});
    }
    body["params"]["_meta"]["openai/session"] = json!(format!(
        "project-chat:{}",
        scope["attachment_id"].as_str().unwrap_or("opening")
    ));
    Box::pin(super::mcp_post(State(routed), headers, Json(body))).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_project_directories_can_both_connect() {
        let parent = tempfile::tempdir().unwrap();
        let child = parent.path().join("child");
        std::fs::create_dir_all(&child).unwrap();
        let profiles = vec![
            WorkspaceProfile::new(parent.path().to_string_lossy().into(), None),
            WorkspaceProfile::new(child.to_string_lossy().into(), None),
        ];
        for profile in &profiles {
            assert!(
                project_context_from_profiles(&profile.active_folder_id, &profiles).is_ok(),
                "{}",
                profile.path
            );
        }
    }
    fn state(root: &std::path::Path) -> ListenerState {
        let mut profile = WorkspaceProfile::new(root.to_string_lossy().into(), None);
        profile.id = crate::data::GLOBAL_MCP_ID.into();
        let auth = AuthConfig {
            auth_type: "noauth".into(),
            ..Default::default()
        };
        let mcp = crate::mcp::server::new_state(
            profile.folders.clone(),
            profile.active_folder_id.clone(),
            profile.id.clone(),
            auth.clone(),
            crate::tools::policy::PolicySettings::default(),
            "advanced".into(),
            "trusted".into(),
            Default::default(),
            Default::default(),
        )
        .unwrap();
        ListenerState {
            gateway_epoch: current_epoch(),
            mcp,
            auth,
            workspace_id: profile.id,
            bind_address: "127.0.0.1".into(),
            bind_port: 0,
            configured_public_url: String::new(),
            bearer_token: None,
            oauth: None,
            oauth_client_secret: None,
            transport_mode: "legacy-json".into(),
            redact_telemetry: true,
        }
    }
    async fn request(
        state: ListenerState,
        profiles: &[WorkspaceProfile],
        name: &str,
        args: Value,
    ) -> Value {
        let response=post_with_projects(state,HeaderMap::new(),json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args,"_meta":{"openai/session":"spoofed"}}}),profiles).await;
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn attachment_authorizes_only_its_fixed_project_and_revocation_is_immediate() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("allowed.txt"), "project A").unwrap();
        std::fs::write(b.path().join("private.txt"), "project B").unwrap();
        let profiles = vec![
            WorkspaceProfile::new(a.path().to_string_lossy().into(), None),
            WorkspaceProfile::new(b.path().to_string_lossy().into(), None),
        ];
        let state = state(a.path());
        let ca = crate::tools::chat::ui(a.path(), &json!({"action":"create"})).unwrap()["session"]
            ["id"]
            .clone();
        let cb = crate::tools::chat::ui(b.path(), &json!({"action":"create"})).unwrap()["session"]
            ["id"]
            .clone();
        let denied = request(
            state.clone(),
            &profiles,
            "read_file",
            json!({"workspace_folder_id":profiles[0].active_folder_id,"path":"allowed.txt"}),
        )
        .await;
        assert_eq!(denied["error"]["code"], -32003);
        let open = request(
            state.clone(),
            &profiles,
            "chat_open",
            json!({"workspace_folder_id":profiles[0].active_folder_id,"chat_id":ca}),
        )
        .await;
        let attachment = open["result"]["structuredContent"]["attachment_id"].clone();
        assert!(attachment.is_string(), "{open}");
        let args = json!({"workspace_folder_id":profiles[0].active_folder_id,"chat_id":ca,"attachment_id":attachment,"path":"allowed.txt"});
        let allowed = request(state.clone(), &profiles, "read_file", args.clone()).await;
        assert_eq!(
            allowed["result"]["structuredContent"]["ok"], true,
            "{allowed}"
        );
        let mut foreign = args.clone();
        foreign["workspace_folder_id"] = json!(profiles[1].active_folder_id);
        foreign["chat_id"] = cb;
        assert_eq!(
            request(state.clone(), &profiles, "read_file", foreign).await["error"]["code"],
            -32003
        );
        let mut outside = args.clone();
        outside["path"] = json!(b.path().join("private.txt").to_string_lossy());
        let blocked = request(state.clone(), &profiles, "read_file", outside).await;
        assert_ne!(
            blocked["result"]["structuredContent"]["ok"], true,
            "{blocked}"
        );
        crate::tools::chat::ui(a.path(), &json!({"action":"detach","chat_id":ca})).unwrap();
        assert_eq!(
            request(state, &profiles, "read_file", args).await["error"]["code"],
            -32003
        );
    }
    #[tokio::test]
    async fn empty_connection_discovers_without_granting_control_directory_access() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        let list = request(state.clone(), &[], "list_workspace_folders", json!({})).await;
        assert_eq!(list["result"]["structuredContent"]["folders"], json!([]));
        let denied = request(
            state.clone(),
            &[],
            "read_file",
            json!({"path":"profiles.json"}),
        )
        .await;
        assert_eq!(denied["error"]["code"], -32003);
        let response=post_with_projects(state,HeaderMap::new(),json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}),&[]).await;
        let payload: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(payload["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("attachment_id"));
    }
    #[tokio::test]
    async fn closed_conversation_delivers_terminal_wait_and_denies_further_project_work() {
        let root = tempfile::tempdir().unwrap();
        let profiles = vec![WorkspaceProfile::new(
            root.path().to_string_lossy().into(),
            None,
        )];
        let state = state(root.path());
        let chat = crate::tools::chat::ui(root.path(), &json!({"action":"create"})).unwrap()
            ["session"]["id"]
            .clone();
        let opened = request(
            state.clone(),
            &profiles,
            "chat_open",
            json!({"workspace_folder_id":profiles[0].active_folder_id,"chat_id":chat}),
        )
        .await;
        let args = json!({"workspace_folder_id":profiles[0].active_folder_id,"chat_id":chat,"attachment_id":opened["result"]["structuredContent"]["attachment_id"]});
        let closed = request(state.clone(), &profiles, "chat_close", args.clone()).await;
        assert_eq!(
            closed["result"]["structuredContent"]["ok"], true,
            "{closed}"
        );
        let mut wait = args.clone();
        wait["timeout_ms"] = json!(1);
        let terminal = request(state.clone(), &profiles, "chat_wait", wait).await;
        assert_eq!(
            terminal["result"]["structuredContent"]["status"], "closed",
            "{terminal}"
        );
        let mut read = args;
        read["path"] = json!("AGENTS.md");
        assert_eq!(
            request(state, &profiles, "read_file", read).await["error"]["code"],
            -32003
        );
    }

    #[test]
    fn ambiguous_project_directory_identity_is_denied() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let first = WorkspaceProfile::new(a.path().to_string_lossy().into(), None);
        let mut second = WorkspaceProfile::new(b.path().to_string_lossy().into(), None);
        second.folders[0].id = first.active_folder_id.clone();
        second.active_folder_id = first.active_folder_id.clone();
        assert!(
            project_context_from_profiles(&first.active_folder_id, &[first.clone(), second])
                .is_err()
        );
    }

    #[tokio::test]
    async fn stale_listener_cannot_recreate_project_context() {
        let root = tempfile::tempdir().unwrap();
        let profile = WorkspaceProfile::new(root.path().to_string_lossy().into(), None);
        let mut stale = state(root.path());
        stale.gateway_epoch = current_epoch().wrapping_sub(1);
        assert!(
            project_state_from_profiles(&stale, &profile.active_folder_id, &[profile.clone()])
                .is_err()
        );
        let response = request(stale, &[profile], "chat_open", json!({})).await;
        assert_eq!(response["error"]["code"], -32003);
    }
    #[test]
    fn global_bootstrap_catalog_keeps_chat_open_with_read_only_donor() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        let mut runtime = crate::workspace::RuntimeConfig::default();
        runtime.tool_profile = "read-only".into();
        state
            .mcp
            .shared_runtime_config()
            .update_from_runtime(&runtime);
        assert!(scoped_catalog(&state.mcp)["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "chat_open"));
    }

    #[tokio::test]
    async fn removing_project_revokes_cached_context_and_process_admission() {
        let root = tempfile::tempdir().unwrap();
        let profile = WorkspaceProfile::new(root.path().to_string_lossy().into(), None);
        let context =
            project_context_from_profiles(&profile.active_folder_id, &[profile.clone()]).unwrap();
        stop_project(Some(&profile.id)).await;
        assert!(context.sessions.is_closed());
        assert!(context.sessions.acquire_active_slot().await.is_err());
        assert!(
            project_context_from_profiles(&profile.active_folder_id, &[profile.clone()]).is_err()
        );
        assert_eq!(
            crate::tools::call_tool(&context, "list_files", &json!({}))["error"]["code"],
            "PROJECT_STOPPED"
        );
        unblock_project(&profile.id);
    }

    #[test]
    fn project_context_cannot_disable_isolation_or_expose_native_process_tools() {
        let dir = tempfile::tempdir().unwrap();
        let mut profile = WorkspaceProfile::new(dir.path().to_string_lossy().into(), None);
        profile.runtime.sandbox.backend = "missing-test-backend".into();
        let context = project_state_from_profiles(
            &state(dir.path()),
            &profile.active_folder_id,
            &[profile.clone()],
        )
        .unwrap()
        .mcp;
        context
            .shared_runtime_config()
            .update_from_runtime(&profile.runtime);
        let runtime = context.runtime_config();
        assert!(
            runtime.project_isolation
                && runtime.sandbox.enabled
                && runtime.policy.security_policy.enforce_workspace_boundary
        );
        assert!(runtime.sandbox.external_paths.is_empty());
        let catalog = scoped_catalog(&context);
        assert!(!catalog["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "git_status" || t["name"] == "format_files"));
        assert_eq!(
            crate::tools::call_tool(&context, "git_status", &json!({}))["error"]["code"],
            "PROJECT_ISOLATION_REQUIRED"
        );
        let result = crate::tools::call_tool(
            &context,
            "exec_command",
            &json!({"program":"python3","args":["-c","raise SystemExit('must not run')"]}),
        );
        assert_eq!(result["ok"], false, "{result}");
    }
}
