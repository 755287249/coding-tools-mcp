use std::collections::BTreeSet;

use super::canary::{
    canary_cohort_bucket, CanaryError, CanaryImpactRecord, CanaryImpactStore, CanaryStage,
};
use super::model::{KnowledgeArtifactTarget, KnowledgeStatus};
use super::registries::{EvolvedSkillRecord, EvolvedSkillRegistry};
use super::store::KnowledgeStore;
use super::strategy_implementations::ToolStrategyImplementationId;

pub const EVOLVED_SKILL_CANARY_IMPLEMENTATION: ToolStrategyImplementationId =
    ToolStrategyImplementationId::EvolvedSkillAppendGuidanceV1;

const CANARY_PERCENT: u32 = 25;
const MIN_INTERVENTION_CALLS: u64 = 20;
const MIN_CONTROL_CALLS: u64 = 10;
const MIN_TASK_OUTCOMES: u64 = 3;
const MIN_VERIFIED_TASK_RATE: f64 = 0.8;
const MAX_TASK_FAILURE_RATE: f64 = 0.2;
const MAX_INTERVENTION_FAILURE_RATE: f64 = 0.25;
const MIN_RELATIVE_FAILURE_IMPROVEMENT: f64 = 0.2;
const MIN_ABSOLUTE_FAILURE_IMPROVEMENT: f64 = 0.1;
const MAX_DURATION_REGRESSION_RATIO: f64 = 1.25;
const POST_PROMOTION_MIN_CALLS: u64 = 10;
const MAX_ROLLBACK_FAILURE_RATE_DELTA: f64 = 0.15;

#[derive(Debug, Clone, PartialEq)]
pub struct EvolvedSkillCanaryDecision {
    pub knowledge_id: String,
    pub implementation: ToolStrategyImplementationId,
    pub eligible: bool,
    pub stage: CanaryStage,
    pub applied: bool,
    pub bucket: u32,
    pub already_applied: bool,
    pub record: EvolvedSkillRecord,
}

#[derive(Debug, Clone)]
pub struct EvolvedSkillCanaryEngine {
    knowledge_store: KnowledgeStore,
    impact_store: CanaryImpactStore,
}

impl EvolvedSkillCanaryEngine {
    pub fn new(knowledge_store: KnowledgeStore, impact_store: CanaryImpactStore) -> Self {
        Self {
            knowledge_store,
            impact_store,
        }
    }

    pub fn decide(
        &self,
        skill_source: &str,
        skill_name: &str,
        base_content_sha256: &str,
        current_evolution_knowledge_id: Option<&str>,
        sample_key: &str,
    ) -> Result<Option<EvolvedSkillCanaryDecision>, CanaryError> {
        let rolled_back = self
            .impact_store
            .list()?
            .into_iter()
            .filter(|record| record.rolled_back_at_ms.is_some())
            .map(|record| record.knowledge_id)
            .collect::<BTreeSet<_>>();
        let mut eligible = EvolvedSkillRegistry::new(self.knowledge_store.clone())
            .snapshot(true)?
            .skills
            .into_iter()
            .filter(|record| {
                matches!(
                    record.status,
                    KnowledgeStatus::Validated | KnowledgeStatus::Promoted
                )
            })
            .filter(|record| !rolled_back.contains(&record.knowledge_id))
            .filter(|record| {
                record.base.source == skill_source
                    && record.base.name.eq_ignore_ascii_case(skill_name)
                    && record.base.content_sha256 == base_content_sha256
            })
            .collect::<Vec<_>>();
        eligible.sort_by(|left, right| {
            right
                .generation
                .cmp(&left.generation)
                .then_with(|| left.knowledge_id.cmp(&right.knowledge_id))
        });
        let Some(first) = eligible.first() else {
            return Ok(None);
        };
        let generation = first.generation;
        let latest = eligible
            .into_iter()
            .filter(|record| record.generation == generation)
            .collect::<Vec<_>>();
        if latest.len() != 1 {
            return Ok(None);
        }
        let record = latest.into_iter().next().expect("one evolved Skill record");
        let bucket = canary_cohort_bucket(&record.knowledge_id, sample_key);
        let promoted = record.status == KnowledgeStatus::Promoted;
        Ok(Some(EvolvedSkillCanaryDecision {
            knowledge_id: record.knowledge_id.clone(),
            implementation: EVOLVED_SKILL_CANARY_IMPLEMENTATION,
            eligible: true,
            stage: if promoted {
                CanaryStage::Promoted
            } else {
                CanaryStage::Canary
            },
            applied: promoted || bucket < CANARY_PERCENT,
            bucket,
            already_applied: current_evolution_knowledge_id == Some(record.knowledge_id.as_str()),
            record,
        }))
    }
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator > 0 {
        numerator as f64 / denominator as f64
    } else {
        0.0
    }
}

