//! Project share link: one URL that carries both the credential and the project.
//! The server binds the folder, so the share prompt never mentions headers or folder IDs.
use super::{err, io, now, safe, Result, DIR};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

// The extra dot keeps the name outside the chat ID pattern (`<id>.json`), so chat
// listing never treats it as an archive; like `seed-library.seeds.json`.
const FILE: &str = "project.share.json";
/// v0.1.158 stored the link as `share-link.json`, which the chat list read as a chat.
pub const LEGACY_STEM: &str = "share-link";
pub const ACCESS_READONLY: &str = "readonly";
pub const ACCESS_FULL: &str = "full";

fn file(root: &Path) -> Result<std::path::PathBuf> {
    safe(root, &format!("{DIR}/{FILE}"))
}

fn digest(key: &str) -> String {
    format!("{:x}", Sha256::digest(key.as_bytes()))
}

pub fn valid_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Moves the v0.1.158 file out of the chat ID namespace. Best effort and idempotent.
pub fn migrate_legacy(root: &Path) {
    let Ok(legacy) = safe(root, &format!("{DIR}/{LEGACY_STEM}.json")) else {
        return;
    };
    let Ok(bytes) = fs::read(&legacy) else { return };
    let is_share = serde_json::from_slice::<Value>(&bytes).is_ok_and(|v| {
        v["version"] == 1 && v["key_hash"].is_string() && v.get("messages").is_none()
    });
    if !is_share {
        return;
    }
    match file(root) {
        Ok(target) if !target.exists() => {
            let _ = fs::rename(&legacy, &target);
        }
        Ok(_) => {
            let _ = fs::remove_file(&legacy);
        }
        Err(_) => (),
    }
}

fn read(root: &Path) -> Result<Option<Value>> {
    migrate_legacy(root);
    match fs::read(file(root)?) {
        Ok(bytes) if bytes.len() <= 4096 => {
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| err("Invalid share link file"))?;
            Ok((value["version"] == 1).then_some(value))
        }
        Ok(_) => Err(err("Invalid share link file")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(e)),
    }
}

fn status(value: Option<&Value>) -> Value {
    match value {
        Some(v) => {
            json!({"ok":true,"enabled":true,"access":v["access"],"created_at":v["created_at"]})
        }
        None => json!({"ok":true,"enabled":false}),
    }
}

/// UI actions; the caller holds the chat storage lock for mutations.
pub fn ui(root: &Path, args: &Value) -> Result<Value> {
    match args["action"].as_str().unwrap_or("") {
        "share_status" => Ok(status(read(root)?.as_ref())),
        "share_revoke" => {
            match fs::remove_file(file(root)?) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(io(e)),
            }
            Ok(status(None))
        }
        "share_create" => {
            let access = match args["access"].as_str().unwrap_or(ACCESS_READONLY) {
                ACCESS_READONLY => ACCESS_READONLY,
                ACCESS_FULL => ACCESS_FULL,
                _ => return Err(err("access must be readonly or full")),
            };
            let mut bytes = [0u8; 32];
            rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
            let key: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            let record =
                json!({"version":1,"key_hash":digest(&key),"access":access,"created_at":now()});
            fs::create_dir_all(safe(root, DIR)?).map_err(io)?;
            let target = file(root)?;
            let tmp = target.with_extension("json.tmp");
            fs::write(
                &tmp,
                serde_json::to_vec(&record).map_err(|_| err("Invalid share link"))?,
            )
            .map_err(io)?;
            fs::rename(&tmp, &target).map_err(io)?;
            let mut out = status(Some(&record));
            out["url_key"] = json!(key);
            Ok(out)
        }
        _ => Err(err("Unknown share action")),
    }
}

/// Returns the access mode for a valid key. Old links stop working after regenerate or revoke.
pub fn authenticate(root: &Path, key: &str) -> Result<String> {
    if !valid_key(key) {
        return Err(err("Share link invalid or revoked"));
    }
    let record = read(root)?.ok_or_else(|| err("Share link invalid or revoked"))?;
    let expected = record["key_hash"].as_str().unwrap_or("");
    let actual = digest(key);
    // Constant-time comparison of equal-length hex digests.
    let same = expected.len() == actual.len()
        && expected
            .bytes()
            .zip(actual.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
    if !same {
        return Err(err("Share link invalid or revoked"));
    }
    Ok(match record["access"].as_str() {
        Some(ACCESS_FULL) => ACCESS_FULL,
        _ => ACCESS_READONLY,
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_authenticate_regenerate_and_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(
            ui(root, &json!({"action":"share_status"})).unwrap()["enabled"],
            false
        );
        let first = ui(root, &json!({"action":"share_create","access":"full"})).unwrap();
        let key = first["url_key"].as_str().unwrap().to_owned();
        assert!(valid_key(&key));
        assert_eq!(authenticate(root, &key).unwrap(), "full");
        let stored = fs::read_to_string(file(root).unwrap()).unwrap();
        assert!(!stored.contains(&key), "only the hash is stored");
        let second = ui(root, &json!({"action":"share_create"})).unwrap();
        let key2 = second["url_key"].as_str().unwrap();
        assert!(
            authenticate(root, &key).is_err(),
            "regenerate revokes the old link"
        );
        assert_eq!(authenticate(root, key2).unwrap(), "readonly");
        ui(root, &json!({"action":"share_revoke"})).unwrap();
        assert!(authenticate(root, key2).is_err());
        assert!(authenticate(root, "zz").is_err());
        assert!(ui(root, &json!({"action":"share_create","access":"admin"})).is_err());
    }

    #[test]
    fn legacy_file_moves_out_of_the_chat_namespace_and_chats_list() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let key = ui(root, &json!({"action":"share_create","access":"full"})).unwrap()["url_key"]
            .as_str()
            .unwrap()
            .to_owned();
        let target = file(root).unwrap();
        let legacy = root.join(DIR).join(format!("{LEGACY_STEM}.json"));
        fs::rename(&target, &legacy).unwrap();
        // The chat list must not fail on the legacy file, and it moves it away.
        let listed = crate::tools::chat::ui(root, &json!({"action":"list"})).unwrap();
        assert_eq!(listed["sessions"], json!([]));
        assert!(!legacy.exists() && target.exists());
        assert_eq!(authenticate(root, &key).unwrap(), "full");
    }
}
