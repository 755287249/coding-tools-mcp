use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Access {
    password: Option<[u8; 32]>,
    // Kept only in this process for the local desktop settings page. Never serialized to disk.
    display_password: Option<String>,
    origins: Vec<String>,
    sessions: HashMap<String, (String, Instant)>,
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
        let origins = origins
            .iter()
            .map(|o| normalize_origin(o))
            .collect::<Result<Vec<_>, _>>()?;
        if origins.is_empty() || origins.len() > 32 {
            return Err("Choose 1–32 browser origins".into());
        }
        let password = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
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
        self.sessions.retain(|_, (_, expiry)| *expiry > now);
        if self.sessions.len() >= 32 {
            return Err("Too many browser sessions; reset sharing to revoke them".into());
        }
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        self.sessions.insert(
            token.clone(),
            (origin.into(), now + Duration::from_secs(12 * 60 * 60)),
        );
        Ok(token)
    }
    pub fn authorized(&self, origin: &str, token: &str, now: Instant) -> bool {
        self.origin_allowed(origin)
            && self
                .sessions
                .get(token)
                .is_some_and(|(bound, expiry)| bound == origin && *expiry > now)
    }
    pub fn logout(&mut self, token: &str) {
        self.sessions.remove(token);
    }
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
