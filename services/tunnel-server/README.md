# Coding Tools 內建 WSS 隧道伺服器

[English](README.en.md)

專為 Coding Tools MCP 設計的窄域反向 HTTP 隧道。**由 Caddy（或其他反向代理）終止 TLS**；本行程在內部 HTTP 連接埠接聽，並透過伺服器管理的 WSS worker 代理公開路由。

| 連接埠（容器） | 用途 |
|---|---|
| `8088` | 公開隧道：WSS、註冊 POST、MCP / Actions 代理、`/health` |
| `8089` | 選用 Admin WebUI（不會掛在公開路由器上） |

線上協定：**`coding-tools-tunnel-v4`**（版本 `4`）。舊版用戶端會被拒絕，因為 v4 強制 Server/Device 雙向 Ed25519 驗證。設計說明：[`docs/builtin-wss-tunnel.md`](../../docs/builtin-wss-tunnel.md)。

## 驗證模型（現行）

**沒有共用或 per-client 隧道 token。**

| 密鑰 | 誰產生 | 存在哪裡 |
|---|---|---|
| 裝置 Ed25519 私鑰 | 桌面端（首次註冊時隨機） | 僅用戶端 OS 密鑰庫 |
| 裝置公鑰 | 桌面端 | 伺服器 SQLite（`tunnel.db`） |
| 一次性註冊碼 | Admin WebUI 或 `enroll create` CLI | 伺服器只存 **SHA-256 digest** |
| Admin 密碼 | **你**（伺服器不會自動產生） | 啟動時環境變數或密碼檔 |
| Admin session / CSRF | 伺服器（每次登入隨機） | 伺服器記憶體 + `HttpOnly` cookie |

伺服器從不持有裝置私鑰。WSS 驗證為 challenge–response：隨機 nonce、用戶端簽章、伺服器用已註冊公鑰驗證。

## 設定

所有設定皆為環境變數。伺服器初始化時**不會**自動為你產生金鑰（Admin 登入後的 session 材料除外）。

| 變數 | 必填 | 預設 | 說明 |
|---|---|---|---|
| `CODING_TOOLS_TUNNEL_PUBLIC_ORIGIN` | 強烈建議 | 二進位內建後備 origin | 設為真實 HTTPS origin；註冊連結會用到 |
| `CODING_TOOLS_TUNNEL_BIND` | 否 | `0.0.0.0:8088` | 公開監聽 |
| `CODING_TOOLS_TUNNEL_ADMIN_BIND` | 否 | *（停用）* | 例如 `0.0.0.0:8089` 啟用 Admin WebUI |
| `CODING_TOOLS_TUNNEL_ADMIN_USERNAME` | 啟用 Admin 時 | — | 登入帳號 |
| `CODING_TOOLS_TUNNEL_ADMIN_PASSWORD_FILE` | 啟用 Admin 時\* | — | 建議；讀取後 trim；**≥ 12 bytes** |
| `CODING_TOOLS_TUNNEL_ADMIN_PASSWORD` | 啟用 Admin 時\* | — | 未設 `_FILE` 時的內嵌後備 |
| `CODING_TOOLS_TUNNEL_ADMIN_SESSION_SECONDS` | 否 | `28800`（8 小時） | 允許範圍：5 分鐘–7 天 |
| `CODING_TOOLS_TUNNEL_DB` | 否 | `tunnel-data/tunnel.db` | 容器映像預設 `/data/tunnel.db` |
| `CODING_TOOLS_TUNNEL_LOG_DIR` | 否 | `<db 父目錄>/logs` | 容器映像預設 `/data/logs` |
| `CODING_TOOLS_TUNNEL_MAX_BODY_BYTES` | 否 | 8 MiB | 公開請求 body 緩衝上限 |
| `CODING_TOOLS_TUNNEL_RESPONSE_HEAD_TIMEOUT_MS` | 否 | `30000` | Worker **接單後**等待回應標頭的保護期限；不包含排隊時間，也不是工具執行上限。桌面端會先送標頭並串流結果。 |
| `CODING_TOOLS_TUNNEL_RECONNECT_GRACE_MS` | 否 | 內建預設 | worker 重連寬限 |
| `RUST_LOG` | 否 | `coding_tools_tunnel_server=info` | tracing 過濾 |