fn task_outcomes(record: &CanaryImpactRecord, intervention: bool) -> u64 {
    if intervention {
        record.intervention_task_verified_successes
            + record.intervention_task_unverified_successes
            + record.intervention_task_failures
            + record.intervention_task_rollbacks
    } else {
        record.control_task_verified_successes
            + record.control_task_unverified_successes
            + record.control_task_failures
            + record.control_task_rollbacks
    }
}

fn failure_improved(control_rate: f64, intervention_rate: f64) -> bool {
    if control_rate <= 0.0 {
        return false;
    }
    let absolute = control_rate - intervention_rate;
    let relative = absolute / control_rate;
    absolute >= MIN_ABSOLUTE_FAILURE_IMPROVEMENT || relative >= MIN_RELATIVE_FAILURE_IMPROVEMENT
}

pub fn evaluate_evolved_skill_promotions(
    store: &KnowledgeStore,
    canary: &CanaryImpactStore,
    timestamp_ms: u64,
) -> Result<(usize, usize), CanaryError> {
    let mut evaluated = 0;
    let mut promoted = 0;
    for impact in canary.list()? {
        if impact.implementation != EVOLVED_SKILL_CANARY_IMPLEMENTATION
            || impact.rolled_back_at_ms.is_some()
            || impact.promoted_at_ms.is_some()
        {
            continue;
        }
        let Some(record) = store.get(&impact.knowledge_id)? else {
            continue;
        };
        if record.target != KnowledgeArtifactTarget::EvolvedSkill
            || record.status != KnowledgeStatus::Validated
            || impact.intervention_calls < MIN_INTERVENTION_CALLS
            || impact.control_calls < MIN_CONTROL_CALLS
        {
            continue;
        }
        evaluated += 1;
        if impact.intervention_task_rollbacks > 0 {
            continue;
        }
        let tasks = task_outcomes(&impact, true);
        if tasks < MIN_TASK_OUTCOMES
            || ratio(impact.intervention_task_verified_successes, tasks) < MIN_VERIFIED_TASK_RATE
            || ratio(impact.intervention_task_failures, tasks) > MAX_TASK_FAILURE_RATE
        {
            continue;
        }
        let intervention_failure = ratio(impact.intervention_failures, impact.intervention_calls);
        let control_failure = ratio(impact.control_failures, impact.control_calls);
        if intervention_failure > MAX_INTERVENTION_FAILURE_RATE
            || !failure_improved(control_failure, intervention_failure)
        {
            continue;
        }
        let intervention_average =
            ratio(impact.intervention_duration_ms, impact.intervention_calls);
        let control_average = ratio(impact.control_duration_ms, impact.control_calls);
        if control_average > 0.0
            && intervention_average > control_average * MAX_DURATION_REGRESSION_RATIO
        {
            continue;
        }
        if store
            .transition_status(
                &record.id,
                &[KnowledgeStatus::Validated],
                KnowledgeStatus::Promoted,
                timestamp_ms,
            )?
            .is_none()
        {
            continue;
        }
        match canary.mark_promoted(&record.id, timestamp_ms) {
            Ok(true) => promoted += 1,
            Ok(false) => {
                let _ = store.transition_status(
                    &record.id,
                    &[KnowledgeStatus::Promoted],
                    KnowledgeStatus::Validated,
                    timestamp_ms,
                )?;
            }
            Err(error) => {
                let _ = store.transition_status(
                    &record.id,
                    &[KnowledgeStatus::Promoted],
                    KnowledgeStatus::Validated,
                    timestamp_ms,
                );
                return Err(error);
            }
        }
    }
    Ok((evaluated, promoted))
}

