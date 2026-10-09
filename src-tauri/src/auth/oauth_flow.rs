use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::bearer::constant_time_eq_str;

pub const OAUTH_CODE_TTL_SECONDS: u64 = 300;
pub const OAUTH_TOKEN_TTL_SECONDS: i64 = 60 * 60 * 24 * 7;
pub const OAUTH_TOKEN_TTL_MAX_SECONDS: i64 = 60 * 60 * 24 * 30;
/// Refresh tokens let a client that lost (or outlived) its access token renew
/// without consuming the single-use authorization password again.
pub const OAUTH_REFRESH_TTL_SECONDS: i64 = 60 * 60 * 24 * 30;
const REFRESH_TOKEN_TYPE: &str = "refresh";
#[allow(dead_code)]
pub const OAUTH_MAX_BODY_BYTES: usize = 8_192;

/// HTTPS callback origins of hosted MCP clients (ChatGPT, Claude, VS Code web).
const OAUTH_REDIRECT_ORIGINS: &[&str] = &[
    "https://chatgpt.com",
    "https://chat.openai.com",
    "https://claude.ai",
    "https://claude.com",
    "https://vscode.dev",
    "https://insiders.vscode.dev",
];

/// Private-use URI schemes of desktop MCP clients (RFC 8252 §7.1).
const OAUTH_REDIRECT_APP_SCHEMES: &[&str] = &["cursor", "vscode", "vscode-insiders", "windsurf"];

pub type PasswordPersister = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
pub struct OAuthRuntime {
    pub client_id: String,
    pub client_secret: Option<String>,
    password: Arc<Mutex<String>>,
    pub token_secret: String,
    token_ttl_seconds: i64,
    pending: Arc<Mutex<HashMap<String, PendingCode>>>,
    password_persister: Option<PasswordPersister>,
    authorization_lock: Arc<Mutex<()>>,
}

#[derive(Clone)]
#[allow(dead_code)]
struct PendingCode {
    code_challenge: String,
    client_id: String,
    redirect_uri: String,
    state: String,
    expires_at: u64,
    server_url: String,
}

#[derive(Serialize, Deserialize)]
struct TokenClaims {
    iss: String,
    aud: String,
    iat: i64,
    exp: i64,
    scope: String,
    /// `None` for access tokens; `Some("refresh")` for refresh tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    typ: Option<String>,
    /// Client bound to a refresh token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cid: Option<String>,
}

impl OAuthRuntime {
    pub fn try_new(
        base_url: String,
        client_id: String,
        client_secret: Option<String>,
        password: Option<String>,
        token_secret: Option<String>,
    ) -> Result<Self, String> {
        Self::try_new_with_password_persister(
            base_url,
            client_id,
            client_secret,
            password,
            token_secret,
            None,
        )
    }

