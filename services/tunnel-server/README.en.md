# Coding Tools Built-in WSS Tunnel Server

[繁體中文](README.md)

Narrow reverse HTTP tunnel for Coding Tools MCP. **Caddy (or another reverse proxy) terminates TLS**; this process listens on internal HTTP ports and proxies public routes over server-managed WSS workers.

| Port (container) | Role |
|---|---|
| `8088` | Public tunnel: WSS, enrollment POST, MCP / Actions proxy, `/health` |
| `8089` | Optional Admin WebUI (never mounted on the public router) |

Wire protocol: **`coding-tools-tunnel-v4`** (version `4`). Older clients are rejected because v4 requires mutual Ed25519 server/device authentication. Design notes: [`docs/builtin-wss-tunnel.md`](../../docs/builtin-wss-tunnel.md).

## Authentication model (current)

There are **no shared or per-client tunnel tokens**.

| Secret | Who creates it | Where it lives |
|---|---|---|
| Device Ed25519 private key | Desktop app (random, on first enroll) | Client OS secret store only |
| Device public key | Desktop app | Server SQLite (`tunnel.db`) |
| One-time enrollment code | Admin WebUI or `enroll create` CLI | Server stores **SHA-256 digest** only |
| Admin password | **You** (not auto-generated) | Env or password file at process start |
| Admin session / CSRF | Server (random per login) | Server memory + `HttpOnly` cookie |

The server never holds device private keys. WSS auth is challenge–response: random nonce, client signs, server verifies the enrolled public key.

## Configuration

All settings are environment variables. Nothing is auto-generated for you at server init except runtime session material after Admin login.

| Variable | Required | Default | Notes |
|---|---|---|---|
| `CODING_TOOLS_TUNNEL_PUBLIC_ORIGIN` | Strongly recommended | hard-coded fallback origin in binary | Set to your real HTTPS origin; enrollment links use this |
| `CODING_TOOLS_TUNNEL_BIND` | No | `0.0.0.0:8088` | Public listener |
| `CODING_TOOLS_TUNNEL_ADMIN_BIND` | No | *(disabled)* | e.g. `0.0.0.0:8089` enables Admin WebUI |
| `CODING_TOOLS_TUNNEL_ADMIN_USERNAME` | If admin bind set | — | Login username |
| `CODING_TOOLS_TUNNEL_ADMIN_PASSWORD_FILE` | If admin bind set\* | — | Preferred; file contents trimmed; **≥ 12 bytes** |
| `CODING_TOOLS_TUNNEL_ADMIN_PASSWORD` | If admin bind set\* | — | Inline fallback when `_FILE` is unset |
| `CODING_TOOLS_TUNNEL_ADMIN_SESSION_SECONDS` | No | `28800` (8h) | Allowed range: 5 minutes–7 days |
| `CODING_TOOLS_TUNNEL_DB` | No | `tunnel-data/tunnel.db` | Container image sets `/data/tunnel.db` |
| `CODING_TOOLS_TUNNEL_LOG_DIR` | No | `<db-parent>/logs` | Container image sets `/data/logs` |
| `CODING_TOOLS_TUNNEL_MAX_BODY_BYTES` | No | 8 MiB | Buffered public request body limit |
| `CODING_TOOLS_TUNNEL_RESPONSE_HEAD_TIMEOUT_MS` | No | `30000` | Guard for response headers **after a worker accepts the job**. Queue time is separate; this is not a tool runtime limit. The desktop sends headers first and streams the result. |
| `CODING_TOOLS_TUNNEL_RECONNECT_GRACE_MS` | No | built-in default | Worker reconnect grace |
| `RUST_LOG` | No | `coding_tools_tunnel_server=info` | Tracing filter |

\* Exactly one of `ADMIN_PASSWORD_FILE` or `ADMIN_PASSWORD` is required when Admin is enabled. If `ADMIN_PASSWORD_FILE` is set, it is used (file must exist and be readable).

