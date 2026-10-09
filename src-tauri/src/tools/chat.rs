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
            if !m["tool_event"].is_null() {
                out.push_str(&format!(
                    "Tool (AI reported): {} · {}\n\n",
                    m["tool_event"]["name"].as_str().unwrap_or(""),
                    m["tool_event"]["status"].as_str().unwrap_or("")
                ));
            }
            out.push_str(&format!(
                "## {role} · {}\n\n{}\n\n",
                m["created_at"],
                m["text"].as_str().unwrap_or("")
            ));
            if let Some(files) = m["attachments"].as_array() {
                for f in files {
                    out.push_str(&format!(
                        "Attachment: {} ({} bytes)\nPath: {}\n\n",
                        f["name"].as_str().unwrap_or(""),
                        f["size"],
                        f["path"].as_str().unwrap_or("")
                    ));
                }
            }
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match fs::create_dir(&p) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::time::Instant::now() >= deadline {
                    return Err(err("Chat storage is busy; retry shortly. If it persists, stop all clients before inspecting docs/chat-sessions/.lock"));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(io(e)),
        }
    }
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
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
fn file_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[255, 216, 255]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}
fn file_path(s: &Value, f: &Value) -> Result<String> {
    let ext = match f["mime"].as_str().unwrap_or("") {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    };
    Ok(format!("{DIR}/{}-{}.{}", id(&s["id"])?, id(&f["id"])?, ext))
}
fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
fn upload(root: &Path, s: &mut Value, args: &Value) -> Result<Value> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    if s["closed"] == true {
        return Err(err("Conversation is closed"));
    }
    let upload_id = id(&args["upload_id"])?;
    let name = text(&args["name"], 240)?;
    if name
        .chars()
        .any(|c| c.is_ascii_control() || c == '/' || c == '\\')
    {
        return Err(err("Invalid attachment name"));
    }
    let encoded = args["data_base64"].as_str().unwrap_or("");
    if encoded.len() > 2796204 {
        return Err(err("Invalid attachment encoding or size"));
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| err("Invalid attachment encoding or size"))?;
    if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
        return Err(err("Attachment must contain 1–2097152 bytes"));
    }
    let sha256 = digest(&bytes);
    if s["files"].is_null() {
        s["files"] = json!([]);
    }
    let files = s["files"]
        .as_array()
        .ok_or_else(|| err("Invalid attachment manifest"))?;
    if let Some(existing) = files.iter().find(|f| f["id"] == upload_id) {
        if existing["sha256"] != sha256 || existing["name"] != name {
            return Err(err("Attachment ID conflict"));
        }
        save(root, s)?;
        return Ok(existing.clone());
    }
    if files.len() >= 32
        || files
            .iter()
            .map(|f| f["size"].as_u64().unwrap_or(0))
            .sum::<u64>()
            + bytes.len() as u64
            > 32 * 1024 * 1024
    {
        return Err(err("Session attachment limit reached"));
    }
    let mut f = json!({"id":upload_id,"name":name,"mime":file_mime(&bytes),"size":bytes.len(),"sha256":sha256});
    f["path"] = json!(file_path(s, &f)?);
    let target = safe(root, f["path"].as_str().unwrap())?;
    if target.exists() {
        if fs::metadata(&target).map_err(io)?.len() != bytes.len() as u64
            || fs::read(&target).map_err(io)? != bytes
        {
            return Err(err("Attachment ID conflict"));
        }
    } else {
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut dest = options.open(target).map_err(io)?;
        dest.write_all(&bytes).map_err(io)?;
        dest.sync_all().map_err(io)?;
    }
    s["files"].as_array_mut().unwrap().push(f.clone());
    save(root, s)?;
    Ok(f)
}
fn message_files(s: &Value, ids: Option<&Value>) -> Result<Vec<Value>> {
    let Some(ids) = ids else {
        return Ok(vec![]);
    };
    let ids = ids
        .as_array()
        .ok_or_else(|| err("Select up to 5 unique attachments"))?;
    if ids.len() > 5 {
        return Err(err("Select up to 5 unique attachments"));
    }
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for value in ids {
        let attachment_id = id(value)?;
        if !seen.insert(attachment_id) {
            return Err(err("Select up to 5 unique attachments"));
        }
        let f = s["files"]
            .as_array()
            .and_then(|files| files.iter().find(|f| f["id"] == attachment_id))
            .ok_or_else(|| err("Attachment does not belong to this conversation"))?;
        result.push(f.clone());
    }
    Ok(result)
}
fn tool_event(value: Option<&Value>, final_reply: bool) -> Result<Value> {
    let Some(value) = value else {
        return Ok(Value::Null);
    };
    if !value.is_object()
        || final_reply
        || !matches!(
            value["status"].as_str(),
            Some("running" | "completed" | "failed")
        )
    {
        return Err(err("Tool events require final=false and a valid status"));
    }
    Ok(json!({"name":text(&value["name"],120)?,"status":value["status"]}))
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
        "upload" => return Ok(json!({"attachment":upload(root,&mut s,args)?})),
        "read_attachment" => {
            use base64::{engine::general_purpose::STANDARD, Engine};
            let f = message_files(&s, Some(&json!([args["upload_id"]])))?.remove(0);
            let target = safe(root, &file_path(&s, &f)?)?;
            if fs::metadata(&target).map_err(io)?.len() > MAX_FILE_BYTES as u64 {
                return Err(err("Attachment exceeds size limit"));
            }
            let bytes = fs::read(target).map_err(io)?;
            if digest(&bytes) != f["sha256"].as_str().unwrap_or("") {
                return Err(err("Attachment content changed"));
            }
            return Ok(json!({"attachment":f,"data_base64":STANDARD.encode(bytes)}));
        }
        "send" => {
            if s["closed"] == true {
                return Err(err("Conversation is closed"));
            }
            let message_id = id(&args["message_id"])?;
            let attachments = message_files(&s, args.get("attachment_ids"))?;
            let value = if args["text"].as_str().unwrap_or("").is_empty() && !attachments.is_empty()
            {
                json!("📎")
            } else {
                args["text"].clone()
            };
            let content = text(&value, 32000)?;
            if let Some(m) = s["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["id"] == message_id)
            {
                if m["role"] != "user"
                    || m["text"] != content
                    || m.get("attachments").cloned().unwrap_or(json!([])) != json!(attachments)
                {
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
                    .push(json!({"id":message_id,"role":"user","text":content,"attachments":attachments,"created_at":now()}));
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
            let event = tool_event(args.get("tool_event"), final_reply)?;
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
                    || m["tool_event"] != event
                {
                    return Err(err("Message ID conflicts with an existing reply"));
                }
            } else {
                if pending(&s).map(|m| m["id"].clone()) != Some(json!(reply_to)) {
                    return Err(err("Reply must address the oldest unanswered user message"));
                }
                s["messages"].as_array_mut().unwrap().push(json!({"id":message_id,"role":"assistant","text":content,"reply_to":reply_to,"final":final_reply,"tool_event":event,"created_at":now()}));
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
    #[test]
    fn temporary_lock_contention_recovers_without_stealing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(DIR)).unwrap();
        let path = root.join(DIR).join(".lock");
        fs::create_dir(&path).unwrap();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            fs::remove_dir(path).unwrap();
        });
        assert!(ui(root, &json!({"action":"list"})).is_ok());
        worker.join().unwrap();
    }
    #[tokio::test]
    async fn attachments_and_tool_events_preserve_message_contract() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let data = STANDARD.encode(b"hello attachment");
        let mut upload_args = json!({"action":"upload","chat_id":cid,"upload_id":"file1","name":"notes.txt","data_base64":data});
        let f = ui(root, &upload_args).unwrap()["attachment"].clone();
        assert_eq!(ui(root, &upload_args).unwrap()["attachment"], f);
        upload_args["name"] = json!("../bad");
        assert!(ui(root, &upload_args).is_err());
        let other = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        assert!(ui(root,&json!({"action":"send","chat_id":other,"message_id":"cross","text":"x","attachment_ids":["file1"]})).is_err());
        let send = json!({"action":"send","chat_id":cid,"message_id":"u1","text":"","attachment_ids":["file1"]});
        ui(root, &send).unwrap();
        ui(root, &send).unwrap();
        assert_eq!(
            ui(
                root,
                &json!({"action":"read_attachment","chat_id":cid,"upload_id":"file1"})
            )
            .unwrap()["data_base64"],
            data
        );
        let opened = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap();
        let mut args =
            json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"timeout_ms":0});
        assert_eq!(
            wait(root, &args).await.unwrap()["message"]["attachments"][0],
            f
        );
        args["message_id"] = json!("t1");
        args["reply_to"] = json!("u1");
        args["text"] = json!("Read project root");
        args["final"] = json!(false);
        args["tool_event"] = json!({"name":"list_files","status":"completed"});
        tool(root, "chat_reply", &args).unwrap();
        tool(root, "chat_reply", &args).unwrap();
        assert_eq!(wait(root, &args).await.unwrap()["status"], "message");
        args["final"] = json!(true);
        assert!(tool(root, "chat_reply", &args).is_err());
        fs::write(root.join(f["path"].as_str().unwrap()), b"changed").unwrap();
        assert!(ui(
            root,
            &json!({"action":"read_attachment","chat_id":cid,"upload_id":"file1"})
        )
        .is_err());
    }
}
