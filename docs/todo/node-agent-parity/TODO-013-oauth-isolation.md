<!-- parity-id: NP-013 -->
<!-- parity-status: done -->
# NP-013 — OAuth runtime isolation

- Priority: P1
- Area: authentication
- Status: done

## Gap

Resolved in Node Agent 0.9.0. Each Agent runtime now owns an OAuthRuntime with an isolated pending authorization-code store and clears it when the HTTP server closes.

## Rust evidence

- `src-tauri/src/auth/oauth_flow.rs`
- `src-tauri/src/auth/oauth.rs`

## Node current state

- `packages/node-agent/src/oauth.ts`
- `packages/node-agent/src/server.ts`
- `packages/node-agent/test/oauth.test.mjs`

## Implementation scope

Implemented an OAuthRuntime instance per Agent runtime and routed authorization, token exchange, bearer-token verification, metadata, and lifecycle disposal through it. The implementation uses the workspace's fixed client identity with Authorization Code + S256 PKCE and refresh tokens. Public-client registration returns that configured identity after validating callback metadata, matching Desktop.

## Acceptance checklist

- [x] Pending authorization codes are isolated by Agent runtime.
- [x] A code issued by runtime A is rejected by runtime B.
- [x] Runtime shutdown clears pending state.
- [x] Expired authorization codes are removed when a new code is issued.
- [x] Existing PKCE, redirect, issuer, audience, and client-secret checks continue to pass.

## Verification

Covered by `packages/node-agent/test/oauth.test.mjs`, `packages/node-agent/test/server.test.mjs`, and the full `npm run verify:repo` suite.

## Connection reliability follow-up (0.1.68)

- Desktop and Node consume authorization codes only after callback and PKCE validation succeeds. A corrected failed request can retry within the five-minute lifetime; successful redemption remains single-use, including concurrent Desktop requests.
- Node retains pending codes during unchanged or TTL-only configuration updates. Changing client credentials, password, or signing key clears them.
- Node now matches Desktop callback origins and application schemes (ChatGPT, Claude, VS Code, Cursor, Windsurf and HTTP loopback callbacks), and exposes `/oauth/register` for public clients, including the matching workspace-scoped built-in tunnel route. Confidential clients still require manual credentials.
- Both runtimes resolve a rotated `*.trycloudflare.com` hostname from the live request when the saved URL is also a Quick Tunnel. Static public URLs keep their configured identity.
- Authorization passwords are reusable by default; refresh support and long default token lifetimes require running the updated application. Existing tokens keep their original expiry, and explicit configured lifetimes remain in effect.

Regression coverage: `oauth.test.mjs` includes failed-then-correct exchange, TTL-only hot update, callback parity, rotated tunnel discovery, and HTTP native-client registration/PKCE with prefixed routes. Rust OAuth tests cover corrected exchange and simultaneous single-use redemption. Never include passwords, codes, PKCE verifiers, access tokens or refresh tokens in diagnostic reports.
