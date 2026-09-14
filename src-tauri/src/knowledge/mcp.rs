use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::tools::SharedToolContext;

use super::{
    knowledge_store_for_context, KnowledgeArtifactTarget, KnowledgeRecord,
    ToolEvolutionChangeProposalRecord, ToolEvolutionChangeProposalStatus, ToolEvolutionError,
    ToolEvolutionProposalStore,
};

const RESOURCE_PREFIX: &str = "tool-evolution://coding-tools/";

struct ProposalEntry {
    folder_id: String,
    folder_name: String,
    proposal: ToolEvolutionChangeProposalRecord,
    knowledge: KnowledgeRecord,
    proposal_revision: String,
    knowledge_revision: String,
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn encode_part(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if is_unreserved(*byte) {
            output.push(*byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_part(value: &str) -> Result<String, Value> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(invalid_resource_identifier());
        }
        let Some(high) = hex_value(bytes[index + 1]) else {
            return Err(invalid_resource_identifier());
        };
        let Some(low) = hex_value(bytes[index + 2]) else {
            return Err(invalid_resource_identifier());
        };
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).map_err(|_| invalid_resource_identifier())
}

fn invalid_resource_identifier() -> Value {
    json!({ "code": -32602, "message": "Invalid Tool Evolution resource identifier." })
}

fn resource_not_found(uri: &str) -> Value {
    json!({ "code": -32002, "message": format!("Resource not found: {uri}") })
}

pub fn tool_evolution_resource_uri(folder_id: &str, proposal_id: &str) -> String {
    format!(
        "{RESOURCE_PREFIX}{}/{}",
        encode_part(folder_id),
        encode_part(proposal_id)
    )
}

pub fn is_tool_evolution_resource_uri(uri: &str) -> bool {
    uri.starts_with(RESOURCE_PREFIX)
}

fn parse_resource_uri(uri: &str) -> Result<(String, String), Value> {
    if !is_tool_evolution_resource_uri(uri) {
        return Err(resource_not_found(uri));
    }
    let parts = uri[RESOURCE_PREFIX.len()..].split('/').collect::<Vec<_>>();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(resource_not_found(uri));
    }
    Ok((decode_part(parts[0])?, decode_part(parts[1])?))
}

fn context_for_folder(
    state: &SharedToolContext,
    folder_id: &str,
) -> Result<SharedToolContext, String> {
    match crate::tools::hub::resolve_profile_folder_context(&state.profile_id, folder_id) {
        Ok(context) => Ok(context),
        Err(_error) if folder_id == "legacy" => Ok(state.clone()),
        Err(error) => Err(error),
    }
}

fn folder_catalog(state: &SharedToolContext) -> Result<Vec<(String, String)>, Value> {
    let listing = crate::tools::hub::list_workspace_folders(state, None);
    if listing.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(json!({
            "code": -32002,
            "message": listing.get("error").and_then(|error| error.get("message")).and_then(Value::as_str).unwrap_or("Workspace folder routing failed")
        }));
    }
    let mut folders = listing
        .get("folders")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|folder| {
            let id = folder.get("id")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            let name = folder
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(id);
            Some((id.to_string(), name.to_string()))
        })
        .collect::<Vec<_>>();
    folders.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(folders)
}

fn evidence_summary(record: &KnowledgeRecord) -> Value {
    json!({
        "observations": record.evidence.observations,
        "successes": record.evidence.successes,
        "failures": record.evidence.failures,
        "confidence": record.confidence,
        "task_context_count": record.evidence.task_context_ids.as_ref().map_or(0, Vec::len),
        "conversation_context_count": record.evidence.conversation_context_ids.as_ref().map_or(0, Vec::len),
        "runtime_boot_context_count": record.evidence.runtime_boot_context_ids.as_ref().map_or(0, Vec::len),
        "first_seen_at_ms": record.evidence.first_seen_at_ms,
        "last_seen_at_ms": record.evidence.last_seen_at_ms,
    })
}

