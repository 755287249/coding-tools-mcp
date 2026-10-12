//! UI plugin helpers: an allow-listed HTTPS proxy and a login window that
//! captures the CodeRabbit web session (same rules as the Node Agent's
//! `management/pluginHttp.ts`).
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

use crate::error::{AppError, AppResult};

const HEADERS: [&str; 8] = [
    "authorization",
    "content-type",
    "accept",
    "x-trpc-source",
    "x-clerk-git-provider",
    "x-coderabbitai-organization",
    "x-coderabbitai-workspace",
    "x-github-api-version",
];
const MAX_RESPONSE: usize = 4 * 1024 * 1024;
const CAPTURE_HOST: &str = "ctmcp-capture.invalid";
const LOGIN_LABEL: &str = "plugin-login";

fn rules() -> &'static [(&'static str, regex::Regex)] {
    static RULES: std::sync::OnceLock<Vec<(&'static str, regex::Regex)>> = std::sync::OnceLock::new();
    RULES.get_or_init(|| {
        [
            ("GET", r"^https://app\.coderabbit\.ai/trpc/[A-Za-z][A-Za-z0-9.,]{0,200}\?batch=1(&input=[^#]*)?$"),
            ("POST", r"^https://app\.coderabbit\.ai/trpc/[A-Za-z][A-Za-z0-9.,]{0,200}\?batch=1$"),
            ("POST", r"^https://github\.com/login/device/code$"),
            ("POST", r"^https://github\.com/login/oauth/access_token$"),
            ("POST", r"^https://api\.github\.com/user/repos$"),
            ("PUT", r"^https://api\.github\.com/repos/[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}/contents/[A-Za-z0-9_./-]{1,200}$"),
        ]
        .into_iter()
        .map(|(method, pattern)| (method, regex::Regex::new(pattern).expect("valid plugin rule")))
        .collect()
    })
}

pub(crate) fn plugin_request_allowed(method: &str, url: &str) -> bool {
    !url.contains("..") && rules().iter().any(|(m, rule)| *m == method && rule.is_match(url))
}

#[derive(Deserialize)]
pub struct PluginHttpRequest {
    url: String,
    method: String,
    #[serde(default)]
    headers: std::collections::HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

#[tauri::command(async)]
pub async fn plugin_http(request: PluginHttpRequest) -> AppResult<Value> {
    if !plugin_request_allowed(&request.method, &request.url) {
        return Err(AppError::Message("Plugin request not allowed".into()));
    }
    if request.body.as_ref().is_some_and(|b| b.len() > 256 * 1024) {
        return Err(AppError::Message("Plugin request body too large".into()));
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .user_agent("Coding-Tools-MCP-Plugin/1.0")
        .build()
        .map_err(|e| AppError::Message(e.to_string()))?;
    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|_| AppError::Message("Invalid method".into()))?;
    let mut builder = client.request(method.clone(), &request.url);
    for (key, value) in &request.headers {
        let name = key.to_ascii_lowercase();
        if HEADERS.contains(&name.as_str()) && !value.contains(['\r', '\n']) {
            builder = builder.header(name, value);
        }
    }
    if method != reqwest::Method::GET {
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
    }
    let response = builder.send().await.map_err(|e| AppError::Message(e.to_string()))?;
    let status = response.status().as_u16();
    let bytes = response.bytes().await.map_err(|e| AppError::Message(e.to_string()))?;
    if bytes.len() > MAX_RESPONSE {
        return Err(AppError::Message("Plugin response too large".into()));
    }
    Ok(json!({"status": status, "body": String::from_utf8_lossy(&bytes)}))
}

#[derive(Deserialize)]
pub struct PluginLoginOptions {
    url: String,
    capture: bool,
    #[serde(default)]
    auto: bool,
    #[serde(default)]
    hidden: bool,
}

/// Runs in every page of the login window. On app.coderabbit.ai it waits for
/// the web app's own session (access token, user and organization) and hands
/// it over by navigating to a reserved host that the window intercepts.
fn capture_script(auto: bool) -> String {
    format!(
        r#"(function(){{
  if (location.hostname !== 'app.coderabbit.ai') return;
  var AUTO = {auto};
  function tick(){{
    try {{
      var a = sessionStorage.getItem('accessToken'), u = sessionStorage.getItem('user');
      var state = u ? JSON.parse(u).state : null;
      if (a && state && state.user && state.user.id && state.currentOrganization && state.currentOrganization.id && location.pathname.indexOf('/login') !== 0) {{
        var d = {{accessToken: a, user: u, clerkGitProvider: localStorage.getItem('clerkGitProvider') || '', clerkSsoWorkspaceId: localStorage.getItem('clerkSsoWorkspaceId') || ''}};
        location.href = 'https://{CAPTURE_HOST}/?d=' + encodeURIComponent(btoa(unescape(encodeURIComponent(JSON.stringify(d)))));
        return;
      }}
      if (AUTO && location.pathname.indexOf('/login') === 0 && !window.__ctmcpClicked) {{
        var b = Array.prototype.find.call(document.querySelectorAll('button'), function(x){{ return x.textContent.trim() === 'GitHub Cloud'; }});
        if (b) {{ window.__ctmcpClicked = true; b.click(); }}
      }}
    }} catch (e) {{}}
    setTimeout(tick, 1000);
  }}
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', tick); else tick();
}})();"#
    )
}

