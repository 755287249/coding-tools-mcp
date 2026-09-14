use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::Deserialize;

use super::protection::{read_maybe_wrapped_json, write_wrapped_json};
use crate::error::{AppError, AppResult};
use crate::settings::AppSettings;
use crate::workspace::canonical::{
    canonical_from_desktop_profile, desktop_profile_from_canonical,
    merge_desktop_profile_on_canonical, parse_canonical_workspace, roundtrip_desktop_profile,
    serialize_canonical_workspace, CanonicalWorkspace,
};
use crate::workspace::legacy_import::import_legacy_profiles_if_empty;
use crate::workspace::pack::{
    build_workspace_pack, profile_from_workspace_pack, secret_presence_from_map,
    shared_secrets_file_next_to, shared_workspace_file, shared_workspaces_root,
};
use crate::workspace::resources::assign_free_workspace_ports;
use crate::workspace::WorkspaceProfile;

use super::migrate::{data_file_path, load_or_migrate, maybe_backup_legacy_files, save};
use super::model::AppData;

static DATA_FILE_LOCK: Mutex<()> = Mutex::new(());

const SHARED_KEYS: &[&str] = &[
    "oauth_client_id",
    "bearer_token",
    "oauth_client_secret",
    "oauth_password",
    "oauth_token_secret",
    "actions_api_key",
    "actions_oauth_client_secret",
    "actions_oauth_password",
    "actions_oauth_token_secret",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeHandoffState {
    #[serde(default)]
    mcp_workspace_ids: Vec<String>,
    #[serde(default)]
    actions_workspace_ids: Vec<String>,
}

#[derive(Debug)]
pub struct DataStore {
    data: AppData,
}

impl DataStore {
    pub fn load() -> AppResult<Self> {
        let _guard = lock_data_file()?;
        let path = data_file_path()?;
        let existed_before = path.exists();
        let mut data = load_or_migrate()?;
        let imported = import_legacy_profiles_if_empty(&mut data)?;
        let normalized = data.profiles.iter_mut().fold(false, |changed, profile| {
            let folders_changed = profile.normalize_folders();
            let bindings_changed = profile.normalize_bind_addresses();
            let concurrency_changed = profile.normalize_runtime_concurrency_defaults();
            let sandbox_changed = profile.normalize_sandbox();
            let before = serde_json::to_value(&*profile).ok();
            *profile = roundtrip_desktop_profile(profile.clone());
            let canonical_changed = before != serde_json::to_value(&*profile).ok();
            folders_changed
                || bindings_changed
                || concurrency_changed
                || sandbox_changed
                || canonical_changed
                || changed
        });
        let shared_changed = sync_shared_live_store(&mut data)?;
        let store = Self { data };
        if !existed_before || imported > 0 || normalized || shared_changed {
            store.persist_unlocked()?;
        }
        if !existed_before {
            maybe_backup_legacy_files(&path)?;
        }
        Ok(store)
    }

    pub fn read_file<R>(f: impl FnOnce(&AppData) -> AppResult<R>) -> AppResult<R> {
        let _guard = lock_data_file()?;
        let data = load_or_migrate()?;
        f(&data)
    }

    pub fn update_file<R>(f: impl FnOnce(&mut AppData) -> AppResult<R>) -> AppResult<R> {
        let _guard = lock_data_file()?;
        let mut data = load_or_migrate()?;
        let result = f(&mut data)?;
        save(&data)?;
        Ok(result)
    }

    pub fn read_workspace_secret_latest(profile_id: &str, key: &str) -> AppResult<Option<String>> {
        let _guard = lock_data_file()?;
        let data = load_or_migrate()?;
        workspace_secret_value(&data, profile_id, key)
    }

    pub fn write_workspace_secret_latest(
        profile_id: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()> {
        let _guard = lock_data_file()?;
        let mut data = load_or_migrate()?;
        update_workspace_secret_value(&mut data, profile_id, key, value)?;
        save(&data)
    }

    pub fn data(&self) -> &AppData {
        &self.data
    }

    pub fn save(&self) -> AppResult<()> {
        let _guard = lock_data_file()?;
        self.persist_unlocked()
    }

    fn persist_unlocked(&self) -> AppResult<()> {
        save(&self.data)
    }

    pub fn settings(&self) -> AppSettings {
        AppSettings::from_data(&self.data)
    }

    pub fn update_settings(&mut self, settings: AppSettings) -> AppResult<()> {
        settings.apply_to(&mut self.data);
        self.save()
    }

    pub fn list(&self) -> &[WorkspaceProfile] {
        &self.data.profiles
    }

    pub fn get(&self, id: &str) -> Option<&WorkspaceProfile> {
        self.data.profiles.iter().find(|profile| profile.id == id)
    }

    pub fn feature_document(&self, id: &str) -> AppResult<CanonicalWorkspace> {
        let profile = self
            .get(id)
            .ok_or_else(|| AppError::Message(format!("workspace not found: {id}")))?;
        let path = shared_workspace_file(id);
        if path.is_file() {
            read_shared_canonical(&path)
        } else {
            Ok(canonical_from_desktop_profile(profile))
        }
    }

    pub fn update_feature_document<R>(
        &mut self,
        id: &str,
        update: impl FnOnce(&mut CanonicalWorkspace) -> AppResult<R>,
    ) -> AppResult<R> {
        let mut document = self.feature_document(id)?;
        let result = update(&mut document)?;
        let path = shared_workspace_file(id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let value = serialize_canonical_workspace(&document).map_err(AppError::Message)?;
        write_wrapped_json(&path, &value)?;
        Ok(result)
    }

    pub fn mcp_runtime_enabled(&self, id: &str) -> bool {
        self.data
            .mcp_enabled_workspace_ids
            .iter()
            .any(|workspace_id| workspace_id == id)
    }

    /// Return enabled MCP workspaces in profile order. Stale ids are ignored,
    /// which also makes upgrades/recovery tolerant of partially removed data.
    pub fn mcp_auto_start_workspace_ids(&self) -> Vec<String> {
        self.data
            .profiles
            .iter()
            .filter(|profile| self.mcp_runtime_enabled(&profile.id))
            .map(|profile| profile.id.clone())
            .collect()
    }

    pub fn actions_runtime_enabled(&self, id: &str) -> bool {
        self.data
            .actions_enabled_workspace_ids
            .iter()
            .any(|workspace_id| workspace_id == id)
    }

    pub fn actions_auto_start_workspace_ids(&self) -> Vec<String> {
        self.data
            .profiles
            .iter()
            .filter(|profile| self.actions_runtime_enabled(&profile.id))
            .map(|profile| profile.id.clone())
            .collect()
    }

    pub fn set_mcp_runtime_enabled(&mut self, id: &str, enabled: bool) -> AppResult<()> {
        if self.get(id).is_none() {
            return Err(AppError::Message(format!("workspace not found: {id}")));
        }

        let already_enabled = self.mcp_runtime_enabled(id);
        if already_enabled == enabled {
            return Ok(());
        }

        if enabled {
            self.data.mcp_enabled_workspace_ids.push(id.to_string());
        } else {
            self.data
                .mcp_enabled_workspace_ids
                .retain(|workspace_id| workspace_id != id);
        }
        self.save()
    }

    pub fn set_actions_runtime_enabled(&mut self, id: &str, enabled: bool) -> AppResult<()> {
        if self.get(id).is_none() {
            return Err(AppError::Message(format!("workspace not found: {id}")));
        }

        let already_enabled = self.actions_runtime_enabled(id);
        if already_enabled == enabled {
            return Ok(());
        }

        if enabled {
            self.data.actions_enabled_workspace_ids.push(id.to_string());
        } else {
            self.data
                .actions_enabled_workspace_ids
                .retain(|workspace_id| workspace_id != id);
        }
        self.save()
    }

    /// Replace the durable desired runtime state with a version-handoff
    /// snapshot. Stale workspace ids are ignored.
    pub fn apply_runtime_handoff_state(
        &mut self,
        mcp_ids: &[String],
        actions_ids: &[String],
    ) -> AppResult<()> {
        self.data.mcp_enabled_workspace_ids = self
            .data
            .profiles
            .iter()
            .filter(|profile| mcp_ids.iter().any(|id| id == &profile.id))
            .map(|profile| profile.id.clone())
            .collect();
        self.data.actions_enabled_workspace_ids = self
            .data
            .profiles
            .iter()
            .filter(|profile| actions_ids.iter().any(|id| id == &profile.id))
            .map(|profile| profile.id.clone())
            .collect();
        self.save()
    }

    /// Consume the one-shot runtime snapshot written by the detached update
    /// handoff worker. The snapshot contains only workspace ids and is removed
    /// after the durable state has been saved successfully.
    pub fn consume_runtime_handoff_state(&mut self) -> AppResult<bool> {
        let path = data_file_path()?.with_file_name("runtime-handoff.json");
        if !path.is_file() {
            return Ok(false);
        }

        let raw = fs::read_to_string(&path)?;
        let state: RuntimeHandoffState = serde_json::from_str(&raw)?;
        self.apply_runtime_handoff_state(&state.mcp_workspace_ids, &state.actions_workspace_ids)?;
        fs::remove_file(path)?;
        Ok(true)
    }

    pub fn add(&mut self, mut profile: WorkspaceProfile) -> AppResult<()> {
        profile.normalize_folders();
        profile.normalize_bind_addresses();
        profile.normalize_sandbox();
        profile
            .validate_bind_addresses()
            .map_err(AppError::Message)?;
        self.data.profiles.push(roundtrip_desktop_profile(profile));
        if let Some(saved) = self.data.profiles.last() {
            persist_shared_profile(saved)?;
        }
        self.save()
    }

    pub fn update(&mut self, mut profile: WorkspaceProfile) -> AppResult<()> {
        profile.normalize_folders();
        profile.normalize_bind_addresses();
        profile.normalize_sandbox();
        profile
            .validate_bind_addresses()
            .map_err(AppError::Message)?;
        profile = roundtrip_desktop_profile(profile);
        let Some(index) = self
            .data
            .profiles
            .iter()
            .position(|item| item.id == profile.id)
        else {
            return Err(AppError::Message(format!(
                "workspace not found: {}",
                profile.id
            )));
        };
        self.data.profiles[index] = profile;
        persist_shared_profile(&self.data.profiles[index])?;
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> AppResult<Option<WorkspaceProfile>> {
        let Some(index) = self.data.profiles.iter().position(|item| item.id == id) else {
            return Ok(None);
        };
        let removed = self.data.profiles.remove(index);
        self.data.workspace_secrets.remove(id);
        self.data
            .mcp_enabled_workspace_ids
            .retain(|workspace_id| workspace_id != id);
        self.data
            .actions_enabled_workspace_ids
            .retain(|workspace_id| workspace_id != id);
        self.save()?;
        Ok(Some(removed))
    }

    pub fn export_workspace_pack(&self, id: &str) -> AppResult<serde_json::Value> {
        export_workspace_pack_from_data(&self.data, id)
    }

    pub fn import_workspace_pack(
        &mut self,
        pack: serde_json::Value,
    ) -> AppResult<WorkspaceProfile> {
        let profile = import_workspace_pack_into_data(&mut self.data, pack)?;
        self.save()?;
        Ok(profile)
    }

    pub fn export_shared_workspace(&self, id: &str) -> AppResult<std::path::PathBuf> {
        let path = shared_workspace_file(id);
        export_shared_workspace_from_data(&self.data, id, &path)?;
        Ok(path)
    }

    pub fn open_shared_workspace(&mut self, id: &str) -> AppResult<WorkspaceProfile> {
        self.open_shared_workspace_file(&shared_workspace_file(id))
    }

    pub fn open_shared_workspace_file(
        &mut self,
        path: &std::path::Path,
    ) -> AppResult<WorkspaceProfile> {
        let profile = open_shared_workspace_file_into_data(&mut self.data, path)?;
        self.save()?;
        Ok(profile)
    }

    pub fn init_workspace_secrets(&mut self, profile_id: &str) -> AppResult<()> {
        // oauth_client_secret is optional for MCP OAuth (ChatGPT PKCE); not auto-generated.
        self.set_workspace_secret(profile_id, "oauth_password", &random_secret())?;
        self.set_workspace_secret(profile_id, "oauth_token_secret", &random_secret())?;
        self.set_workspace_secret(profile_id, "bearer_token", &random_secret())?;
        // The built-in tunnel token must match the self-hosted server, so it is
        // intentionally not generated automatically.
        self.set_workspace_secret(profile_id, "actions_api_key", &random_secret())?;
        self.set_workspace_secret(profile_id, "actions_oauth_client_secret", &random_secret())?;
        self.set_workspace_secret(profile_id, "actions_oauth_password", &random_secret())?;
        self.set_workspace_secret(profile_id, "actions_oauth_token_secret", &random_secret())?;
        Ok(())
    }

    pub fn init_shared_secrets(&mut self) -> AppResult<()> {
        let mut changed = false;
        for key in SHARED_KEYS {
            if !self.data.shared_secrets.contains_key(*key) {
                self.data
                    .shared_secrets
                    .insert(key.to_string(), shared_value_for_key(key));
                changed = true;
            }
        }
        if changed {
            self.save()?;
        }
        Ok(())
    }

    pub fn get_workspace_secret(&self, profile_id: &str, key: &str) -> AppResult<Option<String>> {
        workspace_secret_value(&self.data, profile_id, key)
    }

    pub fn set_workspace_secret(
        &mut self,
        profile_id: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()> {
        update_workspace_secret_value(&mut self.data, profile_id, key, value)?;
        self.save()
    }

    pub fn regenerate_workspace_secret(
        &mut self,
        profile_id: &str,
        key: &str,
    ) -> AppResult<String> {
        let value = shared_value_for_key(key);
        self.set_workspace_secret(profile_id, key, &value)?;
        Ok(value)
    }

    pub fn remove_workspace_secrets(&mut self, profile_id: &str) -> AppResult<()> {
        self.data.workspace_secrets.remove(profile_id);
        if shared_live_store_enabled() {
            write_shared_secret_values(profile_id, &std::collections::HashMap::new())?;
        }
        self.save()
    }

    pub fn get_shared_secret(&self, key: &str) -> Option<String> {
        self.data.shared_secrets.get(key).cloned()
    }

    pub fn set_shared_secret(&mut self, key: &str, value: &str) -> AppResult<()> {
        self.data
            .shared_secrets
            .insert(key.to_string(), value.to_string());
        self.save()
    }

    pub fn regenerate_shared_secret(&mut self, key: &str) -> AppResult<String> {
        let value = random_secret();
        self.set_shared_secret(key, &value)?;
        Ok(value)
    }

    pub fn get_app_secret(&self, scope: &str, item_id: &str) -> Option<String> {
        self.data
            .app_secrets
            .get(scope)
            .and_then(|items| items.get(item_id))
            .filter(|value| !value.is_empty())
            .cloned()
    }

    pub fn set_app_secret(&mut self, scope: &str, item_id: &str, value: &str) -> AppResult<()> {
        self.data
            .app_secrets
            .entry(scope.to_string())
            .or_default()
            .insert(item_id.to_string(), value.to_string());
        self.save()
    }

    pub fn delete_app_secret(&mut self, scope: &str, item_id: &str) -> AppResult<()> {
        if let Some(items) = self.data.app_secrets.get_mut(scope) {
            items.remove(item_id);
            if items.is_empty() {
                self.data.app_secrets.remove(scope);
            }
        }
        self.save()
    }
}

fn workspace_secret_value(
    data: &AppData,
    profile_id: &str,
    key: &str,
) -> AppResult<Option<String>> {
    let shared_root = shared_live_store_enabled().then(shared_workspaces_root);
    workspace_secret_value_at(data, profile_id, key, shared_root.as_deref())
}

fn workspace_secret_value_at(
    data: &AppData,
    profile_id: &str,
    key: &str,
    shared_root: Option<&Path>,
) -> AppResult<Option<String>> {
    if let Some(root) = shared_root {
        let path = shared_secrets_file_next_to(&shared_workspace_file_in(root, profile_id));
        if path.is_file() {
            return Ok(shared_secret_values(&read_maybe_wrapped_json(&path)?)
                .get(key)
                .filter(|value| !value.is_empty())
                .cloned());
        }
    }
    Ok(data
        .workspace_secrets
        .get(profile_id)
        .and_then(|secrets| secrets.get(key))
        .filter(|value| !value.is_empty())
        .cloned())
}

fn update_workspace_secret_value(
    data: &mut AppData,
    profile_id: &str,
    key: &str,
    value: &str,
) -> AppResult<()> {
    let shared_root = shared_live_store_enabled().then(shared_workspaces_root);
    update_workspace_secret_value_at(data, profile_id, key, value, shared_root.as_deref())
}

fn update_workspace_secret_value_at(
    data: &mut AppData,
    profile_id: &str,
    key: &str,
    value: &str,
    shared_root: Option<&Path>,
) -> AppResult<()> {
    let shared_path = shared_root
        .map(|root| shared_secrets_file_next_to(&shared_workspace_file_in(root, profile_id)));
    let mut latest = if let Some(path) = shared_path.as_ref().filter(|path| path.is_file()) {
        shared_secret_values(&read_maybe_wrapped_json(path)?)
    } else {
        data.workspace_secrets
            .get(profile_id)
            .cloned()
            .unwrap_or_default()
    };
    latest.insert(key.to_string(), value.to_string());
    data.workspace_secrets
        .insert(profile_id.to_string(), latest.clone());
    if let Some(path) = shared_path {
        write_shared_secret_values_at(&path, &latest)?;
    }
    Ok(())
}

fn shared_live_store_enabled() -> bool {
    if std::env::var("CTMCP_SHARED_STORE_DISABLED").ok().as_deref() == Some("1") {
        return false;
    }
    #[cfg(test)]
    {
        return std::env::var_os("CTMCP_SHARED_WORKSPACES_ROOT").is_some();
    }
    #[cfg(not(test))]
    {
        true
    }
}

fn shared_workspace_file_in(root: &Path, id: &str) -> PathBuf {
    root.join(id).join("workspace.json")
}

fn normalize_folder_path(value: &str) -> String {
    let path = PathBuf::from(value.trim());
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::new())
            .join(path)
    };
    let mut clean = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = clean.pop();
            }
            _ => clean.push(component.as_os_str()),
        }
    }
    let mut normalized = clean.to_string_lossy().replace('\\', "/");
    if normalized.ends_with('/') {
        normalized.pop();
    }
    #[cfg(windows)]
    {
        normalized = normalized.to_lowercase();
    }
    normalized
}

