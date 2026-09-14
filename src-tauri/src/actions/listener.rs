use std::sync::Arc;

use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    middleware,
    response::{Html, IntoResponse, Json, Response},
    routing::{get, post},
    Extension, Router,
};
use serde_json::{json, Value};
use tokio::sync::{oneshot, RwLock};
use tower_http::cors::CorsLayer;

use crate::auth::{
    authorization_server_metadata, authorize_get, authorize_post, external_base_url,
    require_configured_secret, token_exchange, AuthorizeForm, AuthorizeParams, OAuthRuntime,
    TokenForm,
};
use crate::secret::SecretStore;
use crate::tools::hub::{HubConfig, HubRouter};
use crate::tools::{
    self, policy::PolicySettings, wrap_tool_result, ExecutionLimits, SharedRuntimeToolConfig,
};
use crate::tunnel::append_profile_log_buffered;
use crate::workspace::{
    parse_bind_address, socket_addr_for_bind, url_host_for_bind, SandboxConfig, WorkspaceFolder,
};

use super::auth::{require_actions_auth, AuthConfig};
use super::openapi;

pub type ShutdownSender = oneshot::Sender<()>;

#[derive(Clone)]
struct AppState {
    router: Arc<HubRouter>,
    openapi: Arc<RwLock<Value>>,
    auth: Arc<AuthConfig>,
    bind_port: u16,
    configured_public_url: String,
    oauth: Option<Arc<OAuthRuntime>>,
    oauth_client_secret: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_listener(
    workspace_id: &str,
    actions_port: u16,
    bind_address: String,
    folders: Vec<WorkspaceFolder>,
    public_base_url: String,
    auth_type: String,
    api_key: Option<String>,
    oauth_client_id: String,
    oauth_client_secret: Option<String>,
    oauth_password: Option<String>,
    oauth_token_secret: Option<String>,
    use_shared_secrets: bool,
    policy: PolicySettings,
    sandbox: SandboxConfig,
    execution_limits: ExecutionLimits,
) -> Result<(ShutdownSender, crate::task_runtime::JoinHandle<()>), String> {
    let api_key = if auth_type == "api_key" {
        Some(require_configured_secret(api_key, "Actions API key")?)
    } else {
        None
    };
    let oauth_client_secret = if auth_type == "oauth" {
        Some(require_configured_secret(
            oauth_client_secret,
            "Actions OAuth client secret",
        )?)
    } else {
        None
    };

    let configured_public_url = public_base_url.trim().to_string();
    let bind_address = parse_bind_address(&bind_address)?.to_string();
    let oauth = if auth_type == "oauth" {
        let oauth_base = if configured_public_url.is_empty() {
            format!("http://{}:{actions_port}", url_host_for_bind(&bind_address))
        } else {
            external_base_url(&HeaderMap::new(), actions_port, &configured_public_url)
        };
        let password_workspace_id = workspace_id.to_string();
        let password_persister = Arc::new(move |value: &str| {
            if use_shared_secrets {
                SecretStore::set_shared("actions_oauth_password", value)
            } else {
                SecretStore::set(&password_workspace_id, "actions_oauth_password", value)
            }
            .map_err(|error| error.to_string())
        });
        Some(Arc::new(OAuthRuntime::try_new_with_password_persister(
            oauth_base,
            oauth_client_id,
            oauth_client_secret.clone(),
            oauth_password,
            oauth_token_secret,
            Some(password_persister),
        )?))
    } else {
        None
    };

    // 在返回 Running 之前完成 bind，避免后台任务里的端口冲突被伪装成启动成功。
    let listener = bind_listener(&bind_address, actions_port)?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let profile_id = workspace_id.to_string();
    let handle = crate::task_runtime::spawn(async move {
        let result = serve(
            listener,
            actions_port,
            bind_address,
            &profile_id,
            folders,
            configured_public_url,
            auth_type,
            api_key,
            oauth,
            oauth_client_secret,
            policy,
            sandbox,
            execution_limits,
            shutdown_rx,
        )
        .await;
        if let Err(err) = &result {
            append_profile_log_buffered(
                &profile_id,
                "actions-stderr.log",
                &format!("[actions] listener stopped: {err}"),
            );
            eprintln!("actions listener stopped: {err}");
        } else {
            append_profile_log_buffered(
                &profile_id,
                "actions-stderr.log",
                "[actions] listener stopped",
            );
        }
    });
    Ok((shutdown_tx, handle))
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    listener: tokio::net::TcpListener,
    actions_port: u16,
    bind_address: String,
    profile_id: &str,
    folders: Vec<WorkspaceFolder>,
    configured_public_url: String,
    auth_type: String,
    api_key: Option<String>,
    oauth: Option<Arc<OAuthRuntime>>,
    oauth_client_secret: Option<String>,
    policy: PolicySettings,
    sandbox: SandboxConfig,
    execution_limits: ExecutionLimits,
    shutdown: oneshot::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let router = HubRouter::new(
        profile_id.to_string(),
        folders,
        HubConfig {
            auth: crate::workspace::AuthConfig {
                auth_type: auth_type.clone(),
                ..crate::workspace::AuthConfig::default()
            },
            runtime_config: SharedRuntimeToolConfig::new_with_sandbox(
                policy.clone(),
                "full".into(),
                policy.permission_mode.clone(),
                sandbox,
            ),
            limits: execution_limits,
            execution_resource_namespace: "actions".into(),
        },
    )
    .map_err(std::io::Error::other)?;
    let tools: Vec<Value> = tools::list_tools()
        .into_iter()
        .filter(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| tools::policy::validate_actions_exposure(name).is_ok())
        })
        .collect();
    let public_base_url = if configured_public_url.is_empty() {
        format!("http://{}:{actions_port}", url_host_for_bind(&bind_address))
    } else {
        configured_public_url.clone()
    };
    let openapi_doc = openapi::build_openapi(&tools, &public_base_url, &auth_type);

    let auth = Arc::new(AuthConfig::new(
        auth_type,
        api_key,
        oauth.clone(),
        actions_port,
        configured_public_url.clone(),
    ));

    let state = AppState {
        router,
        openapi: Arc::new(RwLock::new(openapi_doc)),
        auth: auth.clone(),
        bind_port: actions_port,
        configured_public_url,
        oauth,
        oauth_client_secret,
    };

    let protected = Router::new()
        .route("/actions/{tool_name}", post(execute_action))
        .layer(middleware::from_fn(require_actions_auth))
        .layer(Extension(auth));

    let app = Router::new()
        .route("/health", get(health))
        .route("/openapi.json", get(openapi_json))
        .route("/privacy", get(privacy))
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_authorization_server_metadata),
        )
        .route(
            "/oauth/authorize",
            get(oauth_authorize_get).post(oauth_authorize_post),
        )
        .route("/oauth/token", post(oauth_token_post))
        .merge(protected)
        .with_state(state)
        .layer(CorsLayer::permissive());

    let listening_address = listener.local_addr()?;

    append_profile_log_buffered(
        profile_id,
        "actions-stdout.log",
        &format!("[actions] listening on http://{listening_address} (public: {public_base_url})"),
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = shutdown.await;
        })
        .await?;
    Ok(())
}

