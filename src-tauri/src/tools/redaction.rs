use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::workspace::SecurityPolicy;

const REDACTED: &str = "[REDACTED]";

pub struct OutputRedactionContext {
    tool_name: String,
    sensitive_source: bool,
    redact_sensitive_output: bool,
    withhold_sensitive_source_output: bool,
}

impl OutputRedactionContext {
    pub fn new(tool_name: &str, arguments: &Value) -> Self {
        Self::with_options(tool_name, arguments, true, true)
    }

    pub fn new_with_policy(tool_name: &str, arguments: &Value, policy: &SecurityPolicy) -> Self {
        Self::with_options(
            tool_name,
            arguments,
            policy.redact_sensitive_output,
            policy.withhold_sensitive_source_output,
        )
    }

    fn with_options(
        tool_name: &str,
        arguments: &Value,
        redact_sensitive_output: bool,
        withhold_sensitive_source_output: bool,
    ) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            sensitive_source: arguments_reference_sensitive_source(arguments),
            redact_sensitive_output,
            withhold_sensitive_source_output,
        }
    }

    pub fn redact(&self, mut value: Value) -> Value {
        let mut redaction_count = 0u64;
        if self.withhold_sensitive_source_output {
            redact_sensitive_source_output(self, &mut value, &mut redaction_count);
        }
        if self.redact_sensitive_output {
            redact_value(&mut value, None, &mut redaction_count);
        }
        if redaction_count == 0 {
            return value;
        }

        if let Some(object) = value.as_object_mut() {
            object.insert("sensitive_data_redacted".into(), Value::Bool(true));
            object.insert("redaction_count".into(), json!(redaction_count));
            append_warning(
                object,
                "Sensitive values were automatically redacted from the tool result.",
            );
        }
        value
    }
}

pub fn redact_tool_output(tool_name: &str, arguments: &Value, value: Value) -> Value {
    OutputRedactionContext::new(tool_name, arguments).redact(value)
}

pub fn redact_tool_output_with_policy(
    tool_name: &str,
    arguments: &Value,
    value: Value,
    policy: &SecurityPolicy,
) -> Value {
    OutputRedactionContext::new_with_policy(tool_name, arguments, policy).redact(value)
}

fn references_secret_input(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(references_secret_input),
        Value::Object(values) => {
            values
                .get("stdin_secret")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
                || values
                    .get("secret_env")
                    .and_then(Value::as_object)
                    .is_some_and(|value| !value.is_empty())
                || values.values().any(references_secret_input)
        }
        _ => false,
    }
}

pub fn arguments_reference_sensitive_source(arguments: &Value) -> bool {
    references_secret_input(arguments)
        || serde_json::to_string(arguments)
            .ok()
            .is_some_and(|serialized| contains_sensitive_path(&serialized))
}

pub fn contains_sensitive_path(value: &str) -> bool {
    let normalized = value
        .replace('\\', "/")
        .to_ascii_lowercase()
        .replace(".env.example", "")
        .replace(".env.sample", "")
        .replace(".env.template", "");

    let named_secret = [
        "profiles.json",
        "secrets.json",
        "secret.json",
        ".npmrc",
        ".pypirc",
        ".netrc",
        "/credentials",
        "credentials.json",
        "service-account.json",
        "service_account.json",
        "id_rsa",
        "id_ed25519",
        ".pem",
        ".p12",
        ".pfx",
        ".key",
        "/.ssh/",
        "/.aws/credentials",
        "/keyring",
    ]
    .iter()
    .any(|needle| normalized.contains(needle));
    let env_file = normalized.contains("/.env")
        || normalized.contains(" .env")
        || normalized.contains("' .env")
        || normalized.ends_with(".env")
        || normalized.contains("\".env\"");
    named_secret || env_file
}

