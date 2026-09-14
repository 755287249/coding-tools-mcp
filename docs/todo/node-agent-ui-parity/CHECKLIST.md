# Node Agent Management UI parity checklist

**Status:** Complete

**Baseline:** Rust/Desktop Client `0.1.59` · Node Agent `0.29.35` · client compatibility `0.1.59`

This checklist tracks shared Management UI capabilities that should remain synchronized with the Rust Desktop console while preserving the pure Node Agent product boundary. It does not require visual identity or desktop-only process management.

## Product boundary retained

The following remain intentional exclusions rather than Node UI defects:

- Actions/OpenAPI service and Actions-specific settings.
- FRP and Cloudflare executable management.
- Native desktop software installation, tray, and auto-start controls.
- Static Bearer and no-auth MCP modes; Node remains OAuth-only.
- Legacy JSON transport; Node remains streamable-HTTP-only.
- Per-service runtime supervisor, automatic free-port selection, and stale-listener recovery.
- Live history-session running/active/inactive badges; the Node UI provides a read-only archive browser.
- Raw Desktop per-service stdout/stderr and Actions/FRP/Cloudflare log files; Node exposes structured persisted operation logs and sanitized telemetry instead.

## Follow-up (not this checklist)

Workspace **config** sharing across Desktop and Node is planned separately: [shared-workspace-config](../shared-workspace-config/README.md). Do not treat host-specific documents or `node-map.ts` overlays as UI parity defects.

## UI-001 — Telemetry browser

Rust evidence: `src/lib/components/TelemetryViewer.svelte`

Node evidence: `packages/node-agent/src/managementObservability.ts`, `src/lib/components/TelemetryViewer.svelte`, `src/lib/backend/node.ts`

- [x] Query retained telemetry by runtime/version/all scope.
- [x] Support errors-only, record limit, minimum duration, and aggregate sort controls.
- [x] Expose the complete backend sort set, including request bytes, response bytes, and queue wait.
- [x] Show call/error/latency summaries and per-tool aggregates.
- [x] Show expandable sanitized recent records without raw arguments, commands, paths, session IDs, or runtime IDs.
- [x] Keep telemetry access behind loopback, admin-token, and same-origin Management controls.

## UI-002 — History archive browser

Rust evidence: `src/lib/components/HistoryViewer.svelte`

Node evidence: `packages/node-agent/src/historyStorage.ts`, `packages/node-agent/src/historyMarkdown.ts`, `src/lib/components/HistoryViewer.svelte`

- [x] Restrict browsing to configured workspace folders and `docs/history-session`.
- [x] Reject a history directory whose canonical target escapes through a symlink or junction.
- [x] List numbered sessions with title, status, timestamps, checkpoint count, summary, and integrity warnings.
- [x] Support manual refresh while preserving the selected session when it still exists.
- [x] Abort stale list and detail requests so rapid folder/session changes cannot overwrite the active selection.
- [x] Render structured checkpoint fields and an optional raw Markdown view.
- [x] Redact the stable history session key from list, detail, and raw Markdown responses.
- [x] Keep archive access read-only and omit the intentionally excluded live activity badges.

## UI-003 — Active MCP health diagnostics

Rust evidence: `src/lib/components/HealthPanel.svelte`

Node evidence: `packages/node-agent/src/managementObservability.ts`, `src/lib/components/HealthPanel.svelte`

- [x] Probe only fixed routes on the same local Agent listener, derived from the accepted socket rather than the HTTP `Host` header.
- [x] Validate endpoint-specific contracts for `/health`, `/mcp/info`, OAuth authorization metadata, and protected-resource metadata.
- [x] Send an unauthenticated local MCP initialize request and require the expected HTTP 401 OAuth protected-resource challenge.
- [x] Report OAuth configuration and optional Built-in WSS runtime state.
- [x] Provide actionable failure hints without fetching an arbitrary public URL.

## UI-004 — Dashboard contract completion

Rust evidence: `src/routes/workspace/[id]/+page.svelte`

Node evidence: `packages/node-agent/src/dashboard.ts`, `src/routes/workspace/[id]/+page.svelte`

- [x] Display pending permissions by workspace folder.
- [x] Display persistent telemetry scanned, matched, and invalid-record counts.
- [x] Display tunnel worker, request, policy-revision, and timeout details already returned by the backend.
- [x] Preserve bounded summaries without exposing command, output, environment, or operation payloads.
- [x] Expose the six Workspace sections as ARIA tabs with Arrow Left/Right and Home/End keyboard navigation.