fn bind_listener(bind_address: &str, port: u16) -> Result<tokio::net::TcpListener, String> {
    let addr = socket_addr_for_bind(bind_address, port)?;
    let listener = std::net::TcpListener::bind(addr)
        .map_err(|err| format!("Actions 監聽位址 {addr} 綁定失敗: {err}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| format!("Actions 本地端口 {port} 设置非阻塞失败: {err}"))?;
    tokio::net::TcpListener::from_std(listener)
        .map_err(|err| format!("Actions 本地监听器初始化失败: {err}"))
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    let tools_loaded = state
        .openapi
        .read()
        .await
        .get("paths")
        .and_then(Value::as_object)
        .map(|paths| paths.len())
        .unwrap_or(0);
    let folders = state.router.action_folder_listing();

    let folder_count = folders
        .get("folders")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    Json(json!({
        "ok": true,
        "service": "coding-tools-actions",
        "workspace": Value::Null,
        "workspace_selected": false,
        "multi_folder": folders.get("multi_folder").cloned().unwrap_or(Value::Bool(false)),
        "folder_count": folder_count,
        "auth_type": state.auth.auth_type,
        "tools_loaded": tools_loaded
    }))
}

async fn openapi_json(State(state): State<AppState>) -> Json<Value> {
    Json(state.openapi.read().await.clone())
}

async fn privacy() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="utf-8">
    <title>Coding Tools Actions Privacy</title>
  </head>
  <body>
    <h1>隐私政策</h1>
    <p>本服务仅供仓库所有者本人使用。</p>
    <p>请求内容只用于执行用户主动发起的代码操作。</p>
    <p>服务不会出售或共享请求数据。</p>
    <p>API 密钥、GitHub 令牌和环境变量不会返回给模型。</p>
  </body>
</html>"#,
    )
}