fn proposal_meta(entry: &ProposalEntry) -> Value {
    json!({
        "coding-tools/resource-kind": "tool-evolution-proposal",
        "coding-tools/workspace-folder-id": entry.folder_id,
        "coding-tools/workspace-folder-name": entry.folder_name,
        "coding-tools/tool-evolution-proposal-id": entry.proposal.proposal_id,
        "coding-tools/tool-evolution-tool": entry.proposal.tool,
        "coding-tools/tool-evolution-proposal-type": entry.proposal.proposal_type,
        "coding-tools/tool-evolution-base-revision": entry.proposal.base_revision,
        "coding-tools/tool-evolution-proposal-revision": entry.proposal_revision,
        "coding-tools/knowledge-revision": entry.knowledge_revision,
    })
}

fn entries(state: &SharedToolContext) -> Result<(Vec<ProposalEntry>, String), Value> {
    let mut output = Vec::new();
    let mut revision_material = Vec::new();
    for (folder_id, folder_name) in folder_catalog(state)? {
        let context = context_for_folder(state, &folder_id)
            .map_err(|message| json!({ "code": -32002, "message": message }))?;
        let knowledge_store = knowledge_store_for_context(context.as_ref());
        let proposal_store = ToolEvolutionProposalStore::new(knowledge_store.root());
        let proposals = proposal_store.list().map_err(tool_evolution_rpc_error)?;
        let proposal_revision = proposal_store
            .revision()
            .map_err(tool_evolution_rpc_error)?;
        let knowledge_revision = knowledge_store
            .revision()
            .map_err(|error| tool_evolution_rpc_error(error.into()))?;
        revision_material.push(json!({
            "folderId": folder_id,
            "proposalRevision": proposal_revision,
            "knowledgeRevision": knowledge_revision,
        }));
        for proposal in proposals {
            if proposal.status != ToolEvolutionChangeProposalStatus::Proposed {
                continue;
            }
            let Some(knowledge) = knowledge_store
                .get(&proposal.knowledge_id)
                .map_err(|error| tool_evolution_rpc_error(error.into()))?
            else {
                continue;
            };
            if knowledge.target != KnowledgeArtifactTarget::ToolEvolution {
                continue;
            }
            output.push(ProposalEntry {
                folder_id: folder_id.clone(),
                folder_name: folder_name.clone(),
                proposal,
                knowledge,
                proposal_revision: proposal_revision.clone(),
                knowledge_revision: knowledge_revision.clone(),
            });
        }
    }
    output.sort_by(|left, right| {
        left.folder_id
            .cmp(&right.folder_id)
            .then_with(|| left.proposal.tool.cmp(&right.proposal.tool))
            .then_with(|| left.proposal.proposal_id.cmp(&right.proposal.proposal_id))
    });
    let revision = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&revision_material).unwrap_or_default())
    );
    Ok((output, revision))
}

fn find_entry(
    state: &SharedToolContext,
    folder_id: &str,
    proposal_id: &str,
) -> Result<Option<ProposalEntry>, Value> {
    let Some((_, folder_name)) = folder_catalog(state)?
        .into_iter()
        .find(|(candidate, _)| candidate == folder_id)
    else {
        return Ok(None);
    };
    let context = context_for_folder(state, folder_id)
        .map_err(|message| json!({ "code": -32002, "message": message }))?;
    let knowledge_store = knowledge_store_for_context(context.as_ref());
    let proposal_store = ToolEvolutionProposalStore::new(knowledge_store.root());
    let Some(proposal) = proposal_store
        .get(proposal_id)
        .map_err(tool_evolution_rpc_error)?
    else {
        return Ok(None);
    };
    if proposal.status != ToolEvolutionChangeProposalStatus::Proposed {
        return Ok(None);
    }
    let Some(knowledge) = knowledge_store
        .get(&proposal.knowledge_id)
        .map_err(|error| tool_evolution_rpc_error(error.into()))?
    else {
        return Ok(None);
    };
    if knowledge.target != KnowledgeArtifactTarget::ToolEvolution {
        return Ok(None);
    }
    Ok(Some(ProposalEntry {
        folder_id: folder_id.to_string(),
        folder_name,
        proposal,
        knowledge,
        proposal_revision: proposal_store
            .revision()
            .map_err(tool_evolution_rpc_error)?,
        knowledge_revision: knowledge_store
            .revision()
            .map_err(|error| tool_evolution_rpc_error(error.into()))?,
    }))
}

