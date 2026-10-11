//! Folder-scoped seed capabilities and durable assignment ownership.
use super::*;
use serde::{Deserialize, Serialize};
static EXCHANGES: OnceLock<Mutex<std::collections::HashMap<String, (String, u64)>>> =
    OnceLock::new();
const FILE: &str = "docs/chat-sessions/seed-library.seeds.json";
pub const SUSPECT_MS: u64 = 600_000;
pub const REPLACE_MS: u64 = 720_000;
#[derive(Clone, Serialize, Deserialize)]
pub struct Seed {
    pub id: String,
    pub account: String,
    pub repo_id: String,
    pub branch: String,
    pub created_at: u64,
    pub enrollment_hash: String,
    pub access_hash: String,
    pub enrollment_until: u64,
    pub redeemed_at: u64,
    pub access_until: u64,
    pub last_seen: u64,
    pub ready: bool,
    pub retired_at: u64,
    pub reason: String,
    pub archived: bool,
    pub host_task_id: String,
    pub chat_id: String,
    pub attachment_id: String,
    pub generation: u64,
    pub inflight: Vec<String>,
    pub operations: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Library {
    version: u8,
    enabled: bool,
    seeds: Vec<Seed>,
}
fn read(root: &Path) -> Result<Library> {
    let target = safe(root, FILE)?;
    let bytes = match fs::read(target) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Library {
                version: 1,
                enabled: false,
                seeds: vec![],
            })
        }
        Err(e) => return Err(io(e)),
    };
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(err("Seed registry exceeds limit"));
    }
    let data: Library = serde_json::from_slice(&bytes).map_err(|_| err("Invalid seed registry"))?;
    if data.version != 1 || data.seeds.len() > 1000 {
        return Err(err("Invalid seed registry"));
    }
    for s in &data.seeds {
        id(&json!(s.id))?;
    }
    Ok(data)
}
fn write(root: &Path, data: &Library) -> Result<()> {
    let bytes = serde_json::to_vec(data).map_err(|_| err("Invalid seed registry"))?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(err("Seed registry exceeds limit"));
    }
    atomic(root, "seed-library", "seeds.json", &bytes)
}
pub fn enabled(root: &Path) -> Result<bool> {
    Ok(read(root)?.enabled)
}
fn retire(s: &mut Seed, reason: &str, time: u64) {
    s.retired_at = time;
    s.reason = reason.into();
    s.ready = false;
}
fn eligible(chat: &Value) -> bool {
    let messages = chat["messages"].as_array();
    let last = messages
        .into_iter()
        .flatten()
        .rev()
        .find(|m| m["role"] == "user");
    let awaiting = last.is_some_and(|u| {
        messages.into_iter().flatten().any(|m| {
            m["role"] == "assistant"
                && m["reply_to"] == u["id"]
                && m["final"] == true
                && m["awaiting_user"] == true
        })
    });
    chat["seed_auto"] == true
        && chat["closed"] != true
        && chat["archived"] != true
        && !group::grouped(chat)
        && !awaiting
}
fn public(s: &Seed, time: u64) -> Value {
    let status = if s.retired_at > 0 {
        "retired"
    } else if !s.inflight.is_empty() || !s.operations.is_empty() {
        "working"
    } else if s.redeemed_at == 0 {
        "preparing"
    } else if time.saturating_sub(s.last_seen) > SUSPECT_MS {
        "offline"
    } else if !s.chat_id.is_empty() {
        "assigned"
    } else if s.ready {
        "ready"
    } else {
        "preparing"
    };
    json!({"id":s.id,"account":s.account,"repo_id":s.repo_id,"branch":s.branch,"created_at":s.created_at,"last_seen":s.last_seen,"status":status,"reason":s.reason,"archived":s.archived,"retired_at":s.retired_at,"host_task_id":s.host_task_id,"chat_id":s.chat_id,"generation":s.generation,"unresolved_operations":s.inflight.len()+s.operations.len()})
}
/// Caller owns the chat lock; no nested locking.
pub fn ui(root: &Path, args: &Value) -> Result<Value> {
    let mut data = read(root)?;
    let time = now();
    match args["action"].as_str().unwrap_or("") {
        "seed_settings" => {
            data.enabled = args["enabled"]
                .as_bool()
                .ok_or_else(|| err("enabled must be boolean"))?;
            write(root, &data)?;
        }
        "seed_batch" => {
            let count = args["count"]
                .as_u64()
                .filter(|c| *c >= 1 && *c <= 50)
                .ok_or_else(|| err("Batch must contain 1–50 seeds"))?;
            if data.seeds.len() + count as usize > 1000 {
                return Err(err("Seed library limit 1000"));
            }
            let account = text(&args["account"], 200)?;
            let repo_id = text(&args["repo_id"], 200)?;
            let branch = text(&args["branch"], 200)?;
            let mut batch = vec![];
            if data
                .seeds
                .iter()
                .any(|s| s.repo_id != repo_id || s.branch != branch)
            {
                return Err(err("This folder is bound to another repository or branch; use a separate project folder"));
            }
            for _ in 0..count {
                let ticket = format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                );
                let s = Seed {
                    id: uuid::Uuid::new_v4().to_string(),
                    account: account.clone(),
                    repo_id: repo_id.clone(),
                    branch: branch.clone(),
                    created_at: time,
                    enrollment_hash: digest(ticket.as_bytes()),
                    access_hash: String::new(),
                    enrollment_until: time + 3600_000,
                    redeemed_at: 0,
                    access_until: 0,
                    last_seen: 0,
                    ready: false,
                    retired_at: 0,
                    reason: String::new(),
                    archived: false,
                    host_task_id: String::new(),
                    chat_id: String::new(),
                    attachment_id: String::new(),
                    generation: 0,
                    inflight: vec![],
                    operations: vec![],
                };
                batch.push(json!({"seed_id":s.id,"ticket":ticket,"repo_id":repo_id,"branch":branch,"account":account,"expires_at":s.enrollment_until}));
                data.seeds.push(s);
            }
            write(root, &data)?;
            return Ok(json!({"batch":batch}));
        }
        "seed_retire" => {
            let sid = id(&args["seed_id"])?;
            let s = data
                .seeds
                .iter_mut()
                .find(|s| s.id == sid)
                .ok_or_else(|| err("Seed not found"))?;
            if !s.inflight.is_empty() || !s.operations.is_empty() {
                return Err(err(
                    "Seed has unresolved operations; finish or inspect them before retirement",
                ));
            }
            if s.retired_at == 0 {
                retire(s, "manual", time);
            }
            write(root, &data)?;
        }
        "seed_cleanup" => {
            for s in &mut data.seeds {
                if s.retired_at == 0
                    && s.chat_id.is_empty()
                    && s.inflight.is_empty()
                    && s.operations.is_empty()
                    && ((s.redeemed_at == 0 && time > s.enrollment_until)
                        || (s.redeemed_at > 0 && time.saturating_sub(s.last_seen) > REPLACE_MS))
                {
                    retire(s, "inactive", time);
                }
                if s.retired_at > 0 {
                    s.archived = true;
                }
            }
            write(root, &data)?;
        }
        "seed_resume_chat" => {
            let mut chat = load(root, id(&args["chat_id"])?)?;
            if chat["closed"] == true || group::grouped(&chat) {
                return Err(err("Only open work chats support seeds"));
            }
            chat["seed_auto"] = json!(true);
            save(root, &chat)?;
        }
        "seed_list" => (),
        _ => return Err(err("Unknown seed action")),
    }
    Ok(
        json!({"enabled":data.enabled,"seeds":data.seeds.iter().map(|s|public(s,time)).collect::<Vec<_>>(),"retired_count":data.seeds.iter().filter(|s|s.retired_at>0).count()}),
    )
}
pub fn initialize(root: &Path, sid: &str, ticket: &str) -> Result<String> {
    let _lock = lock(root)?;
    let mut data = read(root)?;
    let time = now();
    let s = data
        .seeds
        .iter_mut()
        .find(|s| s.id == sid)
        .ok_or_else(|| err("Seed unavailable"))?;
    if s.retired_at > 0
        || ticket.is_empty()
        || digest(ticket.as_bytes()) != s.enrollment_hash
        || time > s.enrollment_until
        || (s.redeemed_at > 0 && time.saturating_sub(s.redeemed_at) > 60_000)
    {
        return Err(err("Seed enrollment expired or invalid"));
    }
    let key = format!("{}:{sid}", safe(root, FILE)?.display());
    let mut exchanges = EXCHANGES
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| err("Seed exchange unavailable"))?;
    exchanges.retain(|_, (_, expires)| *expires >= time);
    if s.redeemed_at > 0 {
        return exchanges
            .get(&key)
            .map(|(token, _)| token.clone())
            .ok_or_else(|| err("Enrollment already used; initialize receipt unavailable"));
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    s.redeemed_at = time;
    s.access_until = time + 7 * 86400_000;
    s.access_hash = digest(token.as_bytes());
    s.last_seen = time;
    write(root, &data)?;
    exchanges.insert(key, (token.clone(), time + 60000));
    Ok(token)
}
fn authorized(data: &Library, sid: &str, token: &str, time: u64) -> Result<usize> {
    let index = data
        .seeds
        .iter()
        .position(|s| s.id == sid)
        .ok_or_else(|| err("Seed unavailable"))?;
    let s = &data.seeds[index];
    if s.retired_at > 0
        || s.access_hash.is_empty()
        || token.is_empty()
        || digest(token.as_bytes()) != s.access_hash
        || time > s.access_until
    {
        return Err(err("Seed access expired, retired or invalid"));
    }
    Ok(index)
}
pub fn authenticate(root: &Path, sid: &str, token: &str) -> Result<()> {
    let _lock = lock(root)?;
    authorized(&read(root)?, sid, token, now())?;
    Ok(())
}
pub fn created(root: &Path, sid: &str, ticket: &str, task: &str) -> Result<()> {
    id(&json!(task))?;
    let _lock = lock(root)?;
    let mut data = read(root)?;
    let s = data
        .seeds
        .iter_mut()
        .find(|s| s.id == sid)
        .ok_or_else(|| err("Seed unavailable"))?;
    if s.retired_at > 0
        || digest(ticket.as_bytes()) != s.enrollment_hash
        || now() > s.enrollment_until
    {
        return Err(err("Invalid enrollment"));
    }
    if !s.host_task_id.is_empty() && s.host_task_id != task {
        return Err(err("Seed already bound to another task"));
    }
    s.host_task_id = task.into();
    write(root, &data)
}
pub fn poll(root: &Path, sid: &str, token: &str) -> Result<Value> {
    poll_at(root, sid, token, now())
}
fn poll_at(root: &Path, sid: &str, token: &str, time: u64) -> Result<Value> {
    let _lock = lock(root)?;
    let mut data = read(root)?;
    let index = authorized(&data, sid, token, time)?;
    let before = serde_json::to_string(&data).map_err(|_| err("Invalid seed registry"))?;
    if time.saturating_sub(data.seeds[index].last_seen) >= 15000 {
        data.seeds[index].last_seen = time;
    }
    data.seeds[index].ready = true;
    let mut chats = vec![];
    for entry in fs::read_dir(safe(root, DIR)?).map_err(io)? {
        let filename = entry.map_err(io)?.file_name().to_string_lossy().to_string();
        if let Some(cid) = filename
            .strip_suffix(".json")
            .filter(|cid| id(&json!(cid)).is_ok())
        {
            let chat = load(root, cid)?;
            if let Some(owner) = chat["seed_owner"]
                .as_str()
                .filter(|_| {
                    chat["seed_auto"] == true
                        && !chat["attachment_id"].as_str().unwrap_or("").is_empty()
                })
                .and_then(|owner| {
                    data.seeds
                        .iter_mut()
                        .find(|s| s.id == owner && s.retired_at == 0)
                })
            {
                owner.chat_id = cid.into();
                owner.attachment_id = chat["attachment_id"].as_str().unwrap_or("").into();
                owner.generation = chat["seed_generation"].as_u64().unwrap_or(1);
            }
            chats.push(chat);
        }
    }
    if !data.seeds[index].chat_id.is_empty() {
        let seed = &data.seeds[index];
        let valid = chats.iter().any(|chat| {
            chat["id"] == seed.chat_id
                && chat["closed"] != true
                && chat["seed_auto"] == true
                && chat["seed_owner"] == seed.id
                && chat["attachment_id"] == seed.attachment_id
        });
        if !valid {
            retire(&mut data.seeds[index], "conversation_detached", time);
            write(root, &data)?;
            return Ok(json!({"status":"retired"}));
        }
    }
    if data.enabled && data.seeds[index].chat_id.is_empty() {
        chats.sort_by_key(|chat| chat["created_at"].as_u64().unwrap_or(0));
        for mut chat in chats {
            if !eligible(&chat) {
                continue;
            }
            let old = chat["seed_owner"]
                .as_str()
                .and_then(|owner| data.seeds.iter().position(|s| s.id == owner));
            if let Some(old) = old {
                let seed = &data.seeds[old];
                if !seed.inflight.is_empty() || !seed.operations.is_empty() {
                    continue;
                }
                if seed.retired_at == 0
                    && (time.saturating_sub(seed.last_seen) <= REPLACE_MS
                        || chat["lease_until"].as_u64().unwrap_or(0) > time)
                {
                    continue;
                }
                if seed.retired_at == 0 {
                    retire(&mut data.seeds[old], "activity_timeout", time);
                }
            } else if !chat["attachment_id"].as_str().unwrap_or("").is_empty() {
                continue;
            }
            write(root, &data)?;
            let attachment = uuid::Uuid::new_v4().to_string();
            let generation = chat["seed_generation"].as_u64().unwrap_or(0) + 1;
            chat["seed_owner"] = json!(sid);
            chat["seed_generation"] = json!(generation);
            chat["attachment_id"] = json!(attachment);
            chat["lease_until"] = json!(0);
            chat["seed_pending"] = json!(true);
            chat["agent_name"] = json!(format!("Seed-{}", sid.chars().take(8).collect::<String>()));
            chat["updated_at"] = json!(time);
            save(root, &chat)?;
            let s = &mut data.seeds[index];
            s.chat_id = id(&chat["id"])?.into();
            s.attachment_id = attachment;
            s.generation = generation;
            break;
        }
    }
    if serde_json::to_string(&data).map_err(|_| err("Invalid seed registry"))? != before {
        write(root, &data)?;
    }
    let s = &data.seeds[index];
    Ok(if s.chat_id.is_empty() {
        json!({"status":"idle","instruction":"Make one new independent seed_wait(timeout_ms:25000). Idle is normal. Respect host execution limits."})
    } else {
        json!({"status":"assigned","chat_id":s.chat_id,"attachment_id":s.attachment_id,"generation":s.generation,"instruction":"Call chat_open with this chat_id and attachment_id. Read the complete returned skill and history. Preserve the existing pending message, plan and user constraints; inspect files and prior operation results before resuming. Work through this MCP workspace. Do not replay completed actions or use an old host sandbox as the project source."})
    })
}
pub fn begin(
    root: &Path,
    sid: &str,
    token: &str,
    name: &str,
    args: &Value,
    request: &str,
) -> Result<Value> {
    let _lock = lock(root)?;
    let mut data = read(root)?;
    let index = authorized(&data, sid, token, now())?;
    let s = &mut data.seeds[index];
    if s.chat_id.is_empty() {
        return Err(err("Seed is not assigned; use seed_wait"));
    }
    let chat = load(root, &s.chat_id)?;
    if chat["closed"] == true
        || chat["seed_auto"] != true
        || chat["seed_owner"] != s.id
        || chat["attachment_id"] != s.attachment_id
    {
        return Err(err("Seed assignment replaced or paused"));
    }
    if chat["seed_pending"] == true && name != "chat_open" {
        return Err(err("Confirm assignment with chat_open before project work"));
    }
    if args.get("chat_id").is_some() && args["chat_id"] != s.chat_id {
        return Err(err("Seed may only access its assigned chat"));
    }
    if args.get("attachment_id").is_some() && args["attachment_id"] != s.attachment_id {
        return Err(err("Seed attachment mismatch"));
    }
    if matches!(
        name,
        "wait_command" | "send_input" | "kill_session" | "read_output"
    ) && !s
        .operations
        .iter()
        .any(|id| args["session_id"].as_str() == Some(id))
    {
        return Err(err("Process does not belong to this seed"));
    }
    let call_id = digest(request.as_bytes());
    if s.inflight.contains(&call_id) {
        return Err(err("Request already in flight; do not repeat it"));
    }
    if s.inflight.len() >= 4 {
        return Err(err("Too many seed requests in flight"));
    }
    if matches!(
        name,
        "exec_command"
            | "apply_patch"
            | "edit"
            | "edit_file"
            | "write_file"
            | "file_ops"
            | "format_files"
            | "send_input"
            | "kill_session"
    ) {
        s.inflight.push(call_id.clone());
    }
    s.last_seen = now();
    let result = json!({"chat_id":s.chat_id,"attachment_id":s.attachment_id,"call_id":call_id});
    write(root, &data)?;
    Ok(result)
}
pub fn finish(root: &Path, sid: &str, call_id: &str, args: &Value, result: &Value) -> Result<()> {
    let _lock = lock(root)?;
    let mut data = read(root)?;
    let Some(s) = data.seeds.iter_mut().find(|s| s.id == sid) else {
        return Ok(());
    };
    let running = result["process_still_running"] == true
        || matches!(result["status"].as_str(), Some("running" | "background"));
    if running {
        if let Some(session) = result["session_id"].as_str() {
            if !s.operations.iter().any(|s| s == session) {
                s.operations.push(session.into());
            }
        }
    }
    if !running && result["process_still_running"] == false {
        if let Some(session) = args["session_id"].as_str() {
            s.operations.retain(|s| s != session);
        }
    }
    if result["__unknown"] != true {
        s.inflight.retain(|id| id != call_id);
    }
    s.last_seen = now();
    write(root, &data)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Vec<Value>, Vec<String>) {
        let root = tempfile::tempdir().unwrap();
        super::super::ui(
            root.path(),
            &json!({"action":"seed_settings","enabled":true}),
        )
        .unwrap();
        let batch=super::super::ui(root.path(),&json!({"action":"seed_batch","count":3,"account":"test","repo_id":"exact","branch":"main"})).unwrap()["batch"].as_array().unwrap().clone();
        let tokens = batch
            .iter()
            .map(|s| {
                initialize(
                    root.path(),
                    s["seed_id"].as_str().unwrap(),
                    s["ticket"].as_str().unwrap(),
                )
                .unwrap()
            })
            .collect();
        (root, batch, tokens)
    }
    #[test]
    fn seed_exchange_is_scoped_and_no_secrets_are_stored() {
        let (root, batch, tokens) = fixture();
        let sid = batch[0]["seed_id"].as_str().unwrap();
        let ticket = batch[0]["ticket"].as_str().unwrap();
        assert_eq!(initialize(root.path(), sid, ticket).unwrap(), tokens[0]);
        assert!(authenticate(root.path(), sid, ticket).is_err());
        assert!(authenticate(root.path(), sid, &tokens[0]).is_ok());
        let archive = fs::read_to_string(safe(root.path(), FILE).unwrap()).unwrap();
        assert!(!archive.contains(ticket));
        assert!(!archive.contains(&tokens[0]));
        let public = super::super::ui(root.path(), &json!({"action":"seed_list"}))
            .unwrap()
            .to_string();
        assert!(!public.contains("hash"));
        assert!(super::super::ui(root.path(),&json!({"action":"seed_batch","count":51,"account":"test","repo_id":"exact","branch":"main"})).is_err());
        assert_eq!(
            super::super::ui(root.path(), &json!({"action":"list"})).unwrap()["sessions"],
            json!([])
        );
    }
    #[test]
    fn seed_replacement_preserves_chat_and_rejects_old_identity() {
        let (root, batch, tokens) = fixture();
        let sid = |i: usize| batch[i]["seed_id"].as_str().unwrap();
        let chat =
            super::super::ui(root.path(), &json!({"action":"create"})).unwrap()["session"].clone();
        super::super::ui(
            root.path(),
            &json!({"action":"send","chat_id":chat["id"],"message_id":"u","text":"continue"}),
        )
        .unwrap();
        let first = poll(root.path(), sid(0), &tokens[0]).unwrap();
        assert_eq!(first["chat_id"], chat["id"]);
        assert_eq!(
            poll(root.path(), sid(1), &tokens[1]).unwrap()["status"],
            "idle"
        );
        assert!(tool(root.path(), "chat_open", &json!({"chat_id":chat["id"]})).is_err());
        let second = poll_at(root.path(), sid(1), &tokens[1], now() + REPLACE_MS + 1).unwrap();
        assert_eq!(second["chat_id"], chat["id"]);
        assert_ne!(first["attachment_id"], second["attachment_id"]);
        assert!(begin(
            root.path(),
            sid(0),
            &tokens[0],
            "apply_patch",
            &json!({}),
            "old"
        )
        .is_err());
        assert_eq!(
            tool(root.path(), "chat_open", &second).unwrap()["session"]["messages"][0]["id"],
            "u"
        );
        super::super::ui(root.path(), &json!({"action":"seed_cleanup"})).unwrap();
        assert_eq!(
            super::super::ui(root.path(), &json!({"action":"seed_list"})).unwrap()["retired_count"],
            1
        );
    }
    #[test]
    fn seed_operations_and_explicit_detach_block_takeover() {
        let (root, batch, tokens) = fixture();
        let sid = |i: usize| batch[i]["seed_id"].as_str().unwrap();
        let chat =
            super::super::ui(root.path(), &json!({"action":"create"})).unwrap()["session"].clone();
        let assigned = poll(root.path(), sid(0), &tokens[0]).unwrap();
        tool(root.path(), "chat_open", &assigned).unwrap();
        let call = begin(
            root.path(),
            sid(0),
            &tokens[0],
            "exec_command",
            &json!({}),
            "run",
        )
        .unwrap();
        let call_id = call["call_id"].as_str().unwrap();
        assert_eq!(
            poll_at(root.path(), sid(1), &tokens[1], now() + REPLACE_MS + 1).unwrap()["status"],
            "idle"
        );
        assert!(super::super::ui(
            root.path(),
            &json!({"action":"seed_retire","seed_id":sid(0)})
        )
        .is_err());
        finish(
            root.path(),
            sid(0),
            call_id,
            &json!({}),
            &json!({"process_still_running":true,"session_id":"proc"}),
        )
        .unwrap();
        assert_eq!(
            poll_at(root.path(), sid(1), &tokens[1], now() + REPLACE_MS + 1).unwrap()["status"],
            "idle"
        );
        assert!(begin(
            root.path(),
            sid(0),
            &tokens[0],
            "kill_session",
            &json!({"session_id":"other"}),
            "kill"
        )
        .is_err());
        let call = begin(
            root.path(),
            sid(0),
            &tokens[0],
            "wait_command",
            &json!({"session_id":"proc"}),
            "wait",
        )
        .unwrap();
        finish(
            root.path(),
            sid(0),
            call["call_id"].as_str().unwrap(),
            &json!({"session_id":"proc"}),
            &json!({"process_still_running":false}),
        )
        .unwrap();
        super::super::ui(
            root.path(),
            &json!({"action":"detach","chat_id":chat["id"]}),
        )
        .unwrap();
        assert_eq!(
            poll_at(root.path(), sid(1), &tokens[1], now() + REPLACE_MS + 1).unwrap()["status"],
            "idle"
        );
        assert!(begin(
            root.path(),
            sid(0),
            &tokens[0],
            "apply_patch",
            &json!({}),
            "late"
        )
        .is_err());
    }
    #[test]
    fn seed_assignment_requires_ack_and_recovers_registry_write() {
        let (root, batch, tokens) = fixture();
        let sid = |i: usize| batch[i]["seed_id"].as_str().unwrap();
        super::super::ui(root.path(), &json!({"action":"create"})).unwrap();
        let registry = safe(root.path(), FILE).unwrap();
        let before = fs::read(&registry).unwrap();
        let assigned = poll(root.path(), sid(0), &tokens[0]).unwrap();
        assert_eq!(
            super::super::ui(root.path(), &json!({"action":"list"})).unwrap()["sessions"][0]
                ["status"],
            "offline"
        );
        assert!(begin(
            root.path(),
            sid(0),
            &tokens[0],
            "read_file",
            &json!({}),
            "early"
        )
        .is_err());
        fs::write(registry, before).unwrap();
        assert_eq!(
            poll(root.path(), sid(1), &tokens[1]).unwrap()["status"],
            "idle"
        );
        assert_eq!(
            poll(root.path(), sid(0), &tokens[0]).unwrap()["attachment_id"],
            assigned["attachment_id"]
        );
        assert_eq!(
            tool(root.path(), "chat_open", &assigned).unwrap()["session"]["status"],
            "connected"
        );
        assert!(begin(
            root.path(),
            sid(0),
            &tokens[0],
            "read_file",
            &json!({}),
            "ready"
        )
        .is_ok());
    }
    #[test]
    fn seed_corrupt_storage_is_not_treated_as_empty() {
        let (root, _, _) = fixture();
        fs::write(safe(root.path(), FILE).unwrap(), b"{").unwrap();
        assert!(super::super::ui(root.path(), &json!({"action":"seed_list"})).is_err());
    }
}

pub fn pending_processes(root: &Path) -> Result<Vec<(String, String)>> {
    let _lock = lock(root)?;
    Ok(read(root)?
        .seeds
        .into_iter()
        .flat_map(|s| {
            s.operations
                .into_iter()
                .map(move |session| (s.id.clone(), session))
        })
        .take(16)
        .collect())
}
pub fn process_settled(root: &Path, sid: &str, session: &str) -> Result<()> {
    let _lock = lock(root)?;
    let mut data = read(root)?;
    if let Some(s) = data.seeds.iter_mut().find(|s| s.id == sid) {
        if s.operations.iter().any(|id| id == session) {
            s.operations.retain(|id| id != session);
            write(root, &data)?;
        }
    }
    Ok(())
}
