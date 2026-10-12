//! Folder-scoped seed capabilities and durable assignment ownership.
use super::*;
use serde::{Deserialize, Serialize};
static EXCHANGES: OnceLock<Mutex<std::collections::HashMap<String, (String, u64)>>> =
    OnceLock::new();
static SCANS: OnceLock<Mutex<std::collections::HashMap<(PathBuf, String, String, String), u64>>> =
    OnceLock::new();
const FILE: &str = "docs/chat-sessions/seed-library.seeds.json";
pub const LIBRARY_LIMIT: usize = 10000;
const REGISTRY_BYTES: usize = 32 * 1024 * 1024;
pub const SUSPECT_MS: u64 = 600_000;
pub const REPLACE_MS: u64 = 720_000;
#[derive(Clone, Serialize, Deserialize)]
pub struct Seed {
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub manual: bool,
    #[serde(default)]
    pub provision_key: String,
    #[serde(default)]
    pub metadata: std::collections::BTreeMap<String, String>,
    pub id: String,
    pub account: String,
    pub repo_id: String,
    pub branch: String,
    pub created_at: u64,
    #[serde(default)]
    pub url_hash: String,
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
    /// A still-valid AI that left a conversation (disconnect or switch) and
    /// now waits in the library as an assignable standby seed.
    #[serde(default)]
    pub standby: bool,
    /// Direct-connect URL for one chat ([接入AI]): assigned only to that chat.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bound_chat: String,
}
#[derive(Serialize, Deserialize)]
struct Library {
    #[serde(default)]
    pairing_hash: String,
    version: u8,
    enabled: bool,
    seeds: Vec<Seed>,
}
fn default_source() -> String {
    "coderabbit".into()
}
fn optional_label(value: &Value) -> Result<String> {
    if value.is_null() || value == "" {
        return Ok(String::new());
    }
    let value = text(value, 200)?;
    if value.contains(['\r', '\n', '\0']) {
        return Err(err("Invalid seed label"));
    }
    Ok(value)
}
pub fn parse_filter(value: &Value) -> Result<Value> {
    if value.is_null() {
        return Ok(json!({"source":"","keyword":""}));
    }
    if !value.is_object() {
        return Err(err("Invalid seed filter"));
    }
    let mut filter = json!({"source":optional_label(&value["source"])?.to_lowercase(),"keyword":optional_label(&value["keyword"])?});
    for field in ["prefix", "account"] {
        if !value[field].is_null() {
            filter[field] = json!(optional_label(&value[field])?);
        }
    }
    if value["standby"] == true {
        filter["standby"] = json!(true);
    }
    if let Some(seed_id) = value["seed_id"].as_str().filter(|v| !v.is_empty()) {
        filter["seed_id"] = json!(text(&json!(seed_id), 80)?);
    }
    Ok(filter)
}
fn matches(seed: &Seed, filter: &Value) -> bool {
    let source = filter["source"].as_str().unwrap_or("");
    let keyword = filter["keyword"].as_str().unwrap_or("");
    (source.is_empty() || seed.source == source)
        && (keyword.is_empty() || seed.prefix.to_lowercase().contains(&keyword.to_lowercase()))
        && filter["prefix"]
            .as_str()
            .is_none_or(|prefix| seed.prefix == prefix)
        && filter["account"]
            .as_str()
            .filter(|account| !account.is_empty())
            .is_none_or(|account| seed.account == account)
        && (filter["standby"] != true || seed.standby)
        && filter["seed_id"]
            .as_str()
            .is_none_or(|seed_id| seed.id == seed_id)
}
fn read(root: &Path) -> Result<Library> {
    let target = safe(root, FILE)?;
    let bytes = match fs::read(target) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Library {
                pairing_hash: String::new(),
                version: 1,
                enabled: false,
                seeds: vec![],
            })
        }
        Err(e) => return Err(io(e)),
    };
    if bytes.len() > REGISTRY_BYTES {
        return Err(err("Seed registry exceeds limit"));
    }
    let data: Library = serde_json::from_slice(&bytes).map_err(|_| err("Invalid seed registry"))?;
    if data.version != 1 || data.seeds.len() > LIBRARY_LIMIT {
        return Err(err("Invalid seed registry"));
    }
    for s in &data.seeds {
        id(&json!(s.id))?;
    }
    Ok(data)
}
fn write(root: &Path, data: &Library) -> Result<()> {
    let bytes = serde_json::to_vec(data).map_err(|_| err("Invalid seed registry"))?;
    if bytes.len() > REGISTRY_BYTES {
        return Err(err("Seed registry exceeds limit"));
    }
    atomic(root, "seed-library", "seeds.json", &bytes)
}
fn scan_root(root: &Path) -> PathBuf {
    // UI callers and the MCP workspace can spell the same Windows root differently.
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}
pub fn invalidate(root: &Path) {
    let canonical = scan_root(root);
    if let Ok(mut scans) = SCANS.get_or_init(Default::default).lock() {
        scans.retain(|(folder, _, _, _), _| folder != &canonical);
    }
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
    json!({"enrollment_until":s.enrollment_until,"redeemed_at":s.redeemed_at,"access_until":s.access_until,"id":s.id,"source":s.source,"prefix":s.prefix,"manual":s.manual,"account":s.account,"repo_id":s.repo_id,"branch":s.branch,"metadata":s.metadata,"created_at":s.created_at,"last_seen":s.last_seen,"status":status,"reason":s.reason,"archived":s.archived,"retired_at":s.retired_at,"host_task_id":s.host_task_id,"chat_id":s.chat_id,"generation":s.generation,"unresolved_operations":s.inflight.len()+s.operations.len(),"standby":s.standby})
}
fn inventory(data: &Library, args: &Value, time: u64) -> Value {
    let mut grouped = std::collections::BTreeMap::<(String, String), (usize, usize)>::new();
    for seed in data.seeds.iter().filter(|s| s.retired_at == 0) {
        let count = grouped
            .entry((seed.source.clone(), seed.prefix.clone()))
            .or_default();
        count.0 += 1;
        if public(seed, time)["status"] == "ready" {
            count.1 += 1;
        }
    }
    let groups:Vec<_>=grouped.into_iter().map(|((source,prefix),(total,ready))|json!({"source":source,"prefix":prefix,"total":total,"ready":ready})).collect();
    let sources: std::collections::BTreeSet<_> =
        data.seeds.iter().map(|s| s.source.clone()).collect();
    let mut ready_groups = std::collections::BTreeMap::<(String, String, String), usize>::new();
    for seed in &data.seeds {
        if public(seed, time)["status"] == "ready" {
            *ready_groups
                .entry((
                    seed.source.clone(),
                    seed.prefix.clone(),
                    seed.account.clone(),
                ))
                .or_default() += 1;
        }
    }
    let ready_groups: Vec<_> = ready_groups.into_iter().map(|((source,prefix,account),ready)|json!({"source":source,"prefix":prefix,"account":account,"ready":ready})).collect();
    // Every non-retired pool, including empty ones, so a new chat can always
    // show the categories (with 0 ready) instead of hiding the choice.
    let mut pool_groups =
        std::collections::BTreeMap::<(String, String, String, bool), (usize, usize)>::new();
    for seed in data
        .seeds
        .iter()
        .filter(|s| s.retired_at == 0 && s.bound_chat.is_empty())
    {
        let entry = pool_groups
            .entry((
                seed.source.clone(),
                seed.prefix.clone(),
                seed.account.clone(),
                seed.standby,
            ))
            .or_default();
        entry.0 += 1;
        if public(seed, time)["status"] == "ready" {
            entry.1 += 1;
        }
    }
    let pool_groups: Vec<_> = pool_groups.into_iter().map(|((source,prefix,account,standby),(total,ready))|json!({"source":source,"prefix":prefix,"account":account,"standby":standby,"total":total,"ready":ready})).collect();
    let source = args["source"].as_str().unwrap_or("").trim().to_lowercase();
    let keyword = args["keyword"].as_str().unwrap_or("").trim().to_lowercase();
    let all: Vec<_> = data.seeds.iter().map(|s| public(s, time)).collect();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut accounts = std::collections::BTreeSet::new();
    for s in &all {
        *counts
            .entry(s["status"].as_str().unwrap_or("").into())
            .or_default() += 1;
        accounts.insert(s["account"].as_str().unwrap_or("").to_owned());
    }
    let search = args["search"].as_str().unwrap_or("").trim().to_lowercase();
    let filtered: Vec<_> = all
        .iter()
        .filter(|s| {
            (args["include_archived"] != false || s["archived"] != true)
                && (source.is_empty() || s["source"] == source)
                && (keyword.is_empty()
                    || s["prefix"]
                        .as_str()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&keyword))
                && args["account"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .is_none_or(|v| s["account"] == v)
                && args["status"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .is_none_or(|v| s["status"] == v)
                && (search.is_empty()
                    || [
                        "id",
                        "account",
                        "source",
                        "prefix",
                        "repo_id",
                        "branch",
                        "host_task_id",
                    ]
                    .iter()
                    .any(|k| s[k].as_str().unwrap_or("").to_lowercase().contains(&search))
                    || s["metadata"].as_object().is_some_and(|m| {
                        m.values()
                            .any(|v| v.as_str().unwrap_or("").to_lowercase().contains(&search))
                    }))
        })
        .collect();
    // Aggregate the full filtered inventory before slicing the detail page.
    let mut summaries = std::collections::BTreeMap::<
        String,
        (usize, usize, std::collections::BTreeMap<String, usize>),
    >::new();
    for seed in &filtered {
        let group = summaries
            .entry(seed["account"].as_str().unwrap_or("").to_owned())
            .or_default();
        let status = seed["status"].as_str().unwrap_or("");
        group.0 += 1;
        *group.2.entry(status.to_owned()).or_default() += 1;
        if matches!(status, "ready" | "assigned" | "working") {
            group.1 += 1;
        }
    }
    let account_summaries: Vec<_> = summaries.into_iter().map(|(account, (total, valid, counts))| json!({"account":account,"total":total,"valid":valid,"counts":counts})).collect();
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let limit = args["limit"]
        .as_u64()
        .map(|v| v.clamp(1, 200) as usize)
        .unwrap_or(all.len());
    json!({"pool_groups":pool_groups,"ready_groups":ready_groups,"account_summaries":account_summaries,"groups":groups,"sources":sources,"enabled":data.enabled,"seeds":filtered.iter().skip(offset).take(limit).collect::<Vec<_>>(),"retired_count":counts.get("retired").copied().unwrap_or(0),"total":all.len(),"filtered_total":filtered.len(),"offset":offset,"limit":limit,"counts":counts,"accounts":accounts,"pairing_enabled":!data.pairing_hash.is_empty(),"capacity":LIBRARY_LIMIT})
}
/// Caller owns the chat lock; no nested locking.
fn ensure_settled(seed: &Seed) -> Result<()> {
    if !seed.inflight.is_empty() || !seed.operations.is_empty() {
        return Err(err(
            "Seed has unresolved operations; finish or inspect them before release",
        ));
    }
    Ok(())
}
fn release_seed(root: &Path, seed: &mut Seed) -> Result<()> {
    ensure_settled(seed)?;
    if !seed.chat_id.is_empty() && safe(root, &format!("{DIR}/{}.json", seed.chat_id))?.exists() {
        let mut chat = load(root, &seed.chat_id)?;
        // Never disconnect a replacement owner or a direct manual attachment.
        if chat["seed_owner"] == seed.id && chat["attachment_id"] == seed.attachment_id {
            chat["attachment_id"] = json!("");
            chat["seed_owner"] = json!("");
            chat["seed_auto"] = json!(false);
            chat["seed_pending"] = json!(false);
            chat["seed_generation"] = json!(chat["seed_generation"].as_u64().unwrap_or(0) + 1);
            chat["updated_at"] = json!(now());
            save(root, &chat)?;
        }
    }
    seed.chat_id.clear();
    seed.attachment_id.clear();
    seed.generation += 1;
    seed.last_seen = 0;
    seed.ready = false;
    seed.reason = "workspace_removed".into();
    Ok(())
}
/// Preserve the folder's pool on disk; a stopped endpoint is offline, not ready.
pub fn release_workspace(root: &Path) -> Result<()> {
    if !safe(root, FILE)?.exists() {
        return Ok(());
    }
    let _lock = lock(root)?;
    let mut data = read(root)?;
    for seed in &data.seeds {
        ensure_settled(seed)?;
    }
    // Durable pause first: partial storage failures cannot reassign another chat.
    data.enabled = false;
    write(root, &data)?;
    // Explicit pool choices bypass `enabled`; withdraw them as well.
    if safe(root, DIR)?.is_dir() {
        for entry in fs::read_dir(safe(root, DIR)?).map_err(io)? {
            let filename = entry.map_err(io)?.file_name().to_string_lossy().to_string();
            if let Some(cid) = filename
                .strip_suffix(".json")
                .filter(|cid| id(&json!(cid)).is_ok())
            {
                // One unreadable chat must not stop the workspace release.
                let Ok(mut chat) = load(root, cid) else {
                    continue;
                };
                if chat["seed_explicit"] == true || chat["seed_direct"] == true {
                    // A pending direct URL does not survive a release: the
                    // chat returns to a plain [接入AI] state.
                    if chat["seed_direct"] == true
                        && chat["seed_owner"].as_str().unwrap_or("").is_empty()
                    {
                        chat["seed_auto"] = json!(false);
                        chat["seed_direct"] = json!(false);
                    }
                    chat["seed_explicit"] = json!(false);
                    save(root, &chat)?;
                }
            }
        }
    }
    let time = now();
    for seed in &mut data.seeds {
        if seed.retired_at == 0 && !seed.bound_chat.is_empty() && seed.chat_id.is_empty() {
            retire(seed, "workspace_released", time);
        }
    }
    for seed in &mut data.seeds {
        if seed.retired_at == 0 {
            release_seed(root, seed)?;
        }
    }
    write(root, &data)?;
    invalidate(root);
    Ok(())
}

pub fn ui(root: &Path, args: &Value) -> Result<Value> {
    if args["action"] == "seed_direct" {
        return direct_url(root, args);
    }
    let mut data = read(root)?;
    let time = now();
    match args["action"].as_str().unwrap_or("") {
        "seed_pairing" => {
            if args["enabled"] == false {
                data.pairing_hash.clear();
            } else {
                let token = args["token"].as_str().unwrap_or("");
                if token.len() != 64
                    || !token
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                {
                    return Err(err("Pairing token must contain 64 hexadecimal characters"));
                }
                data.pairing_hash = digest(token.as_bytes());
            }
            write(root, &data)?;
        }
        "seed_settings" => {
            data.enabled = args["enabled"]
                .as_bool()
                .ok_or_else(|| err("enabled must be boolean"))?;
            write(root, &data)?;
        }
        "seed_batch" | "seed_manual" => {
            let manual = args["action"] == "seed_manual";
            let source = optional_label(&args["source"])?;
            let source = if source.is_empty() {
                if manual {
                    "custom".into()
                } else {
                    default_source()
                }
            } else {
                source.to_lowercase()
            };
            let prefix = optional_label(&args["prefix"])?;
            let count_value = if manual {
                json!(1)
            } else {
                args["count"].clone()
            };
            let count = count_value
                .as_u64()
                .filter(|c| *c >= 1 && *c <= 50)
                .ok_or_else(|| err("Batch must contain 1–50 seeds"))?;
            if data.seeds.len() + count as usize > LIBRARY_LIMIT {
                return Err(err("Seed library limit 10000"));
            }
            let account = text(&args["account"], 200)?;
            let repo_id = if manual {
                String::new()
            } else {
                text(&args["repo_id"], 200)?
            };
            let branch = if manual {
                String::new()
            } else {
                text(&args["branch"], 200)?
            };
            let mut batch = vec![];
            // Host repository metadata belongs to each seed, not the whole folder.
            // Preserve existing tickets, owners and history when another host joins.
            for _ in 0..count {
                let sid = uuid::Uuid::new_v4().to_string();
                let provision_key = args["_provision_key"].as_str().unwrap_or("").to_owned();
                let ticket = if !provision_key.is_empty() {
                    digest(
                        format!(
                            "{}\0{}\0{}",
                            args["_provision_token"].as_str().unwrap_or(""),
                            provision_key,
                            sid
                        )
                        .as_bytes(),
                    )
                } else {
                    format!(
                        "{}{}",
                        uuid::Uuid::new_v4().simple(),
                        uuid::Uuid::new_v4().simple()
                    )
                };
                let url_key = digest(format!("seed-url-v1\0{ticket}").as_bytes());
                let mut metadata = std::collections::BTreeMap::new();
                for key in [
                    "account_id",
                    "provider_user_id",
                    "repo_name",
                    "organization_id",
                    "workspace_id",
                ] {
                    if let Some(value) = args["metadata"][key].as_str().filter(|v| !v.is_empty()) {
                        metadata.insert(key.to_owned(), text(&json!(value), 200)?);
                    }
                }
                let s = Seed {
                    source: source.clone(),
                    prefix: prefix.clone(),
                    manual,
                    id: sid,
                    provision_key,
                    metadata,
                    account: account.clone(),
                    repo_id: repo_id.clone(),
                    branch: branch.clone(),
                    created_at: time,
                    url_hash: digest(url_key.as_bytes()),
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
                    standby: args["_standby"] == true,
                    bound_chat: args["_bound_chat"].as_str().unwrap_or("").to_owned(),
                };
                batch.push(json!({"seed_id":s.id,"ticket":ticket,"url_key":url_key,"source":source,"prefix":prefix,"manual":manual,"repo_id":repo_id,"branch":branch,"account":account,"expires_at":s.enrollment_until}));
                data.seeds.push(s);
            }
            write(root, &data)?;
            return Ok(json!({"batch":batch}));
        }
        "seed_label" | "seed_retire" | "seed_purge" | "seed_release" => {
            let ids = if let Some(ids) = args["seed_ids"].as_array() {
                ids.clone()
            } else {
                vec![args["seed_id"].clone()]
            };
            if ids.is_empty() || ids.len() > 100 {
                return Err(err("Select 1–100 seeds"));
            }
            let mut selected = std::collections::HashSet::new();
            for value in &ids {
                let sid = id(value)?;
                if !selected.insert(sid.to_owned()) {
                    return Err(err("Duplicate seed ID"));
                }
                if !data.seeds.iter().any(|s| s.id == sid) {
                    return Err(err("Seed not found"));
                }
            }
            let action = args["action"].as_str().unwrap();
            let source = optional_label(&args["source"])?;
            let prefix = optional_label(&args["prefix"])?;
            for seed in data.seeds.iter().filter(|s| selected.contains(&s.id)) {
                if action != "seed_label" {
                    ensure_settled(seed)?;
                }
                if action == "seed_purge" && seed.retired_at == 0 {
                    return Err(err("Only retired seeds can be permanently deleted"));
                }
                if action == "seed_release" && seed.retired_at > 0 {
                    return Err(err("Retired seeds cannot be released"));
                }
            }
            for seed in data.seeds.iter_mut().filter(|s| selected.contains(&s.id)) {
                match action {
                    "seed_label" => {
                        if !source.is_empty() {
                            seed.source = source.to_lowercase();
                        }
                        seed.prefix = prefix.clone();
                    }
                    "seed_retire" => {
                        if seed.retired_at == 0 {
                            retire(seed, "manual", time);
                        }
                    }
                    "seed_release" => release_seed(root, seed)?,
                    _ => (),
                }
            }
            if action == "seed_purge" {
                data.seeds.retain(|s| !selected.contains(&s.id));
            }
            write(root, &data)?;
            invalidate(root);
            let mut result = inventory(&data, args, time);
            result["affected_count"] = json!(selected.len());
            return Ok(result);
        }
        "seed_delete_account" => {
            // One account card = every seed with that account label. Detach live chats first, then delete atomically.
            let account = args["account"].as_str().unwrap_or("").to_owned();
            if account.is_empty() || !data.seeds.iter().any(|s| s.account == account) {
                return Err(err("Account not found"));
            }
            for seed in data.seeds.iter().filter(|s| s.account == account) {
                ensure_settled(seed)?;
            }
            let mut affected = 0;
            for seed in data.seeds.iter_mut().filter(|s| s.account == account) {
                if !seed.chat_id.is_empty() {
                    release_seed(root, seed)?;
                }
                affected += 1;
            }
            data.seeds.retain(|s| s.account != account);
            write(root, &data)?;
            invalidate(root);
            let mut query = args.clone();
            query["account"] = json!("");
            let mut result = inventory(&data, &query, time);
            result["affected_count"] = json!(affected);
            return Ok(result);
        }
        "seed_reissue" => {
            let sid = id(&args["seed_id"])?;
            let seed = data
                .seeds
                .iter_mut()
                .find(|s| s.id == sid)
                .ok_or_else(|| err("Seed not found"))?;
            ensure_settled(seed)?;
            if seed.retired_at > 0
                || seed.redeemed_at > 0
                || time <= seed.enrollment_until
                || !seed.chat_id.is_empty()
            {
                return Err(err("Only expired, unconnected seeds can be reissued"));
            }
            let ticket = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            let url_key = digest(format!("seed-url-v1\0{ticket}").as_bytes());
            seed.enrollment_hash = digest(ticket.as_bytes());
            seed.url_hash = digest(url_key.as_bytes());
            seed.enrollment_until = time + 3600_000;
            seed.reason = "reissued".into();
            seed.host_task_id.clear();
            let ticket = json!({"seed_id":seed.id,"ticket":ticket,"url_key":url_key,"source":seed.source,"prefix":seed.prefix,"manual":seed.manual,"account":seed.account,"repo_id":seed.repo_id,"branch":seed.branch,"expires_at":seed.enrollment_until});
            write(root, &data)?;
            return Ok(json!({"batch":[ticket]}));
        }
        "seed_cleanup" => {
            let mut affected = 0;
            for s in &mut data.seeds {
                if s.retired_at == 0
                    && s.reason != "workspace_removed"
                    && s.chat_id.is_empty()
                    && s.inflight.is_empty()
                    && s.operations.is_empty()
                    && ((s.redeemed_at == 0 && time > s.enrollment_until)
                        || (s.redeemed_at > 0 && time.saturating_sub(s.last_seen) > REPLACE_MS))
                {
                    retire(s, "inactive", time);
                    affected += 1;
                }
                if s.retired_at > 0 {
                    s.archived = true;
                }
            }
            write(root, &data)?;
            let mut result = inventory(&data, args, time);
            result["affected_count"] = json!(affected);
            return Ok(result);
        }
        "seed_resume_chat" => {
            let mut chat = load(root, id(&args["chat_id"])?)?;
            if chat["closed"] == true || group::grouped(&chat) {
                return Err(err("Only open work chats support seeds"));
            }
            // A stale seed_owner without an attachment is replaced by
            // poll_at once that seed is gone; only a live attachment blocks.
            if !chat["attachment_id"].as_str().unwrap_or("").is_empty() {
                // Explicit pool choice for a chat that still has an AI attached
                // must go through detach first (the UI does this on switch).
                if args.get("seed_filter").is_some_and(Value::is_object) {
                    return Err(err("Disconnect the current AI before choosing a seed pool"));
                }
            }
            chat["seed_auto"] = json!(true);
            if args.get("seed_filter").is_some_and(Value::is_object) {
                chat["seed_filter"] = parse_filter(&args["seed_filter"])?;
                chat["seed_explicit"] = json!(true);
                chat["seed_direct"] = json!(false);
            }
            chat["updated_at"] = json!(time);
            save(root, &chat)?;
            invalidate(root);
        }
        "seed_list" => (),
        _ => return Err(err("Unknown seed action")),
    }
    Ok(inventory(&data, args, time))
}
/// Browser pairing is folder-scoped and only grants status/provisioning access.
/// Return a still-valid seed that owns `chat` to the library as standby.
/// Caller holds the chat lock and detaches the chat afterwards. Returns false
/// when the seed is gone, offline or busy; it is then retired as before.
pub(super) fn release_to_standby(root: &Path, chat: &Value) -> Result<bool> {
    let owner = chat["seed_owner"].as_str().unwrap_or("");
    if owner.is_empty() || !safe(root, FILE)?.exists() {
        return Ok(false);
    }
    let mut data = read(root)?;
    let time = now();
    let Some(seed) = data.seeds.iter_mut().find(|s| s.id == owner) else {
        return Ok(false);
    };
    let live = seed.retired_at == 0
        && seed.redeemed_at > 0
        && seed.chat_id == chat["id"].as_str().unwrap_or("")
        && seed.attachment_id == chat["attachment_id"].as_str().unwrap_or("")
        && seed.inflight.is_empty()
        && seed.operations.is_empty()
        && time.saturating_sub(seed.last_seen) <= SUSPECT_MS;
    if !live {
        return Ok(false);
    }
    seed.chat_id.clear();
    seed.attachment_id.clear();
    seed.generation += 1;
    seed.standby = true;
    seed.bound_chat.clear();
    seed.ready = true;
    seed.reason = "standby".into();
    write(root, &data)?;
    invalidate(root);
    Ok(true)
}
/// [接入AI] without OAuth: a one-hour URL that carries its own credential,
/// like a manual seed, but bound to one chat. The chat waits for that seed
/// only, even when automatic assignment is off.
fn direct_url(root: &Path, args: &Value) -> Result<Value> {
    let cid = id(&args["chat_id"])?.to_owned();
    let mut chat = load(root, &cid)?;
    if chat["closed"] == true || group::grouped(&chat) {
        return Err(err("Only open work chats support direct connect URLs"));
    }
    if !chat["attachment_id"].as_str().unwrap_or("").is_empty() {
        return Err(err(
            "Disconnect the current AI before connecting another one",
        ));
    }
    let time = now();
    let mut data = read(root)?;
    let mut changed = false;
    // Re-issuing replaces every URL of this chat that is not serving it, used
    // or not, so no AI is left waiting on a credential that can never match.
    // Never-used rows are dropped so retired URLs do not pile up.
    let before = data.seeds.len();
    data.seeds
        .retain(|s| !(s.bound_chat == cid && s.redeemed_at == 0));
    changed |= data.seeds.len() != before;
    for seed in data
        .seeds
        .iter_mut()
        .filter(|s| s.bound_chat == cid && s.retired_at == 0 && s.chat_id.is_empty())
    {
        retire(seed, "replaced", time);
        changed = true;
    }
    if changed {
        write(root, &data)?;
    }
    let created = ui(
        root,
        &json!({"action":"seed_manual","source":"direct","prefix":"接入AI","account":"direct","_bound_chat":cid}),
    )?;
    let ticket = created["batch"][0].clone();
    let sid = ticket["seed_id"]
        .as_str()
        .ok_or_else(|| err("Direct connect URL failed"))?;
    // A stale owner (e.g. a busy seed that was plainly detached) would block
    // the bound seed until the replacement timeout.
    if !chat["seed_owner"].as_str().unwrap_or("").is_empty() {
        chat["seed_owner"] = json!("");
        chat["seed_pending"] = json!(false);
        chat["seed_generation"] = json!(chat["seed_generation"].as_u64().unwrap_or(0) + 1);
    }
    chat["seed_auto"] = json!(true);
    chat["seed_explicit"] = json!(true);
    chat["seed_direct"] = json!(true);
    chat["seed_filter"] = json!({"source":"","keyword":"","seed_id":sid});
    chat["updated_at"] = json!(time);
    save(root, &chat)?;
    invalidate(root);
    Ok(json!({"ok":true,"seed":ticket}))
}
/// A direct-connect URL that was never used stops working when its chat is
/// disconnected or switched.
pub(super) fn retire_unused_direct(root: &Path, chat_id: &str) -> Result<()> {
    if !safe(root, FILE)?.exists() {
        return Ok(());
    }
    let mut data = read(root)?;
    let time = now();
    let mut changed = false;
    for seed in data
        .seeds
        .iter_mut()
        .filter(|s| s.bound_chat == chat_id && s.retired_at == 0 && s.chat_id.is_empty())
    {
        retire(seed, "conversation_detached", time);
        changed = true;
    }
    if changed {
        write(root, &data)?;
        invalidate(root);
    }
    Ok(())
}
/// Enroll a directly connected AI that left a chat as a standby seed. Returns
/// the one-hour URL key; the caller hands it to that AI's next chat_wait.
pub(super) fn enroll_direct_standby(root: &Path, agent_name: &str) -> Result<String> {
    let label: String = agent_name
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect::<String>()
        .trim()
        .to_owned();
    let label = if label.is_empty() {
        "AI".to_owned()
    } else {
        label
    };
    // Standby URLs that were never connected expire after an hour; retire
    // them so the standby card does not fill with dead entries.
    let mut data = read(root)?;
    let time = now();
    let mut changed = false;
    for seed in data.seeds.iter_mut().filter(|s| {
        s.standby && s.retired_at == 0 && s.redeemed_at == 0 && time > s.enrollment_until
    }) {
        retire(seed, "inactive", time);
        changed = true;
    }
    if changed {
        write(root, &data)?;
    }
    let created = ui(
        root,
        &json!({"action":"seed_manual","source":"standby","prefix":label,"account":label,"_standby":true}),
    )?;
    created["batch"][0]["url_key"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| err("Standby enrollment failed"))
}
pub fn provision(root: &Path, token: &str, args: &Value) -> Result<Value> {
    let _lock = lock(root)?;
    let data = read(root)?;
    if token.len() != 64
        || data.pairing_hash.is_empty()
        || digest(token.as_bytes()) != data.pairing_hash
    {
        return Err(err("Pairing token invalid or revoked"));
    }
    if args["action"] == "status" {
        return Ok(
            json!({"ok":true,"total":data.seeds.len(),"capacity":LIBRARY_LIMIT,"batch_limit":50}),
        );
    }
    if args["action"] != "batch" {
        return Err(err("Pairing only supports status and batch"));
    }
    let request = id(&args["request_id"])?;
    let key = digest(format!("{token}\0{request}").as_bytes());
    let existing: Vec<_> = data
        .seeds
        .iter()
        .filter(|s| s.provision_key == key)
        .collect();
    if !existing.is_empty() {
        if args["count"].as_u64() != Some(existing.len() as u64)
            || existing.iter().any(|s| {
                args["account"] != s.account
                    || args["repo_id"] != s.repo_id
                    || args["branch"] != s.branch
                    || args["prefix"].as_str().unwrap_or("").trim() != s.prefix
                    || s.retired_at > 0
                    || s.enrollment_hash
                        != digest(digest(format!("{token}\0{key}\0{}", s.id).as_bytes()).as_bytes())
                    || s.enrollment_until <= now()
            })
        {
            return Err(err(
                "Provision request changed or expired; use a new request ID",
            ));
        }
        return Ok(json!({"batch":existing.iter().map(|s| {
                let ticket = digest(format!("{token}\0{key}\0{}",s.id).as_bytes());
                let mut row = json!({"seed_id":s.id,"ticket":ticket,"source":s.source,"prefix":s.prefix,"manual":s.manual,"repo_id":s.repo_id,"branch":s.branch,"account":s.account,"expires_at":s.enrollment_until});
                if !s.url_hash.is_empty() { row["url_key"] = json!(digest(format!("seed-url-v1\0{ticket}").as_bytes())); }
                row
            }).collect::<Vec<_>>()}));
    }
    ui(
        root,
        &json!({"action":"seed_batch","source":"coderabbit","prefix":args["prefix"],"account":args["account"],"repo_id":args["repo_id"],"branch":args["branch"],"count":args["count"],"metadata":args["metadata"],"_provision_key":key,"_provision_token":token}),
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
/// URL keys are separate from public seed IDs and stored only as digests.
pub fn url(root: &Path, key: &str, redeem: bool) -> Result<String> {
    let _lock = lock(root)?;
    if key.len() != 64
        || !key
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(err("Seed URL invalid or expired"));
    }
    let mut data = read(root)?;
    let s = data
        .seeds
        .iter_mut()
        .find(|s| s.url_hash == digest(key.as_bytes()))
        .ok_or_else(|| err("Seed URL invalid or expired"))?;
    let time = now();
    if s.retired_at > 0
        || time
            > if s.redeemed_at > 0 {
                s.access_until
            } else {
                s.enrollment_until
            }
    {
        return Err(err("Seed URL invalid or expired"));
    }
    let sid = s.id.clone();
    if redeem && s.redeemed_at == 0 {
        s.redeemed_at = time;
        s.access_until = time + 7 * 86400_000;
        s.last_seen = time;
        write(root, &data)?;
        super::forget_standby_url(key);
    }
    Ok(sid)
}
pub fn skill() -> Value {
    json!({"uri":"coding-tools://skills/seed-connect","name":"seed-connect","mimeType":"text/markdown","text":include_str!("../../../skills/seed-connect/SKILL.md").replace("\r\n", "\n")})
}
fn authorized(data: &Library, sid: &str, token: &str, time: u64) -> Result<usize> {
    let index = data
        .seeds
        .iter()
        .position(|s| s.id == sid)
        .ok_or_else(|| err("Seed unavailable"))?;
    let s = &data.seeds[index];
    if s.retired_at > 0
        || s.redeemed_at == 0
        || token.is_empty()
        || (digest(token.as_bytes()) != s.access_hash && digest(token.as_bytes()) != s.url_hash)
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
    let was_ready = data.seeds[index].ready;
    let mut dirty = !was_ready;
    if time.saturating_sub(data.seeds[index].last_seen) >= 60000 {
        data.seeds[index].last_seen = time;
        dirty = true;
    }
    data.seeds[index].ready = true;
    if data.seeds[index].chat_id.is_empty() {
        let scan_key = (
            scan_root(root),
            data.seeds[index].source.clone(),
            data.seeds[index].prefix.clone(),
            data.seeds[index].account.clone(),
        );
        let mut scans = SCANS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| err("Seed scan lock unavailable"))?;
        if was_ready
            && scans
                .get(&scan_key)
                .is_some_and(|last| time >= *last && time - *last < 1000)
        {
            if dirty {
                write(root, &data)?;
            }
            return Ok(
                json!({"status":"idle","instruction":"Make one new independent wait (chat_wait or seed_wait, timeout_ms:25000). Idle is normal. The user writes in the app at any time; idle only means no new message yet, so just wait again. Respect host execution limits."}),
            );
        }
        if scans.len() > 1024 {
            scans.retain(|_, last| time.saturating_sub(*last) < 60000);
        }
        scans.insert(scan_key, time);
    }
    let mut chats = vec![];
    let own_chat = data.seeds[index].chat_id.clone();
    let mut chat_ids = vec![];
    if !own_chat.is_empty() {
        if safe(root, &format!("{DIR}/{own_chat}.json"))?.exists() {
            chat_ids.push(own_chat);
        }
    } else {
        for entry in fs::read_dir(safe(root, DIR)?).map_err(io)? {
            let filename = entry.map_err(io)?.file_name().to_string_lossy().to_string();
            if let Some(cid) = filename
                .strip_suffix(".json")
                .filter(|cid| id(&json!(cid)).is_ok())
            {
                chat_ids.push(cid.to_owned());
            }
        }
    }
    let by_id: std::collections::HashMap<_, _> = data
        .seeds
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.clone(), i))
        .collect();
    for cid in chat_ids {
        let chat = load(root, &cid)?;
        if let Some(owner) = chat["seed_owner"]
            .as_str()
            .filter(|_| {
                chat["seed_auto"] == true
                    && !chat["attachment_id"].as_str().unwrap_or("").is_empty()
            })
            .and_then(|owner| {
                by_id
                    .get(owner)
                    .and_then(|index| data.seeds.get_mut(*index))
                    .filter(|s| s.retired_at == 0)
            })
        {
            let attachment = chat["attachment_id"].as_str().unwrap_or("");
            let generation = chat["seed_generation"].as_u64().unwrap_or(1);
            if owner.chat_id != cid
                || owner.attachment_id != attachment
                || owner.generation != generation
            {
                dirty = true;
            }
            owner.chat_id = cid;
            owner.attachment_id = chat["attachment_id"].as_str().unwrap_or("").into();
            owner.generation = chat["seed_generation"].as_u64().unwrap_or(1);
        }
        chats.push(chat);
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
    if data.seeds[index].chat_id.is_empty() {
        chats.sort_by_key(|chat| chat["created_at"].as_u64().unwrap_or(0));
        for mut chat in chats {
            // The library switch only governs automatic assignment. A pool the
            // user explicitly chose for a chat is always served.
            if !data.enabled && chat["seed_explicit"] != true {
                continue;
            }
            // A direct-connect seed serves only its own chat.
            let bound = &data.seeds[index].bound_chat;
            if !bound.is_empty() && chat["id"].as_str() != Some(bound.as_str()) {
                continue;
            }
            if !eligible(&chat)
                || !matches(&data.seeds[index], &parse_filter(&chat["seed_filter"])?)
            {
                continue;
            }
            let old = chat["seed_owner"]
                .as_str()
                .and_then(|owner| by_id.get(owner).copied());
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
            s.standby = false;
            s.reason.clear();
            dirty = true;
            invalidate(root);
            break;
        }
    }
    if dirty {
        write(root, &data)?;
    }
    let s = &data.seeds[index];
    Ok(if s.chat_id.is_empty() {
        json!({"status":"idle","instruction":"Make one new independent wait (chat_wait or seed_wait, timeout_ms:25000). Idle is normal. The user writes in the app at any time; idle only means no new message yet, so just wait again. Respect host execution limits."})
    } else {
        json!({"status":"assigned","needs_open":load(root, &s.chat_id)?["seed_pending"] == true,"chat_id":s.chat_id,"attachment_id":s.attachment_id,"generation":s.generation,"instruction":"Call chat_open with this chat_id and attachment_id. Read the complete returned skill and history. Preserve the existing pending message, plan and user constraints; inspect files and prior operation results before resuming. Work through this MCP workspace. Do not replay completed actions or use an old host sandbox as the project source."})
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
    fn workspace_release_preserves_pool_and_fences_old_assignment() {
        let (root, batch, tokens) = fixture();
        let sid = batch[0]["seed_id"].as_str().unwrap();
        let chat = super::super::ui(
            root.path(),
            &json!({"action":"create","title":"Keep history"}),
        )
        .unwrap()["session"]
            .clone();
        let assigned = poll(root.path(), sid, &tokens[0]).unwrap();
        assert_eq!(assigned["chat_id"], chat["id"]);
        release_workspace(root.path()).unwrap();
        let data = read(root.path()).unwrap();
        assert!(!data.enabled);
        assert_eq!(data.seeds[0].retired_at, 0);
        assert!(data.seeds[0].chat_id.is_empty());
        assert_eq!(public(&data.seeds[0], now())["status"], "offline");
        assert!(begin(
            root.path(),
            sid,
            &tokens[0],
            "chat_reply",
            &json!({"chat_id":chat["id"],"attachment_id":assigned["attachment_id"]}),
            "old"
        )
        .is_err());
        assert_eq!(
            load(root.path(), chat["id"].as_str().unwrap()).unwrap()["title"],
            "Keep history"
        );
        assert_eq!(
            poll(root.path(), sid, &tokens[0]).unwrap()["status"],
            "idle"
        );
        release_workspace(root.path()).unwrap();
        super::super::ui(root.path(), &json!({"action":"seed_cleanup"})).unwrap();
        assert_eq!(read(root.path()).unwrap().seeds[0].retired_at, 0);
        super::super::ui(
            root.path(),
            &json!({"action":"seed_settings","enabled":true}),
        )
        .unwrap();
        let next = super::super::ui(
            root.path(),
            &json!({"action":"create","title":"New assignment"}),
        )
        .unwrap()["session"]
            .clone();
        assert_eq!(
            poll(root.path(), sid, &tokens[0]).unwrap()["chat_id"],
            next["id"]
        );
    }

    #[test]
    fn deleting_an_account_card_removes_only_its_settled_seeds() {
        let (root, _, _) = fixture();
        let run = |args: Value| super::super::ui(root.path(), &args);
        run(json!({"action":"seed_manual","account":"other","source":"arena"})).unwrap();
        assert!(run(json!({"action":"seed_delete_account","account":"missing"})).is_err());
        let mut data = read(root.path()).unwrap();
        data.seeds[0].operations.push("running-operation".into());
        write(root.path(), &data).unwrap();
        assert!(run(json!({"action":"seed_delete_account","account":"test"})).is_err());
        assert_eq!(read(root.path()).unwrap().seeds.len(), 4);
        let mut data = read(root.path()).unwrap();
        data.seeds[0].operations.clear();
        write(root.path(), &data).unwrap();
        let result = run(json!({"action":"seed_delete_account","account":"test"})).unwrap();
        assert_eq!(result["affected_count"], 3);
        let left = read(root.path()).unwrap().seeds;
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].account, "other");
    }

    #[test]
    fn workspace_release_refuses_unresolved_operations_before_mutation() {
        let (root, _, _) = fixture();
        let mut data = read(root.path()).unwrap();
        data.seeds[1].operations.push("running-operation".into());
        write(root.path(), &data).unwrap();
        assert!(release_workspace(root.path()).is_err());
        assert!(read(root.path()).unwrap().enabled);
    }

    #[test]
    fn batch_changes_preflight_all_records_and_only_purge_retired() {
        let (root, batch, _) = fixture();
        let ids: Vec<_> = batch.iter().map(|s| s["seed_id"].clone()).collect();
        let run = |args: Value| super::super::ui(root.path(), &args);
        assert!(run(json!({"action":"seed_retire","seed_ids":[ids[0],"missing"]})).is_err());
        assert_eq!(read(root.path()).unwrap().seeds[0].retired_at, 0);
        assert!(run(json!({"action":"seed_purge","seed_ids":ids})).is_err());
        assert!(run(json!({"action":"seed_retire","seed_ids":[ids[0],ids[0]]})).is_err());
        assert_eq!(
            run(json!({"action":"seed_label","seed_ids":ids,"source":"Arena","prefix":"Opus"}))
                .unwrap()["affected_count"],
            ids.len()
        );
        assert!(read(root.path())
            .unwrap()
            .seeds
            .iter()
            .all(|s| s.source == "arena" && s.prefix == "Opus"));
        run(json!({"action":"seed_retire","seed_ids":[ids[0],ids[1]]})).unwrap();
        assert!(run(json!({"action":"seed_purge","seed_ids":ids})).is_err());
        let purged = run(json!({"action":"seed_purge","seed_ids":[ids[0],ids[1]]})).unwrap();
        assert_eq!(purged["affected_count"], 2);
        assert_eq!(purged["total"], ids.len() - 2);
    }

    #[test]
    fn reissue_rotates_expired_unconnected_credentials_and_rejects_stale_provision() {
        let (root, batch, _) = fixture();
        let run = |args: Value| super::super::ui(root.path(), &args);
        assert!(run(json!({"action":"seed_reissue","seed_id":batch[0]["seed_id"]})).is_err());
        let token = "7".repeat(64);
        run(json!({"action":"seed_pairing","token":token})).unwrap();
        let request = json!({"action":"batch","request_id":"reissue-test","count":1,"account":"browser","repo_id":"repo","branch":"main"});
        let old = provision(root.path(), &token, &request).unwrap()["batch"][0].clone();
        assert!(run(json!({"action":"seed_reissue","seed_id":old["seed_id"]})).is_err());
        let mut data = read(root.path()).unwrap();
        data.seeds
            .iter_mut()
            .find(|s| s.id == old["seed_id"])
            .unwrap()
            .enrollment_until = now() - 1;
        write(root.path(), &data).unwrap();
        let fresh = run(json!({"action":"seed_reissue","seed_id":old["seed_id"]})).unwrap()
            ["batch"][0]
            .clone();
        assert_eq!(fresh["seed_id"], old["seed_id"]);
        assert_ne!(fresh["url_key"], old["url_key"]);
        assert!(initialize(
            root.path(),
            old["seed_id"].as_str().unwrap(),
            old["ticket"].as_str().unwrap()
        )
        .is_err());
        assert!(url(root.path(), old["url_key"].as_str().unwrap(), true).is_err());
        assert!(provision(root.path(), &token, &request).is_err());
        assert!(initialize(
            root.path(),
            fresh["seed_id"].as_str().unwrap(),
            fresh["ticket"].as_str().unwrap()
        )
        .is_ok());
        let persisted = fs::read_to_string(safe(root.path(), FILE).unwrap()).unwrap();
        assert!(!persisted.contains(fresh["ticket"].as_str().unwrap()));
        assert!(!persisted.contains(fresh["url_key"].as_str().unwrap()));
    }

    #[test]
    fn account_cards_summarize_all_filtered_seeds_before_pagination() {
        let (root, _, _) = fixture();
        let mut data = read(root.path()).unwrap();
        let template = data.seeds[0].clone();
        let time = now();
        data.seeds = (0..60)
            .map(|i| {
                let mut seed = template.clone();
                seed.id = format!("summary-{i}");
                seed.account = "account-a".into();
                seed.prefix = "Astra".into();
                seed.ready = true;
                seed.last_seen = time;
                match i % 6 {
                    1 => seed.chat_id = "assigned-chat".into(),
                    2 => seed.inflight.push("operation".into()),
                    3 => seed.redeemed_at = 0,
                    4 => seed.last_seen = time - 700_000,
                    5 => seed.retired_at = time,
                    _ => {}
                }
                seed.archived = i == 59;
                seed
            })
            .collect();
        let mut other = template;
        other.id = "summary-b".into();
        other.account = "__proto__".into();
        other.source = "custom".into();
        other.prefix = "Sol".into();
        other.ready = true;
        other.last_seen = time;
        data.seeds.push(other);
        let list = |mut args: Value| {
            if args["include_archived"].is_null() {
                args["include_archived"] = json!(false);
            }
            inventory(&data, &args, time)
        };
        let page = list(json!({"limit":1,"offset":40}));
        let expected = json!({"account":"account-a","total":59,"valid":30,"counts":{"ready":10,"assigned":10,"working":10,"preparing":10,"offline":10,"retired":9}});
        assert_eq!(page["seeds"].as_array().unwrap().len(), 1);
        assert_eq!(page["filtered_total"], 60);
        assert_eq!(
            page["ready_groups"]
                .as_array()
                .unwrap()
                .iter()
                .map(|g| g["ready"].as_u64().unwrap())
                .sum::<u64>(),
            11
        );
        assert_eq!(page["account_summaries"][1], expected);
        assert_eq!(
            page["account_summaries"],
            list(json!({"limit":1}))["account_summaries"]
        );
        assert_eq!(page["account_summaries"][0]["valid"], 1);
        assert_eq!(
            list(json!({"include_archived":true}))["account_summaries"][1]["total"],
            60
        );
        for filter in [
            json!({"account":"account-a"}),
            json!({"source":"coderabbit","keyword":"ASTRA"}),
            json!({"search":"ACCOUNT-A"}),
        ] {
            assert_eq!(list(filter)["account_summaries"], json!([expected]));
        }
        assert_eq!(
            list(json!({"account":"account-a","status":"offline"}))["account_summaries"],
            json!([{"account":"account-a","total":10,"valid":0,"counts":{"offline":10}}])
        );
        assert_eq!(
            list(json!({"keyword":"no-match"}))["account_summaries"],
            json!([])
        );
    }
    #[test]
    fn exact_prefix_and_account_fence_owners_and_isolate_scan_caches() {
        let (root, mut batch, mut tokens) = fixture();
        let extra = ui(root.path(), &json!({"action":"seed_batch","count":3,"account":"b","repo_id":"host","branch":"main"})).unwrap()["batch"].as_array().unwrap().clone();
        for seed in &extra {
            tokens.push(
                initialize(
                    root.path(),
                    seed["seed_id"].as_str().unwrap(),
                    seed["ticket"].as_str().unwrap(),
                )
                .unwrap(),
            );
        }
        batch.extend(extra);
        let mut data = read(root.path()).unwrap();
        for (i, seed) in data.seeds.iter_mut().enumerate() {
            seed.account = if i == 0 { "a" } else { "b" }.into();
            seed.prefix = ["Astra", "Astra", "Astra", "Astra-plus", "astra", ""][i].into();
        }
        write(root.path(), &data).unwrap();
        let time = now();
        let poll_seed = |i: usize, time| {
            poll_at(
                root.path(),
                batch[i]["seed_id"].as_str().unwrap(),
                &tokens[i],
                time,
            )
            .unwrap()
        };
        for i in 0..6 {
            assert_eq!(poll_seed(i, time)["status"], "idle");
        }
        let ready = ui(root.path(), &json!({"action":"seed_list","limit":1})).unwrap()
            ["ready_groups"]
            .clone();
        assert_eq!(
            ready
                .as_array()
                .unwrap()
                .iter()
                .map(|g| g["ready"].as_u64().unwrap())
                .sum::<u64>(),
            6
        );
        let filter = json!({"source":"coderabbit","keyword":"","prefix":"Astra","account":"b"});
        let chat = super::super::ui(
            root.path(),
            &json!({"action":"create","seed_filter":filter}),
        )
        .unwrap()["session"]
            .clone();
        assert_eq!(chat["seed_filter"], filter);
        for i in [0, 3, 4, 5] {
            assert_eq!(poll_seed(i, time)["status"], "idle");
        }
        assert_eq!(poll_seed(1, time)["chat_id"], chat["id"]);
        let after = ui(root.path(), &json!({"action":"seed_list","limit":1})).unwrap()
            ["ready_groups"]
            .clone();
        assert_eq!(
            after
                .as_array()
                .unwrap()
                .iter()
                .find(|g| g["account"] == "b" && g["prefix"] == "Astra")
                .unwrap()["ready"],
            1
        );
        let future = time + REPLACE_MS + 1;
        for i in [0, 3, 4] {
            assert_eq!(poll_seed(i, future)["status"], "idle");
        }
        let replacement = poll_seed(2, future);
        assert_eq!(replacement["chat_id"], chat["id"]);
        assert_eq!(replacement["generation"], 2);
        assert_eq!(
            super::super::ui(root.path(), &json!({"action":"read","chat_id":chat["id"]})).unwrap()
                ["session"]["seed_filter"],
            filter
        );
        let empty = super::super::ui(
            root.path(),
            &json!({"action":"create","seed_filter":{"source":"","keyword":"","prefix":""}}),
        )
        .unwrap()["session"]
            .clone();
        assert_eq!(poll_seed(4, future)["status"], "idle");
        assert_eq!(poll_seed(5, future)["chat_id"], empty["id"]);
        for field in ["prefix", "account"] {
            assert!(parse_filter(&json!({field:{"bad":true}})).is_err());
        }
    }
    #[test]
    fn classified_pools_keep_initial_and_replacement_routing_across_host_repositories() {
        let root = tempfile::tempdir().unwrap();
        super::super::ui(
            root.path(),
            &json!({"action":"seed_settings","enabled":true}),
        )
        .unwrap();
        let mut batch = vec![];
        let mut tokens = vec![];
        for i in 0..3 {
            let seed = ui(root.path(), &json!({"action":"seed_batch","count":1,"account":format!("account-{i}"),"repo_id":format!("host-{i}"),"branch":format!("branch-{i}")})).unwrap()["batch"][0].clone();
            tokens.push(
                initialize(
                    root.path(),
                    seed["seed_id"].as_str().unwrap(),
                    seed["ticket"].as_str().unwrap(),
                )
                .unwrap(),
            );
            batch.push(seed);
        }
        for i in 0..3 {
            ui(root.path(),&json!({"action":"seed_label","seed_id":batch[i]["seed_id"],"source":"coderabbit","prefix":if i==1{"GPT-6-sol"}else{"GPT-6-astra"}})).unwrap();
            assert_eq!(
                poll(
                    root.path(),
                    batch[i]["seed_id"].as_str().unwrap(),
                    &tokens[i]
                )
                .unwrap()["status"],
                "idle"
            );
        }
        let chat = super::super::ui(
            root.path(),
            &json!({"action":"create","seed_filter":{"source":"coderabbit","keyword":"ASTRA"}}),
        )
        .unwrap()["session"]
            .clone();
        assert_eq!(
            poll(
                root.path(),
                batch[1]["seed_id"].as_str().unwrap(),
                &tokens[1]
            )
            .unwrap()["status"],
            "idle"
        );
        assert_eq!(
            poll(
                root.path(),
                batch[0]["seed_id"].as_str().unwrap(),
                &tokens[0]
            )
            .unwrap()["chat_id"],
            chat["id"]
        );
        let future = now() + REPLACE_MS + 5;
        assert_eq!(
            poll_at(
                root.path(),
                batch[1]["seed_id"].as_str().unwrap(),
                &tokens[1],
                future
            )
            .unwrap()["status"],
            "idle"
        );
        let replacement = poll_at(
            root.path(),
            batch[2]["seed_id"].as_str().unwrap(),
            &tokens[2],
            future,
        )
        .unwrap();
        assert_eq!(replacement["chat_id"], chat["id"]);
        assert_eq!(replacement["generation"], 2);
        assert_eq!(
            super::super::ui(root.path(), &json!({"action":"read","chat_id":chat["id"]})).unwrap()
                ["session"]["seed_filter"],
            chat["seed_filter"]
        );
        assert!(
            super::super::ui(root.path(), &json!({"action":"create","seed_manual":true})).unwrap()
                ["session"]["seed_auto"]
                .is_null()
        );
    }
    #[test]
    fn manual_enrollment_is_independent_of_browser_repository_and_prefix_retry_is_stable() {
        let (root, _, _) = fixture();
        let manual = ui(
            root.path(),
            &json!({"action":"seed_manual","account":"personal","source":"Arena","prefix":"Opus"}),
        )
        .unwrap()["batch"][0]
            .clone();
        assert_eq!(manual["repo_id"], "");
        assert_eq!(manual["source"], "arena");
        let token = initialize(
            root.path(),
            manual["seed_id"].as_str().unwrap(),
            manual["ticket"].as_str().unwrap(),
        )
        .unwrap();
        let chat = super::super::ui(
            root.path(),
            &json!({"action":"create","seed_filter":{"source":"arena","keyword":"opus"}}),
        )
        .unwrap()["session"]
            .clone();
        assert_eq!(
            poll(root.path(), manual["seed_id"].as_str().unwrap(), &token).unwrap()["chat_id"],
            chat["id"]
        );
        let library = ui(
            root.path(),
            &json!({"action":"seed_list","source":"arena","keyword":"OPUS"}),
        )
        .unwrap();
        assert_eq!(library["seeds"].as_array().unwrap().len(), 1);
        assert!(!library
            .to_string()
            .contains(manual["ticket"].as_str().unwrap()));
        let paired = "f".repeat(64);
        ui(
            root.path(),
            &json!({"action":"seed_pairing","token":paired}),
        )
        .unwrap();
        let args = json!({"action":"batch","request_id":"classified","count":1,"account":"test","repo_id":"exact","branch":"main","prefix":"GPT-6-astra"});
        let first = provision(root.path(), &paired, &args).unwrap();
        assert_eq!(provision(root.path(), &paired, &args).unwrap(), first);
        let mut other = args;
        other["prefix"] = json!("grok");
        assert!(provision(root.path(), &paired, &other).is_err());
        other["prefix"] = first["batch"][0]["prefix"].clone();
        other["repo_id"] = json!("another-host");
        assert!(provision(root.path(), &paired, &other).is_err());
        other["request_id"] = json!("other");
        assert_eq!(
            provision(root.path(), &paired, &other).unwrap()["batch"][0]["repo_id"],
            "another-host"
        );
    }
    #[test]
    fn pairing_rotation_retry_and_large_inventory() {
        let (root, _, tokens) = fixture();
        let token = "b".repeat(64);
        super::super::ui(root.path(), &json!({"action":"seed_pairing","token":token})).unwrap();
        let args = json!({"action":"batch","request_id":"attempt","count":2,"account":"test","repo_id":"exact","branch":"main","metadata":{"repo_name":"owner/repo"}});
        let first = provision(root.path(), &token, &args).unwrap();
        assert_eq!(provision(root.path(), &token, &args).unwrap(), first);
        let mut changed = args.clone();
        changed["count"] = json!(3);
        assert!(provision(root.path(), &token, &changed).is_err());
        assert!(provision(root.path(), &token, &json!({"action":"read_file"})).is_err());
        let other = tempfile::tempdir().unwrap();
        assert!(provision(other.path(), &token, &json!({"action":"status"})).is_err());
        let raw = fs::read_to_string(safe(root.path(), FILE).unwrap()).unwrap();
        assert!(!raw.contains(&token));
        assert!(!raw.contains(first["batch"][0]["ticket"].as_str().unwrap()));
        super::super::ui(
            root.path(),
            &json!({"action":"seed_pairing","token":"c".repeat(64)}),
        )
        .unwrap();
        assert!(provision(root.path(), &token, &json!({"action":"status"})).is_err());
        super::super::ui(
            root.path(),
            &json!({"action":"seed_pairing","enabled":false}),
        )
        .unwrap();
        assert!(provision(root.path(), &"c".repeat(64), &json!({"action":"status"})).is_err());
        let mut data = read(root.path()).unwrap();
        let template = data.seeds[0].clone();
        let time = now();
        data.seeds = (0..600)
            .map(|i| {
                let mut s = template.clone();
                s.id = format!("load-{i}");
                s.account = if i % 2 == 0 { "even" } else { "odd" }.into();
                s.ready = true;
                s.last_seen = time;
                s
            })
            .collect();
        write(root.path(), &data).unwrap();
        let page = ui(
            root.path(),
            &json!({"action":"seed_list","limit":50,"offset":50,"account":"odd","status":"ready"}),
        )
        .unwrap();
        assert_eq!(page["total"], 600);
        assert_eq!(page["filtered_total"], 300);
        assert_eq!(page["seeds"].as_array().unwrap().len(), 50);
        assert_eq!(page["seeds"][0]["id"], "load-101");
        assert_eq!(page["counts"]["ready"], 600);
        for i in 0..500 {
            assert_eq!(
                poll_at(root.path(), &format!("load-{i}"), &tokens[0], time).unwrap()["status"],
                "idle"
            );
        }
        let chat = super::super::ui(root.path(), &json!({"action":"create"})).unwrap();
        assert_eq!(
            poll_at(root.path(), "load-0", &tokens[0], time).unwrap()["chat_id"],
            chat["session"]["id"]
        );
    }
    #[test]
    fn mixed_host_repositories_preserve_pending_connected_and_retired_seeds() {
        let root = tempfile::tempdir().unwrap();
        let original = json!({"action":"seed_batch","count":1,"account":"a","repo_id":"repo-a","branch":"main"});
        let first = super::super::ui(root.path(), &original).unwrap()["batch"][0].clone();
        let before = serde_json::to_value(&read(root.path()).unwrap().seeds[0]).unwrap();
        let other = json!({"action":"seed_batch","count":1,"account":"b","repo_id":"repo-b","branch":"develop","replace_unconnected":true});
        let next = ui(root.path(), &other).unwrap()["batch"][0].clone();
        assert_eq!(
            serde_json::to_value(&read(root.path()).unwrap().seeds[0]).unwrap(),
            before
        );
        let token = initialize(
            root.path(),
            first["seed_id"].as_str().unwrap(),
            first["ticket"].as_str().unwrap(),
        )
        .unwrap();
        initialize(
            root.path(),
            next["seed_id"].as_str().unwrap(),
            next["ticket"].as_str().unwrap(),
        )
        .unwrap();
        assert!(ui(root.path(), &other).is_ok());
        authenticate(root.path(), first["seed_id"].as_str().unwrap(), &token).unwrap();
        let separate = tempfile::tempdir().unwrap();
        assert!(authenticate(separate.path(), first["seed_id"].as_str().unwrap(), &token).is_err());
        ui(
            root.path(),
            &json!({"action":"seed_retire","seed_id":first["seed_id"]}),
        )
        .unwrap();
        assert!(ui(root.path(), &other).is_ok());
        let list = ui(root.path(), &json!({"action":"seed_list"})).unwrap();
        assert_eq!(list["retired_count"], 1);
        assert_eq!(list["total"], 4);
        assert_eq!(next["repo_id"], "repo-b");
        assert_eq!(next["branch"], "develop");
    }
    #[test]
    fn submitted_host_task_survives_another_repository_batch() {
        let root = tempfile::tempdir().unwrap();
        let mut args =
            json!({"action":"seed_batch","count":1,"account":"a","repo_id":"repo","branch":"main"});
        let first = super::super::ui(root.path(), &args).unwrap()["batch"][0].clone();
        created(
            root.path(),
            first["seed_id"].as_str().unwrap(),
            first["ticket"].as_str().unwrap(),
            "task-id",
        )
        .unwrap();
        let before = serde_json::to_value(&read(root.path()).unwrap().seeds[0]).unwrap();
        args["repo_id"] = json!("other-repo");
        args["branch"] = json!("other");
        assert!(ui(root.path(), &args).is_ok());
        assert_eq!(
            serde_json::to_value(&read(root.path()).unwrap().seeds[0]).unwrap(),
            before
        );
        assert_eq!(
            ui(root.path(), &json!({"action":"seed_list"})).unwrap()["retired_count"],
            0
        );
    }
    #[test]
    fn standby_cache_invalidation_uses_canonical_workspace_identity() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        let alias = root.path().join("nested").join("..");
        let canonical = root.path().canonicalize().unwrap();
        super::super::ui(
            root.path(),
            &json!({"action":"seed_settings","enabled":true}),
        )
        .unwrap();
        let seed = super::super::ui(
            root.path(),
            &json!({"action":"seed_manual","account":"alias"}),
        )
        .unwrap()["batch"][0]
            .clone();
        let sid = seed["seed_id"].as_str().unwrap();
        let token = initialize(root.path(), sid, seed["ticket"].as_str().unwrap()).unwrap();
        let time = now();
        assert_eq!(
            poll_at(&canonical, sid, &token, time).unwrap()["status"],
            "idle"
        );
        let chat =
            super::super::ui(&alias, &json!({"action":"create"})).unwrap()["session"].clone();
        assert_eq!(
            poll_at(&canonical, sid, &token, time + 1).unwrap()["chat_id"],
            chat["id"]
        );
    }
    #[test]
    fn url_capability_persists_without_receipts_and_does_not_extend_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let issued = super::super::ui(root, &json!({"action":"seed_manual","account":"url"}))
            .unwrap()["batch"][0]
            .clone();
        let key = issued["url_key"].as_str().unwrap();
        let sid = issued["seed_id"].as_str().unwrap();
        assert_eq!(url(root, key, false).unwrap(), sid);
        assert!(authenticate(root, sid, key).is_err());
        assert_eq!(url(root, key, true).unwrap(), sid);
        authenticate(root, sid, key).unwrap();
        let expires = read(root).unwrap().seeds[0].access_until;
        assert_eq!(url(root, key, true).unwrap(), sid);
        assert_eq!(read(root).unwrap().seeds[0].access_until, expires);
        assert!(url(tempfile::tempdir().unwrap().path(), key, true).is_err());
        let serialized = fs::read_to_string(safe(root, FILE).unwrap()).unwrap();
        assert!(!serialized.contains(key));
        assert!(!serialized.contains(issued["ticket"].as_str().unwrap()));
        let mut data = read(root).unwrap();
        data.seeds[0].access_until = now() - 1;
        write(root, &data).unwrap();
        assert!(url(root, key, true).is_err());
        assert!(authenticate(root, sid, key).is_err());
        data.seeds[0].redeemed_at = 0;
        data.seeds[0].enrollment_until = now() - 1;
        write(root, &data).unwrap();
        assert!(url(root, key, true).is_err());
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

