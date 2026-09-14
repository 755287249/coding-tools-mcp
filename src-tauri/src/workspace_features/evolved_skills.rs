use sha2::{Digest, Sha256};

use crate::knowledge::EvolvedSkillRecord;

use super::{SkillDescriptor, SkillDiagnostic, SkillEvolution};

const MAX_GUIDANCE_ITEMS: usize = 16;
const MAX_GUIDANCE_ITEM_BYTES: usize = 1_000;
const MAX_GUIDANCE_TOTAL_BYTES: usize = 8 * 1024;

fn same_base_identity(skill: &SkillDescriptor, record: &EvolvedSkillRecord) -> bool {
    record.base.source == skill.source
        && record.base.name.to_lowercase() == skill.name.to_lowercase()
}

fn normalized_guidance(record: &EvolvedSkillRecord) -> Result<Vec<String>, String> {
    if record.overlay.len() != 1 || !record.overlay.contains_key("append_guidance") {
        return Err("Only append_guidance is allowed in an evolved Skill overlay.".into());
    }
    let items = record
        .overlay
        .get("append_guidance")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "append_guidance must be an array.".to_string())?;
    if items.is_empty() || items.len() > MAX_GUIDANCE_ITEMS {
        return Err(format!(
            "append_guidance must contain between 1 and {MAX_GUIDANCE_ITEMS} items."
        ));
    }
    let mut total_bytes = 0usize;
    let mut guidance = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let value = item
            .as_str()
            .ok_or_else(|| format!("append_guidance[{index}] must be a string."))?
            .trim();
        let bytes = value.len();
        if value.is_empty()
            || value.contains('\0')
            || value.contains('\n')
            || value.contains('\r')
            || bytes > MAX_GUIDANCE_ITEM_BYTES
        {
            return Err(format!(
                "append_guidance[{index}] must be a bounded single-line guidance item."
            ));
        }
        total_bytes = total_bytes.saturating_add(bytes);
        if total_bytes > MAX_GUIDANCE_TOTAL_BYTES {
            return Err(format!(
                "append_guidance exceeds {MAX_GUIDANCE_TOTAL_BYTES} total bytes."
            ));
        }
        guidance.push(value.to_string());
    }
    Ok(guidance)
}

fn apply_guidance(
    mut skill: SkillDescriptor,
    record: &EvolvedSkillRecord,
    guidance: &[String],
) -> SkillDescriptor {
    let eol = if skill.content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let novel_guidance = guidance
        .iter()
        .filter(|item| !skill.body.contains(&format!("- {item}")))
        .cloned()
        .collect::<Vec<_>>();
    if novel_guidance.is_empty() {
        skill.evolution = Some(SkillEvolution {
            knowledge_id: record.knowledge_id.clone(),
            generation: record.generation,
            base_content_sha256: record.base.content_sha256.clone(),
        });
        return skill;
    }
    let mut lines = vec![
        "## Learned guidance".to_string(),
        String::new(),
        "The following guidance was promoted from validated local workflow evidence. It remains subordinate to runtime permissions, workspace policy, and sandbox/security boundaries.".to_string(),
        String::new(),
    ];
    lines.extend(novel_guidance.iter().map(|item| format!("- {item}")));
    let section = lines.join(eol);
    skill.content = format!(
        "{}{}{}{}{}",
        skill.content.trim_end_matches(['\r', '\n']),
        eol,
        eol,
        section,
        eol
    );
    skill.body = format!(
        "{}{}{}{}{}",
        skill.body.trim_end_matches(['\r', '\n']),
        eol,
        eol,
        section,
        eol
    );
    skill.evolution = Some(SkillEvolution {
        knowledge_id: record.knowledge_id.clone(),
        generation: record.generation,
        base_content_sha256: record.base.content_sha256.clone(),
    });
    skill
}

pub fn apply_evolved_skill_record(
    skill: SkillDescriptor,
    record: &EvolvedSkillRecord,
) -> Result<SkillDescriptor, String> {
    if !same_base_identity(&skill, record) {
        return Err("Evolved Skill base identity does not match the selected Skill.".into());
    }
    let base_sha = skill
        .evolution
        .as_ref()
        .map(|evolution| evolution.base_content_sha256.clone())
        .unwrap_or_else(|| format!("{:x}", Sha256::digest(skill.content.as_bytes())));
    if record.base.content_sha256 != base_sha {
        return Err("Evolved Skill base content changed.".into());
    }
    let guidance = normalized_guidance(record)?;
    Ok(apply_guidance(skill, record, &guidance))
}