pub fn redact_value(value: &mut Value, key: Option<&str>, redaction_count: &mut u64) {
    if key.is_some_and(is_sensitive_key) {
        if value.as_str() != Some(REDACTED) {
            *value = Value::String(REDACTED.into());
            *redaction_count += 1;
        }
        return;
    }

    match value {
        Value::Object(object) => {
            for (child_key, child_value) in object.iter_mut() {
                redact_value(child_value, Some(child_key), redaction_count);
            }
        }
        Value::Array(values) => {
            for child in values {
                redact_value(child, None, redaction_count);
            }
        }
        Value::String(text) => {
            let (redacted, count) = redact_sensitive_text(text);
            if count > 0 {
                *text = redacted;
                *redaction_count += count;
            }
        }
        _ => {}
    }
}

pub fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '.', ' '], "_");

    if normalized.ends_with("_count")
        || normalized.ends_with("_bytes")
        || normalized.ends_with("_duration")
        || normalized.ends_with("_duration_ms")
        || normalized.ends_with("_length")
        || normalized.ends_with("_present")
        || normalized.ends_with("_available")
    {
        return false;
    }

    normalized == "stdin"
        || normalized == "chars"
        || normalized == "authorization"
        || normalized == "cookie"
        || normalized == "set_cookie"
        || normalized == "credential"
        || normalized == "credentials"
        || normalized == "api_key"
        || normalized == "apikey"
        || normalized == "private_key"
        || normalized == "client_secret"
        || normalized == "shared_secrets"
        || normalized == "workspace_secrets"
        || normalized == "app_secrets"
        || normalized.contains("password")
        || normalized.contains("passwd")
        || normalized.contains("private_key")
        || normalized.ends_with("_secret")
        || normalized.starts_with("secret_")
        || normalized == "secret"
        || normalized.ends_with("_token")
        || normalized.starts_with("token_")
        || normalized == "token"
        || normalized.ends_with("_credential")
        || normalized.ends_with("_credentials")
}

pub fn redact_sensitive_text(value: &str) -> (String, u64) {
    static JSON_QUOTED_RE: OnceLock<Regex> = OnceLock::new();
    static KEY_VALUE_RE: OnceLock<Regex> = OnceLock::new();
    static SECRET_FLAG_RE: OnceLock<Regex> = OnceLock::new();
    static BEARER_RE: OnceLock<Regex> = OnceLock::new();
    static BASIC_AUTH_URL_RE: OnceLock<Regex> = OnceLock::new();
    static PRIVATE_KEY_RE: OnceLock<Regex> = OnceLock::new();
    static JWT_RE: OnceLock<Regex> = OnceLock::new();
    static KNOWN_TOKEN_RE: OnceLock<Regex> = OnceLock::new();

    let json_quoted_re = JSON_QUOTED_RE.get_or_init(|| {
        Regex::new(
            r#"(?i)(\"(?:password|passwd|secret|token|api[-_]?key|authorization|credential|private[-_]?key|client[-_]?secret|bearer[-_]?token|oauth[-_]?password|oauth[-_]?token[-_]?secret)\"\s*:\s*)\"(?:\\.|[^\"\\])*\""#,
        )
        .expect("valid JSON secret regex")
    });
    let key_value_re = KEY_VALUE_RE.get_or_init(|| {
        Regex::new(
            r#"(?i)\b((?:password|passwd|secret|token|api[-_]?key|authorization|credential|private[-_]?key|client[-_]?secret|bearer[-_]?token|oauth[-_]?password|oauth[-_]?token[-_]?secret)\s*[:=]\s*)(?:(?:\"(?:\\.|[^\"\\])*\")|(?:'[^']*')|[^\s;,}\]]+)"#,
        )
        .expect("valid key/value secret regex")
    });
    let flag_re = SECRET_FLAG_RE.get_or_init(|| {
        Regex::new(
            r#"(?i)(--?(?:password|passwd|secret|token|api[-_]?key|authorization|credential|private[-_]?key|client[-_]?secret)(?:\s+|=))(?:(?:\"[^\"]*\")|(?:'[^']*')|[^\s]+)"#,
        )
        .expect("valid secret flag regex")
    });
    let bearer_re = BEARER_RE.get_or_init(|| {
        Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]+").expect("valid bearer regex")
    });
    let basic_auth_url_re = BASIC_AUTH_URL_RE.get_or_init(|| {
        Regex::new(r"(?i)(https?://[^\s:/@]+:)[^\s/@]+(@)").expect("valid URL auth regex")
    });
    let private_key_re = PRIVATE_KEY_RE.get_or_init(|| {
        Regex::new(r"(?s)-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----.*?-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----")
            .expect("valid private key regex")
    });
    let jwt_re = JWT_RE.get_or_init(|| {
        Regex::new(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b")
            .expect("valid JWT regex")
    });
    let known_token_re = KNOWN_TOKEN_RE.get_or_init(|| {
        Regex::new(
            r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16})\b",
        )
        .expect("valid known token regex")
    });

    let mut count = 0u64;
    let mut redacted = replace_count(private_key_re, value, REDACTED, &mut count);
    redacted = replace_count(jwt_re, &redacted, REDACTED, &mut count);
    redacted = replace_count(known_token_re, &redacted, REDACTED, &mut count);
    redacted = replace_count(bearer_re, &redacted, "$1[REDACTED]", &mut count);
    redacted = replace_count(basic_auth_url_re, &redacted, "$1[REDACTED]$2", &mut count);
    redacted = replace_count(json_quoted_re, &redacted, "$1\"[REDACTED]\"", &mut count);
    redacted = replace_count(flag_re, &redacted, "$1[REDACTED]", &mut count);
    redacted = replace_count(key_value_re, &redacted, "$1[REDACTED]", &mut count);
    (redacted, count)
}