    pub fn try_new_with_password_persister(
        _base_url: String,
        client_id: String,
        client_secret: Option<String>,
        password: Option<String>,
        token_secret: Option<String>,
        password_persister: Option<PasswordPersister>,
    ) -> Result<Self, String> {
        let client_id = client_id.trim().to_string();
        if client_id.is_empty() {
            return Err("OAuth client ID is not configured".into());
        }
        let password = require_configured_secret(password, "OAuth password")?;
        let token_secret = require_configured_secret(token_secret, "OAuth token secret")?;
        let client_secret = client_secret.filter(|value| !value.trim().is_empty());
        Ok(Self {
            client_id,
            client_secret,
            password: Arc::new(Mutex::new(password)),
            token_secret,
            token_ttl_seconds: OAUTH_TOKEN_TTL_SECONDS,
            pending: Arc::new(Mutex::new(HashMap::new())),
            password_persister,
            authorization_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn with_token_ttl_seconds(mut self, token_ttl_seconds: u64) -> Result<Self, String> {
        if token_ttl_seconds == 0 {
            return Err("OAuth token TTL must be greater than zero".into());
        }
        self.token_ttl_seconds = token_ttl_seconds.min(OAUTH_TOKEN_TTL_MAX_SECONDS as u64) as i64;
        Ok(self)
    }

    pub fn client_id_allowed(&self, client_id: &str) -> bool {
        if client_id.is_empty() {
            return false;
        }
        if self.client_id.is_empty() {
            return true;
        }
        constant_time_eq_str(client_id, &self.client_id)
    }

    pub fn verify_access_token(&self, token: &str, server_url: &str) -> bool {
        let issuer = server_url.trim_end_matches('/');
        let resource = format!("{issuer}/mcp");
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_audience(&[resource.as_str(), issuer]);
        validation.set_issuer(&[issuer]);
        decode::<TokenClaims>(
            token,
            &DecodingKey::from_secret(self.token_secret.as_bytes()),
            &validation,
        )
        .is_ok_and(|data| data.claims.typ.is_none())
    }
}

pub fn require_configured_secret(value: Option<String>, label: &str) -> Result<String, String> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{label} is not configured"))
}

pub fn redirect_uri_allowed(redirect_uri: &str) -> bool {
    if redirect_uri.is_empty() || redirect_uri.trim() != redirect_uri {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(redirect_uri) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return false;
    }
    match url.scheme() {
        "https" => {
            if url.port_or_known_default() != Some(443) {
                return false;
            }
            let origin = url.origin().ascii_serialization();
            OAUTH_REDIRECT_ORIGINS.contains(&origin.as_str())
        }
        // Native apps receive the code on a loopback listener (RFC 8252 §7.3):
        // Claude Code, Codex CLI, Cursor, VS Code, MCP Inspector, …
        "http" => matches!(
            url.host_str(),
            Some("127.0.0.1") | Some("localhost") | Some("[::1]")
        ),
        scheme => OAUTH_REDIRECT_APP_SCHEMES.contains(&scheme),
    }
}

pub fn verify_oauth_bearer_header(
    headers: &HeaderMap,
    oauth: &OAuthRuntime,
    server_url: &str,
) -> Option<Response> {
    let Some(header_value) = headers.get(AUTHORIZATION) else {
        return Some((StatusCode::UNAUTHORIZED, "Missing Authorization header").into_response());
    };
    let Ok(header_str) = header_value.to_str() else {
        return Some((StatusCode::UNAUTHORIZED, "Invalid Authorization header").into_response());
    };
    let Some(token) = header_str.strip_prefix("Bearer ").map(str::trim) else {
        return Some((StatusCode::UNAUTHORIZED, "Invalid bearer token").into_response());
    };
    if oauth.verify_access_token(token, server_url) {
        None
    } else {
        Some((StatusCode::UNAUTHORIZED, "Invalid bearer token").into_response())
    }
}

#[derive(Debug, Deserialize)]
pub struct AuthorizeParams {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    #[serde(default)]
    pub state: String,
}

#[derive(Debug, Deserialize)]
pub struct AuthorizeForm {
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    #[serde(default)]
    pub state: String,
    pub password: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct TokenForm {
    pub grant_type: String,
    pub code: String,
    pub redirect_uri: String,
    pub code_verifier: String,
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
}

pub fn authorize_get(
    oauth: &OAuthRuntime,
    params: AuthorizeParams,
    workspace_path: Option<&str>,
) -> Response {
    if params.response_type != "code" {
        return html_error("response_type must be 'code'", StatusCode::BAD_REQUEST);
    }
    if !oauth.client_id_allowed(&params.client_id) {
        return html_error("Unknown client_id", StatusCode::BAD_REQUEST);
    }
    if !redirect_uri_allowed(&params.redirect_uri) {
        return html_error("redirect_uri is not allowed", StatusCode::BAD_REQUEST);
    }
    if params.code_challenge_method != "S256" || params.code_challenge.is_empty() {
        return html_error(
            "code_challenge_method must be S256 and code_challenge is required",
            StatusCode::BAD_REQUEST,
        );
    }
    Html(login_page(
        &params.client_id,
        &params.redirect_uri,
        &params.code_challenge,
        &params.code_challenge_method,
        &params.state,
        "",
        workspace_path,
    ))
    .into_response()
}

pub fn authorize_post(oauth: &OAuthRuntime, form: AuthorizeForm, server_url: &str) -> Response {
    if !redirect_uri_allowed(&form.redirect_uri) {
        return html_error("redirect_uri is not allowed", StatusCode::BAD_REQUEST);
    }
    if !oauth.client_id_allowed(&form.client_id) {
        return Html(login_page(
            &form.client_id,
            &form.redirect_uri,
            &form.code_challenge,
            &form.code_challenge_method,
            &form.state,
            "Invalid client",
            None,
        ))
        .into_response();
    }
    if form.code_challenge_method != "S256" || form.code_challenge.is_empty() {
        return Html(login_page(
            &form.client_id,
            &form.redirect_uri,
            &form.code_challenge,
            &form.code_challenge_method,
            &form.state,
            "Invalid PKCE parameters",
            None,
        ))
        .into_response();
    }
    let _authorization_guard = oauth
        .authorization_lock
        .lock()
        .expect("oauth authorization lock");
    let current_password = oauth.password.lock().expect("oauth password lock").clone();
    if !constant_time_eq_str(&form.password, &current_password) {
        return (
            StatusCode::UNAUTHORIZED,
            Html(login_page(
                &form.client_id,
                &form.redirect_uri,
                &form.code_challenge,
                &form.code_challenge_method,
                &form.state,
                "Invalid password",
                None,
            )),
        )
            .into_response();
    }

    let next_password =
        format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()).replace('-', "");
    if let Some(persist) = oauth.password_persister.as_ref() {
        if persist(&next_password).is_err() {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Html(login_page(
                    &form.client_id,
                    &form.redirect_uri,
                    &form.code_challenge,
                    &form.code_challenge_method,
                    &form.state,
                    "Authorization password rotation failed; try again later",
                    None,
                )),
            )
                .into_response();
        }
    }
    *oauth.password.lock().expect("oauth password lock") = next_password;

    let server_url = server_url.trim_end_matches('/').to_string();
    let code = uuid::Uuid::new_v4().to_string().replace('-', "");
    let now = unix_now();
    {
        let mut pending = oauth.pending.lock().expect("oauth pending lock");
        pending.retain(|_, v| v.expires_at >= now);
        pending.insert(
            code.clone(),
            PendingCode {
                code_challenge: form.code_challenge.clone(),
                client_id: form.client_id.clone(),
                redirect_uri: form.redirect_uri.clone(),
                state: form.state.clone(),
                expires_at: now + OAUTH_CODE_TTL_SECONDS,
                server_url: server_url.clone(),
            },
        );
    }

    let mut qs = format!("code={}", urlencoding_encode(&code));
    if !form.state.is_empty() {
        qs.push_str(&format!("&state={}", urlencoding_encode(&form.state)));
    }
    let sep = if form.redirect_uri.contains('?') {
        '&'
    } else {
        '?'
    };
    // 授权页面通过 POST 表单提交，但客户端回调必须使用 GET。
    // 307 会保留 POST 并把表单体转发到 ChatGPT connector，导致 Bad Request。
    Redirect::to(&format!("{}{}{}", form.redirect_uri, sep, qs)).into_response()
}

pub fn token_exchange(
    oauth: &OAuthRuntime,
    headers: &HeaderMap,
    mut form: TokenForm,
    server_url: &str,
) -> Response {
    if form.grant_type != "authorization_code" && form.grant_type != "refresh_token" {
        return token_error(
            "unsupported_grant_type",
            "Only authorization_code and refresh_token are supported",
        );
    }

    if let Some((id, secret)) = basic_auth_credentials(headers) {
        if form.client_id.is_empty() {
            form.client_id = id;
        }
        if form.client_secret.is_empty() {
            form.client_secret = secret;
        }
    }

    if !oauth.client_id_allowed(&form.client_id) {
        return token_error("invalid_client", "Unknown client_id");
    }
    if let Some(expected) = oauth.client_secret.as_deref() {
        if !constant_time_eq_str(&form.client_secret, expected) {
            return token_error("invalid_client", "Invalid client_secret");
        }
    }
    if form.grant_type == "refresh_token" {
        return refresh_exchange(oauth, &form, server_url);
    }
    if form.code.is_empty() {
        return token_error("invalid_grant", "code is required");
    }
    if !valid_code_verifier(&form.code_verifier) {
        return token_error("invalid_grant", "Invalid code_verifier");
    }
    if !redirect_uri_allowed(&form.redirect_uri) {
        return token_error("invalid_grant", "redirect_uri is not allowed");
    }

    let code_data = {
        let mut pending = oauth.pending.lock().expect("oauth pending lock");
        pending.remove(&form.code)
    };
    let Some(code_data) = code_data else {
        return token_error(
            "invalid_grant",
            "Unknown or already-used authorization code",
        );
    };
    if unix_now() > code_data.expires_at {
        return token_error("invalid_grant", "Authorization code expired");
    }
    if !constant_time_eq_str(&code_data.client_id, &form.client_id) {
        return token_error("invalid_grant", "client_id mismatch");
    }
    if !constant_time_eq_str(&code_data.redirect_uri, &form.redirect_uri) {
        return token_error("invalid_grant", "redirect_uri mismatch");
    }
    if !verify_pkce(&form.code_verifier, &code_data.code_challenge) {
        return token_error("invalid_grant", "PKCE verification failed");
    }

    let issuer = if code_data.server_url.trim().is_empty() {
        server_url.trim_end_matches('/').to_string()
    } else {
        code_data.server_url.trim_end_matches('/').to_string()
    };
    issue_token_pair(oauth, &issuer, &form.client_id)
}

fn refresh_audience(issuer: &str) -> String {
    format!("{issuer}/oauth/refresh")
}

fn issue_token_pair(oauth: &OAuthRuntime, issuer: &str, client_id: &str) -> Response {
    let audience = format!("{issuer}/mcp");
    let access = create_access_token(issuer, &audience, &oauth.token_secret, oauth.token_ttl_seconds);
    let refresh = create_refresh_token(issuer, client_id, &oauth.token_secret);
    match (access, refresh) {
        (Ok(access_token), Ok(refresh_token)) => (
            StatusCode::OK,
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            axum::Json(json!({
                "access_token": access_token,
                "token_type": "Bearer",
                "expires_in": oauth.token_ttl_seconds,
                "refresh_token": refresh_token,
                "scope": "mcp"
            })),
        )
            .into_response(),
        _ => token_error("server_error", "Failed to issue access token"),
    }
}

/// `grant_type=refresh_token`: verify a refresh token signed by this server for
/// the same client and issue a fresh access/refresh pair. Rotating the
/// workspace token secret revokes every outstanding refresh token.
fn refresh_exchange(oauth: &OAuthRuntime, form: &TokenForm, server_url: &str) -> Response {
    if form.refresh_token.is_empty() {
        return token_error("invalid_request", "refresh_token is required");
    }
    let issuer = server_url.trim_end_matches('/').to_string();
    let audience = refresh_audience(&issuer);
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_audience(&[audience.as_str()]);
    validation.set_issuer(&[issuer.as_str()]);
    let claims = match decode::<TokenClaims>(
        &form.refresh_token,
        &DecodingKey::from_secret(oauth.token_secret.as_bytes()),
        &validation,
    ) {
        Ok(data) => data.claims,
        Err(_) => return token_error("invalid_grant", "Invalid or expired refresh_token"),
    };
    if claims.typ.as_deref() != Some(REFRESH_TOKEN_TYPE) {
        return token_error("invalid_grant", "Invalid or expired refresh_token");
    }
    let bound_client = claims.cid.unwrap_or_default();
    if !constant_time_eq_str(&bound_client, &form.client_id) {
        return token_error("invalid_grant", "client_id mismatch");
    }
    issue_token_pair(oauth, &issuer, &form.client_id)
}

fn create_refresh_token(issuer: &str, client_id: &str, token_secret: &str) -> Result<String, ()> {
    let now = unix_now() as i64;
    let claims = TokenClaims {
        iss: issuer.to_string(),
        aud: refresh_audience(issuer),
        iat: now,
        exp: now + OAUTH_REFRESH_TTL_SECONDS,
        scope: "mcp".into(),
        typ: Some(REFRESH_TOKEN_TYPE.into()),
        cid: Some(client_id.to_string()),
    };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(token_secret.as_bytes()),
    )
    .map_err(|_| ())
}

