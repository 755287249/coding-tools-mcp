# 插件板块修复交接（2026-10-13）

分支：`vorflux/plugin-panel-fix`（基于 `vorflux/fix-cloudflared-cwd-and-rollback`，v0.1.149 的插件平台就在那个分支上，`main` 还没有插件代码）。

> 给接手的机器人：先读本文件，按「进度」继续。每完成一步就更新本文件并推送。不要把任何令牌写进仓库。

## 使用者回报的问题

1. 「自定义 → 插件 → 添加 → 应用插件 → 导入插件文件」选 JSON 后什么都没发生，列表也是空的（截图里对话框只有「导入插件文件」，连内置 CodeRabbit 都不见）。
2. 种子库「添加种子」第三张卡「CodeRabbit 插件」打开后整个面板空白。
3. 没有主动添加插件，客户端却已经显示 CodeRabbit 插件。
4. MCP 服务器（例如 `~/.claude.json` 里的 `vorflux-1`）启用后一直「http · 未连接 · 0 个工具」，没有错误提示，也没有办法在介面里新增 MCP 服务器（「MCP 服务器」分页只有一段示例 JSON）。

## 根本原因

| # | 原因 | 位置 |
|---|------|------|
| A | Tauri 正式版 CSP `connect-src` 只有 `ipc:`，`fetch('/plugins/coderabbit-seeds.ctplugin.json')` 被挡；`refreshPlugins()` 用 `Promise.all`，一个失败就整个失败，而且错误被 `void` 吞掉 → `APP_PLUGINS`、`pluginPackages` 一直是空。开发模式 `devCsp` 有 `localhost:1420`，所以开发时看不出来。 | `src/lib/plugins/registry.ts`、`src-tauri/tauri.conf.json` |
| B | Tauri 会把 app CSP 当成 HTTP 头加到**每一个** `.html` 资源（`tauri-2.11.5/src/manager/mod.rs::get_asset`）。CSP 有 `frame-ancestors 'none'` → `plugin-frame.html` 不能被嵌入 → iframe 空白。 | `src-tauri/tauri.conf.json` |
| C | 同上，Tauri 为 HTML 内联 `<style>` 加 nonce，`style-src` 有 nonce 时 `'unsafe-inline'` 失效 → 插件用 JS 插入的 `<style>` 会被挡（插件没有样式）。 | `static/plugin-frame.html` |
| D | 导入失败的错误写在 `feedback`，但应用插件分页的对话框里没有显示 `feedback`（只显示在对话框后面的页面上，被遮住）。另外使用者常选的是 MCP 配置 JSON（`{"mcpServers":{...}}`），不是 `.ctplugin.json`，直接被当成插件包解析失败。 | `FeatureCatalog.svelte` |
| E | 已添加清单存在 localStorage（`ctmcp-app-plugins-v1`）。旧版本把 `coderabbit-seeds` 写进去了，所以没添加也显示。 | `registry.ts` |
| F | 桌面版（Rust）外部 MCP 是「懒连接」：只有 MCP 客户端呼叫 `tools/list` 才连线，所以介面永远显示未连接、0 工具、没有错误。Node Agent 在 inventory 时就会连线。 | `src-tauri/src/workspace_features/mod.rs::extension_inventory` |
| G | 「MCP 服务器」分页没有新增功能，只能手动改文件再按刷新。 | `FeatureCatalog.svelte` |

## 修复方案

- A：内置插件包改为 Vite 直接 `import` JSON（不再 fetch）；`refreshPlugins` 每个来源独立 try/catch，错误存到 `pluginError` store 并在介面显示；CSP `connect-src` 加 `'self'`。
- B：`frame-ancestors 'none'` → `'self'`（只允许本应用自己嵌入）。
- C：`plugin-frame.html` 不放内联 `<style>`，基础样式改由 `plugin-frame.js` 建立。
- D：导入同时支持两种 JSON：`.ctplugin.json`（应用插件）与 MCP 配置（`mcpServers` / `servers` / 单个 `{command|url}`）。错误在对话框内显示。
- E：已添加清单只保留真的存在的插件；内置插件不会自动添加（需要使用者按「添加」）。
- F：Rust `extension_inventory` 在工作区运行时先连线已启用的服务器（最多 25 秒），回传真实的连线状态/错误；工作区未运行时给出提示。
- G：新增「添加 MCP 服务器」：贴上 JSON 或选文件 → 写入目前项目文件夹的 `.mcp.json` → 自动刷新并启用。后端：Rust `add_workspace_mcp_servers`、Node `POST /admin/api/workspaces/{id}/extensions/mcp`。

