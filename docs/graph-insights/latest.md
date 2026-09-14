# 專案圖譜洞察

更新時間：2026-08-11
本文件索引資料快照：2026-08-11 18:52:42（UTC+08:00）

## 索引狀態

- Repository：`coding-tools-mcp`
- Branch／content：`main`／本文件所在提交的完整 working-tree snapshot
- GitNexus：`1.6.9`
- 索引範圍：566 files、48,340 nodes、117,585 edges、521 clusters、300 execution flows
- PDG：已啟用；CFG、REACHING_DEF 與 interprocedural taint summary 已建立
- Embeddings：0
- 狀態：`up-to-date`

本次以 `node .gitnexus/run.cjs analyze --force --pdg --index-only --wal-checkpoint-threshold 67108864` 完整重建。精確 symbol 查詢可解析 public patch facades、patch domain 子模組、session construction/attachment/control/output/process-lifecycle/registry/finalization/snapshot，移入 `session/construction.rs` 的 `ExecSession::{new,new_with_mode,new_with_mode_and_checks,with_execution_identity,with_active_slot,with_sensitive_output,with_telemetry}`、移入 `session/attachment.rs` 的 `ExecSession::{attach_harness_operation,operation_id,command_fingerprint,touch_attachment,mark_detached,is_still_detached}`、移入 `session/process_lifecycle.rs` 的 child-process methods，以及移入 `session/registry.rs` 的 `SessionStore` registry/admission methods exact UIDs；`tools/exec.rs::{exec_command_async,exec_health_check} → tools/exec/identity.rs` 的 dedupe/resource-lock identity 邊界、`exec_command_async → tools/exec/native_diagnostic.rs::run_native_diagnostic → list_directory` 的零子程序快速路徑、`tools/exec.rs::{exec_command_async,exec_health_check} → tools/exec/lifecycle.rs::run_command → spawn_lifecycle_monitor` 的單一 main-process lifecycle 邊界、`spawn_lifecycle_monitor → tools/exec/post_check.rs::run_post_checks → run_post_check` 的 post-check 邊界、`lifecycle::run_command → tools/exec/{runner,result}.rs → platform/wsl.rs` 的跨檔直接呼叫鏈、`run_connected_worker → connection::connect_authenticated_worker → {protocol_io::receive_control,auth_signing_payload,connection::unix_ms}` 的 authenticated WSS 邊界、`run_worker_pool → metrics::{set_policy,set_pool_counts}` 的 metrics 更新邊界，以及 `tools/{exec,session}/tests.rs` 與 `tunnel/builtin/tests.rs` 內保留的原測試 namespace 亦可解析。Node `ConversationStoreContract`、`StateStoreContract` 與 `ToolDispatchRequest` 也取得 exact UIDs，前兩者的 concrete `implements` edge 與 dispatch contract 的 8 個直接 importers 均可解析。

## 已知索引限制

- LadybugDB FTS extension 在目前 Windows 環境無法載入；`--repair-fts` 即使允許自動安裝仍失敗。
- 因此 `context`、`impact`、`detect_changes`、graph/PDG 可用，但 BM25/keyword `query` 會降級或回傳空結果。
- GitNexus 1.6.9 可建立跨檔 `impl ExecSession` method UIDs，但 incoming caller edges 重建不一致；例如 `run_command → new_with_mode_and_checks` 可解析，部分 attachment/builders 仍無 incoming edges，其 caller 邊界另以原始碼引用、結構檢查與 exec/session runtime tests 驗證。
- 搬移至 `tunnel/builtin/protocol_io.rs` 的 async send helpers 仍有跨檔 incoming edge 遺漏；新 `connection::connect_authenticated_worker` 的 parent incoming edge 可精確解析，但其中 `send_control` outgoing edge 未被重建。exact UID、parent 原始碼引用、UI structural checker 與 tunnel runtime tests 共同作為邊界證據。
- `metrics.rs` 的 `set_policy`／`set_pool_counts` incoming edges 可回到 `run_worker_pool`，但 GitNexus 1.6.9 未重建 `supervisor.rs → BuiltinTunnelSnapshot::availability_state` 與 `run_connected_worker → ConnectedWorkerGuard::new` 的跨檔 caller edge；原始碼引用、結構檢查與 targeted/full runtime tests 補足這兩個邊界。
- Tunnel Server 有 11 個大型／複雜函式無法完成 CDG reverse-reachability；CFG 與 REACHING_DEF 未受影響。