fn create_access_token(
    issuer: &str,
    audience: &str,
    token_secret: &str,
    ttl: i64,
) -> Result<String, ()> {
    let now = unix_now() as i64;
    let claims = TokenClaims {
        iss: issuer.to_string(),
        aud: audience.to_string(),
        iat: now,
        exp: now + ttl,
        scope: "mcp".into(),
        typ: None,
        cid: None,
    };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(token_secret.as_bytes()),
    )
    .map_err(|_| ())
}

fn verify_pkce(code_verifier: &str, code_challenge: &str) -> bool {
    let digest = Sha256::digest(code_verifier.as_bytes());
    let expected = URL_SAFE_NO_PAD.encode(digest);
    constant_time_eq_str(&expected, code_challenge)
}

fn valid_code_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | '~'))
}

fn basic_auth_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    let header = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let encoded = header.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (id, secret) = text.split_once(':')?;
    Some((id.to_string(), secret.to_string()))
}

fn token_error(error: &str, description: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(json!({
            "error": error,
            "error_description": description
        })),
    )
        .into_response()
}

fn html_error(message: &str, status: StatusCode) -> Response {
    (status, Html(format!("<h2>Error</h2><p>{message}</p>"))).into_response()
}

fn login_page(
    client_id: &str,
    redirect_uri: &str,
    code_challenge: &str,
    code_challenge_method: &str,
    state: &str,
    error: &str,
    workspace_path: Option<&str>,
) -> String {
    let error_block = if error.is_empty() {
        String::new()
    } else {
        format!("<p style=\"color:red\">{}</p>", html_escape(error))
    };
    let workspace_block = workspace_path
        .filter(|path| !path.is_empty())
        .map(|path| format!("<p>Workspace: <code>{}</code></p>", html_escape(path)))
        .unwrap_or_default();
    format!(
        "<!DOCTYPE html><html lang='en'><head><meta charset='utf-8'>\
        <title>Authorize MCP Server</title>\
        <style>body{{font-family:sans-serif;max-width:380px;margin:4rem auto;padding:1rem}}\
        input{{width:100%;padding:.5rem;margin:.4rem 0;box-sizing:border-box}}\
        button{{width:100%;padding:.7rem;background:#0066cc;color:#fff;border:none;cursor:pointer}}</style>\
        </head><body>\
        <h2>Authorize Coding Tools MCP</h2>\
        {workspace_block}\
        <p>Client: <strong>{}</strong></p>\
        <p>Redirect URI: <code>{}</code></p>\
        {error_block}\
        <form method='POST' action=''>\
        <input type='hidden' name='client_id' value='{}'>\
        <input type='hidden' name='redirect_uri' value='{}'>\
        <input type='hidden' name='code_challenge' value='{}'>\
        <input type='hidden' name='code_challenge_method' value='{}'>\
        <input type='hidden' name='state' value='{}'>\
        <label>Password<input type='password' name='password' autocomplete='current-password' required></label>\
        <button type='submit'>Authorize</button>\
        </form></body></html>",
        html_escape(client_id),
        html_escape(redirect_uri),
        html_escape(client_id),
        html_escape(redirect_uri),
        html_escape(code_challenge),
        html_escape(code_challenge_method),
        html_escape(state),
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&#39;")
}

fn urlencoding_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn authorize_login_form_posts_to_current_path() {
        // Path-scoped public URLs (builtin tunnel) serve the form under
        // /builtin/clients/<id>/oauth/authorize. A root-absolute action would
        // drop that prefix and hit the public origin with a 502.
        let html = login_page(
            "chatgpt-client-test",
            "https://chatgpt.com/connector/oauth/test",
            "challenge",
            "S256",
            "state",
            "",
            Some("/tmp/workspace"),
        );
        assert!(
            html.contains("method='POST' action=''"),
            "login form must post to the current URL path, got: {html}"
        );
        assert!(
            !html.contains("action='/oauth/authorize'"),
            "login form must not use a root-absolute /oauth/authorize action"
        );
    }

    #[test]
    fn token_exchange_without_client_secret() {
        let oauth = OAuthRuntime::try_new(
            "https://lb.example.com".into(),
            "chatgpt-client-test".into(),
            None,
            Some("test-password".into()),
            Some("token-signing-secret".into()),
        )
        .expect("valid OAuth runtime");
        let verifier = "dBjftJeZ4CVP-mB92Kpru-AEJvkQlLgi3ThpmQ45N_Xyo";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let redirect_uri = "https://chatgpt.com/connector/oauth/test";
        let redirect = authorize_post(
            &oauth,
            AuthorizeForm {
                client_id: "chatgpt-client-test".into(),
                redirect_uri: redirect_uri.into(),
                code_challenge: challenge,
                code_challenge_method: "S256".into(),
                state: "state".into(),
                password: "test-password".into(),
            },
            "https://lb.example.com",
        );
        assert_eq!(redirect.status(), StatusCode::SEE_OTHER);
        let code = {
            let pending = oauth.pending.lock().expect("lock");
            pending.keys().next().cloned().unwrap()
        };

        let response = token_exchange(
            &oauth,
            &HeaderMap::new(),
            TokenForm {
                grant_type: "authorization_code".into(),
                code,
                redirect_uri: redirect_uri.into(),
                code_verifier: verifier.into(),
                client_id: "chatgpt-client-test".into(),
                client_secret: String::new(),
                ..Default::default()
            },
            "https://lb.example.com",
        );
        assert_eq!(response.status(), StatusCode::OK);
    }

    fn token_json(response: Response) -> serde_json::Value {
        let body = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(axum::body::to_bytes(response.into_body(), 1 << 20))
            .expect("token body");
        serde_json::from_slice(&body).expect("token json")
    }

    #[test]
    fn refresh_token_renews_access_without_password() {
        let base = "https://lb.example.com";
        let oauth = OAuthRuntime::try_new(
            base.into(),
            "chatgpt-client-test".into(),
            None,
            Some("test-password".into()),
            Some("token-signing-secret".into()),
        )
        .expect("valid OAuth runtime");
        let refresh = create_refresh_token(base, "chatgpt-client-test", "token-signing-secret")
            .expect("refresh token");
        // A refresh token is never accepted as a bearer access token.
        assert!(!oauth.verify_access_token(&refresh, base));

        let form = |token: &str, client: &str| TokenForm {
            grant_type: "refresh_token".into(),
            refresh_token: token.into(),
            client_id: client.into(),
            ..Default::default()
        };
        let response = token_exchange(&oauth, &HeaderMap::new(), form(&refresh, "chatgpt-client-test"), base);
        assert_eq!(response.status(), StatusCode::OK);
        let body = token_json(response);
        let access = body["access_token"].as_str().expect("access token");
        assert!(oauth.verify_access_token(access, base));
        let rotated = body["refresh_token"].as_str().expect("rotated refresh token");
        assert!(!oauth.verify_access_token(rotated, base));

        // An access token cannot be used as a refresh token.
        let misuse = token_exchange(&oauth, &HeaderMap::new(), form(access, "chatgpt-client-test"), base);
        assert_eq!(misuse.status(), StatusCode::BAD_REQUEST);
        // Garbage and empty refresh tokens are rejected.
        let garbage = token_exchange(&oauth, &HeaderMap::new(), form("not-a-token", "chatgpt-client-test"), base);
        assert_eq!(garbage.status(), StatusCode::BAD_REQUEST);
        let empty = token_exchange(&oauth, &HeaderMap::new(), form("", "chatgpt-client-test"), base);
        assert_eq!(empty.status(), StatusCode::BAD_REQUEST);
        // A token signed with another secret is rejected.
        let foreign = create_refresh_token(base, "chatgpt-client-test", "other-secret").unwrap();
        let foreign = token_exchange(&oauth, &HeaderMap::new(), form(&foreign, "chatgpt-client-test"), base);
        assert_eq!(foreign.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn refresh_token_is_bound_to_its_client() {
        let base = "https://lb.example.com";
        let oauth = OAuthRuntime::try_new(
            base.into(),
            "client-b".into(),
            None,
            Some("test-password".into()),
            Some("token-signing-secret".into()),
        )
        .expect("valid OAuth runtime");
        let refresh = create_refresh_token(base, "client-a", "token-signing-secret").unwrap();
        let response = token_exchange(
            &oauth,
            &HeaderMap::new(),
            TokenForm {
                grant_type: "refresh_token".into(),
                refresh_token: refresh,
                client_id: "client-b".into(),
                ..Default::default()
            },
            base,
        );
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn redirect_allowlist_accepts_supported_chatgpt_origins() {
        assert!(redirect_uri_allowed(
            "https://chatgpt.com/connector/oauth/test"
        ));
        assert!(redirect_uri_allowed(
            "https://chat.openai.com/aip/test/oauth/callback?source=connector"
        ));
        assert!(redirect_uri_allowed(
            "https://chatgpt.com:443/connector/oauth/test"
        ));
    }

    #[test]
    fn redirect_allowlist_accepts_common_mcp_clients() {
        for redirect_uri in [
            "https://claude.ai/api/mcp/auth_callback",
            "https://claude.com/api/mcp/auth_callback",
            "https://vscode.dev/redirect",
            "http://127.0.0.1:33418/callback",
            "http://localhost:6274/oauth/callback",
            "http://[::1]:8080/cb",
            "cursor://anysphere.cursor-mcp/oauth/callback",
        ] {
            assert!(redirect_uri_allowed(redirect_uri), "{redirect_uri}");
        }
    }

    #[test]
    fn redirect_allowlist_rejects_untrusted_or_ambiguous_urls() {
        for redirect_uri in [
            "http://127.0.0.1.attacker.example/callback",
            "http://attacker.example/callback",
            "javascript://alert(1)",
            "http://user@127.0.0.1:8080/cb",
            "http://chatgpt.com/connector/oauth/test",
            "https://attacker.example/callback",
            "https://chatgpt.com.attacker.example/callback",
            "https://chatgpt.com@attacker.example/callback",
            "https://chatgpt.com:444/connector/oauth/test",
            "https://chatgpt.com/connector/oauth/test#fragment",
            " https://chatgpt.com/connector/oauth/test",
        ] {
            assert!(!redirect_uri_allowed(redirect_uri), "{redirect_uri}");
        }
    }

    #[test]
    fn authorize_endpoints_reject_untrusted_redirect_without_issuing_code() {
        let oauth = OAuthRuntime::try_new(
            "https://lb.example.com".into(),
            "chatgpt-client-test".into(),
            None,
            Some("test-password".into()),
            Some("token-signing-secret".into()),
        )
        .expect("valid OAuth runtime");
        let get_response = authorize_get(
            &oauth,
            AuthorizeParams {
                response_type: "code".into(),
                client_id: "chatgpt-client-test".into(),
                redirect_uri: "https://attacker.example/callback".into(),
                code_challenge: "challenge".into(),
                code_challenge_method: "S256".into(),
                state: "state".into(),
            },
            None,
        );
        assert_eq!(get_response.status(), StatusCode::BAD_REQUEST);

        let post_response = authorize_post(
            &oauth,
            AuthorizeForm {
                client_id: "chatgpt-client-test".into(),
                redirect_uri: "https://attacker.example/callback".into(),
                code_challenge: "challenge".into(),
                code_challenge_method: "S256".into(),
                state: "state".into(),
                password: "test-password".into(),
            },
            "https://lb.example.com",
        );
        assert_eq!(post_response.status(), StatusCode::BAD_REQUEST);
        assert!(oauth.pending.lock().expect("lock").is_empty());
    }

    fn authorize_form(password: &str, state: &str) -> AuthorizeForm {
        AuthorizeForm {
            client_id: "chatgpt-client-test".into(),
            redirect_uri: "https://chatgpt.com/connector/oauth/test".into(),
            code_challenge: "challenge".into(),
            code_challenge_method: "S256".into(),
            state: state.into(),
            password: password.into(),
        }
    }

    #[test]
    fn authorization_password_is_single_use_under_concurrency() {
        let persisted = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let persisted_for_callback = persisted.clone();
        let oauth = OAuthRuntime::try_new_with_password_persister(
            "https://lb.example.com".into(),
            "chatgpt-client-test".into(),
            None,
            Some("test-password".into()),
            Some("token-signing-secret".into()),
            Some(std::sync::Arc::new(move |value: &str| {
                persisted_for_callback
                    .lock()
                    .expect("persisted lock")
                    .push(value.to_string());
                Ok(())
            })),
        )
        .expect("valid OAuth runtime");

        let first_runtime = oauth.clone();
        let second_runtime = oauth.clone();
        let first = std::thread::spawn(move || {
            authorize_post(
                &first_runtime,
                authorize_form("test-password", "first"),
                "https://lb.example.com",
            )
            .status()
        });
        let second = std::thread::spawn(move || {
            authorize_post(
                &second_runtime,
                authorize_form("test-password", "second"),
                "https://lb.example.com",
            )
            .status()
        });
        let statuses = [
            first.join().expect("first join"),
            second.join().expect("second join"),
        ];
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::SEE_OTHER)
                .count(),
            1
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::UNAUTHORIZED)
                .count(),
            1
        );
        assert_eq!(persisted.lock().expect("persisted lock").len(), 1);
        assert_ne!(
            oauth.password.lock().expect("password lock").as_str(),
            "test-password"
        );
    }

    #[test]
    fn authorization_password_persistence_failure_preserves_current_password() {
        let oauth = OAuthRuntime::try_new_with_password_persister(
            "https://lb.example.com".into(),
            "chatgpt-client-test".into(),
            None,
            Some("test-password".into()),
            Some("token-signing-secret".into()),
            Some(std::sync::Arc::new(|_| {
                Err("simulated persistence failure".into())
            })),
        )
        .expect("valid OAuth runtime");
        let response = authorize_post(
            &oauth,
            authorize_form("test-password", "persist-failure"),
            "https://lb.example.com",
        );
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            oauth.password.lock().expect("password lock").as_str(),
            "test-password"
        );
        assert!(oauth.pending.lock().expect("pending lock").is_empty());
    }

    #[test]
    fn oauth_token_ttl_is_configurable_with_a_thirty_day_cap() {
        let oauth = OAuthRuntime::try_new(
            "https://lb.example.com".into(),
            "chatgpt-client-test".into(),
            None,
            Some("test-password".into()),
            Some("token-signing-secret".into()),
        )
        .expect("valid OAuth runtime")
        .with_token_ttl_seconds(60 * 60)
        .expect("custom TTL");
        assert_eq!(oauth.token_ttl_seconds, 60 * 60);

        let capped = OAuthRuntime::try_new(
            "https://lb.example.com".into(),
            "chatgpt-client-test".into(),
            None,
            Some("test-password".into()),
            Some("token-signing-secret".into()),
        )
        .expect("valid OAuth runtime")
        .with_token_ttl_seconds(365 * 24 * 60 * 60)
        .expect("capped TTL");
        assert_eq!(capped.token_ttl_seconds, OAUTH_TOKEN_TTL_MAX_SECONDS);

        let zero = OAuthRuntime::try_new(
            "https://lb.example.com".into(),
            "chatgpt-client-test".into(),
            None,
            Some("test-password".into()),
            Some("token-signing-secret".into()),
        )
        .expect("valid OAuth runtime")
        .with_token_ttl_seconds(0);
        assert!(zero.is_err());
    }

    #[test]
    fn configured_secret_rejects_missing_empty_or_whitespace_values() {
        assert!(require_configured_secret(None, "test secret").is_err());
        assert!(require_configured_secret(Some(String::new()), "test secret").is_err());
        assert!(require_configured_secret(Some("   ".into()), "test secret").is_err());
        assert_eq!(
            require_configured_secret(Some("configured".into()), "test secret")
                .expect("configured secret"),
            "configured"
        );
    }

    #[test]
    fn oauth_runtime_rejects_missing_required_credentials() {
        assert!(OAuthRuntime::try_new(
            "https://lb.example.com".into(),
            " ".into(),
            None,
            Some("password".into()),
            Some("token-secret".into()),
        )
        .is_err());
        assert!(OAuthRuntime::try_new(
            "https://lb.example.com".into(),
            "client".into(),
            None,
            None,
            Some("token-secret".into()),
        )
        .is_err());
        assert!(OAuthRuntime::try_new(
            "https://lb.example.com".into(),
            "client".into(),
            None,
            Some("password".into()),
            Some("   ".into()),
        )
        .is_err());
    }

    #[test]
    fn pkce_round_trip() {
        let verifier = "dBjftJeZ4CVP-mB92Kpru-AEJvkQlLgi3ThpmQ45N_Xyo";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert!(verify_pkce(verifier, &challenge));
    }
}
