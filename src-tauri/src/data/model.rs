use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::settings::{DownloadConfig, FrpProfile, ProxyConfig};
use crate::workspace::canonical::{decode_stored_profile, encode_stored_profile};
use crate::workspace::WorkspaceProfile;

/// Unified on-disk payload stored in `data/profiles.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppData {
    /// Global MCP transport; it grants no directory access on its own.
    #[serde(default)]
    pub mcp_connection: Option<WorkspaceProfile>,
    #[serde(default)]
    pub connection_migration_pending: bool,
    #[serde(default)]
    pub connection_project_origins: HashMap<String, String>,
    #[serde(default)]
    pub connection_feature_snapshots: HashMap<String, Value>,
    #[serde(default)]
    pub frp_profiles: Vec<FrpProfile>,
    #[serde(default)]
    pub last_workspace_id: String,
    #[serde(default)]
    pub download: DownloadConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub shared_secrets: HashMap<String, String>,
    #[serde(default)]
    pub workspace_secrets: HashMap<String, HashMap<String, String>>,
    #[serde(default)]
    pub app_secrets: HashMap<String, HashMap<String, String>>,
    /// Workspace ids whose MCP runtime was last left enabled. This is runtime
    /// state, not workspace configuration, so it stays in the app data envelope.
    #[serde(default)]
    pub mcp_enabled_workspace_ids: Vec<String>,
    /// Workspace ids whose Actions runtime was last left enabled. Update
    /// handoff restores this together with MCP so version switches do not leave
    /// Actions offline after the desktop process is replaced.
    #[serde(default)]
    pub actions_enabled_workspace_ids: Vec<String>,
    #[serde(
        default,
        serialize_with = "serialize_profiles",
        deserialize_with = "deserialize_profiles"
    )]
    pub profiles: Vec<WorkspaceProfile>,
}

fn serialize_profiles<S>(profiles: &[WorkspaceProfile], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let encoded: Result<Vec<Value>, _> = profiles.iter().map(encode_stored_profile).collect();
    encoded
        .map_err(serde::ser::Error::custom)?
        .serialize(serializer)
}

fn deserialize_profiles<'de, D>(deserializer: D) -> Result<Vec<WorkspaceProfile>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<Value>::deserialize(deserializer)?;
    values
        .iter()
        .map(decode_stored_profile)
        .collect::<Result<Vec<_>, _>>()
        .map_err(serde::de::Error::custom)
}

/// Legacy `{ "profiles": [...] }` file at the app root.
#[derive(Debug, Deserialize)]
pub struct LegacyProfilesOnlyFile {
    pub profiles: Vec<WorkspaceProfile>,
}
