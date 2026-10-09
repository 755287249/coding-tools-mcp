# 需求文檔：quick-setup-wizard

## 功能概述

為第一次連接 ChatGPT 的使用者提供一條獨立快速設定流程。第一步先選擇反向代理來源，再建立工作區、選擇 MCP 或 Actions，依來源完成必要軟體與連線設定，最後啟動服務並顯示 ChatGPT 端每個欄位應填入的實際值。既有工作區頁、服務面板與「新增工作區」流程維持不變。

## 歷史經驗與坑

- **可複用經驗**：沿用既有 `createWorkspace`、software、FRP settings、workspace secrets、runtime start 與 `GptQuickCopy` 契約，不建立第二套後端狀態。
- **必須規避的坑**：MCP 的 FRP 由唯一 `/clients/<id>/mcp` 公開路徑推導 WSS 路由，Actions 的 FRP 則使用獨立子網域；Cloudflare Named Tunnel 同時需要 token 與固定公開網址；一次性註冊連結與 token 不得回顯。

## 術語定義

- **內建 WSS**：不需外部用戶端，使用一次性註冊連結完成裝置註冊的內建反向代理。
- **FRP**：使用 `frpc` 連到自管或組織提供的 FRP 伺服器；全域設定包含伺服器、連接埠與可選 token。
- **Cloudflare Quick Tunnel**：免 token、啟動時取得暫時 `trycloudflare.com` 網址的模式。
- **Cloudflare Named Tunnel**：需要 Tunnel Token 與固定 HTTPS 公開網址的長期模式。
- **快速設定**：獨立於既有工作區管理頁的逐步引導流程。

## 範圍邊界

**In Scope**

- 從側邊欄進入獨立快速設定頁，第一步選擇內建 WSS、FRP 或 Cloudflare。
- 自訂工作區名稱、一次選擇一個或多個專案資料夾並建立新工作區，再選擇 MCP 或 Actions。
- 內建 WSS 驗證一次性註冊連結。
- FRP 檢查並可自動安裝 `frpc`，選擇既有或建立全域 FRP 設定，依服務填入公開 MCP URL 或子網域。
- Cloudflare 檢查並可自動安裝 `cloudflared`，選擇 Quick 或 Named Tunnel；Named 模式收集 token 與固定公開網址。
- 保存對應設定、測試通道、啟動服務並顯示實際 ChatGPT 填寫值。
- 支援返回前一步、錯誤提示、重新嘗試與前往進階工作區。

**Out of Scope**

- 同時啟動 MCP 與 Actions。
- 修改既有工作區頁的完整隧道、認證或權限進階設定。
- 建立伺服器端邀請連結、FRP 伺服器或 Cloudflare Named Tunnel。
- 改動 Rust/Tauri IPC 契約。

## 需求列表

### FR-1：先選擇反向代理來源

**優先級：** Must
**使用者故事：** 作為第一次使用者，我想先選擇公開連線來源，以便後續只看到該來源需要的設定。

#### 驗收標準（EARS）

1. WHEN 快速設定頁開啟 THEN 系統 SHALL 先顯示內建 WSS、FRP、Cloudflare 三種選項與用途、必要條件及適用情境。
2. WHEN 使用者選擇來源 THEN 系統 SHALL 保存暫存選擇並進入建立工作區步驟，不啟動 runtime 或通道。
3. WHEN 使用者返回第一步並改選來源 THEN 系統 SHALL 僅切換快速流程暫存設定，不修改既有工作區功能。

### FR-2：建立快速設定工作區

**優先級：** Must
**使用者故事：** 作為第一次使用者，我想從引導中選擇專案資料夾並建立工作區，以便不必先理解完整主控台。

#### 驗收標準（EARS）

1. WHEN 使用者選擇一個或多個有效資料夾 THEN 系統 SHALL 以第一個資料夾建立工作區、用既有 API 加入其餘資料夾，並顯示完整資料夾清單。
2. WHEN 使用者輸入工作區名稱 THEN 系統 SHALL 使用該名稱建立工作區；IF 名稱留白 THEN 系統 SHALL 沿用第一個資料夾名稱。
3. IF 使用者取消資料夾選擇 THEN 系統 SHALL 留在目前步驟且不建立資料。
4. IF 建立失敗 THEN 系統 SHALL 顯示可理解的錯誤並允許重試。

### FR-3：選擇服務並完成來源專屬設定

**優先級：** Must
**使用者故事：** 作為 ChatGPT 使用者，我想選擇 MCP 或 Actions 並獲得來源專屬欄位指引，以便填入可實際啟動的值。

#### 驗收標準（EARS）

