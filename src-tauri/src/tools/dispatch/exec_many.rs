use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::tools::context::{SharedToolContext, ToolContext};
use crate::tools::execution_timeout::{configured_job_timeout_max_ms, resolve_process_timeout};
use crate::tools::parallel_stats::{
    parallel_pair_history, parallel_safety_lower_bound, record_parallel_observations,
    ParallelPairStats,
};
use crate::tools::redaction::OutputRedactionContext;
use crate::tools::session;
use crate::tools::workspace::{tool_err, tool_ok, WorkspaceError};
use crate::tools::ABSOLUTE_COMMAND_TIMEOUT_MAX_MS;

use super::{
    admission_error, attach_admission_metadata, call_exec_tool_async, call_tool_inner,
    ToolCallExecutionContext, ADMISSION_TIMEOUT,
};

mod retained;
use retained::RetainedExecManyGraph;

const PARALLEL_MIN_CONFIDENT_SAMPLES: u64 = 5;
const PARALLEL_SAFE_LOWER_BOUND: f64 = 0.70;
const MAX_PARALLEL_OBSERVATIONS: usize = 128;
const IO_HEAVY_ADMISSION_LIMIT: usize = 1;

fn io_heavy_admission() -> Arc<tokio::sync::Semaphore> {
    static ADMISSION: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    ADMISSION
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(IO_HEAVY_ADMISSION_LIMIT)))
        .clone()
}

fn is_io_heavy(arguments: &Value) -> bool {
    arguments.get("resource_class").and_then(Value::as_str) == Some("io_heavy")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParallelPrior {
    Safe,
    EvidenceRequired,
    Unsafe,
}

#[derive(Clone, Debug)]
pub(super) struct ParallelDecision {
    pub(super) mode: &'static str,
    pub(super) source: &'static str,
    confidence: f64,
    pub(super) history_samples: u64,
    blocked_pairs: usize,
    recommended_max_parallel: usize,
    reasons: Vec<String>,
}

#[derive(Clone)]
pub(super) struct ExecBatchCommand {
    index: usize,
    id: String,
    depends_on: Vec<String>,
    run_if: String,
    pub(super) lock_group: Option<String>,
    pub(super) lock_group_inferred: bool,
    pub(super) parallel_signature: String,
    parallel_prior: ParallelPrior,
    args: Value,
}

pub(super) async fn call_exec_many_async(ctx: SharedToolContext, args: &Value) -> Value {
    retained::call_exec_many_retained_async(ctx, args).await
}

async fn execute_exec_many_async(
    ctx: SharedToolContext,
    args: &Value,
    retained: Option<Arc<RetainedExecManyGraph>>,
) -> Value {
    let Some(commands) = args.get("commands").and_then(Value::as_array) else {
        return tool_err(WorkspaceError::invalid_argument("commands is required"));
    };
    let commands = match parse_exec_batch_commands(commands) {
        Ok(commands) => commands,
        Err(error) => return tool_err(error),
    };
    if let Err(error) = validate_exec_batch_timeout_contracts(&commands) {
        return tool_err(error);
    }
    let requested_mode = match exec_many_mode(args) {
        Ok(mode) => mode,
        Err(error) => return tool_err(error),
    };
    let requested_result_mode = match exec_many_result_mode(args) {
        Ok(mode) => mode,
        Err(error) => return tool_err(error),
    };
    let stop_on_error = args
        .get("stop_on_error")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let process_limit = ctx.execution_limits().process_admission.clamp(1, 256);
    let default_parallel = default_exec_many_parallelism(commands.len(), process_limit);
    let decision =
        resolve_exec_many_decision(&ctx.profile_id, requested_mode, &commands, default_parallel);
    let mode = decision.mode;
    if mode != "dag" && commands.iter().any(|command| command.run_if != "success") {
        return tool_err(WorkspaceError::invalid_argument(
            "run_if=failure or run_if=always requires dag mode (or auto with dependencies)",
        ));
    }
    if mode == "dag" {
        if let Err(error) = validate_exec_batch_dag(&commands) {
            return tool_err(error);
        }
    }
    let configured_parallel = args
        .get("max_parallel")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(default_parallel)
        .clamp(1, process_limit);
    let max_parallel = match mode {
        "sequential" => 1,
        _ if requested_mode == "auto" => configured_parallel
            .min(decision.recommended_max_parallel)
            .max(1),
        _ => configured_parallel,
    };
    let inferred_lock_groups = commands
        .iter()
        .filter(|command| command.lock_group_inferred)
        .count();
    let started = Instant::now();
    let mut warnings = Vec::<String>::new();
    if requested_mode == "auto" {
        warnings.push(format!(
            "auto scheduler selected {mode}; source={}; confidence={:.3}; samples={}; max_parallel={max_parallel}; inferred_lock_groups={inferred_lock_groups}",
            decision.source,
            decision.confidence,
            decision.history_samples,
        ));
        warnings.extend(decision.reasons.iter().take(8).cloned());
    }

    let results = match mode {
        "sequential" => {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
            let mut results = Vec::with_capacity(commands.len());
            for command in commands.iter().cloned() {
                let result = run_exec_batch_command(
                    ctx.clone(),
                    command,
                    semaphore.clone(),
                    retained.clone(),
                )
                .await;
                let command_ok = result.get("command_ok").and_then(Value::as_bool) == Some(true);
                results.push(result);
                if stop_on_error && !command_ok {
                    break;
                }
            }
            results
        }
        "parallel" => {
            if stop_on_error {
                warnings.push(
                    "parallel mode schedules independent commands immediately; stop_on_error cannot cancel commands that already started".into(),
                );
            }
            run_exec_batch_wave(
                ctx.clone(),
                commands.clone(),
                max_parallel,
                retained.clone(),
            )
            .await
        }
        "dag" => {
            run_exec_batch_dag(
                ctx.clone(),
                commands.clone(),
                max_parallel,
                stop_on_error,
                retained.clone(),
            )
            .await
        }
        _ => unreachable!("validated exec_many mode"),
    };

    let (parallelism_observations, observations_truncated) =
        collect_parallelism_observations(&commands, &results, mode);
    record_parallel_observations(&ctx.profile_id, &parallelism_observations);
    let observation_count = parallelism_observations.len();
    let mut output = exec_many_output(
        ctx.as_ref(),
        mode,
        max_parallel,
        stop_on_error,
        commands.len(),
        results,
        requested_result_mode,
        started,
        warnings,
        "async_batch",
    );
    if let Some(object) = output.as_object_mut() {
        object.insert("requested_mode".into(), json!(requested_mode));
        object.insert("auto_selected".into(), json!(requested_mode == "auto"));
        object.insert(
            "inferred_lock_group_count".into(),
            json!(inferred_lock_groups),
        );
        object.insert("parallel_decision_source".into(), json!(decision.source));
        object.insert(
            "parallel_confidence".into(),
            json!(round_parallel_confidence(decision.confidence)),
        );
        object.insert(
            "parallel_history_samples".into(),
            json!(decision.history_samples),
        );
        object.insert(
            "parallel_blocked_pair_count".into(),
            json!(decision.blocked_pairs),
        );
        object.insert(
            "recommended_max_parallel".into(),
            json!(decision.recommended_max_parallel),
        );
        object.insert("parallel_decision_reasons".into(), json!(decision.reasons));
        object.insert(
            "parallel_observation_count".into(),
            json!(observation_count),
        );
        object.insert(
            "parallelism_observation_truncated".into(),
            json!(observations_truncated),
        );
        object.insert(
            "parallelism_observations".into(),
            Value::Array(parallelism_observations),
        );
    }
    output
}

pub(super) fn call_exec_many_sync(ctx: &ToolContext, args: &Value) -> Value {
    let requested_mode = match exec_many_mode(args) {
        Ok(mode) => mode,
        Err(error) => return tool_err(error),
    };
    let requested_result_mode = match exec_many_result_mode(args) {
        Ok(mode) => mode,
        Err(error) => return tool_err(error),
    };
    if !matches!(requested_mode, "auto" | "sequential") {
        return tool_err(WorkspaceError::invalid_argument(
            "parallel and dag exec_many modes require the async MCP/Actions execution path",
        ));
    }
    let mode = "sequential";
    let Some(commands) = args.get("commands").and_then(Value::as_array) else {
        return tool_err(WorkspaceError::invalid_argument("commands is required"));
    };
    let commands = match parse_exec_batch_commands(commands) {
        Ok(commands) => commands,
        Err(error) => return tool_err(error),
    };
    if let Err(error) = validate_exec_batch_timeout_contracts(&commands) {
        return tool_err(error);
    }
    let stop_on_error = args
        .get("stop_on_error")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let started = Instant::now();
    let mut results = Vec::with_capacity(commands.len());
    for command in commands.iter().cloned() {
        let mut result = call_tool_inner(
            ctx,
            "exec_command",
            &command.args,
            false,
            &ToolCallExecutionContext::default(),
        );
        while result.get("process_still_running").and_then(Value::as_bool) == Some(true) {
            let Some(session_id) = result
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                break;
            };
            let cursor = result
                .get("next_cursor")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            result = match session::wait_command(
                &ctx.sessions,
                &json!({
                    "session_id": session_id,
                    "cursor": cursor,
                    "timeout_ms": session::WAIT_COMMAND_TIMEOUT_DEFAULT_MS,
                    "until": "finalized",
                    "output_mode": "tail"
                }),
            ) {
                Ok(value) => value,
                Err(error) => tool_err(error),
            };
        }
        let command_ok = result.get("command_ok").and_then(Value::as_bool) == Some(true);
        results.push(batch_result(
            &command, result, command_ok, false, None, 0, 0,
        ));
        if stop_on_error && !command_ok {
            break;
        }
    }
    let mut output = exec_many_output(
        ctx,
        mode,
        1,
        stop_on_error,
        commands.len(),
        results,
        requested_result_mode,
        started,
        if requested_mode == "auto" {
            vec!["auto scheduler fell back to sequential on the blocking execution path".into()]
        } else {
            Vec::new()
        },
        "blocking_batch",
    );
    if let Some(object) = output.as_object_mut() {
        object.insert("requested_mode".into(), json!(requested_mode));
        object.insert("auto_selected".into(), json!(requested_mode == "auto"));
        object.insert(
            "parallel_decision_source".into(),
            json!("blocking_execution_path"),
        );
        object.insert("parallel_confidence".into(), json!(1.0));
        object.insert("parallel_history_samples".into(), json!(0));
        object.insert("parallel_blocked_pair_count".into(), json!(0));
        object.insert("recommended_max_parallel".into(), json!(1));
        object.insert(
            "parallel_decision_reasons".into(),
            json!(["The blocking execution path only supports sequential exec_many scheduling."]),
        );
        object.insert("parallel_observation_count".into(), json!(0));
        object.insert("parallelism_observation_truncated".into(), json!(false));
        object.insert("parallelism_observations".into(), json!([]));
    }
    output
}