## 產品與執行面

```text
Desktop Svelte UI
  → Tauri commands
  → DataStore + RuntimeSupervisor + TunnelSupervisor
  → MCP / Actions listeners
  → shared Rust tool dispatch

Node React PWA
  → management API + React Query
  → folder-scoped Node runtimes
  → MCP dispatcher + domain toolDispatchers

Public HTTPS
  → Rust Tunnel Server
  → coding-tools-tunnel-v4 workers
  → Desktop 或 Node local listener
```

## 核心圖譜觀察

### Tool dispatch 是高影響入口

Rust `call_tool_async` 的直接 production callers 包含：

- `src-tauri/src/mcp/server.rs::handle_tools_call_async`
- `src-tauri/src/actions/listener.rs::execute_action`

其下游直接進入 mutation lock、async dispatch 與 output redaction。此區重構必須保留單一入口與 MCP／Actions 一致性。

### Patch orchestration 與核心 engines 已抽離

`apply_patch`、`patch_check`、`edit_file`、`edit`、`edit_many` 與 `file_ops` 保留原 public facade。unified patch preflight/commit orchestration 位於 `tools/patch/apply_ops.rs`，單檔／多檔 edit orchestration 位於 `tools/patch/edit_ops.rs`，unified/Codex patch parsing 位於 `tools/patch/parser.rs`，hunk location/preflight/application 位於 `tools/patch/hunk.rs`，precise edit contract/matching/application 位於 `tools/patch/precise_edit.rs`，proposal store/validation/restricted patch 位於 `tools/patch/proposal.rs`，transaction 位於 `tools/patch/transaction.rs`，transactional file operations orchestration 與 hash precondition 位於 `tools/patch/file_ops.rs`。diff、hash、version guard、replay plan、安全錯誤與 recovery metadata 位於 `tools/patch/support.rs`；domain 子模組不再反向依賴 facade 內 helper，`tools/patch.rs` 只保留 41 行 public facade／test module declaration，回歸測試實作位於 `tools/patch/tests.rs`。

### Tunnel identity、endpoint、authenticated connection、metrics、policy、request mapping、protocol I/O 與 tests 已分離

Rust `tunnel/builtin/pool_policy.rs` 集中 worker pool 計畫、scale up/down reason、connecting 上限、burst warm floor、reconnect backoff/jitter 與 worker recycle 判斷；109 行的 `tunnel/builtin/metrics.rs` 集中 public snapshot、availability derivation、原子 counters、policy/pool/recycle/error 更新與 connected-worker RAII guard，`BuiltinTunnelSnapshot` 由 parent re-export；`tunnel/builtin/request_mapping.rs` 集中 incoming request DTO、相對路徑轉換、local HTTP request builder 與 response header 清理；`tunnel/builtin/protocol_io.rs` 集中 WebSocket client types、control JSON codec、heartbeat deadline、bounded close handshake，以及基本 control frame 收送；`tunnel/builtin/connection.rs` 集中 WSS request headers、connect timeout、accepted subprotocol validation、Challenge／Authenticate／HelloAck 與 initial policy validation。`metrics.rs` 不建立 channel/task/transport、forwarding、streaming、select loop 或 cancellation owner，`pool_policy.rs`／`request_mapping.rs` 不持有 WebSocket、enrollment、channel、task、streaming 或 cancellation lifecycle，`protocol_io.rs` 不持有 worker policy、enrollment、forwarding、channel、task、streaming 或 cancellation lifecycle，`connection.rs` 不持有 enrollment acquisition、policy publication、Ready、connected state、worker/task、forwarding、streaming 或 cancellation lifecycle。原本內嵌的 17 個 parent-level 回歸案例已移至 `tunnel/builtin/tests.rs`，另有 2 個 helper-module tests，targeted suite 仍是 19 項且 namespace 維持 `tunnel::builtin::tests`。`tunnel/builtin.rs` 現為 1,114 行，保留公開 snapshot 路徑，以及唯一的認證後 policy publication → Ready → connected handoff、worker/task lifecycle、async forwarding、response streaming、policy update select loop 與 cancellation owner。Node `BuiltinTunnelManager` 的行為由既有 worker-policy/forwarding/parity assertions 對齊，本次是 Rust 內部邊界調整，沒有新增協定差異。

### Node dispatch 已按 domain 拆分

