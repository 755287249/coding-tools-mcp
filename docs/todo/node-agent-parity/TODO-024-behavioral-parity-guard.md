<!-- parity-id: NP-024 -->
<!-- parity-status: done -->
# NP-024 — Behavioral parity regression guard

- Priority: P2
- Area: verification
- Status: done

## Gap

The current roadmap checker validates manifest structure, source references, dependencies, status markers, and checkbox completion. It does not prove Rust and Node behavior, stable errors, response shapes, limits, or security invariants are aligned. The stale baseline allowed completed status to overstate parity confidence.

## Rust evidence

- `src-tauri/src/tools/registry.rs`
- `src-tauri/src/tools/context.rs`
- `src-tauri/src/tools/hub.rs`
- `src-tauri/src/tools/process_start.rs`
- `src-tauri/src/tunnel/builtin.rs`
- `src-tauri/examples/export_tool_catalog.rs`

## Node current state

- `scripts/check-node-agent-parity.mjs`
- `scripts/run-node-agent-parity-assertions.mjs`
- `tests/node-agent-parity.test.mjs`
- `tests/node-agent-tool-contracts.test.mjs`
- `docs/todo/node-agent-parity/assertions.json`
- `packages/node-agent/scripts/sync-rust-catalog.mjs`
- `packages/node-agent/src/rustCatalog.generated.ts`
- `packages/node-agent/test/behavioralParity.test.mjs`

## Required implementation

Add machine-readable behavioral assertions and a small differential fixture suite for high-risk shared contracts. The guard must detect baseline/version drift, missing invariant coverage, changed constants, and a completed TODO whose required behavioral assertion is absent.

## Acceptance checklist

- [x] Manifest baseline is verified against repository HEAD, Desktop compatibility version, and Node Agent version.
- [x] Every non-product parity item declares behavioral assertion IDs in addition to prose acceptance tests.
- [x] Assertions cover path containment, folder isolation, process startup, admission, workspace state, MCP response/errors, and tunnel timing.
- [x] Rust constants or exported fixtures are compared without brittle source-text snapshots where possible.
- [x] Security regressions fail `node-agent:parity:check`, not only package-specific tests.
- [x] A completed item with missing or skipped required assertions fails validation.
- [x] Intentional divergences remain explicit and do not silently suppress shared-contract checks.
- [x] CI output identifies the exact item and assertion that drifted.
- [x] Every generated Rust catalog tool name appears in the Node regression suite, preventing silently untested handlers.
- [x] Focused source guards protect consumed public bounds and order-sensitive cross-runtime semantics such as clean-before-truncation.
- [x] Source guards require both runtimes to consume `finish_task.summary` and `change_summary.change_id`, persist `latest_change_id`, and retain executable restart/selection regressions.

## Dependencies

Requires `NP-017` through `NP-023`; it is the final guard after the newly audited behavior is implemented.

## Verification

Verified schema-v2 baseline and assertion ownership with six validator fixtures covering stale versions and commit ancestry, missing bidirectional assertion links, failed and skipped differentials, completed-item dependency regression, and intentional divergence links. At completion, `node scripts/check-node-agent-parity.mjs` passed all 24 required assertions, including eight executable high-risk fixtures. `npm run verify:repo` completed 198 Node tests with 197 passing and the explicitly gated live-WSL test skipped; native dependency, Rust catalog, Desktop compatibility version, and parity checks all passed.

A later MCP transport re-audit resolved `NP-016` as an intentional exclusion and added `NP-025` through `NP-028`. Node Agent `0.28.3` completed that phase: all 28 assertions are now required, including executable Origin/protocol/envelope, streaming-heartbeat, HTTP-semantics, and Rust MCP transport constant fixtures.

The 0.29.3 follow-up inventory added `tests/node-agent-tool-contracts.test.mjs`: all 50 Rust catalog tools must have an explicit Node regression reference, and focused guards verify `exec_health_check`, `task_context.max_bytes`, and `project_state.clean` remain synchronized across Rust and Node.

The subsequent schema-consumption pass extends the same guard to `finish_task.summary` and `change_summary.change_id`, including persisted completion reasons, immutable change snapshots, restart recovery, latest-change fallback, and cross-task mismatch validation.

## Seed library v1

Desktop `tools/chat_seeds.rs` and Node `chat/seeds.ts` use the same v1 registry,
assignment generation, credential TTLs, inactivity grace and retirement records.
The dedicated capability gateways both restrict project/chat/process ownership and
preserve ordinary MCP authentication. Focused verification lives in Rust seed
unit/HTTP tests and Node `test/seeds.test.mjs`; shared UI and browser provisioning
are covered by root plugin fixtures. No Node parity exception is introduced.