fn exec_many_mode(args: &Value) -> Result<&str, WorkspaceError> {
    let mode = args.get("mode").and_then(Value::as_str).unwrap_or("auto");
    if matches!(mode, "auto" | "sequential" | "parallel" | "dag") {
        Ok(mode)
    } else {
        Err(WorkspaceError::invalid_argument(
            "mode must be auto, sequential, parallel, or dag",
        ))
    }
}

fn exec_many_result_mode(args: &Value) -> Result<Option<&str>, WorkspaceError> {
    let Some(mode) = args.get("result_mode").and_then(Value::as_str) else {
        return Ok(None);
    };
    if matches!(mode, "full" | "summary" | "none") {
        Ok(Some(mode))
    } else {
        Err(WorkspaceError::invalid_argument(
            "exec_many result_mode must be full, summary, or none",
        ))
    }
}

pub(super) fn default_exec_many_parallelism(command_count: usize, process_limit: usize) -> usize {
    command_count.min(process_limit).clamp(1, 8)
}

fn resolve_exec_many_decision(
    profile_id: &str,
    requested: &str,
    commands: &[ExecBatchCommand],
    default_parallel: usize,
) -> ParallelDecision {
    let pairs = parallel_command_pairs(commands);
    let pair_keys = pairs
        .iter()
        .map(|(pair, _)| pair.clone())
        .collect::<Vec<_>>();
    let history = parallel_pair_history(profile_id, &pair_keys);
    resolve_exec_many_decision_with_history(requested, commands, default_parallel, &history)
}

pub(super) fn resolve_exec_many_decision_with_history(
    requested: &str,
    commands: &[ExecBatchCommand],
    default_parallel: usize,
    history: &BTreeMap<String, ParallelPairStats>,
) -> ParallelDecision {
    if requested != "auto" {
        let mode = match requested {
            "parallel" => "parallel",
            "dag" => "dag",
            _ => "sequential",
        };
        return ParallelDecision {
            mode,
            source: "explicit",
            confidence: 1.0,
            history_samples: 0,
            blocked_pairs: 0,
            recommended_max_parallel: if mode == "sequential" {
                1
            } else {
                default_parallel
            },
            reasons: vec!["The caller explicitly selected the execution mode.".into()],
        };
    }
    if commands
        .iter()
        .any(|command| !command.depends_on.is_empty())
    {
        return ParallelDecision {
            mode: "dag",
            source: "dependency_graph",
            confidence: 1.0,
            history_samples: 0,
            blocked_pairs: 0,
            recommended_max_parallel: default_parallel,
            reasons: vec!["Command dependencies require DAG scheduling.".into()],
        };
    }
    if commands.len() <= 1 {
        return ParallelDecision {
            mode: "sequential",
            source: "single_command",
            confidence: 1.0,
            history_samples: 0,
            blocked_pairs: 0,
            recommended_max_parallel: 1,
            reasons: vec!["Only one command was supplied.".into()],
        };
    }
    let unsafe_commands = commands
        .iter()
        .filter(|command| command.parallel_prior == ParallelPrior::Unsafe)
        .count();
    if unsafe_commands > 0 {
        return ParallelDecision {
            mode: "sequential",
            source: "hard_safety_rule",
            confidence: 1.0,
            history_samples: 0,
            blocked_pairs: 0,
            recommended_max_parallel: 1,
            reasons: vec![format!(
                "{unsafe_commands} opaque or shell command(s) cannot be proven safe for automatic parallel execution."
            )],
        };
    }

    let pairs = parallel_command_pairs(commands);
    let mut confidence = 0.75_f64;
    let mut history_samples = 0_u64;
    let mut blocked_pairs = 0_usize;
    let mut conflict_blocks = 0_usize;
    let mut serialization_pairs = 0_usize;
    let mut reasons = Vec::<String>::new();

    for (pair, requires_evidence) in &pairs {
        let stats = history.get(pair);
        let mut pair_blocked = false;
        if let Some(stats) = stats {
            let samples = stats.safety_samples();
            history_samples = history_samples.saturating_add(samples);
            if samples > 0 {
                confidence = confidence.min(parallel_safety_lower_bound(stats));
            }
            let conflict_prone =
                stats.conflicts >= 2 || (samples >= 3 && stats.conflict_rate() >= 0.10);
            if conflict_prone {
                conflict_blocks += 1;
                pair_blocked = true;
                reasons.push(format!(
                    "Historical conflicts block pair {pair}: conflicts={}, safety_samples={}.",
                    stats.conflicts, samples
                ));
            }
            if stats.attempts >= 3 && stats.serialization_rate() >= 0.60 {
                serialization_pairs += 1;
            }
            if *requires_evidence
                && (samples < PARALLEL_MIN_CONFIDENT_SAMPLES
                    || parallel_safety_lower_bound(stats) < PARALLEL_SAFE_LOWER_BOUND)
            {
                pair_blocked = true;
                reasons.push(format!(
                    "Pair {pair} lacks sufficient safe evidence: samples={samples}, lower_bound={:.3}.",
                    parallel_safety_lower_bound(stats)
                ));
            }
        } else if *requires_evidence {
            pair_blocked = true;
            confidence = 0.0;
            reasons.push(format!(
                "Pair {pair} has no parallel safety history; run it explicitly in parallel to collect evidence."
            ));
        }
        if pair_blocked {
            blocked_pairs += 1;
        }
    }

    if blocked_pairs > 0 {
        return ParallelDecision {
            mode: "sequential",
            source: if conflict_blocks > 0 {
                "historical_conflict"
            } else {
                "insufficient_history"
            },
            confidence,
            history_samples,
            blocked_pairs,
            recommended_max_parallel: 1,
            reasons,
        };
    }
    if !pairs.is_empty() && serialization_pairs == pairs.len() {
        reasons.push(
            "All observed command pairs mostly serialize on resource locks; parallel dispatch adds no useful overlap."
                .into(),
        );
        return ParallelDecision {
            mode: "sequential",
            source: "historical_serialization",
            confidence,
            history_samples,
            blocked_pairs: 0,
            recommended_max_parallel: 1,
            reasons,
        };
    }

    let recommended_max_parallel = if serialization_pairs > 0 {
        reasons.push(format!(
            "{serialization_pairs} pair(s) usually serialize on resource locks; cap automatic parallelism at 2."
        ));
        default_parallel.min(2)
    } else {
        default_parallel
    };
    if history_samples == 0 {
        reasons.push(
            "Known safe command families and hard resource locks permit parallel execution.".into(),
        );
    } else {
        reasons.push(format!(
            "Historical evidence supports parallel execution with an 80% Wilson lower bound of {:.3}.",
            confidence
        ));
    }
    ParallelDecision {
        mode: "parallel",
        source: if history_samples == 0 {
            "hard_rules"
        } else {
            "historical_statistics"
        },
        confidence,
        history_samples,
        blocked_pairs: 0,
        recommended_max_parallel,
        reasons,
    }
}