Node `toolDispatchers/` 已分成 workspace、process、git、history、task、runtime；Rust `tools/dispatch.rs` 仍是較集中的 facade/orchestrator。Node 的 domain 邊界可作為 Rust 後續模組化參考，但不能直接複製刻意排除的產品功能。

Node management 的 hot-apply `RuntimeHotApplyTarget` 與 `TunnelRuntimeController` 已移至純 `management/runtimeContract.ts`。`management/types.ts` 保留 public re-export 與 `ConfigStore` aggregate options，`configStore.ts` 則直接依賴 runtime contract；此方向解除原本的 config-store/types 循環且不改公開型別入口。

Node telemetry 的 public request/input/store surface 已移至無 import 的 `toolUsage/contract.ts`。`ToolContext` 只依賴 `ToolUsageStoreContract`，`toolUsage.ts` 保留實作並 re-export 原有 public types；完整重索引可精確解析 contract 的 8 個 methods、`ToolUsageStore implements ToolUsageStoreContract`，以及來自 `types.ts`／`toolUsage.ts` 的單向 imports。這解除原本 `catalog → types → toolUsage → toolRuntime → catalog` 循環，且不改 telemetry runtime、檔案格式或查詢行為。

Node conversation、durable state 與 domain dispatch 的型別所有權已分別移至 `conversation/contract.ts`、`state/contract.ts`、`toolDispatch/contract.ts`。前兩者無 imports，`ConversationStore`／`StateStore` 明確實作 contract，`ToolContext` 只依賴 store surface；dispatch contract 只有 type-only imports，permission 與六個 domain handler 模組不再反向依賴 registry facade。`conversation.ts`、`toolDispatch.ts` 與 `types.ts` 保留原 public type re-export，runtime state、persistence、permission decision 與 handler registry owner 均未搬移。

### Session construction、attachment、registry、control 與 lifecycle/read model 已分離

Rust `tools/session.rs` 仍持有唯一的 `SessionStore`／`SessionRegistry`／`ExecSession` state、finalization/status primitives 與 snapshot/public tool facades。`tools/session/construction.rs` 以同一個 `ExecSession` extension impl 集中 constructors、Windows process-tree attach、stdin/watch/state initialization 與 execution identity/active-slot/sensitive-output/telemetry builders；完整重索引可精確解析 `run_command → new_with_mode_and_checks → unix_timestamp_ms`。`tools/session/attachment.rs` 集中 harness correlation、operation identity getters 與 detach/reattach generation，並保留 finalized session 補記 harness terminal record 的行為。`tools/session/registry.rs` 集中 store constructors、active-slot admission、session/index CRUD 與 retention-pruning coordination；`tools/session/process_lifecycle.rs` 集中 stdout/stderr reader tasks、exit waiter、status refresh/record、kill fallback、platform termination 與 bounded change waits。以上 extension 都不定義第二份 state struct。`tools/session/lifecycle.rs` 是無狀態 finalization helper，集中 retention pruning、idempotent finalization、active-slot release、telemetry 與 harness terminal recording；`tools/session/snapshot.rs` 集中 delta event batching、retained stream snapshot、encoding-aware payload shaping、status/result fields 與 sensitive-output redaction。public snapshot/control facades 與原 `tools::session::tests` namespace 保持不變。production state owner 現為 261 行，construction/builder extension 為 122 行，attachment/correlation extension 為 52 行，registry/admission extension 為 169 行，process-lifecycle extension 為 239 行，snapshot read-model helper 為 404 行，finalization helper 為 135 行。

### Exec request、operation admission 與 process lifecycle 已分離