fn canonical_folder_identity(canonical: &CanonicalWorkspace) -> Vec<String> {
    let mut folders = canonical
        .folders
        .iter()
        .map(|folder| normalize_folder_path(&folder.path))
        .collect::<Vec<_>>();
    folders.sort();
    folders
}

fn profile_folder_identity(profile: &WorkspaceProfile) -> Vec<String> {
    canonical_folder_identity(&canonical_from_desktop_profile(profile))
}

fn canonical_has_node_state(canonical: &CanonicalWorkspace) -> bool {
    canonical
        .host
        .node
        .as_object()
        .map(|value| !value.is_empty())
        .unwrap_or(false)
}

fn read_shared_canonical(path: &Path) -> AppResult<CanonicalWorkspace> {
    parse_canonical_workspace(&read_maybe_wrapped_json(path)?).map_err(AppError::Message)
}

fn find_node_counterpart_shared_workspace_id_at(
    root: &Path,
    profile: &WorkspaceProfile,
    current_id: &str,
) -> AppResult<Option<String>> {
    let identity = profile_folder_identity(profile);
    let current_path = shared_workspace_file_in(root, current_id);
    if current_path.is_file() {
        let current = read_shared_canonical(&current_path)?;
        if canonical_has_node_state(&current) && canonical_folder_identity(&current) == identity {
            return Ok(Some(current_id.to_string()));
        }
    }

    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut matches = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if id == current_id {
            continue;
        }
        let workspace_path = entry.path().join("workspace.json");
        if !workspace_path.is_file() {
            continue;
        }
        let candidate = read_shared_canonical(&workspace_path)?;
        if canonical_has_node_state(&candidate) && canonical_folder_identity(&candidate) == identity
        {
            matches.push(id);
        }
    }
    Ok((matches.len() == 1).then(|| matches.remove(0)))
}

