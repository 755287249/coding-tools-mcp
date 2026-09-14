# Built-in WSS Tunnel

## Goal

Add a third tunnel choice to Coding Tools MCP without removing the existing FRP and Cloudflare providers.

- `builtin` — embedded Rust WSS client and dedicated Rust server; default for new workspaces
- `frp` — existing external FRP provider
- `cloudflare` — existing Cloudflare Tunnel provider

Existing workspace data is not migrated automatically: missing legacy tunnel fields continue to deserialize as FRP.

## Public routes

| Purpose | Route |
|---|---|
| WSS control | `/_tunnel/v1` |
| One-time enrollment | `/_tunnel/enroll/<code>` |
| MCP | `/builtin/clients/<client-id>/mcp` |
| MCP authorization metadata | `/.well-known/oauth-authorization-server/builtin/clients/<client-id>` |
| MCP protected-resource metadata | `/.well-known/oauth-protected-resource/builtin/clients/<client-id>/mcp` |
| Actions | `/builtin/actions/<client-id>` |

The WSS subprotocol is `coding-tools-tunnel-v4` and the wire protocol version is `4`. Older clients are intentionally rejected because v4 requires mutual server/device authentication.

## Components

```text
ChatGPT / public HTTP client
        | HTTPS 443
        v
      Caddy
        |
        v
coding-tools-tunnel-server (Rust / Axum / SQLite)
        | server-managed dynamic WSS worker pool per selected service
        v
Coding Tools MCP embedded Rust client
        | loopback HTTP
        v
local MCP or Actions listener
```

Shared protocol definitions live in `crates/tunnel-protocol`. The server is in `services/tunnel-server`; the desktop client is in `src-tauri/src/tunnel/builtin.rs`.

## Enrollment

1. The server administrator runs the local `enroll create` CLI for a Client ID.
2. The CLI creates a random one-time code, stores only its SHA-256 digest, and prints a short-lived HTTPS link.
3. The user pastes the link into the workspace.
4. The desktop app generates an Ed25519 keypair and device ID locally, then stores the private key in the operating-system secret store before contacting the server.
5. The server treats the Client ID bound to the enrollment code as authoritative, returns it together with the server's persistent Ed25519 identity (`server_id` plus public key), and stores only the device ID, Client ID, public key, allowed services, timestamps, and revocation state.
6. The desktop app verifies that `server_id = base64url(SHA-256(server_public_key))`, pins that server identity beside the device identity, rebuilds and saves the public route from the server-assigned Client ID, then clears the link from the local secret store.

An interrupted enrollment response is safely retryable: the same code, device ID, and public key return the original successful result and server-assigned Client ID. A different device cannot reuse the consumed code.

MCP and Actions in the same workspace share one device identity. Pasting a fresh enrollment link after revocation rotates to a new device ID and private key.

## WSS authentication

No shared token, per-client token, or Bearer token is supported. TLS still validates the HTTPS/WSS hostname and CA chain, but the long-lived tunnel binding is an independent application-layer Ed25519 server identity, so ordinary TLS certificate renewal or CA replacement does not break enrollment.

1. Client connects with Client ID, service, device ID, worker ID, and the `coding-tools-tunnel-v4` subprotocol.
2. Server sends a random short-lived nonce, its pinned `server_id`, and an Ed25519 signature over protocol version, nonce, expiry, server ID, device ID, Client ID, service, and worker ID.
3. Client verifies the server ID and signature against the public key pinned during enrollment **before it sends any device proof**.
4. Client signs the canonical device-auth payload containing protocol version, nonce, device ID, Client ID, service, and worker ID.
5. Server loads the registered device Ed25519 public key from SQLite, checks revocation and service permission, verifies the signature, and only then registers the worker route.
6. Server signs the final handshake transcript, including the authenticated device/route identity and authoritative worker policy, and returns it in `hello_ack`.
7. Client verifies that signed acknowledgement before accepting the worker policy or sending `Ready`.

A captured authentication response cannot be replayed on another connection because each challenge is random and short-lived. A WSS endpoint with a valid HTTPS certificate but a different application-layer server key is rejected by an already-enrolled client.

The server stores independent MCP and Actions policies. Each policy applies separately to every `(Client ID, service)` route: the default is 4 startup workers, 2 minimum idle, 4 maximum idle, and 16 maximum workers per client service. The desktop opens one bootstrap connection, learns the policy, and grows or gracefully shrinks the pool. Admin changes are pushed live to idle workers; the server also refuses connections above the route-level maximum.

Workers recycle after the configured request count or connection lifetime, with configurable jitter to avoid a simultaneous restart wave. Retirement happens only at an idle boundary, so an active request finishes and is never replayed automatically. The default limits are 500 requests or 3600 seconds, a 60-second scale-down delay, and 10 percent recycle jitter.

The desktop reports configured maximum, authenticated, idle, busy, recycled, and policy revision telemetry. It marks an active pool with no authenticated workers as `reconnecting`, sends WebSocket Ping frames every 15 seconds, and reconnects after 45 seconds without inbound traffic. Reconnect delay uses bounded exponential backoff with per-worker jitter and resets after a successful authentication.

## Wire messages

JSON text frames:

- `challenge`, `authenticate`, `hello_ack`, `policy_update`, `ready`
- `request_head`, `request_end`
- `response_head`, `response_end`
- `cancel`, `error`