pub fn apply_promoted_evolved_skills(
    base_skills: Vec<SkillDescriptor>,
    records: &[EvolvedSkillRecord],
) -> (Vec<SkillDescriptor>, Vec<SkillDiagnostic>) {
    let promoted = records
        .iter()
        .filter(|record| record.status == crate::knowledge::KnowledgeStatus::Promoted)
        .collect::<Vec<_>>();
    let mut diagnostics = Vec::new();
    let mut matched_identities = std::collections::HashSet::new();
    let skills = base_skills
        .into_iter()
        .map(|skill| {
            let identity_matches = promoted
                .iter()
                .copied()
                .filter(|record| same_base_identity(&skill, record))
                .collect::<Vec<_>>();
            if identity_matches.is_empty() {
                return skill;
            }
            matched_identities.insert((skill.source.clone(), skill.name.to_lowercase()));
            let current = identity_matches
                .into_iter()
                .filter(|record| record.base.content_sha256 == format!("{:x}", Sha256::digest(skill.content.as_bytes())))
                .collect::<Vec<_>>();
            if current.is_empty() {
                diagnostics.push(SkillDiagnostic {
                    code: "EVOLVED_SKILL_BASE_CHANGED".into(),
                    message: format!(
                        "Promoted evolved guidance for {} was ignored because the base Skill content changed.",
                        skill.name
                    ),
                    path: Some(skill.relative_path.clone()),
                    name: Some(skill.name.clone()),
                    source: Some(skill.source.clone()),
                    scope: Some(skill.scope.clone()),
                });
                return skill;
            }
            let generation = current.iter().map(|record| record.generation).max().unwrap_or(0);
            let latest = current
                .into_iter()
                .filter(|record| record.generation == generation)
                .collect::<Vec<_>>();
            if latest.len() != 1 {
                diagnostics.push(SkillDiagnostic {
                    code: "EVOLVED_SKILL_CONFLICT".into(),
                    message: format!(
                        "Multiple promoted evolved Skill overlays claim generation {generation} for {}; all were ignored.",
                        skill.name
                    ),
                    path: Some(skill.relative_path.clone()),
                    name: Some(skill.name.clone()),
                    source: Some(skill.source.clone()),
                    scope: Some(skill.scope.clone()),
                });
                return skill;
            }
            let record = latest[0];
            match normalized_guidance(record) {
                Ok(guidance) => apply_guidance(skill, record, &guidance),
                Err(message) => {
                    diagnostics.push(SkillDiagnostic {
                        code: "EVOLVED_SKILL_INVALID_OVERLAY".into(),
                        message,
                        path: Some(skill.relative_path.clone()),
                        name: Some(skill.name.clone()),
                        source: Some(skill.source.clone()),
                        scope: Some(skill.scope.clone()),
                    });
                    skill
                }
            }
        })
        .collect::<Vec<_>>();

    for record in promoted {
        let identity = (record.base.source.clone(), record.base.name.to_lowercase());
        if matched_identities.contains(&identity) {
            continue;
        }
        diagnostics.push(SkillDiagnostic {
            code: "EVOLVED_SKILL_BASE_NOT_FOUND".into(),
            message: format!(
                "Promoted evolved guidance for {} was ignored because its base Skill is not available.",
                if record.base.name.is_empty() { &record.name } else { &record.base.name }
            ),
            path: None,
            name: Some(if record.base.name.is_empty() { record.name.clone() } else { record.base.name.clone() }),
            source: None,
            scope: None,
        });
    }
    (skills, diagnostics)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Map};

    use super::*;
    use crate::knowledge::{EvolvedSkillBase, KnowledgeEvidence, KnowledgeStatus};

    fn skill() -> SkillDescriptor {
        SkillDescriptor {
            key: "workspace:folder:project:skills/demo/SKILL.md".into(),
            name: "demo".into(),
            description: "Demo".into(),
            source: "project".into(),
            scope: "workspace".into(),
            relative_path: "skills/demo/SKILL.md".into(),
            root_relative_path: "skills/demo".into(),
            version: None,
            content: "---\nname: demo\ndescription: Demo\n---\nBase workflow\n".into(),
            body: "Base workflow".into(),
            folder_id: Some("folder".into()),
            folder_name: Some("folder".into()),
            evolution: None,
        }
    }

    fn record(base_sha: String) -> EvolvedSkillRecord {
        let mut overlay = Map::new();
        overlay.insert(
            "append_guidance".into(),
            json!(["Re-read the current file before rebuilding a stale guarded edit."]),
        );
        EvolvedSkillRecord {
            knowledge_id: "evolved-demo-1".into(),
            name: "demo".into(),
            status: KnowledgeStatus::Promoted,
            confidence: 0.95,
            base: EvolvedSkillBase {
                source: "project".into(),
                name: "demo".into(),
                content_sha256: base_sha,
            },
            generation: 1,
            overlay,
            evidence: KnowledgeEvidence {
                observations: 30,
                successes: 30,
                failures: 0,
                verified_successes: None,
                verification_failures: None,
                recovery_successes: None,
                recovery_failures: None,
                total_duration_ms: 100,
                total_request_bytes: 0,
                total_response_bytes: 0,
                first_seen_at_ms: 1,
                last_seen_at_ms: 2,
                task_context_ids: None,
                conversation_context_ids: None,
                runtime_boot_context_ids: None,
            },
        }
    }

    #[test]
    fn promoted_guidance_is_hash_bound_and_fail_closed() {
        let base = skill();
        let sha = format!("{:x}", Sha256::digest(base.content.as_bytes()));
        let (skills, diagnostics) =
            apply_promoted_evolved_skills(vec![base.clone()], &[record(sha)]);
        assert!(diagnostics.is_empty());
        assert!(skills[0].body.contains("## Learned guidance"));
        assert_eq!(skills[0].evolution.as_ref().unwrap().generation, 1);

        let (stale, diagnostics) =
            apply_promoted_evolved_skills(vec![base], &[record("0".repeat(64))]);
        assert!(!stale[0].body.contains("## Learned guidance"));
        assert!(diagnostics
            .iter()
            .any(|item| item.code == "EVOLVED_SKILL_BASE_CHANGED"));
    }
}