\* 啟用 Admin 時，`ADMIN_PASSWORD_FILE` 與 `ADMIN_PASSWORD` 須擇一。若有設 `ADMIN_PASSWORD_FILE`，以檔案為準（檔案必須存在且可讀）。

請掛載整個 `/data` 目錄（DB + 日誌），不要只掛 `tunnel.db`。

Admin 憑證**不會**由伺服器隨機初始化。首次啟動前請自行建立夠長的密碼。

## Worker 容量、排隊與錯誤語意

Admin WebUI 可分別調整 MCP / Actions 的啟動、閒置、上限、排隊、連線寬限、分段縮容與 burst 保溫政策。預設值包含：`start=4`、`min idle=2`、`max idle=4`、`max workers=16`、最多 32 個 pending、取得 Worker 期限 10 秒、每次最多縮 4 個、burst 保溫 120 秒。

公開請求分成兩個期限：

1. **取得 Worker**：pending queue 有真正的政策上限；queue 滿或期限到會回 `503 Service Unavailable`、`Retry-After: 1` 與 `X-Tunnel-Error`（`worker_capacity_exhausted` 或 `worker_acquire_timeout`）。
2. **等待 response head**：只有 Worker 已接單後才開始計時；期限到才回 `504 Gateway Timeout`。

Server 會在 `request_head` 附上短期 demand hint，桌面端可一次補多個 Worker；連線中的 Worker 只在 grace 期間算作預期容量，超過後不再阻止補充新容量，但不會只因網路較慢就被終止。負載結束後先以固定 step 縮容，近期 burst 則暫時保留 warm floor，避免 `4 → 16 → 4` 反覆震盪。

Dashboard 顯示目前／峰值排隊量、平均／最長 queue wait、容量拒絕與 Worker 取得逾時。成功代理的回應會附上 `X-Tunnel-Queue-Wait-Ms`。

## 公開 MCP 併發壓測

倉庫提供 `scripts/tunnel-load-test.py`。Access token 僅從環境變數讀取，不會寫入報告：

```powershell
$env:CODING_TOOLS_MCP_ACCESS_TOKEN = "<access-token>"
python scripts/tunnel-load-test.py `
  --endpoint "https://example.com/clients/<client-id>/mcp" `
  --workspace-folder-id "<folder-id>" `
  --concurrency 20 `
  --duration-seconds 45
```

輸出 JSON 會分類成功、503 容量保護、504、RPC／傳輸錯誤，並提供 latency 與 queue-wait p50 / p95。預設允許預期的 503；CI 要把容量保護視為失敗時加上 `--fail-on-capacity`。

## 快速開始：Docker Compose（建議範例）

Compose 檔是**選用範例**：建映像、掛 `/data`、啟用 Admin，並對 `GET /health` 做 healthcheck。TLS 仍由你負責（主機 Caddy 或其他代理）。

### 1. 建立設定與密碼（不會自動產生）

在**倉庫根目錄**：

```sh
cp services/tunnel-server/.env.example services/tunnel-server/.env
cp services/tunnel-server/admin-password.example.txt services/tunnel-server/admin-password.txt
```

然後編輯：

1. `services/tunnel-server/.env` → 將 `TUNNEL_PUBLIC_ORIGIN` 設為真實 HTTPS origin（例如 `https://tunnel.example.com`）。
2. `services/tunnel-server/admin-password.txt` → 改成夠長的隨機密碼（trim 後**至少 12 字元／bytes**）。

不要提交 `.env` 或 `admin-password.txt`。

### 2. 建置並啟動

```sh
docker compose \
  --env-file services/tunnel-server/.env \
  -f services/tunnel-server/compose.example.yml \
  up -d --build
```