Persist the **whole** `/data` directory (DB + logs), not only `tunnel.db`.

Admin credentials are **not** randomly initialized by the server. Create a long password yourself before first start.

## Worker capacity, queueing, and error semantics

Admin WebUI manages independent MCP / Actions startup, idle, maximum, pending queue, connecting grace, staged shrink, and burst-warm policies. Defaults include `start=4`, `min idle=2`, `max idle=4`, `max workers=16`, 32 pending requests, a 10-second worker-acquire deadline, a scale-down step of 4, and a 120-second burst-warm window.

Public requests now have two separate deadlines:

1. **Worker acquisition**: the pending queue has a real policy limit. A full queue or expired acquisition deadline returns `503 Service Unavailable`, `Retry-After: 1`, and `X-Tunnel-Error` (`worker_capacity_exhausted` or `worker_acquire_timeout`).
2. **Response head**: timing begins only after a worker accepts the job. Only this stage can return `504 Gateway Timeout`.

The server attaches a short-lived demand hint to `request_head`, allowing the desktop to add several workers at once. Connecting workers count as expected capacity only during grace; afterwards they stop suppressing fresh capacity but are not killed merely because the network is slow. After a burst, scale-down occurs in fixed steps and temporarily retains a warm floor to avoid repeated `4 → 16 → 4` oscillation.

The dashboard exposes current/peak queue depth, average/maximum queue wait, capacity rejections, and worker-acquire timeouts. Successful proxied responses include `X-Tunnel-Queue-Wait-Ms`.

## Public MCP concurrency load test

Use `scripts/tunnel-load-test.py`. The access token is read only from an environment variable and is never written to the report:

```powershell
$env:CODING_TOOLS_MCP_ACCESS_TOKEN = "<access-token>"
python scripts/tunnel-load-test.py `
  --endpoint "https://example.com/clients/<client-id>/mcp" `
  --workspace-folder-id "<folder-id>" `
  --concurrency 20 `
  --duration-seconds 45
```

The JSON report classifies success, expected 503 capacity protection, 504, RPC/transport errors, and reports latency plus queue-wait p50/p95. Expected 503 responses are allowed by default; use `--fail-on-capacity` in CI when they should fail the run.

## Quick start: Docker Compose (recommended example)

The Compose file is an **optional example**. It builds the image, mounts `/data`, enables Admin, and health-checks `GET /health`. TLS is still your job (host Caddy or another proxy).

### 1. Create config and password (not auto-generated)

From the **repository root**:

```sh
cp services/tunnel-server/.env.example services/tunnel-server/.env
cp services/tunnel-server/admin-password.example.txt services/tunnel-server/admin-password.txt
```

Edit:

1. `services/tunnel-server/.env` → set `TUNNEL_PUBLIC_ORIGIN` to your real HTTPS origin (e.g. `https://tunnel.example.com`).
2. `services/tunnel-server/admin-password.txt` → replace with a long random password (**at least 12 characters/bytes** after trim).

Do not commit `.env` or `admin-password.txt`.

### 2. Build and start

```sh
docker compose \
  --env-file services/tunnel-server/.env \
  -f services/tunnel-server/compose.example.yml \
  up -d --build
```

Check:

```sh
curl -sS http://127.0.0.1:8088/health
# expect: ok

docker compose \
  --env-file services/tunnel-server/.env \
  -f services/tunnel-server/compose.example.yml \
  ps
```

Admin WebUI (loopback only by default): open `http://127.0.0.1:8089/` and log in with `TUNNEL_ADMIN_USERNAME` + the password file contents.

### 3. Enroll a desktop workspace

**Option A — Admin WebUI**

1. Open Admin → create enrollment (Client ID, service `mcp` / `actions` / `both`, TTL).
2. Copy the printed HTTPS link.
3. In Coding Tools MCP workspace tunnel settings, paste the link.
4. The app generates the device keypair locally, enrolls the public key, and stores the private key in the OS secret store.

