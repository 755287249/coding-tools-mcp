# Managed long-running commands

## Goal
Run a complete multi-step benchmark in one retained child process without confusing an MCP wait window with a process lifetime. Do not restart live agents or touch concurrent benchmarks during development.

## Contract
- Keep ordinary `timeout_ms` bounded by host command policy. Reject invalid or oversized explicit budgets before spawn; never silently shorten one.
- Add opt-in `job_timeout_ms`, mutually exclusive with `timeout_ms`. Require a nonempty stable `operation_id`. This is a fixed total child-process budget, not an idle timeout or a renewal lease.
- Host policy: `CTMCP_JOB_TIMEOUT_MAX_MS`, default 6 hours, absolute maximum 24 hours; 0 disables long jobs. Child `env` cannot override the host limit. Existing workspace, shell, sandbox, cancellation and concurrency policy remains authoritative.
- `yield_time_ms` and `wait_command.timeout_ms` control response waiting only. Polling, reattachment and output do not extend the deadline. Reuse transport-safe waits and session/operation identity instead of creating a second job registry.
- Return execution_mode, requested_process_timeout_ms, effective_process_timeout_ms, process_timeout_limit_ms, process_deadline_ts_ms, process_timeout_remaining_ms, timeout_clamped=false, polling_extends_process_deadline=false and timeout_scope=process. Preserve metadata in retained session projections and diagnostics.
- Execution identity includes mode and budget. Reusing an ID for a different job fails instead of launching a duplicate. Completed retained jobs remain reattachable.
- Post-checks retain existing bounded command budgets and reject job_timeout_ms.
- Preserve fixed deadlines, cancellation, output retention and detached recovery grace. Silence alone does not prove an LLM round is stuck; benchmark-level no-progress detection remains independent.

## Implementation sequence
1. Red tests for resolution, invalid/oversized input, mode conflicts, disabled host policy and metadata.
2. Node/Rust deadline contracts and retained-session integration, shared schemas, descriptions and diagnostics.
3. Short-budget lifecycle tests: survive ordinary cap, polling/reattachment without restart, hard deadline, cancellation and batch validation.
4. Cross-runtime generation/parity, focused/full regression and build checks. Document unverified coverage.

## Not in this change
No automatic promotion into a long job, no unlimited lifetime, no deadline renewal through polling, no benchmark resume redesign, no MCP Tasks protocol claim, no live deployment and no new UI setting. Host environment configuration applies to both runtimes.
