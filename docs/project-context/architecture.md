# 架構設計

> 本文件描述目前已落地的 Coding Tools MCP 架構，不再以早期 Tauri 骨架規劃為準。

## Repository 結構

```text
coding-tools-mcp/
├── src-tauri/                    # Rust/Tauri Desktop 與共用工具核心
│   ├── src/
│   │   ├── commands/             # Tauri IPC commands
│   │   ├── data/                 # profile/settings/secrets 持久化與保護
│   │   ├── runtime/              # MCP/Actions runtime supervisor
│   │   ├── mcp/                  # Streamable HTTP、JSON-RPC、telemetry
│   │   ├── actions/              # OpenAPI/Actions gateway
│   │   ├── tools/                # file/edit/exec/git/history/harness 工具核心
│   │   ├── tunnel/               # Built-in WSS、FRP、Cloudflare
│   │   ├── auth/                 # OAuth 與授權流程
│   │   └── platform/             # Windows/macOS/Linux/WSL 差異
│   └── tests/                    # Rust 整合與契約測試
├── src/                          # 共用 SvelteKit UI（Desktop `/`；Node `/ui/`）
├── packages/node-agent/
│   ├── src/                      # Headless Agent、MCP、管理 API、工具實作
│   ├── management-static/        # Node PWA manifest、icon、Service Worker
│   └── test/                     # Node 行為與 parity 測試
├── crates/
│   ├── command-policy/           # Rust 共用指令政策
│   └── tunnel-protocol/          # Desktop/server 共用 WSS 協定
├── services/tunnel-server/       # Rust Built-in WSS 公網伺服器
├── scripts/                      # 驗證、版本同步、portable build
├── tests/                        # Repository-level contract/parity tests
└── docs/                         # 規格、架構、驗證與歷史文件
```

## Desktop 執行鏈路

```text
Svelte route/component
  → src/lib/api/* → FrontendBackend
  → Tauri invoke（Desktop host adapter）
  → src-tauri/src/commands/*
  → AppState
      ├─ DataStore：workspace、settings、加密 secrets
      ├─ RuntimeSupervisor：MCP／Actions listener 狀態
      └─ TunnelSupervisor：Built-in WSS／FRP／Cloudflare
  → tools::call_tool / call_tool_async
      ├─ policy、permission、admission、redaction
      ├─ file／patch／exec／git／history／harness
      └─ telemetry 與 retained process session
```

`src-tauri/src/tools/mod.rs` 將 `call_tool` 與 `call_tool_async` 定義為唯一共用工具入口。MCP 與 Actions 必須經過同一個 dispatch、政策與輸出清理層，不能各自實作工具行為。

## Node Agent 執行鏈路