**Option B — CLI inside the running stack** (same SQLite volume)

Compose `ENTRYPOINT` is the server binary, so extra args become CLI subcommands:

```sh
docker compose \
  --env-file services/tunnel-server/.env \
  -f services/tunnel-server/compose.example.yml \
  run --rm --no-deps tunnel-server \
  enroll create --client-id pc-a --service both --ttl-seconds 600
```

Against an already-running container (`docker exec` does **not** use `ENTRYPOINT`):

```sh
docker compose \
  --env-file services/tunnel-server/.env \
  -f services/tunnel-server/compose.example.yml \
  exec tunnel-server \
  /usr/local/bin/coding-tools-tunnel-server \
  enroll create --client-id pc-a --service both --ttl-seconds 600
```

List / revoke devices:

```sh
# list
docker compose ... exec tunnel-server \
  /usr/local/bin/coding-tools-tunnel-server devices list

# revoke
docker compose ... exec tunnel-server \
  /usr/local/bin/coding-tools-tunnel-server \
  devices revoke --device-id <device-id>
```

After revoke, create a new enrollment link; the desktop app rotates to a new device ID and private key.

### 4. Stop / wipe (careful)

```sh
docker compose \
  --env-file services/tunnel-server/.env \
  -f services/tunnel-server/compose.example.yml \
  down

# also delete enrolled devices + logs volume:
# docker compose ... down -v
```

## Docker image

Build context is the **repository root** (needs `crates/tunnel-protocol` + `services/tunnel-server`):

```sh
docker build \
  -f services/tunnel-server/Dockerfile \
  -t coding-tools-tunnel-server:local \
  .
```

Image defaults:

- User: `tunnel` (non-root)
- DB: `/data/tunnel.db`
- Logs: `/data/logs`
- `wget` installed for health checks
- `HEALTHCHECK` hits `http://127.0.0.1:8088/health`

Run without Compose (Admin via env password):

```sh
docker run --rm \
  --name coding-tools-tunnel \
  -p 127.0.0.1:8088:8088 \
  -p 127.0.0.1:8089:8089 \
  -v coding-tools-tunnel-data:/data \
  -e CODING_TOOLS_TUNNEL_PUBLIC_ORIGIN=https://tunnel.example.com \
  -e CODING_TOOLS_TUNNEL_ADMIN_BIND=0.0.0.0:8089 \
  -e CODING_TOOLS_TUNNEL_ADMIN_USERNAME=admin \
  -e CODING_TOOLS_TUNNEL_ADMIN_PASSWORD='replace-with-a-long-random-password' \
  coding-tools-tunnel-server:local
```

Or mount a password file:

```sh
docker run --rm \
  -v coding-tools-tunnel-data:/data \
  -v /secure/admin-password.txt:/run/secrets/admin_password:ro \
  -e CODING_TOOLS_TUNNEL_PUBLIC_ORIGIN=https://tunnel.example.com \
  -e CODING_TOOLS_TUNNEL_ADMIN_BIND=0.0.0.0:8089 \
  -e CODING_TOOLS_TUNNEL_ADMIN_USERNAME=admin \
  -e CODING_TOOLS_TUNNEL_ADMIN_PASSWORD_FILE=/run/secrets/admin_password \
  coding-tools-tunnel-server:local
```

## Local binary (no Docker)

```sh
cargo run --manifest-path services/tunnel-server/Cargo.toml --release
```

With Admin:

```sh
CODING_TOOLS_TUNNEL_PUBLIC_ORIGIN=https://tunnel.example.com \
CODING_TOOLS_TUNNEL_ADMIN_BIND=127.0.0.1:8089 \
CODING_TOOLS_TUNNEL_ADMIN_USERNAME=admin \
CODING_TOOLS_TUNNEL_ADMIN_PASSWORD='replace-with-a-long-random-password' \
cargo run --manifest-path services/tunnel-server/Cargo.toml --release
```