fn replace_workspace_id(values: &mut Vec<String>, old_id: &str, new_id: &str) -> bool {
    let mut next = Vec::with_capacity(values.len());
    for value in values.iter() {
        let value = if value == old_id { new_id } else { value };
        if !next.iter().any(|item| item == value) {
            next.push(value.to_string());
        }
    }
    if *values == next {
        false
    } else {
        *values = next;
        true
    }
}

fn merge_desktop_host_state_on_existing(
    existing: &CanonicalWorkspace,
    profile: &WorkspaceProfile,
    target_id: &str,
) -> CanonicalWorkspace {
    let desktop_merged = merge_desktop_profile_on_canonical(Some(existing), profile);
    let mut next = existing.clone();
    next.id = target_id.to_string();
    next.host.desktop = desktop_merged.host.desktop;

    for folder in &mut next.folders {
        let identity = normalize_folder_path(&folder.path);
        if let Some(source) = desktop_merged
            .folders
            .iter()
            .find(|candidate| normalize_folder_path(&candidate.path) == identity)
        {
            if let Some(execution) = source.extra.get("execution") {
                folder.extra.insert("execution".into(), execution.clone());
            } else {
                folder.extra.remove("execution");
            }
        }
    }
    next
}

fn write_shared_secret_values_at(
    path: &Path,
    secrets: &std::collections::HashMap<String, String>,
) -> AppResult<()> {
    let mut values = serde_json::Map::new();
    for (key, value) in secrets {
        if !value.is_empty() {
            values.insert(key.clone(), serde_json::Value::String(value.clone()));
        }
    }
    write_wrapped_json(
        path,
        &serde_json::json!({
            "schemaVersion": 1,
            "values": values,
        }),
    )
}

