# ACL、完全授权与跨项目种子库修复交接

日期：2026-10-13

## 用户报告

1. Windows AppContainer 在准备项目目录 ACL 时返回
   `SANDBOX_ANCESTOR_GRANT_FAILED`，Git 因此没有启动。
2. 用户不需要「完全授权 / 一般授权 / 严格授权」选择。全部新旧对话都应固定使用完全授权。
3. 旧项目显示 27 个可分配种子。选择另一个工作路径创建项目或对话后，种子库显示 0。
4. 用户要求把调查、方案和进度持续写入仓库，方便其他机器人接手。

## 代码基线

- 工作分支：`vorflux/fix-acl-seed-access`
- 基线：`v0.1.152` / `fb06b1b`
- 注意：`main` 不包含当前种子库和插件实现。本次工作必须基于最新功能分支。

## 已确认根因

### AppContainer / Git

- Windows 命令沙箱在 `prepare_workspace_ancestor_grants` 中为工作目录的每一层父目录临时写入
  AppContainer ACL。
- 任意父目录授权失败都会返回 `SANDBOX_ANCESTOR_GRANT_FAILED`。
- 当前调用方禁止回退，所以 Git 不会以非沙箱方式启动。
- 用户已经明确要求测试电脑固定完全授权。因此完全授权模式不应再启用 AppContainer 命令沙箱。

### 对话授权模式

- 「完全 / 一般 / 严格」是对话级功能，不是工作空间设置。
- 前端入口是 `src/lib/components/chat/AccessMode.svelte` 和 `ChatPanel.svelte`。
- Rust 实现位于 `src-tauri/src/tools/chat_access.rs`。
- Node Agent 对等实现位于 `packages/node-agent/src/chat/access.ts` 和 `chat/store.ts`。
- 旧对话在没有显式字段时默认为 `normal`，所以只隐藏菜单并不能解除限制。

### 新项目种子为 0

- 种子库当前明确按项目目录保存：
  `<项目目录>/docs/chat-sessions/seed-library.seeds.json`。
- 种子 URL、注册表、等待队列和工具调用都绑定 `folder_id`。
- 新路径会创建新的空注册表，所以旧路径的 27 个种子不会出现在新项目。
- 这不是前端计数错误。要满足需求，必须增加跨项目种子发现，并在分配后把工具上下文路由到目标项目。

## 实施目标

- [x] 移除对话授权模式选择和批准卡片入口。
- [x] Rust 与 Node Agent 对全部新旧对话强制返回 `full`，忽略旧的 `normal` / `strict` 值。
- [x] 完全授权配置禁用命令沙箱，避免 AppContainer ACL 失败阻止 Git。
- [x] 保留 AppContainer 代码供未来显式沙箱功能使用，但完全授权路径不调用它。
- [x] 增加跨项目种子发现、分配和目标项目路由。
- [x] 同步 Rust / Node Agent 行为和测试。
- [x] 更新本文件的验证结果、未完成事项和回滚说明。

## 已实施方案

### 固定完全授权

- `WorkspaceProfile::normalize_sandbox()` 现在固定使用 `dangerous`、`advanced` 和不限制的安全策略。
- 完全授权配置固定关闭命令沙箱，因此不会进入 AppContainer 祖先目录 ACL 准备流程。
- Rust 与 Node Agent 的对话访问模式固定为 `full`。旧模式写入会迁移为 `full` 并清空旧批准记录。
- 工具调用不再创建批准请求。支持确认参数的工具会直接收到 `confirm=true`。
- 前端删除授权模式选择和批准卡片组件。

### 全局种子库与目标项目路由

- Rust Desktop 和 Node Agent 都会汇总全部已配置项目的种子注册表。
- 分配过程会扫描全部已配置项目的候选对话，并把目标 `workspace_folder_id` 写入种子记录。
- `chat_open` 和后续项目工具会使用已分配目标项目的运行时与文件系统上下文。
- Node 与 Rust 都使用进程级分配串行化，防止多个源注册表同时占用同一个目标对话。
- Rust gateway 保留旧记录兼容逻辑：旧记录把源目录路径写入 `workspace_folder_id` 时，使用请求路由的项目 ID。

## 验证结果

- Node Agent server build：通过。
- Node seed 与 ProjectGateway 测试：46 通过，0 失败。
- Rust seed 测试：27 通过，0 失败。
- Rust seed HTTP gateway 测试：1 通过，0 失败。
- Rust 旧授权迁移测试：通过。
- Rust 工作空间完全授权与 AppContainer 禁用测试：通过。
- Svelte 类型检查：0 错误，0 警告。
- `git diff --check`：通过。
- `verify:fast` 的 301 个测试中 300 个通过。唯一失败是既有的 Node Agent UI CSP 检查：`management UI CSP allows unsafe-inline`。本次变更未修改该 CSP。

