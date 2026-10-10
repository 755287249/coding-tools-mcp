//! Ephemeral pairing indicators. A ticket confers no MCP or conversation access.
use serde_json::{json, Value};
use std::{collections::HashMap, path::{Path, PathBuf}, sync::{Mutex, OnceLock}};

const TTL: u64 = 15 * 60_000;
const LIMIT: usize = 256;
#[derive(Clone)]
struct Entry { root: PathBuf, chat: String, attempt: String, expires: u64, started: Option<u64> }
#[derive(Default)]
struct Registry { entries: HashMap<String, Entry> }
impl Registry {
    fn prune(&mut self, now: u64) { self.entries.retain(|_, e| e.expires > now); }
    fn issue(&mut self, root: PathBuf, chat: &str, attempt: &str, now: u64) -> Value {
        self.prune(now);
        if let Some((ticket, e)) = self.entries.iter().find(|(_, e)| e.root == root && e.chat == chat && e.attempt == attempt) {
            return json!({"ticket":ticket,"attempt_id":attempt,"expires_at":e.expires});
        }
        self.entries.retain(|_, e| e.root != root || e.chat != chat);
        if self.entries.len() >= LIMIT {
            if let Some(oldest) = self.entries.iter().min_by_key(|(_, e)| e.expires).map(|(k, _)| k.clone()) { self.entries.remove(&oldest); }
        }
        let ticket = uuid::Uuid::new_v4().simple().to_string();
        self.entries.insert(ticket.clone(), Entry {root, chat:chat.into(), attempt:attempt.into(), expires:now+TTL, started:None});
        json!({"ticket":ticket,"attempt_id":attempt,"expires_at":now+TTL})
    }
    fn mark(&mut self, ticket: &str, roots: &[PathBuf], now: u64) -> bool {
        if ticket.len() != 32 || !ticket.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { return false; }
        self.prune(now);
        let Some(e) = self.entries.get_mut(ticket).filter(|e| roots.contains(&e.root)) else { return false; };
        e.started.get_or_insert(now); true
    }
    fn status(&mut self, root: &Path, chat: &str, now: u64) -> Value {
        self.prune(now);
        self.entries.values().find(|e| e.root == root && e.chat == chat).map(|e| json!({"attempt_id":e.attempt,"expires_at":e.expires,"started_at":e.started})).unwrap_or(Value::Null)
    }
}
fn registry() -> &'static Mutex<Registry> { static VALUE: OnceLock<Mutex<Registry>> = OnceLock::new(); VALUE.get_or_init(|| Mutex::new(Registry::default())) }
pub(super) fn prepare(root: &Path, chat: &str, attempt: &str) -> super::Result<Value> {
    Ok(registry().lock().unwrap().issue(std::fs::canonicalize(root).map_err(super::io)?, chat, attempt, super::now()))
}
pub(super) fn status(root: &Path, chat: &str) -> Value {
    std::fs::canonicalize(root).map(|root| registry().lock().unwrap().status(&root, chat, super::now())).unwrap_or(Value::Null)
}
pub(crate) fn mark(ticket: &str, roots: &[PathBuf]) -> bool {
    let roots: Vec<_> = roots.iter().filter_map(|p| std::fs::canonicalize(p).ok()).collect();
    registry().lock().unwrap().mark(ticket, &roots, super::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn group_greeting_survives_discarded_status_ticket() {
        let dir=tempfile::tempdir().unwrap();let root=dir.path();
        let chat=super::super::ui(root,&json!({"action":"create","mode":"group"})).unwrap()["session"]["id"].clone();
        let prepared=super::super::ui(root,&json!({"action":"prepare_pairing","chat_id":chat,"message_id":"after-restart"})).unwrap();
        let canonical=std::fs::canonicalize(root).unwrap();registry().lock().unwrap().entries.retain(|_,e|e.root!=canonical);
        assert!(status(root,chat.as_str().unwrap()).is_null());
        let archive=std::fs::read_to_string(root.join("docs/chat-sessions").join(format!("{}.json",chat.as_str().unwrap()))).unwrap();
        assert!(!archive.contains(prepared["pairing"]["ticket"].as_str().unwrap()));
        let opened=super::super::tool(root,"chat_open",&json!({"chat_id":chat,"agent_name":"Restarted"})).unwrap();
        let received=super::super::tool(root,"chat_wait",&json!({"chat_id":chat,"attachment_id":opened["attachment_id"]})).unwrap();
        assert_eq!(received["message"]["id"],"after-restart");
    }
    #[test]
    fn intent_is_scoped_expiring_idempotent_and_non_authorizing() {
        let mut r=Registry::default(); let root=PathBuf::from("test");
        let a=r.issue(root.clone(),"chat","attempt",100); let t=a["ticket"].as_str().unwrap();
        assert_eq!(r.issue(root.clone(),"chat","attempt",101),a);
        assert!(!r.mark(t,&[PathBuf::from("other")],102));
        assert!(r.mark(t,&[root.clone()],103)); assert!(r.mark(t,&[root.clone()],104));
        let status=r.status(&root,"chat",105);assert_eq!(status["started_at"],103);assert!(status.get("ticket").is_none());
        assert!(r.status(&root,"other",105).is_null());
        assert!(!r.mark(t,&[root.clone()],100+TTL));assert!(r.status(&root,"chat",100+TTL).is_null());
        let b=r.issue(root.clone(),"chat","next",100+TTL);assert_ne!(b["ticket"],a["ticket"]);
        let c=r.issue(root.clone(),"chat","replacement",101+TTL);assert_ne!(c["ticket"],b["ticket"]);
        assert!(!r.mark(b["ticket"].as_str().unwrap(),&[root.clone()],102+TTL));
        for i in 0..300 {r.issue(root.clone(),&i.to_string(),"a",103+TTL);}
        assert_eq!(r.entries.len(),LIMIT);
    }
}