fn migrate_profile_to_node_counterpart_at(
    data: &mut AppData,
    index: usize,
    root: &Path,
    target_id: &str,
) -> AppResult<bool> {
    let old_id = data.profiles[index].id.clone();
    if old_id == target_id
        || data
            .profiles
            .iter()
            .enumerate()
            .any(|(other, profile)| other != index && profile.id == target_id)
    {
        return Ok(false);
    }

    let target_workspace_path = shared_workspace_file_in(root, target_id);
    let target_canonical = read_shared_canonical(&target_workspace_path)?;
    let mut desktop_source = data.profiles[index].clone();
    let old_workspace_path = shared_workspace_file_in(root, &old_id);
    if old_workspace_path.is_file() {
        let old_canonical = read_shared_canonical(&old_workspace_path)?;
        if canonical_folder_identity(&old_canonical) == profile_folder_identity(&desktop_source) {
            desktop_source = desktop_profile_from_canonical(&old_canonical);
        }
    }
    desktop_source.id = target_id.to_string();
    let merged_canonical =
        merge_desktop_host_state_on_existing(&target_canonical, &desktop_source, target_id);
    let serialized = serialize_canonical_workspace(&merged_canonical).map_err(AppError::Message)?;
    write_wrapped_json(&target_workspace_path, &serialized)?;
    data.profiles[index] = desktop_profile_from_canonical(&merged_canonical);

    let mut merged_secrets = data.workspace_secrets.remove(&old_id).unwrap_or_default();
    let old_secrets_path = shared_secrets_file_next_to(&old_workspace_path);
    if old_secrets_path.is_file() {
        for (key, value) in shared_secret_values(&read_maybe_wrapped_json(&old_secrets_path)?) {
            merged_secrets.insert(key, value);
        }
    }
    if let Some(target_local) = data.workspace_secrets.remove(target_id) {
        for (key, value) in target_local {
            merged_secrets.entry(key).or_insert(value);
        }
    }
    let target_secrets_path = shared_secrets_file_next_to(&target_workspace_path);
    if target_secrets_path.is_file() {
        for (key, value) in shared_secret_values(&read_maybe_wrapped_json(&target_secrets_path)?) {
            merged_secrets.insert(key, value);
        }
    }
    if !merged_secrets.is_empty() || target_secrets_path.is_file() {
        write_shared_secret_values_at(&target_secrets_path, &merged_secrets)?;
        data.workspace_secrets
            .insert(target_id.to_string(), merged_secrets);
    }

    if data.last_workspace_id == old_id {
        data.last_workspace_id = target_id.to_string();
    }
    replace_workspace_id(&mut data.mcp_enabled_workspace_ids, &old_id, target_id);
    replace_workspace_id(&mut data.actions_enabled_workspace_ids, &old_id, target_id);
    Ok(true)
}

