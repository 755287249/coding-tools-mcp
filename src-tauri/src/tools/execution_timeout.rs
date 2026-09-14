//! Fixed child-process budgets. Transport wait windows never renew these deadlines.
use serde_json::{json, Value};

use super::workspace::WorkspaceError;

pub const DEFAULT_JOB_TIMEOUT_MAX_MS: u64 = 6 * 60 * 60_000;
pub const ABSOLUTE_JOB_TIMEOUT_MAX_MS: u64 = 24 * 60 * 60_000;

#[derive(Clone, Debug)]
pub(crate) struct ProcessTimeoutContract {
    pub execution_mode: &'static str,
    pub requested_timeout_ms: Option<u64>,
    pub effective_timeout_ms: u64,
    pub limit_ms: u64,
}

pub(crate) fn job_timeout_limit_from_value(raw: Option<&str>) -> u64 {
    match raw {
        None => DEFAULT_JOB_TIMEOUT_MAX_MS,
        Some(raw) if !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) => raw
            .parse::<u64>()
            .ok()
            .filter(|value| *value <= 9_007_199_254_740_991)
            .map(|value| value.min(ABSOLUTE_JOB_TIMEOUT_MAX_MS))
            .unwrap_or(0),
        Some(_) => 0,
    }
}

pub(crate) fn configured_job_timeout_max_ms() -> u64 {
    match std::env::var("CTMCP_JOB_TIMEOUT_MAX_MS") {
        Ok(value) => job_timeout_limit_from_value(Some(&value)),
        Err(std::env::VarError::NotPresent) => DEFAULT_JOB_TIMEOUT_MAX_MS,
        Err(_) => 0,
    }
}

pub(crate) fn resolve_process_timeout(
    args: &Value,
    default_timeout_ms: u64,
    command_max_ms: u64,
    job_max_ms: u64,
) -> Result<ProcessTimeoutContract, WorkspaceError> {
    let job = args.get("job_timeout_ms").is_some();
    let ordinary = args.get("timeout_ms").is_some();
    if job && ordinary {
        return Err(WorkspaceError::invalid_argument(
            "job_timeout_ms and timeout_ms are mutually exclusive; wait_command.timeout_ms only controls polling",
        ));
    }
    if job
        && !args
            .get("operation_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.trim().is_empty() && id.encode_utf16().count() <= 128)
    {
        return Err(WorkspaceError::invalid_argument(
            "job_timeout_ms requires a stable nonempty operation_id of at most 128 characters",
        ));
    }
    let field = if job { "job_timeout_ms" } else { "timeout_ms" };
    let requested_timeout_ms = if job || ordinary {
        Some(
            args.get(field)
                .and_then(Value::as_u64)
                .filter(|value| *value > 0 && *value <= 9_007_199_254_740_991)
                .ok_or_else(|| {
                    WorkspaceError::invalid_argument(format!(
                        "{field} must be a positive safe integer in milliseconds"
                    ))
                })?,
        )
    } else {
        None
    };
    let limit_ms = if job {
        job_max_ms.min(ABSOLUTE_JOB_TIMEOUT_MAX_MS)
    } else {
        command_max_ms.max(1)
    };
    let execution_mode = if job { "job" } else { "command" };
    if job && limit_ms == 0 {
        return Err(WorkspaceError::ToolDetails {
            code: "LONG_RUNNING_JOBS_DISABLED",
            message: "Long-running jobs are disabled by host CTMCP_JOB_TIMEOUT_MAX_MS".into(),
            category: "policy",
            retryable: false,
            details: json!({"process_started": false, "timeout_scope": "process"}),
        });
    }
    if requested_timeout_ms.is_some_and(|requested| requested > limit_ms) {
        return Err(WorkspaceError::ToolDetails {
            code: "COMMAND_TIMEOUT_EXCEEDS_LIMIT",
            message: format!(
                "{field} exceeds configured limit ({limit_ms} ms); no process was started"
            ),
            category: "policy",
            retryable: false,
            details: json!({
                "process_started": false, "timeout_scope": "process", "execution_mode": execution_mode,
                "requested_process_timeout_ms": requested_timeout_ms, "process_timeout_limit_ms": limit_ms,
                "polling_extends_process_deadline": false,
                "suggestion": if job { "Choose a budget within the host job limit or ask the host administrator to change it." }
                else { "For an authorized long job, use job_timeout_ms with a stable operation_id instead of timeout_ms. Polling cannot extend a process deadline." }
            }),
        });
    }
    Ok(ProcessTimeoutContract {
        execution_mode,
        requested_timeout_ms,
        effective_timeout_ms: requested_timeout_ms.unwrap_or(default_timeout_ms.clamp(1, limit_ms)),
        limit_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const COMMAND_MAX: u64 = 1_800_000;

    #[test]
    fn job_timeout_resolves_two_hours_without_command_clamp() {
        let value = resolve_process_timeout(
            &json!({"job_timeout_ms":7_200_000,"operation_id":"full-run"}),
            30_000,
            COMMAND_MAX,
            DEFAULT_JOB_TIMEOUT_MAX_MS,
        )
        .unwrap();
        assert_eq!(value.execution_mode, "job");
        assert_eq!(value.requested_timeout_ms, Some(7_200_000));
        assert_eq!(value.effective_timeout_ms, 7_200_000);
        assert_eq!(value.limit_ms, DEFAULT_JOB_TIMEOUT_MAX_MS);
    }

    #[test]
    fn job_timeout_rejects_invalid_conflicting_and_oversized_budgets() {
        for args in [
            json!({"job_timeout_ms":1000}),
            json!({"job_timeout_ms":1000,"operation_id":" "}),
            json!({"job_timeout_ms":1000,"operation_id":42}),
            json!({"job_timeout_ms":1000,"operation_id":"x","timeout_ms":1}),
            json!({"timeout_ms":0}),
            json!({"timeout_ms":null}),
            json!({"timeout_ms":"1000"}),
            json!({"timeout_ms":0.5}),
            json!({"timeout_ms":-1}),
            json!({"timeout_ms":COMMAND_MAX+1}),
            json!({"job_timeout_ms":ABSOLUTE_JOB_TIMEOUT_MAX_MS+1,"operation_id":"job"}),
        ] {
            assert!(
                resolve_process_timeout(&args, 30_000, COMMAND_MAX, u64::MAX).is_err(),
                "accepted {args}"
            );
        }
    }

    #[test]
    fn job_timeout_host_configuration_fails_closed_and_preserves_normal_defaults() {
        assert_eq!(
            job_timeout_limit_from_value(None),
            DEFAULT_JOB_TIMEOUT_MAX_MS
        );
        assert_eq!(
            job_timeout_limit_from_value(Some("99999999999")),
            ABSOLUTE_JOB_TIMEOUT_MAX_MS
        );
        assert_eq!(job_timeout_limit_from_value(Some("7200000")), 7_200_000);
        for raw in ["0", "", "bad", "-1", "1.5"] {
            assert_eq!(job_timeout_limit_from_value(Some(raw)), 0);
        }
        assert!(resolve_process_timeout(
            &json!({"job_timeout_ms":1,"operation_id":"job"}),
            30_000,
            COMMAND_MAX,
            0
        )
        .is_err());
        let normal = resolve_process_timeout(&json!({}), 30_000, COMMAND_MAX, 0).unwrap();
        assert_eq!(normal.execution_mode, "command");
        assert_eq!(normal.effective_timeout_ms, 30_000);
        assert_eq!(normal.requested_timeout_ms, None);
    }
}