1. WHEN 使用者選擇 MCP 或 Actions THEN 系統 SHALL 顯示該方式的用途與 ChatGPT 端設定差異。
2. IF 選擇內建 WSS THEN 系統 SHALL 只接受 HTTPS、無帳密/query/fragment 且路徑符合 `/_tunnel/enroll/<code>` 的一次性連結。
3. IF 選擇 FRP THEN 系統 SHALL 顯示 `frpc` 安裝狀態，未安裝時提供既有自動安裝操作，並要求選擇或建立含伺服器與連接埠的全域設定。
4. WHEN FRP 搭配 MCP THEN 系統 SHALL 顯示並保存 `https://<FRP-server>/clients/<workspace-id>/mcp`；WHEN FRP 搭配 Actions THEN 系統 SHALL 要求獨立子網域並顯示 `https://<subdomain>.<FRP-server>`。
5. IF 選擇 Cloudflare THEN 系統 SHALL 顯示 `cloudflared` 安裝狀態與自動安裝操作，並提供 Quick、Named 兩種模式。
6. WHEN Cloudflare Named 被選擇 THEN 系統 SHALL 要求 Tunnel Token 與固定 HTTPS 公開網址；WHEN Quick 被選擇 THEN 系統 SHALL 說明網址會在測試通道時自動產生。
7. IF 必填值無效或軟體未安裝 THEN 系統 SHALL 阻止啟用並指出正確值或下一個操作。

### FR-4：啟用所選服務

**優先級：** Must
**使用者故事：** 作為使用者，我想由引導自動保存必要設定並啟動服務，以便完成本機與公開通道連接。

#### 驗收標準（EARS）

1. WHEN 使用者確認啟用 THEN 系統 SHALL 保存所選來源與服務的 profile、必要 secret，測試通道後啟動該服務。
2. WHILE 安裝、測試或啟用進行中 THE 系統 SHALL 禁用重複提交並顯示進度。
3. IF 安裝、通道測試或啟動失敗 THEN 系統 SHALL 保留工作區與已輸入設定、顯示錯誤並允許重試或前往進階設定。
4. WHEN 啟動成功 THEN 系統 SHALL 重新讀取工作區與 runtime 狀態，取得最終公開端點。

### FR-5：顯示逐欄設定教學與實際值

**優先級：** Must
**使用者故事：** 作為非專家使用者，我想知道 ChatGPT 每一欄要填什麼，以便不需自行判斷 URL 或認證方式。

#### 驗收標準（EARS）

1. WHEN MCP 啟用成功 THEN 系統 SHALL 依序引導使用者貼上公開 MCP endpoint、選擇 OAuth、輸入 OAuth Client ID、保留其他 OAuth 預設值，再按下一步與連線並輸入授權密碼（重新產生前可重複使用）。
2. WHEN MCP 快速引導顯示設定值 THEN 系統 SHALL 只顯示公開 MCP endpoint、OAuth Client ID 與授權密碼（重新產生前可重複使用），不要求 PKCE 流程不使用的 Client Secret。
3. WHEN Actions 啟用成功 THEN 系統 SHALL 顯示 GPT Actions 的操作順序與 OpenAPI Schema URL、隱私權網址、API Key 等實際值。
4. WHEN 值可用 THEN 系統 SHALL 提供複製操作；IF 值尚未產生 THEN 系統 SHALL 明確標示未設定。
5. WHEN 使用者完成 THEN 系統 SHALL 可前往新工作區，且不改變既有頁面行為。

## 非功能需求

- **NFR-1（效能）**：頁面載入與來源選擇不得啟動 runtime；只在使用者確認後呼叫通道測試與啟動 API。
- **NFR-2（安全）**：一次性連結與 token 採密碼輸入、不在完成頁回顯；錯誤訊息不得主動拼接完整 secret。
- **NFR-3（相容性）**：通過 Svelte 型別檢查、i18n locale 完整性與可見文案測試；不以任何特定語言字元判斷是否完成翻譯，且不新增後端命令。
- **NFR-4（可用性）**：所有步驟可鍵盤操作，進度有文字標籤，安裝狀態與預期值可直接辨識，並遵守 reduced-motion 設定。

## 依赖关系

- 既有 Tauri dialog、workspace、software、settings、secrets、tunnel 與 runtime API。
- 既有 built-in WSS enrollment、FRP profile、Cloudflare tunnel 與 `GptQuickCopy` 邏輯。
- 內建伺服器端邀請連結必須允許使用者選定的服務；FRP 與 Cloudflare Named 的伺服器端資源需由使用者或管理員預先建立。

## 檢查清單

- [x] 已消化現有三種 tunnel、軟體安裝與設定元件的行為
- [x] 需求覆蓋核心與錯誤場景
- [x] 每條需求有唯一 ID 與可測驗收標準
- [x] 已標注 MoSCoW 優先級
- [x] 範圍與非功能需求明確