fn shared_secret_values(value: &serde_json::Value) -> std::collections::HashMap<String, String> {
    let mut values = std::collections::HashMap::new();
    if let Some(object) = value.get("values").and_then(serde_json::Value::as_object) {
        for (key, value) in object {
            if let Some(text) = value.as_str().filter(|text| !text.is_empty()) {
                values.insert(key.clone(), text.to_string());
            }
        }
    }
    for (json_key, store_key) in [
        ("oauthPassword", "oauth_password"),
        ("oauthClientSecret", "oauth_client_secret"),
        ("oauthTokenSecret", "oauth_token_secret"),
        ("tunnelEnrollmentUrl", "builtin_tunnel_enrollment_url"),
    ] {
        if values.contains_key(store_key) {
            continue;
        }
        if let Some(text) = value
            .get(json_key)
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
        {
            values.insert(store_key.to_string(), text.to_string());
        }
    }
    values
}

fn write_shared_secret_values(
    id: &str,
    secrets: &std::collections::HashMap<String, String>,
) -> AppResult<()> {
    if !shared_live_store_enabled() {
        return Ok(());
    }
    let path = shared_secrets_file_next_to(&shared_workspace_file(id));
    write_shared_secret_values_at(&path, secrets)
}
fn persist_shared_profile(profile: &WorkspaceProfile) -> AppResult<()> {
    if !shared_live_store_enabled() {
        return Ok(());
    }
    persist_shared_profile_at(&shared_workspace_file(&profile.id), profile)
}

fn persist_shared_profile_at(path: &Path, profile: &WorkspaceProfile) -> AppResult<()> {
    let existing = if path.is_file() {
        Some(
            parse_canonical_workspace(&read_maybe_wrapped_json(path)?)
                .map_err(AppError::Message)?,
        )
    } else {
        None
    };
    let canonical = merge_desktop_profile_on_canonical(existing.as_ref(), profile);
    let value = serialize_canonical_workspace(&canonical).map_err(AppError::Message)?;
    write_wrapped_json(path, &value)
}

fn sync_shared_live_store(data: &mut AppData) -> AppResult<bool> {
    if !shared_live_store_enabled() {
        return Ok(false);
    }
    sync_shared_live_store_at(data, &shared_workspaces_root())
}

fn sync_shared_live_store_at(data: &mut AppData, root: &Path) -> AppResult<bool> {
    let mut changed = false;
    for index in 0..data.profiles.len() {
        let current_id = data.profiles[index].id.clone();
        if let Some(target_id) =
            find_node_counterpart_shared_workspace_id_at(root, &data.profiles[index], &current_id)?
        {
            changed |= migrate_profile_to_node_counterpart_at(data, index, root, &target_id)?;
        }

        let id = data.profiles[index].id.clone();
        let workspace_path = shared_workspace_file_in(root, &id);
        if workspace_path.is_file() {
            let canonical = read_shared_canonical(&workspace_path)?;
            let shared_profile = desktop_profile_from_canonical(&canonical);
            if serde_json::to_value(&data.profiles[index]).ok()
                != serde_json::to_value(&shared_profile).ok()
            {
                data.profiles[index] = shared_profile;
                changed = true;
            }
        } else {
            persist_shared_profile_at(&workspace_path, &data.profiles[index])?;
        }

        let secrets_path = shared_secrets_file_next_to(&workspace_path);
        if secrets_path.is_file() {
            let shared = shared_secret_values(&read_maybe_wrapped_json(&secrets_path)?);
            if data.workspace_secrets.get(&id) != Some(&shared) {
                data.workspace_secrets.insert(id.clone(), shared);
                changed = true;
            }
        } else if let Some(secrets) = data.workspace_secrets.get(&id) {
            write_shared_secret_values_at(&secrets_path, secrets)?;
        }
    }
    Ok(changed)
}

pub fn export_workspace_pack_from_data(data: &AppData, id: &str) -> AppResult<serde_json::Value> {
    let profile = data
        .profiles
        .iter()
        .find(|profile| profile.id == id)
        .ok_or_else(|| AppError::Message(format!("workspace not found: {id}")))?;
    let presence = secret_presence_from_map(data.workspace_secrets.get(id));
    build_workspace_pack(profile, presence).map_err(AppError::Message)
}

pub fn open_shared_workspace_file_into_data(
    data: &mut AppData,
    path: &std::path::Path,
) -> AppResult<WorkspaceProfile> {
    let pack = read_maybe_wrapped_json(path)?;
    let profile = import_workspace_pack_into_data(data, pack)?;
    let secrets_path = shared_secrets_file_next_to(path);
    if secrets_path.is_file() {
        let secrets = read_maybe_wrapped_json(&secrets_path)?;
        apply_shared_secret_document(data, &profile.id, &secrets);
    }
    Ok(profile)
}

pub fn export_shared_workspace_from_data(
    data: &AppData,
    id: &str,
    workspace_path: &std::path::Path,
) -> AppResult<()> {
    let profile = data
        .profiles
        .iter()
        .find(|profile| profile.id == id)
        .ok_or_else(|| AppError::Message(format!("workspace not found: {id}")))?;
    let existing = if workspace_path.is_file() {
        Some(
            parse_canonical_workspace(&read_maybe_wrapped_json(workspace_path)?)
                .map_err(AppError::Message)?,
        )
    } else {
        None
    };
    let canonical = merge_desktop_profile_on_canonical(existing.as_ref(), profile);
    let value = serialize_canonical_workspace(&canonical).map_err(AppError::Message)?;
    write_wrapped_json(workspace_path, &value)?;
    write_wrapped_json(
        &shared_secrets_file_next_to(workspace_path),
        &shared_secret_document(data, id),
    )?;
    Ok(())
}