Rust `tools/exec/request.rs` 集中 workspace/cwd resolution、filesystem scope validation、command/post-check specification、output options，以及 timeout/yield/TTY/stdin/sensitive-output request parsing；它不存取 session、operation lock 或 process startup。`tools/exec/admission.rs` 集中 operation-id lock、30 秒 automatic dedupe grace、command fingerprint conflict、expired-session removal 與 retained-session refresh/reattach；它不取得 startup permit、不 spawn process，也不建立 session。`tools/exec/identity.rs` 集中 `ExecutionIdentity`、command fingerprint、operation dedupe identity、Cargo target lock group/target 與 normalized lock path 計算。`tools/exec/native_diagnostic.rs` 集中 `pwd`、`ls`／`dir`、`which` 與 `echo` 的 allowlisted in-process 快速路徑、workspace path resolution、directory errors 與 `native_builtin` 回應。`tools/exec/lifecycle.rs` 仍是唯一的 main-process lifecycle owner，集中 resource-lock admission、startup permit/loader retry、session registration、request detachment cleanup、stdin handoff、yield/final snapshot、timeout monitor、main-command failure skip 與 post-check completion；`run_command` 仍直接呼叫 `prepared_command` 並透過 `process_start_workspace_error → process_start_error_json` 保留 recovery contract。`tools/exec/post_check.rs` 集中最多四路的並行 post-check、單項 timeout、bounded stdout/stderr、startup diagnostics 與結果彙整；每個 post-check 仍透過共用 `spawn_with_control` 啟動，模組不讀寫 `ExecSession`。`tools/exec/spec.rs` 擁有 `ExecSpec`／`PostCheckSpec`、PowerShell runtime 選擇與 UTF-8 script 包裝、structured command parsing、workspace-local executable policy，以及 WSL path validation。`tools/exec/runner.rs` 擁有 WSL invocation、cwd/env/stdin/stdout/stderr 設定、Windows no-window/process-group flags，以及 `.cmd`／`.ps1` runner escaping。`tools/exec/result.rs` 統一 session capacity metadata、process-start diagnostic JSON、workspace error envelope、execution failure result 與 post-check result merge。`tools/exec.rs` 現為 233 行 public orchestration facade，保留 public command/health entry、native fast path、request/admission/lifecycle delegation 與 response boundary metadata；request helper 為 126 行、operation admission helper 為 113 行，單一 lifecycle owner 仍為 293 行。原本內嵌的 996 行 `#[cfg(test)]` suite 位於 `tools/exec/tests.rs`，測試路徑仍是 `tools::exec::tests::*`。

## 結構檢查

`gitnexus check --cycles --json` 回傳 `cycleCount: 0`。原 `conversation.ts ↔ types.ts`、索引後補顯露的 `state.ts ↔ types.ts`，以及 `permissionTools.ts ↔ toolDispatch.ts` 已由上述三個 contract 解除；management、telemetry/catalog 與 Rust context/session/dispatch 的既有循環也維持已解除狀態。

## 優先維護建議

1. `tools/session.rs` 已收斂為 state/public facade owner；後續優先維持 construction/attachment/registry/process-lifecycle extension 的單一 state ownership，不再為縮行數搬移零散 accessors。
2. 將 `tools/exec.rs` 視為穩定 public orchestration facade；維持 request session-free、operation admission process-start-free，且 `tools/exec/lifecycle.rs` 必須是唯一 main-process startup retry、cancellation 與 session-registration owner。
3. 維持 `tunnel/builtin/{connection,metrics,pool_policy,request_mapping,protocol_io}.rs` 的窄邊界；`metrics.rs` 只持有 snapshot/counters/guard 並由 parent re-export public snapshot，`connection.rs` 只回傳 authenticated transport 與 initial policy，policy publication／Ready／connected handoff 必須留在 parent。若後續拆 async forwarding，仍須讓 `builtin.rs` 保有單一 worker/cancellation lifecycle owner，且不得讓 helper 取得 channel/task/select-loop ownership。
4. 視成長量再拆 `tools/patch/tests.rs`；Node import graph 已零循環，後續只在 concrete owner 出現新的責任群聚或測試隔離需求時才拆分，不再為縮行數抽零散 methods。
5. 將 PDG 模式固定在 `.gitnexusrc`，並在可用的 LadybugDB FTS 環境重跑 `--repair-fts`。

## 本次驗證