fn parallel_command_pairs(commands: &[ExecBatchCommand]) -> Vec<(String, bool)> {
    let mut pairs = Vec::new();
    for left in 0..commands.len() {
        for right in (left + 1)..commands.len() {
            let requires_evidence = commands[left].parallel_prior
                == ParallelPrior::EvidenceRequired
                || commands[right].parallel_prior == ParallelPrior::EvidenceRequired;
            pairs.push((
                parallel_pair_key(
                    &commands[left].parallel_signature,
                    &commands[right].parallel_signature,
                ),
                requires_evidence,
            ));
        }
    }
    pairs
}

pub(super) fn parallel_pair_key(left: &str, right: &str) -> String {
    if left <= right {
        format!("{left}|{right}")
    } else {
        format!("{right}|{left}")
    }
}

fn command_parallel_prior(arguments: &Value, lock_group: Option<&str>) -> ParallelPrior {
    let Some(program) = normalized_program(arguments) else {
        return ParallelPrior::Unsafe;
    };
    if arguments
        .get("shell")
        .and_then(Value::as_str)
        .is_some_and(|shell| shell != "none")
    {
        return ParallelPrior::Unsafe;
    }
    if lock_group.is_some() || command_has_version_flag(arguments) {
        return ParallelPrior::Safe;
    }
    let verb = normalized_command_verb(arguments, &program);
    if program == "git"
        && matches!(
            verb.as_str(),
            "status" | "diff" | "log" | "show" | "rev-parse" | "ls-files" | "cat-file"
        )
    {
        ParallelPrior::Safe
    } else {
        ParallelPrior::EvidenceRequired
    }
}

pub(super) fn command_parallel_signature(arguments: &Value) -> String {
    let program = normalized_program(arguments).unwrap_or_else(|| "opaque".into());
    let verb = normalized_command_verb(arguments, &program);
    let workdir = arguments
        .get("workdir")
        .and_then(Value::as_str)
        .unwrap_or(".");
    let digest = format!("{:x}", Sha256::digest(workdir.as_bytes()));
    format!("{program}:{verb}@{}", &digest[..12])
}

fn normalized_program(arguments: &Value) -> Option<String> {
    let value = arguments
        .get("program")
        .and_then(Value::as_str)
        .and_then(|program| Path::new(program).file_stem())
        .and_then(|name| name.to_str())?;
    let normalized = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(64)
        .collect::<String>()
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }
    if matches!(
        normalized.as_str(),
        "cargo"
            | "rustc"
            | "git"
            | "npm"
            | "pnpm"
            | "yarn"
            | "bun"
            | "python"
            | "python3"
            | "node"
            | "pwsh"
            | "powershell"
            | "sh"
            | "cmd"
    ) {
        return Some(normalized);
    }
    let digest = format!("{:x}", Sha256::digest(normalized.as_bytes()));
    Some(format!("custom-{}", &digest[..12]))
}

fn normalized_command_verb(arguments: &Value, program: &str) -> String {
    if command_has_version_flag(arguments) {
        return "version".into();
    }
    let first = arguments
        .get("args")
        .and_then(Value::as_array)
        .and_then(|args| args.iter().find_map(Value::as_str))
        .unwrap_or("");
    let known_family = matches!(
        program,
        "cargo" | "rustc" | "git" | "npm" | "pnpm" | "yarn" | "bun"
    );
    if !known_family {
        return if matches!(
            program,
            "python" | "python3" | "node" | "pwsh" | "powershell" | "sh" | "cmd"
        ) {
            "script".into()
        } else {
            "default".into()
        };
    }
    let normalized = first
        .trim_start_matches('-')
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .take(32)
        .collect::<String>()
        .to_ascii_lowercase();
    if normalized.is_empty() {
        "default".into()
    } else {
        normalized
    }
}

fn command_has_version_flag(arguments: &Value) -> bool {
    arguments
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .any(|argument| matches!(argument, "--version" | "-V" | "-v" | "version"))
        })
}

fn round_parallel_confidence(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}
pub(super) fn collect_parallelism_observations(
    commands: &[ExecBatchCommand],
    results: &[Value],
    mode: &str,
) -> (Vec<Value>, bool) {
    if mode != "parallel" || commands.len() < 2 {
        return (Vec::new(), false);
    }
    let result_by_id = results
        .iter()
        .filter_map(|result| {
            result
                .get("id")
                .and_then(Value::as_str)
                .map(|id| (id, result))
        })
        .collect::<HashMap<_, _>>();
    let total_pairs = commands
        .len()
        .saturating_mul(commands.len().saturating_sub(1))
        / 2;
    let mut observations = Vec::with_capacity(total_pairs.min(MAX_PARALLEL_OBSERVATIONS));
    for left in 0..commands.len() {
        for right in (left + 1)..commands.len() {
            if observations.len() == MAX_PARALLEL_OBSERVATIONS {
                return (observations, true);
            }
            let Some(left_result) = result_by_id.get(commands[left].id.as_str()) else {
                continue;
            };
            let Some(right_result) = result_by_id.get(commands[right].id.as_str()) else {
                continue;
            };
            let overlap_ms = execution_overlap_ms(left_result, right_result);
            let lock_wait_ms =
                result_lock_wait_ms(left_result).saturating_add(result_lock_wait_ms(right_result));
            let same_lock_group = commands[left].lock_group.is_some()
                && commands[left].lock_group == commands[right].lock_group;
            let left_ok = left_result.get("command_ok").and_then(Value::as_bool) == Some(true);
            let right_ok = right_result.get("command_ok").and_then(Value::as_bool) == Some(true);
            let outcome = if overlap_ms == 0 && (same_lock_group || lock_wait_ms > 0) {
                "serialized"
            } else if overlap_ms > 0
                && (result_has_conflict_marker(left_result)
                    || result_has_conflict_marker(right_result))
            {
                "conflict"
            } else if overlap_ms > 0 && left_ok && right_ok {
                "success"
            } else if overlap_ms > 0 {
                "failure"
            } else {
                "not_overlapped"
            };
            observations.push(json!({
                "pair": parallel_pair_key(
                    &commands[left].parallel_signature,
                    &commands[right].parallel_signature,
                ),
                "left": commands[left].parallel_signature,
                "right": commands[right].parallel_signature,
                "outcome": outcome,
                "overlap_ms": overlap_ms,
                "lock_wait_ms": lock_wait_ms,
                "same_lock_group": same_lock_group
            }));
        }
    }
    (observations, total_pairs > MAX_PARALLEL_OBSERVATIONS)
}

