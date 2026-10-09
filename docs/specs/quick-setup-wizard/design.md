# 設計文檔：quick-setup-wizard

## 概述

本設計以 SvelteKit 獨立路由承載五階段 wizard，重用既有 Tauri API 與 `GptQuickCopy`。第一階段先選反向代理來源；連線階段再以小型元件載入軟體與 FRP 設定，避免主頁超過 500 行，且不更動 Rust runtime、工作區資料模型或既有工作區頁。

**對應需求：** FR-1 至 FR-5、NFR-1 至 NFR-4

## 技术方案

### 技術選型

| 類別 | 選擇 | 理由 | 關聯需求 |
|------|------|------|----------|
| 頁面 | SvelteKit `/quick-setup` | 與既有路由一致且流程完全隔離 | FR-1, FR-2, FR-5 |
| 狀態 | 頁面內 Svelte 5 state | wizard 與 secret 都是暫存 UI 狀態 | FR-1, FR-3 |
| 分支表單 | `QuickTunnelSetup.svelte` | 將軟體、FRP 與 Cloudflare 欄位從主流程拆出並控制單檔規模 | FR-3 |
| 後端整合 | 現有 software/settings/workspace/secrets/tunnel/runtime API | 保持單一資料契約與啟停流程 | FR-2 至 FR-4 |
| 完成值 | 現有 `GptQuickCopy` | 已正確區分 MCP/Actions 與密鑰來源 | FR-5 |

### 架構設計

```text
側邊欄「快速設定」
  → /quick-setup
    → 選內建 WSS / FRP / Cloudflare
    → 輸入工作區名稱 → 選一或多個資料夾
      → createWorkspace(第一個資料夾, 名稱)
      → addWorkspaceFolder(其餘資料夾)
    → 選 MCP / Actions
    → QuickTunnelSetup
      ├─ built-in: enrollment URL
      ├─ FRP: list/install frpc + list/create profile + URL/subdomain
      └─ Cloudflare: list/install cloudflared + quick/named fields
    → updateWorkspace + setWorkspaceSecret（依分支）
    → testTunnel
    → startRuntime / startActionsRuntime
    → listWorkspaces 取得最終 public URL
    → GptQuickCopy 顯示 ChatGPT 實際填入值
```

步驟順序採「反向代理 → 工作區 → 服務 → 設定與啟用 → ChatGPT 填寫值」。先選來源可讓使用者先理解是否需要管理員邀請、額外軟體或外部服務；工作區建立後才產生 FRP 的唯一 URL；服務選定後才顯示 MCP 或 Actions 對應欄位。

## 資料模型

不新增持久資料模型。新增的前端 `QuickTunnelDraft` 暫存：

```ts
type TunnelProvider = "builtin" | "frp" | "cloudflare";
type CloudflareMode = "quick" | "named";

interface QuickTunnelDraft {
  enrollmentUrl: string;
  frpProfileId: string;
  frpSubdomain: string;
  cloudflareMode: CloudflareMode;
  cloudflareToken: string;
  cloudflarePublicUrl: string;
  useProxy: boolean;
}
```

工作區仍使用 `WorkspaceProfile`；FRP token 只在建立全域設定時交由 `saveFrpProfile`；內建與 Cloudflare token 仍使用 workspace secret。

## API 設計

| 方法/函數 | 入參 | 出參 | 關聯需求 |
|-----------|------|------|----------|
| `createWorkspace(path, name?)` | 第一個專案絕對路徑與可選名稱 | `WorkspaceProfile` | FR-2 |
| `addWorkspaceFolder(id, path)` | 工作區 ID 與其他專案路徑 | `WorkspaceProfile` | FR-2 |
| `listSoftware()` / `installSoftware(kind)` | 無或 `frpc` / `cloudflared` | 安裝狀態 | FR-3 |
| `listFrpProfiles()` / `saveFrpProfile(profile, token?)` | profile 與可選 token | 全域 FRP profile | FR-3 |
| `updateWorkspace(profile)` | 來源專屬 tunnel 設定 | `void` | FR-4 |
| `setWorkspaceSecret(id, key, value)` | workspace id、分支 secret key、secret | `void` | FR-3, FR-4 |
| `testTunnel(id, service)` | workspace id、MCP/Actions | 測試結果與公開 URL | FR-4 |
| `startRuntime` / `startActionsRuntime` | workspace id | `RuntimeStatus` | FR-4 |
| `listWorkspaces()` | 無 | 最新 profiles | FR-4, FR-5 |

