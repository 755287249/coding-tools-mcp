//! Read-only health snapshot for the single MCP connection.
use crate::app_state::AppState;
use crate::error::AppResult;
use serde::Serialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionHealth {
    pub state: String,
    pub observed_at: u64,
    pub service: String,
    pub entry: String,
    pub detail: String,
    pub endpoint: String,
}

async fn probe(endpoint: &str, local: bool) -> Result<(), String> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .redirect(reqwest::redirect::Policy::none());
    if local {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|e| e.to_string())?;
    let response = client
        .get(format!("{}/info", endpoint.trim_end_matches('/')))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let value: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    if value["name"] != "coding-tools-mcp" || !value["protocolVersion"].is_string() {
        return Err("MCP discovery response does not match".into());
    }
    Ok(())
}

pub async fn health(state: &AppState) -> AppResult<ConnectionHealth> {
    let profile = state.with_data(|store| store.global_connection())?;
    let runtime = super::runtime::mcp_runtime_status(state, &profile.id)?;
    let local_only = matches!(profile.tunnel.tunnel_type.as_str(), "" | "none");
    let mut snapshot = ConnectionHealth {
        state: "offline".into(),
        observed_at: 0,
        service: runtime.state.clone(),
        entry: if local_only { "local" } else { "stopped" }.into(),
        detail: runtime.local_message,
        endpoint: if local_only {
            profile.local_endpoint()
        } else {
            profile.public_endpoint()
        },
    };
    match runtime.state.as_str() {
        "starting" | "stopping" => snapshot.state = "connecting".into(),
        "error" => snapshot.state = "error".into(),
        "running" => {
            let local = profile.local_endpoint();
            let local_probe = probe(&local, true);
            let public_probe = async {
                if local_only {
                    Ok(())
                } else {
                    probe(&snapshot.endpoint, false).await
                }
            };
            let (service, entry) = tokio::join!(local_probe, public_probe);
            snapshot.service = if service.is_ok() { "running" } else { "error" }.into();
            snapshot.entry = if local_only {
                "local"
            } else if entry.is_ok() {
                "online"
            } else {
                "error"
            }
            .into();
            snapshot.state = if service.is_ok() && entry.is_ok() {
                "online"
            } else {
                "error"
            }
            .into();
            snapshot.detail = match (service, entry) {
                (Err(e), _) => format!("MCP: {e}"),
                (_, Err(e)) => format!("Entry: {e}"),
                _ => String::new(),
            };
        }
        _ => {}
    }
    snapshot.observed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn discovery_probe_requires_mcp_json_and_never_initializes_sessions() {
        use axum::{routing::get, Json, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener,Router::new().route("/mcp/info",get(||async{Json(serde_json::json!({"name":"coding-tools-mcp","protocolVersion":"2025-11-25"}))})).route("/other/info",get(||async{"not mcp"}))).await.unwrap()
        });
        assert!(probe(&endpoint, true).await.is_ok());
        assert!(probe(&endpoint.replace("/mcp", "/other"), true)
            .await
            .is_err());
        server.abort();
    }
}
