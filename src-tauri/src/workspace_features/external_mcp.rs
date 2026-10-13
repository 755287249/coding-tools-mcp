use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use reqwest::header::{HeaderName, HeaderValue};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use super::{discover_extensions, runtime, McpServerDescriptor};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug)]
pub struct ExternalTool {
    pub name: String,
    pub logical_name: String,
    pub server_key: String,
    pub tool_name: String,
    pub folder_id: String,
    pub definition: Value,
}

struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

enum Transport {
    None,
    Stdio(StdioTransport),
    Http(reqwest::Client),
}

pub struct ExternalMcpConnection {
    pub server: McpServerDescriptor,
    workspace_root: PathBuf,
    next_id: u64,
    session_id: Option<String>,
    initialized: bool,
    tools: Vec<ExternalTool>,
    error_message: Option<String>,
    transport: Transport,
}

fn record(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

fn sanitized(value: &str) -> String {
    let mut result = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    result = result.trim_matches('_').chars().take(40).collect();
    if result.is_empty() {
        "item".into()
    } else {
        result
    }
}

fn proxy_tool_name(server: &McpServerDescriptor, tool_name: &str) -> String {
    let scope = if !server.folder_id.is_empty() {
        sanitized(&server.folder_id)
    } else {
        server.scope.clone()
    };
    let base = format!(
        "mcp__{}__{}__{}__{}",
        server.provider,
        scope,
        sanitized(&server.name),
        sanitized(tool_name)
    );
    if base.len() <= 120 {
        base
    } else {
        let digest = format!("{:x}", Sha256::digest(base.as_bytes()));
        format!(
            "{}_{}",
            base.chars().take(103).collect::<String>(),
            &digest[..16]
        )
    }
}

fn logical_tool_name(server: &McpServerDescriptor, tool_name: &str) -> String {
    format!("mcp__{}__{}", sanitized(&server.name), sanitized(tool_name))
}

fn workspace_root(
    server: &McpServerDescriptor,
    folders: &[crate::workspace::WorkspaceFolder],
) -> PathBuf {
    folders
        .iter()
        .find(|folder| folder.id == server.folder_id)
        .map(|folder| PathBuf::from(&folder.path))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

impl ExternalMcpConnection {
    pub fn new(server: McpServerDescriptor, workspace_root: PathBuf) -> Self {
        Self {
            server,
            workspace_root,
            next_id: 1,
            session_id: None,
            initialized: false,
            tools: Vec::new(),
            error_message: None,
            transport: Transport::None,
        }
    }

    pub fn connected(&self) -> bool {
        self.initialized
    }

    pub fn error(&self) -> Option<&str> {
        self.error_message.as_deref()
    }

    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    async fn ensure_initialized(&mut self) -> Result<(), String> {
        if self.initialized {
            return Ok(());
        }
        let result: Result<(), String> = async {
            let initialized = self
                .request(
                    "initialize",
                    json!({
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "clientInfo": { "name": "coding-tools-mcp-rust", "version": "extension-proxy" }
                    }),
                )
                .await?;
            if !initialized.is_object() {
                return Err("MCP server returned no initialize result".into());
            }
            self.notify("notifications/initialized", json!({})).await?;
            let listed = self.request("tools/list", json!({})).await?;
            self.tools = listed
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|raw| {
                    let tool = record(raw);
                    let name = tool.get("name")?.as_str()?.trim().to_string();
                    if name.is_empty() {
                        return None;
                    }
                    let annotations = tool.get("annotations").map(record).unwrap_or_default();
                    let definition = json!({
                        "name": proxy_tool_name(&self.server, &name),
                        "title": tool.get("title").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("{}: {}", self.server.name, name)),
                        "description": tool.get("description").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("Tool {name} from external MCP server {}.", self.server.name)),
                        "inputSchema": tool.get("inputSchema").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
                        "annotations": {
                            "title": annotations.get("title").and_then(Value::as_str).or_else(|| tool.get("title").and_then(Value::as_str)).unwrap_or(&name),
                            "readOnlyHint": annotations.get("readOnlyHint").and_then(Value::as_bool) == Some(true),
                            "destructiveHint": annotations.get("destructiveHint").and_then(Value::as_bool) != Some(false),
                            "idempotentHint": annotations.get("idempotentHint").and_then(Value::as_bool) == Some(true),
                            "openWorldHint": annotations.get("openWorldHint").and_then(Value::as_bool) != Some(false)
                        }
                    });
                    Some(ExternalTool {
                        name: proxy_tool_name(&self.server, &name),
                        logical_name: logical_tool_name(&self.server, &name),
                        server_key: self.server.key.clone(),
                        tool_name: name,
                        folder_id: self.server.folder_id.clone(),
                        definition,
                    })
                })
                .collect();
            self.initialized = true;
            self.error_message = None;
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            self.error_message = Some(error.clone());
            self.close().await;
        }
        result
    }

    pub async fn refresh_tools(&mut self) -> Result<Vec<ExternalTool>, String> {
        self.ensure_initialized().await?;
        Ok(self.tools.clone())
    }

    pub async fn call(&mut self, tool_name: &str, args: Value) -> Result<Value, String> {
        self.ensure_initialized().await?;
        self.request(
            "tools/call",
            json!({ "name": tool_name, "arguments": args }),
        )
        .await
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        match self.server.transport.as_str() {
            "stdio" => self.stdio_request(id, method, params).await,
            "http" => self.http_request(id, method, params).await,
            transport => Err(format!("Unsupported MCP transport: {transport}")),
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let payload = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        match self.server.transport.as_str() {
            "stdio" => {
                self.ensure_process().await?;
                let Transport::Stdio(transport) = &mut self.transport else {
                    return Err("MCP stdio process is unavailable".into());
                };
                let mut encoded =
                    serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
                encoded.push(b'\n');
                transport
                    .stdin
                    .write_all(&encoded)
                    .await
                    .map_err(|error| error.to_string())?;
                transport
                    .stdin
                    .flush()
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(())
            }
            "http" => {
                let _ = self.http_exchange(payload).await?;
                Ok(())
            }
            transport => Err(format!("Unsupported MCP transport: {transport}")),
        }
    }

    async fn ensure_process(&mut self) -> Result<(), String> {
        if matches!(self.transport, Transport::Stdio(_)) {
            return Ok(());
        }
        let command = self
            .server
            .command
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "MCP stdio server is missing command".to_string())?;
        let cwd = self
            .server
            .cwd
            .as_deref()
            .map(PathBuf::from)
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    self.workspace_root.join(path)
                }
            })
            .unwrap_or_else(|| self.workspace_root.clone());
        let extension = Path::new(command)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let requires_shell = cfg!(windows) && matches!(extension.as_str(), "bat" | "cmd");
        let mut process = if requires_shell {
            let mut process =
                Command::new(std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into()));
            process.args(["/d", "/s", "/c", "call", command]);
            process.args(&self.server.args);
            process
        } else {
            let mut process = Command::new(command);
            process.args(&self.server.args);
            process
        };
        process
            .current_dir(cwd)
            .envs(&self.server.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for name in &self.server.env_vars {
            if let Ok(value) = std::env::var(name) {
                process.env(name, value);
            }
        }
        let mut child = process.spawn().map_err(|error| error.to_string())?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "MCP stdin is unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "MCP stdout is unavailable".to_string())?;
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut sink = tokio::io::sink();
                let _ = tokio::io::copy(&mut stderr, &mut sink).await;
            });
        }
        self.transport = Transport::Stdio(StdioTransport {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        });
        Ok(())
    }

    async fn stdio_request(
        &mut self,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.ensure_process().await?;
        let Transport::Stdio(transport) = &mut self.transport else {
            return Err("MCP stdio process is unavailable".into());
        };
        let payload = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut encoded = serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
        encoded.push(b'\n');
        transport
            .stdin
            .write_all(&encoded)
            .await
            .map_err(|error| error.to_string())?;
        transport
            .stdin
            .flush()
            .await
            .map_err(|error| error.to_string())?;
        let response = tokio::time::timeout(REQUEST_TIMEOUT, async {
            let mut line = String::new();
            loop {
                line.clear();
                let bytes = transport
                    .stdout
                    .read_line(&mut line)
                    .await
                    .map_err(|error| error.to_string())?;
                if bytes == 0 {
                    return Err("MCP server closed stdout".to_string());
                }
                let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                    continue;
                };
                if message.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = message.get("error") {
                    return Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("MCP request failed")
                        .to_string());
                }
                return Ok(message.get("result").cloned().unwrap_or_else(|| json!({})));
            }
        })
        .await
        .map_err(|_| format!("MCP request timed out: {method}"))??;
        Ok(response)
    }

    async fn http_client(&mut self) -> Result<&reqwest::Client, String> {
        if !matches!(self.transport, Transport::Http(_)) {
            self.transport = Transport::Http(
                reqwest::Client::builder()
                    .timeout(REQUEST_TIMEOUT)
                    .build()
                    .map_err(|error| error.to_string())?,
            );
        }
        let Transport::Http(client) = &self.transport else {
            unreachable!();
        };
        Ok(client)
    }

    async fn http_request(
        &mut self,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let response = self
            .http_exchange(
                json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
            )
            .await?;
        Ok(response.get("result").cloned().unwrap_or_else(|| json!({})))
    }

    async fn http_exchange(&mut self, payload: Value) -> Result<Value, String> {
        let url = self
            .server
            .url
            .clone()
            .ok_or_else(|| "MCP HTTP server is missing URL".to_string())?;
        let server_headers = self.server.headers.clone();
        let env_headers = self.server.env_headers.clone();
        let bearer_env = self.server.bearer_token_env_var.clone();
        let session_id = self.session_id.clone();
        let client = self.http_client().await?.clone();
        let mut request = client
            .post(url)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json");
        for (key, value) in server_headers {
            let name = HeaderName::from_bytes(key.as_bytes()).map_err(|error| error.to_string())?;
            let value = HeaderValue::from_str(&value).map_err(|error| error.to_string())?;
            request = request.header(name, value);
        }
        for (key, env_name) in env_headers {
            if let Ok(value) = std::env::var(&env_name) {
                let name =
                    HeaderName::from_bytes(key.as_bytes()).map_err(|error| error.to_string())?;
                let value = HeaderValue::from_str(&value).map_err(|error| error.to_string())?;
                request = request.header(name, value);
            }
        }
        if let Some(env_name) = bearer_env {
            if let Ok(token) = std::env::var(env_name) {
                if !token.is_empty() {
                    request = request.bearer_auth(token);
                }
            }
        }
        if let Some(session_id) = session_id {
            request = request.header("mcp-session-id", session_id);
        }
        let response = request
            .json(&payload)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            self.session_id = Some(session.to_string());
        }
        let status = response.status();
        if !status.is_success() {
            return Err(format!("MCP HTTP {}", status.as_u16()));
        }
        if status.as_u16() == 202 {
            return Ok(json!({}));
        }
        let is_sse = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.contains("text/event-stream"))
            .unwrap_or(false);
        let text = response.text().await.map_err(|error| error.to_string())?;
        let message = if is_sse {
            text.lines()
                .filter_map(|line| line.strip_prefix("data:").map(str::trim))
                .filter(|line| !line.is_empty())
                .last()
                .map(serde_json::from_str::<Value>)
                .transpose()
                .map_err(|error| error.to_string())?
                .unwrap_or_else(|| json!({}))
        } else if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str::<Value>(&text).map_err(|error| error.to_string())?
        };
        if let Some(error) = message.get("error") {
            return Err(error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("MCP request failed")
                .to_string());
        }
        Ok(message)
    }

    pub async fn close(&mut self) {
        self.initialized = false;
        self.tools.clear();
        self.session_id = None;
        let transport = std::mem::replace(&mut self.transport, Transport::None);
        if let Transport::Stdio(mut stdio) = transport {
            let _ = stdio.child.kill().await;
            let _ = tokio::time::timeout(Duration::from_secs(1), stdio.child.wait()).await;
        }
    }
}

