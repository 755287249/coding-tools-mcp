use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const REMEMBER_SECONDS: u64 = 30 * 24 * 60 * 60;

#[derive(Clone)]
struct Session {
    origin: String,
    expiry: Instant,
    remembered: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SavedSession {
    token_hash: String,
    origin: String,
    expires_at: u64,
}

#[derive(Default, Clone)]
pub struct Access {
    password: Option<[u8; 32]>,
    // Decrypted only for local desktop controls; persistence uses the encrypted app secret store.
    display_password: Option<String>,
    origins: Vec<String>,
    sessions: HashMap<String, Session>,
    attempts: Vec<Instant>,
}

pub fn normalize_origin(value: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(value.trim()).map_err(|_| "Invalid browser URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err("Use an HTTP(S) origin without credentials, path, query or fragment".into());
    }
    Ok(url.origin().ascii_serialization())
}
fn hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}
fn equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}
impl Access {
    pub fn enabled(&self) -> bool {
        self.password.is_some()
    }
    pub fn origins(&self) -> &[String] {
        &self.origins
    }
    pub fn local_password(&self) -> Option<&str> {
        self.display_password.as_deref()
    }
    pub fn configure(&mut self, origins: Vec<String>) -> Result<String, String> {
        self.configure_password(
            origins,
            format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
        )
    }
    pub fn configure_password(
        &mut self,
        origins: Vec<String>,
        password: String,
    ) -> Result<String, String> {
        if password.trim().is_empty() || password.len() < 8 || password.len() > 256 {
            return Err("Sharing password must contain 8–256 bytes".into());
        }
        let origins = origins
            .iter()
            .map(|o| normalize_origin(o))
            .collect::<Result<Vec<_>, _>>()?;
        if origins.is_empty() || origins.len() > 32 {
            return Err("Choose 1–32 browser origins".into());
        }
        self.disable();
        self.origins = origins;
        self.password = Some(hash(&password));
        self.display_password = Some(password.clone());
        Ok(password)
    }
    pub fn disable(&mut self) {
        self.password = None;
        self.display_password = None;
        self.sessions.clear();
        self.origins.clear();
        self.attempts.clear();
    }
    pub fn host_allowed(&self, host: &str) -> bool {
        self.enabled()
            && self.origins.iter().any(|origin| {
                origin
                    .split_once("://")
                    .is_some_and(|(_, v)| v.eq_ignore_ascii_case(host))
            })
    }
    pub fn origin_allowed(&self, origin: &str) -> bool {
        self.enabled() && self.origins.iter().any(|v| v == origin)
    }
    pub fn login(&mut self, origin: &str, password: &str, now: Instant) -> Result<String, String> {
        self.login_remembered(origin, password, now, false)
    }
    pub fn login_remembered(
        &mut self,
        origin: &str,
        password: &str,
        now: Instant,
        remember: bool,
    ) -> Result<String, String> {
        if !self.origin_allowed(origin) {
            return Err("Browser sharing is disabled or this origin is not allowed".into());
        }
        self.attempts
            .retain(|at| now.saturating_duration_since(*at) < Duration::from_secs(60));
        if self.attempts.len() >= 20 {
            return Err("Too many login attempts; wait one minute".into());
        }
        self.attempts.push(now);
        if !self
            .password
            .as_ref()
            .is_some_and(|expected| equal(expected, &hash(password)))
        {
            return Err("Invalid sharing password".into());
        }
        self.sessions.retain(|_, session| session.expiry > now);
        if self.sessions.len() >= 32 {
            return Err("Too many browser sessions; reset sharing to revoke them".into());
        }
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        self.sessions.insert(
            token_hash(&token),
            Session {
                origin: origin.into(),
                expiry: now
                    + Duration::from_secs(if remember {
                        REMEMBER_SECONDS
                    } else {
                        12 * 60 * 60
                    }),
                remembered: remember,
            },
        );
        Ok(token)
    }
    pub fn authorized(&self, origin: &str, token: &str, now: Instant) -> bool {
        self.origin_allowed(origin)
            && self
                .sessions
                .get(&token_hash(token))
                .is_some_and(|session| session.origin == origin && session.expiry > now)
    }
    pub fn logout(&mut self, token: &str) {
        self.sessions.remove(&token_hash(token));
    }
    /// Persist only hashes, bound origins and absolute expiry in the encrypted store.
    pub fn saved_sessions(&self, now: Instant, wall_seconds: u64) -> Vec<SavedSession> {
        self.sessions
            .iter()
            .filter(|(_, session)| session.remembered && session.expiry > now)
            .map(|(token_hash, session)| SavedSession {
                token_hash: token_hash.clone(),
                origin: session.origin.clone(),
                expires_at: wall_seconds
                    .saturating_add(session.expiry.duration_since(now).as_secs()),
            })
            .collect()
    }
    pub fn restore_sessions(
        &mut self,
        sessions: Vec<SavedSession>,
        now: Instant,
        wall_seconds: u64,
    ) {
        for saved in sessions.into_iter().take(32) {
            let remaining = saved.expires_at.saturating_sub(wall_seconds);
            if remaining == 0
                || remaining > REMEMBER_SECONDS
                || !self.origin_allowed(&saved.origin)
                || saved.token_hash.len() != 64
                || !saved.token_hash.bytes().all(|b| b.is_ascii_hexdigit())
            {
                continue;
            }
            self.sessions.insert(
                saved.token_hash,
                Session {
                    origin: saved.origin,
                    expiry: now + Duration::from_secs(remaining),
                    remembered: true,
                },
            );
        }
    }
}
fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sessions_require_password_origin_and_expiry_and_are_revoked() {
        let mut a = Access::default();
        let now = Instant::now();
        assert!(!a.host_allowed("localhost:9000"));
        let pass = a
            .configure(vec![
                "http://localhost:9000/".into(),
                "https://share.example".into(),
            ])
            .unwrap();
        assert!(a.host_allowed("share.example"));
        assert!(!a.host_allowed("attacker.example"));
        assert!(a.login("https://attacker.example", &pass, now).is_err());
        assert!(a.login("https://share.example", "wrong", now).is_err());
        let token = a.login("https://share.example", &pass, now).unwrap();
        assert!(a.authorized("https://share.example", &token, now));
        assert!(!a.authorized("http://localhost:9000", &token, now));
        assert!(!a.authorized(
            "https://share.example",
            &token,
            now + Duration::from_secs(43201)
        ));
        a.disable();
        assert!(!a.authorized("https://share.example", &token, now));
    }
    #[test]
    fn password_rotation_and_rate_limit() {
        let mut a = Access::default();
        let now = Instant::now();
        let origin = "https://share.example";
        let old = a.configure(vec![origin.into()]).unwrap();
        let token = a.login(origin, &old, now).unwrap();
        let new = a.configure(vec![origin.into()]).unwrap();
        assert_ne!(old, new);
        assert!(!a.authorized(origin, &token, now));
        assert!(a.login(origin, &old, now).is_err());
        for _ in 0..19 {
            assert!(a.login(origin, "wrong", now).is_err());
        }
        assert!(a.login(origin, &new, now).unwrap_err().contains("Too many"));
        assert!(a.login(origin, &new, now + Duration::from_secs(61)).is_ok());
    }
    #[test]
    fn local_password_survives_reads_and_clears_on_disable() {
        let mut gate = Access::default();
        assert!(gate.local_password().is_none());
        let origin = "https://share.example";
        let first = gate.configure(vec![origin.into()]).unwrap();
        let token = gate.login(origin, &first, Instant::now()).unwrap();
        for _ in 0..3 {
            assert_eq!(gate.local_password(), Some(first.as_str()));
            assert!(gate.enabled());
            assert!(gate.authorized(origin, &token, Instant::now()));
        }
        assert!(gate.configure(vec!["file:///invalid".into()]).is_err());
        assert_eq!(gate.local_password(), Some(first.as_str()));
        let next = gate.configure(vec![origin.into()]).unwrap();
        assert_ne!(first, next);
        assert_eq!(gate.local_password(), Some(next.as_str()));
        assert!(!gate.authorized(origin, &token, Instant::now()));
        gate.disable();
        assert!(gate.local_password().is_none());
        assert!(!gate.enabled());
    }
    #[test]
    fn rejects_unsafe_origins() {
        for url in [
            "file:///a",
            "https://u:p@host",
            "https://host/path",
            "https://host?x=1",
            "https://host/#secret",
        ] {
            assert!(normalize_origin(url).is_err());
        }
    }
}