fn replace_count(regex: &Regex, input: &str, replacement: &str, count: &mut u64) -> String {
    let matches = regex.find_iter(input).count() as u64;
    if matches == 0 {
        return input.to_string();
    }
    *count += matches;
    regex.replace_all(input, replacement).into_owned()
}

fn append_warning(object: &mut Map<String, Value>, message: &str) {
    let warnings = object
        .entry("warnings")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(warnings) = warnings.as_array_mut() else {
        return;
    };
    if !warnings.iter().any(|value| value.as_str() == Some(message)) {
        warnings.push(Value::String(message.into()));
    }
}

fn redact_sensitive_source_output(
    context: &OutputRedactionContext,
    value: &mut Value,
    redaction_count: &mut u64,
) {
    match context.tool_name.as_str() {
        "exec_command" | "exec_many" if context.sensitive_source => {
            redact_named_fields(value, &["stdout", "stderr", "data"], redaction_count);
        }
        "read_file" => {
            if context.sensitive_source {
                redact_named_fields(value, &["content"], redaction_count);
            }
        }
        "read_many" => {
            redact_path_scoped_fields(value, &["content"], redaction_count);
        }
        "search_text" => {
            redact_path_scoped_fields(
                value,
                &["match", "preview", "before", "after", "content"],
                redaction_count,
            );
        }
        "git_diff" => {
            if context.sensitive_source || value_contains_sensitive_path(value) {
                redact_named_fields(value, &["diff"], redaction_count);
            }
        }
        "git_show" => {
            if context.sensitive_source || value_contains_sensitive_path(value) {
                redact_named_fields(value, &["content"], redaction_count);
            }
        }
        "git_blame" if context.sensitive_source => {
            redact_named_fields(value, &["content", "line", "text"], redaction_count);
        }
        _ => {}
    }
}

fn value_contains_sensitive_path(value: &Value) -> bool {
    serde_json::to_string(value)
        .ok()
        .is_some_and(|serialized| contains_sensitive_path(&serialized))
}

fn redact_named_fields(value: &mut Value, fields: &[&str], redaction_count: &mut u64) {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                if fields.iter().any(|field| key == field) {
                    if child.as_str() != Some(REDACTED) && !child.is_null() {
                        *child = Value::String(REDACTED.into());
                        *redaction_count += 1;
                    }
                } else {
                    redact_named_fields(child, fields, redaction_count);
                }
            }
        }
        Value::Array(values) => {
            for child in values {
                redact_named_fields(child, fields, redaction_count);
            }
        }
        _ => {}
    }
}

