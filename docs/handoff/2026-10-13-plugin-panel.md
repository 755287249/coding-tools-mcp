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

- [ ] A registry 改为内置 import + 容错 + 错误显示
- [ ] B/C Tauri CSP 与 plugin-frame
- [ ] D 导入支持 MCP JSON，对话框内错误
- [ ] E 已添加清单清理
- [ ] F Rust 主动连线
- [ ] G 新增 MCP 服务器（前端 + Rust + Node）
- [ ] 测试：`pnpm test`、`pnpm check`、`cargo check --no-default-features --lib`、Node Agent 浏览器实测（模拟 Tauri CSP 头）
- [ ] PR

## 验证方式（给接手者）

- 前端单元测试：`pnpm test`（`tests/plugin-platform.test.mjs` 等）。
- 类型检查：`pnpm check`。
- Rust（Linux 无 GUI）：`cargo check --manifest-path src-tauri/Cargo.toml --no-default-features --lib`。
- 模拟桌面 CSP：启动 Node Agent，在它前面加一个代理把 `.html` 回应的 CSP 换成 `tauri.conf.json` 的 `csp`，用浏览器打开看插件 iframe 是否显示。
