//! Local chat. JSON manifests and immutable pages are authoritative; Markdown is a projection.
#[path = "chat_questions.rs"]
mod questions;
#[path = "chat_history.rs"]
mod history;
#[path = "chat_operations.rs"]
pub(crate) mod operations;
#[path = "chat_group.rs"]
mod group;
#[path = "chat_discussion.rs"]
mod discussion;
#[path = "chat_collaboration.rs"]
mod collaboration;
#[path = "chat_pairing.rs"]
pub(crate) mod pairing;
use super::workspace::WorkspaceError;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

pub const LOCAL_CHAT_SKILL_URI: &str = "coding-tools://skills/local-chat";
const LOCAL_CHAT_SKILL: &str = include_str!("../../../skills/local-chat/SKILL.md");

pub fn local_chat_skill() -> Value {
    json!({"uri": LOCAL_CHAT_SKILL_URI, "name": "local-chat", "title": "Local continuous chat",
        "mimeType": "text/markdown", "text": LOCAL_CHAT_SKILL.replace("\r\n", "\n")})
}

pub fn local_chat_skill_resource() -> Value {
    let mut resource = local_chat_skill();
    resource.as_object_mut().expect("skill object").remove("text");
    resource
}

const DIR: &str = "docs/chat-sessions";
const ASSET_DIR: &str = "mcp-assistant/chat-assets";
const ARTIFACT_DIR: &str = "mcp-assistant/artifacts/";
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
    message_text(v,max).map(|s|s.trim().to_owned())
}
fn message_text(v: &Value, max: usize) -> Result<String> {
    let s = v.as_str().unwrap_or("");
    if s.trim().is_empty() || s.len() > max {
        return Err(err(format!("Text must contain 1–{max} bytes")));
    }
    Ok(super::redaction::redact_sensitive_text(s).0)
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
    load_snapshot(root, chat_id)?.ok_or_else(|| err("Chat storage: conversation no longer exists"))
}
// Atomic replacement gives readers a complete old or new archive. Only a missing
// file is a normal list/delete race; malformed archives and unsafe paths still fail.
fn load_snapshot(root: &Path, chat_id: &str) -> Result<Option<Value>> {
    use std::io::Read;
    let p = file(root, chat_id)?;
    let source = match fs::File::open(p) {
        Ok(source) => source,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(e)),
    };
    let mut bytes = Vec::new();
    source.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(err("Chat archive exceeds size limit"));
    }
    let stored: Value = serde_json::from_slice(&bytes).map_err(|e| err(e.to_string()))?;
    let mut s = history::unpack(root, chat_id, stored)?;
    if (s["version"] != 1 && s["version"] != 2) || s["id"] != chat_id || !s["messages"].is_array() {
        return Err(err("Invalid chat archive"));
    }
    label_files(&mut s);
    Ok(Some(s))
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
    if let Some(reply) = replies.iter().rev().find(|reply| reply["final"] == true && group::complete(s,message)) {
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
    let visible=collaboration::visible(s);let s=&visible;
    let mut out = format!(
        "# {}\n\nSession: {}\n\n",
        s["title"].as_str().unwrap_or(""),
        s["id"].as_str().unwrap_or("")
    );
    if let Some(note)=s["note"].as_str().filter(|note|!note.is_empty()){out.push_str(&format!("备注：{note}\n\n"));}
    out.push_str(&group::markdown(s,None));
    if let Some(messages) = s["messages"].as_array() {
        for m in messages {
            let role = if m["kind"] == "connection_request" {
                "接入请求"
            } else if m["role"] == "user" {
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
            out.push_str(&questions::markdown(m));
            out.push_str(&group::markdown(s,Some(m)));
            out.push_str(&super::chat_plan::markdown(&m["task_plan"]));
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
                        "Attachment: {} ({} bytes)\nPath: {}\nReference: @{}\n\n",
                        f["name"].as_str().unwrap_or(""),
                        f["size"],
                        f["path"].as_str().unwrap_or(""),
                        f["label"].as_str().unwrap_or("")
                    ));
                }
            }
        }
    }
    if let Some(queue)=s["queue"].as_array(){if !queue.is_empty(){out.push_str("\n## 待发送队列\n\n");for (i,m) in queue.iter().enumerate(){out.push_str(&format!("### 队列{}\n\n{}\n\n",i+1,m["text"].as_str().unwrap_or("")));if let Some(files)=m["attachments"].as_array(){for f in files{out.push_str(&format!("@{}: {}\n",f["label"].as_str().unwrap_or(""),f["path"].as_str().unwrap_or("")));}}}}}
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
    let stored = history::pack(root, s)?;
    let bytes = serde_json::to_vec_pretty(&stored).map_err(|e| err(e.to_string()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(err("Chat metadata exceeds limit; preserve the archive and start a new conversation"));
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
    if group::grouped(s){return group::pending_for(s,None,false);}
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
fn delivery(s:&Value,args:&Value,waiting:bool)->Result<Option<Value>>{if group::grouped(s){Ok(group::pending_for(s,Some(&group::member_for(s,args,false)?["id"]),waiting))}else{Ok(pending(s))}}
fn wait_key(root:&Path,s:&Value,args:&Value)->Result<PathBuf>{Ok(file(root,id(&s["id"])?)?.join(id(&args["attachment_id"])?))}

fn view(root: &Path, s: &Value) -> Result<Value> {
    let visible=collaboration::visible(s);let s=&visible;
    let waiting = WAITERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .contains(&file(root, id(&s["id"])?)?.join(s["attachment_id"].as_str().unwrap_or("")));
    let public_members:Vec<_>=group::members(s).iter().map(|m|{let waiting=WAITERS.get_or_init(Default::default).lock().unwrap().contains(&file(root,id(&s["id"]).unwrap()).unwrap().join(m["attachment_id"].as_str().unwrap_or("")));json!({"id":m["id"],"name":m["name"],"role":m["role"],"paused":m["paused"]==true,"status":if s["closed"]==true{"offline"}else if waiting{"waiting"}else if m["lease_until"].as_u64().unwrap_or(0)>now(){"connected"}else{"offline"}})}).collect();
    let status = if s["closed"] == true {
        "closed"
    } else if group::grouped(s) {if public_members.iter().any(|m|m["status"]=="waiting"){"waiting"}else if public_members.iter().any(|m|m["status"]=="connected"){"connected"}else{"offline"}
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
    o.remove("version");o.insert("mode".into(),json!(s["mode"].as_str().unwrap_or("work")));o.insert("members".into(),json!(public_members));
    o.remove("queue");o.remove("queue_receipts");
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
fn reference_path(value: &Value) -> Result<String> {
    let relative = value.as_str().unwrap_or("");
    if !relative.starts_with(ARTIFACT_DIR) || relative.len() > 4096
        || relative.chars().any(|c| c.is_ascii_control() || c == '\\' || c == ':')
        || relative.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
        return Err(err("Local files must be inside mcp-assistant/artifacts"));
    }
    Ok(relative.to_owned())
}
fn fingerprint(target: &Path) -> Result<(String, u64, &'static str)> {
    use std::io::Read;
    use sha2::{Digest, Sha256};
    if !fs::metadata(target).map_err(io)?.is_file() { return Err(err("Attachment must be a regular file")); }
    let mut source = fs::File::open(target).map_err(io)?;
    if !source.metadata().map_err(io)?.is_file() { return Err(err("Attachment must be a regular file")); }
    let mut buffer = [0u8; 64 * 1024];
    let mut hash = Sha256::new(); let mut size = 0u64; let mut mime = "application/octet-stream";
    loop {
        let count = source.read(&mut buffer).map_err(io)?;
        if count == 0 { break; }
        if size == 0 { mime = file_mime(&buffer[..count]); }
        hash.update(&buffer[..count]); size += count as u64;
    }
    if size == 0 { return Err(err("Attachment must not be empty")); }
    Ok((format!("{:x}", hash.finalize()), size, mime))
}
fn file_path(s: &Value, f: &Value) -> Result<String> {
    if f["local_reference"] == true { return reference_path(&f["path"]); }
    let ext = match f["mime"].as_str().unwrap_or("") {
        "image/png" => "png", "image/jpeg" => "jpg", "image/gif" => "gif", "image/webp" => "webp", _ => "bin",
    };
    let legacy = format!("{DIR}/{}-{}.{}", id(&s["id"])?, id(&f["id"])?, ext);
    let current = format!("{ASSET_DIR}/{}/{}.{}", id(&s["id"])?, id(&f["id"])?, ext);
    match f["path"].as_str() {
        Some(p) if p == legacy => Ok(legacy),
        None | Some("") => Ok(current),
        Some(p) if p == current => Ok(current),
        _ => Err(err("Invalid attachment path")),
    }
}
fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
fn upload(root: &Path, s: &mut Value, args: &Value) -> Result<Value> {
    upload_with(root,s,args,&|value|save(root,value))
}
fn upload_with(root: &Path, s: &mut Value, args: &Value, persist:&dyn Fn(&Value)->Result<()>) -> Result<Value> {
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
    if args.get("source_path").is_some() {
        if args.get("data_base64").is_some() { return Err(err("Choose source_path or data_base64, not both")); }
        let relative = reference_path(&args["source_path"])?;
        let (sha256, size, mime) = fingerprint(&safe(root, &relative)?)?;
        if s["files"].is_null() { s["files"] = json!([]); }
        let files = s["files"].as_array().ok_or_else(|| err("Invalid attachment manifest"))?;
        if let Some(existing) = files.iter().find(|f| f["id"] == upload_id) {
            if existing["local_reference"] != true || existing["path"] != relative || existing["sha256"] != sha256 || existing["name"] != name { return Err(err("Attachment ID conflict")); }
            persist(s)?; return Ok(existing.clone());
        }
        let f = json!({"label":next_label(s,mime),"id":upload_id,"name":name,"path":relative,"local_reference":true,"sha256":sha256,"size":size,"mime":mime});
        s["files"].as_array_mut().unwrap().push(f.clone()); persist(s)?; return Ok(f);
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
        if existing["local_reference"] == true || existing["sha256"] != sha256 || existing["name"] != name {
            return Err(err("Attachment ID conflict"));
        }
        persist(s)?;
        return Ok(existing.clone());
    }
    let mut f = json!({"label":next_label(s,file_mime(&bytes)),"id":upload_id,"name":name,"mime":file_mime(&bytes),"size":bytes.len(),"sha256":sha256});
    f["path"] = json!(file_path(s, &f)?);
    fs::create_dir_all(safe(root, &format!("{ASSET_DIR}/{}", id(&s["id"])?))?).map_err(io)?;
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
    persist(s)?;
    Ok(f)
}
fn message_files(s: &Value, ids: Option<&Value>) -> Result<Vec<Value>> {
    let Some(ids) = ids else {
        return Ok(vec![]);
    };
    let ids = ids
        .as_array()
        .ok_or_else(|| err("Select unique attachments"))?;
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for value in ids {
        let attachment_id = id(value)?;
        if !seen.insert(attachment_id) {
            return Err(err("Select unique attachments"));
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
            event[field] = json!(message_text(detail, limit)?);
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
fn read_artifact(root: &Path, value: &Value) -> Result<Value> {
    use std::io::Read;
    use base64::{engine::general_purpose::STANDARD, Engine};
    let relative = reference_path(value)?;
    let target = safe(root, &relative)?;
    if !fs::metadata(&target).map_err(io)?.is_file() { return Err(err("Preview requires a regular image file")); }
    let source = fs::File::open(target).map_err(io)?;
    let meta = source.metadata().map_err(io)?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_FILE_BYTES as u64 { return Err(err("Image preview limit is 2 MiB")); }
    let mut bytes = Vec::new();
    source.take((MAX_FILE_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(io)?;
    if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES { return Err(err("Image preview limit is 2 MiB")); }
    let mime = file_mime(&bytes);
    if !mime.starts_with("image/") { return Err(err("Only PNG, JPEG, GIF and WebP images can be previewed")); }
    Ok(json!({"name":relative.rsplit('/').next().unwrap_or("image"), "mime":mime, "data_base64":STANDARD.encode(bytes)}))
}
pub fn ui(root: &Path, args: &Value) -> Result<Value> {
    let action = args["action"].as_str().unwrap_or("");
    // Polling and file reads consume atomic snapshots, without joining the writer
    // queue. Every mutation (including unknown/discussion actions) stays locked.
    let read_only = matches!(action, "list" | "read" | "read_attachment" | "read_attachment_chunk" | "read_artifact" | "reveal_path");
    let _lock = if read_only { None } else { Some(lock(root)?) };
    if action.starts_with("discussion_"){return discussion::action(root,args,None);}
    if action == "list" {
        let mut sessions = Vec::new();
        let entries = match fs::read_dir(safe(root, DIR)?) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({"sessions": []})),
            Err(e) => return Err(io(e)),
        };
        for entry in entries {
            let p = entry.map_err(io)?.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if id(&json!(stem)).is_err() {
                continue;
            }
            let Some(snapshot) = load_snapshot(root, stem)? else { continue; };
            let mut v = view(root, &snapshot)?;
            v.as_object_mut().unwrap().remove("messages");
            sessions.push(v);
        }
        sessions.sort_by_key(|s| (s["archived"]==true,std::cmp::Reverse((s["pinned"]==true,s["updated_at"].as_u64().unwrap_or(0)))));
        return Ok(json!({"sessions": sessions}));
    }
    if action == "create" {
        let title = text(args.get("title").unwrap_or(&json!("新对话")), 240)?;
        let mut s = json!({"version":1,"id":uuid::Uuid::new_v4().to_string(),"title":title,"created_at":now(),"updated_at":now(),"closed":false,"messages":[],"attachment_id":"","lease_until":0});
        if let Some(mode)=args.get("mode"){group::set_mode(&mut s,mode)?;}
        save(root, &s)?;
        return Ok(json!({"session":local_view(root,&s)?}));
    }
    if let Some(gid)=args["chat_id"].as_str().and_then(|v|v.strip_prefix("discussion:")){return discussion::attachment_action(root,gid,args);}
    let mut s = load(root, id(&args["chat_id"])?)?;
    match action {
        "prepare_pairing" => {
            if s["closed"]==true{return Err(err("Conversation is closed"));}
            let attempt=id(&args["message_id"])?;
            if s["messages"].as_array().unwrap().iter().any(|m|m["id"]==attempt){return Err(err("Pairing ID already used"));}
            if group::grouped(&s){s["pending_pairing"]=json!(attempt);save(root,&s)?;}
            return Ok(json!({"pairing":pairing::prepare(root,id(&s["id"])?,attempt)?}));
        }
        "set_mode"|"rename_member"|"detach_member"|"resume_member"|"set_coordinator"=>{group::ui(&mut s,args)?;s["updated_at"]=json!(now());save(root,&s)?;}
        "reveal_path" => {
            crate::platform::reveal::reveal_chat_path(root, args["source_path"].as_str().unwrap_or("")).map_err(err)?;
            return Ok(json!({"ok":true}));
        }
        "read_artifact" => return read_artifact(root, &args["source_path"]),
        "upload_chunk" => return upload_chunk(root,&mut s,args),
        "read_attachment_chunk" => return read_attachment_chunk(root,&s,args),
        "upload" => return Ok(json!({"attachment":upload(root,&mut s,args)?})),
        "read_attachment" => {
            use base64::{engine::general_purpose::STANDARD, Engine};
            let f = message_files(&s, Some(&json!([args["upload_id"]])))?.remove(0);
            let target = safe(root, &file_path(&s, &f)?)?;
            let (sha256, size, _) = fingerprint(&target)?;
            if sha256 != f["sha256"].as_str().unwrap_or("") || Some(size) != f["size"].as_u64() { return Err(err("Attachment content changed")); }
            if size > MAX_FILE_BYTES as u64 { return Ok(json!({"attachment":f,"local_only":true})); }
            let bytes = fs::read(target).map_err(io)?;
            if digest(&bytes) != f["sha256"].as_str().unwrap_or("") {
                return Err(err("Attachment content changed"));
            }
            return Ok(json!({"attachment":f,"data_base64":STANDARD.encode(bytes)}));
        }
        "request_connection" => {
            if s["closed"] == true { return Err(err("Conversation is closed")); }
            let message_id = id(&args["message_id"])?;
            if s["queue_receipts"].get(message_id).is_some() { return Err(err("Message ID conflicts with a queued delivery")); }
            let messages = s["messages"].as_array().unwrap();
            let existing = messages.iter().chain(s["queue"].as_array().into_iter().flatten()).find(|m| m["id"] == message_id);
            if existing.is_some_and(|m| m["kind"] != "connection_request") {
                return Err(err("Message ID conflicts with an existing message"));
            }
            let request = existing.or_else(|| messages.iter().find(|m| m["kind"] == "connection_request"
                && !messages.iter().any(|r| r["role"] == "assistant" && r["reply_to"] == m["id"] && r["final"] == true)));
            let request_id = request.map(|m| m["id"].clone()).unwrap_or(json!(message_id));
            if request.is_none() {
                s["messages"].as_array_mut().unwrap().push(json!({"id":message_id,"role":"user","kind":"connection_request",
                    "text":"请通过 chat_reply 回复“你好，有什么能帮到你？”（final=true），确认接入后继续 chat_wait。","created_at":now()}));
                s["updated_at"] = json!(now());
            }
            save(root, &s)?;
            return Ok(json!({"session":local_view(root, &s)?,"connection_request_id":request_id}));
        }
        "answer_question" => { questions::answer(&mut s,args)?; save(root,&s)?; }
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
            let content = message_text(&value, 32000)?;
            if let Some(receipt)=s["queue_receipts"][message_id].as_str(){if receipt!=queued_fingerprint(&content,&attachments){return Err(err("Message ID conflicts with a queued delivery"));}save(root,&s)?;return Ok(json!({"session":local_view(root,&s)?}));}
            if let Some(m) = s["messages"].as_array().unwrap().iter().chain(s["queue"].as_array().into_iter().flatten()).find(|m|m["id"]==message_id)
            {
                if m["role"] != "user"
                    || m["kind"] == "connection_request"
                    || m["text"] != content
                    || m.get("attachments").cloned().unwrap_or(json!([])) != json!(attachments)
                {
                    return Err(err("Message ID conflicts with an existing message"));
                }
            } else {
                if !s["messages"].as_array().unwrap().iter().any(|m| m["role"] == "user" && m["kind"] != "connection_request") && s["title_custom"] != true && s["title_agent_name"].as_str().is_none() {
                    s["title"] = json!(content
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(36)
                        .collect::<String>());
                }
                let mut message=json!({"id":message_id,"role":"user","text":content,"attachments":attachments,"created_at":now()});group::target_user(&s,&mut message);
                if !awaiting_confirmation(&s)&&(pending(&s).is_some()||s["queue"].as_array().is_some_and(|q|!q.is_empty())){if s["queue"].is_null(){s["queue"]=json!([]);}s["queue"].as_array_mut().unwrap().push(message);}else{s["messages"].as_array_mut().unwrap().push(message);}
                s["updated_at"] = json!(now());
            }
            save(root, &s)?;
        }
        "cancel_queued" => {
            let message_id = id(&args["message_id"])?;
            if let Some(index) = s["queue"].as_array().and_then(|queue| queue.iter().position(|message| message["id"] == message_id)) {
                let message = s["queue"][index].clone();
                if message.get("discussion").is_some() { return Err(err("Collaboration deliveries cannot be cancelled from the outbox")); }
                // The send receipt also protects cancellation from delayed retries.
                if s["queue_receipts"].is_null() { s["queue_receipts"] = json!({}); }
                s["queue_receipts"][message_id] = json!(queued_fingerprint(message["text"].as_str().unwrap_or(""), message["attachments"].as_array().map(Vec::as_slice).unwrap_or(&[])));
                s["queue"].as_array_mut().unwrap().remove(index);
                s["updated_at"] = json!(now());
                save(root, &s)?;
            }
        }
        "set_queue_mode" => {if args["mode"]!="merge"&&args["mode"]!="split"{return Err(err("Invalid queue mode"));}s["queue_mode"]=args["mode"].clone();save(root,&s)?;}
        "pin" => {if !args["pinned"].is_boolean(){return Err(err("pinned must be a boolean"));}s["pinned"]=args["pinned"].clone();save(root,&s)?;}
        "archive" => {if !args["archived"].is_boolean(){return Err(err("archived must be a boolean"));}s["archived"]=args["archived"].clone();save(root,&s)?;}
        "delete" => {
            let status = view(root, &s)?["status"].clone();
            if status == "connected" || status == "waiting" {
                return Err(err("Disconnect the AI before deleting this conversation"));
            }
            let chat_id = id(&s["id"])?.to_string();
            delete_session_files(root, &chat_id)?;
            return Ok(json!({"deleted":true,"chat_id":chat_id}));
        }
        "set_note" => {
            let note=args["note"].as_str().filter(|note|note.len()<=1000).ok_or_else(||err("Note must be a string of at most 1000 bytes"))?;
            let note=if note.trim().is_empty(){String::new()}else{text(&args["note"],1000)?.split_whitespace().collect::<Vec<_>>().join(" ")};
            s["note"]=json!(note);s["updated_at"]=json!(now());save(root,&s)?;
        }
        "rename" => {
            let title = text(&args["title"], 240)?;
            s["title"] = json!(title.split_whitespace().collect::<Vec<_>>().join(" "));
            s["title_custom"] = json!(true);
            s["updated_at"] = json!(now());
            save(root, &s)?;
        }
        "detach" => {
            if let Some(members)=s["members"].as_array_mut(){for m in members{m["lease_until"]=json!(0);m["paused"]=json!(true);}}
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
    Ok(json!({"session":local_view(root,&s)?}))
}
/// Removes a conversation's JSON record, Markdown projection, sidecar records
/// (`<id>.*`), pasted images (`<id>-<uuid>.<image>`) and its chat-assets folder.
fn delete_session_files(root: &Path, chat_id: &str) -> Result<()> {
    let dir = safe(root, DIR)?;
    let dotted = format!("{chat_id}.");
    let dashed = format!("{chat_id}-");
    let mut targets = Vec::new();
    for entry in fs::read_dir(&dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        let name = entry.file_name().to_string_lossy().to_string();
        let image = name.rsplit_once('.').is_some_and(|(_, ext)| {
            matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
        });
        if !(name.starts_with(&dotted) || (name.starts_with(&dashed) && image)) {
            continue;
        }
        let path = safe(root, &format!("{DIR}/{name}"))?;
        if fs::symlink_metadata(&path).map_err(io)?.is_file() {
            targets.push(path);
        }
    }
    let assets = safe(root, &format!("{ASSET_DIR}/{chat_id}"))?;
    match fs::symlink_metadata(&assets) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(&assets).map_err(io)?,
        Ok(_) => return Err(err("Chat asset path is not a directory")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(io(e)),
    }
    // Keep the authoritative record until every other delete has succeeded.
    let record = file(root, chat_id)?;
    for target in targets.iter().filter(|target| **target != record) {
        fs::remove_file(target).map_err(io)?;
    }
    fs::remove_file(record).map_err(io)?;
    Ok(())
}
fn owned(s: &Value, args: &Value) -> Result<()> {
    if group::grouped(s){group::member_for(s,args,false)?;return Ok(());}
    // The lease only decides whether another AI may take over; the current
    // holder keeps working after a long task and every call renews the lease.
    if args["attachment_id"].as_str().unwrap_or("").is_empty()
        || args["attachment_id"] != s["attachment_id"]
    {
        return Err(err("Chat attachment expired; call chat_open again"));
    }
    Ok(())
}
// Allocate while holding the folder storage lock; keep names stable on resume.
fn assign_agent_title(root:&Path,s:&mut Value,name:&str)->Result<()> {
    if name.is_empty()||s["title_custom"]==true||s["title_agent_name"]==name{return Ok(());}
    let mut occupied=std::collections::HashSet::new();
    for entry in fs::read_dir(safe(root,DIR)?).map_err(io)? {
        let filename=entry.map_err(io)?.file_name().to_string_lossy().to_string();
        if let Some(cid)=filename.strip_suffix(".json").filter(|cid|id(&json!(cid)).is_ok()) {
            if s["id"]==cid{continue;}
            let other=load(root,cid)?;
            if let Some(title)=other["title"].as_str(){occupied.insert(title.to_string());}
        }
    }
    let mut candidate=name.to_string();let mut index=2;
    while occupied.contains(&candidate){candidate=format!("{name}{index}");index+=1;}
    s["title"]=json!(candidate);s["title_agent_name"]=json!(name);s["updated_at"]=json!(now());Ok(())
}
const CHAT_OPEN_INSTRUCTION: &str = "Read the complete skill.text and follow it for this session. Save the returned attachment_id; use the same chat_id, workspace_folder_id and attachment_id. Call chat_wait(timeout_ms:25000) now, shorter if the host requires. Reply only to delivered message IDs.";
const CHAT_MESSAGE_INSTRUCTION: &str = "Process the delivered message; use message.id as reply_to. Follow skill.text, check existing replies before repeating work, and require chat_reply persisted=true. Progress uses final=false; after a final reply start another independent chat_wait(timeout_ms:25000), within host limits.";
const CHAT_IDLE_INSTRUCTION: &str = "No message. Immediately make one new independent chat_wait(timeout_ms:25000), shorter if the host requires, with the same session and attachment. Idle is not an exit; do not batch waits in a polling script.";
const CHAT_PROGRESS_INSTRUCTION: &str = "Progress reply persisted. Continue the current task under skill.text; send a final chat_reply when complete or when user input is needed. Require persisted=true before advancing.";
const CHAT_FINAL_INSTRUCTION: &str = "Final reply persisted; this acknowledges the message and does not close the chat. Immediately make one independent chat_wait(timeout_ms:25000) with the same session and attachment, shorter if the host requires, including when awaiting user input.";
const CHAT_CLOSED_INSTRUCTION: &str = "Conversation is closed. Stop waiting; do not retry chat_wait or create a new attachment for this closed session.";

pub fn tool(root: &Path, name: &str, args: &Value) -> Result<Value> {
    let _lock = lock(root)?;
    let mut discussion_error=None::<&str>;
    if name=="chat_wait"{let current=load(root,id(&args["chat_id"])?)?;if current["closed"]!=true{owned(&current,args)?;if discussion::inbox(root,id(&args["chat_id"])?) .is_err(){discussion_error=Some("Discussion result synchronization is pending; retry chat_wait or inspect the discussion group");}}}
    let mut s = load(root, id(&args["chat_id"])?)?;
    if s["closed"] == true {
        return Ok(json!({"ok":true,"status":"closed","instruction":CHAT_CLOSED_INSTRUCTION}));
    }
    if name == "chat_open" {
        if group::grouped(&s){let member=group::open(&mut s,args)?;
            let prepared=s["pending_pairing"].clone();
            if let Some(attempt)=prepared.as_str().filter(|_|args["attachment_id"].as_str().unwrap_or("").is_empty()) {
                if !s["messages"].as_array().unwrap().iter().any(|m|m["id"]==attempt){s["messages"].as_array_mut().unwrap().push(json!({"id":attempt,"role":"user","kind":"connection_request","text":"请通过 chat_reply 回复“你好，有什么能帮到你？”（final=true），确认接入后继续 chat_wait。","created_at":now(),"recipient_ids":[member["id"]]}));}
            }
            if args["attachment_id"].as_str().unwrap_or("").is_empty(){s.as_object_mut().unwrap().remove("pending_pairing");}
            if member["role"]=="coordinator"{assign_agent_title(root,&mut s,member["name"].as_str().unwrap_or(""))?;}group::bind_targets(&mut s);save(root,&s)?;return Ok(json!({"ok":true,"attachment_id":member["attachment_id"],"agent_id":member["id"],"role":member["role"],"session":history::agent_view(root,&s,&json!({"attachment_id":member["attachment_id"]}))?,"instruction":CHAT_OPEN_INSTRUCTION,"skill":local_chat_skill()}));}
        if args.get("agent_name").is_some(){s["agent_name"]=json!(group::member_name(&args["agent_name"])?);}

        // Keepalive: the saved attachment_id always resumes, even after the lease
        // lapsed, as long as no other AI attached in the meantime.
        let resuming = args["attachment_id"].as_str().is_some_and(|v| !v.is_empty())
            && args["attachment_id"] == s["attachment_id"];
        if args["attachment_id"].as_str().is_some_and(|id|!id.is_empty()) && !resuming {
            return Err(err("Chat attachment expired or replaced; start a new connection from the UI"));
        }
        if !resuming && s["lease_until"].as_u64().unwrap_or(0) > now() {
            return Err(err(
                "Conversation already attached; close it in the UI or wait for the lease to expire",
            ));
        }
        if !resuming {
            s["attachment_id"] = json!(uuid::Uuid::new_v4().to_string());
        }
        let agent_name=s["agent_name"].as_str().unwrap_or("").to_string();assign_agent_title(root,&mut s,&agent_name)?;
        group::renew(&mut s,args)?;
        save(root, &s)?;
        return Ok(
            json!({"ok":true,"attachment_id":s["attachment_id"],"session":history::agent_view(root,&s,args)?,"instruction":CHAT_OPEN_INSTRUCTION,"skill":local_chat_skill()}),
        );
    }
    owned(&s, args)?;
    if name=="chat_discuss"{return discussion::action(root,args,Some(&s));}
    match name {
        "chat_upload" => {
            use base64::{engine::general_purpose::STANDARD, Engine};
            let encoded = args["data_base64"].as_str().unwrap_or("");
            if encoded.len() > 699052 || STANDARD.decode(encoded).is_ok_and(|bytes| bytes.len() > 512 * 1024) {
                return Err(err("MCP attachment must not exceed 512 KiB; compress it before upload"));
            }
            group::renew(&mut s,args)?;
            let attachment = upload(root, &mut s, args)?;
            Ok(json!({"ok":true,"attachment":attachment}))
        }
        "chat_reply" => {
            let message_id = id(&args["message_id"])?;
            if s["queue_receipts"].get(message_id).is_some() { return Err(err("Message ID conflicts with a queued delivery")); }
            let reply_to = id(&args["reply_to"])?;
            let content = message_text(&args["text"], 32000)?;
            let attachments = message_files(&s, args.get("attachment_ids"))?;
            let final_reply = args["final"] != false;
            if args.get("awaiting_user").is_some_and(|v| !v.is_boolean()) {
                return Err(err("awaiting_user must be a boolean"));
            }
            let awaiting_user = args["awaiting_user"] == true;
            if awaiting_user && !final_reply {
                return Err(err("awaiting_user requires final=true"));
            }
            let questions = questions::parse(args.get("questions"))?;
            if !questions.is_null() {questions::validate_context(&s,reply_to,final_reply,awaiting_user)?;}
            let event = tool_event(args.get("tool_event"), final_reply)?;
            let identity=group::reply_identity(&s,args,final_reply)?;
            if let Some(m) = s["messages"]
                .as_array()
                .unwrap()
                .iter().chain(s["queue"].as_array().into_iter().flatten())
                .find(|m| m["id"] == message_id)
            {
                if m["agent_id"]!=identity["agent_id"] || m["recipient_ids"]!=identity["recipient_ids"] || m["role"] != "assistant"
                    || m["text"] != content
                    || m["reply_to"] != reply_to
                    || m["final"] != final_reply
                    || (m["awaiting_user"] == true) != awaiting_user
                    || m["questions"] != questions
                    || m["tool_event"] != event
                    || m.get("attachments").cloned().unwrap_or(json!([])) != json!(attachments)
                {
                    return Err(err("Message ID conflicts with an existing reply"));
                }
            } else {
                group::validate_final(&s,args)?;
                if delivery(&s,args,false)?.map(|m| m["id"].clone()) != Some(json!(reply_to)) {
                    return Err(err("Reply must address the oldest unanswered user message"));
                }
                let mut reply=json!({"id":message_id,"role":"assistant","text":content,"reply_to":reply_to,"final":final_reply,"awaiting_user":awaiting_user,"tool_event":event,"attachments":attachments,"created_at":now()});reply.as_object_mut().unwrap().extend(identity.as_object().unwrap().clone());if !questions.is_null(){reply["questions"]=questions;}s["messages"].as_array_mut().unwrap().push(reply);
                s["updated_at"] = json!(now());
            }
            group::renew(&mut s,args)?;
            save(root, &s)?;
            Ok(json!({"ok":true,"persisted":true,"message_id":message_id,"instruction":if final_reply {CHAT_FINAL_INSTRUCTION}else{CHAT_PROGRESS_INSTRUCTION}}))
        }
        "chat_close" => {
            if group::grouped(&s)&&group::member_for(&s,args,false)?["role"]!="coordinator"{return Err(err("Only the coordinator can close the group"));}
            s["closed"] = json!(true);
            s["attachment_id"] = json!("");
            s["lease_until"] = json!(0);
            save(root, &s)?;
            Ok(json!({"ok":true,"status":"closed","instruction":CHAT_CLOSED_INSTRUCTION}))
        }
        "chat_wait" => {
            publish_queued(&mut s);
            group::renew(&mut s,args)?;
            if let Some(message) = delivery(&s,args,true)? {
                if group::grouped(&s){let agent=group::member_for(&s,args,false)?["id"].clone();let stored=s["messages"].as_array_mut().unwrap().iter_mut().find(|m|m["id"]==message["id"]).unwrap();if stored["received_by"].is_null(){stored["received_by"]=json!([]);}if !stored["received_by"].as_array().unwrap().contains(&agent){stored["received_by"].as_array_mut().unwrap().push(agent);s["updated_at"]=json!(now());}}
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
            let message = delivery(&s,args,true)?;
            Ok(
                json!({"ok":true,"status":if message.is_some(){"message"}else{"idle"},"instruction":if message.is_some(){CHAT_MESSAGE_INSTRUCTION}else{CHAT_IDLE_INSTRUCTION},"discussion_error":discussion_error,"message":message,"session":if group::grouped(&s){history::agent_view(root,&s,args)?}else{Value::Null}}),
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
    let path = wait_key(root,&load(root,chat_id)?,args)?;
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
            return Ok(json!({"ok":true,"status":"closed","instruction":CHAT_CLOSED_INSTRUCTION}));
        }
        owned(&s, args)?;
        if delivery(&s,args,true)?.is_some() || (pending(&s).is_none() && s["queue"].as_array().is_some_and(|q|!q.is_empty()) && !awaiting_confirmation(&s)) {
            return tool(root, "chat_wait", args);
        }
        tokio::time::sleep_until(std::cmp::min(
            deadline,
            tokio::time::Instant::now() + std::time::Duration::from_millis(250),
        ))
        .await;
    }
    Ok(
        json!({"ok":true,"status":"idle","instruction":CHAT_IDLE_INSTRUCTION}),
    )
}

const CHUNK_BYTES: usize = 512 * 1024;
fn label_files(s: &mut Value) {
    let mut images = 0; let mut files = 0;
    let mut labels = std::collections::HashMap::new();
    if let Some(entries) = s["files"].as_array_mut() {
        for f in entries {
            let label = if f["mime"].as_str().unwrap_or("").starts_with("image/") { images += 1; format!("图片{images}") } else { files += 1; format!("文件{files}") };
            f["label"] = json!(label); labels.insert(f["id"].as_str().unwrap_or("").to_owned(), label);
        }
    }
    if let Some(messages) = s["messages"].as_array_mut() { for message in messages { if message["discussion"].is_object(){continue;} if let Some(entries) = message["attachments"].as_array_mut() { for f in entries { if let Some(label) = labels.get(f["id"].as_str().unwrap_or("")) { f["label"] = json!(label); } } } } }
}
fn next_label(s: &Value, mime: &str) -> String {
    let image = mime.starts_with("image/");
    let count = s["files"].as_array().map(|files| files.iter().filter(|f| f["mime"].as_str().unwrap_or("").starts_with("image/") == image).count()).unwrap_or(0);
    format!("{}{}", if image {"图片"} else {"文件"}, count + 1)
}
fn upload_chunk(root: &Path, s: &mut Value, args: &Value) -> Result<Value> {
    upload_chunk_with(root,s,args,&|value|save(root,value))
}
fn upload_chunk_with(root: &Path, s: &mut Value, args: &Value, persist:&dyn Fn(&Value)->Result<()>) -> Result<Value> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::io::{Read, Seek, SeekFrom, Write};
    if s["closed"] == true { return Err(err("Conversation is closed")); }
    let upload_id = id(&args["upload_id"])?; let name = text(&args["name"], 240)?;
    if name.chars().any(|c| c.is_ascii_control() || c == '/' || c == '\\') { return Err(err("Invalid attachment name")); }
    let offset = args["offset"].as_u64().ok_or_else(|| err("Invalid upload range"))?;
    let total = args["total_size"].as_u64().filter(|v| *v > 0 && *v <= 9007199254740991 && offset < *v).ok_or_else(|| err("Invalid upload range"))?;
    let encoded = args["data_base64"].as_str().unwrap_or("");
    if encoded.len() > 699052 { return Err(err("Invalid chunk encoding or size")); }
    let bytes = STANDARD.decode(encoded).map_err(|_| err("Invalid chunk encoding or size"))?;
    if bytes.is_empty() || bytes.len() > CHUNK_BYTES || STANDARD.encode(&bytes) != encoded || offset + bytes.len() as u64 > total { return Err(err("Invalid chunk encoding or size")); }
    let existing = s["files"].as_array().and_then(|files| files.iter().find(|f| f["id"] == upload_id)).cloned();
    let dir = format!("{ASSET_DIR}/{}", id(&s["id"])?);
    fs::create_dir_all(safe(root,&dir)?).map_err(io)?;
    let meta_path = safe(root,&format!("{dir}/{upload_id}.upload.json"))?;
    let part_path = safe(root,&format!("{dir}/{upload_id}.part"))?;
    let meta: Value;
    if let Some(f) = &existing {
        if f["local_reference"] == true || f["name"] != name || f["size"] != total { return Err(err("Attachment ID conflict")); }
        meta = f.clone();
    } else if meta_path.exists() {
        meta = serde_json::from_slice(&fs::read(&meta_path).map_err(io)?).map_err(|_| err("Invalid upload metadata"))?;
        if meta["name"] != name || meta["size"] != total { return Err(err("Attachment ID conflict")); }
    } else {
        if offset != 0 { return Err(err("Upload must start at offset 0")); }
        meta = json!({"name":name,"size":total,"mime":file_mime(&bytes)});
        let mut options = fs::OpenOptions::new(); options.write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&meta_path).map_err(io)?; file.write_all(meta.to_string().as_bytes()).map_err(io)?; file.sync_all().map_err(io)?;
    }
    let mut f = json!({"id":upload_id,"name":name,"size":total,"mime":meta["mime"]});
    f["path"] = json!(file_path(s,&f)?); let target = safe(root,f["path"].as_str().unwrap())?;
    let completed = existing.is_some() || target.exists(); let source = if completed {&target} else {&part_path};
    if !source.exists() {
        let mut options = fs::OpenOptions::new(); options.write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        options.open(source).map_err(io)?;
    }
    let mut file = fs::OpenOptions::new().read(true).write(!completed).open(source).map_err(io)?;
    let size = file.metadata().map_err(io)?.len();
    if size > total || offset > size { return Err(err("Upload offset does not match stored bytes")); }
    let overlap = (size-offset).min(bytes.len() as u64) as usize;
    let mut prior = vec![0;overlap]; file.seek(SeekFrom::Start(offset)).map_err(io)?; file.read_exact(&mut prior).map_err(io)?;
    if prior != bytes[..overlap] { return Err(err("Attachment ID conflict")); }
    if overlap < bytes.len() {
        if completed { return Err(err("Attachment content changed")); }
        file.write_all(&bytes[overlap..]).map_err(io)?; file.sync_all().map_err(io)?;
    }
    drop(file);
    if let Some(existing) = existing { persist(s)?; return Ok(json!({"attachment":existing,"next_offset":offset+bytes.len() as u64})); }
    if fs::metadata(source).map_err(io)?.len() < total { return Ok(json!({"next_offset":offset+bytes.len() as u64})); }
    let (sha256,size,mime) = fingerprint(source)?;
    f["sha256"]=json!(sha256);f["size"]=json!(size);f["mime"]=json!(mime);f["label"]=json!(next_label(s,mime));
    if !completed {fs::rename(&part_path,&target).map_err(io)?;}
    if s["files"].is_null() {s["files"]=json!([]);}
    s["files"].as_array_mut().unwrap().push(f.clone());persist(s)?;fs::remove_file(meta_path).map_err(io)?;
    Ok(json!({"attachment":f,"next_offset":offset+bytes.len() as u64}))
}
fn read_attachment_chunk(root: &Path, s: &Value, args: &Value) -> Result<Value> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::io::{Read, Seek, SeekFrom};
    let upload_id = id(&args["upload_id"])?;
    let f = s["files"].as_array().and_then(|files| files.iter().find(|f| f["id"] == upload_id)).ok_or_else(|| err("Attachment not found"))?;
    let size = f["size"].as_u64().unwrap_or(0);
    let offset = args["offset"].as_u64().filter(|offset| *offset < size).ok_or_else(|| err("Invalid attachment range"))?;
    let mut file = fs::File::open(safe(root,&file_path(s,f)?)?).map_err(io)?;
    if !file.metadata().map_err(io)?.is_file() || file.metadata().map_err(io)?.len() != size {return Err(err("Attachment content changed"));}
    let mut bytes = vec![0; (size-offset).min(CHUNK_BYTES as u64) as usize];file.seek(SeekFrom::Start(offset)).map_err(io)?;file.read_exact(&mut bytes).map_err(io)?;
    Ok(json!({"attachment":f,"data_base64":STANDARD.encode(&bytes),"next_offset":offset+bytes.len() as u64}))
}

fn awaiting_confirmation(s: &Value) -> bool {
    let visible=collaboration::visible(s);let s=&visible;
    if let Some(messages)=s["messages"].as_array(){for m in messages.iter().rev(){if m["role"]=="user"{return false;}if m["role"]=="assistant"&&m["final"]==true{return m["awaiting_user"]==true;}}}false
}
fn queued_fingerprint(content: &str, attachments: &[Value]) -> String {
    digest(json!([content,attachments.iter().map(|f|f["id"].clone()).collect::<Vec<_>>()]).to_string().as_bytes())
}
fn local_view(root: &Path,s: &Value)->Result<Value>{
    let visible=collaboration::visible(s);let s=&visible;
    let mut result=view(root,s)?;
    let active_question=questions::active_id(s);
    if let Some(messages)=result["messages"].as_array_mut(){for message in messages{if message["questions"].is_array(){message["questions_active"]=json!(message["id"].as_str()==active_question.as_deref());}}}
    result["pairing"]=pairing::status(root,id(&s["id"])?);
    if let Some(attachment)=s["attachment_id"].as_str().filter(|id|!id.is_empty()) {
        result["connection_id"]=json!(format!("{:x}",Sha256::digest(attachment.as_bytes()))[..12].to_string());
    }
    result.as_object_mut().unwrap().extend(operations::view(root,id(&s["id"])?).as_object().unwrap().clone());if let Some(ops)=result["operations"].as_array_mut(){ops.retain(|op|s["messages"].as_array().into_iter().flatten().any(|m|m["id"]==op["reply_to"]));}result["queued_messages"]=s.get("queue").cloned().unwrap_or(json!([]));result["queue_mode"]=s.get("queue_mode").cloned().unwrap_or_else(||json!(if group::grouped(s){"split"}else{"merge"}));Ok(result)
}
fn publish_queued(s: &mut Value) {
    if pending(s).is_some()||awaiting_confirmation(s){return;}
    let split=s["queue_mode"].as_str().unwrap_or(if group::grouped(s){"split"}else{"merge"})=="split";
    let Some(queue)=s["queue"].as_array_mut() else{return;};if queue.is_empty(){return;}
    let count=if split||queue[0].get("discussion").is_some(){1}else{queue.iter().position(|m|m.get("discussion").is_some()).unwrap_or(queue.len())};let items=queue.drain(..count).collect::<Vec<_>>();
    let mut message=items[0].clone();let mut attachments=Vec::new();let mut seen=HashSet::new();
    for item in &items {if let Some(files)=item["attachments"].as_array(){for file in files{if seen.insert(file["id"].as_str().unwrap_or("").to_owned()){attachments.push(file.clone());}}}}
    if s["queue_receipts"].is_null(){s["queue_receipts"]=json!({});}
    for item in &items{s["queue_receipts"][item["id"].as_str().unwrap()]=json!(queued_fingerprint(item["text"].as_str().unwrap_or(""),item["attachments"].as_array().map(Vec::as_slice).unwrap_or(&[])));}
    if items.len()>1 {message["text"]=json!(items.iter().enumerate().map(|(i,m)|format!("队列{}：\n\n{}",i+1,m["text"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n\n"));}
    message["attachments"]=json!(attachments);message["created_at"]=json!(now());if group::grouped(s){
        // Union the saved identities, not names reparsed from merged text.
        let mut targets=Vec::new();
        for item in &items {let mut item=item.clone();if item["recipient_ids"].as_array().is_none_or(|ids|ids.is_empty()){group::target_user(s,&mut item);}
            if let Some(ids)=item["recipient_ids"].as_array(){for id in ids {if !targets.contains(id){targets.push(id.clone());}}}}
        message["recipient_ids"]=json!(targets);
    }s["messages"].as_array_mut().unwrap().push(message);s["updated_at"]=json!(now());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_message_format_survives_persistence_queue_and_reply() {
        let dir=tempfile::tempdir().unwrap();let root=dir.path();
        let cid=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let attachment=tool(root,"chat_open",&json!({"chat_id":cid})).unwrap()["attachment_id"].clone();
        let body="    code\r\n\r\n**正文**  \n\tend\n\n";
        let send=json!({"action":"send","chat_id":cid,"message_id":"format-a","text":body});
        ui(root,&send).unwrap();ui(root,&send).unwrap();
        let args=json!({"chat_id":cid,"attachment_id":attachment});
        assert_eq!(tool(root,"chat_wait",&args).unwrap()["message"]["text"],body);
        let mut conflict=send.clone();conflict["text"]=json!(body.trim());assert!(ui(root,&conflict).is_err());
        for id in ["format-b","format-c"]{let mut next=send.clone();next["message_id"]=json!(id);ui(root,&next).unwrap();}
        let reply=json!({"chat_id":cid,"attachment_id":attachment,"message_id":"format-reply","reply_to":"format-a","text":body,"final":false,"tool_event":{"name":"format-check","status":"completed","input":body,"output":body}});
        tool(root,"chat_reply",&reply).unwrap();tool(root,"chat_reply",&reply).unwrap();
        let detail=ui(root,&json!({"action":"read","chat_id":cid})).unwrap()["session"].clone();
        assert_eq!(detail["messages"][1]["text"],body);assert_eq!(detail["messages"][1]["tool_event"]["output"],body);
        assert!(fs::read_to_string(root.join(detail["archive_path"].as_str().unwrap())).unwrap().contains(body));
        tool(root,"chat_reply",&json!({"chat_id":cid,"attachment_id":attachment,"message_id":"done","reply_to":"format-a","text":"done","final":true})).unwrap();
        assert_eq!(tool(root,"chat_wait",&args).unwrap()["message"]["text"],format!("队列1：\n\n{body}\n\n队列2：\n\n{body}"));
        assert!(message_text(&json!(" ".repeat(32000)+"x"),32000).is_err());
    }

    #[test]
    fn cancel_one_queued_message_preserves_order_and_retry_receipts() {
        for grouped in [false, true] { for mode in ["merge", "split"] {
            let dir = tempfile::tempdir().unwrap(); let root = dir.path();
            let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
            let aid = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap()["attachment_id"].clone();
            if grouped { ui(root, &json!({"action":"set_mode","chat_id":cid,"mode":"group"})).unwrap(); }
            ui(root, &json!({"action":"set_queue_mode","chat_id":cid,"mode":mode})).unwrap();
            let read = || ui(root, &json!({"action":"read","chat_id":cid})).unwrap()["session"].clone();
            let send = |id: &str, text: &str| ui(root, &json!({"action":"send","chat_id":cid,"message_id":id,"text":text}));
            let cancel = |id: &str| ui(root, &json!({"action":"cancel_queued","chat_id":cid,"message_id":id})).unwrap();
            send("active", "Current work").unwrap();
            for id in ["q1", "q2", "q3"] { send(id, &format!("Queued {id}")).unwrap(); }
            let before = read();
            cancel("q2"); cancel("q2"); send("q2", "Queued q2").unwrap();
            let expected: Vec<Value> = before["queued_messages"].as_array().unwrap().iter().filter(|m| m["id"] != "q2").cloned().collect();
            assert_eq!(read()["queued_messages"], json!(expected));
            assert_eq!(read()["messages"], before["messages"]);
            assert!(send("q2", "different").is_err());
            cancel("active"); cancel("missing"); assert_eq!(read()["messages"], before["messages"]);
            assert!(!fs::read_to_string(root.join(read()["archive_path"].as_str().unwrap())).unwrap().contains("Queued q2"));
            tool(root, "chat_reply", &json!({"chat_id":cid,"attachment_id":aid,"reply_to":"active","message_id":"done","text":"Done","final":true})).unwrap();
            let message = tool(root, "chat_wait", &json!({"chat_id":cid,"attachment_id":aid})).unwrap()["message"].clone();
            assert!(message["text"].as_str().unwrap().contains("Queued q1"));
            assert!(!message["text"].as_str().unwrap().contains("Queued q2"));
            let published = read()["messages"].clone(); cancel("q1"); assert_eq!(read()["messages"], published);
            assert_eq!(read()["queued_messages"].as_array().unwrap().len(), if mode == "split" {1} else {0});
        }}
    }

    #[test]
    fn task_plans_are_owned_persistent_and_message_scoped() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path();
        let a=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let b=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();
        for cid in [&a,&b]{ui(root,&json!({"action":"send","chat_id":cid,"message_id":"user","text":"task"})).unwrap();}
        let aid=tool(root,"chat_open",&json!({"chat_id":a})).unwrap()["attachment_id"].clone();
        let bid=tool(root,"chat_open",&json!({"chat_id":b})).unwrap()["attachment_id"].clone();
        let mut args=json!({"chat_id":a,"attachment_id":aid,"reply_to":"user","goal":"Goal A","todos":[{"id":"read","title":"Read","status":"completed"},{"id":"build","title":"Build","status":"in_progress"}]});
        assert_eq!(plan(root,"set_todos",&args).unwrap()["persisted"],true);
        let read=|cid:&Value|ui(root,&json!({"action":"read","chat_id":cid})).unwrap()["session"].clone();
        assert!(read(&b)["messages"][0]["task_plan"].is_null());
        args["attachment_id"]=bid;assert!(plan(root,"set_todos",&args).is_err());args["attachment_id"]=aid.clone();
        assert!(plan(root,"set_todos",&json!({"attachment_id":aid})).is_err());
        let mut progress=json!({"chat_id":a,"attachment_id":aid,"reply_to":"user","message":"password=synthetic-value working","percent":40,"todo_id":"build"});
        assert_eq!(plan(root,"report_progress",&progress).unwrap()["persisted"],true);
        assert!(!read(&a).to_string().contains("synthetic-value"));
        progress["percent"]=json!(101);assert!(plan(root,"report_progress",&progress).is_err());progress["percent"]=json!(40);progress["todo_id"]=json!("missing");assert!(plan(root,"report_progress",&progress).is_err());
        let update=json!({"chat_id":a,"attachment_id":aid,"reply_to":"user","explanation":"Next","plan":[{"step":"Read","status":"completed"},{"step":"Build","status":"completed"},{"step":"Verify","status":"in_progress"}]});
        let next=plan(root,"update_plan",&update).unwrap();assert_eq!(next["plan"]["todos"][1]["id"],"build");assert_eq!(next["plan"]["todos"][2]["id"],"todo-1");
        assert!(fs::read_to_string(root.join(format!("{DIR}/{}.md",a.as_str().unwrap()))).unwrap().contains("任务计划"));
        assert_eq!(tool(root,"chat_open",&json!({"chat_id":a,"attachment_id":aid})).unwrap()["session"]["messages"][0]["task_plan"]["goal"],"Goal A");
        tool(root,"chat_reply",&json!({"chat_id":a,"attachment_id":aid,"reply_to":"user","message_id":"done","text":"done","final":true})).unwrap();
        ui(root,&json!({"action":"send","chat_id":a,"message_id":"new","text":"new task"})).unwrap();
        assert!(read(&a)["messages"][2]["task_plan"].is_null());assert!(plan(root,"set_todos",&args).is_err());
        args["reply_to"]=json!("new");plan(root,"set_todos",&args).unwrap();args["todos"]=json!([]);assert_eq!(plan(root,"set_todos",&args).unwrap()["plan"]["cleared"],true);
        ui(root,&json!({"action":"detach","chat_id":a})).unwrap();assert!(plan(root,"set_todos",&args).is_err());
    }

    #[test]
    fn group_outbox_modes_are_reversible_and_merge_preserves_targets() {
        let dir=tempfile::tempdir().unwrap();let root=dir.path();
        let cid=ui(root,&json!({"action":"create","mode":"group"})).unwrap()["session"]["id"].clone();
        let a=tool(root,"chat_open",&json!({"chat_id":cid,"agent_name":"Chief"})).unwrap();
        let b=tool(root,"chat_open",&json!({"chat_id":cid,"agent_name":"Helper"})).unwrap();
        let file=ui(root,&json!({"action":"upload","chat_id":cid,"upload_id":"queue-file","name":"note.txt","data_base64":"ZmlsZQ=="})).unwrap()["attachment"]["id"].clone();
        let send=|id:&str,text:&str|ui(root,&json!({"action":"send","chat_id":cid,"message_id":id,"text":text,"attachment_ids":[file]}));
        let read=||ui(root,&json!({"action":"read","chat_id":cid})).unwrap()["session"].clone();
        let wait=|who:&Value|tool(root,"chat_wait",&json!({"chat_id":cid,"attachment_id":who["attachment_id"]})).unwrap();
        let reply=|who:&Value,id:&str,to:&str|tool(root,"chat_reply",&json!({"chat_id":cid,"attachment_id":who["attachment_id"],"message_id":id,"reply_to":to,"text":"done","final":true}));
        send("active","Active").unwrap();send("q1","First").unwrap();send("q2","Second").unwrap();send("q3","@Helper Third 😀").unwrap();
        let before=read()["queued_messages"].clone();assert_eq!(read()["queue_mode"],"split");
        for mode in ["merge","split","merge","split"] {
            let state=ui(root,&json!({"action":"set_queue_mode","chat_id":cid,"mode":mode})).unwrap()["session"].clone();
            assert_eq!(state["queue_mode"],mode);assert_eq!(state["queued_messages"],before);assert_eq!(wait(&a)["message"]["id"],"active");
        }
        reply(&a,"active-done","active").unwrap();assert_eq!(wait(&a)["message"]["id"],"q1");assert_eq!(read()["queued_messages"].as_array().unwrap().len(),2);
        ui(root,&json!({"action":"set_queue_mode","chat_id":cid,"mode":"merge"})).unwrap();
        ui(root,&json!({"action":"rename_member","chat_id":cid,"member_id":b["agent_id"],"name":"Renamed"})).unwrap();
        reply(&a,"q1-done","q1").unwrap();assert_eq!(read()["queued_messages"].as_array().unwrap().len(),2);
        let merged=wait(&a)["message"].clone();assert_eq!(merged["text"],"队列1：\n\nSecond\n\n队列2：\n\n@Helper Third 😀");
        assert_eq!(merged["recipient_ids"],json!([a["agent_id"],b["agent_id"]]));assert_eq!(merged["attachments"].as_array().unwrap().len(),1);
        assert_eq!(wait(&b)["message"]["id"],"q2");assert!(reply(&a,"early","q2").is_err());reply(&b,"b-done","q2").unwrap();reply(&a,"a-done","q2").unwrap();
        send("q2","Second").unwrap();send("q3","@Helper Third 😀").unwrap();assert_eq!(read()["queued_messages"],json!([]));assert_eq!(wait(&a)["status"],"idle");assert!(send("q3","Changed").is_err());
    }

    #[test]
    fn outbox_wait_boundary_merge_receipts_and_pin_persist() {
        let dir = tempfile::tempdir().unwrap(); let root=dir.path();
        let session=ui(root,&json!({"action":"create"})).unwrap(); let cid=&session["session"]["id"];
        let opened=tool(root,"chat_open",&json!({"chat_id":cid})).unwrap();
        let args=json!({"chat_id":cid,"attachment_id":opened["attachment_id"]});
        let uploaded=ui(root,&json!({"action":"upload","chat_id":cid,"upload_id":"q-file","name":"file.txt","data_base64":"ZmlsZQ=="})).unwrap();
        let fid=&uploaded["attachment"]["id"];
        let send=|id:&str, text:&str|ui(root,&json!({"action":"send","chat_id":cid,"message_id":id,"text":text,"attachment_ids":[fid]}));
        send("u1","u1").unwrap();send("__proto__","__proto__").unwrap();send("constructor","constructor").unwrap();send("constructor","constructor").unwrap();
        let read=||ui(root,&json!({"action":"read","chat_id":cid})).unwrap()["session"].clone();
        assert_eq!(read()["messages"].as_array().unwrap().len(),1);
        assert_eq!(read()["queued_messages"].as_array().unwrap().len(),2);
        let public=tool(root,"chat_open",&args).unwrap();
        for key in ["queue","queued_messages","queue_receipts"] {assert!(public["session"].get(key).is_none());}
        tool(root,"chat_reply",&json!({"chat_id":cid,"attachment_id":args["attachment_id"],"message_id":"progress","reply_to":"u1","text":"working","final":false})).unwrap();
        assert_eq!(tool(root,"chat_wait",&args).unwrap()["message"]["id"],"u1");
        tool(root,"chat_reply",&json!({"chat_id":cid,"attachment_id":args["attachment_id"],"message_id":"done","reply_to":"u1","text":"done","final":true})).unwrap();
        assert_eq!(read()["queued_messages"].as_array().unwrap().len(),2);
        assert_eq!(tool(root,"chat_open",&args).unwrap()["session"]["messages"].as_array().unwrap().len(),3);
        let delivered=tool(root,"chat_wait",&args).unwrap()["message"].clone();
        assert_eq!(delivered["id"],"__proto__");
        assert_eq!(delivered["text"],"队列1：\n\n__proto__\n\n队列2：\n\nconstructor");
        assert_eq!(delivered["attachments"].as_array().unwrap().len(),1);
        assert!(delivered["received_at"].as_u64().unwrap()>=delivered["created_at"].as_u64().unwrap());
        send("__proto__","__proto__").unwrap();send("constructor","constructor").unwrap();
        assert_eq!(read()["messages"].as_array().unwrap().len(),4);
        assert!(send("constructor","different").is_err());
        assert!(ui(root,&json!({"action":"request_connection","chat_id":cid,"message_id":"constructor"})).is_err());
        assert!(tool(root,"chat_reply",&json!({"chat_id":cid,"attachment_id":args["attachment_id"],"message_id":"constructor","reply_to":"__proto__","text":"collision","final":true})).is_err());
        ui(root,&json!({"action":"create","title":"newer"})).unwrap();
        ui(root,&json!({"action":"pin","chat_id":cid,"pinned":true})).unwrap();
        assert_eq!(ui(root,&json!({"action":"list"})).unwrap()["sessions"][0]["id"],*cid);
        assert!(ui(root,&json!({"action":"pin","chat_id":cid,"pinned":"yes"})).is_err());
        ui(root,&json!({"action":"pin","chat_id":cid,"pinned":false})).unwrap();
        assert_eq!(read()["pinned"],false);
    }

    #[test]
    fn archive_and_delete_conversations() {
        let dir=tempfile::tempdir().unwrap();let root=dir.path();
        let older=ui(root,&json!({"action":"create","title":"older"})).unwrap()["session"]["id"].as_str().unwrap().to_string();
        let newer=ui(root,&json!({"action":"create","title":"newer"})).unwrap()["session"]["id"].as_str().unwrap().to_string();
        // Explicit timestamps avoid relying on filesystem work crossing a millisecond.
        let mut previous=load(root,&older).unwrap();previous["updated_at"]=json!(1);save(root,&previous).unwrap();
        assert!(ui(root,&json!({"action":"archive","chat_id":newer,"archived":"yes"})).is_err());
        let archived=ui(root,&json!({"action":"archive","chat_id":newer,"archived":true})).unwrap();
        assert_eq!(archived["session"]["archived"],true);
        let list=ui(root,&json!({"action":"list"})).unwrap();
        assert_eq!(list["sessions"][0]["id"],*older,"archived conversations sort last");
        assert_eq!(list["sessions"][1]["archived"],true);
        ui(root,&json!({"action":"archive","chat_id":newer,"archived":false})).unwrap();
        assert_eq!(ui(root,&json!({"action":"list"})).unwrap()["sessions"][0]["id"],*newer);

        // Delete removes the record, Markdown, sidecars, pasted images and assets only for that chat.
        let d=root.join(DIR);
        fs::write(d.join(format!("{newer}.activity.json")),b"{}").unwrap();
        fs::write(d.join(format!("{newer}-0f1e2d3c-0000-4000-8000-000000000000.png")),b"png").unwrap();
        let assets=root.join(ASSET_DIR).join(&newer);fs::create_dir_all(&assets).unwrap();fs::write(assets.join("a.txt"),b"a").unwrap();
        let other_assets=root.join(ASSET_DIR).join(&older);fs::create_dir_all(&other_assets).unwrap();
        let deleted=ui(root,&json!({"action":"delete","chat_id":newer})).unwrap();
        assert_eq!(deleted["deleted"],true);
        for name in [format!("{newer}.json"),format!("{newer}.md"),format!("{newer}.activity.json"),format!("{newer}-0f1e2d3c-0000-4000-8000-000000000000.png")] {
            assert!(!d.join(&name).exists(),"{name} should be deleted");
        }
        assert!(!assets.exists());
        assert!(d.join(format!("{older}.json")).exists());
        assert!(other_assets.exists());
        let list=ui(root,&json!({"action":"list"})).unwrap();
        assert_eq!(list["sessions"].as_array().unwrap().len(),1);
        assert!(ui(root,&json!({"action":"delete","chat_id":newer})).is_err());

        // A conversation with a live AI attachment cannot be deleted until it is detached.
        tool(root,"chat_open",&json!({"chat_id":older,"agent_name":"Tester"})).unwrap();
        assert!(ui(root,&json!({"action":"delete","chat_id":older})).is_err());
        ui(root,&json!({"action":"detach","chat_id":older})).unwrap();
        ui(root,&json!({"action":"delete","chat_id":older})).unwrap();
        assert!(!d.join(format!("{older}.json")).exists());
    }

    #[test]
    fn invalid_deletion_assets_preserve_chat_and_operations() {
        let dir = tempfile::tempdir().unwrap(); let root = dir.path();
        let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].as_str().unwrap().to_string();
        let record = file(root, &cid).unwrap(); let original = fs::read(&record).unwrap();
        let operations = root.join(DIR).join(format!("{cid}.operations.json"));
        fs::write(&operations, b"[]").unwrap();
        fs::create_dir_all(root.join(ASSET_DIR)).unwrap();
        fs::write(root.join(ASSET_DIR).join(&cid), b"invalid directory fixture").unwrap();
        assert!(ui(root, &json!({"action":"delete","chat_id":cid})).is_err());
        assert_eq!(fs::read(record).unwrap(), original);
        assert_eq!(fs::read(operations).unwrap(), b"[]");
        assert!(root.join(DIR).join(format!("{cid}.md")).exists());
    }

    #[tokio::test]
    async fn split_outbox_pauses_for_confirmation_then_resumes_in_order() {
        let dir=tempfile::tempdir().unwrap();let root=dir.path();
        let session=ui(root,&json!({"action":"create"})).unwrap();let cid=&session["session"]["id"];
        let opened=tool(root,"chat_open",&json!({"chat_id":cid})).unwrap();let aid=&opened["attachment_id"];
        let args=json!({"chat_id":cid,"attachment_id":aid,"timeout_ms":0});
        let send=|id:&str|ui(root,&json!({"action":"send","chat_id":cid,"message_id":id,"text":id})).unwrap();
        let reply=|id:&str,to:&str,confirm:bool|tool(root,"chat_reply",&json!({"chat_id":cid,"attachment_id":aid,"message_id":id,"reply_to":to,"text":id,"final":true,"awaiting_user":confirm})).unwrap();
        send("u1");send("u2");send("u3");
        ui(root,&json!({"action":"set_queue_mode","chat_id":cid,"mode":"split"})).unwrap();
        assert!(ui(root,&json!({"action":"set_queue_mode","chat_id":cid,"mode":"bad"})).is_err());
        reply("question","u1",true);
        assert_eq!(wait(root,&args).await.unwrap()["status"],"idle");
        send("confirmation");
        assert_eq!(wait(root,&args).await.unwrap()["message"]["id"],"confirmation");
        let read=||ui(root,&json!({"action":"read","chat_id":cid})).unwrap()["session"].clone();
        assert_eq!(read()["queued_messages"].as_array().unwrap().len(),2);
        reply("confirmed","confirmation",false);
        assert_eq!(wait(root,&args).await.unwrap()["message"]["id"],"u2");
        assert_eq!(wait(root,&args).await.unwrap()["message"]["id"],"u2");
        assert_eq!(read()["queued_messages"].as_array().unwrap().len(),1);
        reply("a2","u2",false);
        assert_eq!(wait(root,&args).await.unwrap()["message"]["id"],"u3");
        reply("a3","u3",false);
        assert_eq!(wait(root,&args).await.unwrap()["status"],"idle");
    }

    #[test]
    fn chunk_uploads_preserve_large_files_and_stable_labels() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let temp = tempfile::tempdir().unwrap(); let root = temp.path();
        let session = ui(root,&json!({"action":"create"})).unwrap();let cid = session["session"]["id"].clone();
        let mut bytes = vec![7u8;3*1024*1024+17];bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        let chunk = |offset:usize| json!({"action":"upload_chunk","chat_id":cid,"upload_id":"large","name":"image.png","offset":offset,"total_size":bytes.len(),"data_base64":STANDARD.encode(&bytes[offset..(offset+CHUNK_BYTES).min(bytes.len())])});
        assert!(ui(root,&chunk(0)).unwrap()["attachment"].is_null());
        assert_eq!(ui(root,&chunk(0)).unwrap()["next_offset"],CHUNK_BYTES);
        let mut wrong=chunk(0);wrong["data_base64"]=json!("YQ==");assert!(ui(root,&wrong).is_err());
        assert!(ui(root,&chunk(2*CHUNK_BYTES)).is_err());
        let mut result=json!(null);for offset in (CHUNK_BYTES..bytes.len()).step_by(CHUNK_BYTES){result=ui(root,&chunk(offset)).unwrap();}
        let file=&result["attachment"];assert_eq!(file["label"],"图片1");assert_eq!(fs::read(root.join(file["path"].as_str().unwrap())).unwrap(),bytes);
        assert_eq!(ui(root,&chunk(bytes.len()-17)).unwrap()["attachment"]["id"],"large");
        let mut ids=vec![json!("large")];for i in 0..6 {let id=format!("extra{i}");let f=ui(root,&json!({"action":"upload_chunk","chat_id":cid,"upload_id":id,"name":"report.txt","offset":0,"total_size":1,"data_base64":"YQ=="})).unwrap();assert_eq!(f["attachment"]["label"],format!("文件{}",i+1));ids.push(json!(id));}
        let sent=ui(root,&json!({"action":"send","chat_id":cid,"message_id":"many","text":"参考@图片1和@文件6","attachment_ids":ids})).unwrap();assert_eq!(sent["session"]["messages"][0]["attachments"].as_array().unwrap().len(),7);
        let part=ui(root,&json!({"action":"read_attachment_chunk","chat_id":cid,"upload_id":"large","offset":0})).unwrap();assert_eq!(STANDARD.decode(part["data_base64"].as_str().unwrap()).unwrap(),bytes[..CHUNK_BYTES]);
        assert!(markdown(&sent["session"]).contains("Reference: @图片1"));
    }

    #[test]
    fn keepalive_attachment_survives_lease_expiry_until_another_ai_takes_over() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let session = ui(root, &json!({"action":"create"})).unwrap();
        let cid = session["session"]["id"].as_str().unwrap().to_string();
        let open = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap();
        let args = json!({"chat_id":cid,"attachment_id":open["attachment_id"]});
        let expire = || {
            let mut s = load(root, &cid).unwrap();
            s["lease_until"] = json!(0);
            save(root, &s).unwrap();
        };
        expire();
        assert!(tool(root, "chat_wait", &args).is_ok());
        let public_id=ui(root,&json!({"action":"read","chat_id":cid})).unwrap()["session"]["connection_id"].clone();
        assert_eq!(public_id.as_str().unwrap().len(),12);assert_ne!(public_id,args["attachment_id"]);
        ui(root,&json!({"action":"send","chat_id":cid,"message_id":"long-task","text":"Work"})).unwrap();
        tool(root,"chat_wait",&args).unwrap();expire();
        assert_eq!(tool(root,"chat_reply",&json!({"chat_id":cid,"attachment_id":args["attachment_id"],"reply_to":"long-task","message_id":"late-result","text":"Done","final":true})).unwrap()["persisted"],true);
        assert_eq!(ui(root,&json!({"action":"read","chat_id":cid})).unwrap()["session"]["connection_id"],public_id);
        expire();
        assert_eq!(tool(root, "chat_open", &args).unwrap()["attachment_id"], open["attachment_id"]);
        expire();
        let other = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap();
        assert_ne!(other["attachment_id"], open["attachment_id"]);
        assert!(tool(root, "chat_wait", &args).is_err());
    }

    #[test]
    fn group_reconnect_resumes_after_heartbeat_but_rejects_paused_members() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path();
        let session=ui(root,&json!({"action":"create","mode":"group"})).unwrap();let cid=&session["session"]["id"];
        let joined=tool(root,"chat_open",&json!({"chat_id":cid,"agent_name":"Tester"})).unwrap();
        let args=json!({"chat_id":cid,"attachment_id":joined["attachment_id"]});
        let mut stored=load(root,cid.as_str().unwrap()).unwrap();stored["members"][0]["lease_until"]=json!(0);save(root,&stored).unwrap();
        assert!(tool(root,"chat_wait",&args).is_ok());
        ui(root,&json!({"action":"detach_member","chat_id":cid,"member_id":joined["agent_id"]})).unwrap();
        assert!(tool(root,"chat_wait",&args).is_err());assert!(tool(root,"chat_open",&args).is_err());
        ui(root,&json!({"action":"resume_member","chat_id":cid,"member_id":joined["agent_id"]})).unwrap();
        assert_eq!(tool(root,"chat_open",&args).unwrap()["agent_id"],joined["agent_id"]);
        ui(root,&json!({"action":"detach","chat_id":cid})).unwrap();
        assert!(tool(root,"chat_open",&args).is_err());assert!(tool(root,"chat_wait",&args).is_err());
        ui(root,&json!({"action":"set_mode","chat_id":cid,"mode":"work"})).unwrap();
        assert!(tool(root,"chat_open",&args).is_err());
        assert_ne!(tool(root,"chat_open",&json!({"chat_id":cid})).unwrap()["attachment_id"],joined["attachment_id"]);
    }

    #[test]
    fn prepared_group_pairing_targets_only_new_member_and_resumes_once() {
        let root=tempfile::tempdir().unwrap();let root=root.path();
        let cid=ui(root,&json!({"action":"create","mode":"group"})).unwrap()["session"]["id"].clone();
        let chief=tool(root,"chat_open",&json!({"chat_id":cid,"agent_name":"Chief"})).unwrap();
        ui(root,&json!({"action":"prepare_pairing","chat_id":cid,"message_id":"member-pair"})).unwrap();
        let member=tool(root,"chat_open",&json!({"chat_id":cid,"agent_name":"Member"})).unwrap();
        assert_eq!(tool(root,"chat_wait",&json!({"chat_id":cid,"attachment_id":chief["attachment_id"]})).unwrap()["status"],"idle");
        let args=json!({"chat_id":cid,"attachment_id":member["attachment_id"]});
        assert_eq!(tool(root,"chat_wait",&args).unwrap()["message"]["id"],"member-pair");
        tool(root,"chat_reply",&json!({"chat_id":cid,"attachment_id":member["attachment_id"],"message_id":"hello","reply_to":"member-pair","text":"你好，有什么能帮到你？","final":true})).unwrap();
        tool(root,"chat_open",&args).unwrap();
        let read=ui(root,&json!({"action":"read","chat_id":cid})).unwrap();
        assert_eq!(read["session"]["messages"].as_array().unwrap().iter().filter(|m|m["kind"]=="connection_request").count(),1);
        assert_eq!(read["session"]["messages"][1]["agent_id"],member["agent_id"]);
    }

    #[test]
    fn connection_request_is_idempotent_and_preserves_first_user_title() {
        let root = tempfile::tempdir().unwrap();
        let session = ui(root.path(), &json!({"action":"create"})).unwrap();
        let chat_id = session["session"]["id"].clone();
        let request = json!({"action":"request_connection","chat_id":chat_id,"message_id":"connect-1"});
        let first = ui(root.path(), &request).unwrap();
        assert_eq!(first["session"]["messages"].as_array().unwrap().len(), 1);
        assert_eq!(first["session"]["messages"][0]["kind"], "connection_request");
        assert_eq!(ui(root.path(), &request).unwrap()["connection_request_id"], "connect-1");
        assert_eq!(ui(root.path(), &json!({"action":"request_connection","chat_id":chat_id,"message_id":"connect-2"})).unwrap()["connection_request_id"], "connect-1");
        let open = tool(root.path(), "chat_open", &json!({"chat_id":chat_id})).unwrap();
        let args = json!({"chat_id":chat_id,"attachment_id":open["attachment_id"]});
        assert_eq!(tool(root.path(), "chat_wait", &args).unwrap()["message"]["id"], "connect-1");
        assert!(ui(root.path(), &json!({"action":"send","chat_id":chat_id,"message_id":"connect-1","text":first["session"]["messages"][0]["text"]})).is_err());
        let reply = json!({"chat_id":chat_id,"attachment_id":open["attachment_id"],"message_id":"hello","reply_to":"connect-1","text":"你好，有什么能帮到你？","final":true});
        assert_eq!(tool(root.path(), "chat_reply", &reply).unwrap()["persisted"], true);
        assert_eq!(tool(root.path(), "chat_wait", &args).unwrap()["status"], "idle");
        let sent = ui(root.path(), &json!({"action":"send","chat_id":chat_id,"message_id":"work","text":"Actual work title"})).unwrap();
        assert_eq!(sent["session"]["title"], "Actual work title");
        assert!(markdown(&sent["session"]).contains("接入请求"));
        assert!(ui(root.path(), &json!({"action":"request_connection","chat_id":chat_id,"message_id":"work"})).is_err());
    }

    #[test]
    fn artifact_preview_is_read_only_and_bounded() {
        let temp = tempfile::tempdir().unwrap(); let root = temp.path();
        let session = ui(root, &json!({"action":"create"})).unwrap(); let cid = &session["session"]["id"];
        fs::create_dir_all(root.join("mcp-assistant/artifacts")).unwrap();
        let bytes = b"\x89PNG\r\n\x1a\nfixture";
        fs::write(root.join("mcp-assistant/artifacts/preview.png"), bytes).unwrap();
        ui(root, &json!({"action":"close","chat_id":cid})).unwrap();
        let session_path = root.join(format!("{DIR}/{}.json", cid.as_str().unwrap())); let before = fs::read(&session_path).unwrap();
        let read = |path: &str| ui(root, &json!({"action":"read_artifact","chat_id":cid,"source_path":path}));
        assert_eq!(read("mcp-assistant/artifacts/preview.png").unwrap()["mime"], "image/png");
        assert_eq!(fs::read(&session_path).unwrap(), before);
        for path in ["../preview.png", "mcp-assistant/artifacts/../preview.png", "mcp-assistant/artifacts/x:stream", "C:/preview.png"] { assert!(read(path).is_err()); }
        fs::write(root.join("mcp-assistant/artifacts/fake.png"), b"not an image").unwrap(); assert!(read("mcp-assistant/artifacts/fake.png").is_err());
        fs::write(root.join("mcp-assistant/artifacts/huge.png"), vec![0; MAX_FILE_BYTES+1]).unwrap(); assert!(read("mcp-assistant/artifacts/huge.png").is_err());
        #[cfg(unix)] { std::os::unix::fs::symlink(root.join("mcp-assistant/artifacts/preview.png"),root.join("mcp-assistant/artifacts/link.png")).unwrap(); assert!(read("mcp-assistant/artifacts/link.png").is_err()); }
    }

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
    fn snapshots_remain_readable_under_writer_lock() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(ui(root, &json!({"action":"list"})).unwrap()["sessions"], json!([]));
        assert!(!root.join(DIR).exists());
        let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        ui(root, &json!({"action":"upload","chat_id":cid,"upload_id":"snapshot","name":"notes.txt","data_base64":STANDARD.encode(b"snapshot")})).unwrap();
        let record = file(root, cid.as_str().unwrap()).unwrap();
        let before = fs::read(&record).unwrap();
        let guard = lock(root).unwrap();
        assert_eq!(ui(root, &json!({"action":"list"})).unwrap()["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(ui(root, &json!({"action":"read","chat_id":cid})).unwrap()["session"]["id"], cid);
        assert_eq!(ui(root, &json!({"action":"read_attachment","chat_id":cid,"upload_id":"snapshot"})).unwrap()["data_base64"], STANDARD.encode(b"snapshot"));
        assert_eq!(fs::read(&record).unwrap(), before);
        assert!(ui(root, &json!({"action":"create"})).is_err());
        assert!(root.join(DIR).join(".lock").exists());
        fs::write(&record, b"broken").unwrap();
        assert!(ui(root, &json!({"action":"list"})).is_err());
        fs::remove_file(&record).unwrap();
        assert_eq!(ui(root, &json!({"action":"list"})).unwrap()["sessions"], json!([]));
        assert!(ui(root, &json!({"action":"read","chat_id":cid})).is_err());
        drop(guard);
    }
    #[test]
    fn concurrent_writers_preserve_messages_during_snapshot_reads() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        std::thread::scope(|scope| {
            let mut workers = Vec::new();
            for worker in 0..3 {
                let cid = &cid;
                workers.push(scope.spawn(move || {
                    for n in 0..12 {
                        ui(root, &json!({"action":"send","chat_id":cid,"message_id":format!("writer-{worker}-{n}"),"text":"message"})).unwrap();
                    }
                }));
            }
            while workers.iter().any(|worker| !worker.is_finished()) {
                assert_eq!(ui(root, &json!({"action":"list"})).unwrap()["sessions"].as_array().unwrap().len(), 1);
                assert_eq!(ui(root, &json!({"action":"read","chat_id":cid})).unwrap()["session"]["id"], cid);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        let s = load(root, cid.as_str().unwrap()).unwrap();
        let messages = s["messages"].as_array().unwrap().iter().chain(s["queue"].as_array().unwrap().iter()).collect::<Vec<_>>();
        assert_eq!(messages.len(), 36);
        assert_eq!(messages.iter().map(|m| m["id"].as_str().unwrap()).collect::<HashSet<_>>().len(), 36);
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
        assert!(ui(root, &json!({"action":"create"})).is_ok());
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
    #[test]
    fn disk_pool_and_local_references_preserve_legacy_and_integrity() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let dir = tempfile::tempdir().unwrap(); let root = dir.path();
        let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let opened = tool(root, "chat_open", &json!({"chat_id":cid})).unwrap();
        let encoded = STANDARD.encode(vec![7u8; 1024 * 1024]); let mut first = Value::Null;
        for i in 0..33 {
            let f = ui(root, &json!({"action":"upload","chat_id":cid,"upload_id":format!("pool-{i}"),"name":"asset.bin","data_base64":encoded})).unwrap()["attachment"].clone();
            assert!(f["path"].as_str().unwrap().starts_with(ASSET_DIR)); if i == 0 { first = f; }
        }
        let mut s = load(root, cid.as_str().unwrap()).unwrap();
        let legacy = format!("{DIR}/{}-{}.bin", cid.as_str().unwrap(), first["id"].as_str().unwrap());
        fs::rename(root.join(first["path"].as_str().unwrap()), root.join(&legacy)).unwrap();
        s["files"][0]["path"] = json!(legacy); save(root, &s).unwrap();
        assert_eq!(ui(root, &json!({"action":"read_attachment","chat_id":cid,"upload_id":first["id"]})).unwrap()["data_base64"], encoded);
        s["files"][0]["path"] = json!("../outside.bin"); save(root,&s).unwrap();
        assert!(ui(root, &json!({"action":"read_attachment","chat_id":cid,"upload_id":first["id"]})).is_err());
        fs::create_dir_all(root.join("mcp-assistant/artifacts")).unwrap();
        let relative = "mcp-assistant/artifacts/large.bin"; fs::write(root.join(relative),vec![9u8;3*1024*1024]).unwrap();
        let request = json!({"chat_id":cid,"attachment_id":opened["attachment_id"],"upload_id":"reference","name":"large.bin","source_path":relative});
        let f = tool(root,"chat_upload",&request).unwrap()["attachment"].clone();
        assert_eq!(f["local_reference"],true); assert_eq!(f["size"],3*1024*1024);
        assert_eq!(tool(root,"chat_upload",&request).unwrap()["attachment"],f);
        assert_eq!(ui(root,&json!({"action":"read_attachment","chat_id":cid,"upload_id":"reference"})).unwrap()["local_only"],true);
        for value in ["../secret","/etc/passwd","mcp-assistant/artifacts/../secret","mcp-assistant/artifacts/x:stream","mcp-assistant/artifacts//secret"] {
            let mut bad=request.clone();bad["source_path"]=json!(value);assert!(tool(root,"chat_upload",&bad).is_err());
        }
        let mut both=request.clone();both["data_base64"]=json!("YQ==");assert!(tool(root,"chat_upload",&both).is_err());
        fs::write(root.join(relative),b"changed").unwrap();assert!(tool(root,"chat_upload",&request).is_err());
        assert!(ui(root,&json!({"action":"read_attachment","chat_id":cid,"upload_id":"reference"})).is_err());
        #[cfg(unix)] { std::os::unix::fs::symlink(root.join(relative),root.join("mcp-assistant/artifacts/link")).unwrap();let mut bad=request.clone();bad["source_path"]=json!("mcp-assistant/artifacts/link");assert!(tool(root,"chat_upload",&bad).is_err()); }
    }

}

/// Mutate only the active user's plan under the same lock as messages and leases.
pub fn plan(root:&Path,name:&str,args:&Value)->Result<Value>{
 let _lock=lock(root)?;let mut session=load(root,id(&args["chat_id"])?)?;owned(&session,args)?;
 if session["closed"]==true{return Err(err("Conversation is closed"));}
 let reply_to=id(&args["reply_to"])?;
 let message=delivery(&session,args,false)?.filter(|m|m["id"]==reply_to&&m["kind"]!="connection_request").ok_or_else(||err("Plan must address the current unanswered user message"))?;
 let actor=if group::grouped(&session){group::member_for(&session,args,false)?["id"].clone()}else{Value::Null};
 let previous=if actor.is_null(){message["task_plan"].clone()}else{message["agent_plans"].as_array().and_then(|ps|ps.iter().find(|p|p["agent_id"]==actor)).map(|p|p["plan"].clone()).unwrap_or(Value::Null)};
 let next=super::chat_plan::reduce(&previous,name,args,now())?;
 let stored=session["messages"].as_array_mut().unwrap().iter_mut().find(|m|m["id"]==reply_to).unwrap();
 if !actor.is_null(){if stored["agent_plans"].is_null(){stored["agent_plans"]=json!([]);}let plans=stored["agent_plans"].as_array_mut().unwrap();plans.retain(|p|p["agent_id"]!=actor);if !next.is_null(){plans.push(json!({"agent_id":actor,"plan":next}));}}
 else if next.is_null(){stored.as_object_mut().unwrap().remove("task_plan");}else{stored["task_plan"]=next.clone();}
 session["updated_at"]=json!(now());group::renew(&mut session,args)?;save(root,&session)?;
 let mut result=json!({"ok":true,"persisted":true,"chat_id":session["id"],"reply_to":reply_to,"plan":super::chat_plan::summary(&next)});if name=="report_progress"{result["progress"]=next["progress"].clone();}Ok(result)
}

#[cfg(test)]
mod metadata_tests {
    use super::*;
    fn create(root: &Path) -> String {
        ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].as_str().unwrap().into()
    }
    fn read(root: &Path, cid: &str) -> Value {
        ui(root, &json!({"action":"read","chat_id":cid})).unwrap()["session"].clone()
    }
    fn open(root: &Path, cid: &str, name: &str) -> Value {
        tool(root,"chat_open",&json!({"chat_id":cid,"agent_name":name})).unwrap()
    }
    #[test]
    fn agent_names_reuse_gaps_and_preserve_resumes_and_custom_titles() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path();
        let a=create(root);let b=create(root);let c=create(root);
        let first=open(root,&a,"CodeRabbit");open(root,&b,"CodeRabbit");open(root,&c,"CodeRabbit");
        assert_eq!(read(root,&a)["title"],"CodeRabbit");assert_eq!(read(root,&b)["title"],"CodeRabbit2");assert_eq!(read(root,&c)["title"],"CodeRabbit3");
        ui(root,&json!({"action":"send","chat_id":b,"message_id":"u","text":"Keep the AI title"})).unwrap();assert_eq!(read(root,&b)["title"],"CodeRabbit2");
        ui(root,&json!({"action":"detach","chat_id":b})).unwrap();ui(root,&json!({"action":"delete","chat_id":b})).unwrap();let d=create(root);assert_eq!(open(root,&d,"CodeRabbit")["session"]["title"],"CodeRabbit2");assert_eq!(read(root,&c)["title"],"CodeRabbit3");
        let resumed=tool(root,"chat_open",&json!({"chat_id":a,"attachment_id":first["attachment_id"]})).unwrap();assert_eq!(resumed["session"]["title"],"CodeRabbit");
        assert!(tool(root,"chat_open",&json!({"chat_id":a,"agent_name":"OtherAI"})).is_err());assert_eq!(read(root,&a)["agent_name"],"CodeRabbit");
        ui(root,&json!({"action":"rename","chat_id":c,"title":"My title"})).unwrap();ui(root,&json!({"action":"detach","chat_id":c})).unwrap();assert_eq!(open(root,&c,"OtherAI")["session"]["title"],"My title");
    }
    #[test]
    fn group_names_reserve_archived_titles_and_ignore_members() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path();let a=create(root);
        ui(root,&json!({"action":"rename","chat_id":a,"title":"CodeRabbit"})).unwrap();ui(root,&json!({"action":"archive","chat_id":a,"archived":true})).unwrap();
        let g=ui(root,&json!({"action":"create","mode":"group"})).unwrap()["session"]["id"].as_str().unwrap().to_string();let chief=open(root,&g,"CodeRabbit");assert_eq!(chief["session"]["title"],"CodeRabbit2");open(root,&g,"Helper");assert_eq!(read(root,&g)["title"],"CodeRabbit2");
        assert_eq!(tool(root,"chat_open",&json!({"chat_id":g,"attachment_id":chief["attachment_id"]})).unwrap()["session"]["title"],"CodeRabbit2");
    }
    #[test]
    fn note_roundtrip_does_not_change_title_messages_or_identity() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path();let cid=create(root);open(root,&cid,"CodeRabbit");
        ui(root,&json!({"action":"send","chat_id":cid,"message_id":"u","text":"Original"})).unwrap();let before=load(root,&cid).unwrap();
        let updated=ui(root,&json!({"action":"set_note","chat_id":cid,"note":"  构建\n 服务  "})).unwrap();assert_eq!(updated["session"]["note"],"构建 服务");
        let saved=load(root,&cid).unwrap();for field in ["title","messages","attachment_id"]{assert_eq!(saved[field],before[field]);}
        assert_eq!(ui(root,&json!({"action":"list"})).unwrap()["sessions"][0]["note"],"构建 服务");assert!(markdown(&saved).contains("备注：构建 服务"));
        for note in [Value::Null,json!(3),json!("中".repeat(334))]{assert!(ui(root,&json!({"action":"set_note","chat_id":cid,"note":note})).is_err());}
        assert_eq!(read(root,&cid)["note"],"构建 服务");ui(root,&json!({"action":"set_note","chat_id":cid,"note":""})).unwrap();assert_eq!(read(root,&cid)["note"],"");
    }
}