## 进度

- [x] A registry 改为内置 import + 容错 + 错误显示（`src/lib/plugins/registry.ts`，localStorage key 改 `ctmcp-app-plugins-v2`，新增 `pluginError`、`validatePluginPackage`）
- [x] B/C Tauri CSP 与 plugin-frame（`tauri.conf.json` 的 `connect-src 'self'`、`frame-ancestors 'self'`；`plugin-frame.js` 建立样式并回报 `loaded`/`error`）
- [x] 插件面板不再空白：`PluginFrame.svelte` 追踪状态（loading/loaded/ready/error，12 秒逾时），`PluginSlot.svelte` 显示「载入中 / 未添加（一键添加并打开）/ 载入失败 + 原因」
- [x] D 导入支持 MCP JSON，对话框内显示成功/错误（`src/lib/plugins/mcp-import.ts::detectImport`，`FeatureCatalog.svelte::importValue`）
- [x] E 已添加清单清理（内置插件不再自动添加）
- [x] F Rust 主动连线（`external_mcp::connect_enabled`，25 秒上限；工作区未运行时 error=`workspace-not-running`，UI 翻成 `mcp.notRunning`）；MCP 详情未连线时有「重新连线」按钮
- [x] G 新增 MCP 服务器：对话框「MCP 服务器」分页可贴 JSON 或选文件 → 写入目前项目 `.mcp.json` → 自动打开 MCP 总开关并启用新服务器。后端 Rust `add_workspace_mcp_servers`（`commands/workspace_features.rs::merge_mcp_json`）、Node `POST /admin/api/workspaces/{id}/extensions/mcp`（`management/mcpJson.ts`）
- [x] SeedLibrary：CodeRabbit 卡片只打开浮层，不再把方法切到不存在的 `rabbit` 分页
- [x] 测试：`pnpm test`（2 个失败是本分支原本就有的：`chat-message-actions`、`node-agent-ui-parity`，与本修改无关）、`pnpm check` 0 错误、`cargo check --no-default-features --lib` 通过、新增单元测试 `tests/mcp-import.test.mjs`、`packages/node-agent/test/mcpJson.test.mjs`、Rust `mcp_json_tests`
- [ ] 浏览器实测（模拟 Tauri CSP 头）与真实 Windows Tauri 安装包实测 —— 尚未完成
- [ ] PR：令牌没有 Pull requests 权限，无法自动建立。手动建立：https://github.com/755287249/coding-tools-mcp/compare/vorflux/fix-cloudflared-cwd-and-rollback...vorflux/plugin-panel-fix

## 已知限制 / 后续

- `add_workspace_mcp_servers` 写入的是第一个（或指定的）项目文件夹的 `.mcp.json`；使用者级 `~/.claude.json` 不会被改动。
- 若 `.mcp.json` 已存在但不是合法 JSON，会拒绝写入并显示错误，不会覆盖。
- 前端 `UI` 的「重新连线」其实是重新拉一次 inventory；Rust 端会对已启用但未连上的服务器重试连线。
- `tests/node-agent-ui-parity.test.mjs` 与 `tests/chat-message-actions.test.mjs` 在基底分支本来就失败，未处理。

## 验证方式（给接手者）

- 前端单元测试：`pnpm test`（`tests/plugin-platform.test.mjs` 等）。
- 类型检查：`pnpm check`。
- Rust（Linux 无 GUI）：`cargo check --manifest-path src-tauri/Cargo.toml --no-default-features --lib`。
- 模拟桌面 CSP：启动 Node Agent，在它前面加一个代理把 `.html` 回应的 CSP 换成 `tauri.conf.json` 的 `csp`，用浏览器打开看插件 iframe 是否显示。