新增回归测试明确验证：旧项目种子在新项目全局清单中可见，能分配新项目对话，并保存目标项目 ID。gateway 测试验证项目工具读取目标项目文件。

## 后续审查修复（c685176 之后）

- 全局 Node runtime 会显式登记全部可路由项目；非全局项目 runtime 会清空共享项目发现，因此 per-project seed endpoint 不能跨项目分配。
- Node 跨项目分配在进程级串行化外，还会在写入前锁定并重新读取目标对话；源注册表只在目标持久化 claim 成功后记录 assignment。
- Node 与 Rust 的 `begin` 路径会将带空 `workspace_folder_id` 的旧活跃 assignment 迁移为源项目 folder ID 并持久化，再路由工具调用。
- 完全授权仍然固定关闭命令沙箱；项目 gateway 的逻辑 scope 不得重新启用 AppContainer。
- Rust 与 Node 在跨项目 claim 前都会锁定并重新读取目标对话，避免覆盖目标项目中的并发更新。
- Rust per-project listener 和 Node per-project endpoint 只扫描本项目。只有 global connection 可以跨项目分配和路由。
- gateway 回归测试确认逻辑项目边界仍生效，但 `exec_command` 不再重新启用 AppContainer。

审查曾把关闭工作区边界、敏感输出保护和环境保护列为高安全风险。本次没有缩小这项授权，因为用户明确选择对全部新旧工作空间固定使用 `dangerous + advanced` 完全授权。该行为只适合用户指定的测试电脑，发布前应再次确认产品范围。

## 未完成事项

- 尚未在真实 Windows 机器运行 AppContainer/Git UI 端到端验证。
- 尚未用真实的 27 个用户种子数据执行迁移验证。
- 浏览器预览只到达“Sign in to desktop”页面。当前环境也缺少 `glib-2.0.pc`，因此无法完成真实 ChatPanel 视觉验证。
- Node Agent UI CSP 的既有检查仍失败，与本次修改无关。

## 回滚说明

- 若要恢复沙箱和多级授权，请同时回滚工作空间规范化、Rust/Node 访问门控和前端入口。
- 若要恢复项目隔离种子库，请同时回滚全局清单、跨项目分配、`workspace_folder_id` 持久化和 gateway 目标路由。
- 不要只回滚前端。仅恢复前端会造成界面与后端授权行为不一致。

## 调查工具结果

- GitNexus `prepare_workspace_ancestor_grants` 上游影响：低风险，直接影响准备函数和 Windows 测试。
- GitNexus `poll_at` 上游影响：中风险，会影响种子等待、分配、替换和 HTTP seed gateway。
- GitNexus `accessMode` 上游影响：高风险，涉及 Node seed route 和 chat UI 返回模型。
- `availablePools` 的图索引没有解析 Svelte 调用边；文本搜索确认它被 `SeedStart.svelte` 使用。

## 安全与凭据

- 用户在聊天中公开了 GitHub Personal Access Token。
- 不要把该令牌写入代码、配置、日志、命令行或本文档。
- 本次仓库读取使用公开 HTTPS 地址，不需要该令牌。
- 用户应立即在 https://github.com/settings/tokens 撤销并重新生成令牌。

## 接手说明

1. 先运行 `git status --short`，避免覆盖并行工作。
2. 继续更新本文档中的复选框和验证结果。
3. 修改 seed 分配前先检查 Rust 与 Node Agent 对等实现。
4. 不要从 `main` 重新开始；`main` 缺少本次问题涉及的大部分功能。

## 2026-10-13 复查（v0.1.154 之后）

- `v0.1.153` 用户实测仍报 AppContainer ACL 超时；`v0.1.154` 在 `mcp::server::new_state` 强制关闭沙箱。
- 复查发现 `new_state` 之外仍有可重新启用 AppContainer 的路径：
  - `data::connection::enforce_project_isolation` 会设 `sandbox.enabled = true`（全局连接迁移、包导入）。
  - `DataStore::load` 在规范化之后才执行 `sync_shared_live_store`，共享文件与 `mcp_connection` 未规范化；全局配置更新也未规范化。
  - 网关 `projects()` 直接读取原始数据文件。
  - Hub 热应用 `sync_preflighted` 经 `update_from_runtime` 绕过 `new_state`。
- 修复（提交 `fix: keep AppContainer off on every runtime config path`）：
  - `RuntimeToolConfig::normalized` 在 `dangerous` 下强制 `sandbox.enabled=false`，覆盖所有构造与热更新路径。
  - `enforce_project_isolation` 只保留逻辑边界，不再开启沙箱。
  - 加载、全局更新、包导入、网关读取均调用 `normalize_sandbox()`。
