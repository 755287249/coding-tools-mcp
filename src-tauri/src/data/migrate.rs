#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::AppResult;
use crate::platform::platform;
use crate::settings::AppSettings;

use super::model::{AppData, LegacyProfilesOnlyFile};
use super::protection::{protect_secrets, unprotect_secrets};

const LEGACY_PROFILES_FILE: &str = "profiles.json";
const LEGACY_SETTINGS_FILE: &str = "app_settings.json";

pub fn data_file_path() -> AppResult<PathBuf> {
    Ok(platform()
        .app_config_dir()?
        .join("data")
        .join("profiles.json"))
}

pub fn load_or_migrate() -> AppResult<AppData> {
    let path = data_file_path()?;
    if path.exists() {
        return load_existing_with_recovery(&path);
    }

    let app_root = platform().app_config_dir()?;
    let mut data = AppData::default();

    let legacy_profiles = app_root.join(LEGACY_PROFILES_FILE);
    if legacy_profiles.exists() {
        let raw = fs::read_to_string(&legacy_profiles)?;
        if let Ok(file) = serde_json::from_str::<LegacyProfilesOnlyFile>(&raw) {
            data.profiles = file.profiles;
        }
    }

    let legacy_settings = app_root.join(LEGACY_SETTINGS_FILE);
    if legacy_settings.exists() {
        let raw = fs::read_to_string(&legacy_settings)?;
        if let Ok(settings) = serde_json::from_str::<AppSettings>(&raw) {
            merge_settings(&mut data, settings);
        }
    }

    Ok(data)
}

pub fn save(data: &AppData) -> AppResult<()> {
    let path = data_file_path()?;
    write_data(&path, data)
}

fn write_data(path: &Path, data: &AppData) -> AppResult<()> {
    write_data_with_backup(path, data, true)
}

fn write_data_with_backup(path: &Path, data: &AppData, preserve_existing: bool) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        restrict_data_directory(parent)?;
    }
    let protected = protect_secrets(data, path)?;
    let text = serde_json::to_string_pretty(&protected)?;
    let temp = sibling_path(path, &format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        restrict_data_file(&temp)?;

        if preserve_existing && path.exists() {
            let backup = backup_path(path);
            let backup_temp = sibling_path(path, &format!("bak-{}", uuid::Uuid::new_v4()));
            fs::copy(path, &backup_temp)?;
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&backup_temp)?
                .sync_all()?;
            restrict_data_file(&backup_temp)?;
            atomic_replace(&backup_temp, &backup)?;
        }
        atomic_replace(&temp, path)?;
        sync_parent(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn load_existing_with_recovery(path: &Path) -> AppResult<AppData> {
    match load_data_file(path) {
        Ok((data, plaintext_legacy)) => {
            if plaintext_legacy {
                // Never copy the legacy plaintext file into the recovery backup.
                write_data_with_backup(path, &data, false)?;
            } else if let Some(parent) = path.parent() {
                restrict_data_directory(parent)?;
                restrict_data_file(path)?;
                let backup = backup_path(path);
                if backup.exists() {
                    restrict_data_file(&backup)?;
                }
            }
            Ok(data)
        }
        Err(primary_error) => {
            let backup = backup_path(path);
            let (data, _) = load_data_file(&backup).map_err(|backup_error| {
                crate::error::AppError::Message(format!(
                    "設定檔與備份都無法讀取。主檔：{primary_error}；備份：{backup_error}"
                ))
            })?;
            let corrupt = sibling_path(
                path,
                &format!(
                    "corrupt-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                ),
            );
            fs::rename(path, &corrupt)?;
            restrict_data_file(&corrupt)?;
            write_data(path, &data)?;
            Ok(data)
        }
    }
}

fn load_data_file(path: &Path) -> AppResult<(AppData, bool)> {
    let raw = fs::read_to_string(path)?;
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    match serde_json::from_value::<AppData>(value) {
        Ok(mut data) => {
            let plaintext_legacy = unprotect_secrets(&mut data, path)?;
            Ok((data, plaintext_legacy))
        }
        Err(error) => {
            let backup = backup_path(path);
            if !backup.exists() {
                fs::copy(path, &backup)?;
            }
            Err(crate::error::AppError::Message(format!(
                "workspace profile migrate failed: {error}"
            )))
        }
    }
}

fn backup_path(path: &Path) -> PathBuf {
    sibling_path(path, "bak")
}

fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("profiles.json");
    path.with_file_name(format!("{file_name}.{suffix}"))
}

#[cfg(windows)]
pub(super) fn atomic_replace(source: &Path, destination: &Path) -> AppResult<()> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;

    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|error| {
            crate::error::AppError::Message(format!("atomic settings replace failed: {error}"))
        })
    }
}