## UI-005 — Fine-grained policy management

Rust evidence: `src/routes/workspace/[id]/+page.svelte`

Node evidence: `packages/node-agent/src/config.ts`, `packages/node-agent/src/management/configStore.ts`, `packages/node-agent/src/sandbox.ts`, `src/lib/components/RuntimePolicyForm.svelte`, `src/lib/backend/node-map.ts`

- [x] Edit allowed commands.
- [x] Edit workspace-local executable policy.
- [x] Edit workspace script extensions.
- [x] Edit maximum patch bytes.
- [x] Edit workspace and global blocking/process concurrency limits.
- [x] Configure command sandbox enablement, backend, external filesystem grants, and backend options.
- [x] Configure Rust MCP security protections independently with per-item checkboxes and preserve legacy mode/profile compatibility fields.
- [x] Preserve policy, global limits, and sandbox settings when Quick Setup changes tunnel or OAuth settings.

## UI-006 — Sanitized diagnostics export

Rust evidence: `src/lib/components/TelemetryViewer.svelte`, `src/lib/components/HealthPanel.svelte`

Node evidence: `packages/node-agent/src/managementObservability.ts`, `src/routes/workspace/[id]/+page.svelte`

- [x] Export version, platform, tool profile, admission, session, permission, task, tunnel, policy, and telemetry summaries as JSON.
- [x] Exclude secrets, configured paths, public endpoints, raw commands, raw arguments, process output, and private runtime/session identifiers.
- [x] Keep export generation within the protected Management API and download it client-side.

## UI-007 — Structured operation log browser

Rust evidence: `src-tauri/src/harness/model.rs`, `src-tauri/src/harness/store.rs`, `src-tauri/src/harness/tools.rs`, `src-tauri/src/tools/dispatch.rs`, `src-tauri/src/tools/session.rs`

Node evidence: `packages/node-agent/src/state.ts`, `packages/node-agent/src/taskTools.ts`, `packages/node-agent/src/operationSummary.ts`, `packages/node-agent/src/processes.ts`, `packages/node-agent/src/managementObservability.ts`, `src/lib/components/OperationLogViewer.svelte`

- [x] Read persisted Rust-shaped Harness operation JSONL by canonical workspace identity, with legacy folder-ID compatibility.
- [x] Pair started and completed/failed events by correlation ID and derive duration, affected-file count, task tracking, and incomplete operations.
- [x] Treat legacy provisional `completed/running` records as incomplete until a real terminal event appears.
- [x] Persist the same bounded Rust/Node result summary for command, transport, execution, verification, error, termination, exit, timeout, truncation, traffic, wait, and warning diagnostics.
- [x] Both Rust and Node defer retained-process terminal records until actual session finalization; exit, timeout, kill, restart, and post-check results keep the original correlation ID.
- [x] Support folder, status, exact tool, failures-only, record-limit, cursor pagination, manual refresh, and load-older controls.
- [x] Abort stale requests so rapid filter or folder changes cannot overwrite the active log view.
- [x] Expose only an explicit structured summary; omit workspace/task IDs, raw arguments, commands, environment, stdin/stdout/stderr, output, and affected paths.
- [x] Redact sensitive reason text, raw command fragments, multiline tails, configured roots, and arbitrary absolute paths while retaining actionable failure context.
- [x] Keep operation-log access behind loopback, admin-token, same-origin, no-store Management controls.

## Security invariants

- [x] New Management routes inherit loopback-only, ephemeral admin-token, same-origin, no-store, CSP, and frame-denial controls.
- [x] Telemetry uses an explicit response-field allowlist.
- [x] Operation logs use an explicit derived response contract and never spread persisted raw records.
- [x] Health diagnostics cannot become an SSRF primitive.
- [x] Health diagnostics do not trust the request `Host` header, and history browsing enforces canonical containment.
- [x] Health probes validate expected response schemas and verify MCP OAuth protection without using a stored credential.
- [x] History raw content redacts the stable host session key.
- [x] Diagnostics exports remain sanitized and support-oriented.

## Verification

```powershell
npm run node-agent:ui-parity:check
node --test tests/node-agent-ui-parity.test.mjs
node --test packages/node-agent/test/management.test.mjs
npm run node-agent:verify-repo
npm run node-agent:parity:check
npm run node-agent:parity:complete
```