fn execution_overlap_ms(left: &Value, right: &Value) -> u64 {
    let Some((left_start, left_end)) = execution_interval(left) else {
        return 0;
    };
    let Some((right_start, right_end)) = execution_interval(right) else {
        return 0;
    };
    left_end
        .min(right_end)
        .saturating_sub(left_start.max(right_start))
}

fn execution_interval(batch_result: &Value) -> Option<(u64, u64)> {
    let result = batch_result.get("result")?;
    let start = result.get("started_ts_ms").and_then(Value::as_u64)?;
    let end = result
        .get("completed_ts_ms")
        .and_then(Value::as_u64)
        .or_else(|| {
            result
                .get("elapsed_ms")
                .or_else(|| result.get("duration_ms"))
                .and_then(Value::as_u64)
                .map(|duration| start.saturating_add(duration))
        })?;
    Some((start, end.max(start)))
}

fn result_lock_wait_ms(batch_result: &Value) -> u64 {
    batch_result
        .get("resource_lock_wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn result_has_conflict_marker(batch_result: &Value) -> bool {
    let text = serde_json::to_string(batch_result.get("result").unwrap_or(&Value::Null))
        .unwrap_or_default()
        .to_ascii_lowercase();
    [
        "resource busy",
        "file in use",
        "sharing violation",
        "index.lock",
        "another git process",
        "could not lock",
        "database is locked",
        "lock wait timeout",
        "text file busy",
        "ebusy",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn inferred_exec_lock_group(arguments: &Value) -> Option<String> {
    let program = arguments
        .get("program")
        .and_then(Value::as_str)
        .and_then(|program| Path::new(program).file_stem())
        .and_then(|name| name.to_str())?
        .to_ascii_lowercase();
    let verb = arguments
        .get("args")
        .and_then(Value::as_array)
        .and_then(|args| args.iter().find_map(Value::as_str))
        .unwrap_or("")
        .trim_start_matches('-')
        .to_ascii_lowercase();
    let workdir = arguments
        .get("workdir")
        .and_then(Value::as_str)
        .unwrap_or(".");
    let scope = workdir
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .take(80)
        .collect::<String>();
    let group = match program.as_str() {
        "cargo" | "rustc" => format!("cargo-target:{scope}"),
        "git"
            if matches!(
                verb.as_str(),
                "add"
                    | "am"
                    | "apply"
                    | "bisect"
                    | "branch"
                    | "checkout"
                    | "cherry-pick"
                    | "clean"
                    | "commit"
                    | "merge"
                    | "mv"
                    | "rebase"
                    | "reset"
                    | "restore"
                    | "revert"
                    | "rm"
                    | "stash"
                    | "switch"
                    | "tag"
            ) =>
        {
            format!("git-index:{scope}")
        }
        "npm" | "pnpm" | "yarn" | "bun"
            if matches!(
                verb.as_str(),
                "add" | "ci" | "dedupe" | "install" | "remove" | "uninstall" | "update"
            ) =>
        {
            format!("node-generated:{scope}")
        }
        _ => return None,
    };
    Some(group)
}

pub(super) fn parse_exec_batch_commands(
    commands: &[Value],
) -> Result<Vec<ExecBatchCommand>, WorkspaceError> {
    if commands.is_empty() || commands.len() > 256 {
        return Err(WorkspaceError::invalid_argument(
            "commands must contain between 1 and 256 entries",
        ));
    }
    let mut parsed = Vec::with_capacity(commands.len());
    let mut ids = HashSet::with_capacity(commands.len());
    for (index, command) in commands.iter().enumerate() {
        let Some(_) = command.as_object() else {
            return Err(WorkspaceError::invalid_argument(format!(
                "commands[{index}] must be an object"
            )));
        };
        let mut command_args = command.clone();
        let object = command_args
            .as_object_mut()
            .expect("validated command object");
        let id = match object.remove("id") {
            None => format!("command-{index}"),
            Some(Value::String(value)) => value,
            Some(_) => {
                return Err(WorkspaceError::invalid_argument(format!(
                    "commands[{index}].id must be a string"
                )))
            }
        };
        if id.trim().is_empty() || id.len() > 128 {
            return Err(WorkspaceError::invalid_argument(format!(
                "commands[{index}].id must contain between 1 and 128 bytes"
            )));
        }
        if !ids.insert(id.clone()) {
            return Err(WorkspaceError::invalid_argument(format!(
                "duplicate exec_many command id: {id}"
            )));
        }
        let depends_on = match object.remove("depends_on") {
            None => Vec::new(),
            Some(Value::Array(values)) => values
                .into_iter()
                .map(|value| {
                    value.as_str().map(str::to_string).ok_or_else(|| {
                        WorkspaceError::invalid_argument(format!(
                            "commands[{index}].depends_on must contain strings"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => {
                return Err(WorkspaceError::invalid_argument(format!(
                    "commands[{index}].depends_on must be an array"
                )))
            }
        };
        let run_if = match object.remove("run_if") {
            None => "success".to_string(),
            Some(Value::String(value))
                if matches!(value.as_str(), "success" | "failure" | "always") =>
            {
                value
            }
            Some(Value::String(_)) => {
                return Err(WorkspaceError::invalid_argument(format!(
                    "commands[{index}].run_if must be success, failure, or always"
                )))
            }
            Some(_) => {
                return Err(WorkspaceError::invalid_argument(format!(
                    "commands[{index}].run_if must be a string"
                )))
            }
        };
        if matches!(run_if.as_str(), "failure" | "always") && depends_on.is_empty() {
            return Err(WorkspaceError::invalid_argument(format!(
                "commands[{index}].run_if={run_if} requires depends_on"
            )));
        }
        let explicit_lock_group = match object.remove("lock_group") {
            None => None,
            Some(Value::String(value)) => {
                let value = value.trim().to_string();
                if value.is_empty() || value.len() > 128 {
                    return Err(WorkspaceError::invalid_argument(format!(
                        "commands[{index}].lock_group must contain between 1 and 128 bytes"
                    )));
                }
                Some(value)
            }
            Some(_) => {
                return Err(WorkspaceError::invalid_argument(format!(
                    "commands[{index}].lock_group must be a string"
                )))
            }
        };
        let (lock_group, lock_group_inferred) = match explicit_lock_group {
            Some(group) => (Some(group), false),
            None => (
                inferred_exec_lock_group(&Value::Object(object.clone())),
                true,
            ),
        };
        let lock_group_inferred = lock_group_inferred && lock_group.is_some();
        if let Some(group) = lock_group.as_ref() {
            object.insert("lock_group".into(), Value::String(group.clone()));
        }
        if command_args.get("yield_time_ms").is_none() {
            command_args["yield_time_ms"] = json!(30_000);
        }
        if command_args.get("output_mode").is_none() {
            command_args["output_mode"] = json!("tail");
        }
        let parallel_signature = command_parallel_signature(&command_args);
        let parallel_prior = command_parallel_prior(&command_args, lock_group.as_deref());
        parsed.push(ExecBatchCommand {
            index,
            id,
            depends_on,
            run_if,
            lock_group,
            lock_group_inferred,
            parallel_signature,
            parallel_prior,
            args: command_args,
        });
    }
    let known_ids = parsed
        .iter()
        .map(|command| command.id.as_str())
        .collect::<HashSet<_>>();
    for command in &parsed {
        for dependency in &command.depends_on {
            if dependency == &command.id {
                return Err(WorkspaceError::invalid_argument(format!(
                    "command {} cannot depend on itself",
                    command.id
                )));
            }
            if !known_ids.contains(dependency.as_str()) {
                return Err(WorkspaceError::invalid_argument(format!(
                    "command {} depends on unknown command {}",
                    command.id, dependency
                )));
            }
        }
    }
    Ok(parsed)
}

fn validate_exec_batch_timeout_contracts(
    commands: &[ExecBatchCommand],
) -> Result<(), WorkspaceError> {
    let job_max_ms = configured_job_timeout_max_ms();
    for command in commands {
        resolve_process_timeout(
            &command.args,
            30_000,
            ABSOLUTE_COMMAND_TIMEOUT_MAX_MS,
            job_max_ms,
        )
        .map_err(|error| {
            WorkspaceError::invalid_argument(format!(
                "commands[{}] rejected before scheduling: {error}",
                command.index
            ))
        })?;
    }
    Ok(())
}

fn validate_exec_batch_dag(commands: &[ExecBatchCommand]) -> Result<(), WorkspaceError> {
    let mut indegree = commands
        .iter()
        .map(|command| (command.id.clone(), command.depends_on.len()))
        .collect::<HashMap<_, _>>();
    let mut dependents = HashMap::<String, Vec<String>>::new();
    for command in commands {
        for dependency in &command.depends_on {
            dependents
                .entry(dependency.clone())
                .or_default()
                .push(command.id.clone());
        }
    }
    let mut ready = indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(id.clone()))
        .collect::<Vec<_>>();
    let mut visited = 0_usize;
    while let Some(id) = ready.pop() {
        visited += 1;
        for dependent in dependents.get(&id).into_iter().flatten() {
            let degree = indegree
                .get_mut(dependent)
                .expect("validated dependent command");
            *degree = degree.saturating_sub(1);
            if *degree == 0 {
                ready.push(dependent.clone());
            }
        }
    }
    if visited == commands.len() {
        Ok(())
    } else {
        Err(WorkspaceError::invalid_argument(
            "exec_many DAG contains a dependency cycle",
        ))
    }
}

async fn run_exec_batch_dag(
    ctx: SharedToolContext,
    commands: Vec<ExecBatchCommand>,
    max_parallel: usize,
    stop_on_error: bool,
    retained: Option<Arc<RetainedExecManyGraph>>,
) -> Vec<Value> {
    let mut pending = commands;
    let mut completed = HashMap::<String, bool>::new();
    let mut results = Vec::new();
    let mut stop_on_error_triggered = false;
    while !pending.is_empty() {
        let mut ready = Vec::new();
        let mut remaining = Vec::new();
        for command in pending {
            let dependencies_resolved = command
                .depends_on
                .iter()
                .all(|dependency| completed.contains_key(dependency));
            if !dependencies_resolved {
                remaining.push(command);
                continue;
            }
            let dependency_failed = command
                .depends_on
                .iter()
                .any(|dependency| completed.get(dependency) == Some(&false));
            match command.run_if.as_str() {
                "success" if dependency_failed => {
                    completed.insert(command.id.clone(), false);
                    results.push(skipped_batch_result(command, "dependency_failed"));
                }
                "failure" if !dependency_failed => {
                    completed.insert(command.id.clone(), false);
                    results.push(conditional_skipped_batch_result(command));
                }
                "success" if stop_on_error_triggered => {
                    completed.insert(command.id.clone(), false);
                    results.push(skipped_batch_result(command, "stopped_after_failure"));
                }
                _ => ready.push(command),
            }
        }
        if ready.is_empty() {
            if remaining.is_empty() {
                break;
            }
            for command in remaining {
                completed.insert(command.id.clone(), false);
                results.push(skipped_batch_result(command, "dependency_unresolved"));
            }
            break;
        }
        let wave_results =
            run_exec_batch_wave(ctx.clone(), ready, max_parallel, retained.clone()).await;
        let wave_failed = wave_results
            .iter()
            .any(|result| result.get("command_ok").and_then(Value::as_bool) != Some(true));
        for result in &wave_results {
            if let Some(id) = result.get("id").and_then(Value::as_str) {
                completed.insert(
                    id.to_string(),
                    result.get("command_ok").and_then(Value::as_bool) == Some(true),
                );
            }
        }
        results.extend(wave_results);
        if stop_on_error && wave_failed {
            stop_on_error_triggered = true;
        }
        pending = remaining;
    }
    results.sort_by_key(|result| {
        result
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
    });
    results
}

async fn run_exec_batch_wave(
    ctx: SharedToolContext,
    commands: Vec<ExecBatchCommand>,
    max_parallel: usize,
    retained: Option<Arc<RetainedExecManyGraph>>,
) -> Vec<Value> {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_parallel.max(1)));
    let mut tasks = tokio::task::JoinSet::new();
    for command in commands {
        let ctx = ctx.clone();
        let semaphore = semaphore.clone();
        let retained = retained.clone();
        tasks.spawn(async move {
            let index = command.index;
            (
                index,
                run_exec_batch_command(ctx, command, semaphore, retained).await,
            )
        });
    }
    let mut results = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((_, result)) => results.push(result),
            Err(error) => results.push(json!({
                "index": u64::MAX,
                "id": "join-failure",
                "command_ok": false,
                "skipped": false,
                "result": tool_err(WorkspaceError::ToolDetails {
                    code: "BATCH_WORKER_FAILED",
                    message: error.to_string(),
                    category: "runtime",
                    retryable: true,
                    details: json!({"stage": "exec_many_join"})
                })
            })),
        }
    }
    results.sort_by_key(|result| {
        result
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
    });
    results
}

async fn run_exec_batch_command(
    ctx: SharedToolContext,
    command: ExecBatchCommand,
    semaphore: Arc<tokio::sync::Semaphore>,
    retained: Option<Arc<RetainedExecManyGraph>>,
) -> Value {
    let resource_lock_wait_ms = 0u128;
    let batch_started = Instant::now();
    let batch_permit = semaphore
        .acquire_owned()
        .await
        .expect("exec_many semaphore closed");
    let batch_queue_wait_ms = batch_started.elapsed().as_millis();
    if retained
        .as_ref()
        .map(|graph| graph.cancel_requested())
        .unwrap_or(false)
    {
        drop(batch_permit);
        return skipped_batch_result(command, "graph_cancelled");
    }
    let policy = ctx.runtime_config().policy.security_policy;
    let redaction = OutputRedactionContext::new_with_policy("exec_command", &command.args, &policy);
    let Some((
        admission_lane,
        admission_limit,
        admission,
        global_admission_limit,
        global_admission,
    )) = ctx.admission_for("exec_command")
    else {
        return batch_result(
            &command,
            tool_err(WorkspaceError::Tool {
                code: "EXEC_ADMISSION_UNAVAILABLE",
                message: "exec_command admission resources are unavailable".into(),
                category: "runtime",
                retryable: true,
            }),
            false,
            false,
            None,
            resource_lock_wait_ms,
            batch_queue_wait_ms,
        );
    };
    let admission_started = Instant::now();
    let workspace_started = Instant::now();
    let permit = match tokio::time::timeout(ADMISSION_TIMEOUT, admission.acquire_owned()).await {
        Ok(Ok(permit)) => permit,
        Ok(Err(error)) => {
            return batch_result(
                &command,
                admission_error(
                    admission_lane,
                    "workspace",
                    admission_limit,
                    workspace_started.elapsed().as_millis(),
                    0,
                    format!("Workspace tool admission lane closed: {error}"),
                ),
                false,
                false,
                None,
                resource_lock_wait_ms,
                batch_queue_wait_ms,
            )
        }
        Err(_) => {
            return batch_result(
                &command,
                admission_error(
                    admission_lane,
                    "workspace",
                    admission_limit,
                    workspace_started.elapsed().as_millis(),
                    0,
                    "Workspace tool admission queue exceeded 30 seconds".into(),
                ),
                false,
                false,
                None,
                resource_lock_wait_ms,
                batch_queue_wait_ms,
            )
        }
    };
    let workspace_admission_wait_ms = workspace_started.elapsed().as_millis();
    let remaining = ADMISSION_TIMEOUT.saturating_sub(admission_started.elapsed());
    let global_started = Instant::now();
    let global_permit =
        match tokio::time::timeout(remaining, global_admission.acquire_owned()).await {
            Ok(Ok(permit)) => permit,
            Ok(Err(error)) => {
                return batch_result(
                    &command,
                    admission_error(
                        admission_lane,
                        "global",
                        global_admission_limit,
                        workspace_admission_wait_ms,
                        global_started.elapsed().as_millis(),
                        format!("Global tool admission lane closed: {error}"),
                    ),
                    false,
                    false,
                    None,
                    resource_lock_wait_ms,
                    batch_queue_wait_ms,
                )
            }
            Err(_) => {
                return batch_result(
                    &command,
                    admission_error(
                        admission_lane,
                        "global",
                        global_admission_limit,
                        workspace_admission_wait_ms,
                        global_started.elapsed().as_millis(),
                        "Combined workspace/global admission queue exceeded 30 seconds".into(),
                    ),
                    false,
                    false,
                    None,
                    resource_lock_wait_ms,
                    batch_queue_wait_ms,
                )
            }
        };
    let global_admission_wait_ms = global_started.elapsed().as_millis();
    let admission_queue_wait_ms = admission_started.elapsed().as_millis();
    let io_heavy = is_io_heavy(&command.args);
    let io_admission_started = Instant::now();
    let io_heavy_permit = if io_heavy {
        let remaining = ADMISSION_TIMEOUT.saturating_sub(admission_started.elapsed());
        match tokio::time::timeout(remaining, io_heavy_admission().acquire_owned()).await {
            Ok(Ok(permit)) => Some(permit),
            Ok(Err(error)) => {
                return batch_result(
                    &command,
                    tool_err(WorkspaceError::Tool {
                        code: "IO_HEAVY_ADMISSION_UNAVAILABLE",
                        message: format!("Host-wide heavy I/O admission lane closed: {error}"),
                        category: "runtime",
                        retryable: true,
                    }),
                    false,
                    false,
                    None,
                    resource_lock_wait_ms,
                    batch_queue_wait_ms,
                );
            }
            Err(_) => {
                return batch_result(
                    &command,
                    tool_err(WorkspaceError::Tool {
                        code: "IO_HEAVY_ADMISSION_TIMEOUT",
                        message: "Host-wide heavy I/O admission queue exceeded the combined 30 second admission budget".into(),
                        category: "runtime",
                        retryable: true,
                    }),
                    false,
                    false,
                    None,
                    resource_lock_wait_ms,
                    batch_queue_wait_ms,
                );
            }
        }
    } else {
        None
    };
    let io_heavy_admission_wait_ms = if io_heavy {
        io_admission_started.elapsed().as_millis()
    } else {
        0
    };
    let mut exec_args = command.args.clone();
    if retained.is_some() {
        if let Some(object) = exec_args.as_object_mut() {
            object.insert("yield_time_ms".into(), json!(0));
        }
    }
    let mut result = call_exec_tool_async(ctx.as_ref(), "exec_command", &exec_args).await;
    if let Some(object) = result.as_object_mut() {
        object.insert("execution_lane".into(), json!("async_process"));
        object.insert("blocking_queue_wait_ms".into(), json!(0));
        attach_admission_metadata(
            object,
            admission_lane,
            admission_limit,
            global_admission_limit,
            workspace_admission_wait_ms,
            global_admission_wait_ms,
            admission_queue_wait_ms,
        );
        object.insert(
            "resource_class".into(),
            json!(if io_heavy { "io_heavy" } else { "default" }),
        );
        object.insert(
            "io_heavy_admission_limit".into(),
            json!(if io_heavy {
                IO_HEAVY_ADMISSION_LIMIT
            } else {
                0
            }),
        );
        object.insert(
            "io_heavy_admission_wait_ms".into(),
            json!(io_heavy_admission_wait_ms),
        );
    }
    let mut result = redaction.redact(result);
    let tracked_session_id = result
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let (Some(graph), Some(session_id)) = (retained.as_ref(), tracked_session_id.as_deref()) {
        if graph.register_session(session_id) || graph.cancel_requested() {
            result = match session::kill_session_async(
                &ctx.sessions,
                &json!({"session_id": session_id, "signal": "TERM", "wait_ms": 5_000}),
            )
            .await
            {
                Ok(value) => value,
                Err(error) => tool_err(error),
            };
        }
    }
    while result.get("process_still_running").and_then(Value::as_bool) == Some(true) {
        let Some(session_id) = result
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            break;
        };
        let cursor = result
            .get("next_cursor")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        result = match session::wait_command_async(
            &ctx.sessions,
            &json!({
                "session_id": session_id,
                "cursor": cursor,
                "timeout_ms": session::WAIT_COMMAND_TIMEOUT_DEFAULT_MS,
                "until": "finalized",
                "output_mode": "tail"
            }),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => tool_err(error),
        };
    }
    if let Some(object) = result.as_object_mut() {
        object.insert("execution_lane".into(), json!("async_process"));
        object.insert("blocking_queue_wait_ms".into(), json!(0));
        attach_admission_metadata(
            object,
            admission_lane,
            admission_limit,
            global_admission_limit,
            workspace_admission_wait_ms,
            global_admission_wait_ms,
            admission_queue_wait_ms,
        );
        object.insert(
            "resource_class".into(),
            json!(if io_heavy { "io_heavy" } else { "default" }),
        );
        object.insert(
            "io_heavy_admission_limit".into(),
            json!(if io_heavy {
                IO_HEAVY_ADMISSION_LIMIT
            } else {
                0
            }),
        );
        object.insert(
            "io_heavy_admission_wait_ms".into(),
            json!(io_heavy_admission_wait_ms),
        );
    }
    if let (Some(graph), Some(session_id)) = (retained.as_ref(), tracked_session_id.as_deref()) {
        graph.unregister_session(session_id);
    }
    drop(io_heavy_permit);
    drop(global_permit);
    drop(permit);
    drop(batch_permit);
    let resource_lock_wait_ms = result
        .get("resource_lock_wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u128;
    let command_ok = result.get("command_ok").and_then(Value::as_bool) == Some(true);
    batch_result(
        &command,
        result,
        command_ok,
        false,
        None,
        resource_lock_wait_ms,
        batch_queue_wait_ms,
    )
}

fn batch_result(
    command: &ExecBatchCommand,
    result: Value,
    command_ok: bool,
    skipped: bool,
    skip_reason: Option<&str>,
    resource_lock_wait_ms: u128,
    batch_queue_wait_ms: u128,
) -> Value {
    json!({
        "index": command.index,
        "id": command.id,
        "depends_on": command.depends_on,
        "run_if": command.run_if,
        "lock_group": command.lock_group,
        "command": command.args,
        "command_ok": command_ok,
        "skipped": skipped,
        "skip_reason": skip_reason,
        "resource_lock_wait_ms": resource_lock_wait_ms,
        "batch_queue_wait_ms": batch_queue_wait_ms,
        "result": result
    })
}

fn skipped_batch_result(command: ExecBatchCommand, reason: &str) -> Value {
    batch_result(
        &command,
        json!({
            "ok": true,
            "command_ok": false,
            "status": "skipped",
            "outcome_class": "skipped",
            "reason": reason
        }),
        false,
        true,
        Some(reason),
        0,
        0,
    )
}

fn conditional_skipped_batch_result(command: ExecBatchCommand) -> Value {
    let mut result = batch_result(
        &command,
        json!({
            "ok": true,
            "command_ok": null,
            "status": "skipped",
            "outcome_class": "condition_not_met",
            "reason": "run_condition_not_met"
        }),
        false,
        true,
        Some("run_condition_not_met"),
        0,
        0,
    );
    result["conditional_skip"] = json!(true);
    result
}

#[allow(clippy::too_many_arguments)]
fn batch_failure_summary(result: &Value) -> Value {
    let nested = result.get("result").unwrap_or(&Value::Null);
    json!({
        "id": result.get("id").cloned().unwrap_or(Value::Null),
        "index": result.get("index").cloned().unwrap_or(Value::Null),
        "status": nested.get("status").cloned().unwrap_or(Value::Null),
        "outcome_class": nested
            .get("outcome_class")
            .cloned()
            .unwrap_or(Value::Null),
        "error_code": nested
            .get("error")
            .and_then(|error| error.get("code"))
            .cloned()
            .unwrap_or(Value::Null),
        "process_exit_code": nested
            .get("process_exit_code")
            .or_else(|| nested.get("exit_code"))
            .cloned()
            .unwrap_or(Value::Null)
    })
}

const EXEC_MANY_AUTO_COMPACT_OUTPUT_BYTES: u64 = 16 * 1024;
const EXEC_MANY_FAILURE_SUMMARY_CHARS: usize = 2_048;

struct ExecManyResultShape {
    mode: &'static str,
    reason: &'static str,
    auto_compacted_output_bytes: u64,
    results_omitted_count: usize,
    results: Vec<Value>,
}

fn bounded_output_summary(value: Option<&Value>) -> Option<String> {
    let text = value.and_then(Value::as_str)?;
    if text.is_empty() {
        return None;
    }
    let chars = text.chars().collect::<Vec<_>>();
    let start = chars.len().saturating_sub(EXEC_MANY_FAILURE_SUMMARY_CHARS);
    Some(chars[start..].iter().collect())
}

fn compact_exec_many_result(batch: &Value) -> Value {
    let result = batch.get("result").unwrap_or(&Value::Null);
    let command_ok = batch.get("command_ok").and_then(Value::as_bool);
    let failed = command_ok == Some(false);
    json!({
        "index": batch.get("index").cloned().unwrap_or(Value::Null),
        "id": batch.get("id").cloned().unwrap_or(Value::Null),
        "command_ok": batch.get("command_ok").cloned().unwrap_or(Value::Null),
        "skipped": batch.get("skipped").cloned().unwrap_or(json!(false)),
        "skip_reason": batch.get("skip_reason").cloned().unwrap_or(Value::Null),
        "conditional_skip": batch.get("conditional_skip").cloned().unwrap_or(json!(false)),
        "run_if": batch.get("run_if").cloned().unwrap_or(json!("success")),
        "resource_lock_wait_ms": batch.get("resource_lock_wait_ms").cloned().unwrap_or(json!(0)),
        "batch_queue_wait_ms": batch.get("batch_queue_wait_ms").cloned().unwrap_or(json!(0)),
        "status": result.get("status").cloned().unwrap_or(Value::Null),
        "termination_reason": result.get("termination_reason").cloned().unwrap_or(Value::Null),
        "exit_code": result.get("exit_code").cloned().unwrap_or_else(|| result.get("process_exit_code").cloned().unwrap_or(Value::Null)),
        "process_exit_code": result.get("process_exit_code").cloned().unwrap_or(Value::Null),
        "execution_mode": result.get("execution_mode").cloned().unwrap_or(Value::Null),
        "requested_process_timeout_ms": result.get("requested_process_timeout_ms").cloned().unwrap_or(Value::Null),
        "effective_process_timeout_ms": result.get("effective_process_timeout_ms").cloned().unwrap_or(Value::Null),
        "process_timeout_limit_ms": result.get("process_timeout_limit_ms").cloned().unwrap_or(Value::Null),
        "process_deadline_ts_ms": result.get("process_deadline_ts_ms").cloned().unwrap_or(Value::Null),
        "process_timeout_remaining_ms": result.get("process_timeout_remaining_ms").cloned().unwrap_or(Value::Null),
        "timeout_clamped": result.get("timeout_clamped").cloned().unwrap_or(Value::Null),
        "polling_extends_process_deadline": result.get("polling_extends_process_deadline").cloned().unwrap_or(Value::Null),
        "timeout_scope": result.get("timeout_scope").cloned().unwrap_or(Value::Null),
        "elapsed_ms": result.get("elapsed_ms").cloned().unwrap_or(Value::Null),
        "stdout_bytes": result.get("stdout_bytes").cloned().unwrap_or(json!(0)),
        "stderr_bytes": result.get("stderr_bytes").cloned().unwrap_or(json!(0)),
        "output_refs": result.get("output_refs").cloned().unwrap_or(Value::Null),
        "error_code": result.get("error").and_then(|error| error.get("code")).cloned().unwrap_or(Value::Null),
        "error_message": result.get("error").and_then(|error| error.get("message")).cloned().unwrap_or(Value::Null),
        "stdout_summary": if failed { bounded_output_summary(result.get("stdout")) } else { None },
        "stderr_summary": if failed { bounded_output_summary(result.get("stderr")) } else { None }
    })
}

fn shape_exec_many_results(
    results: &[Value],
    requested_mode: Option<&str>,
) -> Result<ExecManyResultShape, WorkspaceError> {
    let total_output_bytes = results
        .iter()
        .map(|batch| {
            let result = batch.get("result").unwrap_or(&Value::Null);
            result
                .get("stdout_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + result
                    .get("stderr_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
        })
        .sum::<u64>();
    let (mode, reason, auto_compacted_output_bytes) = match requested_mode {
        Some("full") => ("full", "explicit", 0),
        Some("summary") => ("summary", "explicit", 0),
        Some("none") => ("none", "explicit", 0),
        Some(_) => {
            return Err(WorkspaceError::invalid_argument(
                "exec_many result_mode must be full, summary, or none",
            ))
        }
        None if total_output_bytes > EXEC_MANY_AUTO_COMPACT_OUTPUT_BYTES => {
            ("summary", "large_output", total_output_bytes)
        }
        None => ("full", "default", 0),
    };
    let shaped = match mode {
        "full" => results.to_vec(),
        "summary" => results.iter().map(compact_exec_many_result).collect(),
        "none" => Vec::new(),
        _ => unreachable!(),
    };
    Ok(ExecManyResultShape {
        mode,
        reason,
        auto_compacted_output_bytes,
        results_omitted_count: if mode == "none" { results.len() } else { 0 },
        results: shaped,
    })
}

fn exec_many_output(
    ctx: &ToolContext,
    mode: &str,
    max_parallel: usize,
    stop_on_error: bool,
    commands_requested: usize,
    mut results: Vec<Value>,
    requested_result_mode: Option<&str>,
    started: Instant,
    warnings: Vec<String>,
    execution_lane: &str,
) -> Value {
    results.sort_by_key(|result| {
        result
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
    });
    let explicit_skipped = results
        .iter()
        .filter(|result| result.get("skipped").and_then(Value::as_bool) == Some(true))
        .count();
    let commands_executed = results.len().saturating_sub(explicit_skipped);
    let implicit_skipped = commands_requested.saturating_sub(results.len());
    let skipped_command_count = explicit_skipped + implicit_skipped;
    let failed_command_ids = results
        .iter()
        .filter(|result| {
            result.get("skipped").and_then(Value::as_bool) != Some(true)
                && result.get("command_ok").and_then(Value::as_bool) != Some(true)
        })
        .filter_map(|result| result.get("id").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    let failed_command_count = failed_command_ids.len();
    let skipped_command_ids = results
        .iter()
        .filter(|result| result.get("skipped").and_then(Value::as_bool) == Some(true))
        .filter_map(|result| result.get("id").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    let conditional_skipped_command_ids = results
        .iter()
        .filter(|result| {
            result.get("conditional_skip").and_then(Value::as_bool) == Some(true)
                || result.get("skip_reason").and_then(Value::as_str)
                    == Some("run_condition_not_met")
        })
        .filter_map(|result| result.get("id").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    let conditional_skipped_command_count = conditional_skipped_command_ids.len();
    let blocking_skipped_command_count =
        skipped_command_count.saturating_sub(conditional_skipped_command_count);
    let successful_command_count = results
        .iter()
        .filter(|result| result.get("command_ok").and_then(Value::as_bool) == Some(true))
        .count();
    let first_failed_command = results
        .iter()
        .find(|result| {
            result.get("skipped").and_then(Value::as_bool) != Some(true)
                && result.get("command_ok").and_then(Value::as_bool) != Some(true)
        })
        .map(batch_failure_summary);
    let all_commands_ok = failed_command_count == 0
        && blocking_skipped_command_count == 0
        && successful_command_count + conditional_skipped_command_count == commands_requested;
    let outcome_class = if all_commands_ok {
        "success"
    } else if successful_command_count > 0 {
        "partial_failure"
    } else {
        "command_failed"
    };
    let (workspace_limit, global_limit) = ctx
        .admission_for("exec_command")
        .map(|(_, workspace, _, global, _)| (workspace, global))
        .unwrap_or((0, 0));
    let batch_summary = if all_commands_ok && conditional_skipped_command_count > 0 {
        format!(
            "{successful_command_count} succeeded, {conditional_skipped_command_count} conditionally skipped"
        )
    } else if all_commands_ok {
        format!("All {commands_requested} commands succeeded")
    } else {
        format!(
            "{failed_command_count} failed, {skipped_command_count} skipped, {successful_command_count} succeeded"
        )
    };
    let recovery_actions = if failed_command_ids.is_empty() {
        Vec::<Value>::new()
    } else {
        vec![
            json!({
                "action": "inspect_failed_commands",
                "command_ids": failed_command_ids.clone(),
                "reason": "exec_many_command_failure"
            }),
            json!({
                "action": "rerun_failed_commands",
                "tool": "exec_many",
                "command_ids": failed_command_ids,
                "required_arguments": ["commands"],
                "reason": "rerun_only_after_fixing_the_reported_failure"
            }),
        ]
    };
    let stopped_early = blocking_skipped_command_count > 0 || results.len() < commands_requested;
    let result_shape = shape_exec_many_results(&results, requested_result_mode)
        .expect("result_mode was validated before command execution");

    let mut output = tool_ok(json!({
        "mode": mode,
        "max_parallel": max_parallel,
        "commands_requested": commands_requested,
        "commands_executed": commands_executed,
        "successful_command_count": successful_command_count,
        "failed_command_count": failed_command_count,
        "failed_command_ids": failed_command_ids,
        "skipped_command_count": skipped_command_count,
        "skipped_command_ids": skipped_command_ids,
        "conditional_skipped_command_count": conditional_skipped_command_count,
        "conditional_skipped_command_ids": conditional_skipped_command_ids,
        "first_failed_command": first_failed_command,
        "batch_summary": batch_summary,
        "stop_on_error": stop_on_error,
        "stopped_early": stopped_early,
        "command_ok": all_commands_ok,
        "all_commands_ok": all_commands_ok,
        "outcome_class": outcome_class,
        "recovery_actions": recovery_actions,
        "result_mode": result_shape.mode,
        "result_mode_reason": result_shape.reason,
        "auto_compacted_output_bytes": result_shape.auto_compacted_output_bytes,
        "results_included": result_shape.mode != "none",
        "result_output_included": result_shape.mode == "full",
        "results_omitted_count": result_shape.results_omitted_count,
        "results": result_shape.results,
        "duration_ms": started.elapsed().as_millis(),
        "warnings": warnings
    }));
    if let Some(object) = output.as_object_mut() {
        object.insert("execution_lane".into(), json!(execution_lane));
        object.insert("blocking_queue_wait_ms".into(), json!(0));
        object.insert("admission_lane".into(), json!("per_command_process"));
        object.insert("admission_limit".into(), json!(workspace_limit));
        object.insert("global_admission_limit".into(), json!(global_limit));
        object.insert("admission_queue_wait_ms".into(), json!(0));
    }
    output
}

#[cfg(test)]
mod result_mode_tests {
    use super::*;

    fn synthetic_result(stdout_bytes: u64) -> Value {
        json!({
            "index": 0,
            "id": "large-output",
            "command_ok": true,
            "skipped": false,
            "resource_lock_wait_ms": 0,
            "batch_queue_wait_ms": 0,
            "result": {
                "status": "exited",
                "command_ok": true,
                "process_exit_code": 0,
                "execution_mode": "job",
                "requested_process_timeout_ms": 5_000,
                "effective_process_timeout_ms": 5_000,
                "process_timeout_limit_ms": 21_600_000,
                "process_deadline_ts_ms": 123_456_789,
                "process_timeout_remaining_ms": 4_000,
                "timeout_clamped": false,
                "polling_extends_process_deadline": false,
                "timeout_scope": "process",
                "elapsed_ms": 10,
                "stdout": "x",
                "stderr": "",
                "stdout_bytes": stdout_bytes,
                "stderr_bytes": 0,
                "output_refs": {
                    "stdout": "output://session/stdout",
                    "stderr": "output://session/stderr"
                }
            }
        })
    }

    #[test]
    fn result_mode_auto_compacts_large_output_and_honors_explicit_modes() {
        let results = vec![synthetic_result(32_768)];
        let auto = shape_exec_many_results(&results, None).expect("auto result mode");
        assert_eq!(auto.mode, "summary");
        assert_eq!(auto.reason, "large_output");
        assert_eq!(auto.auto_compacted_output_bytes, 32_768);
        assert_eq!(auto.results[0]["stdout_bytes"], 32_768);
        assert_eq!(auto.results[0]["execution_mode"], "job");
        assert_eq!(auto.results[0]["effective_process_timeout_ms"], 5_000);
        assert_eq!(auto.results[0]["polling_extends_process_deadline"], false);
        assert_eq!(auto.results[0]["timeout_scope"], "process");
        assert_eq!(
            auto.results[0]["output_refs"]["stdout"],
            "output://session/stdout"
        );
        assert!(auto.results[0].get("stdout").is_none());

        let full = shape_exec_many_results(&results, Some("full")).expect("full result mode");
        assert_eq!(full.mode, "full");
        assert_eq!(full.results[0]["result"]["stdout"], "x");

        let none = shape_exec_many_results(&results, Some("none")).expect("none result mode");
        assert_eq!(none.mode, "none");
        assert!(none.results.is_empty());
        assert_eq!(none.results_omitted_count, 1);

        assert!(shape_exec_many_results(&results, Some("invalid")).is_err());
    }
}