#[cfg(unix)]
pub(super) fn atomic_replace(source: &Path, destination: &Path) -> AppResult<()> {
    fs::rename(source, destination)?;
    Ok(())
}

#[cfg(unix)]
pub(super) fn sync_parent(path: &Path) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn sync_parent(_path: &Path) -> AppResult<()> {
    Ok(())
}

#[cfg(windows)]
fn restrict_data_directory(path: &Path) -> AppResult<()> {
    run_icacls(path, true)
}

#[cfg(windows)]
fn restrict_data_file(path: &Path) -> AppResult<()> {
    run_icacls(path, false)
}

#[cfg(windows)]
fn run_icacls(path: &Path, directory: bool) -> AppResult<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let user = std::env::var("USERNAME").map_err(|_| {
        crate::error::AppError::Message("USERNAME is unavailable for ACL setup".into())
    })?;
    let reset = Command::new("icacls.exe")
        .arg(path)
        .arg("/reset")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if !reset.success() {
        return Err(crate::error::AppError::Message(format!(
            "failed to reset settings ACL: {reset}"
        )));
    }
    let inheritance = if directory { "(OI)(CI)(F)" } else { "(F)" };
    let status = Command::new("icacls.exe")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:{inheritance}"))
        .arg(format!("*S-1-5-18:{inheritance}"))
        .arg(format!("*S-1-5-32-544:{inheritance}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(crate::error::AppError::Message(format!(
            "failed to restrict settings ACL: {status}"
        )))
    }
}

#[cfg(unix)]
fn restrict_data_directory(path: &Path) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(unix)]
fn restrict_data_file(path: &Path) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Folder (next to `data/profiles.json`) that keeps a one-time copy of the
/// settings as they were right before the one-way global MCP connection
/// migration introduced in 0.1.142:
///
/// - `profiles.json`: the original desktop data file.
/// - `workspaces/<id>/workspace.json` and `secrets.json`: the original shared
///   workspace mirrors that the migration rewrites.
///
/// Restoring these lets an older desktop (0.1.141 and earlier) read its
/// original settings again. See `scripts/rollback-desktop.ps1`.
pub const PRE_GLOBAL_CONNECTION_BACKUP_DIR: &str = "pre-global-connection";

/// Copy the current settings once, before the global connection migration
/// rewrites them. `shared_mirrors` lists `(workspace id, files)` to keep.
/// Returns the backup folder when a new copy was written. An existing backup
/// is never overwritten, so it always holds the oldest pre-migration state.
pub fn backup_before_global_connection_migration(
    shared_mirrors: &[(String, Vec<PathBuf>)],
) -> AppResult<Option<PathBuf>> {
    backup_before_global_connection_migration_at(&data_file_path()?, shared_mirrors)
}

pub(super) fn backup_before_global_connection_migration_at(
    path: &Path,
    shared_mirrors: &[(String, Vec<PathBuf>)],
) -> AppResult<Option<PathBuf>> {
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    let backup = parent.join(PRE_GLOBAL_CONNECTION_BACKUP_DIR);
    if backup.exists() {
        return Ok(None);
    }
    let temp = parent.join(format!(
        "{PRE_GLOBAL_CONNECTION_BACKUP_DIR}.tmp-{}",
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> AppResult<()> {
        fs::create_dir_all(&temp)?;
        restrict_data_directory(&temp)?;
        let copy = |source: &Path, destination: &Path| -> AppResult<()> {
            fs::copy(source, destination)?;
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(destination)?
                .sync_all()?;
            restrict_data_file(destination)
        };
        copy(path, &temp.join("profiles.json"))?;
        for (id, files) in shared_mirrors {
            let safe_id = !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !safe_id {
                continue;
            }
            let target_dir = temp.join("workspaces").join(id);
            for file in files.iter().filter(|file| file.is_file()) {
                let Some(name) = file.file_name() else {
                    continue;
                };
                fs::create_dir_all(&target_dir)?;
                copy(file, &target_dir.join(name))?;
            }
        }
        fs::rename(&temp, &backup)?;
        sync_parent(&backup)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temp);
    }
    result.map(|()| Some(backup))
}

pub fn maybe_backup_legacy_files(path: &Path) -> AppResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let app_root = platform().app_config_dir()?;
    for name in [LEGACY_PROFILES_FILE, LEGACY_SETTINGS_FILE] {
        let legacy = app_root.join(name);
        if legacy.exists() {
            let backup = app_root.join(format!("{name}.bak"));
            if !backup.exists() {
                let _ = fs::rename(&legacy, &backup);
            }
        }
    }
    Ok(())
}