#[cfg(test)]
mod persistent_password_tests {
    use super::*;
    #[test]
    fn custom_password_restores_in_a_fresh_gate_and_rotation_revokes_sessions() {
        let origin = "https://share.example";
        let custom = "synthetic-fixed-password";
        let mut gate = Access::default();
        gate.configure_password(vec![origin.into()], custom.into())
            .unwrap();
        let now = Instant::now();
        let token = gate.login(origin, custom, now).unwrap();
        assert!(gate
            .configure_password(vec![origin.into()], "short".into())
            .is_err());
        assert!(gate.authorized(origin, &token, now));
        let mut restarted = Access::default();
        restarted
            .configure_password(vec![origin.into()], custom.into())
            .unwrap();
        assert!(restarted.login(origin, custom, now).is_ok());
        assert!(!restarted.authorized(origin, &token, now));
        gate.configure_password(vec![origin.into()], "synthetic-changed-password".into())
            .unwrap();
        assert!(!gate.authorized(origin, &token, now));
        assert!(gate.login(origin, custom, now).is_err());
    }
}

#[cfg(test)]
mod remembered_session_tests {
    use super::*;
    #[test]
    fn remembered_sessions_survive_restart_but_temporary_sessions_do_not() {
        let origin = "https://share.example";
        let password = "synthetic-remember-password";
        let now = Instant::now();
        let mut gate = Access::default();
        gate.configure_password(vec![origin.into()], password.into())
            .unwrap();
        let remembered = gate.login_remembered(origin, password, now, true).unwrap();
        let temporary = gate.login(origin, password, now).unwrap();
        let saved = gate.saved_sessions(now, 1_000_000);
        let json = serde_json::to_string(&saved).unwrap();
        assert!(!json.contains(&remembered));
        assert!(!json.contains(&temporary));
        assert_eq!(saved.len(), 1);
        let mut restarted = Access::default();
        restarted
            .configure_password(vec![origin.into()], password.into())
            .unwrap();
        restarted.restore_sessions(saved, now, 1_000_060);
        assert!(restarted.authorized(origin, &remembered, now));
        assert!(!restarted.authorized(origin, &temporary, now));
        assert!(!restarted.authorized("https://other.example", &remembered, now));
        assert!(!restarted.authorized(
            origin,
            &remembered,
            now + Duration::from_secs(REMEMBER_SECONDS)
        ));
        restarted.logout(&remembered);
        assert!(restarted.saved_sessions(now, 1_000_060).is_empty());
    }
    #[test]
    fn expired_removed_origin_and_unbounded_saved_sessions_are_rejected() {
        let origin = "https://share.example";
        let password = "synthetic-remember-password";
        let now = Instant::now();
        let mut gate = Access::default();
        gate.configure_password(vec![origin.into()], password.into())
            .unwrap();
        let token = gate.login_remembered(origin, password, now, true).unwrap();
        let saved = gate.saved_sessions(now, 100);
        for (origins, time) in [
            (vec![origin.into()], 100 + REMEMBER_SECONDS),
            (vec!["https://other.example".into()], 100),
        ] {
            let mut fresh = Access::default();
            fresh.configure_password(origins, password.into()).unwrap();
            fresh.restore_sessions(saved.clone(), now, time);
            assert!(!fresh.authorized(origin, &token, now));
        }
        let mut excessive = saved;
        excessive[0].expires_at = u64::MAX;
        gate.sessions.clear();
        gate.restore_sessions(excessive, now, 100);
        assert!(!gate.authorized(origin, &token, now));
    }
}