檢查：

```sh
curl -sS http://127.0.0.1:8088/health
# 預期：ok

docker compose \
  --env-file services/tunnel-server/.env \
  -f services/tunnel-server/compose.example.yml \
  ps
```

Admin WebUI（預設僅 loopback）：開啟 `http://127.0.0.1:8089/`，以 `TUNNEL_ADMIN_USERNAME` + 密碼檔內容登入。

### 3. 註冊桌面工作區

**方式 A — Admin WebUI**

1. 開啟 Admin → 建立註冊（Client ID、服務 `mcp` / `actions` / `both`、TTL）。
2. 複製印出的 HTTPS 連結。
3. 在 Coding Tools MCP 工作區隧道設定貼上連結。
4. 應用在本機產生裝置金鑰對、註冊公鑰，並把私鑰存進 OS 密鑰庫。

**方式 B — 在執行中的 stack 用 CLI**（同一 SQLite volume）

Compose 的 `ENTRYPOINT` 是伺服器 binary，額外參數會變成 CLI 子命令：

```sh
docker compose \
  --env-file services/tunnel-server/.env \
  -f services/tunnel-server/compose.example.yml \
  run --rm --no-deps tunnel-server \
  enroll create --client-id pc-a --service both --ttl-seconds 600
```

對已在跑的容器（`docker exec` **不會**使用 `ENTRYPOINT`）：

```sh
docker compose \
  --env-file services/tunnel-server/.env \
  -f services/tunnel-server/compose.example.yml \
  exec tunnel-server \
  /usr/local/bin/coding-tools-tunnel-server \
  enroll create --client-id pc-a --service both --ttl-seconds 600
```

列出／撤銷裝置：

```sh
# list
docker compose ... exec tunnel-server \
  /usr/local/bin/coding-tools-tunnel-server devices list

# revoke
docker compose ... exec tunnel-server \
  /usr/local/bin/coding-tools-tunnel-server \
  devices revoke --device-id <device-id>
```

撤銷後請建立**新的**註冊連結；桌面端會輪換為新的 device ID 與私鑰。

### 4. 停止／清除（小心）

```sh
docker compose \
  --env-file services/tunnel-server/.env \
  -f services/tunnel-server/compose.example.yml \
  down

# 連註冊裝置與日誌 volume 一併刪除：
# docker compose ... down -v
```

## Docker 映像

建置 context 為**倉庫根目錄**（需要 `crates/tunnel-protocol` + `services/tunnel-server`）：

```sh
docker build \
  -f services/tunnel-server/Dockerfile \
  -t coding-tools-tunnel-server:local \
  .
```

映像預設：

- 使用者：`tunnel`（非 root）
- DB：`/data/tunnel.db`
- 日誌：`/data/logs`
- 已安裝 `wget` 供健康檢查
- `HEALTHCHECK` 探測 `http://127.0.0.1:8088/health`

不用 Compose 直接跑（Admin 用環境變數密碼）：

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

或掛載密碼檔：

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

## 本機 binary（不用 Docker）

```sh
cargo run --manifest-path services/tunnel-server/Cargo.toml --release
```

含 Admin：

```sh
CODING_TOOLS_TUNNEL_PUBLIC_ORIGIN=https://tunnel.example.com \
CODING_TOOLS_TUNNEL_ADMIN_BIND=127.0.0.1:8089 \
CODING_TOOLS_TUNNEL_ADMIN_USERNAME=admin \
CODING_TOOLS_TUNNEL_ADMIN_PASSWORD='replace-with-a-long-random-password' \
cargo run --manifest-path services/tunnel-server/Cargo.toml --release
```

CLI（指令結束後程序退出；若要共用狀態，需與長跑伺服器使用相同 `CODING_TOOLS_TUNNEL_DB`）：

```sh
coding-tools-tunnel-server enroll create \
  --client-id pc-a \
  --service both \
  --ttl-seconds 600

coding-tools-tunnel-server devices list
coding-tools-tunnel-server devices revoke --device-id <device-id>
```

