use axum::http::HeaderMap;
use serde_json::{json, Value};

use crate::workspace::AuthConfig;

impl AuthConfig {
    pub fn oauth_enabled(&self) -> bool {
        self.auth_type == "oauth"
    }

    pub fn bearer_enabled(&self) -> bool {
        self.auth_type == "bearer"
    }

    pub fn auth_enabled(&self) -> bool {
        self.auth_type != "noauth"
    }
}

/// Resolve the external OAuth/MCP base URL for a request.
/// Matches the Python server's behavior: prefer configured URL,
/// then `X-Forwarded-*` / `Host`, then localhost.
pub fn external_base_url(headers: &HeaderMap, bind_port: u16, configured_url: &str) -> String {
    let configured = configured_url.trim().trim_end_matches('/');
    if !configured.is_empty() {
        if let Some(live) = rotated_quick_tunnel_base(headers, configured) {
            return live;
        }
        return configured.to_string();
    }

    let proto = {
        let value = first_header_value(headers, "x-forwarded-proto");
        if value.is_empty() {
            forwarded_header_param(headers, "proto")
        } else {
            value
        }
    };
    let host = {
        let value = safe_external_host(&first_header_value(headers, "x-forwarded-host"));
        if !value.is_empty() {
            value
        } else {
            let value = safe_external_host(&forwarded_header_param(headers, "host"));
            if !value.is_empty() {
                value
            } else {
                safe_external_host(&first_header_value(headers, "host"))
            }
        }
    };

    let host = if host.is_empty() {
        format!("127.0.0.1:{bind_port}")
    } else {
        host
    };
    let proto = resolve_external_proto(
        if proto.is_empty() {
            None
        } else {
            Some(proto.as_str())
        },
        &host,
    );
    format!("{proto}://{host}")
}

const QUICK_TUNNEL_SUFFIX: &str = ".trycloudflare.com";

/// Cloudflare Quick Tunnels receive a new random `*.trycloudflare.com` host
/// every time cloudflared starts, but a listener captures the previously
/// persisted public URL when it is spawned (before the tunnel reports its new
/// URL). When the configured URL and the incoming request are both Quick
/// Tunnel hosts and they differ, advertise the live request host so OAuth
/// metadata never points at an expired tunnel.
fn rotated_quick_tunnel_base(headers: &HeaderMap, configured: &str) -> Option<String> {
    let rest = configured
        .strip_prefix("https://")
        .or_else(|| configured.strip_prefix("http://"))?;
    let (configured_authority, configured_path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    let configured_host = strip_port(configured_authority).to_ascii_lowercase();
    if !is_quick_tunnel_host(&configured_host) {
        return None;
    }

    let request_authority = {
        let value = safe_external_host(&first_header_value(headers, "x-forwarded-host"));
        if !value.is_empty() {
            value
        } else {
            let value = safe_external_host(&forwarded_header_param(headers, "host"));
            if !value.is_empty() {
                value
            } else {
                safe_external_host(&first_header_value(headers, "host"))
            }
        }
    };
    let request_host = strip_port(&request_authority).to_ascii_lowercase();
    if !is_quick_tunnel_host(&request_host) || request_host == configured_host {
        return None;
    }

    Some(format!(
        "https://{request_host}{}",
        configured_path.trim_end_matches('/')
    ))
}

fn strip_port(authority: &str) -> &str {
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(']') && port.chars().all(|ch| ch.is_ascii_digit()) => {
            host
        }
        _ => authority,
    }
}

fn is_quick_tunnel_host(host: &str) -> bool {
    host.len() > QUICK_TUNNEL_SUFFIX.len()
        && host.ends_with(QUICK_TUNNEL_SUFFIX)
        && host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '.')
}

fn first_header_value(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(',').next().unwrap_or("").trim().to_string())
        .unwrap_or_default()
}

fn forwarded_header_param(headers: &HeaderMap, name: &str) -> String {
    let first = first_header_value(headers, "forwarded");
    for part in first.split(';') {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=') {
            if key.trim().eq_ignore_ascii_case(name) {
                return value.trim().trim_matches('"').to_string();
            }
        }
    }
    String::new()
}

fn safe_external_host(host: &str) -> String {
    let host = host.trim();
    if host.is_empty()
        || host
            .chars()
            .any(|ch| matches!(ch, '\r' | '\n' | '/' | '\\'))
    {
        String::new()
    } else {
        host.to_string()
    }
}

fn resolve_external_proto(proto: Option<&str>, host: &str) -> &'static str {
    if let Some(proto) = proto {
        let proto = proto.trim().to_ascii_lowercase();
        if proto == "http" {
            return "http";
        }
        if proto == "https" {
            return "https";
        }
    }

    let host_without_port = host
        .rsplit_once(':')
        .map(|(value, _)| value.trim_matches('[').trim_matches(']'))
        .unwrap_or_else(|| host.trim_matches('[').trim_matches(']'));
    if is_loopback_host(host_without_port) {
        "http"
    } else {
        "https"
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn token_endpoint_auth_methods(client_secret: Option<&str>) -> Vec<&'static str> {
    match client_secret {
        Some(secret) if !secret.is_empty() => {
            vec!["client_secret_post", "client_secret_basic"]
        }
        _ => vec!["none"],
    }
}