fn promoted_delta(record: &CanaryImpactRecord) -> (u64, u64, u64, u64, u64, u64, u64) {
    (
        record.intervention_calls.saturating_sub(
            record
                .promotion_intervention_calls
                .unwrap_or(record.intervention_calls),
        ),
        record.intervention_failures.saturating_sub(
            record
                .promotion_intervention_failures
                .unwrap_or(record.intervention_failures),
        ),
        record.intervention_duration_ms.saturating_sub(
            record
                .promotion_intervention_duration_ms
                .unwrap_or(record.intervention_duration_ms),
        ),
        record.intervention_task_verified_successes.saturating_sub(
            record
                .promotion_intervention_task_verified_successes
                .unwrap_or(record.intervention_task_verified_successes),
        ),
        record
            .intervention_task_unverified_successes
            .saturating_sub(
                record
                    .promotion_intervention_task_unverified_successes
                    .unwrap_or(record.intervention_task_unverified_successes),
            ),
        record.intervention_task_failures.saturating_sub(
            record
                .promotion_intervention_task_failures
                .unwrap_or(record.intervention_task_failures),
        ),
        record.intervention_task_rollbacks.saturating_sub(
            record
                .promotion_intervention_task_rollbacks
                .unwrap_or(record.intervention_task_rollbacks),
        ),
    )
}