async fn enabled_descriptors(workspace_id: &str) -> Result<Vec<McpServerDescriptor>, String> {
    let runtime = runtime(workspace_id)
        .ok_or_else(|| "Workspace feature runtime is not active.".to_string())?;
    let config = runtime.config();
    if !config.extensions.mcp.active {
        return Ok(Vec::new());
    }
    let selected = config
        .extensions
        .mcp
        .enabled
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let discovered = discover_extensions(&runtime.folders).await;
    Ok(discovered
        .mcp_servers
        .into_iter()
        .filter(|server| {
            selected.contains(&server.key) && server.supported && server.source_enabled
        })
        .collect())
}

pub async fn list_external_tools(workspace_id: &str) -> Vec<ExternalTool> {
    let Some(runtime) = runtime(workspace_id) else {
        return Vec::new();
    };
    let descriptors = match enabled_descriptors(workspace_id).await {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let enabled_keys = descriptors
        .iter()
        .map(|server| server.key.clone())
        .collect::<HashSet<_>>();
    let mut connections = runtime.connections.lock().await;
    let stale = connections
        .keys()
        .filter(|key| !enabled_keys.contains(*key))
        .cloned()
        .collect::<Vec<_>>();
    for key in stale {
        if let Some(mut connection) = connections.remove(&key) {
            connection.close().await;
        }
    }
    let mut tools = Vec::new();
    for server in descriptors {
        let root = workspace_root(&server, &runtime.folders);
        let connection = connections
            .entry(server.key.clone())
            .or_insert_with(|| ExternalMcpConnection::new(server.clone(), root));
        match connection.refresh_tools().await {
            Ok(found) => tools.extend(found),
            Err(_) => {}
        }
    }
    tools
}

/// Connects every enabled server of a running workspace so the settings page shows
/// real status (connected / tool count / error) instead of waiting for the first
/// MCP client `tools/list`. Connections are cached, so already-connected servers
/// return immediately. Returns `false` when the workspace runtime is not running.
pub async fn connect_enabled(workspace_id: &str, limit: Duration) -> bool {
    if runtime(workspace_id).is_none() {
        return false;
    }
    let _ = tokio::time::timeout(limit, list_external_tools(workspace_id)).await;
    true
}

pub async fn call_external_tool(
    workspace_id: &str,
    tool: &ExternalTool,
    args: Value,
) -> Result<Value, String> {
    let runtime = runtime(workspace_id)
        .ok_or_else(|| "Workspace feature runtime is not active.".to_string())?;
    let mut connections = runtime.connections.lock().await;
    let connection = connections
        .get_mut(&tool.server_key)
        .ok_or_else(|| "External MCP server is not connected.".to_string())?;
    connection.call(&tool.tool_name, args).await
}

pub async fn status(workspace_id: &str, server_key: &str) -> (bool, usize, Option<String>) {
    let Some(runtime) = runtime(workspace_id) else {
        return (false, 0, None);
    };
    let connections = runtime.connections.lock().await;
    let Some(connection) = connections.get(server_key) else {
        return (false, 0, None);
    };
    (
        connection.connected(),
        connection.tool_count(),
        connection.error().map(str::to_string),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn proxy_tool_names_match_node_shape() {
        let server = McpServerDescriptor {
            key: "k".into(),
            provider: "claude".into(),
            scope: "workspace".into(),
            folder_id: "folder".into(),
            name: "my server".into(),
            transport: "stdio".into(),
            command: Some("x".into()),
            args: vec![],
            env: HashMap::new(),
            env_vars: vec![],
            cwd: None,
            url: None,
            headers: HashMap::new(),
            env_headers: HashMap::new(),
            bearer_token_env_var: None,
            source_path: ".mcp.json".into(),
            source_enabled: true,
            supported: true,
        };
        assert_eq!(
            proxy_tool_name(&server, "read file"),
            "mcp__claude__folder__my_server__read_file"
        );
    }
}