pub fn protected_resource_metadata_url(base_url: &str) -> String {
    well_known_url(base_url, "oauth-protected-resource", true)
}

fn well_known_url(base_url: &str, suffix: &str, include_mcp: bool) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let Ok(mut url) = reqwest::Url::parse(base) else {
        let resource_suffix = if include_mcp { "/mcp" } else { "" };
        return format!("{base}/.well-known/{suffix}{resource_suffix}");
    };
    let base_path = url.path().trim_end_matches('/');
    let resource_path = if include_mcp {
        format!("{base_path}/mcp")
    } else {
        base_path.to_string()
    };
    url.set_path(&format!("/.well-known/{suffix}{resource_path}"));
    url.set_query(None);
    url.set_fragment(None);
    url.as_str().trim_end_matches('/').to_string()
}

pub fn authorization_server_metadata(base_url: &str, client_secret: Option<&str>) -> Value {
    let base = base_url.trim_end_matches('/');
    let methods = token_endpoint_auth_methods(client_secret);
    json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": methods,
    })
}

pub fn protected_resource_metadata(base_url: &str) -> Value {
    let base = base_url.trim_end_matches('/');
    json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["mcp"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_enabled_only_for_oauth_type() {
        let mut auth = AuthConfig::default();
        assert!(auth.oauth_enabled());
        auth.auth_type = "bearer".into();
        assert!(!auth.oauth_enabled());
        auth.auth_type = "noauth".into();
        assert!(!auth.oauth_enabled());
    }

    #[test]
    fn authorization_metadata_includes_token_auth_methods() {
        let meta = authorization_server_metadata("https://example.com", None);
        assert_eq!(
            meta["token_endpoint_auth_methods_supported"],
            json!(["none"])
        );
        let meta = authorization_server_metadata("https://example.com", Some("secret"));
        assert_eq!(
            meta["token_endpoint_auth_methods_supported"],
            json!(["client_secret_post", "client_secret_basic"])
        );
    }
    #[test]
    fn path_scoped_well_known_urls_follow_rfc_layout() {
        let base = "https://example.com/clients/pc-a";
        assert_eq!(
            well_known_url(base, "oauth-authorization-server", false),
            "https://example.com/.well-known/oauth-authorization-server/clients/pc-a"
        );
        assert_eq!(
            protected_resource_metadata_url(base),
            "https://example.com/.well-known/oauth-protected-resource/clients/pc-a/mcp"
        );
    }

    #[test]
    fn root_resource_metadata_url_includes_mcp_resource_path() {
        assert_eq!(
            protected_resource_metadata_url("https://example.com"),
            "https://example.com/.well-known/oauth-protected-resource/mcp"
        );
    }

    #[test]
    fn protected_resource_metadata_lists_authorization_servers() {
        let meta = protected_resource_metadata("https://example.com");
        assert_eq!(
            meta["authorization_servers"],
            json!(["https://example.com"])
        );
        assert_eq!(meta["resource"], "https://example.com/mcp");
    }

    #[test]
    fn external_base_url_prefers_configured_url() {
        let headers = HeaderMap::new();
        assert_eq!(
            external_base_url(&headers, 28767, "https://lb.frp-tx1.evwali.com"),
            "https://lb.frp-tx1.evwali.com"
        );
    }

    #[test]
    fn external_base_url_follows_rotated_quick_tunnel_host() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "host",
            "layer-personalized-coleman-frame.trycloudflare.com"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            external_base_url(
                &headers,
                28766,
                "https://minneapolis-keep-fresh-three.trycloudflare.com"
            ),
            "https://layer-personalized-coleman-frame.trycloudflare.com"
        );
    }

    #[test]
    fn external_base_url_keeps_configured_quick_tunnel_for_other_hosts() {
        let configured = "https://minneapolis-keep-fresh-three.trycloudflare.com";
        let mut local = HeaderMap::new();
        local.insert("host", "127.0.0.1:28766".parse().unwrap());
        assert_eq!(external_base_url(&local, 28766, configured), configured);

        let mut foreign = HeaderMap::new();
        foreign.insert("host", "evil.example.com".parse().unwrap());
        assert_eq!(external_base_url(&foreign, 28766, configured), configured);
    }

    #[test]
    fn external_base_url_does_not_rewrite_non_quick_tunnel_config() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "abc.trycloudflare.com".parse().unwrap());
        assert_eq!(
            external_base_url(&headers, 28767, "https://lb.frp-tx1.evwali.com"),
            "https://lb.frp-tx1.evwali.com"
        );
    }

    #[test]
    fn external_base_url_uses_forwarded_host() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        headers.insert("x-forwarded-host", "lb.frp-tx1.evwali.com".parse().unwrap());
        assert_eq!(
            external_base_url(&headers, 28767, ""),
            "https://lb.frp-tx1.evwali.com"
        );
    }

    #[test]
    fn external_base_url_uses_host_header() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "lb.frp-tx1.evwali.com".parse().unwrap());
        assert_eq!(
            external_base_url(&headers, 28767, ""),
            "https://lb.frp-tx1.evwali.com"
        );
    }
}