fn merge_settings(data: &mut AppData, settings: AppSettings) {
    data.frp_profiles = settings.frp_profiles;
    data.last_workspace_id = settings.last_workspace_id;
    data.download = settings.download;
    data.proxy = settings.proxy;
    data.shared_secrets = settings.shared_secrets;
    data.workspace_secrets = settings.workspace_secrets;
    data.app_secrets = settings.app_secrets;
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    use crate::workspace::WorkspaceProfile;

    fn data_with_secret(secret: &str) -> AppData {
        let mut data = AppData::default();
        data.shared_secrets
            .insert("bearer_token".into(), secret.into());
        data
    }

    #[test]
    fn legacy_plaintext_is_migrated_to_an_encrypted_atomic_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("profiles.json");
        let data = data_with_secret("legacy-plaintext-secret");
        fs::write(&path, serde_json::to_vec_pretty(&data).expect("json")).expect("legacy write");

        let loaded = load_existing_with_recovery(&path).expect("load and migrate");

        assert_eq!(
            loaded.shared_secrets["bearer_token"],
            "legacy-plaintext-secret"
        );
        let disk = fs::read_to_string(&path).expect("disk data");
        assert!(!disk.contains("legacy-plaintext-secret"));
        assert!(!backup_path(&path).exists());
    }

    #[test]
    fn atomic_write_keeps_the_previous_valid_backup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("profiles.json");
        let first = data_with_secret("first-secret");
        let second = data_with_secret("second-secret");

        write_data(&path, &first).expect("first write");
        write_data(&path, &second).expect("second write");

        let (current, _) = load_data_file(&path).expect("current");
        let (backup, _) = load_data_file(&backup_path(&path)).expect("backup");
        assert_eq!(current.shared_secrets["bearer_token"], "second-secret");
        assert_eq!(backup.shared_secrets["bearer_token"], "first-secret");
    }

    #[test]
    fn corrupt_primary_is_archived_and_restored_from_backup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("profiles.json");
        let first = data_with_secret("recover-me");
        let second = data_with_secret("newer-value");
        write_data(&path, &first).expect("first write");
        write_data(&path, &second).expect("second write");
        fs::write(&path, b"{truncated").expect("corrupt primary");

        let recovered = load_existing_with_recovery(&path).expect("recover backup");

        assert_eq!(recovered.shared_secrets["bearer_token"], "recover-me");
        let (restored, _) = load_data_file(&path).expect("restored primary");
        assert_eq!(restored.shared_secrets["bearer_token"], "recover-me");
        assert!(fs::read_dir(temp.path())
            .expect("dir")
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains(".corrupt-")));
    }

    #[test]
    fn save_writes_canonical_profiles_and_keeps_envelope_secrets() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("profiles.json");
        let profile = WorkspaceProfile::new("C:/workspace/repo".into(), Some("repo".into()));
        let id = profile.id.clone();
        let envelope = json!({
            "frp_profiles": [{
                "id": "frp-1",
                "name": "edge",
                "server": "frp.example",
                "server_port": 7000
            }],
            "last_workspace_id": id,
            "download": { "githubMirror": "", "proxyMode": "none", "proxyUrl": "" },
            "proxy": { "mode": "none", "url": "" },
            "shared_secrets": { "oauth_password": "shared-oauth-password" },
            "workspace_secrets": { (id.clone()): { "oauth_password": "envelope-oauth-password" } },
            "app_secrets": {},
            "mcp_enabled_workspace_ids": [id],
            "actions_enabled_workspace_ids": [],
            "profiles": [serde_json::to_value(&profile).expect("snake profile")]
        });
        fs::write(&path, serde_json::to_vec_pretty(&envelope).expect("write")).expect("seed");

        let loaded = load_existing_with_recovery(&path).expect("load snake_case envelope");
        assert_eq!(loaded.profiles[0].name, "repo");
        write_data(&path, &loaded).expect("save canonical");

        let disk: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("disk")).expect("json");
        let stored = &disk["profiles"][0];
        assert_eq!(stored["schemaVersion"], 2);
        assert!(stored.get("bind").is_some());
        assert!(stored["host"].get("desktop").is_some());
        assert!(stored.get("runtime").is_none());
        assert_eq!(disk["last_workspace_id"], id);
        assert_eq!(disk["frp_profiles"][0]["id"], "frp-1");
        assert_eq!(disk["mcp_enabled_workspace_ids"][0], id);
        assert!(disk["workspace_secrets"][id.as_str()].is_object());
        assert!(disk["shared_secrets"].is_object());
        let profile_text = stored.to_string();
        assert!(!profile_text.contains("envelope-oauth-password"));
        assert!(!profile_text.contains("shared-oauth-password"));
        assert!(!profile_text.contains("oauth_password"));
        if let Ok(dump) = std::env::var("CTMCP_PHASE4_DUMP") {
            fs::write(
                format!("{dump}/phase4-profiles.json"),
                serde_json::to_vec_pretty(&disk).expect("dump"),
            )
            .expect("dump after");
        }
    }

    #[test]
    fn invalid_profile_migrate_leaves_original_and_writes_bak() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("profiles.json");
        let original =
            br#"{"last_workspace_id":"keep-me","profiles":[{"schemaVersion":2,"id":"broken"}]}"#;
        fs::write(&path, original).expect("seed invalid");
        let before = fs::read(&path).expect("before");

        let error = load_data_file(&path).expect_err("migrate should fail");
        assert!(error.to_string().contains("migrate failed"));
        assert_eq!(fs::read(&path).expect("after"), before);
        assert!(backup_path(&path).exists());
        if let Ok(dump) = std::env::var("CTMCP_PHASE4_DUMP") {
            let listing = format!(
                "original_unchanged={}\nbak_exists={}\nbak_path={}\nerror={}\n",
                fs::read(&path).expect("after") == before,
                backup_path(&path).exists(),
                backup_path(&path).display(),
                error
            );
            fs::write(format!("{dump}/phase4-bak.txt"), listing).expect("dump bak");
        }
    }
}