#[cfg(test)]
mod standby_tests {
    use super::*;
    fn library(enabled: bool) -> (tempfile::TempDir, Vec<Value>, Vec<String>) {
        let root = tempfile::tempdir().unwrap();
        super::super::ui(
            root.path(),
            &json!({"action":"seed_settings","enabled":enabled}),
        )
        .unwrap();
        let batch = super::super::ui(root.path(), &json!({"action":"seed_batch","count":2,"account":"acct","prefix":"p","repo_id":"r","branch":"main"})).unwrap()["batch"].as_array().unwrap().clone();
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
    fn seed_row(root: &Path, sid: &str) -> Value {
        super::super::ui(root, &json!({"action":"seed_list"})).unwrap()["seeds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == sid)
            .cloned()
            .unwrap()
    }

    #[test]
    fn explicit_pool_is_served_while_automatic_assignment_is_off() {
        let (root, batch, tokens) = library(false);
        let sid = batch[0]["seed_id"].as_str().unwrap();
        let plain =
            super::super::ui(root.path(), &json!({"action":"create"})).unwrap()["session"].clone();
        assert_ne!(plain["seed_auto"], true);
        let chosen = super::super::ui(
            root.path(),
            &json!({"action":"create","seed_filter":{"source":"","keyword":"","account":"acct"}}),
        )
        .unwrap()["session"]
            .clone();
        assert_eq!(chosen["seed_auto"], true);
        let assigned = poll(root.path(), sid, &tokens[0]).unwrap();
        assert_eq!(assigned["status"], "assigned");
        assert_eq!(assigned["chat_id"], chosen["id"]);
        let inventory = super::super::ui(root.path(), &json!({"action":"seed_list"})).unwrap();
        assert!(inventory["pool_groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["account"] == "acct" && g["total"] == 2));
    }

    #[test]
    fn disconnecting_a_live_seed_returns_it_as_assignable_standby() {
        let (root, batch, tokens) = library(true);
        let sid = batch[0]["seed_id"].as_str().unwrap();
        let chat =
            super::super::ui(root.path(), &json!({"action":"create"})).unwrap()["session"].clone();
        let assigned = poll(root.path(), sid, &tokens[0]).unwrap();
        assert_eq!(assigned["chat_id"], chat["id"]);
        super::super::tool(root.path(), "chat_open", &assigned).unwrap();
        let detached = super::super::ui(
            root.path(),
            &json!({"action":"detach","chat_id":chat["id"],"standby":true}),
        )
        .unwrap();
        assert_eq!(detached["standby"], "seed");
        let handoff = super::super::standby_handoff(&assigned).unwrap();
        assert_eq!(handoff["status"], "idle");
        assert_eq!(handoff["standby"], true);
        let row = seed_row(root.path(), sid);
        assert_eq!(row["standby"], true);
        assert_eq!(row["retired_at"], 0);
        assert_eq!(
            poll_at(root.path(), sid, &tokens[0], now() + 2_000).unwrap()["status"],
            "idle"
        );
        assert_eq!(seed_row(root.path(), sid)["status"], "ready");
        let next = super::super::ui(
            root.path(),
            &json!({"action":"create","seed_filter":{"source":"","keyword":"","standby":true}}),
        )
        .unwrap()["session"]
            .clone();
        // The other (non-standby) seed must not match a standby-only pool.
        assert_eq!(
            poll_at(
                root.path(),
                batch[1]["seed_id"].as_str().unwrap(),
                &tokens[1],
                now() + 4_000
            )
            .unwrap()["status"],
            "idle"
        );
        let again = poll_at(root.path(), sid, &tokens[0], now() + 6_000).unwrap();
        assert_eq!(again["chat_id"], next["id"]);
        assert_eq!(seed_row(root.path(), sid)["standby"], false);
    }

    #[test]
    fn disconnecting_a_live_direct_ai_enrolls_a_standby_seed() {
        let root = tempfile::tempdir().unwrap();
        let chat = super::super::ui(root.path(), &json!({"action":"create","seed_manual":true}))
            .unwrap()["session"]
            .clone();
        let opened = super::super::tool(
            root.path(),
            "chat_open",
            &json!({"chat_id":chat["id"],"agent_name":"Claude"}),
        )
        .unwrap();
        let attachment = opened["attachment_id"].as_str().unwrap().to_owned();
        let detached = super::super::ui(
            root.path(),
            &json!({"action":"detach","chat_id":chat["id"],"standby":true}),
        )
        .unwrap();
        assert_eq!(detached["standby"], "direct");
        assert_eq!(detached["session"]["seed_auto"], false);
        let handoff = super::super::standby_handoff(
            &json!({"attachment_id":attachment,"workspace_folder_id":"folder1"}),
        )
        .unwrap();
        assert_eq!(handoff["status"], "standby");
        let suffix = handoff["seed_url_suffix"].as_str().unwrap();
        let key = suffix.strip_prefix("/seeds/folder1/").unwrap();
        assert_eq!(key.len(), 64);
        let sid = url(root.path(), key, true).unwrap();
        // Once redeemed, the URL is no longer handed out again.
        assert!(super::super::standby_handoff(
            &json!({"attachment_id":attachment,"workspace_folder_id":"folder1"})
        )
        .is_none());
        let row = seed_row(root.path(), &sid);
        assert_eq!(row["source"], "standby");
        assert_eq!(row["standby"], true);
        // The old attachment can no longer act on the chat.
        assert!(super::super::authorize_project_attachment(
            root.path(),
            &json!({"chat_id":chat["id"],"attachment_id":attachment}),
            true
        )
        .is_err());
    }

    #[test]
    fn choosing_a_pool_after_a_stale_seed_owner_is_allowed() {
        let (root, batch, tokens) = library(false);
        let sid = batch[0]["seed_id"].as_str().unwrap();
        let chat = super::super::ui(
            root.path(),
            &json!({"action":"create","seed_filter":{"source":"","keyword":""}}),
        )
        .unwrap()["session"]
            .clone();
        let assigned = poll(root.path(), sid, &tokens[0]).unwrap();
        assert_eq!(assigned["chat_id"], chat["id"]);
        // Plain detach (e.g. the seed was offline) keeps seed_owner set.
        super::super::ui(
            root.path(),
            &json!({"action":"detach","chat_id":chat["id"]}),
        )
        .unwrap();
        super::super::ui(
            root.path(),
            &json!({"action":"seed_resume_chat","chat_id":chat["id"],"seed_filter":{"source":"","keyword":"","account":"acct"}}),
        )
        .unwrap();
        let read =
            super::super::ui(root.path(), &json!({"action":"read","chat_id":chat["id"]})).unwrap();
        assert_eq!(read["session"]["seed_auto"], true);
    }

    #[test]
    fn direct_url_is_bound_to_its_chat_only() {
        let root = tempfile::tempdir().unwrap();
        let other = super::super::ui(
            root.path(),
            &json!({"action":"create","seed_filter":{"source":"","keyword":""}}),
        )
        .unwrap()["session"]
            .clone();
        let chat = super::super::ui(root.path(), &json!({"action":"create","seed_manual":true}))
            .unwrap()["session"]
            .clone();
        let first = super::super::ui(
            root.path(),
            &json!({"action":"seed_direct","chat_id":chat["id"]}),
        )
        .unwrap();
        // The first URL is used but not yet assigned when the guide reissues.
        let first_key = first["seed"]["url_key"].as_str().unwrap().to_owned();
        let first_sid = url(root.path(), &first_key, true).unwrap();
        let issued = super::super::ui(
            root.path(),
            &json!({"action":"seed_direct","chat_id":chat["id"]}),
        )
        .unwrap();
        // Its AI is told to stop instead of waiting forever.
        assert!(poll(root.path(), &first_sid, &first_key).is_err());
        // Re-issuing retires the unused URL.
        assert!(url(
            root.path(),
            first["seed"]["url_key"].as_str().unwrap(),
            false
        )
        .is_err());
        let key = issued["seed"]["url_key"].as_str().unwrap();
        let sid = url(root.path(), key, true).unwrap();
        let read =
            super::super::ui(root.path(), &json!({"action":"read","chat_id":chat["id"]})).unwrap();
        assert_eq!(read["session"]["seed_direct"], true);
        assert_eq!(read["session"]["seed_auto"], true);
        let seed = seed_row(root.path(), &sid);
        let assigned = poll(root.path(), &sid, key).unwrap();
        // The older pool chat must not take the bound seed.
        assert_eq!(assigned["chat_id"], chat["id"], "{seed}");
        assert_ne!(assigned["chat_id"], other["id"]);
        let inventory = super::super::ui(root.path(), &json!({"action":"seed_list"})).unwrap();
        assert!(inventory["pool_groups"].as_array().unwrap().is_empty());
        // Disconnecting before the URL is used retires it.
        let pending = super::super::ui(root.path(), &json!({"action":"create","seed_manual":true}))
            .unwrap()["session"]
            .clone();
        let unused = super::super::ui(
            root.path(),
            &json!({"action":"seed_direct","chat_id":pending["id"]}),
        )
        .unwrap();
        super::super::ui(
            root.path(),
            &json!({"action":"detach","chat_id":pending["id"]}),
        )
        .unwrap();
        assert!(url(
            root.path(),
            unused["seed"]["url_key"].as_str().unwrap(),
            false
        )
        .is_err());
    }

    #[test]
    fn plain_detach_keeps_previous_behaviour() {
        let root = tempfile::tempdir().unwrap();
        let chat = super::super::ui(root.path(), &json!({"action":"create","seed_manual":true}))
            .unwrap()["session"]
            .clone();
        super::super::tool(root.path(), "chat_open", &json!({"chat_id":chat["id"]})).unwrap();
        let detached = super::super::ui(
            root.path(),
            &json!({"action":"detach","chat_id":chat["id"]}),
        )
        .unwrap();
        assert!(detached["standby"].is_null());
        let inventory = super::super::ui(root.path(), &json!({"action":"seed_list"})).unwrap();
        assert_eq!(inventory["total"], 0);
    }
}