Request and response body chunks use binary frames. Request bodies are bounded to 8 MiB. Response bodies use bounded channels and backpressure.

## Device administration

The optional Admin WebUI uses a separate listener, normally `8089`, and is never mounted on the public tunnel router. Deployment keeps this port inside the container network (or bound to loopback) and exposes it only through a private-path reverse proxy when remote access is required.

Admin login uses a username plus password (not a shared Bearer admin key):

- `CODING_TOOLS_TUNNEL_ADMIN_USERNAME`
- `CODING_TOOLS_TUNNEL_ADMIN_PASSWORD_FILE` (preferred) or `CODING_TOOLS_TUNNEL_ADMIN_PASSWORD`
- Password must be at least 12 bytes after trim; the server does **not** auto-generate it
- Successful login creates a random server-side session with a `Secure`, `HttpOnly`, `SameSite=Strict` host-only cookie; mutating requests also require a per-session CSRF token
- Password verification uses Argon2 and is unrelated to device WSS authentication

The WebUI can create enrollment links, list device status, revoke devices, and edit the independent MCP and Actions worker-pool policies. Local CLI remains available as a recovery and headless-management path:

```text
coding-tools-tunnel-server enroll create --client-id <id> --service both --ttl-seconds 600
coding-tools-tunnel-server devices list
coding-tools-tunnel-server devices revoke --device-id <id>
```

Docker / Compose setup, password file layout, and container CLI examples live in [`services/tunnel-server/README.md`](../services/tunnel-server/README.md).

No management route is exposed on the public `8088` listener.

## Path handling

MCP preserves the complete public path because the local listener exposes the same namespaced MCP and OAuth routes. Actions removes only `/builtin/actions/<client-id>` before forwarding. Partial path-segment matches are rejected.

## Security limits

- HTTPS/WSS only
- strict Client ID and device ID character set
- Ed25519 device keys generated locally
- persistent Ed25519 server identity pinned at enrollment; `server_id` is SHA-256 of the server public key
- server challenge is verified before the client reveals its device proof
- final `hello_ack` is signed over the authenticated identities and worker policy
- HTTPS certificate rotation is independent of the pinned server identity
- server stores public keys, not private keys
- one-time enrollment codes stored as SHA-256 digests
- atomic SQLite enrollment consumption and device revocation
- separate opt-in management listener with mandatory Admin username/password (min 12 bytes)
- management login uses Argon2 password verification, random session cookies, and CSRF tokens
- management HTML sets no-store, nosniff, no-referrer, frame denial and a restrictive CSP
- fixed loopback target; server cannot request arbitrary hosts or ports
- hop-by-hop headers removed in both directions
- bounded request, worker, and response queues
- stale-worker expiry and response-head cancellation release unusable or abandoned workers
- no TCP, UDP, remote-port, P2P, plugins, dashboard, or arbitrary proxy features
- isolated route namespace allows FRP and Built-in to coexist

## Deployment

The public server listens on internal port `8088`. The optional management server listens on a separately configured internal address such as `0.0.0.0:8089`. Prefer binding published ports to loopback or a private Docker network; do not expose Admin on the public internet. Mount the complete `/data` directory as a private named volume; it contains `tunnel.db`, the persistent `server-identity.json` used for application-layer pinning, the most recent 2,000 Admin WebUI log entries, and daily tracing files under `/data/logs`. The identity path can be overridden with `CODING_TOOLS_TUNNEL_SERVER_IDENTITY`. Back up this server identity with the database: replacing or losing it intentionally causes pinned clients to reject the server and requires re-enrollment; replacing only the HTTPS certificate does not.

An optional Compose example (build, secrets, healthcheck, volume) is under `services/tunnel-server/compose.example.yml`. See that directory’s README for step-by-step create/start/enroll commands. Caddy must route these public paths before the FRP fallback:

```text
/_tunnel/v1
/_tunnel/enroll/*
/builtin/*
/.well-known/oauth-authorization-server/builtin/*
/.well-known/oauth-protected-resource/builtin/*
```

## v3 to v4 migration

Existing enrolled v3 clients do not silently trust a new server key. Their existing device private key is preserved, but they fail closed until a one-time enrollment succeeds and records the v4 server pin. After that migration, stale enrollment URLs are ignored as before. Server identity rotation is likewise fail-closed; there is no silent key rollover in v4.

## Validation

```text
cargo test --manifest-path crates/tunnel-protocol/Cargo.toml
cargo test --manifest-path services/tunnel-server/Cargo.toml
cargo clippy --manifest-path services/tunnel-server/Cargo.toml --all-targets -- -D warnings
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=1
pnpm run verify
```

The server suite performs actual HTTP enrollment, Ed25519 challenge-response over WebSocket, public HTTP proxying, revocation rejection, idempotent enrollment retry, stale-worker expiry and reconnect, response-timeout cancellation, and immediate `503` behavior when no worker is connected.

## Deliberate limitations

- Request bodies are buffered rather than streamed end to end.
- Route entries remain registered after all workers disconnect, but requests fail immediately with `503` until workers reconnect.
- Requests that lose a connection are not replayed automatically; callers may retry idempotent operations.
- Public deployment still requires validation in the real reverse-proxy / Docker environment.