pub fn evaluate_evolved_skill_rollbacks(
    store: &KnowledgeStore,
    canary: &CanaryImpactStore,
    timestamp_ms: u64,
) -> Result<(usize, usize), CanaryError> {
    let mut evaluated = 0;
    let mut rolled_back = 0;
    for impact in canary.list()? {
        if impact.implementation != EVOLVED_SKILL_CANARY_IMPLEMENTATION
            || impact.rolled_back_at_ms.is_some()
        {
            continue;
        }
        let Some(record) = store.get(&impact.knowledge_id)? else {
            continue;
        };
        if record.target != KnowledgeArtifactTarget::EvolvedSkill
            || !matches!(
                record.status,
                KnowledgeStatus::Validated | KnowledgeStatus::Promoted
            )
            || impact.intervention_calls == 0
        {
            continue;
        }
        evaluated += 1;
        let control_failure = ratio(impact.control_failures, impact.control_calls);
        let control_average = ratio(impact.control_duration_ms, impact.control_calls);
        let mut reason = None;
        if record.status == KnowledgeStatus::Promoted {
            let (calls, failures, duration_ms, verified, unverified, task_failures, task_rollbacks) =
                promoted_delta(&impact);
            if task_rollbacks > 0 {
                reason = Some("promoted_task_rollback");
            } else if calls >= POST_PROMOTION_MIN_CALLS {
                let failure_rate = ratio(failures, calls);
                if failure_rate
                    > MAX_INTERVENTION_FAILURE_RATE
                        .max(control_failure + MAX_ROLLBACK_FAILURE_RATE_DELTA)
                {
                    reason = Some("promoted_edit_failure_rate_regression");
                } else if control_average > 0.0
                    && ratio(duration_ms, calls) > control_average * MAX_DURATION_REGRESSION_RATIO
                {
                    reason = Some("promoted_edit_duration_regression");
                } else {
                    let tasks = verified + unverified + task_failures + task_rollbacks;
                    if tasks >= MIN_TASK_OUTCOMES && ratio(task_failures, tasks) > 0.3 {
                        reason = Some("promoted_task_failure_regression");
                    }
                }
            }
        } else if impact.intervention_task_rollbacks > 0 {
            reason = Some("intervention_task_rollback");
        } else if impact.intervention_calls >= 10 {
            let failure_rate = ratio(impact.intervention_failures, impact.intervention_calls);
            if failure_rate
                > MAX_INTERVENTION_FAILURE_RATE
                    .max(control_failure + MAX_ROLLBACK_FAILURE_RATE_DELTA)
            {
                reason = Some("intervention_edit_failure_rate_regression");
            } else if control_average > 0.0
                && ratio(impact.intervention_duration_ms, impact.intervention_calls)
                    > control_average * MAX_DURATION_REGRESSION_RATIO
            {
                reason = Some("intervention_edit_duration_regression");
            } else {
                let tasks = task_outcomes(&impact, true);
                if tasks >= MIN_TASK_OUTCOMES
                    && ratio(impact.intervention_task_failures, tasks) > 0.3
                {
                    reason = Some("intervention_task_failure_regression");
                }
            }
        }
        let Some(reason) = reason else {
            continue;
        };
        if store
            .transition_status(
                &record.id,
                &[record.status],
                KnowledgeStatus::Rejected,
                timestamp_ms,
            )?
            .is_none()
        {
            continue;
        }
        match canary.mark_rolled_back(&record.id, reason, timestamp_ms) {
            Ok(true) => rolled_back += 1,
            Ok(false) => {
                let _ = store.transition_status(
                    &record.id,
                    &[KnowledgeStatus::Rejected],
                    record.status,
                    timestamp_ms,
                )?;
            }
            Err(error) => {
                let _ = store.transition_status(
                    &record.id,
                    &[KnowledgeStatus::Rejected],
                    record.status,
                    timestamp_ms,
                );
                return Err(error);
            }
        }
    }
    Ok((evaluated, rolled_back))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{
        CanaryObservation, ExperienceOutcome, KnowledgeEvidence, KnowledgeImpactRecord,
        KnowledgeImpactStore, KnowledgeRecord, KnowledgeScope, KnowledgeTaskOutcome,
        ShadowEvaluator, ValidationGate,
    };
    use serde_json::json;
    use tempfile::tempdir;

    fn evolved_skill(status: KnowledgeStatus) -> KnowledgeRecord {
        KnowledgeRecord {
            id: String::new(),
            schema_version: 1,
            scope: KnowledgeScope::Workspace,
            target: KnowledgeArtifactTarget::EvolvedSkill,
            status,
            trigger: json!({"tool":"edit","pattern":"stale_guarded_edit_recovery"})
                .as_object()
                .unwrap()
                .clone(),
            hypothesis: "A loaded Skill can learn deterministic stale-edit recovery guidance."
                .into(),
            recommended_action: json!({
                "append_guidance": [
                    "When a guarded edit reports stale state, re-read the current file and rebuild the guarded edit before retrying."
                ]
            })
            .as_object()
            .unwrap()
            .clone(),
            evidence: KnowledgeEvidence {
                observations: 5,
                successes: 0,
                failures: 5,
                verified_successes: None,
                verification_failures: None,
                recovery_successes: None,
                recovery_failures: None,
                total_duration_ms: 500,
                total_request_bytes: 0,
                total_response_bytes: 0,
                first_seen_at_ms: 1,
                last_seen_at_ms: 5,
                task_context_ids: None,
                conversation_context_ids: Some(vec!["a".repeat(64), "b".repeat(64)]),
                runtime_boot_context_ids: None,
            },
            confidence: 0.8,
            compatibility: None,
            source_event_ids: (0..5).map(|index| format!("train-{index}")).collect(),
            counterexample_event_ids: vec![],
            supersedes: None,
            superseded_by: None,
            created_at_ms: 1,
            updated_at_ms: 5,
            metadata: Some(
                json!({
                    "name": "release-helper",
                    "base": {
                        "source": "project",
                        "name": "release-helper",
                        "contentSha256": "a".repeat(64)
                    },
                    "generation": 1
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        }
    }

    fn impact(id: &str) -> KnowledgeImpactRecord {
        KnowledgeImpactRecord {
            schema_version: 1,
            knowledge_id: id.into(),
            target: KnowledgeArtifactTarget::EvolvedSkill,
            tool: "edit".into(),
            source_event_ids: (0..10).map(|index| format!("heldout-{index}")).collect(),
            matched_task_ids: (0..3).map(|index| format!("shadow-task-{index}")).collect(),
            credited_task_ids: vec![],
            considered: 10,
            matched: 10,
            actual_successes: 0,
            actual_failures: 10,
            verified_successes: 0,
            verification_failures: 0,
            recovery_successes: 0,
            recovery_failures: 0,
            task_verified_successes: 0,
            task_unverified_successes: 0,
            task_failures: 0,
            task_rollbacks: 0,
            total_observed_duration_ms: 900,
            total_observed_request_bytes: 0,
            total_observed_response_bytes: 0,
            first_seen_at_ms: 10,
            last_seen_at_ms: 19,
        }
    }

    fn canary_observation(
        id: String,
        task_id: Option<String>,
        outcome: ExperienceOutcome,
        duration_ms: u64,
        timestamp_ms: u64,
        decision: &EvolvedSkillCanaryDecision,
        applied: bool,
    ) -> CanaryObservation {
        CanaryObservation {
            event_id: id,
            task_id,
            outcome,
            duration_ms,
            timestamp_ms,
            decision: super::super::canary::KnowledgeCanaryDecision {
                knowledge_id: decision.knowledge_id.clone(),
                implementation: decision.implementation,
                eligible: decision.eligible,
                stage: decision.stage,
                applied,
                bucket: if applied { 1 } else { 50 },
            },
        }
    }

    #[test]
    fn evolved_skill_lifecycle_validates_canaries_promotes_and_rolls_back() {
        let root = tempdir().unwrap();
        let store = KnowledgeStore::new(root.path());
        let shadow_impact = KnowledgeImpactStore::new(root.path());
        let canary_impact = CanaryImpactStore::new(root.path());

        let candidate = store
            .upsert(evolved_skill(KnowledgeStatus::Candidate))
            .unwrap();
        let shadow = ShadowEvaluator::new(store.clone(), shadow_impact.clone());
        assert_eq!(shadow.activate_candidates(6).unwrap(), 1);
        assert_eq!(
            store.get(&candidate.id).unwrap().unwrap().status,
            KnowledgeStatus::Shadow
        );

        shadow_impact.record(impact(&candidate.id)).unwrap();
        for index in 0..3 {
            assert_eq!(
                shadow_impact
                    .credit_task_outcome(
                        &format!("shadow-task-{index}"),
                        KnowledgeTaskOutcome::VerifiedSuccess,
                        20 + index,
                    )
                    .unwrap(),
                1
            );
        }
        let validation = ValidationGate::new(store.clone(), shadow_impact)
            .evaluate(30)
            .unwrap();
        assert_eq!(validation.validated, 1);
        assert_eq!(
            store.get(&candidate.id).unwrap().unwrap().status,
            KnowledgeStatus::Validated
        );

        let engine = EvolvedSkillCanaryEngine::new(store.clone(), canary_impact.clone());
        let mut intervention = None;
        let mut control = None;
        for index in 0..100 {
            let decision = engine
                .decide(
                    "project",
                    "release-helper",
                    &"a".repeat(64),
                    None,
                    &format!("session-{index}"),
                )
                .unwrap()
                .unwrap();
            if decision.applied && intervention.is_none() {
                intervention = Some(decision.clone());
            }
            if !decision.applied && control.is_none() {
                control = Some(decision.clone());
            }
        }
        let intervention = intervention.expect("intervention cohort");
        let control = control.expect("control cohort");
        assert_eq!(
            intervention.implementation,
            EVOLVED_SKILL_CANARY_IMPLEMENTATION
        );
        assert_eq!(control.implementation, EVOLVED_SKILL_CANARY_IMPLEMENTATION);

        let mut observations = Vec::new();
        for index in 0..10 {
            observations.push(canary_observation(
                format!("control-{index}"),
                None,
                if index < 4 {
                    ExperienceOutcome::Failure
                } else {
                    ExperienceOutcome::Success
                },
                1_000,
                100 + index,
                &control,
                false,
            ));
        }
        for index in 0..20 {
            observations.push(canary_observation(
                format!("intervention-{index}"),
                (index < 3).then(|| format!("promote-task-{index}")),
                if index < 2 {
                    ExperienceOutcome::Failure
                } else {
                    ExperienceOutcome::Success
                },
                900,
                200 + index,
                &intervention,
                true,
            ));
        }
        assert_eq!(
            canary_impact.record_observations(&observations).unwrap(),
            30
        );
        for index in 0..3 {
            assert_eq!(
                canary_impact
                    .credit_task_outcome(
                        &format!("promote-task-{index}"),
                        KnowledgeTaskOutcome::VerifiedSuccess,
                        300 + index,
                    )
                    .unwrap(),
                1
            );
        }
        assert_eq!(
            evaluate_evolved_skill_promotions(&store, &canary_impact, 400).unwrap(),
            (1, 1)
        );
        assert_eq!(
            store.get(&candidate.id).unwrap().unwrap().status,
            KnowledgeStatus::Promoted
        );

        let promoted = engine
            .decide(
                "project",
                "release-helper",
                &"a".repeat(64),
                Some(&candidate.id),
                "after-promotion",
            )
            .unwrap()
            .unwrap();
        assert_eq!(promoted.stage, CanaryStage::Promoted);
        assert!(promoted.applied);
        assert!(promoted.already_applied);

        assert_eq!(
            canary_impact
                .record_observations(&[canary_observation(
                    "post-promotion".into(),
                    Some("post-task".into()),
                    ExperienceOutcome::Failure,
                    900,
                    500,
                    &promoted,
                    true,
                )])
                .unwrap(),
            1
        );
        assert_eq!(
            canary_impact
                .credit_task_outcome("post-task", KnowledgeTaskOutcome::RolledBack, 501)
                .unwrap(),
            1
        );
        assert_eq!(
            evaluate_evolved_skill_rollbacks(&store, &canary_impact, 502).unwrap(),
            (1, 1)
        );
        assert_eq!(
            store.get(&candidate.id).unwrap().unwrap().status,
            KnowledgeStatus::Rejected
        );
        assert!(engine
            .decide(
                "project",
                "release-helper",
                &"a".repeat(64),
                None,
                "after-rollback",
            )
            .unwrap()
            .is_none());
    }

    #[test]
    fn evolved_skill_promotion_marker_write_failure_restores_validated_status() {
        let root = tempdir().unwrap();
        let store = KnowledgeStore::new(root.path());
        let stored = store
            .upsert(evolved_skill(KnowledgeStatus::Validated))
            .unwrap();
        let canary = CanaryImpactStore::new(root.path());
        let engine = EvolvedSkillCanaryEngine::new(store.clone(), canary.clone());
        let decision = engine
            .decide(
                "project",
                "release-helper",
                &"a".repeat(64),
                None,
                "promotion-write-failure",
            )
            .unwrap()
            .unwrap();
        let mut observations = Vec::new();
        for index in 0..10 {
            observations.push(canary_observation(
                format!("promotion-control-{index}"),
                None,
                if index < 4 {
                    ExperienceOutcome::Failure
                } else {
                    ExperienceOutcome::Success
                },
                1_000,
                100 + index,
                &decision,
                false,
            ));
        }
        for index in 0..20 {
            observations.push(canary_observation(
                format!("promotion-intervention-{index}"),
                (index < 3).then(|| format!("promotion-task-{index}")),
                if index < 2 {
                    ExperienceOutcome::Failure
                } else {
                    ExperienceOutcome::Success
                },
                900,
                200 + index,
                &decision,
                true,
            ));
        }
        canary.record_observations(&observations).unwrap();
        for index in 0..3 {
            canary
                .credit_task_outcome(
                    &format!("promotion-task-{index}"),
                    KnowledgeTaskOutcome::VerifiedSuccess,
                    300 + index,
                )
                .unwrap();
        }

        std::fs::create_dir(root.path().join("canary-impact.json.tmp")).unwrap();
        assert!(evaluate_evolved_skill_promotions(&store, &canary, 400).is_err());
        assert_eq!(
            store.get(&stored.id).unwrap().unwrap().status,
            KnowledgeStatus::Validated
        );
    }

    #[test]
    fn evolved_skill_rollback_marker_write_failure_restores_original_status() {
        let root = tempdir().unwrap();
        let store = KnowledgeStore::new(root.path());
        let stored = store
            .upsert(evolved_skill(KnowledgeStatus::Validated))
            .unwrap();
        let canary = CanaryImpactStore::new(root.path());
        let engine = EvolvedSkillCanaryEngine::new(store.clone(), canary.clone());
        let decision = engine
            .decide(
                "project",
                "release-helper",
                &"a".repeat(64),
                None,
                "rollback-write-failure",
            )
            .unwrap()
            .unwrap();
        canary
            .record_observations(&[canary_observation(
                "rollback-event".into(),
                Some("rollback-task".into()),
                ExperienceOutcome::Success,
                100,
                10,
                &decision,
                true,
            )])
            .unwrap();
        canary
            .credit_task_outcome("rollback-task", KnowledgeTaskOutcome::RolledBack, 11)
            .unwrap();

        std::fs::create_dir(root.path().join("canary-impact.json.tmp")).unwrap();
        assert!(evaluate_evolved_skill_rollbacks(&store, &canary, 12).is_err());
        assert_eq!(
            store.get(&stored.id).unwrap().unwrap().status,
            KnowledgeStatus::Validated
        );
    }
}
