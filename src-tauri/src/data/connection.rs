//! One transport configuration, separate from immutable project roots.
use super::model::AppData;
use crate::workspace::WorkspaceProfile;
use sha2::{Digest, Sha256};
pub const GLOBAL_MCP_ID: &str = "00000000000000000000000000000001";

pub fn control_folder() -> Result<crate::workspace::WorkspaceFolder, String> {
    let root = crate::platform::platform()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("connection-control");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    Ok(crate::workspace::WorkspaceFolder::new(
        root.to_string_lossy().into_owned(),
        Some("MCP transport".into()),
    ))
}

/// Idempotent in-memory migration; callers persist the complete envelope before use.
pub(super) fn migrate(data: &mut AppData) -> Result<bool, String> {
    if data.mcp_connection.is_some() {
        return Ok(false);
    }
    if data.profiles.iter().any(|p| p.id == GLOBAL_MCP_ID) {
        return Err("Reserved global connection ID conflicts with a project".into());
    }
    let source = data
        .profiles
        .iter()
        .find(|p| data.mcp_enabled_workspace_ids.contains(&p.id))
        .or(data.profiles.first())
        .cloned();
    let mut connection = source
        .clone()
        .unwrap_or_else(|| WorkspaceProfile::new(String::new(), Some("MCP".into())));
    if let Some(source) = &source {
        data.connection_project_origins
            .insert(GLOBAL_MCP_ID.into(), source.id.clone());
        if let Some(secrets) = data.workspace_secrets.get(&source.id).cloned() {
            data.workspace_secrets.insert(GLOBAL_MCP_ID.into(), secrets);
        }
    }
    connection.id = GLOBAL_MCP_ID.into();
    connection.name = "MCP".into();
    connection.path.clear();
    connection.folders.clear();
    connection.active_folder_id.clear();
    if source.is_none() {
        connection.tunnel.public_url.clear();
        connection.tunnel.tunnel_type = "cloudflare".into();
    }
    let actions_enabled = data
        .profiles
        .iter()
        .any(|p| data.actions_enabled_workspace_ids.contains(&p.id));
    if let Some(actions) = data
        .profiles
        .iter()
        .find(|p| data.actions_enabled_workspace_ids.contains(&p.id))
    {
        connection.actions = actions.actions.clone();
        if let Some(values) = data.workspace_secrets.get(&actions.id).cloned() {
            let global = data
                .workspace_secrets
                .entry(GLOBAL_MCP_ID.into())
                .or_default();
            for (key, value) in values {
                if key.starts_with("actions_") {
                    global.insert(key, value);
                }
            }
        }
    }
    let enabled = data
        .profiles
        .iter()
        .any(|p| data.mcp_enabled_workspace_ids.contains(&p.id));
    let mut projects = Vec::new();
    let mut folders = std::collections::HashSet::new();
    let mut ids = data
        .profiles
        .iter()
        .map(|p| p.id.clone())
        .collect::<std::collections::HashSet<_>>();
    if ids.len() != data.profiles.len() {
        return Err("Duplicate project IDs".into());
    }
    ids.insert(GLOBAL_MCP_ID.into());
    let mut reserved = data.profiles.clone();
    reserved.push(connection.clone());
    for profile in &data.profiles {
        if profile.folders.is_empty() {
            return Err(format!("Project {} has no directory", profile.name));
        }
        let primary = profile
            .folders
            .iter()
            .position(|f| f.id == profile.active_folder_id)
            .unwrap_or(0);
        for (index, folder) in profile.folders.iter().enumerate() {
            if !folders.insert(folder.id.clone()) {
                return Err("Project directory IDs must be unique".into());
            }
            let mut project = profile.clone();
            project.path = folder.path.clone();
            project.active_folder_id = folder.id.clone();
            project.folders = vec![folder.clone()];
            if index != primary {
                project.id = format!(
                    "{:x}",
                    Sha256::digest(format!("project:{}:{}", profile.id, folder.id).as_bytes())
                )[..32]
                    .into();
                project.name = folder.name.clone();
                if !ids.insert(project.id.clone()) {
                    return Err("Migrated project ID conflict".into());
                }
                crate::workspace::resources::assign_free_workspace_ports(&reserved, &mut project)
                    .map_err(|e| e.to_string())?;
                project.actions.public_url.clear();
                project.actions.frp_subdomain.clear();
                project.actions.tunnel_type = "none".into();
                reserved.push(project.clone());
                if let Some(secrets) = data.workspace_secrets.get(&profile.id).cloned() {
                    data.workspace_secrets.insert(project.id.clone(), secrets);
                }
                // Actions retain their original primary; secondary projects do not auto-start.
            }
            data.connection_project_origins
                .insert(project.id.clone(), profile.id.clone());
            enforce_project_isolation(&mut project);
            projects.push(project);
        }
    }
    data.actions_enabled_workspace_ids = if actions_enabled {
        vec![GLOBAL_MCP_ID.into()]
    } else {
        vec![]
    };
    data.profiles = projects;
    data.mcp_connection = Some(connection);
    data.mcp_enabled_workspace_ids = if enabled {
        vec![GLOBAL_MCP_ID.into()]
    } else {
        vec![]
    };
    Ok(true)
}