fn redact_path_scoped_fields(value: &mut Value, fields: &[&str], redaction_count: &mut u64) {
    match value {
        Value::Object(object) => {
            let sensitive = object
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(contains_sensitive_path);
            if sensitive {
                for field in fields {
                    if let Some(child) = object.get_mut(*field) {
                        if child.as_str() != Some(REDACTED) && !child.is_null() {
                            *child = Value::String(REDACTED.into());
                            *redaction_count += 1;
                        }
                    }
                }
            }
            for child in object.values_mut() {
                redact_path_scoped_fields(child, fields, redaction_count);
            }
        }
        Value::Array(values) => {
            for child in values {
                redact_path_scoped_fields(child, fields, redaction_count);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_nested_secret_keys_and_embedded_structured_text() {
        let output = redact_tool_output(
            "server_info",
            &json!({}),
            json!({
                "ok": true,
                "oauth_password": "top-secret",
                "stdout": r#"{"bearer_token":"abc123","safe":"visible"} @{api_key=hidden; name=demo}"#,
                "nested": {"shared_secrets": {"client_secret": "never-show"}},
                "token_count": 9
            }),
        );

        let serialized = output.to_string();
        assert!(!serialized.contains("top-secret"));
        assert!(!serialized.contains("abc123"));
        assert!(!serialized.contains("hidden"));
        assert!(!serialized.contains("never-show"));
        assert!(serialized.contains("visible"));
        assert_eq!(output["token_count"], 9);
        assert_eq!(output["sensitive_data_redacted"], true);
    }

    #[test]
    fn redacts_bearer_jwt_private_keys_and_known_token_prefixes() {
        let input = "Authorization: Bearer abc.def.ghi eyJabcdefgh.abcdefgh.abcdefgh ghp_abcdefghijklmnopqrstuvwxyz123456 -----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----";
        let (redacted, count) = redact_sensitive_text(input);
        assert!(count >= 4, "{redacted}");
        assert!(!redacted.contains("abc.def.ghi"));
        assert!(!redacted.contains("eyJabcdefgh"));
        assert!(!redacted.contains("ghp_"));
        assert!(!redacted.contains("BEGIN PRIVATE KEY"));
    }

    #[test]
    fn does_not_redact_non_secret_metrics_or_hashes() {
        let output = redact_tool_output(
            "server_info",
            &json!({}),
            json!({
                "token_count": 42,
                "toolset_revision": "8bca4fb80d5d74d9",
                "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            }),
        );
        assert_eq!(output["token_count"], 42);
        assert_eq!(output["toolset_revision"], "8bca4fb80d5d74d9");
        assert!(output.get("sensitive_data_redacted").is_none());
    }

    #[test]
    fn sensitive_source_redacts_bare_output_without_secret_labels() {
        let output = redact_tool_output(
            "exec_command",
            &json!({"script": "$p='profiles.json'; Get-Content $p"}),
            json!({"ok": true, "stdout": "bare-value-with-no-label", "stderr": ""}),
        );
        assert_eq!(output["stdout"], REDACTED);
        assert_eq!(output["stderr"], REDACTED);
        assert_eq!(output["sensitive_data_redacted"], true);
    }

    #[test]
    fn detects_sensitive_paths_but_allows_templates() {
        assert!(contains_sensitive_path(
            r"C:\Users\demo\AppData\Roaming\coding-tools-mcp-desktop\data\profiles.json"
        ));
        assert!(contains_sensitive_path("./.env"));
        assert!(contains_sensitive_path("~/.ssh/id_ed25519"));
        assert!(!contains_sensitive_path("./.env.example"));
        assert!(!contains_sensitive_path("src/profile.rs"));
    }

    #[test]
    fn redacts_sensitive_read_and_git_payloads() {
        let read = redact_tool_output(
            "read_file",
            &json!({"path": ".env"}),
            json!({"path": ".env", "content": "UNLABELED_VALUE"}),
        );
        assert_eq!(read["content"], REDACTED);

        let diff = redact_tool_output(
            "git_diff",
            &json!({}),
            json!({
                "diff": "diff --git a/.env b/.env\n+UNLABELED_VALUE",
                "files": [".env"]
            }),
        );
        assert_eq!(diff["diff"], REDACTED);
        assert!(!diff.to_string().contains("UNLABELED_VALUE"));
    }
}
