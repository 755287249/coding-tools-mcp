use super::*;
use crate::tools::chat::compat;
use axum::extract::OriginalUri;
use axum::http::Method;

fn response(status: StatusCode, body: Value, key: &str) -> Response {
    let mut text = serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into());
    if !key.is_empty() {
        text = text.replace(key, "[redacted]");
    }
    (
        status,
        [
            (CONTENT_TYPE, "application/json; charset=utf-8"),
            (CACHE_CONTROL, "private, no-store, max-age=0"),
            (axum::http::header::REFERRER_POLICY, "no-referrer"),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        text,
    )
        .into_response()
}
fn invalid(message: &str) -> String {
    message.into()
}
fn reply_data(raw: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(raw).map_err(|_| invalid("Invalid reply data"))?;
    let o = v.as_object().ok_or_else(|| invalid("Invalid reply data"))?;
    let identifier = |s: &Value| {
        s.as_str().is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 80
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
    };
    if o.keys().any(|k| {
        !["message_id", "reply_to", "text", "final", "awaiting_user"].contains(&k.as_str())
    }) || !identifier(&v["message_id"])
        || !identifier(&v["reply_to"])
        || !v["text"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty() && s.len() <= 2000)
        || !v["final"].is_boolean()
        || o.get("awaiting_user").is_some_and(|b| !b.is_boolean())
    {
        return Err(invalid("Invalid reply data"));
    }
    Ok(v)
}
pub(super) async fn handle(
    State(state): State<ListenerState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if method != Method::GET {
        let mut r = response(
            StatusCode::METHOD_NOT_ALLOWED,
            json!({"ok":false,"error":"GET required"}),
            "",
        );
        r.headers_mut()
            .insert(ALLOW, HeaderValue::from_static("GET"));
        return r;
    }
    if uri.to_string().len() > 16384 {
        return response(
            StatusCode::BAD_REQUEST,
            json!({"ok":false,"error":"Request too large"}),
            "",
        );
    }
    let pairs: Vec<(String, String)> =
        reqwest::Url::parse(&format!("http://localhost/?{}", uri.query().unwrap_or("")))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
    let key = pairs
        .iter()
        .find(|(k, _)| k == "key")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let result = run(&state, &pairs, key).await;
    match result {
        Ok(v) => response(StatusCode::OK, v, key),
        Err(e) => response(
            if e == "Request already running" {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            },
            json!({"ok":false,"error":e}),
            key,
        ),
    }
}
async fn run(
    state: &ListenerState,
    pairs: &[(String, String)],
    key: &str,
) -> Result<Value, String> {
    let q = |name: &str| {
        pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let op = q("op").unwrap_or("");
    let mut allowed = vec!["key", "op", "nonce"];
    if op == "reply" {
        allowed.push("data");
    }
    if op == "wait" {
        allowed.push("timeout_ms");
    }
    if !["info", "open", "wait", "reply"].contains(&op)
        || pairs.iter().any(|(k, _)| {
            !allowed.contains(&k.as_str())
                || pairs.iter().filter(|(other, _)| other == k).count() != 1
        })
        || !q("nonce").is_some_and(|s| !s.is_empty() && s.len() <= 80)
        || key.len() != 64
        || !key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("Invalid compatibility request"));
    }
    let safe_error = |e: crate::tools::workspace::WorkspaceError| {
        let message = e.message();
        if [
            "Compatibility ",
            "GET trial ",
            "Conversation ",
            "Message ID conflicts",
            "Reply must",
            "Chat attachment",
        ]
        .iter()
        .any(|p| message.starts_with(p))
        {
            message
        } else {
            "Compatibility operation failed; inspect the conversation in the client".into()
        }
    };
    let g = compat::get(key, &state.workspace_id).map_err(safe_error)?;
    let listing = crate::tools::hub::list_workspace_folders(&state.mcp, None);
    let folder = listing["folders"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| {
            f["id"] == g.folder
                && f["path"]
                    .as_str()
                    .and_then(|p| std::fs::canonicalize(p).ok())
                    .as_ref()
                    == Some(&g.root)
        })
        .ok_or_else(|| invalid("Configured folder changed"))?;
    compat::check_policy(&state.mcp).map_err(safe_error)?;
    let _busy = g
        .busy
        .try_lock()
        .map_err(|_| invalid("Request already running"))?;
    let mut result = match op {
        "info" => compat::call(&g, "compat_info", json!({})).map_err(safe_error)?,
        "open" => {
            compat::call(&g, "chat_open", json!({"agent_name":"GetChat"})).map_err(safe_error)?;
            json!({"ok":true,"status":"connected","instruction_lines":compat::SKILL.replace("\r\n","\n").split('\n').collect::<Vec<_>>()})
        }
        "reply" => {
            if g.attachment.lock().unwrap().is_none() {
                return Err(invalid("Call open first"));
            }
            let data = reply_data(q("data").unwrap_or(""))?;
            if data.to_string().contains(key) {
                return Err(invalid("Do not put authorization in replies"));
            }
            compat::call(&g, "chat_reply", data).map_err(safe_error)?
        }
        _ => {
            if g.attachment.lock().unwrap().is_none() {
                return Err(invalid("Call open first"));
            }
            let raw = q("timeout_ms").unwrap_or("10000");
            let timeout = raw
                .parse::<u64>()
                .ok()
                .filter(|n| *n <= 10000)
                .filter(|_| {
                    !raw.is_empty() && raw.len() <= 5 && raw.bytes().all(|b| b.is_ascii_digit())
                })
                .ok_or_else(|| invalid("timeout_ms must be 0..10000"))?;
            let deadline = Instant::now() + Duration::from_millis(timeout);
            let r = loop {
                compat::check_policy(&state.mcp).map_err(safe_error)?;
                let r = compat::call(&g, "chat_wait", json!({})).map_err(safe_error)?;
                if r["status"] != "idle" || Instant::now() >= deadline {
                    break r;
                }
                tokio::time::sleep(
                    Duration::from_millis(250)
                        .min(deadline.saturating_duration_since(Instant::now())),
                )
                .await;
            };
            let mut out = json!({"ok":true,"status":r["status"]});
            if r["message"].is_object() {
                let m = &r["message"];
                let chars = m["text"].as_str().unwrap_or("").chars().collect::<Vec<_>>();
                let lines = chars
                    .chunks(120)
                    .map(|c| c.iter().collect::<String>())
                    .collect::<Vec<_>>();
                out["message"] = json!({"id":m["id"],"kind":m["kind"],"text_lines":lines,"attachments":m["attachments"].as_array().cloned().unwrap_or_default()});
            }
            out
        }
    };
    result["protocol"] = json!("chat-get-v1");
    result["chat_id"] = json!(g.chat);
    result["workspace_folder"] =
        json!({"id":folder["id"],"name":folder["name"],"path":folder["path"]});
    result["expires_at"] = json!(g.expires);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{chat, ToolContext};
    #[tokio::test]
    async fn get_compat_http_contract() {
        let dir = tempfile::tempdir().unwrap();
        let harness = tempfile::tempdir().unwrap();
        let mcp =
            Arc::new(ToolContext::for_test(dir.path().into(), harness.path().into()).unwrap());
        let state = ListenerState {
            mcp,
            auth: AuthConfig::default(),
            workspace_id: "http-compat".into(),
            bind_address: "127.0.0.1".into(),
            bind_port: 0,
            configured_public_url: "https://test.invalid/prefix".into(),
            bearer_token: None,
            oauth: None,
            oauth_client_secret: None,
            transport_mode: "streamable-http".into(),
            redact_telemetry: true,
        };
        let chat =
            chat::ui(dir.path(), &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let folder = crate::tools::hub::list_workspace_folders(&state.mcp, None)["folders"][0]
            ["id"]
            .as_str()
            .unwrap()
            .to_string();
        let grant = compat::management(
            dir.path(),
            "http-compat",
            &folder,
            &json!({"action":"prepare_compat","chat_id":chat,"message_id":"attempt"}),
        )
        .unwrap();
        let key = grant["compat"]["key"].as_str().unwrap();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, super::super::routes::build_router(state)).await;
        });
        let client = reqwest::Client::new();
        let base = format!("http://{address}/prefix/mcp/chat-compat");
        let url = |op: &str| {
            let mut u = reqwest::Url::parse(&base).unwrap();
            u.query_pairs_mut()
                .append_pair("key", key)
                .append_pair("nonce", &uuid::Uuid::new_v4().to_string())
                .append_pair("op", op);
            u
        };
        assert_eq!(client.head(url("open")).send().await.unwrap().status(), 405);
        let read = chat::ui(dir.path(), &json!({"action":"read","chat_id":chat})).unwrap();
        assert_eq!(read["session"]["status"], "offline");
        for op in ["exec_command", "tools/call", "revoke"] {
            assert_eq!(client.get(url(op)).send().await.unwrap().status(), 400);
        }
        let mut bad = url("open");
        bad.query_pairs_mut().append_pair("chat_id", "other");
        assert_eq!(client.get(bad).send().await.unwrap().status(), 400);
        let mut duplicate = url("open");
        duplicate.query_pairs_mut().append_pair("op", "reply");
        assert_eq!(client.get(duplicate).send().await.unwrap().status(), 400);
        let info = client.get(url("info")).send().await.unwrap();
        assert_eq!(info.status(), 200);
        assert_eq!(
            info.headers()["cache-control"],
            "private, no-store, max-age=0"
        );
        let info = info.json::<Value>().await.unwrap();
        assert_eq!(info["chat_id"], chat);
        assert_eq!(info["workspace_folder"]["id"], folder);
        let open = client
            .get(url("open"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(!open.contains(key));
        let parsed: Value = serde_json::from_str(&open).unwrap();
        assert_eq!(parsed["status"], "connected");
        assert_eq!(
            parsed["instruction_lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect::<Vec<_>>()
                .join("\n"),
            compat::SKILL
        );
        assert_eq!(client.get(url("open")).send().await.unwrap().status(), 200);
        chat::ui(
            dir.path(),
            &json!({"action":"request_connection","chat_id":chat,"message_id":"greeting"}),
        )
        .unwrap();
        let mut wait = url("wait");
        wait.query_pairs_mut().append_pair("timeout_ms", "0");
        let received = client
            .get(wait.clone())
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(received["status"], "message");
        let data = json!({"message_id":"answer","reply_to":received["message"]["id"],"text":"你好，有什么能帮到你？","final":true});
        let mut reply = url("reply");
        reply
            .query_pairs_mut()
            .append_pair("data", &data.to_string());
        for _ in 0..2 {
            assert_eq!(
                client
                    .get(reply.clone())
                    .send()
                    .await
                    .unwrap()
                    .json::<Value>()
                    .await
                    .unwrap()["persisted"],
                true
            );
        }
        assert_eq!(
            client
                .get(wait)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()["status"],
            "idle"
        );
        let mut waiting = url("wait");
        waiting.query_pairs_mut().append_pair("timeout_ms", "2000");
        let pending = tokio::spawn({
            let client = client.clone();
            async move { client.get(waiting).send().await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(client.get(url("wait")).send().await.unwrap().status(), 409);
        compat::management(
            dir.path(),
            "http-compat",
            &folder,
            &json!({"action":"revoke_compat","chat_id":chat}),
        )
        .unwrap();
        assert_eq!(pending.await.unwrap().status(), 400);
        assert_eq!(client.get(url("open")).send().await.unwrap().status(), 400);
        assert!(!std::fs::read_to_string(dir.path().join(format!(
            "docs/chat-sessions/{}.json",
            chat.as_str().unwrap()
        )))
        .unwrap()
        .contains(key));
        server.abort();
    }
    #[test]
    fn replies_reject_extra_scope_and_bound_utf8() {
        assert!(
            reply_data(r#"{"message_id":"a","reply_to":"b","text":"hi","final":true}"#).is_ok()
        );
        for extra in [
            json!({"chat_id":"other"}),
            json!({"tool":"exec_command"}),
            json!({"final":"true"}),
            json!({"text":"中".repeat(667)}),
        ] {
            let mut data = json!({"message_id":"a","reply_to":"b","text":"hi","final":true});
            data.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(reply_data(&data.to_string()).is_err());
        }
    }
}
