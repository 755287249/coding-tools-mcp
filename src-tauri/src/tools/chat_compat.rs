//! Narrow, independently authorized GET/file conversation adapter.
use super::*;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex as AsyncMutex;

pub(crate) const SKILL: &str = include_str!("../../../skills/chat-get-compat/SKILL.md");
const TTL: u64 = 30 * 60_000;
#[derive(Clone)]
pub(crate) struct Grant {
    pub key: String,
    pub id: String,
    pub profile: String,
    pub folder: String,
    pub root: PathBuf,
    pub chat: String,
    pub attempt: String,
    pub expires: u64,
    pub attachment: Arc<Mutex<Option<String>>>,
    pub busy: Arc<AsyncMutex<()>>,
}
fn registry() -> &'static Mutex<HashMap<String, Grant>> {
    static VALUE: OnceLock<Mutex<HashMap<String, Grant>>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(HashMap::new()))
}
pub(super) fn check_session(s: &Value) -> Result<()> {
    if s["closed"] == true
        || group::grouped(s)
        || ["messages", "queue"].iter().any(|k| {
            s[*k]
                .as_array()
                .into_iter()
                .flatten()
                .any(|m| !m["discussion"].is_null())
        })
    {
        return Err(err(
            "GET trial requires an open work conversation without discussion deliveries",
        ));
    }
    Ok(())
}
fn public(g: &Grant) -> Value {
    json!({"key":g.key,"grant_id":g.id,"expires_at":g.expires})
}
pub(crate) fn management(root: &Path, profile: &str, folder: &str, args: &Value) -> Result<Value> {
    let _lock = lock(root)?;
    let mut s = load(root, id(&args["chat_id"])?)?;
    let mut entries = registry().lock().unwrap();
    entries.retain(|_, g| g.expires > now());
    let same = |g: &Grant| {
        g.profile == profile && g.folder == folder && g.chat == s["id"].as_str().unwrap_or("")
    };
    if args["action"] == "revoke_compat" {
        let owned_attachment = entries
            .values()
            .find(|g| same(g) && s["compat_grant_id"] == g.id)
            .and_then(|g| g.attachment.lock().unwrap().clone());
        entries.retain(|_, g| !same(g));
        if owned_attachment
            .as_deref()
            .is_some_and(|a| s["attachment_id"].as_str() == Some(a))
        {
            s["attachment_id"] = json!("");
            s["lease_until"] = json!(0);
            s["updated_at"] = json!(now());
        }
        s.as_object_mut().unwrap().remove("compat_grant_id");
        save(root, &s)?;
        return Ok(json!({"ok":true,"revoked":true}));
    }
    if args["action"] != "prepare_compat" {
        return Err(err("Unknown compatibility management action"));
    }
    check_session(&s)?;
    let attempt = id(&args["message_id"])?;
    if let Some(g) = entries
        .values()
        .find(|g| same(g) && g.attempt == attempt && s["compat_grant_id"] == g.id)
    {
        return Ok(json!({"compat":public(g)}));
    }
    if !s["attachment_id"].as_str().unwrap_or("").is_empty() {
        return Err(err("Disconnect the current AI before issuing a GET grant"));
    }
    entries.retain(|_, g| !same(g));
    if entries.len() >= 256 {
        return Err(err("Compatibility grant limit reached"));
    }
    let g = Grant {
        key: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        id: uuid::Uuid::new_v4().to_string(),
        profile: profile.into(),
        folder: folder.into(),
        root: fs::canonicalize(root).map_err(io)?,
        chat: id(&s["id"])?.into(),
        attempt: attempt.into(),
        expires: now() + TTL,
        attachment: Arc::new(Mutex::new(None)),
        busy: Arc::new(AsyncMutex::new(())),
    };
    s["compat_grant_id"] = json!(g.id);
    save(root, &s)?;
    let result = json!({"compat":public(&g)});
    entries.insert(g.key.clone(), g);
    Ok(result)
}
pub(crate) fn get(key: &str, profile: &str) -> Result<Grant> {
    let mut entries = registry().lock().unwrap();
    entries.retain(|_, g| g.expires > now());
    entries
        .get(key)
        .filter(|g| g.profile == profile)
        .cloned()
        .ok_or_else(|| err("Compatibility authorization invalid, expired or revoked"))
}
pub(crate) fn check_policy(ctx: &crate::tools::SharedToolContext) -> Result<()> {
    let config = ctx.runtime_config();
    let names = crate::tools::registry::exposed_tool_names(&config.tool_profile);
    let hooks =
        crate::workspace_features::runtime(&ctx.profile_id).map(|r| r.config().extensions.hooks);
    if !["chat_open", "chat_wait", "chat_reply"]
        .iter()
        .all(|n| names.iter().any(|name| name == n))
        || hooks.is_some_and(|h| h.active && !h.enabled.is_empty())
    {
        return Err(err("GET trial requires chat tools and no enabled hooks"));
    }
    Ok(())
}
pub(crate) fn call(g: &Grant, name: &str, data: Value) -> Result<Value> {
    get(&g.key, &g.profile)?;
    let mut args = json!({"chat_id":g.chat});
    if let Some(a) = g.attachment.lock().unwrap().clone() {
        args["attachment_id"] = json!(a);
    }
    if let Some(data) = data.as_object() {
        args.as_object_mut().unwrap().extend(data.clone());
    }
    let result = super::tool_inner(&g.root, name, &args, Some(&g.id))?;
    if name == "chat_open" {
        let a = result["attachment_id"]
            .as_str()
            .ok_or_else(|| err("Compatibility open failed"))?;
        *g.attachment.lock().unwrap() = Some(a.into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, Value, Value) {
        let d = tempfile::tempdir().unwrap();
        let chat = ui(d.path(), &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let args = json!({"action":"prepare_compat","chat_id":chat,"message_id":"attempt"});
        let g = management(d.path(), "compat-test", "folder", &args).unwrap();
        (d, args, g)
    }
    #[test]
    fn grants_are_scoped_expiring_revocable_and_retry_stable() {
        let (d, args, g) = setup();
        let key = g["compat"]["key"].as_str().unwrap();
        assert_eq!(
            management(d.path(), "compat-test", "folder", &args).unwrap(),
            g
        );
        assert!(get(key, "another-profile").is_err());
        let grant = get(key, "compat-test").unwrap();
        call(&grant, "chat_open", json!({})).unwrap();
        let attachment = grant.attachment.lock().unwrap().clone();
        call(&grant, "chat_open", json!({})).unwrap();
        assert_eq!(attachment, *grant.attachment.lock().unwrap());
        assert!(call(&grant, "exec_command", json!({})).is_err());
        management(
            d.path(),
            "compat-test",
            "folder",
            &json!({"action":"revoke_compat","chat_id":args["chat_id"]}),
        )
        .unwrap();
        assert!(call(&grant, "chat_wait", json!({})).is_err());
        assert_eq!(
            ui(
                d.path(),
                &json!({"action":"read","chat_id":args["chat_id"]})
            )
            .unwrap()["session"]["status"],
            "offline"
        );
        ui(
            d.path(),
            &json!({"action":"detach","chat_id":args["chat_id"]}),
        )
        .unwrap();
        let again = management(d.path(), "compat-test", "folder", &args).unwrap();
        let key = again["compat"]["key"].as_str().unwrap();
        registry().lock().unwrap().get_mut(key).unwrap().expires = 0;
        assert!(get(key, "compat-test").is_err());
    }
    #[test]
    fn detach_mode_and_existing_native_attachment_block_stale_grants() {
        let (d, args, g) = setup();
        let grant = get(g["compat"]["key"].as_str().unwrap(), "compat-test").unwrap();
        tool(d.path(), "chat_open", &json!({"chat_id":args["chat_id"]})).unwrap();
        assert!(call(&grant, "chat_open", json!({})).is_err());
        ui(
            d.path(),
            &json!({"action":"detach","chat_id":args["chat_id"]}),
        )
        .unwrap();
        assert!(call(&grant, "chat_open", json!({})).is_err());
        ui(
            d.path(),
            &json!({"action":"set_mode","chat_id":args["chat_id"],"mode":"group"}),
        )
        .unwrap();
        assert!(management(d.path(), "compat-test", "folder", &args).is_err());
    }
}