- 验证：Rust `cargo test --lib --no-default-features` 731 通过、0 失败。
- 未覆盖：Node Agent 的 AppContainer 后端仍由其自身配置控制（默认关闭）；桌面版报错来自 Rust 路径。
- 待办：推送该提交并发布 `v0.1.155`，再请用户在 Windows 实测。

## 2026-10-14 复查（用户用 v0.1.154 实测仍超时）

- 用户实测：`exec_command` 仍在 `C:\Users\belly` 上执行 AppContainer ACL 辅助程序，15 秒超时，`SANDBOX_ANCESTOR_GRANT_FAILED`。
- 新发现的路径：Hub `preflight` 在保存设置时用原始 `runtime.sandbox` 预热 AppContainer，不检查权限模式。
- 新增修复：
  - `CommandExecutionBoundary::from_config` 增加 `permission_mode` 参数；`dangerous` 时始终为 `PolicyOnly`（最后一道防线，即使配置仍写着 sandbox enabled）。
  - `prewarm_sandbox_backend` 与 Hub `preflight` 在 `dangerous` 下直接跳过。
  - 新测试 `full_access_never_selects_appcontainer_boundary`。
- 验证：`cargo test --lib --no-default-features` 732 通过、0 失败。
- 待办：推送并发布 `v0.1.155`。手动 `icacls` 授权不再需要。

## 2026-10-14 v0.1.155 实测：命令可用，写入丢失与边界拒绝（v0.1.156 修复）

- 用户实测 `v0.1.155`：`node --version` 正常，无 `SANDBOX_ANCESTOR_GRANT_FAILED`。
- 问题 1：`file_ops` 返回 `applied=true`，文件却不存在。
  - 根因：`apply_patch`、`edit`、`edit_many`、`file_ops` 仅在 `verify_write_conflicts=true` 时才提交写入。`SecurityPolicy::unrestricted()` 把它设为 `false`，于是只返回成功、不落盘。
  - 修复：写入只受 `dry_run` 控制；版本冲突检查仅在开关打开时执行。回归测试 `writes_persist_when_conflict_verification_is_disabled`。
- 问题 2：PowerShell 写入工作区内绝对路径被 `WORKSPACE_PATH_PROTECTED` 拒绝。
  - 根因：项目隔离强制 `enforce_workspace_boundary=true`；关闭沙箱后，`command_contains_external_path` 把任何 `C:/` 绝对路径都视为外部路径。
  - 修复：`dangerous` 下项目隔离只保留路由，不再开启路径边界（`context.rs`、`connection.rs`）。
- 问题 3（同类）：项目隔离隐藏 Git、格式化等工具，并关闭项目钩子与扩展 MCP。
  - 修复：`dangerous` 下网关与 `tools/list` 暴露全部工具，`project_scope::denied` 放行，不再调用 `workspace_features::require_project_isolation`。
- 受限模式（如 `trusted`）保留原有边界与工具范围，有测试覆盖。
- 验证：`cargo test --lib --no-default-features` 733 通过、0 失败。

## 2026-10-14 完全访问权限复查（v0.1.157）

逐项检查 `SecurityPolicy::unrestricted()` 下仍生效的限制：

| 限制 | 完全访问下状态 |
| --- | --- |
| AppContainer 沙箱 / ACL 授权 | 关闭（执行边界、预热、配置构造三层强制） |
| 命令白名单、危险命令确认、shell 确认、网络命令拦截 | 关闭 |
| 工作区边界（命令写外部路径、`filesystem_scope=host`、绝对 `workdir`） | 关闭 |
| 项目隔离隐藏工具、关闭钩子与扩展 MCP | 关闭（仅保留路由） |
| 写入确认、冲突校验、资源上限、输出脱敏、环境变量保护 | 关闭；写入始终落盘 |
| `.git/.github` 删除保护（命令） | 关闭 |
| 工具档位 | `advanced`（全部工具） |

v0.1.157 新修复：文件工具（`read_file`、`file_ops`、`edit` 等）原来拒绝所有绝对路径。现在完全访问下，指向工作区内部的绝对路径会自动转为相对路径。测试 `full_access_accepts_absolute_paths_inside_the_workspace`。

仍保留的设计限制：
- 文件工具只能操作当前工作区；工作区外文件用 `exec_command` 加 `filesystem_scope="host"`。
- 文件工具不写 `.git/.github`（`PROTECTED_PATH`）；用 Git 工具或命令。
- `shell="none"` 不支持 `&&`、重定向等语法；需指定 `shell`（`powershell`/`cmd`/`sh`）。

验证：`cargo test --lib --no-default-features` 734 通过、0 失败。
