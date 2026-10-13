//! Per-chat authorization mode chosen in the composer (完全/一般/严格授权).
//! Mirrors `packages/node-agent/src/chat/access.ts`.
//! - full:   operations inside the project are approved automatically;
//! - normal: default; the project's guarded approval flow is unchanged;
//! - strict: every tool that is not read-only needs the user's approval in the
//!   app (approval card) before it runs.
//!
//! Enforced by the seed gateway, which both [接入AI] URLs and seeds use.
use super::{err, load, lock, now, save, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

pub(crate) const MODES: [&str; 3] = ["full", "normal", "strict"];
const APPROVAL_TTL_MS: u64 = 30 * 60_000;
const MAX_APPROVALS: usize = 20;
const IGNORED: [&str; 6] = [
    "approval_id",
    "confirm",
    "chat_id",
    "attachment_id",
    "workspace_folder_id",
    "operation_id",
];

pub(crate) fn mode(s: &Value) -> &'static str {
    match s["access_mode"].as_str() {
        Some("full") => "full",
        Some("strict") => "strict",
        _ => "normal",
    }
}
pub(super) fn set_mode(s: &mut Value, mode: &Value) -> Result<()> {
    let Some(mode) = mode.as_str().filter(|m| MODES.contains(m)) else {
        return Err(err("Invalid access mode"));
    };
    s["access_mode"] = json!(mode);
    Ok(())
}
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical(&map[k])))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}
pub(crate) fn approval_hash(tool: &str, args: &Value) -> String {
    let mut kept = serde_json::Map::new();
    if let Some(map) = args.as_object() {
        for (key, value) in map {
            if !IGNORED.contains(&key.as_str()) {
                kept.insert(key.clone(), value.clone());
            }
        }
    }
    let digest = Sha256::digest(format!("{tool}\0{}", canonical(&Value::Object(kept))).as_bytes());
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}
fn summary(tool: &str, args: &Value) -> String {
    let mut lines = vec![tool.to_owned()];
    for key in [
        "path",
        "paths",
        "command",
        "cmd",
        "target",
        "source",
        "destination",
        "operation",
        "pattern",
    ] {
        let value = &args[key];
        if !value.is_null() {
            let text = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            lines.push(format!("{key}: {text}"));
        }
    }
    lines.join("\n").chars().take(600).collect()
}
fn fresh(approval: &Value, at: u64) -> bool {
    at.saturating_sub(approval["created_at"].as_u64().unwrap_or(0)) < APPROVAL_TTL_MS
}
fn prune(s: &mut Value, at: u64) {
    let mut list: Vec<Value> = s["approvals"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|a| fresh(a, at))
        .collect();
    if list.len() > MAX_APPROVALS {
        list.drain(..list.len() - MAX_APPROVALS);
    }
    s["approvals"] = Value::Array(list);
}
/// Returns the approval for this exact call, creating a pending one when needed.
fn request(s: &mut Value, tool: &str, args: &Value, at: u64) -> Value {
    prune(s, at);
    let hash = approval_hash(tool, args);
    let wanted = args["approval_id"].as_str().unwrap_or("");
    let existing = s["approvals"].as_array().and_then(|list| {
        list.iter()
            .rev()
            .find(|a| {
                (wanted.is_empty() || a["id"] == wanted)
                    && a["args_hash"] == hash.as_str()
                    && a["tool"] == tool
            })
            .cloned()
    });
    if let Some(found) = existing {
        return found;
    }
    let created = json!({
        "id": uuid::Uuid::new_v4().simple().to_string()[..16].to_owned(),
        "tool": tool,
        "summary": summary(tool, args),
        "args_hash": hash,
        "created_at": at,
        "status": "pending"
    });
    if let Some(list) = s["approvals"].as_array_mut() {
        list.push(created.clone());
    }
    prune(s, at);
    created
}
/// An approval is single use: consuming it removes it.
fn consume(s: &mut Value, id: &str) {
    if let Some(list) = s["approvals"].as_array_mut() {
        list.retain(|a| a["id"] != id);
    }
}
pub(super) fn resolve(s: &mut Value, args: &Value) -> Result<()> {
    let Some(approve) = args["approve"].as_bool() else {
        return Err(err("approve must be a boolean"));
    };
    let at = now();
    prune(s, at);
    let id = args["approval_id"].as_str().unwrap_or("");
    let Some(found) = s["approvals"]
        .as_array_mut()
        .and_then(|list| list.iter_mut().find(|a| a["id"] == id))
    else {
        return Err(err("Approval request not found or expired"));
    };
    if found["status"] == "pending" {
        found["status"] = json!(if approve { "approved" } else { "denied" });
        found["resolved_at"] = json!(at);
    }
    Ok(())
}
pub(super) fn pending(s: &Value) -> Value {
    let at = now();
    Value::Array(
        s["approvals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["status"] == "pending" && fresh(a, at))
            .cloned()
            .collect(),
    )
}
pub(crate) enum Gate {
    Run(Value),
    Blocked(Value),
    Wait(String),
}
/// Applies the chat's authorization mode to one project tool call.
pub(crate) fn gate(
    root: &Path,
    chat_id: &str,
    tool: &str,
    args: &Value,
    read_only: bool,
    accepts_confirm: bool,
) -> Result<Gate> {
    let _lock = lock(root)?;
    let mut next = args.clone();
    if let Some(map) = next.as_object_mut() {
        map.remove("approval_id");
    }
    // An unreadable chat keeps the project's normal policy.
    let Ok(mut s) = load(root, chat_id) else {
        return Ok(Gate::Run(next));
    };
    let mode = mode(&s);
    if mode == "full" {
        if accepts_confirm {
            next["confirm"] = json!(true);
        }
        return Ok(Gate::Run(next));
    }
    if mode == "normal" || read_only {
        return Ok(Gate::Run(next));
    }
    let approval = request(&mut s, tool, args, now());
    let id = approval["id"].as_str().unwrap_or("").to_owned();
    match approval["status"].as_str() {
        Some("approved") => {
            consume(&mut s, &id);
            save(root, &s)?;
            if accepts_confirm {
                next["confirm"] = json!(true);
            }
            Ok(Gate::Run(next))
        }
        Some("denied") => {
            consume(&mut s, &id);
            save(root, &s)?;
            Ok(Gate::Blocked(json!({
                "ok": false,
                "approval_id": id,
                "error": {
                    "code": "APPROVAL_DENIED",
                    "category": "permission",
                    "retryable": false,
                    "message": "用户拒绝了这个操作（严格授权）。不要重试同一操作；用 chat_reply 说明情况或询问用户下一步。The user denied this operation."
                }
            })))
        }
        _ => {
            save(root, &s)?;
            Ok(Gate::Wait(id))
        }
    }
}
/// Re-reads one approval while the gateway waits for the user's decision.
pub(crate) fn status(root: &Path, chat_id: &str, id: &str) -> String {
    load(root, chat_id)
        .ok()
        .and_then(|s| {
            s["approvals"]
                .as_array()
                .and_then(|list| list.iter().find(|a| a["id"] == id))
                .and_then(|a| a["status"].as_str().map(str::to_owned))
        })
        .unwrap_or_else(|| "missing".into())
}
pub(crate) fn required_result(tool: &str, id: &str) -> Value {
    json!({
        "ok": false,
        "approval_id": id,
        "error": {
            "code": "APPROVAL_REQUIRED",
            "category": "permission",
            "retryable": true,
            "message": format!("严格授权：这个操作需要用户在应用里批准，批准卡片已显示给用户。请用相同参数并加上 approval_id=\"{id}\" 再次调用 {tool}（每次会等待约 20 秒）；也可以先 chat_reply(final=false) 告诉用户正在等待批准。Strict mode: the user must approve this in the app. Retry the same call with approval_id.")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approvals_are_bound_to_the_exact_call_and_single_use() {
        // resolve() checks expiry against the real clock.
        let t0 = now();
        let mut s = json!({"id":"c"});
        assert_eq!(mode(&s), "normal");
        assert!(set_mode(&mut s, &json!("root")).is_err());
        set_mode(&mut s, &json!("strict")).unwrap();
        assert_eq!(mode(&s), "strict");
        let args = json!({"path":"a.txt","content":"x"});
        let first = request(&mut s, "write_file", &args, t0);
        let again = request(&mut s, "write_file", &args, t0 + 1);
        assert_eq!(first["id"], again["id"]);
        let other = request(&mut s, "write_file", &json!({"path":"b.txt"}), t0 + 2);
        assert_ne!(first["id"], other["id"]);
        // Ignored keys do not change the identity of a call.
        assert_eq!(
            approval_hash("t", &json!({"a":1,"b":[1,{"y":2,"x":1}],"confirm":true})),
            approval_hash("t", &json!({"b":[1,{"x":1,"y":2}],"a":1}))
        );
        let id = first["id"].as_str().unwrap().to_owned();
        resolve(&mut s, &json!({"approval_id":id,"approve":true})).unwrap();
        assert!(resolve(&mut s, &json!({"approval_id":"missing","approve":true})).is_err());
        consume(&mut s, &id);
        assert!(s["approvals"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["id"] != id.as_str()));
        prune(&mut s, t0 + 2 + APPROVAL_TTL_MS + 1);
        assert!(s["approvals"].as_array().unwrap().is_empty());
    }
}