CLI (process exits after the command; needs the same `CODING_TOOLS_TUNNEL_DB` as the long-running server if you share state):

```sh
coding-tools-tunnel-server enroll create \
  --client-id pc-a \
  --service both \
  --ttl-seconds 600

coding-tools-tunnel-server devices list
coding-tools-tunnel-server devices revoke --device-id <device-id>
```

## Public reverse-proxy routes

Route these paths to the public listener (`8088`) **before** any FRP fallback:

```text
/_tunnel/v1
/_tunnel/enroll/*
/builtin/*
/.well-known/oauth-authorization-server/builtin/*
/.well-known/oauth-protected-resource/builtin/*
```

Do **not** put Admin (`8089`) on the public internet. Defaults in the Compose example bind both ports to `127.0.0.1` so host Caddy can proxy. If Caddy runs in Docker instead, attach both services to a **private shared network** and avoid publishing Admin on a public interface.

## Admin WebUI behavior

- Separate listener only; no management routes on `8088`.
- Unauthenticated browsers get the login page.
- Login creates a random server-side session with `Secure`, `HttpOnly`, `SameSite=Strict`, host-only cookie (`__Host-coding_tools_admin_session`).
- Mutating requests require a per-session CSRF token.
- Password is verified with **Argon2** (unrelated to device WSS auth).
- Capabilities: create enrollment links, list devices, revoke devices, edit MCP/Actions worker-pool policies, view recent server/client logs (last 2,000 entries kept in SQLite).

## Data and logs

| Path (container) | Content |
|---|---|
| `/data/tunnel.db` | Devices, enrollment digests, worker policies, Admin log buffer |
| `/data/logs/` | Daily tracing files |

Rust tracing also goes to stdout (container logs).

## Gitea Actions image build

[`.gitea/workflows/publish-tunnel-server.yml`](../../.gitea/workflows/publish-tunnel-server.yml) builds this Dockerfile on a trusted self-hosted runner that shares the deployment host Docker daemon. Tags left in the local image store:

```text
coding-tools-tunnel-server:local
coding-tools-tunnel-server:sha-<40-character-commit>
coding-tools-tunnel-server:edge    # non-main
coding-tools-tunnel-server:latest  # main
```

The workflow does not restart or deploy containers; recreate the service manually when convenient. Runner and deploy host must share the same Docker daemon (typically mount `/var/run/docker.sock`). No registry login is required.

## Tests

```sh
cargo test --manifest-path crates/tunnel-protocol/Cargo.toml
cargo test --manifest-path services/tunnel-server/Cargo.toml
cargo clippy --manifest-path services/tunnel-server/Cargo.toml --all-targets -- -D warnings
```

## Troubleshooting

| Symptom | Check |
|---|---|
| Container unhealthy | Image must include `wget` (current Dockerfile does). `curl http://127.0.0.1:8088/health` → `ok` |
| Admin fails to start | `ADMIN_BIND` set but username/password missing, password &lt; 12 bytes, or password file unreadable |
| Enrollment link wrong host | Set `CODING_TOOLS_TUNNEL_PUBLIC_ORIGIN` / `TUNNEL_PUBLIC_ORIGIN` to the public HTTPS origin |
| `enroll create` empty devices on “running” server | CLI used a different DB path/volume; use `compose run`/`exec` against the same stack |
| Desktop enroll fails after revoke | Create a **new** enrollment link; old code is single-use |
| Worker auth fails | Protocol must be v3; ensure desktop and server versions match |

## What this example does **not** include

- TLS certificates or a Caddy service definition
- Full multi-service edge reverse-proxy stack
- Automatic Admin password generation
- Publishing Admin on `0.0.0.0` for the public internet (do not do this)