## 公開反向代理路由

請將下列路徑導向公開監聽（`8088`），且**優先於**任何 FRP 後備：

```text
/_tunnel/v1
/_tunnel/enroll/*
/builtin/*
/.well-known/oauth-authorization-server/builtin/*
/.well-known/oauth-protected-resource/builtin/*
```

**不要**把 Admin（`8089`）暴露到公網。Compose 範例預設兩個連接埠都綁 `127.0.0.1`，方便主機上的 Caddy 代理。若 Caddy 也在 Docker 中，請把兩邊接到**私有共享網路**，避免在公網介面發佈 Admin。

## Admin WebUI 行為

- 僅獨立監聽；`8088` 上沒有管理路由。
- 未驗證瀏覽器只會看到登入頁。
- 登入後建立隨機伺服端 session，cookie 為 `Secure`、`HttpOnly`、`SameSite=Strict`、host-only（`__Host-coding_tools_admin_session`）。
- 變更狀態的請求需要 per-session CSRF token。
- 密碼以 **Argon2** 驗證（與裝置 WSS 驗證無關）。
- 功能：建立註冊連結、列出裝置、撤銷裝置、編輯 MCP／Actions worker 池策略、查看近期伺服器／用戶端日誌（SQLite 保留最近 2,000 筆）。

## 資料與日誌

| 路徑（容器） | 內容 |
|---|---|
| `/data/tunnel.db` | 裝置、註冊 digest、worker 策略、Admin 日誌緩衝 |
| `/data/logs/` | 每日 tracing 檔 |

Rust tracing 也會輸出到 stdout（容器日誌）。

## Gitea Actions 映像建置

[`.gitea/workflows/publish-tunnel-server.yml`](../../.gitea/workflows/publish-tunnel-server.yml) 在信任的 self-hosted runner 上建此 Dockerfile，且 runner 與部署主機共用 Docker daemon。會在本機映像庫留下：

```text
coding-tools-tunnel-server:local
coding-tools-tunnel-server:sha-<40-character-commit>
coding-tools-tunnel-server:edge    # 非 main
coding-tools-tunnel-server:latest  # main
```

workflow 不會重啟或部署容器；請在方便時手動重建服務。Runner 與部署主機須共用同一 Docker daemon（通常掛載 `/var/run/docker.sock`）。不需要 registry 登入。

## 測試

```sh
cargo test --manifest-path crates/tunnel-protocol/Cargo.toml
cargo test --manifest-path services/tunnel-server/Cargo.toml
cargo clippy --manifest-path services/tunnel-server/Cargo.toml --all-targets -- -D warnings
```

## 疑難排解

| 症狀 | 檢查 |
|---|---|
| 容器 unhealthy | 映像須含 `wget`（目前 Dockerfile 有）。`curl http://127.0.0.1:8088/health` → `ok` |
| Admin 無法啟動 | 有設 `ADMIN_BIND` 但缺帳密、密碼 &lt; 12 bytes，或密碼檔無法讀取 |
| 註冊連結主機錯誤 | 設定 `CODING_TOOLS_TUNNEL_PUBLIC_ORIGIN` / `TUNNEL_PUBLIC_ORIGIN` 為公開 HTTPS origin |
| 對「執行中」伺服器 `enroll create` 看不到裝置 | CLI 用了不同 DB 路徑／volume；請對同一 stack 使用 `compose run`／`exec` |
| 撤銷後桌面註冊失敗 | 建立**新的**註冊連結；舊碼只能用一次 |
| Worker 驗證失敗 | 協定必須是 v3；確認桌面與伺服器版本一致 |

## 本範例**不包含**

- TLS 憑證或 Caddy 服務定義
- 完整多服務邊緣反向代理堆疊
- 自動產生 Admin 密碼
- 把 Admin 發佈在 `0.0.0.0` 供公網使用（請勿如此）