fn tool_evolution_rpc_error(error: ToolEvolutionError) -> Value {
    json!({ "code": -32603, "message": format!("Tool Evolution resource error: {error}") })
}

pub fn list_tool_evolution_resources(state: &SharedToolContext) -> Result<Value, Value> {
    let (entries, revision) = entries(state)?;
    Ok(json!({
        "resources": entries.into_iter().map(|entry| {
            json!({
                "uri": tool_evolution_resource_uri(&entry.folder_id, &entry.proposal.proposal_id),
                "name": format!("{}-evolution-{}", entry.proposal.tool, &entry.proposal.proposal_id[..entry.proposal.proposal_id.len().min(12)]),
                "title": format!("Tool Evolution: {} — {}", entry.proposal.tool, entry.folder_name),
                "description": entry.knowledge.hypothesis,
                "mimeType": "application/json",
                "_meta": proposal_meta(&entry),
            })
        }).collect::<Vec<_>>(),
        "_meta": {
            "coding-tools/tool-evolution-resource-revision": revision
        }
    }))
}

pub fn read_tool_evolution_resource(state: &SharedToolContext, uri: &str) -> Result<Value, Value> {
    let (folder_id, proposal_id) = parse_resource_uri(uri)?;
    let Some(entry) = find_entry(state, &folder_id, &proposal_id)? else {
        return Err(resource_not_found(uri));
    };
    let document = json!({
        "schema_version": 1,
        "kind": "tool_evolution_proposal",
        "workspace": {
            "folder_id": entry.folder_id,
            "name": entry.folder_name,
        },
        "proposal": {
            "id": entry.proposal.proposal_id,
            "status": entry.proposal.status,
            "tool": entry.proposal.tool,
            "proposal_type": entry.proposal.proposal_type,
            "base_revision": entry.proposal.base_revision,
            "created_at_ms": entry.proposal.created_at_ms,
            "updated_at_ms": entry.proposal.updated_at_ms,
        },
        "hypothesis": entry.knowledge.hypothesis,
        "recommended_action": entry.proposal.action,
        "evidence": evidence_summary(&entry.knowledge),
        "implementation_contract": {
            "isolated_worktree_required": true,
            "candidate_commit_required": true,
            "required_commit_trailer": format!("Tool-Evolution-Proposal: {}", entry.proposal.proposal_id),
            "base_revision_must_match": entry.proposal.base_revision,
            "preserve_static_security_boundaries": true,
            "no_automatic_promotion": true,
            "benchmark_before_promotion": true,
            "validation_objective": "preserve verified task quality while reducing calls, latency, or response cost",
        }
    });
    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": "application/json",
            "text": format!("{}\n", serde_json::to_string_pretty(&document).unwrap_or_else(|_| "{}".into()))
        }],
        "_meta": proposal_meta(&entry),
    }))
}

