use tauri::State;

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use crate::workspace_features::{
    extension_inventory, skill_inventory, update_runtime_config, ExtensionInventoryPayload,
    ExtensionMasterToggleResult, ExtensionToggleResult, FeatureConfig, SkillInventoryPayload,
    SkillMasterToggleResult, SkillToggleResult,
};

fn normalized_key(value: &str, label: &str) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 4096 {
        return Err(AppError::Message(format!(
            "{label} must contain 1 to 4096 characters"
        )));
    }
    Ok(value.to_string())
}

fn feature_config(state: &AppState, workspace_id: &str) -> AppResult<FeatureConfig> {
    state.with_workspaces(|store| {
        let document = store.feature_document(workspace_id)?;
        Ok(FeatureConfig {
            skills: document.skills,
            extensions: document.extensions,
        })
    })
}

#[tauri::command(async)]
pub fn get_workspace_skills(
    state: State<'_, AppState>,
    workspace_id: String,
) -> AppResult<SkillInventoryPayload> {
    state.with_workspaces(|store| {
        let profile = store
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("workspace not found: {workspace_id}")))?;
        let document = store.feature_document(&workspace_id)?;
        Ok(skill_inventory(
            &workspace_id,
            &profile.folders,
            &document.skills,
        ))
    })
}

#[tauri::command(async)]
pub fn set_workspace_skills_active(
    state: State<'_, AppState>,
    workspace_id: String,
    active: bool,
) -> AppResult<SkillMasterToggleResult> {
    state.with_workspaces(|store| {
        store.update_feature_document(&workspace_id, |document| {
            document.skills.active = active;
            Ok(())
        })
    })?;
    let config = feature_config(&state, &workspace_id)?;
    let applied = update_runtime_config(&workspace_id, config);
    Ok(SkillMasterToggleResult {
        ok: true,
        workspace_id,
        active,
        restart_required: false,
        applied_immediately: applied.then(|| "skills".into()).into_iter().collect(),
    })
}

#[tauri::command(async)]
pub fn set_workspace_skill_enabled(
    state: State<'_, AppState>,
    workspace_id: String,
    skill_key: String,
    enabled: bool,
) -> AppResult<SkillToggleResult> {
    let skill_key = normalized_key(&skill_key, "skill key")?;
    state.with_workspaces(|store| {
        store.update_feature_document(&workspace_id, |document| {
            let mut disabled = document
                .skills
                .disabled
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            if enabled {
                disabled.remove(&skill_key);
            } else {
                disabled.insert(skill_key.clone());
            }
            document.skills.disabled = disabled.into_iter().collect();
            Ok(())
        })
    })?;
    let config = feature_config(&state, &workspace_id)?;
    let applied = update_runtime_config(&workspace_id, config);
    Ok(SkillToggleResult {
        ok: true,
        workspace_id,
        skill_key,
        enabled,
        restart_required: false,
        applied_immediately: applied.then(|| "skills".into()).into_iter().collect(),
    })
}

#[tauri::command]
pub async fn get_workspace_extensions(
    state: State<'_, AppState>,
    workspace_id: String,
) -> AppResult<ExtensionInventoryPayload> {
    let (folders, extensions) = state.with_workspaces(|store| {
        let profile = store
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("workspace not found: {workspace_id}")))?;
        let document = store.feature_document(&workspace_id)?;
        Ok((profile.folders, document.extensions))
    })?;
    Ok(extension_inventory(&workspace_id, &folders, &extensions).await)
}

fn extension_toggle_mut<'a>(
    extensions: &'a mut crate::workspace::canonical::CanonicalExtensions,
    kind: &str,
) -> AppResult<&'a mut crate::workspace::canonical::CanonicalToggle> {
    match kind {
        "hook" => Ok(&mut extensions.hooks),
        "mcp" => Ok(&mut extensions.mcp),
        _ => Err(AppError::Message(
            "extensionKind must be 'hook' or 'mcp'".into(),
        )),
    }
}

#[tauri::command(async)]
pub fn set_workspace_extension_active(
    state: State<'_, AppState>,
    workspace_id: String,
    extension_kind: String,
    active: bool,
) -> AppResult<ExtensionMasterToggleResult> {
    let extension_kind = extension_kind.trim().to_ascii_lowercase();
    state.with_workspaces(|store| {
        store.update_feature_document(&workspace_id, |document| {
            extension_toggle_mut(&mut document.extensions, &extension_kind)?.active = active;
            Ok(())
        })
    })?;
    let config = feature_config(&state, &workspace_id)?;
    let applied = update_runtime_config(&workspace_id, config);
    Ok(ExtensionMasterToggleResult {
        ok: true,
        workspace_id,
        extension_kind: extension_kind.clone(),
        active,
        restart_required: false,
        applied_immediately: applied
            .then(|| format!("extensions.{extension_kind}"))
            .into_iter()
            .collect(),
    })
}

#[tauri::command(async)]
pub fn set_workspace_extension_enabled(
    state: State<'_, AppState>,
    workspace_id: String,
    extension_kind: String,
    extension_key: String,
    enabled: bool,
) -> AppResult<ExtensionToggleResult> {
    let extension_kind = extension_kind.trim().to_ascii_lowercase();
    let extension_key = normalized_key(&extension_key, "extension key")?;
    state.with_workspaces(|store| {
        store.update_feature_document(&workspace_id, |document| {
            let toggle = extension_toggle_mut(&mut document.extensions, &extension_kind)?;
            let mut selected = toggle
                .enabled
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            if enabled {
                selected.insert(extension_key.clone());
            } else {
                selected.remove(&extension_key);
            }
            toggle.enabled = selected.into_iter().collect();
            Ok(())
        })
    })?;
    let config = feature_config(&state, &workspace_id)?;
    let applied = update_runtime_config(&workspace_id, config);
    Ok(ExtensionToggleResult {
        ok: true,
        workspace_id,
        extension_kind: extension_kind.clone(),
        extension_key,
        enabled,
        restart_required: false,
        applied_immediately: applied
            .then(|| format!("extensions.{extension_kind}"))
            .into_iter()
            .collect(),
    })
}
