//! UI plugin helpers: an allow-listed HTTPS proxy and a login window that
//! captures the CodeRabbit web session (same rules as the Node Agent's
//! `management/pluginHttp.ts`).
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

use crate::error::{AppError, AppResult};
use crate::platform::platform;

const MAX_RESPONSE: usize = 4 * 1024 * 1024;
const MAX_REQUEST: usize = 512 * 1024;
const CAPTURE_HOST: &str = "ctmcp-capture.invalid";
const LOGIN_LABEL: &str = "plugin-login";

fn public_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(ip) => {
            let [a, b, _, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || a == 0
                || (a == 100 && (64..=127).contains(&b))
                || (a == 198 && (b == 18 || b == 19)))
        }
        std::net::IpAddr::V6(ip) => {
            if let Some(ipv4) = ip.to_ipv4_mapped() {
                return public_address(std::net::IpAddr::V4(ipv4));
            }
            let first = ip.segments()[0];
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

async fn public_url(raw: &str) -> AppResult<(reqwest::Url, Vec<std::net::SocketAddr>)> {
    let url = reqwest::Url::parse(raw)
        .map_err(|_| AppError::Message("Plugin request URL is invalid".into()))?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::Message("Plugin requests must use HTTPS".into()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| AppError::Message("Plugin request host not allowed".into()))?;
    let lower = host.to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
    {
        return Err(AppError::Message("Plugin request host not allowed".into()));
    }
    let addresses: Vec<_> =
        tokio::net::lookup_host((host, url.port_or_known_default().unwrap_or(443)))
            .await
            .map_err(|_| AppError::Message("Plugin request host not allowed".into()))?
            .collect();
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| !public_address(address.ip()))
    {
        return Err(AppError::Message("Plugin request host not allowed".into()));
    }
    Ok((url, addresses))
}

/// `reqwest` hides the real cause ("connection reset", "timed out", TLS) in
/// the source chain; include it so users can see why a request failed.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = cause.source();
    }
    message
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
    let method = reqwest::Method::from_bytes(request.method.to_ascii_uppercase().as_bytes())
        .map_err(|_| AppError::Message("Invalid method".into()))?;
    if !matches!(
        method,
        reqwest::Method::GET
            | reqwest::Method::POST
            | reqwest::Method::PUT
            | reqwest::Method::PATCH
            | reqwest::Method::DELETE
    ) {
        return Err(AppError::Message(
            "Plugin request method not allowed".into(),
        ));
    }
    let (url, addresses) = public_url(&request.url).await?;
    if request.body.as_ref().is_some_and(|b| b.len() > MAX_REQUEST) {
        return Err(AppError::Message("Plugin request body too large".into()));
    }
    let mut client_builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .user_agent("Coding-Tools-MCP-Plugin/1.0");
    for address in addresses {
        client_builder = client_builder.resolve(url.host_str().unwrap_or_default(), address);
    }
    let client = client_builder
        .build()
        .map_err(|e| AppError::Message(e.to_string()))?;
    let mut builder = client.request(method.clone(), url);
    for (key, value) in &request.headers {
        let name = key.to_ascii_lowercase();
        if !matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "connection"
                | "transfer-encoding"
                | "upgrade"
                | "keep-alive"
                | "proxy-authorization"
                | "te"
                | "trailer"
        ) && !name.starts_with("proxy-")
            && !name.starts_with("sec-")
            && !value.contains(['\r', '\n'])
        {
            builder = builder.header(name, value);
        }
    }
    if method != reqwest::Method::GET {
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
    }
    // Retry connection failures (nothing was sent yet), common on unstable
    // networks to github.com. Other errors are returned with their cause chain.
    let mut attempt = 0;
    let response = loop {
        let Some(request) = builder.try_clone() else {
            break builder
                .send()
                .await
                .map_err(|e| AppError::Message(error_chain(&e)))?;
        };
        match request.send().await {
            Ok(response) => break response,
            Err(error) if attempt < 2 && (error.is_connect() || error.is_timeout()) => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(800 * attempt)).await;
            }
            Err(error) => return Err(AppError::Message(error_chain(&error))),
        }
    };
    let status = response.status().as_u16();
    let mut headers = serde_json::Map::new();
    for name in response.headers().keys() {
        let values: Vec<_> = response
            .headers()
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .map(str::to_owned)
            .collect();
        if values.len() == 1 {
            headers.insert(name.to_string(), Value::String(values[0].clone()));
        } else if !values.is_empty() {
            headers.insert(name.to_string(), json!(values));
        }
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::Message(e.to_string()))?;
    if bytes.len() > MAX_RESPONSE {
        return Err(AppError::Message("Plugin response too large".into()));
    }
    Ok(json!({"status": status, "headers": headers, "body": String::from_utf8_lossy(&bytes)}))
}