## 文件结构

```text
src/
├── routes/quick-setup/+page.svelte
├── lib/components/quick-setup/QuickTunnelSetup.svelte
└── lib/i18n/catalog.ts
tests/
└── quick-setup-wizard.test.mjs
docs/specs/quick-setup-wizard/
```

## 設計決策

### 決策 1：反向代理來源放在第一步（FR-1、FR-3）

**問題**：來源決定是否要邀請連結、下載程式或準備外部服務，若放在最後才選會讓前置條件突然出現。

**選項**：建立工作區後再選；或第一步先說明並選擇。

**決策**：第一步先選來源，但只保存頁面暫存值，不做安裝或啟動。

### 決策 2：FRP 依服務使用現有兩種資料契約（FR-3）

**問題**：MCP FRP 的後端支援 domain path routing，Actions FRP 使用 subdomain routing，兩者不能共用單一欄位。

**決策**：MCP 由選定 profile 自動產生 `https://<server>/clients/<workspace-id>/mcp`；Actions 收集子網域並產生 `https://<subdomain>.<server>`。

### 決策 3：Cloudflare Quick 與 Named 明確分流（FR-3、FR-4）

**問題**：Quick 無需 token 且 URL 啟動後才取得；Named 則在啟動前需要 token 與固定 URL。

**決策**：Quick 只顯示自動產生說明；Named 顯示兩個必填欄位，token 存 secret，URL 存 profile。

### 決策 4：啟用後才顯示 ChatGPT 填寫值（FR-4、FR-5）

**決策**：測試通道並啟動成功後刷新 profile，再將最終值交給 `GptQuickCopy`。

MCP 快速引導使用 `GptQuickCopy` 的獨立 guided 模式，只顯示 Public MCP endpoint、OAuth Client ID 與授權密碼（重新產生前可重複使用）。MCP runtime 使用 PKCE 且不驗證 Client Secret，因此教學要求其他 OAuth 欄位保留預設；一般工作區頁仍保留原本的完整設定卡。

**理由**：內建 enrollment 與 Cloudflare Quick 都可能在啟動期間更新公開 URL。

## 測試策略

- 靜態契約測試確認第一步三種來源、軟體 API、FRP profile、來源專屬 secret、兩種 runtime 分支與完成值元件。
- 執行 `npm test` 驗證 i18n locale 完整性，並以語言無關的 Svelte AST 檢查可見文案。
- 執行 `npm run check` 與 `npm run build` 驗證 Svelte 型別與 bundle。
- 手動檢查取消資料夾、無效 enrollment、FRP 未安裝／無 profile、Cloudflare Quick／Named、啟動失敗重試。

## 風險評估

| 風險 | 影響 | 緩解措施 |
|------|------|----------|
| 自動下載失敗或平台不支援 | 中 | 保留錯誤與重試；明確指出可至軟體管理進階處理 |
| MCP FRP profile 網域與公開網址不一致 | 中 | 公開網址由已選 profile 自動產生，不讓使用者分別填兩個 host |
| Cloudflare Quick URL 每次改變 | 中 | 啟動後刷新並顯示最終 URL，文案標示適合試用 |
| Named token 被回顯 | 高 | 密碼欄位、workspace secret 儲存、完成頁不顯示 token |
| 重複提交啟動 | 低 | busy 期間禁用導覽、安裝與提交按鈕 |

## 檢查清單

- [x] 技術方案與現有架構一致
- [x] 全部 FR 均被覆蓋
- [x] 文件路徑已對照真實程式碼
- [x] 無新增後端資料模型或 IPC
- [x] 測試可驗證主要驗收標準