fn resolve_oauth_base(state: &AppState, headers: &HeaderMap) -> String {
    external_base_url(headers, state.bind_port, &state.configured_public_url)
}

async fn oauth_authorization_server_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !state.auth.oauth_enabled() {
        return oauth_not_configured();
    }
    Json(authorization_server_metadata(
        &resolve_oauth_base(&state, &headers),
        state.oauth_client_secret.as_deref(),
    ))
    .into_response()
}

async fn oauth_authorize_get(
    State(state): State<AppState>,
    Query(params): Query<AuthorizeParams>,
) -> Response {
    let Some(oauth) = state.oauth.as_ref() else {
        return oauth_not_configured();
    };
    authorize_get(oauth, params, None)
}

async fn oauth_authorize_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<AuthorizeForm>,
) -> Response {
    let Some(oauth) = state.oauth.as_ref() else {
        return oauth_not_configured();
    };
    authorize_post(oauth, form, &resolve_oauth_base(&state, &headers))
}

async fn oauth_token_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    let Some(oauth) = state.oauth.as_ref() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unsupported_grant_type" })),
        )
            .into_response();
    };
    token_exchange(oauth, &headers, form, &resolve_oauth_base(&state, &headers))
}

fn oauth_not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "OAuth not configured" })),
    )
        .into_response()
}

async fn execute_action(
    State(state): State<AppState>,
    Path(tool_name): Path<String>,
    body: Option<Json<Value>>,
) -> Response {
    let mut arguments = match body {
        Some(Json(value)) if value.is_object() || value.is_null() => {
            if value.is_null() {
                json!({})
            } else {
                value
            }
        }
        Some(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "Request body must be a JSON object" })),
            )
                .into_response();
        }
        None => json!({}),
    };

    let workspace_folder_id = match take_workspace_folder_id(&mut arguments) {
        Ok(value) => value,
        Err(detail) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "detail": detail }))).into_response();
        }
    };

    if let Err(err) = tools::policy::validate_actions_exposure(&tool_name) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "detail": err.to_string() })),
        )
            .into_response();
    }

    if tool_name == "list_workspace_folders" {
        return action_response(&tool_name, None, None, state.router.action_folder_listing());
    }

    let routed = match state.router.resolve_action_context(
        workspace_folder_id.as_deref(),
        &tool_name,
        &arguments,
    ) {
        Ok(routed) => routed,
        Err(message) => {
            let listing = state.router.action_folder_listing();
            return action_response(&tool_name, None, None, routing_error(message, &listing));
        }
    };

    let structured =
        tools::call_tool_async(routed.context.clone(), tool_name.clone(), arguments.clone()).await;
    state
        .router
        .record_action_result(&routed.folder_id, &structured);

    action_response(
        &tool_name,
        Some(&routed.folder_id),
        Some(&routed.folder.path),
        structured,
    )
}