pub fn merge_resource_lists(mut skills: Value, evolution: Value) -> Value {
    let skill_resources = skills.get_mut("resources").and_then(Value::as_array_mut);
    let evolution_resources = evolution.get("resources").and_then(Value::as_array);
    if let (Some(target), Some(source)) = (skill_resources, evolution_resources) {
        target.extend(source.iter().cloned());
    } else if skills.get("resources").is_none() {
        skills["resources"] = evolution
            .get("resources")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
    }
    let target_meta = skills
        .as_object_mut()
        .expect("resource list result is an object")
        .entry("_meta")
        .or_insert_with(|| Value::Object(Map::new()));
    if let (Some(target), Some(source)) = (
        target_meta.as_object_mut(),
        evolution.get("_meta").and_then(Value::as_object),
    ) {
        target.extend(source.clone());
    }
    skills
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::knowledge::{
        ExperienceCompiler, ExperienceObservation, ExperienceOutcome, KnowledgeStatus,
        ToolEvolutionPlanner, ToolEvolutionRuntimeSource,
    };
    use crate::tools::ToolContext;

    fn candidate_knowledge(state: &SharedToolContext) -> KnowledgeRecord {
        let store = knowledge_store_for_context(state.as_ref());
        let observations = (0..5)
            .map(|index| ExperienceObservation {
                event_id: format!("resource-event-{index}"),
                tool: "project_state".into(),
                outcome: ExperienceOutcome::Success,
                duration_ms: 2_000,
                request_bytes: None,
                response_bytes: None,
                error_code: None,
                features: json!({"phase_baseline_capture_ms": 1_500})
                    .as_object()
                    .cloned(),
                verification_outcome: None,
                recovery_outcome: None,
                canary: None,
                operation_id: None,
                task_id: Some(format!("resource-task-{index}")),
                conversation_context_id: None,
                skill: None,
                runtime_boot_id: None,
                timestamp_ms: 1_000 + index,
            })
            .collect::<Vec<_>>();
        let record = ExperienceCompiler.compile(&observations).unwrap().remove(0);
        store.upsert(record).unwrap()
    }

    fn state() -> (tempfile::TempDir, tempfile::TempDir, SharedToolContext) {
        let workspace = tempdir().unwrap();
        let harness = tempdir().unwrap();
        let context =
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .unwrap();
        (workspace, harness, std::sync::Arc::new(context))
    }

    #[test]
    fn proposal_resource_list_and_read_are_proposed_only_and_workspace_bound() {
        let (_workspace, _harness, state) = state();
        let knowledge = candidate_knowledge(&state);
        assert_eq!(knowledge.status, KnowledgeStatus::Candidate);
        let store = knowledge_store_for_context(state.as_ref());
        let proposals = ToolEvolutionProposalStore::new(store.root());
        let planner = ToolEvolutionPlanner::new(
            store,
            proposals.clone(),
            None,
            ToolEvolutionRuntimeSource {
                revision: "base-a".into(),
                trusted: true,
                proposal_ids: vec![],
            },
        );
        planner.sync(10).unwrap();
        let proposal = proposals.list().unwrap().remove(0);

        let listed = list_tool_evolution_resources(&state).unwrap();
        assert_eq!(listed["resources"].as_array().unwrap().len(), 1);
        let uri = listed["resources"][0]["uri"].as_str().unwrap().to_string();
        assert!(uri.starts_with(RESOURCE_PREFIX));
        assert!(uri.contains("legacy"));
        let read = read_tool_evolution_resource(&state, &uri).unwrap();
        let text = read["contents"][0]["text"].as_str().unwrap();
        assert!(text.contains("tool_evolution_proposal"));
        assert!(text.contains("Tool-Evolution-Proposal"));
        assert!(text.contains("preserve_static_security_boundaries"));
        assert!(text.contains(&proposal.proposal_id));

        proposals.supersede(&proposal.proposal_id, 20).unwrap();
        assert!(list_tool_evolution_resources(&state).unwrap()["resources"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            read_tool_evolution_resource(&state, &uri).unwrap_err()["code"],
            -32002
        );
    }

    #[test]
    fn resource_uri_percent_encoding_round_trips_utf8_and_slashes() {
        let uri = tool_evolution_resource_uri("資料夾/a", "proposal%id");
        let parsed = parse_resource_uri(&uri).unwrap();
        assert_eq!(parsed.0, "資料夾/a");
        assert_eq!(parsed.1, "proposal%id");
    }

    #[test]
    fn merge_resource_lists_preserves_skill_meta_and_adds_evolution_meta() {
        let merged = merge_resource_lists(
            json!({"resources": [{"uri": "skill://one"}], "_meta": {"skill-revision": "a"}}),
            json!({"resources": [{"uri": "tool-evolution://one"}], "_meta": {"evolution-revision": "b"}}),
        );
        assert_eq!(merged["resources"].as_array().unwrap().len(), 2);
        assert_eq!(merged["_meta"]["skill-revision"], "a");
        assert_eq!(merged["_meta"]["evolution-revision"], "b");
    }
}