```text
CLI
  → config + encrypted secret store
  → folder-scoped runtimes
  → MCP server routes / management routes
  → toolDispatchers/{workspace,process,git,history,task,runtime}
  → fileTools / processes / gitTools / history / taskTools
  → toolUsage implementation behind toolUsage/contract + process/session state

Shared Svelte UI（`CTMCP_UI_HOST=node`，base `/ui`）
  → NodeBackend
  → `/admin/api/*`（loopback、admin token、same-origin）
  → management routes
```

Node Agent 是 headless MCP 產品，不實作 Tauri shell、Actions/OpenAPI、FRP/Cloudflare 程序與 Desktop runtime/port supervisor；這些差異記錄在 parity manifest 的 `intentional_divergences`。

## Built-in WSS 鏈路

```text
Public HTTPS client
  → reverse proxy
  → services/tunnel-server
  → coding-tools-tunnel-v4 WebSocket workers
  → Desktop builtin tunnel 或 Node BuiltinTunnelManager
  → local MCP / Actions listener
```

`crates/tunnel-protocol` 是 Desktop 與 Tunnel Server 的 Rust 協定來源。Desktop 的 `tunnel/builtin/pool_policy.rs` 只負責 worker pool 計畫、scale reason、reconnect jitter 與 recycle 判斷；`tunnel/builtin/metrics.rs` 擁有公開 snapshot、availability derivation、原子計數器、policy/pool/recycle/error 更新與 connected-worker RAII guard，公開型別由 parent re-export；`tunnel/builtin/request_mapping.rs` 只負責 request DTO、路徑、local HTTP builder 與 response header 映射；`tunnel/builtin/protocol_io.rs` 只負責 WebSocket client types、control codec、heartbeat、bounded close handshake 與基本 control frame 收送；`tunnel/builtin/connection.rs` 只負責 WSS request headers、connect timeout、accepted subprotocol 檢查、Challenge／Authenticate／HelloAck 認證與 initial policy 驗證。`tunnel/builtin.rs` 保留 enrollment identity acquisition、認證後的 policy publication → Ready → connected 狀態順序、worker/task lifecycle、async forwarding、response streaming、policy-update select loop 與 cancellation 所有權。Node Agent 透過 parity fixtures／assertions 對齊 enrollment、身份、worker policy、heartbeat、取消與 request forwarding 行為。

## 持久化與安全邊界

- Desktop profile 資料由 `src-tauri/src/data/` 管理；Windows secrets 使用 DPAPI，Unix 使用 AES-256-GCM 與權限受限的本機 key。
- Node Agent 使用 `agent.json`／`workspace-profiles.json` 與 `agent-secrets.enc.json`（AES-256-GCM）；設定 API 不回傳明文 secrets。兩邊設定文件尚未共用，見 [shared-workspace-config](../todo/shared-workspace-config/README.md)。
- Workspace 路徑 containment、命令政策、敏感輸出 redaction、mutation admission 與 retained session 都有 Rust／Node parity assertions。
- Tunnel Server 以 SQLite 保存 enrollment、client 與管理資料，runtime logs 可寫入持久化資料目錄。

## 主要架構邊界

| 邊界 | 所有權 |
| --- | --- |
| Tool schema/catalog | Rust registry 為來源，Node 產生 catalog 並由 parity checker 驗證 |
| Tool dispatch | Desktop `tools/dispatch.rs`；Node `toolDispatchers/*` |
| Process execution | Desktop `tools/exec.rs` + `tools/exec/{identity,lifecycle,native_diagnostic,post_check,result,runner,spec,tests}.rs` + `tools/session.rs` + `tools/session/{attachment,construction,control,lifecycle,output,process_lifecycle,registry,snapshot,tests}.rs`；Node `processes.ts` + `processes/{commandGraph,output}.ts` |
| Patch/edit recovery | Desktop `tools/patch.rs` + `tools/patch/{apply_ops,edit_ops,file_ops,hunk,parser,precise_edit,proposal,support,transaction}.rs`；Node `fileTools.ts` + `editRecovery.ts` |
| Telemetry | Desktop `mcp/telemetry.rs` + `tools/tool_usage.rs`；Node `toolUsage/contract.ts` public surface + `toolUsage.ts` implementation |
| Node runtime contracts | `conversation/contract.ts` 定義 routing/store surface；`state/contract.ts` 定義 task/change/operation model 與 durable store surface；concrete owners 留在 `conversation.ts`／`state.ts` |
| Node dispatch contract | `toolDispatch/contract.ts` 定義 domain handler request/callback；`toolDispatch.ts` 保留 registry/facade 與 public type re-export |
| Tunnel protocol | `crates/tunnel-protocol` + Desktop `tunnel/builtin/{connection,endpoint,identity,metrics,pool_policy,request_mapping,protocol_io}.rs` + Rust/Node behavior fixtures |
| Node management runtime contract | `management/runtimeContract.ts` 定義 hot-apply target 與 tunnel controller；`management/types.ts` re-export public types，不讓 `configStore.ts` 反向依賴 aggregate types |
| UI server state | 共用 Svelte stores／route state；host 差異由 `FrontendCapabilities` 閘道 |
| Workspace config | 目前 Desktop DataStore 與 Node `agent.json` 分開；共用文件見 [shared-workspace-config](../todo/shared-workspace-config/README.md) |

## 已模組化與剩餘熱點

Rust tunnel 目前另已抽出 authenticated connection、metrics/snapshot、pool policy、request mapping、protocol I/O 與 regression tests；production helpers 與外部測試模組都維持在 parent-owned worker/cancellation lifecycle 之外。

Node management 的 `RuntimeHotApplyTarget` 與 `TunnelRuntimeController` 位於純 `management/runtimeContract.ts`；`management/types.ts` 保留公開 re-export 與含 `ConfigStore` 的 aggregate options。`configStore.ts` 只依賴純 runtime contract，因此不再與 aggregate types 形成雙向 import。

Node telemetry 的 request timing、tool/async-session input 與 store surface 位於無 import 的 `toolUsage/contract.ts`；`ToolContext` 只持有 `ToolUsageStoreContract`，`toolUsage.ts` 實作 contract 並 re-export 原有 public types。這讓 catalog/runtime metadata 可以單向使用 shared types，不再透過 concrete telemetry store 形成循環；log writer、aggregation、redaction 與 persistence 仍由 `toolUsage.ts`／`toolUsage/logStore.ts` 擁有。

Node conversation identity、mutable routing maps 與 store surface 位於無 import 的 `conversation/contract.ts`，`ConversationStore` 保留 persistence、LRU 與 routing state 並明確實作 contract；`ToolContext` 不再依賴 concrete store。task/change/operation models 與 durable store surface 位於無 import 的 `state/contract.ts`，`StateStore` 保留 JSON／JSONL persistence ownership。domain handler request、resume callback 與 handler map 位於 type-only `toolDispatch/contract.ts`；permission 及所有 domain handlers 直接依賴 contract，`toolDispatch.ts` 只組合 registry 並保留原 public type re-export。這三個邊界解除 `conversation ↔ types`、`state ↔ types` 與 `permissionTools ↔ toolDispatch` 循環，不改 runtime owner 或呼叫順序。

Node retained `exec_many` command graph 的 state、fingerprint/dedupe、capacity retention、DAG scheduling、cancel/forget/status、snapshot/result shaping 與 restart abort ownership 位於 `processes/commandGraph.ts`。Windows native launch 的 PATH/PATHEXT program resolution、`.cmd`/`.bat` wrapping、PowerShell script invocation 與 verbatim-argument policy 位於純 `processes/nativeLaunch.ts`，`processes.ts` 僅保留 facade re-export 與呼叫。 單一 process execution 的 command fingerprint、safe auto-dedupe 判定與 Cargo target resource-lock derivation 位於純 `processes/identity.ts`；它只計算 identity/lock metadata，不取得 operation lock、session registry、process startup 或 lifecycle ownership。 Command timeout 的數值 bounding、config maximum、長任務預設判定位於純 `processes/timeoutPolicy.ts`；`processes.ts` 只在 post-check、start/wait/yield 等流程消費計算結果。 Process tool error type 與 startup failure normalization 位於純 `processes/errors.ts`，`processes.ts` 維持 facade re-export 並在 spawn/buffered/command-graph 邊界消費，不讓 error mapping 取得 session/process lifecycle ownership。 Process environment 的 host merge、explicit env pairs 與 remove-env normalization 位於純 `processes/environment.ts`，post-check、WSL、sandbox 與 native launch 共用同一規則。 Retained process session 的 lookup、operation/fingerprint resolution、finalized retention/prune、attachment touch 與 registry removal 位於 `processes/sessionRegistry.ts`；detached timeout 的 kill/wait、finalization 與 child-process lifecycle 仍由 `processes.ts` 單一持有。 Harness operation attach 與 finalized-session 到 durable operation record 的投影位於 `processes/harnessTracking.ts`；它只讀 finalized session 與寫 operation state，不設定 finalizedAt、不釋放 process lock，也不取得 child/session lifecycle ownership。 Child stdout/stderr 的 drain 等待位於純 `processes/childStreams.ts`；它只觀察 stream end/close/error，不終止 child、不更新 session，也不參與 finalization。 Retained process 的 follow-up `next_actions` 投影收斂在既有 `processes/output.ts`，由 lifecycle owner 傳入 wait timeout ceiling；output helper 只建構 recovery view，不執行 wait 或取得 session lifecycle ownership。 Process post-check 的 command resolution、sandbox wrapping、結果裁切與驗證聚合位於 `processes/postChecks.ts`；實際 buffered child execution 與 session finalization 仍由 `processes.ts` 以窄 dependency 注入，因此 process/session lifecycle ownership 不變。`processes.ts` 仍以窄 dependency contract 注入 `startProcess`、`waitForSession`、`killProcessTree` 與 error normalization；child-process startup、session lifecycle、tree termination 與 shutdown wait ownership 因此只有 `processes.ts` 一份，command-graph/native-launch helper 都不得取得 lifecycle ownership。

已拆出的子模組包含 schema domains、MCP listener lifecycle/routes、telemetry log writer、session construction/attachment/registry/admission/control/process-lifecycle/finalization/output/snapshot/tests、exec request/operation-admission/identity/main-process lifecycle/native-diagnostic/post-check/command specification/platform runner/result shaping/tests、patch apply/edit/file-operations/hunk/parser/precise-edit/proposal/support/transaction、file-action transaction、exec-many dispatch、dispatch operation tracking/Harness bookkeeping、tunnel endpoint/identity，以及 Node management/server/tool dispatchers。Rust `tools/session.rs` 保留唯一的 `SessionStore`／`SessionRegistry`／`ExecSession` state、finalization/status primitives 與 snapshot/public tool facades；`ExecSession` constructors、Windows process-tree attach 與 execution identity/active-slot/sensitive-output/telemetry builders 位於同型別 extension impl `tools/session/construction.rs`，harness correlation、operation identity getters 與 detach/reattach generation 位於 `tools/session/attachment.rs`，`SessionStore` constructors、active-slot admission、session/index CRUD 與 retention-pruning coordination 位於 `tools/session/registry.rs`，read/resolve/list/wait/input/kill request handlers 位於 `tools/session/control.rs`，stdout/stderr readers、exit waiter、status recording、kill fallback、platform termination 與 bounded change waits 位於 `tools/session/process_lifecycle.rs`，stream buffering/encoding/pagination 位於 `tools/session/output.rs`，retention pruning 與 finalization side effects 位於無狀態的 `tools/session/lifecycle.rs`，delta batching、retained-stream snapshot、status/result payload 與 sensitive-output redaction 位於無狀態的 `tools/session/snapshot.rs`，回歸 suite 位於 `tools/session/tests.rs` 並維持 `tools::session::tests` namespace。attachment/construction/registry/process_lifecycle 不得定義第二份 store/registry/session state，其他 helper 也不得另建 background process owner。Rust `tools/exec/request.rs` 擁有 workspace/scope/spec/post-check/output/runtime-option request resolution，且不得存取 session 或 lock；`tools/exec/admission.rs` 擁有 operation lock、dedupe grace、fingerprint conflict、expired-session removal 與 reattachment，且不得啟動 process；`tools/exec/identity.rs` 擁有 command fingerprint、operation dedupe identity 與 Cargo target resource-lock derivation；`tools/exec/lifecycle.rs` 是唯一 main-process owner，擁有 resource-lock admission、startup permit/loader retry、session registration、request detachment cleanup、stdin handoff、yield/final snapshot、timeout monitor、main-command skip 與 post-check completion；`tools/exec/native_diagnostic.rs` 擁有 allowlisted in-process diagnostics、workspace-safe directory resolution 與 `native_builtin` 回應，且不得建立子程序；`tools/exec/post_check.rs` 擁有共用 startup controller 下的 bounded parallel post-check execution、timeout、output truncation、diagnostics 與結果彙整，但不讀寫 session；`tools/exec/spec.rs` 擁有 command/post-check specs、PowerShell runtime 選擇、program resolution 與 WSL path validation；`tools/exec/runner.rs` 擁有 WSL invocation、stdio/environment、Windows process flags 與 script adapters；`tools/exec/result.rs` 擁有 session capacity metadata、process-start diagnostics、workspace error envelope、execution failure 與 post-check result merge；`tools/exec/tests.rs` 保存回歸 suite 並維持 `tools::exec::tests` namespace；`tools/exec.rs` 只保留 public command/health facade、native fast path、request/admission/lifecycle delegation 與 response boundary metadata。request/admission/identity/native-diagnostic/post-check/spec/runner/result/tests 不得取得第二份 main-process startup permit 或 session lifecycle ownership，public facade 也不得重新內嵌 request/admission/lifecycle implementation。`patch::{apply_patch,patch_check,edit_file,edit,edit_many,file_ops}` 保留穩定公開 facade；unified patch preflight/commit orchestration 位於 `tools/patch/apply_ops.rs`，單檔／多檔 edit orchestration 位於 `tools/patch/edit_ops.rs`，transactional create/copy/move/delete/mkdir orchestration 位於 `tools/patch/file_ops.rs`，diff、hash、version guard、replay plan、安全錯誤與 recovery metadata 統一由 `tools/patch/support.rs` 提供。

`tools/patch.rs` 已收斂為 41 行公開 facade，跨 domain 回歸測試移至 `tools/patch/tests.rs`；`tools/exec.rs` 的 996 行測試已移至 `tools/exec/tests.rs`，request resolution 位於 126 行的 `tools/exec/request.rs`，operation dedupe/reattach 位於 113 行的 `tools/exec/admission.rs`，293 行的單一 main-process owner 位於 `tools/exec/lifecycle.rs`，164 行的純 identity helper 位於 `tools/exec/identity.rs`，99 行的零子程序快速路徑位於 `tools/exec/native_diagnostic.rs`，140 行的 session-free executor 位於 `tools/exec/post_check.rs`，production orchestration facade 現為 233 行；`tools/session.rs` 的 329 行測試已移至 `tools/session/tests.rs`，122 行的 construction/builder extension 位於 `tools/session/construction.rs`，52 行的 attachment/correlation extension 位於 `tools/session/attachment.rs`，169 行的 store registry/admission extension 位於 `tools/session/registry.rs`，239 行的 child-process extension 位於 `tools/session/process_lifecycle.rs`，retention/finalization helper 位於 135 行的 `tools/session/lifecycle.rs`，snapshot read model 位於 404 行的 `tools/session/snapshot.rs`，production state owner 現為 261 行。Rust `tools/dispatch.rs` 的 operation result summary、Harness operation/event tracking 與 standalone/harness status enrichment 已移至約 308 行的 `tools/dispatch/tracking.rs`，原 `tools::dispatch::operation_result_summary` 路徑由 parent re-export 保持穩定；`dispatch.rs` 保留 routing、policy、admission 與 domain orchestration。Node `processes.ts` 現保留 child-process/session lifecycle 與 command-graph/native-launch facade，retained graph orchestration 位於 `processes/commandGraph.ts`，native launch policy 位於 `processes/nativeLaunch.ts`，single-process identity/resource-lock derivation 位於 `processes/identity.ts`，timeout policy 位於 `processes/timeoutPolicy.ts`，process/startup error mapping 位於 `processes/errors.ts`，environment normalization 位於 `processes/environment.ts`，retained session registry 位於 `processes/sessionRegistry.ts`，harness operation tracking 位於 `processes/harnessTracking.ts`，child stream drain helper 位於 `processes/childStreams.ts`，retained recovery action projection 位於 `processes/output.ts`，post-check orchestration 位於 `processes/postChecks.ts`。較大的核心協調檔仍包括 `tunnel/builtin.rs`、`tools/file_action.rs`、`tools/dispatch.rs`、Node `processes.ts`、`fileTools.ts` 與 `toolUsage.ts`。後續拆分必須保持公開入口與 parity contracts 穩定，且不得把 process/session lifecycle ownership 搬入 tracking 或 command-graph helper。

Rust Built-in tunnel 的純 worker policy 位於 215 行的 `tunnel/builtin/pool_policy.rs`；109 行的 `tunnel/builtin/metrics.rs` 負責公開 snapshot、availability derivation、原子 metrics 與 connected-worker guard；104 行的 `tunnel/builtin/request_mapping.rs` 負責 incoming request DTO、Actions/MCP 路徑、local HTTP RequestBuilder 與 response headers；116 行的 `tunnel/builtin/protocol_io.rs` 負責 WebSocket types、control codec、heartbeat、bounded close handshake 與基本 control I/O；124 行的 `tunnel/builtin/connection.rs` 負責 authenticated WSS connection handshake 與 initial policy validation。1,114 行的 `tunnel/builtin.rs` 透過 `pub use metrics::BuiltinTunnelSnapshot` 保留公開路徑，並持有 enrollment identity acquisition、policy publication → Ready → connected handoff、worker/task lifecycle、policy updates、async forwarding、response streaming 與 cancellation；827 行的回歸 suite 已移至 `tunnel/builtin/tests.rs`，測試 namespace 維持 `tunnel::builtin::tests`。helper 模組不得取得 worker/channel/task/select-loop lifecycle ownership；`metrics.rs` 也不得建立 watch/mpsc channel、task、transport、forwarding 或 cancellation owner，`connection.rs` 不得發送 Ready 或發布 policy／connected 狀態。

---
*返回索引：[../project-context.md](../project-context.md)*
