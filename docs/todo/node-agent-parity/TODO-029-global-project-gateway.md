<!-- parity-id: NP-029 -->
<!-- parity-status: done -->
# NP-029 — Global connection and fixed project capability isolation

- Priority: P0
- Area: security
- Status: done

## Gap

One transport configuration is separate from fixed project directories. Every project operation requires its persisted chat attachment capability. Server-derived conversation identity prevents session-header spoofing. Native processes and extensions are excluded; commands require an OS sandbox.

## Verification

- [x] Node global configuration, empty-project startup, fixed-root migration and secret preservation.
- [x] Real HTTP OAuth project routing and capability revocation tests.
- [x] Rust gateway project/root/closed-chat and stopped-admission tests.
- [x] Production browser, seed transport, concurrent lifecycle and complete repository acceptance.

Release gate: Windows native compilation and portable checks run in `chat-beta.yml` before personal publication; no runtime parity implementation gap remains.

Tests: `packages/node-agent/test/projectGateway.test.mjs`; Rust `mcp::listener::gateway`.

## Rust evidence

`src-tauri/src/mcp/listener/gateway.rs`, `src-tauri/src/data/connection.rs`, and `src-tauri/src/tools/project_scope.rs` implement the new boundary.

## Node current state

`packages/node-agent/src/server/projectGateway.ts`, `application.ts`, and `projectIsolation.ts` implement routing, migration and command constraints. Cross-runtime acceptance is covered by the tests above.
