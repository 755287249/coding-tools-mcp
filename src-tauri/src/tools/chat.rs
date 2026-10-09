//! Local chat v1. JSON is authoritative; Markdown is a rebuildable projection.
use super::workspace::WorkspaceError;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

const DIR: &str = "docs/chat-sessions";
const MAX_BYTES: u64 = 2 * 1024 * 1024;
const LEASE_MS: u64 = 600_000;
static WAITERS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
type Result<T> = std::result::Result<T, WorkspaceError>;
fn err(message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::invalid_argument(message)
}
fn io(e: std::io::Error) -> WorkspaceError {
    err(format!("Chat storage: {e}"))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn id(v: &Value) -> Result<&str> {
    let s = v.as_str().unwrap_or("");
    if s.is_empty()
        || s.len() > 80
        || !s
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(err("Invalid chat/message ID"));
    }
    Ok(s)
}
fn text(v: &Value, max: usize) -> Result<String> {
    let s = v.as_str().unwrap_or("");
    if s.trim().is_empty() || s.len() > max {
        return Err(err(format!("Text must contain 1–{max} bytes")));
    }
    Ok(super::redaction::redact_sensitive_text(s.trim()).0)
}
fn safe(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut p = root.canonicalize().map_err(io)?;
    for part in relative.split('/') {
        p.push(part);
        match fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(err("Chat storage must not contain symlinks"))
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(io(e)),
        }
    }
    Ok(p)
}
fn file(root: &Path, chat_id: &str) -> Result<PathBuf> {
    safe(root, &format!("{DIR}/{chat_id}.json"))
}
fn load(root: &Path, chat_id: &str) -> Result<Value> {
    let p = file(root, chat_id)?;
    if fs::metadata(&p).map_err(io)?.len() > MAX_BYTES {
        return Err(err("Chat archive exceeds size limit"));
    }
    let s: Value =
        serde_json::from_slice(&fs::read(p).map_err(io)?).map_err(|e| err(e.to_string()))?;
    if s["version"] != 1 || s["id"] != chat_id || !s["messages"].is_array() {
        return Err(err("Invalid chat archive"));
    }
    Ok(s)
}
pub fn markdown(s: &Value) -> String {
    let mut out = format!(
        "# {}\n\nSession: {}\n\n",
        s["title"].as_str().unwrap_or(""),
        s["id"].as_str().unwrap_or("")
    );
    if let Some(messages) = s["messages"].as_array() {
        for m in messages {
            let role = if m["role"] == "user" {
                "你"
            } else if m["final"] == false {
                "AI · 进度"
            } else {
                "AI"
            };
            out.push_str(&format!(
                "## {role} · {}\n\n{}\n\n",
                m["created_at"],
                m["text"].as_str().unwrap_or("")
            ));
        }
    }
    out
}
fn atomic(root: &Path, chat_id: &str, extension: &str, bytes: &[u8]) -> Result<()> {
    let temporary = safe(
        root,
        &format!("{DIR}/{chat_id}.{}.tmp", uuid::Uuid::new_v4()),
    )?;
    let target = safe(root, &format!("{DIR}/{chat_id}.{extension}"))?;
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut f = options.open(&temporary).map_err(io)?;
        f.write_all(bytes).map_err(io)?;
        f.sync_all().map_err(io)?;
        drop(f);
        fs::rename(&temporary, target).map_err(io)
    })();
    let _ = fs::remove_file(temporary);
    result
}
fn save(root: &Path, s: &Value) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(s).map_err(|e| err(e.to_string()))?;
    if bytes.len() as u64 > MAX_BYTES || s["messages"].as_array().map_or(0, Vec::len) > 500 {
        return Err(err("Session is full; start a new conversation"));
    }
    let chat_id = id(&s["id"])?;
    atomic(root, chat_id, "json", &bytes)?;
    atomic(root, chat_id, "md", markdown(s).as_bytes())
}
struct DiskLock(PathBuf);
impl Drop for DiskLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}
fn lock(root: &Path) -> Result<DiskLock> {
    fs::create_dir_all(safe(root, DIR)?).map_err(io)?;
    let p = safe(root, &format!("{DIR}/.lock"))?;
    fs::create_dir(&p).map_err(io)?;
    Ok(DiskLock(p))
}
fn pending(s: &Value) -> Option<Value> {
    let messages = s["messages"].as_array()?;
    messages
        .iter()
        .find(|m| {
            m["role"] == "user"
                && !messages.iter().any(|r| {
                    r["role"] == "assistant" && r["reply_to"] == m["id"] && r["final"] == true
                })
        })
        .cloned()
}
fn view(root: &Path, s: &Value) -> Result<Value> {
    let waiting = WAITERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .contains(&file(root, id(&s["id"])?)?);
    let status = if s["closed"] == true {
        "closed"
    } else if waiting {
        "waiting"
    } else if s["lease_until"].as_u64().unwrap_or(0) > now() {
        "connected"
    } else {
        "offline"
    };
    let mut result = s.clone();
    let o = result
        .as_object_mut()
        .ok_or_else(|| err("Invalid chat archive"))?;
    o.remove("attachment_id");
    o.remove("lease_until");
    o.remove("version");
    o.insert("status".into(), json!(status));
    o.insert(
        "archive_path".into(),
        json!(format!("{DIR}/{}.md", id(&s["id"])?)),
    );
    Ok(result)
}
pub fn ui(root: &Path, args: &Value) -> Result<Value> {
    let _lock = lock(root)?;
    let action = args["action"].as_str().unwrap_or("");
    if action == "list" {
        let mut sessions = Vec::new();
        for entry in fs::read_dir(safe(root, DIR)?).map_err(io)? {
            let p = entry.map_err(io)?.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if id(&json!(stem)).is_err() {
                continue;
            }
            let mut v = view(root, &load(root, stem)?)?;
            v.as_object_mut().unwrap().remove("messages");
            sessions.push(v);
        }
        sessions.sort_by_key(|s| std::cmp::Reverse(s["updated_at"].as_u64().unwrap_or(0)));
        return Ok(json!({"sessions": sessions}));
    }
    if action == "create" {
        let title = text(args.get("title").unwrap_or(&json!("新对话")), 240)?;
        let s = json!({"version":1,"id":uuid::Uuid::new_v4().to_string(),"title":title,"created_at":now(),"updated_at":now(),"closed":false,"messages":[],"attachment_id":"","lease_until":0});
        save(root, &s)?;
        return Ok(json!({"session":view(root,&s)?}));
    }
    let mut s = load(root, id(&args["chat_id"])?)?;
    match action {
        "send" => {
            if s["closed"] == true {
                return Err(err("Conversation is closed"));
            }
            let message_id = id(&args["message_id"])?;
            let content = text(&args["text"], 32000)?;
            if let Some(m) = s["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["id"] == message_id)
            {
                if m["role"] != "user" || m["text"] != content {
                    return Err(err("Message ID conflicts with an existing message"));
                }
            } else {
                if s["messages"].as_array().unwrap().is_empty() {
                    s["title"] = json!(content
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(36)
                        .collect::<String>());
                }
                s["messages"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id":message_id,"role":"user","text":content,"created_at":now()}));
                s["updated_at"] = json!(now());
            }
            save(root, &s)?;
        }
        "close" => {
            s["closed"] = json!(true);
            s["attachment_id"] = json!("");
            s["lease_until"] = json!(0);
            s["updated_at"] = json!(now());
            save(root, &s)?;
        }
        "read" => (),
        _ => return Err(err("Unknown chat action")),
    }
    Ok(json!({"session":view(root,&s)?}))
}
fn owned(s: &Value, args: &Value) -> Result<()> {
    if args["attachment_id"].as_str().unwrap_or("").is_empty()
        || args["attachment_id"] != s["attachment_id"]
        || s["lease_until"].as_u64().unwrap_or(0) <= now()
    {
        return Err(err("Chat attachment expired; call chat_open again"));
    }
    Ok(())
}
pub fn tool(root: &Path, name: &str, args: &Value) -> Result<Value> {
    let _lock = lock(root)?;
    let mut s = load(root, id(&args["chat_id"])?)?;
    if s["closed"] == true {
        return Ok(json!({"ok":true,"status":"closed"}));
    }
    if name == "chat_open" {
        if s["lease_until"].as_u64().unwrap_or(0) > now()
            && args["attachment_id"] != s["attachment_id"]
        {
            return Err(err(
                "Conversation already attached; close it in the UI or wait for the lease to expire",
            ));
        }
        if s["lease_until"].as_u64().unwrap_or(0) <= now() {
            s["attachment_id"] = json!(uuid::Uuid::new_v4().to_string());
        }
        s["lease_until"] = json!(now() + LEASE_MS);
        save(root, &s)?;
        return Ok(
            json!({"ok":true,"attachment_id":s["attachment_id"],"session":view(root,&s)?,"instruction":"Call chat_wait. Publish progress and complete replies with chat_reply; final=true acknowledges reply_to. Then wait again until closed. Redelivered messages may have unfinished work: inspect before repeating side effects."}),
        );
    }
    owned(&s, args)?;
    match name {
        "chat_reply" => {
            let message_id = id(&args["message_id"])?;
            let reply_to = id(&args["reply_to"])?;
            let content = text(&args["text"], 32000)?;
            let final_reply = args["final"] != false;
            if let Some(m) = s["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["id"] == message_id)
            {
                if m["role"] != "assistant"
                    || m["text"] != content
                    || m["reply_to"] != reply_to
                    || m["final"] != final_reply
                {
                    return Err(err("Message ID conflicts with an existing reply"));
                }
            } else {
                if pending(&s).map(|m| m["id"].clone()) != Some(json!(reply_to)) {
                    return Err(err("Reply must address the oldest unanswered user message"));
                }
                s["messages"].as_array_mut().unwrap().push(json!({"id":message_id,"role":"assistant","text":content,"reply_to":reply_to,"final":final_reply,"created_at":now()}));
                s["updated_at"] = json!(now());
            }
            s["lease_until"] = json!(now() + LEASE_MS);
            save(root, &s)?;
            Ok(json!({"ok":true,"persisted":true,"message_id":message_id}))
        }
        "chat_close" => {
            s["closed"] = json!(true);
            s["attachment_id"] = json!("");
            s["lease_until"] = json!(0);
            save(root, &s)?;
            Ok(json!({"ok":true,"status":"closed"}))
        }
        "chat_wait" => {
            s["lease_until"] = json!(now() + LEASE_MS);
            save(root, &s)?;
            let message = pending(&s);
            Ok(
                json!({"ok":true,"status":if message.is_some(){"message"}else{"idle"},"message":message}),
            )
        }
        _ => Err(err("Unknown chat tool")),
    }
}
struct Waiting(PathBuf);
impl Drop for Waiting {
    fn drop(&mut self) {
        WAITERS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(&self.0);
    }
}
pub async fn wait(root: &Path, args: &Value) -> Result<Value> {
    let timeout = match args.get("timeout_ms") {
        Some(v) => v
            .as_u64()
            .filter(|n| *n <= 180000)
            .ok_or_else(|| err("timeout_ms must be 0–180000"))?,
        None => 120000,
    };
    let initial = tool(root, "chat_wait", args)?;
    if initial["status"] != "idle" {
        return Ok(initial);
    }
    let chat_id = id(&args["chat_id"])?;
    let path = file(root, chat_id)?;
    if !WAITERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(path.clone())
    {
        return Err(err("A wait request is already active"));
    }
    let _waiting = Waiting(path);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout);
    while tokio::time::Instant::now() < deadline {
        let s = load(root, chat_id)?;
        if s["closed"] == true {
            return Ok(json!({"ok":true,"status":"closed"}));
        }
        owned(&s, args)?;
        if let Some(message) = pending(&s) {
            return Ok(json!({"ok":true,"status":"message","message":message}));
        }
        tokio::time::sleep_until(std::cmp::min(
            deadline,
            tokio::time::Instant::now() + std::time::Duration::from_millis(250),
        ))
        .await;
    }
    Ok(
        json!({"ok":true,"status":"idle","instruction":"No message yet. Call chat_wait again unless the user ended the loop."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn durable_chat_loop_deduplicates_and_cancels_wait() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let created = ui(root, &json!({"action":"create","title":"Chat test"})).unwrap();
        let cid = created["session"]["id"].clone();
        let opened = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap();
        let mut args =
            json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"timeout_ms":0});
        let send = json!({"action":"send","chat_id":cid,"message_id":"u1","text":"hello"});
        ui(root, &send).unwrap();
        ui(root, &send).unwrap();
        assert_eq!(wait(root, &args).await.unwrap()["message"]["id"], "u1");
        assert_eq!(wait(root, &args).await.unwrap()["message"]["id"], "u1");
        args["message_id"] = json!("a1");
        args["reply_to"] = json!("u1");
        args["text"] = json!("password=secret123 done");
        tool(root, "chat_reply", &args).unwrap();
        tool(root, "chat_reply", &args).unwrap();
        assert_eq!(wait(root, &args).await.unwrap()["status"], "idle");
        let s = ui(root, &json!({"action":"read","chat_id":cid})).unwrap();
        assert_eq!(s["session"]["messages"].as_array().unwrap().len(), 2);
        assert!(s["session"]["attachment_id"].is_null());
        let md =
            fs::read_to_string(root.join(s["session"]["archive_path"].as_str().unwrap())).unwrap();
        assert!(!md.contains("secret123"));
        assert!(md.contains("hello"));
        args["timeout_ms"] = json!(1000);
        let timed =
            tokio::time::timeout(std::time::Duration::from_millis(10), wait(root, &args)).await;
        assert!(timed.is_err());
        assert_ne!(
            ui(root, &json!({"action":"read","chat_id":cid})).unwrap()["session"]["status"],
            "waiting"
        );
        ui(root, &json!({"action":"close","chat_id":cid})).unwrap();
        assert_eq!(wait(root, &args).await.unwrap()["status"], "closed");
    }
    #[test]
    fn prevents_cross_folder_access_and_attachment_theft() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let created = ui(a.path(), &json!({"action":"create"})).unwrap();
        let args = json!({"chat_id":created["session"]["id"]});
        tool(a.path(), "chat_open", &args).unwrap();
        assert!(tool(a.path(), "chat_open", &args).is_err());
        assert!(tool(a.path(), "chat_reply", &args).is_err());
        assert!(tool(b.path(), "chat_open", &args).is_err());
        assert!(ui(a.path(), &json!({"action":"read","chat_id":"../other"})).is_err());
    }
}