#[cfg(test)]
mod pre_global_connection_backup_tests {
    use super::*;

    #[test]
    fn backup_keeps_original_settings_once_and_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        fs::create_dir_all(&data_dir).unwrap();
        let path = data_dir.join("profiles.json");
        assert!(backup_before_global_connection_migration_at(&path, &[])
            .unwrap()
            .is_none());

        let shared = temp.path().join("shared").join("abc123");
        fs::create_dir_all(&shared).unwrap();
        fs::write(shared.join("workspace.json"), b"original-workspace").unwrap();
        fs::write(shared.join("secrets.json"), b"original-secrets").unwrap();
        let mirrors = vec![
            (
                "abc123".to_string(),
                vec![
                    shared.join("workspace.json"),
                    shared.join("secrets.json"),
                    shared.join("missing.json"),
                ],
            ),
            ("../escape".to_string(), vec![shared.join("workspace.json")]),
        ];

        fs::write(&path, b"old-profiles").unwrap();
        let backup = backup_before_global_connection_migration_at(&path, &mirrors)
            .unwrap()
            .expect("first call writes a backup");
        assert_eq!(backup, data_dir.join(PRE_GLOBAL_CONNECTION_BACKUP_DIR));
        assert_eq!(
            fs::read(backup.join("profiles.json")).unwrap(),
            b"old-profiles"
        );
        let kept = backup.join("workspaces").join("abc123");
        assert_eq!(
            fs::read(kept.join("workspace.json")).unwrap(),
            b"original-workspace"
        );
        assert_eq!(
            fs::read(kept.join("secrets.json")).unwrap(),
            b"original-secrets"
        );
        assert!(!kept.join("missing.json").exists());
        assert!(!temp.path().join("escape").exists());
        assert!(!backup.join("escape").exists());

        fs::write(&path, b"migrated-profiles").unwrap();
        fs::write(shared.join("workspace.json"), b"migrated-workspace").unwrap();
        assert!(
            backup_before_global_connection_migration_at(&path, &mirrors)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fs::read(backup.join("profiles.json")).unwrap(),
            b"old-profiles"
        );
        assert_eq!(
            fs::read(kept.join("workspace.json")).unwrap(),
            b"original-workspace"
        );
        let leftovers = fs::read_dir(&data_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(leftovers, 0);
    }
}

#[cfg(test)]
mod browser_sharing_persistence_tests {
    use super::*;
    #[test]
    fn browser_sharing_config_uses_encrypted_persistence_and_survives_reload() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("profiles.json");
        let settings=serde_json::json!({"enabled":true,"origins":["https://synthetic.example"],"password":"synthetic-persistent-password"}).to_string();
        let mut data = AppData::default();
        data.app_secrets
            .entry("browser-sharing".into())
            .or_default()
            .insert("settings".into(), settings.clone());
        write_data(&path, &data).unwrap();
        let disk = fs::read_to_string(&path).unwrap();
        assert!(!disk.contains("synthetic-persistent-password"));
        let restored = load_existing_with_recovery(&path).unwrap();
        assert_eq!(
            restored.app_secrets["browser-sharing"]["settings"],
            settings
        );
        write_data(&path, &restored).unwrap();
        assert_eq!(
            load_existing_with_recovery(&path).unwrap().app_secrets["browser-sharing"]["settings"],
            settings
        );
    }
}
