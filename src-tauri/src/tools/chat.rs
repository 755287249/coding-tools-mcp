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
fn user_message_state(s: &Value, message: &Value) -> String {
    let messages = s["messages"].as_array().unwrap();
    let replies: Vec<_> = messages
        .iter()
        .filter(|reply| reply["role"] == "assistant" && reply["reply_to"] == message["id"])
        .collect();
    let read = message["received_at"].as_u64().is_some_and(|n| n > 0) || !replies.is_empty();
    let closed = s["closed"] == true;
    let mut status = if closed {
        "会话已结束"
    } else if read {
        "正在处理"
    } else {
        "排队中"
    };
    if let Some(reply) = replies.iter().find(|reply| reply["final"] == true) {
        let index = messages
            .iter()
            .position(|m| m["id"] == reply["id"])
            .unwrap();
        let answered = messages[index + 1..].iter().any(|m| m["role"] == "user");
        status = if reply["awaiting_user"] == true && !answered {
            if closed {
                "会话已结束"
            } else {
                "待确认"
            }
        } else {
            "已回复"
        };
    }
    format!(
        "消息状态：{} · {status}",
        if read { "已读" } else { "未读" }
    )
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
            if m["role"] == "user" {
                out.push_str(&format!("{}\n\n", user_message_state(s, m)));
            }
            if m["role"] == "assistant" {
                let state = if m["awaiting_user"] == true {
                    "awaiting_user"
                } else if m["final"] == false {
                    "supplementing"
                } else {
                    "complete"
                };
                out.push_str(&format!("Reply state: {state}\n\n"));
            }
            for (field, label) in [("input", "Input"), ("output", "Output")] {
                if let Some(detail) = m["tool_event"][field].as_str() {
                    out.push_str(&format!("{label}:\n{detail}\n\n"));
                }
            }
            if m["tool_event"]["output_truncated"] == true {
                out.push_str("Output truncated\n\n");
            }
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
    let work_state = if s["closed"] == true { None } else {
        pending(s).map(|message| {
            let accepted = message["received_at"].as_u64().is_some_and(|n| n > 0)
                || s["messages"].as_array().is_some_and(|messages| messages.iter().any(|reply| reply["role"] == "assistant" && reply["reply_to"] == message["id"]));
            if accepted { "processing" } else { "queued" }
        })
    };
    o.insert("work_state".into(), json!(work_state));
    o.insert("assistant_message_count".into(), json!(s["messages"].as_array().map_or(0, |messages| messages.iter().filter(|m| m["role"] == "assistant").count())));
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
    let mut event = json!({"name":text(&value["name"],120)?,"status":value["status"]});
    for (field, limit) in [("input", 8000), ("output", 16000)] {
        if let Some(detail) = value.get(field) {
            event[field] = json!(text(detail, limit)?);
        }
    }
    if let Some(truncated) = value.get("output_truncated") {
        if !truncated.is_boolean() {
            return Err(err("output_truncated must be a boolean"));
        }
        if truncated == true {
            event["output_truncated"] = json!(true);
        }
    }
    Ok(event)
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
                if s["messages"].as_array().unwrap().is_empty() && s["title_custom"] != true {
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
        "rename" => {
            let title = text(&args["title"], 240)?;
            s["title"] = json!(title.split_whitespace().collect::<Vec<_>>().join(" "));
            s["title_custom"] = json!(true);
            s["updated_at"] = json!(now());
            save(root, &s)?;
        }
        "detach" => {
            s["attachment_id"] = json!("");
            s["lease_until"] = json!(0);
            s["updated_at"] = json!(now());
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
            json!({"ok":true,"attachment_id":s["attachment_id"],"session":view(root,&s)?,"instruction":"Use chat_reply for all user-visible replies, questions, progress and deliverables. Call chat_wait now with the returned attachment_id. Use final=false for progress and tool_event with actual name/status/input/output for tool calls; report results only after execution, redact secrets and mark shortened output. Use final=true to acknowledge reply_to; add awaiting_user=true for a question, then wait for the next message. Keep unique reply IDs and identical retry payloads; require persisted=true. Inspect records before repeating side effects. Immediately call chat_wait after idle or a persisted final reply; never stop voluntarily for idle duration, task completion or cost. Stop only on user cancellation, closed session, actual host limits or a blocking chat configuration error. Renew via chat_open with the same attachment_id before the 10-minute lease expires."}),
        );
    }
    owned(&s, args)?;
    match name {
        "chat_upload" => {
            use base64::{engine::general_purpose::STANDARD, Engine};
            let encoded = args["data_base64"].as_str().unwrap_or("");
            if encoded.len() > 699052 || STANDARD.decode(encoded).is_ok_and(|bytes| bytes.len() > 512 * 1024) {
                return Err(err("MCP attachment must not exceed 512 KiB; compress it before upload"));
            }
            s["lease_until"] = json!(now() + LEASE_MS);
            let attachment = upload(root, &mut s, args)?;
            Ok(json!({"ok":true,"attachment":attachment}))
        }
        "chat_reply" => {
            let message_id = id(&args["message_id"])?;
            let reply_to = id(&args["reply_to"])?;
            let content = text(&args["text"], 32000)?;
            let attachments = message_files(&s, args.get("attachment_ids"))?;
            let final_reply = args["final"] != false;
            if args.get("awaiting_user").is_some_and(|v| !v.is_boolean()) {
                return Err(err("awaiting_user must be a boolean"));
            }
            let awaiting_user = args["awaiting_user"] == true;
            if awaiting_user && !final_reply {
                return Err(err("awaiting_user requires final=true"));
            }
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
                    || (m["awaiting_user"] == true) != awaiting_user
                    || m["tool_event"] != event
                    || m.get("attachments").cloned().unwrap_or(json!([])) != json!(attachments)
                {
                    return Err(err("Message ID conflicts with an existing reply"));
                }
            } else {
                if pending(&s).map(|m| m["id"].clone()) != Some(json!(reply_to)) {
                    return Err(err("Reply must address the oldest unanswered user message"));
                }
                s["messages"].as_array_mut().unwrap().push(json!({"id":message_id,"role":"assistant","text":content,"reply_to":reply_to,"final":final_reply,"awaiting_user":awaiting_user,"tool_event":event,"attachments":attachments,"created_at":now()}));
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
            if let Some(message) = pending(&s) {
                if message["received_at"].is_null() {
                    let received_at = now();
                    let stored = s["messages"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|m| m["id"] == message["id"])
                        .unwrap();
                    stored["received_at"] = json!(received_at);
                    s["updated_at"] = json!(received_at);
                }
            }
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
        if pending(&s).is_some() {
            return tool(root, "chat_wait", args);
        }
        tokio::time::sleep_until(std::cmp::min(
            deadline,
            tokio::time::Instant::now() + std::time::Duration::from_millis(250),
        ))
        .await;
    }
    Ok(
        json!({"ok":true,"status":"idle","instruction":"No message yet. Immediately call chat_wait again with the same session and attachment. Idle timeout does not end the conversation; do not stop for idle duration or cost."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_receipts_keep_unread_queue_distinct() {
        let mut session = json!({"id":"test","title":"Test","closed":false,"messages":[{"id":"u1","role":"user","text":"first"},{"id":"u2","role":"user","text":"second"}]});
        assert_eq!(
            markdown(&session)
                .matches("消息状态：未读 · 排队中")
                .count(),
            2
        );
        session["messages"][0]["received_at"] = json!(10);
        assert!(markdown(&session).contains("消息状态：已读 · 正在处理"));
        assert_eq!(
            markdown(&session)
                .matches("消息状态：未读 · 排队中")
                .count(),
            1
        );
        session["messages"].as_array_mut().unwrap().push(json!({"id":"q1","role":"assistant","reply_to":"u1","final":true,"awaiting_user":true,"text":"Which?"}));
        assert!(markdown(&session).contains("消息状态：已读 · 待确认"));
        session["messages"][0]
            .as_object_mut()
            .unwrap()
            .remove("received_at");
        assert!(markdown(&session).contains("消息状态：已读 · 待确认"));
        session["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"u3","role":"user","text":"answer"}));
        assert!(markdown(&session).contains("消息状态：已读 · 已回复"));
        session["closed"] = json!(true);
        assert_eq!(
            markdown(&session)
                .matches("消息状态：未读 · 会话已结束")
                .count(),
            2
        );
    }
    #[tokio::test]
    async fn confirmation_and_tool_details_are_durable_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let opened = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap();
        ui(
            root,
            &json!({"action":"send","chat_id":cid,"message_id":"u1","text":"help"}),
        )
        .unwrap();
        let event = json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"message_id":"p1","reply_to":"u1","text":"tool result","final":false,"tool_event":{"name":"bash","status":"completed","input":"echo password=secret123","output":"token=secret456\n<script>untrusted text</script>","output_truncated":true}});
        tool(root, "chat_reply", &event).unwrap();
        tool(root, "chat_reply", &event).unwrap();
        let mut changed = event.clone();
        changed["tool_event"]["output"] = json!("different");
        assert!(tool(root, "chat_reply", &changed).is_err());
        changed["message_id"] = json!("oversize");
        changed["tool_event"]["output"] = json!("x".repeat(16001));
        assert!(tool(root, "chat_reply", &changed).is_err());
        changed["tool_event"]["output"] = json!("ok");
        changed["tool_event"]["output_truncated"] = json!("yes");
        assert!(tool(root, "chat_reply", &changed).is_err());
        let question = json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"message_id":"q1","reply_to":"u1","text":"Which option?","final":true,"awaiting_user":true});
        let mut invalid = question.clone();
        invalid["final"] = json!(false);
        assert!(tool(root, "chat_reply", &invalid).is_err());
        tool(root, "chat_reply", &question).unwrap();
        tool(root, "chat_reply", &question).unwrap();
        invalid = question.clone();
        invalid["awaiting_user"] = json!(false);
        assert!(tool(root, "chat_reply", &invalid).is_err());
        let session = load(root, cid.as_str().unwrap()).unwrap();
        assert_eq!(session["messages"].as_array().unwrap().len(), 3);
        assert_eq!(session["messages"][2]["awaiting_user"], true);
        let md = fs::read_to_string(root.join(DIR).join(format!("{}.md", cid.as_str().unwrap())))
            .unwrap();
        assert!(
            md.contains("Input:")
                && md.contains("Output:")
                && md.contains("Output truncated")
                && md.contains("Reply state: awaiting_user")
        );
        assert!(!md.contains("secret123") && !md.contains("secret456"));
        let args = json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"timeout_ms":0});
        assert_eq!(wait(root, &args).await.unwrap()["status"], "idle");
        ui(
            root,
            &json!({"action":"send","chat_id":cid,"message_id":"u2","text":"Option A"}),
        )
        .unwrap();
        assert_eq!(wait(root, &args).await.unwrap()["message"]["id"], "u2");
    }
    #[tokio::test]
    async fn pickup_receipts_persist_for_immediate_and_delayed_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let opened = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap();
        let args = json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"timeout_ms":0});
        ui(
            root,
            &json!({"action":"send","chat_id":cid,"message_id":"u1","text":"immediate"}),
        )
        .unwrap();
        assert!(load(root, cid.as_str().unwrap()).unwrap()["messages"][0]["received_at"].is_null());
        let first = wait(root, &args).await.unwrap();
        assert!(first["message"]["received_at"].as_u64().unwrap() > 0);
        assert_eq!(
            load(root, cid.as_str().unwrap()).unwrap()["messages"][0]["received_at"],
            first["message"]["received_at"]
        );
        assert_eq!(
            wait(root, &args).await.unwrap()["message"]["received_at"],
            first["message"]["received_at"]
        );
        tool(root, "chat_reply", &json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"message_id":"a1","reply_to":"u1","text":"done"})).unwrap();
        let delayed_args =
            json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"timeout_ms":2000});
        let (delayed, _) = tokio::join!(wait(root, &delayed_args), async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            ui(
                root,
                &json!({"action":"send","chat_id":cid,"message_id":"u2","text":"delayed"}),
            )
            .unwrap();
        });
        let delayed = delayed.unwrap();
        assert_eq!(delayed["message"]["id"], "u2");
        assert!(delayed["message"]["received_at"].as_u64().unwrap() > 0);
        assert_eq!(
            load(root, cid.as_str().unwrap()).unwrap()["messages"][2]["received_at"],
            delayed["message"]["received_at"]
        );
    }
    #[test]
    fn ai_upload_enforces_ownership_and_reply_attachment_identity() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let owner = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap()["attachment_id"].clone();
        let args = json!({"chat_id":cid,"attachment_id":owner,"upload_id":"image","name":"result.txt","data_base64":STANDARD.encode(b"AI test")});
        let mut wrong = args.clone(); wrong["attachment_id"] = json!("wrong");
        assert!(tool(root, "chat_upload", &wrong).is_err());
        let first = tool(root, "chat_upload", &args).unwrap()["attachment"].clone();
        assert_eq!(tool(root, "chat_upload", &args).unwrap()["attachment"], first);
        assert_eq!(fs::read(root.join(first["path"].as_str().unwrap())).unwrap(), b"AI test");
        let mut changed = args.clone(); changed["data_base64"] = json!(STANDARD.encode(b"changed"));
        assert!(tool(root, "chat_upload", &changed).is_err());
        changed = args.clone(); changed["upload_id"] = json!("bad"); changed["name"] = json!("../bad");
        assert!(tool(root, "chat_upload", &changed).is_err());
        changed = args.clone(); changed["data_base64"] = json!(STANDARD.encode(vec![0u8; 512 * 1024 + 1]));
        assert!(tool(root, "chat_upload", &changed).is_err());
        changed = args.clone(); changed["data_base64"] = json!("%%%=");
        assert!(tool(root, "chat_upload", &changed).is_err());
        ui(root, &json!({"action":"send","chat_id":cid,"message_id":"u1","text":"send file"})).unwrap();
        let reply = json!({"chat_id":cid,"attachment_id":owner,"message_id":"a1","reply_to":"u1","text":"Saved file","final":true,"attachment_ids":[first["id"]]});
        let mut invalid = reply.clone(); invalid["attachment_ids"] = json!(["foreign"]);
        assert!(tool(root, "chat_reply", &invalid).is_err());
        assert_eq!(tool(root, "chat_reply", &reply).unwrap()["persisted"], true);
        assert_eq!(tool(root, "chat_reply", &reply).unwrap()["persisted"], true);
        invalid = reply.clone(); invalid["attachment_ids"] = json!([]);
        assert!(tool(root, "chat_reply", &invalid).is_err());
        let session = load(root, cid.as_str().unwrap()).unwrap();
        assert_eq!(session["messages"][1]["attachments"], json!([first]));
        assert!(fs::read_to_string(file(root, cid.as_str().unwrap()).unwrap().with_extension("md")).unwrap().contains("Attachment: result.txt"));
        ui(root, &json!({"action":"detach","chat_id":cid})).unwrap();
        assert!(tool(root, "chat_upload", &args).is_err());
    }
    #[tokio::test]
    async fn summary_work_state_tracks_pickup_final_and_close() {
        let dir = tempfile::tempdir().unwrap(); let root = dir.path();
        let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let opened = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap();
        let args = json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"timeout_ms":0});
        let state = || ui(root, &json!({"action":"list"})).unwrap()["sessions"][0]["work_state"].clone();
        assert_eq!(state(), Value::Null);
        ui(root, &json!({"action":"send","chat_id":cid,"message_id":"u1","text":"hello"})).unwrap();
        assert_eq!(state(), "queued");
        wait(root, &args).await.unwrap();
        assert_eq!(state(), "processing");
        tool(root, "chat_reply", &json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"message_id":"a1","reply_to":"u1","text":"done","final":true})).unwrap();
        assert_eq!(state(), Value::Null);
        ui(root, &json!({"action":"send","chat_id":cid,"message_id":"u2","text":"next"})).unwrap();
        tool(root, "chat_reply", &json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"message_id":"a2","reply_to":"u2","text":"working","final":false})).unwrap();
        assert_eq!(state(), "processing");
        ui(root, &json!({"action":"close","chat_id":cid})).unwrap();
        assert_eq!(state(), Value::Null);
    }
    #[test]
    fn summary_reply_count_deduplicates_and_excludes_user_messages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let opened = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap();
        ui(root, &json!({"action":"send","chat_id":cid,"message_id":"u1","text":"hello"})).unwrap();
        assert_eq!(ui(root, &json!({"action":"list"})).unwrap()["sessions"][0]["assistant_message_count"], 0);
        let reply = json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"message_id":"a1","reply_to":"u1","text":"done","final":true});
        tool(root, "chat_reply", &reply).unwrap();
        tool(root, "chat_reply", &reply).unwrap();
        let summary = ui(root, &json!({"action":"list"})).unwrap();
        assert_eq!(summary["sessions"][0]["assistant_message_count"], 1);
        assert!(summary["sessions"][0].get("messages").is_none());
    }
    #[tokio::test]
    async fn local_detach_preserves_chat_and_invalidates_waiter() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let opened = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap();
        let args = json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"timeout_ms":2000});
        ui(root, &json!({"action":"send","chat_id":cid,"message_id":"u1","text":"keep me"})).unwrap();
        tool(root, "chat_reply", &json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"message_id":"a1","reply_to":"u1","text":"done","final":true})).unwrap();
        let before = load(root, cid.as_str().unwrap()).unwrap();
        let (result, _) = tokio::join!(wait(root, &args), async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            ui(root, &json!({"action":"detach","chat_id":cid})).unwrap();
        });
        assert!(result.is_err());
        let after = load(root, cid.as_str().unwrap()).unwrap();
        assert_eq!(after["closed"], false);
        assert_eq!(after["messages"], before["messages"]);
        assert_eq!(after["title"], before["title"]);
        assert!(tool(root, "chat_wait", &args).is_err());
        let next = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap();
        assert_ne!(next["attachment_id"], opened["attachment_id"]);
        ui(root, &json!({"action":"close","chat_id":cid})).unwrap();
        assert_eq!(ui(root, &json!({"action":"detach","chat_id":cid})).unwrap()["session"]["closed"], true);
    }
    #[test]
    fn rename_preserves_messages_attachment_and_archive() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let opened = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap();
        ui(
            root,
            &json!({"action":"rename","chat_id":cid,"title":"  新名称\n project  "}),
        )
        .unwrap();
        ui(
            root,
            &json!({"action":"send","chat_id":cid,"message_id":"u1","text":"Keep this message"}),
        )
        .unwrap();
        let before = load(root, cid.as_str().unwrap()).unwrap();
        assert_eq!(before["title"], "新名称 project");
        assert_eq!(before["attachment_id"], opened["attachment_id"]);
        let renamed = ui(
            root,
            &json!({"action":"rename","chat_id":cid,"title":"<b>Renamed again</b>"}),
        )
        .unwrap();
        let after = load(root, cid.as_str().unwrap()).unwrap();
        assert_eq!(after["messages"], before["messages"]);
        assert_eq!(after["attachment_id"], before["attachment_id"]);
        assert_eq!(after["lease_until"], before["lease_until"]);
        let archive = renamed["session"]["archive_path"].as_str().unwrap();
        assert_eq!(
            archive,
            format!("docs/chat-sessions/{}.md", cid.as_str().unwrap())
        );
        assert!(fs::read_to_string(root.join(archive))
            .unwrap()
            .starts_with("# <b>Renamed again</b>\n"));
        assert_eq!(
            ui(root, &json!({"action":"list"})).unwrap()["sessions"][0]["title"],
            "<b>Renamed again</b>"
        );
        for title in [String::new(), " \n ".into(), "界".repeat(81)] {
            assert!(ui(
                root,
                &json!({"action":"rename","chat_id":cid,"title":title})
            )
            .is_err());
        }
        assert_eq!(
            load(root, cid.as_str().unwrap()).unwrap()["title"],
            "<b>Renamed again</b>"
        );
        ui(root, &json!({"action":"close","chat_id":cid})).unwrap();
        let closed = ui(
            root,
            &json!({"action":"rename","chat_id":cid,"title":"Archived name"}),
        )
        .unwrap();
        assert_eq!(closed["session"]["closed"], true);
        assert_eq!(closed["session"]["title"], "Archived name");
    }
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
