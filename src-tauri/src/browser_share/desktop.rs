use super::access::Access;
use axum::{
    body::Body,
    extract::DefaultBodyLimit,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use tauri::Manager;
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
static ACCESS: OnceLock<Mutex<Access>> = OnceLock::new();
fn access() -> &'static Mutex<Access> {
    ACCESS.get_or_init(|| Mutex::new(Access::default()))
}
pub fn init(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

#[tauri::command]
pub fn browser_sharing_status() -> Result<Value, String> {
    let gate = access().lock().map_err(|_| "Sharing lock unavailable")?;
    let lan_ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .ok()
        .and_then(|socket| {
            socket.connect("192.0.2.1:80").ok()?;
            Some(socket.local_addr().ok()?.ip().to_string())
        });
    Ok(
        json!({"enabled":gate.enabled(),"origins":gate.origins(),"lanIp":lan_ip,"password":gate.local_password()}),
    )
}
#[tauri::command]
pub fn configure_browser_sharing(enabled: bool, origins: Vec<String>) -> Result<Value, String> {
    let password = {
        let mut gate = access().lock().map_err(|_| "Sharing lock unavailable")?;
        if enabled {
            Some(gate.configure(origins)?)
        } else {
            gate.disable();
            None
        }
    };
    let mut status = browser_sharing_status()?;
    status["password"] = json!(password);
    Ok(status)
}
fn field<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|s| s.to_str().ok())
        .unwrap_or("")
}
fn token(headers: &HeaderMap) -> &str {
    field(headers, "authorization")
        .strip_prefix("Bearer ")
        .unwrap_or("")
}
fn reply(status: StatusCode, data: Value) -> Response {
    let mut response = (status, Json(data)).into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    response
}
async fn login(headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let Ok(mut gate) = access().lock() else {
        return reply(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"Sharing unavailable"}),
        );
    };
    if !gate.host_allowed(field(&headers, "host")) {
        return reply(
            StatusCode::FORBIDDEN,
            json!({"error":"Sharing is disabled or host is not configured"}),
        );
    }
    match gate.login(
        field(&headers, "origin"),
        body["password"].as_str().unwrap_or(""),
        Instant::now(),
    ) {
        Ok(token) => reply(StatusCode::OK, json!({"token":token,"expiresIn":43200})),
        Err(error) => reply(StatusCode::UNAUTHORIZED, json!({"error":error})),
    }
}
async fn invoke(headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let authorized = access().lock().is_ok_and(|gate| {
        gate.host_allowed(field(&headers, "host"))
            && gate.authorized(field(&headers, "origin"), token(&headers), Instant::now())
    });
    if !authorized {
        return reply(
            StatusCode::UNAUTHORIZED,
            json!({"error":"Browser login required"}),
        );
    }
    let Some(app) = APP.get().cloned() else {
        return reply(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"Desktop unavailable"}),
        );
    };
    let result =
        tokio::task::spawn_blocking(move || tauri::async_runtime::block_on(dispatch(app, body)))
            .await;
    match result {
        Ok(Ok(value)) => reply(StatusCode::OK, json!({"value":value})),
        Ok(Err(error)) => reply(StatusCode::BAD_REQUEST, json!({"error":error})),
        Err(_) => reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"Desktop operation failed"}),
        ),
    }
}
async fn logout(headers: HeaderMap) -> Response {
    if let Ok(mut gate) = access().lock() {
        gate.logout(token(&headers));
    }
    reply(StatusCode::OK, json!({"ok":true}))
}
async fn asset(headers: HeaderMap, uri: Uri) -> Response {
    if !access()
        .lock()
        .is_ok_and(|gate| gate.host_allowed(field(&headers, "host")))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(app) = APP.get() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let path = uri.path().trim_start_matches('/');
    let spa = path.is_empty()
        || path == "quick-setup"
        || path.starts_with("workspace/")
        || path.starts_with("settings/");
    let known = spa
        || path.starts_with("_app/")
        || ["favicon.png", "favicon.svg", "chat-preview.html"].contains(&path);
    if !known || path.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(asset) = app.asset_resolver().get(if spa {
        "index.html".into()
    } else {
        path.into()
    }) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let csp = if path == "chat-preview.html" {
        "sandbox allow-scripts; frame-ancestors 'self'"
    } else {
        "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data: blob:; font-src 'self' data:; frame-src 'self' blob:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'"
    };
    Response::builder()
        .status(200)
        .header("Content-Type", asset.mime_type)
        .header("Content-Security-Policy", csp)
        .header("Cache-Control", "no-store")
        .header("Referrer-Policy", "no-referrer")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(asset.bytes))
        .unwrap()
}
pub fn router() -> Router {
    Router::new()
        .route("/browser/login", post(login))
        .route("/browser/invoke", post(invoke))
        .route("/browser/logout", post(logout))
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .fallback(axum::routing::get(asset))
}
fn arg<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null))
        .map_err(|_| format!("Invalid argument: {key}"))
}
fn value<T: serde::Serialize>(result: crate::error::AppResult<T>) -> Result<Value, String> {
    serde_json::to_value(result.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

// Explicit allowlist. No arbitrary Tauri/plugin calls, app shutdown or update installation.
async fn dispatch(app: tauri::AppHandle, body: Value) -> Result<Value, String> {
    let args = &body["args"];
    match body["command"].as_str().unwrap_or("") {
        "read_workspace_activity" => value(crate::commands::read_workspace_activity(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "sinceRev")?,
        )),
        "read_workspace_activity_detail" => value(crate::commands::read_workspace_activity_detail(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "seq")?,
        )),
        "clear_workspace_activity" => value(crate::commands::clear_workspace_activity(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
        )),
        "list_frp_profiles" => value(crate::commands::list_frp_profiles(
            app.state::<crate::app_state::AppState>(),
        )),
        "save_frp_profile" => value(crate::commands::save_frp_profile(
            app.state::<crate::app_state::AppState>(),
            arg(args, "profile")?,
            arg(args, "token")?,
        )),
        "delete_frp_profile" => value(crate::commands::delete_frp_profile(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
        )),
        "get_proxy" => value(crate::commands::get_proxy(
            app.state::<crate::app_state::AppState>(),
        )),
        "set_proxy" => value(crate::commands::set_proxy(
            app.state::<crate::app_state::AppState>(),
            arg(args, "proxy")?,
        )),
        "set_last_workspace" => value(crate::commands::set_last_workspace(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
        )),
        "get_last_workspace_id" => value(crate::commands::get_last_workspace_id(
            app.state::<crate::app_state::AppState>(),
        )),
        "run_health_checks" => value(
            crate::commands::run_health_checks(
                app.state::<crate::app_state::AppState>(),
                arg(args, "id")?,
            )
            .await,
        ),
        "list_history_sessions" => value(crate::commands::list_history_sessions(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "folderId")?,
        )),
        "read_history_session" => value(crate::commands::read_history_session(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "number")?,
            arg(args, "folderId")?,
        )),
        "read_workspace_logs" => value(
            crate::commands::read_workspace_logs(
                app.state::<crate::app_state::AppState>(),
                arg(args, "id")?,
                arg(args, "service")?,
            )
            .await,
        ),
        "start_runtime" => value(
            crate::commands::start_runtime(
                app.state::<crate::app_state::AppState>(),
                arg(args, "id")?,
            )
            .await,
        ),
        "stop_runtime" => value(
            crate::commands::stop_runtime(
                app.state::<crate::app_state::AppState>(),
                arg(args, "id")?,
            )
            .await,
        ),
        "get_runtime_status" => value(crate::commands::get_runtime_status(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
        )),
        "start_actions_runtime" => value(
            crate::commands::start_actions_runtime(
                app.state::<crate::app_state::AppState>(),
                arg(args, "id")?,
            )
            .await,
        ),
        "stop_actions_runtime" => value(
            crate::commands::stop_actions_runtime(
                app.state::<crate::app_state::AppState>(),
                arg(args, "id")?,
            )
            .await,
        ),
        "get_actions_runtime_status" => value(crate::commands::get_actions_runtime_status(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
        )),
        "restart_runtime" => value(crate::commands::restart_runtime(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
        )),
        "restart_actions_runtime" => value(crate::commands::restart_actions_runtime(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
        )),
        "list_sandbox_backends" => serde_json::to_value(crate::commands::list_sandbox_backends())
            .map_err(|e| e.to_string()),
        "get_workspace_secret" => value(crate::commands::get_workspace_secret(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "key")?,
        )),
        "set_workspace_secret" => value(crate::commands::set_workspace_secret(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "key")?,
            arg(args, "value")?,
        )),
        "regenerate_workspace_secret" => value(crate::commands::regenerate_workspace_secret(
            app.clone(),
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "key")?,
        )),
        "get_shared_secret" => value(crate::commands::get_shared_secret(
            app.state::<crate::app_state::AppState>(),
            arg(args, "key")?,
        )),
        "set_shared_secret" => value(crate::commands::set_shared_secret(
            app.clone(),
            app.state::<crate::app_state::AppState>(),
            arg(args, "key")?,
            arg(args, "value")?,
        )),
        "regenerate_shared_secret" => value(crate::commands::regenerate_shared_secret(
            app.clone(),
            app.state::<crate::app_state::AppState>(),
            arg(args, "key")?,
        )),
        "list_software" => value(crate::commands::list_software()),
        "install_software" => value(crate::commands::install_software(arg(args, "kind")?).await),
        "uninstall_software" => value(crate::commands::uninstall_software(arg(args, "kind")?)),
        "get_download_config" => value(crate::commands::get_download_config(
            app.state::<crate::app_state::AppState>(),
        )),
        "set_download_config" => value(crate::commands::set_download_config(
            app.state::<crate::app_state::AppState>(),
            arg(args, "config")?,
        )),
        "read_workspace_telemetry" => value(crate::commands::read_workspace_telemetry(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "limit")?,
            arg(args, "errorsOnly")?,
            arg(args, "minDurationMs")?,
            arg(args, "sinceTsMs")?,
        )),
        "get_frp_snippet" => value(crate::commands::get_frp_snippet(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "service")?,
        )),
        "restart_tunnel" => value(
            crate::commands::restart_tunnel(
                app.state::<crate::app_state::AppState>(),
                arg(args, "id")?,
                arg(args, "service")?,
            )
            .await,
        ),
        "start_tunnel" => value(
            crate::commands::start_tunnel(
                app.state::<crate::app_state::AppState>(),
                arg(args, "id")?,
                arg(args, "service")?,
            )
            .await,
        ),
        "stop_tunnel" => value(
            crate::commands::stop_tunnel(
                app.state::<crate::app_state::AppState>(),
                arg(args, "id")?,
                arg(args, "service")?,
            )
            .await,
        ),
        "test_tunnel" => value(
            crate::commands::test_tunnel(
                app.state::<crate::app_state::AppState>(),
                arg(args, "id")?,
                arg(args, "service")?,
            )
            .await,
        ),
        "list_workspaces" => value(crate::commands::list_workspaces(
            app.state::<crate::app_state::AppState>(),
        )),
        "create_workspace" => value(crate::commands::create_workspace(
            app.state::<crate::app_state::AppState>(),
            arg(args, "path")?,
            arg(args, "name")?,
        )),
        "list_wsl_distributions" => value(crate::commands::list_wsl_distributions()),
        "update_workspace" => value(
            crate::commands::update_workspace(
                app.state::<crate::app_state::AppState>(),
                arg(args, "profile")?,
            )
            .await,
        ),
        "add_workspace_folder" => value(crate::commands::add_workspace_folder(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "path")?,
            arg(args, "name")?,
        )),
        "add_wsl_workspace_folder" => value(crate::commands::add_wsl_workspace_folder(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "distro")?,
            arg(args, "linuxPath")?,
            arg(args, "name")?,
        )),
        "remove_workspace_folder" => value(crate::commands::remove_workspace_folder(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "folderId")?,
        )),
        "delete_workspace" => value(crate::commands::delete_workspace(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
        )),
        "get_workspace_skills" => value(crate::commands::get_workspace_skills(
            app.state::<crate::app_state::AppState>(),
            arg(args, "workspaceId")?,
        )),
        "set_workspace_skills_active" => value(crate::commands::set_workspace_skills_active(
            app.state::<crate::app_state::AppState>(),
            arg(args, "workspaceId")?,
            arg(args, "active")?,
        )),
        "set_workspace_skill_enabled" => value(crate::commands::set_workspace_skill_enabled(
            app.state::<crate::app_state::AppState>(),
            arg(args, "workspaceId")?,
            arg(args, "skillKey")?,
            arg(args, "enabled")?,
        )),
        "get_workspace_extensions" => value(
            crate::commands::get_workspace_extensions(
                app.state::<crate::app_state::AppState>(),
                arg(args, "workspaceId")?,
            )
            .await,
        ),
        "set_workspace_extension_active" => value(crate::commands::set_workspace_extension_active(
            app.state::<crate::app_state::AppState>(),
            arg(args, "workspaceId")?,
            arg(args, "extensionKind")?,
            arg(args, "active")?,
        )),
        "set_workspace_extension_enabled" => {
            value(crate::commands::set_workspace_extension_enabled(
                app.state::<crate::app_state::AppState>(),
                arg(args, "workspaceId")?,
                arg(args, "extensionKind")?,
                arg(args, "extensionKey")?,
                arg(args, "enabled")?,
            ))
        }
        "local_chat" => value(crate::commands::local_chat(
            app.state::<crate::app_state::AppState>(),
            arg(args, "id")?,
            arg(args, "folderId")?,
            arg(args, "args")?,
        )),
        "check_app_update" => {
            serde_json::to_value(crate::updates::check_app_update(arg(args, "repo")?).await?)
                .map_err(|e| e.to_string())
        }
        _ => Err("This operation is available only in the local desktop application".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn http_login_rejects_unknown_host_origin_token_and_revokes_on_disable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let password = access()
            .lock()
            .unwrap()
            .configure(vec![origin.clone()])
            .unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, router()).await.unwrap() });
        let client = reqwest::Client::new();
        let login_url = format!("{origin}/browser/login");
        let api_url = format!("{origin}/browser/invoke");
        assert_eq!(
            client
                .post(&api_url)
                .json(&json!({"command":"list_workspaces"}))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            client
                .post(&login_url)
                .header("Origin", "https://evil.example")
                .json(&json!({"password":password}))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            client
                .post(&login_url)
                .header("Origin", &origin)
                .header("Host", "evil.example")
                .json(&json!({"password":password}))
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            client
                .post(&login_url)
                .header("Origin", &origin)
                .json(&json!({"password":"wrong"}))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        let response = client
            .post(&login_url)
            .header("Origin", &origin)
            .json(&json!({"password":password}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let token = response.json::<Value>().await.unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned();
        // No desktop AppHandle in this test: 503 proves the authenticated request reached dispatch admission.
        assert_eq!(
            client
                .post(&api_url)
                .header("Origin", &origin)
                .bearer_auth(&token)
                .json(&json!({"command":"list_workspaces"}))
                .send()
                .await
                .unwrap()
                .status(),
            503
        );
        assert_eq!(
            client
                .post(&api_url)
                .header("Origin", "https://evil.example")
                .bearer_auth(&token)
                .json(&json!({"command":"list_workspaces"}))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        access().lock().unwrap().disable();
        assert_eq!(
            client
                .post(&api_url)
                .header("Origin", &origin)
                .bearer_auth(&token)
                .json(&json!({"command":"list_workspaces"}))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(client.get(&origin).send().await.unwrap().status(), 404);
        task.abort();
    }
}