fn take_workspace_folder_id(arguments: &mut Value) -> Result<Option<String>, String> {
    let Some(object) = arguments.as_object_mut() else {
        return Ok(None);
    };
    let Some(value) = object.remove("workspace_folder_id") else {
        return Ok(None);
    };
    let Some(folder_id) = value.as_str().map(str::trim) else {
        return Err("workspace_folder_id must be a string".into());
    };
    if folder_id.is_empty() {
        return Err("workspace_folder_id must not be empty".into());
    }
    Ok(Some(folder_id.to_string()))
}

fn routing_error(message: String, folder_listing: &Value) -> Value {
    let (code, message) =
        crate::tools::hub::routing_error_parts(&message, "ACTIONS_WORKSPACE_ROUTING_FAILED");
    let mut details = json!({
        "suggestion": "Call list_workspace_folders and retry with an allowed workspace_folder_id."
    });
    if code == "WORKSPACE_FOLDER_NOT_SELECTED" {
        details["available_folders"] = crate::tools::hub::routing_folder_options(folder_listing);
        details["selected_folder_id"] = folder_listing
            .get("selected_folder_id")
            .cloned()
            .unwrap_or(Value::Null);
        details["next_action"] = Value::String(
            "Choose one available_folders entry and retry with its id as workspace_folder_id."
                .into(),
        );
        details["suggestion"] = Value::String(
            "Choose an available folder and retry with workspace_folder_id; no default folder is selected."
                .into(),
        );
    }
    json!({
        "ok": false,
        "status": "error",
        "summary": message,
        "error": {
            "code": code,
            "message": message,
            "category": "workspace_routing",
            "retryable": false,
            "details": details
        }
    })
}

fn action_response(
    tool_name: &str,
    workspace_folder_id: Option<&str>,
    workspace: Option<&str>,
    structured: Value,
) -> Response {
    let result = wrap_tool_result(structured);
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let status = if is_error {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    (
        status,
        Json(json!({
            "ok": !is_error,
            "tool": tool_name,
            "workspace_folder_id": workspace_folder_id,
            "workspace": workspace,
            "structured_content": result.get("structuredContent").cloned().unwrap_or(Value::Null),
            "content": result.get("content").cloned().unwrap_or_else(|| json!([])),
            "is_error": is_error
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{bind_listener, routing_error, take_workspace_folder_id};

    #[test]
    fn workspace_folder_selector_is_removed_before_tool_dispatch() {
        let mut arguments = json!({"workspace_folder_id": "folder-b", "path": "src/lib.rs"});
        let folder_id = take_workspace_folder_id(&mut arguments).expect("selector");
        assert_eq!(folder_id.as_deref(), Some("folder-b"));
        assert!(arguments.get("workspace_folder_id").is_none());
        assert_eq!(arguments["path"], "src/lib.rs");
    }

    #[test]
    fn missing_workspace_selection_uses_specific_error_code() {
        let result = routing_error(
            "WORKSPACE_FOLDER_NOT_SELECTED: 此 Actions 請求必須明確提供 workspace_folder_id。"
                .into(),
            &json!({
                "selected_folder_id": Value::Null,
                "folders": [{
                    "id": "folder-b",
                    "name": "Folder B",
                    "path": "C:/workspace/folder-b",
                    "history_dir": "C:/workspace/folder-b/docs/history-session"
                }]
            }),
        );

        assert_eq!(result["error"]["code"], "WORKSPACE_FOLDER_NOT_SELECTED");
        assert_eq!(
            result["error"]["message"],
            "此 Actions 請求必須明確提供 workspace_folder_id。"
        );
        assert_eq!(
            result["error"]["details"]["available_folders"],
            json!([{
                "id": "folder-b",
                "name": "Folder B",
                "path": "C:/workspace/folder-b"
            }])
        );
        assert!(result["error"]["details"]["selected_folder_id"].is_null());
        assert!(result["error"]["details"]["next_action"]
            .as_str()
            .expect("next action")
            .contains("workspace_folder_id"));
    }

    #[test]
    fn bind_listener_reports_port_conflict_synchronously() {
        let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("占用测试端口");
        let port = occupied.local_addr().expect("读取测试端口").port();

        assert!(bind_listener("127.0.0.1", port).is_err());
    }
}