- Full graph + PDG rebuild：成功。
- Rebuild command：`node .gitnexus/run.cjs analyze --force --pdg --index-only --wal-checkpoint-threshold 67108864`。
- Index content/status：本文件所在提交的完整 working-tree snapshot，`complete_with_fts_limit`。
- Exact symbol context：`session/construction.rs::ExecSession.new_with_mode_and_checks` 與 `tools/exec.rs::exec_command_async` 均以 exact epistemic 狀態解析成功。
- Session construction context：移入 `session/construction.rs` 的 constructors/builders exact UIDs，以及 `run_command → ExecSession::new_with_mode_and_checks → unix_timestamp_ms` 已確認；Windows process-tree attach、stdin/watch initialization 與 builder 預設值由原碼等價搬移和 runtime tests 驗證。
- Session attachment context：移入 `session/attachment.rs` 的 harness/identity/detachment methods exact UIDs，以及 `attach_harness_operation → record_harness_operation_finalization` 已確認；跨檔 caller incoming edges 不完整，由結構檢查與 runtime tests 補足。
- Session registry/lifecycle context：移入 `session/registry.rs` 的 `SessionStore::{default,new,insert,acquire_active_slot,get_by_operation,get_by_fingerprint,list,remove}` exact UIDs，以及 `SessionStore::{insert,get_by_operation,get_by_fingerprint,list} → lifecycle::prune_finalized_sessions`、`ExecSession::{complete_post_checks,mark_finalized} → lifecycle::finish_session` 已確認。
- Session extension context：moved method exact UIDs 已確認；`exec/lifecycle.rs`／`session/control.rs → ExecSession::{spawn_readers,spawn_exit_waiter,wait_for_readers,kill_and_wait,wait_for_change}` 與跨檔 callers → `SessionStore` registry/admission methods 由原始碼引用、結構檢查與 runtime tests 驗證。因 GitNexus 1.6.9 的跨檔 impl 限制，部分 method context 不含 incoming edges；state structs 仍只位於 `session.rs`。
- Session snapshot context：`ExecSession::{summary,snapshot,snapshot_with_options,stream_snapshot} → snapshot::{build_summary,build_snapshot,build_snapshot_with_options,capture_stream_snapshot}` 已確認。
- Exec identity context：`exec_command_async`／`exec_health_check → identity::execution_identity → cargo_target_lock` 已確認；startup retry、session registration 與 lifecycle monitor 由單一 `exec/lifecycle.rs` 擁有。
- Exec request context：`exec_command_async → request::resolve_exec_request → {validate_child_process_scope,resolve_exec_spec,resolve_post_checks,OutputOptions::from_args}` 已以 exact UID 確認；request 模組不存取 session 或 lock。
- Exec admission context：`exec_command_async → admission::admit_operation → {resource_lock,SessionStore::get_by_operation,result::merge_exec_result}` 已以 exact UID 確認；operation admission 不含 startup permit、spawn 或 session construction。
- Exec result capacity context：`{exec_command_async,admission::admit_operation} → result::attach_session_capacity` 已以 exact UID 確認。
- Exec native diagnostic context：`exec_command_async → native_diagnostic::run_native_diagnostic → list_directory` 已確認；模組不引用 runner、startup controller 或 session。
- Exec lifecycle context：`exec_command_async`／`exec_health_check → lifecycle::run_command → spawn_lifecycle_monitor` 已確認；request facade 不再包含 process-start permit、loader retry、session creation 或 lifecycle monitor implementation。
- Exec post-check context：`lifecycle::spawn_lifecycle_monitor → post_check::run_post_checks → run_post_check → spawn_with_control` 已確認；main-command skip 與 `complete_post_checks` 由單一 lifecycle owner 協調。
- Tunnel pool-policy context：`pool_adjustment`、`worker_should_recycle` 與 `reconnect_delay` 均以 `pool_policy.rs` exact UID 解析，incoming callers 分別回到 `run_worker_pool`、`run_connected_worker`／`receive_live_control` 與 `worker_reconnect_loop`。
- Tunnel request-mapping context：`forward_request → request_mapping::{prepare_local_request,response_headers}` 與 `prepare_local_request → local_path_for_request` 均以 exact UID 解析；async WebSocket、heartbeat、cancel select loop 與 response streaming 仍只位於 `builtin.rs`。
- Tunnel protocol-I/O context：`protocol_io::{decode_control,close_client_websocket,send_heartbeat,receive_control,send_control}` 均以 exact UID 解析；圖譜可直接解析 `run_connected_worker → {close_client_websocket,receive_control}` 與部分 parent control paths → `decode_control`。GitNexus 1.6.9 對搬移後 async send helpers 的跨檔 incoming edges 不完整，其餘 `send_control`／`send_heartbeat`／`decode_control` callers 由原始碼引用、結構檢查與 19 項 tunnel runtime tests 確認；worker policy、forwarding、streaming 與 cancellation select loop 仍只位於 `builtin.rs`。
- Tunnel authenticated-connection context：`connection::{connect_authenticated_worker,unix_ms}` 與 `builtin.rs::run_connected_worker` 均以 exact UID 查詢；WSS request、subprotocol、Challenge／Authenticate／HelloAck、initial policy validation 位於 `connection.rs`，parent 原始碼與結構檢查鎖定 `connect_authenticated_worker → policy publication → Ready → connected` 順序。worker/task、forwarding、streaming 與 cancellation lifecycle 仍只位於 `builtin.rs`。
- Tunnel metrics context：`BuiltinTunnelSnapshot::availability_state`、`BuiltinTunnelMetrics::{new,set_policy,snapshot}` 與 `ConnectedWorkerGuard::{new,drop}` 均取得 `metrics.rs` exact UID；`run_worker_pool → {set_policy,set_pool_counts}` 可由圖譜直接解析，availability 與 guard 的跨檔 callers 由 parent/supervisor 原始碼引用、結構檢查與 19 項 tunnel runtime tests 確認。公開 snapshot path 仍由 `builtin.rs` re-export，worker/task/transport/cancellation lifecycle 仍只位於 parent。
- Tunnel test-module context：`tests.rs::{single_worker_policy,worker_pool_bootstraps_grows_and_gracefully_shrinks_from_server_policy}` 於完整重建後以 exact UID 檢查；`single_worker_policy` 只有 3 個 integration-test callers，外部測試模組不改 production lifecycle 或公開路徑。
- Node management runtime-contract context：`runtimeContract.ts::{RuntimeHotApplyTarget,TunnelRuntimeController}` 均取得 exact UID；incoming imports 只來自 `management/types.ts` 與 `management/configStore.ts`。圖譜 import 邊為 `configStore → runtimeContract`、`types → runtimeContract`、`types → configStore`，不存在 `configStore → types` 反向邊。
- Node telemetry contract context：`toolUsage/contract.ts::ToolUsageStoreContract` 取得 exact UID；圖譜確認 `ToolUsageStore` 實作此 contract，incoming imports 只來自 `types.ts` 與 `toolUsage.ts`，contract 本身沒有 imports。telemetry structural test 與完整 Node suite 固定 public re-export 及 runtime 行為。
- Node conversation contract context：`conversation/contract.ts::ConversationStoreContract` 取得 exact UID；`ConversationStore` 的 implements edge、`types.ts` 的單向 import，以及 contract 的 16 個 methods 均可解析。contract 無 imports，concrete store 保留 routing、LRU 與 persistence owner。
- Node state contract context：`state/contract.ts::StateStoreContract` 取得 exact UID；`StateStore` 的 implements edge、`types.ts` 的單向 import，以及 contract 的 14 個 methods 均可解析。task/change/operation models 與 store surface 位於無 import contract，JSON／JSONL persistence 仍只位於 concrete store。
- Node dispatch contract context：`toolDispatch/contract.ts::ToolDispatchRequest` 取得 exact UID；incoming imports 為 registry facade、permission handler 與六個 domain handler modules。contract 只有 type-only imports，registry 保留 handler aggregation 與 public type re-export。
- FTS availability：本機 LadybugDB FTS extension 無法載入；索引在無 FTS 模式完成。
- Structural cycle check：完成，`cycleCount: 0`；conversation/types、state/types、permission/dispatch、telemetry/catalog、management config-store/types 與原 Rust context/session/dispatch 循環均已解除。
- Detect changes：staged scope 為 24 files、74 symbols、MEDIUM risk、3 affected execution flows；只落在 `CreateToolContext → OperationFromUnknown`、`CreateToolContext → ConversationSelectionMap` 與 `CreateToolContext → ConversationCwdMap`，沒有進入 permission、task mutation 或 tunnel execution flow。以 exact context、三個結構測試、完整 Node suite 與 parity gates 驗證 public re-export 與 runtime owner 未漂移。
- Root UI parity contract tests：6/6 通過；assertions 已指向 management observability route、session attachment 與 session lifecycle 的實際模組邊界。
- 本輪 `npm run node-agent:verify-repo` 完整通過：Node full suite 288 passed/0 failed/1 live WSL skipped，native binary check、Rust catalog contract、Desktop 0.1.42 compatibility、Node behavioral parity 28/28 與 UI parity 7/7 均通過。conversation/dispatch targeted 11/11、state contract 結構測試與既有 harness suite 19/19 亦通過。

---
*來源：GitNexus 1.6.9、`.gitnexus/meta.json`、目前原始碼、版本來源與 parity manifest。*