pub(super) fn enforce_project_isolation(profile: &mut WorkspaceProfile) {
    // Project isolation is logical (workspace boundary). Full access never
    // prepares AppContainer, so it must not turn the OS sandbox back on.
    profile.runtime.sandbox.enabled = false;
    profile.runtime.sandbox.external_paths.clear();
    let mut policy =
        crate::tools::policy::PolicySettings::from_runtime(&profile.runtime).security_policy;
    policy.enforce_workspace_boundary = !profile
        .runtime
        .permission_mode
        .eq_ignore_ascii_case("dangerous");
    profile.runtime.security_policy = Some(policy);
}

pub(super) fn validate_project_update(
    current: &WorkspaceProfile,
    candidate: &WorkspaceProfile,
) -> Result<(), String> {
    if candidate.folders.len() != 1
        || candidate
            .folders
            .first()
            .zip(current.folders.first())
            .is_none_or(|(a, b)| a.id != b.id || a.path != b.path || a.execution != b.execution)
        || candidate.path != current.path
        || candidate.active_folder_id != current.active_folder_id
    {
        return Err(
            "A project has one fixed directory. Create a new project to use another directory."
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn project_isolation_never_enables_appcontainer() {
        let mut profile =
            crate::workspace::WorkspaceProfile::new("C:/workspace/isolated".into(), None);
        profile.runtime.sandbox.enabled = true;
        profile.runtime.permission_mode = "dangerous".into();
        super::enforce_project_isolation(&mut profile);
        assert!(!profile.runtime.sandbox.enabled);
        let boundary = |p: &crate::workspace::WorkspaceProfile| {
            p.runtime
                .security_policy
                .as_ref()
                .unwrap()
                .enforce_workspace_boundary
        };
        // Full access lifts the path boundary; restricted modes keep it.
        assert!(!boundary(&profile));
        profile.runtime.permission_mode = "trusted".into();
        super::enforce_project_isolation(&mut profile);
        assert!(boundary(&profile));
    }

    use super::*;
    #[test]
    fn migrates_connection_secrets_and_preserves_folder_identity_and_policy() {
        let mut data = AppData::default();
        let mut project = WorkspaceProfile::new("/project/a".into(), Some("Account A".into()));
        let extra =
            crate::workspace::WorkspaceFolder::new("/project/b".into(), Some("Account B".into()));
        project.folders.push(extra.clone());
        project.runtime.permission_mode = "guarded".into();
        data.profiles.push(project.clone());
        data.mcp_enabled_workspace_ids.push(project.id.clone());
        data.workspace_secrets.insert(
            project.id.clone(),
            [("oauth_password".into(), "synthetic".into())].into(),
        );
        assert!(migrate(&mut data).unwrap());
        let connection = data.mcp_connection.as_ref().unwrap();
        assert!(connection.folders.is_empty());
        assert!(connection.path.is_empty());
        assert_eq!(connection.tunnel.public_url, project.tunnel.public_url);
        assert_eq!(
            data.workspace_secrets[GLOBAL_MCP_ID]["oauth_password"],
            "synthetic"
        );
        assert_eq!(data.profiles.len(), 2);
        assert_eq!(data.profiles[0].id, project.id);
        assert_eq!(data.profiles[0].name, "Account A");
        assert_eq!(data.profiles[1].folders[0].id, extra.id);
        assert_ne!(
            data.profiles[0].actions.local_port,
            data.profiles[1].actions.local_port
        );
        assert!(data
            .profiles
            .iter()
            .all(|p| p.folders.len() == 1 && p.runtime.permission_mode == "guarded"));
        assert_eq!(data.mcp_enabled_workspace_ids, vec![GLOBAL_MCP_ID]);
        let snapshot = serde_json::to_string(&data).unwrap();
        assert!(!migrate(&mut data).unwrap());
        assert_eq!(snapshot, serde_json::to_string(&data).unwrap());
        let mut changed = data.profiles[0].clone();
        changed.folders[0] = extra;
        assert!(validate_project_update(&data.profiles[0], &changed).is_err());
    }
    #[test]
    fn empty_install_has_connection_without_a_project() {
        let mut data = AppData::default();
        migrate(&mut data).unwrap();
        assert!(data.profiles.is_empty());
        assert!(data.mcp_enabled_workspace_ids.is_empty());
        assert!(data.mcp_connection.unwrap().path.is_empty());
    }
}