#[derive(Clone, Deserialize, Serialize)]
pub struct PluginManifest {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    permissions: Value,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct PluginPackage {
    format: u8,
    manifest: PluginManifest,
    code: String,
}

fn valid_plugin(package: &PluginPackage) -> AppResult<()> {
    let id = &package.manifest.id;
    if package.format != 1
        || id.len() < 2
        || id.len() > 64
        || !id.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        || !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        || package.manifest.name.trim().is_empty()
        || package.manifest.version.trim().is_empty()
        || package.code.trim().is_empty()
        || package.code.len() > MAX_RESPONSE
    {
        return Err(AppError::Message("Invalid plugin package".into()));
    }
    Ok(())
}

fn plugins_dir() -> AppResult<std::path::PathBuf> {
    Ok(platform().app_config_dir()?.join("plugins"))
}
fn plugin_path(id: &str, suffix: &str) -> AppResult<std::path::PathBuf> {
    if id.len() < 2
        || id.len() > 64
        || !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err(AppError::Message("Invalid plugin ID".into()));
    }
    Ok(plugins_dir()?.join(format!("{id}{suffix}")))
}

#[tauri::command]
pub fn plugin_list() -> AppResult<Vec<PluginPackage>> {
    let dir = plugins_dir()?;
    std::fs::create_dir_all(&dir)?;
    let mut result = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.to_string_lossy().ends_with(".ctplugin.json") {
            if let Ok(package) = serde_json::from_slice::<PluginPackage>(&std::fs::read(path)?) {
                if valid_plugin(&package).is_ok() {
                    result.push(package);
                }
            }
        }
    }
    Ok(result)
}

#[tauri::command]
pub fn plugin_install(package: PluginPackage) -> AppResult<PluginPackage> {
    valid_plugin(&package)?;
    let dir = plugins_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = plugin_path(&package.manifest.id, ".ctplugin.json")?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec(&package)?)?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    std::fs::rename(temporary, path)?;
    Ok(package)
}

#[tauri::command]
pub fn plugin_remove(id: String) -> AppResult<()> {
    for suffix in [".ctplugin.json", ".data.json"] {
        let path = plugin_path(&id, suffix)?;
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn plugin_data_get(id: String) -> AppResult<Value> {
    let path = plugin_path(&id, ".data.json")?;
    if !path.exists() {
        return Ok(json!({}));
    }
    let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    if !value.is_object() {
        return Err(AppError::Message("Invalid plugin data".into()));
    }
    Ok(value)
}

#[tauri::command]
pub fn plugin_data_set(id: String, data: Value) -> AppResult<()> {
    if !data.is_object() {
        return Err(AppError::Message("Invalid plugin data".into()));
    }
    let bytes = serde_json::to_vec(&data)?;
    if bytes.len() > 1024 * 1024 {
        return Err(AppError::Message("Plugin data too large".into()));
    }
    let dir = plugins_dir()?;
    std::fs::create_dir_all(dir)?;
    let path = plugin_path(&id, ".data.json")?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes)?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    std::fs::rename(temporary, path)?;
    Ok(())
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
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value["accessToken"].as_str().filter(|s| !s.is_empty())?;
    Some(value)
}

fn login_url_allowed(url: &str) -> bool {
    url == "https://app.coderabbit.ai/login" || url == "https://github.com/login/device"
}

#[tauri::command(async)]
pub async fn plugin_login_window(
    app: tauri::AppHandle,
    options: PluginLoginOptions,
) -> AppResult<Value> {
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
    let window = builder
        .build()
        .map_err(|e| AppError::Message(e.to_string()))?;
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
        Ok(_) => Err(AppError::Message(
            "Login window closed before CodeRabbit login finished".into(),
        )),
        Err(_) => Err(AppError::Message("CodeRabbit login timed out".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::public_address;

    #[test]
    fn plugin_proxy_rejects_non_public_addresses() {
        for address in [
            "127.0.0.1",
            "10.0.0.2",
            "192.168.1.1",
            "169.254.1.2",
            "::1",
            "fd00::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!public_address(address.parse().unwrap()), "{address}");
        }
        assert!(public_address("1.1.1.1".parse().unwrap()));
    }
}
