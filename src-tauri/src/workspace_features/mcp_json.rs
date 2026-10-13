//! Writes MCP servers added from the UI into a project's `.mcp.json`.

use std::path::Path;

use serde_json::{Map, Value};

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphanumeric())
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' '))
}

/// Merges `servers` into the `mcpServers` object of `path`. Existing servers with the
/// same name are replaced and every other entry is kept. A file that is not valid
/// JSON is an error, so user configuration is never clobbered.
pub fn merge_mcp_json(path: &Path, servers: &Map<String, Value>) -> Result<Vec<String>, String> {
    if servers.is_empty() || servers.len() > 64 {
        return Err("servers must contain 1 to 64 entries".into());
    }
    for (name, server) in servers {
        if !valid_name(name) {
            return Err(format!("invalid MCP server name: {name}"));
        }
        let target = |key: &str| {
            server
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        };
        if !server.is_object() || !(target("command") || target("url")) {
            return Err(format!("MCP server {name} needs a command or url"));
        }
    }
    let mut document = match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Value::Object(Map::new()),
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|error| format!("{} is not valid JSON: {error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(error) => return Err(error.to_string()),
    };
    let root = document
        .as_object_mut()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    let existing = root
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| format!("mcpServers in {} must be an object", path.display()))?;
    for (name, server) in servers {
        existing.insert(name.clone(), server.clone());
    }
    let text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())? + "\n";
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, path).map_err(|error| error.to_string())?;
    Ok(servers.keys().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merges_servers_and_keeps_other_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, r#"{"mcpServers":{"old":{"command":"a"}},"other":1}"#).unwrap();
        let servers = json!({"new":{"url":"https://example.com/mcp"}});
        let names = merge_mcp_json(&path, servers.as_object().unwrap()).expect("merge");
        assert_eq!(names, vec!["new".to_string()]);
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["other"], 1);
        assert_eq!(saved["mcpServers"]["old"]["command"], "a");
        assert_eq!(saved["mcpServers"]["new"]["url"], "https://example.com/mcp");
    }

    #[test]
    fn rejects_bad_input_without_touching_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, "not json").unwrap();
        let ok = json!({"x":{"command":"a"}});
        assert!(merge_mcp_json(&path, ok.as_object().unwrap()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
        let missing = json!({"x":{"args":[]}});
        assert!(merge_mcp_json(&dir.path().join("b.json"), missing.as_object().unwrap()).is_err());
        let bad_name = json!({"../x":{"command":"a"}});
        assert!(merge_mcp_json(&dir.path().join("c.json"), bad_name.as_object().unwrap()).is_err());
    }
}