fn shared_secret_document(data: &AppData, id: &str) -> serde_json::Value {
    let empty = std::collections::HashMap::new();
    let secrets = data.workspace_secrets.get(id).unwrap_or(&empty);
    serde_json::json!({
        "schemaVersion": 1,
        "values": secrets,
    })
}

fn apply_shared_secret_document(data: &mut AppData, id: &str, secrets: &serde_json::Value) {
    data.workspace_secrets
        .insert(id.to_string(), shared_secret_values(secrets));
}

pub fn import_workspace_pack_into_data(
    data: &mut AppData,
    pack: serde_json::Value,
) -> AppResult<WorkspaceProfile> {
    let mut profile = profile_from_workspace_pack(&pack)?;
    if profile.id.trim().is_empty() || data.profiles.iter().any(|item| item.id == profile.id) {
        profile.id = uuid::Uuid::new_v4().to_string().replace('-', "");
    }
    assign_free_workspace_ports(&data.profiles, &mut profile)?;
    let secrets = data
        .workspace_secrets
        .entry(profile.id.clone())
        .or_default();
    for key in [
        "oauth_password",
        "oauth_token_secret",
        "bearer_token",
        "actions_api_key",
        "actions_oauth_client_secret",
        "actions_oauth_password",
        "actions_oauth_token_secret",
    ] {
        if secrets
            .get(key)
            .map(|value| value.trim().is_empty())
            .unwrap_or(true)
        {
            secrets.insert(key.to_string(), random_secret());
        }
    }
    data.profiles.push(profile.clone());
    Ok(profile)
}

fn lock_data_file() -> AppResult<std::sync::MutexGuard<'static, ()>> {
    DATA_FILE_LOCK
        .lock()
        .map_err(|_| AppError::Message("data file lock poisoned".into()))
}

fn random_secret() -> String {
    format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()).replace('-', "")
}