fn decode_capture(url: &tauri::Url) -> Option<Value> {
    let data = url.query_pairs().find(|(k, _)| k == "d")?.1.to_string();
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value["accessToken"].as_str().filter(|s| !s.is_empty())?;
    Some(value)
}

fn login_url_allowed(url: &str) -> bool {
    url == "https://app.coderabbit.ai/login" || url == "https://github.com/login/device"
}

#[tauri::command(async)]
pub async fn plugin_login_window(app: tauri::AppHandle, options: PluginLoginOptions) -> AppResult<Value> {
    if !login_url_allowed(&options.url) {
        return Err(AppError::Message("Login page not allowed".into()));
    }
    if let Some(existing) = app.get_webview_window(LOGIN_LABEL) {
        let _ = existing.close();
    }
    let url = tauri::Url::parse(&options.url).map_err(|e| AppError::Message(e.to_string()))?;
    let (sender, receiver) = tokio::sync::oneshot::channel::<Option<Value>>();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let navigation = sender.clone();
    let closer = app.clone();
    let mut builder = WebviewWindowBuilder::new(&app, LOGIN_LABEL, WebviewUrl::External(url))
        .title("CodeRabbit")
        .inner_size(980.0, 760.0)
        .visible(!options.hidden)
        .on_navigation(move |target| {
            if target.host_str() != Some(CAPTURE_HOST) {
                return true;
            }
            if let Some(sender) = navigation.lock().ok().and_then(|mut s| s.take()) {
                let _ = sender.send(decode_capture(target));
            }
            let closer = closer.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(window) = closer.get_webview_window(LOGIN_LABEL) {
                    let _ = window.close();
                }
            });
            false
        });
    if options.capture {
        builder = builder.initialization_script(capture_script(options.auto));
    }
    let window = builder.build().map_err(|e| AppError::Message(e.to_string()))?;
    if !options.capture {
        return Ok(Value::Null);
    }
    let closed = sender.clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            if let Some(sender) = closed.lock().ok().and_then(|mut s| s.take()) {
                let _ = sender.send(None);
            }
        }
    });
    let limit = Duration::from_secs(if options.hidden { 90 } else { 900 });
    let result = tokio::time::timeout(limit, receiver).await;
    if let Some(window) = app.get_webview_window(LOGIN_LABEL) {
        let _ = window.close();
    }
    match result {
        Ok(Ok(Some(value))) => Ok(value),
        Ok(_) => Err(AppError::Message("Login window closed before CodeRabbit login finished".into())),
        Err(_) => Err(AppError::Message("CodeRabbit login timed out".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::plugin_request_allowed;

    #[test]
    fn plugin_proxy_allow_list_matches_node_agent() {
        assert!(plugin_request_allowed("GET", "https://app.coderabbit.ai/trpc/repositories.getAllRepos?batch=1&input=%7B%7D"));
        assert!(plugin_request_allowed("POST", "https://app.coderabbit.ai/trpc/codingAgent.enqueueCodingTask?batch=1"));
        assert!(plugin_request_allowed("PUT", "https://api.github.com/repos/me/seed-1/contents/src/a.py"));
        for (method, url) in [
            ("GET", "http://127.0.0.1:3789/admin/api/config"),
            ("POST", "https://app.coderabbit.ai.evil.test/trpc/x?batch=1"),
            ("DELETE", "https://api.github.com/user/repos"),
            ("PUT", "https://api.github.com/repos/me/r/contents/../x"),
            ("GET", "https://github.com/login/device/code"),
        ] {
            assert!(!plugin_request_allowed(method, url), "{url}");
        }
    }
}