fn shared_value_for_key(key: &str) -> String {
    if key == "oauth_client_id" {
        format!("chatgpt-client-{}", &uuid::Uuid::new_v4().to_string()[..12])
    } else {
        random_secret()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_secret_roundtrip() {
        let id = uuid::Uuid::new_v4().to_string().replace('-', "");
        let mut store = DataStore::load().expect("load");
        store
            .set_workspace_secret(&id, "oauth_client_secret", "roundtrip-secret")
            .expect("set");
        let loaded = store
            .get_workspace_secret(&id, "oauth_client_secret")
            .expect("get");
        assert_eq!(loaded.as_deref(), Some("roundtrip-secret"));
        store.remove_workspace_secrets(&id).expect("remove");
    }

    #[test]
    fn workspace_secret_helpers_prefer_and_update_shared_live_store() {
        let root = tempfile::tempdir().expect("shared root");
        let profile_id = "shared-secret-workspace";
        let key = "builtin_tunnel_device_identity";
        let shared_path =
            shared_secrets_file_next_to(&shared_workspace_file_in(root.path(), profile_id));
        write_shared_secret_values_at(
            &shared_path,
            &std::collections::HashMap::from([(key.into(), "shared-enrolled".into())]),
        )
        .expect("seed shared secret");

        let mut data = AppData::default();
        data.workspace_secrets.insert(
            profile_id.into(),
            std::collections::HashMap::from([(key.into(), "stale-local".into())]),
        );
        assert_eq!(
            workspace_secret_value_at(&data, profile_id, key, Some(root.path()))
                .expect("read shared secret")
                .as_deref(),
            Some("shared-enrolled")
        );

        update_workspace_secret_value_at(
            &mut data,
            profile_id,
            key,
            "updated-enrolled",
            Some(root.path()),
        )
        .expect("update shared secret");
        assert_eq!(data.workspace_secrets[profile_id][key], "updated-enrolled");
        let persisted =
            shared_secret_values(&read_maybe_wrapped_json(&shared_path).expect("read persisted"));
        assert_eq!(persisted[key], "updated-enrolled");
    }

    #[test]
    fn shared_oauth_client_id_uses_client_id_format() {
        let value = shared_value_for_key("oauth_client_id");
        assert!(value.starts_with("chatgpt-client-"));
        assert_eq!(value.len(), "chatgpt-client-".len() + 12);
    }

    #[test]
    fn mcp_auto_start_ids_only_include_enabled_existing_workspaces() {
        let first = WorkspaceProfile::new("C:/workspace/first".into(), Some("first".into()));
        let second = WorkspaceProfile::new("C:/workspace/second".into(), Some("second".into()));
        let mut data = AppData::default();
        data.profiles = vec![first.clone(), second.clone()];
        data.mcp_enabled_workspace_ids = vec![
            second.id.clone(),
            "deleted-workspace".into(),
            second.id.clone(),
        ];
        let store = DataStore { data };

        assert!(!store.mcp_runtime_enabled(&first.id));
        assert!(store.mcp_runtime_enabled(&second.id));
        assert_eq!(store.mcp_auto_start_workspace_ids(), vec![second.id]);
    }

    #[test]
    fn actions_auto_start_ids_only_include_enabled_existing_workspaces() {
        let first =
            WorkspaceProfile::new("C:/workspace/first-actions".into(), Some("first".into()));
        let second =
            WorkspaceProfile::new("C:/workspace/second-actions".into(), Some("second".into()));
        let mut data = AppData::default();
        data.profiles = vec![first.clone(), second.clone()];
        data.actions_enabled_workspace_ids = vec![
            second.id.clone(),
            "deleted-workspace".into(),
            second.id.clone(),
        ];
        let store = DataStore { data };

        assert!(!store.actions_runtime_enabled(&first.id));
        assert!(store.actions_runtime_enabled(&second.id));
        assert_eq!(store.actions_auto_start_workspace_ids(), vec![second.id]);
    }

    #[test]
    fn legacy_app_data_defaults_runtime_state_to_disabled() {
        let data: AppData = serde_json::from_str(r#"{"profiles":[]}"#).expect("deserialize");
        assert!(data.mcp_enabled_workspace_ids.is_empty());
        assert!(data.actions_enabled_workspace_ids.is_empty());
    }

    #[test]
    fn workspace_pack_roundtrip_allocates_port_and_keeps_secrets_out_of_json() {
        let root = tempfile::tempdir().expect("tempdir");
        let folder = root.path().to_string_lossy().into_owned();
        let mut source = WorkspaceProfile::new(folder.clone(), Some("pack-src".into()));
        source.runtime.local_port = 19111;
        let mut data = AppData::default();
        let mut secrets = std::collections::HashMap::new();
        secrets.insert(
            "oauth_password".to_string(),
            "pack-oauth-password".to_string(),
        );
        secrets.insert(
            "oauth_token_secret".to_string(),
            "pack-token-secret".to_string(),
        );
        data.workspace_secrets.insert(source.id.clone(), secrets);
        data.profiles.push(source.clone());

        let pack = export_workspace_pack_from_data(&data, &source.id).expect("export");
        let text = pack.to_string();
        assert!(!text.contains("pack-oauth-password"));
        assert!(!text.contains("pack-token-secret"));
        assert!(pack.pointer("/host/node/dataDir").is_none());
        assert_eq!(pack["secretPresence"]["oauthPassword"], true);

        let mut occupied = WorkspaceProfile::new(folder, Some("occupied".into()));
        occupied.runtime.local_port = 19111;
        occupied.actions.local_port = 19112;
        let mut dest = AppData::default();
        dest.profiles.push(occupied);
        let imported = import_workspace_pack_into_data(&mut dest, pack).expect("import");
        assert_ne!(imported.runtime.local_port, 19111);
        let seeded = dest.workspace_secrets.get(&imported.id).expect("seeded");
        assert!(!seeded["oauth_password"].is_empty());
        assert_ne!(seeded["oauth_password"], "pack-oauth-password");
        if let Ok(dump) = std::env::var("CTMCP_PHASE5_DUMP") {
            let imported_json = serde_json::to_vec_pretty(&dest).expect("import dump");
            std::fs::write(format!("{dump}/import-desktop.json"), imported_json)
                .expect("write import");
        }
    }

    #[test]
    fn open_shared_workspace_file_reads_drop_target_without_changing_primary_layout() {
        let root = tempfile::tempdir().expect("tempdir");
        let folder = root.path().join("repo");
        std::fs::create_dir_all(&folder).expect("repo");
        let drop_dir = root
            .path()
            .join("CodingToolsMCP")
            .join("workspaces")
            .join("ws-drop");
        std::fs::create_dir_all(&drop_dir).expect("drop");
        let drop_file = drop_dir.join("workspace.json");
        let source =
            WorkspaceProfile::new(folder.to_string_lossy().into_owned(), Some("drop".into()));
        let mut seed = AppData::default();
        seed.profiles.push(source.clone());
        let mut secrets = std::collections::HashMap::new();
        secrets.insert(
            "oauth_password".to_string(),
            "shared-drop-password".to_string(),
        );
        secrets.insert(
            "oauth_token_secret".to_string(),
            "shared-drop-token".to_string(),
        );
        seed.workspace_secrets.insert(source.id.clone(), secrets);
        assert!(drop_file
            .to_string_lossy()
            .replace('\\', "/")
            .contains("CodingToolsMCP/workspaces/ws-drop/workspace.json"));

        export_shared_workspace_from_data(&seed, &source.id, &drop_file).expect("wrap export");
        let wrapped = std::fs::read_to_string(&drop_file).expect("wrapped workspace");
        assert!(wrapped.contains("ctmcp-wrap"));
        assert!(!wrapped.contains("shared-drop-password"));
        let wrapped_secrets = std::fs::read_to_string(shared_secrets_file_next_to(&drop_file))
            .expect("wrapped secrets");
        assert!(wrapped_secrets.contains("ctmcp-wrap"));
        assert!(!wrapped_secrets.contains("shared-drop-password"));

        let mut dest = AppData::default();
        let opened = open_shared_workspace_file_into_data(&mut dest, &drop_file).expect("open");
        assert_eq!(opened.name, "drop");
        assert_eq!(
            dest.workspace_secrets[&opened.id]["oauth_password"],
            "shared-drop-password"
        );
        assert_eq!(
            dest.workspace_secrets[&opened.id]["oauth_token_secret"],
            "shared-drop-token"
        );
        if let Ok(dump) = std::env::var("CTMCP_PHASE6_DUMP") {
            std::fs::write(
                format!("{dump}/phase6-open.txt"),
                format!(
                    "opened_id={}\nopened_name={}\ndrop_file={}\nprofiles={}\n",
                    opened.id,
                    opened.name,
                    drop_file.display(),
                    dest.profiles.len()
                ),
            )
            .expect("dump");
        }
    }

    #[test]
    fn shared_live_store_adopts_unique_node_counterpart_and_migrates_state() {
        let root = tempfile::tempdir().expect("shared root");
        let folder = root.path().join("repo");
        std::fs::create_dir_all(&folder).expect("repo");

        let mut desktop = WorkspaceProfile::new(
            folder.to_string_lossy().into_owned(),
            Some("desktop-local".into()),
        );
        desktop.id = "desktop-old".into();
        desktop.runtime.local_port = 3789;
        desktop.runtime.runtime_command = "desktop-runtime".into();
        desktop.folders[0].execution = crate::workspace::ExecutionTarget::Wsl {
            distro: "Ubuntu".into(),
            linux_path: "/workspace/repo".into(),
        };

        let mut node = canonical_from_desktop_profile(&desktop);
        node.id = "node-shared".into();
        node.name = "node-authoritative".into();
        node.bind.port = 4888;
        node.host.desktop = serde_json::json!({});
        node.host.node = serde_json::json!({"management": {"enabled": true}});
        for folder in &mut node.folders {
            folder.extra.remove("execution");
        }
        let node_path = shared_workspace_file_in(root.path(), "node-shared");
        write_wrapped_json(
            &node_path,
            &serialize_canonical_workspace(&node).expect("serialize node"),
        )
        .expect("write node");
        let node_secrets_path = shared_secrets_file_next_to(&node_path);
        write_shared_secret_values_at(
            &node_secrets_path,
            &std::collections::HashMap::from([
                ("oauth_password".into(), "node-password".into()),
                ("node_only".into(), "node-only-value".into()),
            ]),
        )
        .expect("write node secrets");

        let old_path = shared_workspace_file_in(root.path(), "desktop-old");
        let mut old = canonical_from_desktop_profile(&desktop);
        old.host.node = serde_json::json!({});
        write_wrapped_json(
            &old_path,
            &serialize_canonical_workspace(&old).expect("serialize desktop"),
        )
        .expect("write desktop");
        write_shared_secret_values_at(
            &shared_secrets_file_next_to(&old_path),
            &std::collections::HashMap::from([
                ("actions_api_key".into(), "desktop-actions-key".into()),
                ("oauth_password".into(), "desktop-old-password".into()),
            ]),
        )
        .expect("write desktop secrets");

        let mut data = AppData::default();
        data.last_workspace_id = desktop.id.clone();
        data.mcp_enabled_workspace_ids = vec![desktop.id.clone(), "node-shared".into()];
        data.actions_enabled_workspace_ids = vec![desktop.id.clone()];
        data.workspace_secrets.insert(
            desktop.id.clone(),
            std::collections::HashMap::from([(
                "actions_oauth_password".into(),
                "desktop-local-action-password".into(),
            )]),
        );
        data.profiles.push(desktop);

        assert!(sync_shared_live_store_at(&mut data, root.path()).expect("sync"));
        assert_eq!(data.profiles[0].id, "node-shared");
        assert_eq!(data.profiles[0].name, "node-authoritative");
        assert_eq!(data.profiles[0].runtime.local_port, 4888);
        assert_eq!(data.profiles[0].runtime.runtime_command, "desktop-runtime");
        assert_eq!(data.last_workspace_id, "node-shared");
        assert_eq!(data.mcp_enabled_workspace_ids, vec!["node-shared"]);
        assert_eq!(data.actions_enabled_workspace_ids, vec!["node-shared"]);
        assert!(!data.workspace_secrets.contains_key("desktop-old"));
        let secrets = &data.workspace_secrets["node-shared"];
        assert_eq!(secrets["oauth_password"], "node-password");
        assert_eq!(secrets["actions_api_key"], "desktop-actions-key");
        assert_eq!(
            secrets["actions_oauth_password"],
            "desktop-local-action-password"
        );
        assert_eq!(secrets["node_only"], "node-only-value");

        let shared = read_shared_canonical(&node_path).expect("read merged node");
        assert_eq!(shared.id, "node-shared");
        assert_eq!(shared.bind.port, 4888);
        assert_eq!(
            shared.host.node["management"]["enabled"],
            serde_json::json!(true)
        );
        assert_eq!(
            shared.host.desktop["runtimeCommand"],
            serde_json::json!("desktop-runtime")
        );
        assert!(shared.folders[0].extra.get("execution").is_some());
    }

    #[test]
    fn shared_live_store_does_not_adopt_ambiguous_or_different_node_counterpart() {
        let root = tempfile::tempdir().expect("shared root");
        let folder = root.path().join("repo");
        let other = root.path().join("other");
        std::fs::create_dir_all(&folder).expect("repo");
        std::fs::create_dir_all(&other).expect("other");
        let mut desktop = WorkspaceProfile::new(
            folder.to_string_lossy().into_owned(),
            Some("desktop".into()),
        );
        desktop.id = "desktop-old".into();

        for id in ["node-a", "node-b"] {
            let mut candidate = canonical_from_desktop_profile(&desktop);
            candidate.id = id.into();
            candidate.host.desktop = serde_json::json!({});
            candidate.host.node = serde_json::json!({"management": {"enabled": true}});
            let path = shared_workspace_file_in(root.path(), id);
            write_wrapped_json(
                &path,
                &serialize_canonical_workspace(&candidate).expect("serialize"),
            )
            .expect("write candidate");
        }
        let mut different = WorkspaceProfile::new(
            other.to_string_lossy().into_owned(),
            Some("different".into()),
        );
        different.id = "node-different".into();
        let mut different = canonical_from_desktop_profile(&different);
        different.host.node = serde_json::json!({"management": {"enabled": true}});
        let different_path = shared_workspace_file_in(root.path(), "node-different");
        write_wrapped_json(
            &different_path,
            &serialize_canonical_workspace(&different).expect("serialize different"),
        )
        .expect("write different");

        assert_eq!(
            find_node_counterpart_shared_workspace_id_at(root.path(), &desktop, &desktop.id)
                .expect("find"),
            None
        );
        let mut data = AppData::default();
        data.profiles.push(desktop);
        sync_shared_live_store_at(&mut data, root.path()).expect("sync");
        assert_eq!(data.profiles[0].id, "desktop-old");
        assert!(shared_workspace_file_in(root.path(), "desktop-old").is_file());
    }

    #[test]
    fn shared_live_store_does_not_create_duplicate_local_workspace_id() {
        let root = tempfile::tempdir().expect("shared root");
        let folder = root.path().join("repo");
        let occupied_folder = root.path().join("occupied");
        std::fs::create_dir_all(&folder).expect("repo");
        std::fs::create_dir_all(&occupied_folder).expect("occupied");
        let mut desktop = WorkspaceProfile::new(
            folder.to_string_lossy().into_owned(),
            Some("desktop".into()),
        );
        desktop.id = "desktop-old".into();
        let mut occupied = WorkspaceProfile::new(
            occupied_folder.to_string_lossy().into_owned(),
            Some("occupied".into()),
        );
        occupied.id = "node-shared".into();
        let mut candidate = canonical_from_desktop_profile(&desktop);
        candidate.id = "node-shared".into();
        candidate.host.desktop = serde_json::json!({});
        candidate.host.node = serde_json::json!({"management": {"enabled": true}});
        let candidate_path = shared_workspace_file_in(root.path(), "node-shared");
        write_wrapped_json(
            &candidate_path,
            &serialize_canonical_workspace(&candidate).expect("serialize"),
        )
        .expect("write candidate");

        let mut data = AppData::default();
        data.profiles = vec![desktop, occupied];
        sync_shared_live_store_at(&mut data, root.path()).expect("sync");
        assert_eq!(data.profiles[0].id, "desktop-old");
        assert_eq!(data.profiles[1].id, "node-shared");
    }
}
