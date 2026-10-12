# 交接文件：cloudflared os error 123 與 0.1.142 無法回退

> 這份文件給接手的人（或其他 AI 機器人）。每完成一步就更新「進度」一節。
> 若此 session 中斷，從「進度」中第一個未打勾的項目繼續。

## 1. 使用者回報的問題

1. 桌面版（Windows）顯示：
   `本地 MCP 已啟動，但公網隧道啟動失敗：启动 cloudflared 失败: 文件名、目录名或卷标语法不正确。 (os error 123)`
2. 安裝 v0.1.142 之後，無法回退到以前的版本。

## 2. 重要背景：哪個分支才是真正的開發線

- `main` 分支 **不是** 目前的開發線。`main` 停在舊的 0.1.59 架構，上面有幾個
  「Add files via upload」的部分上傳（只有部分檔案，版本號被改成 0.1.143）。
- 真正的開發線是 `coderabbit/sandbox-mcp-setup/bf9588fc`：
  - 標籤 `v0.1.67` … `v0.1.142` 都在這條線上。
  - PR https://github.com/755287249/coding-tools-mcp/pull/1 已合併進這個分支（0.1.143 候選）。
- 本次修正的分支：`vorflux/fix-cloudflared-cwd-and-rollback`，基底是
  `coderabbit/sandbox-mcp-setup/bf9588fc`，PR 也開向這個分支。

## 3. 問題一：cloudflared os error 123

### 根本原因

- v0.1.142（commit `ca37d16` "unify MCP connection, isolated projects"）新增了
  「全局 MCP 連接」（`src-tauri/src/data/connection.rs`，ID `GLOBAL_MCP_ID`）。
- 遷移時會執行 `connection.path.clear()`，所以全局連接的 `profile.path` 是空字串 `""`。
- 隧道由全局連接啟動。`src-tauri/src/tunnel/supervisor.rs` 把
  `Path::new(&profile.path)` 當成 cloudflared 的工作目錄傳入
  `spawn_cloudflare_tunnel`，最後 `cmd.current_dir("")`。
- Windows 的 `CreateProcessW` 收到空的 `lpCurrentDirectory` 會回傳
  `ERROR_INVALID_NAME (123)`：「文件名、目录名或卷标语法不正确」。
- `src-tauri/src/tunnel/frp/client.rs` 的 `cmd.current_dir(&first_profile.path)` 有相同問題
  （全局連接使用 FRP 時會出現 `启动 frpc 失败 ... (os error 123)`）。

### 修正方式

- 新增 `tunnel::process_working_dir(candidate, fallback)`：
  只有在路徑非空且是存在的目錄時才使用它，否則改用一個一定存在的目錄
  （該 profile 的日誌目錄 `log_dir_for_profile(id)`）。
- cloudflare 與 frpc 的啟動都改用這個函式。
- cloudflared 與 frpc 不需要專案目錄當工作目錄（命名隧道用 token，Quick 隧道用 `--url`，
  frpc 用 `-c <絕對路徑>`），所以改用日誌目錄不會改變功能。
- Node Agent 沒有啟動 cloudflared / frpc（只有內建 WSS 隧道），所以不需要同步修改（parity 不適用）。

## 4. 問題二：安裝 0.1.142 後無法回退

### 原因（有兩層）

1. **單一實例 + 啟動交接（startup handoff）只允許升級。**
   - 0.1.67 以後每個版本都內建 `scripts/desktop-startup-handoff.ps1`。
   - 第二個實例啟動時，如果已有實例在執行（通常 0.1.142 縮在系統匣），
     腳本只會取代「版本比自己舊」的實例（`Select-UpgradeTargets` 用 `$version -lt $TargetVersion`）。
   - 所以開啟 0.1.141 時，它看到 0.1.142 在執行，就顯示
     "An equal/newer instance is already running ... Exit it from the tray before opening this copy."
     然後自己退出。看起來就像「回退不了」。
   - 這段邏輯編譯在 **被開啟的那個 exe** 裡。舊版 exe 的行為無法從新版程式碼修改。
2. **0.1.142 的資料遷移是單向的。**
   - `data/profiles.json` 會被改寫：新增 `mcp_connection`，多資料夾工作區拆成多個專案，
     `mcp_enabled_workspace_ids` 改成只有 `GLOBAL_MCP_ID`，專案強制開啟沙箱。
   - 舊版看不懂 `mcp_connection`，所以回退後 MCP 不會自動啟動、隧道設定看起來不見了。
   - 遷移前沒有保存可還原的備份（`profiles.json.bak` 每次存檔都會被覆蓋）。
   - 另外，In-app 更新器本來就只接受較新版本（`updates.rs`：`Already on this version or newer`）。

### 修正方式

1. 新版（0.1.143 起）的 `desktop-startup-handoff.ps1`：沒有較舊實例、但有 **較新** 實例在執行時，
   （注意：這段腳本編譯在「被開啟的 exe」裡，所以只有開啟 0.1.143 以後的版本才會出現確認視窗；
   開啟 0.1.141 仍是舊行為，要先從系統匣結束新版。）
   彈出「是否切換到這個較舊版本」的確認視窗。使用者按「是」才會關閉較新版並啟動被開啟的版本。
   `-NoDialogs` 模式下不會自動降級（保持原來的安全行為）。
2. 在第一次執行「全局連接遷移」之前（`DataStore::ensure_global_connection`），建立一次性備份資料夾
   `%APPDATA%\coding-tools-mcp-desktop\data\pre-global-connection\`：
   - `profiles.json`：遷移前的桌面設定檔。注意它不是 0.1.141 寫的原始位元組，
     而是 0.1.143 載入（正規化、共享鏡像同步）之後、全局連接遷移之前的狀態；舊版可以讀取。
   - `workspaces\<id>\workspace.json`、`secrets.json`：遷移前的共享工作區鏡像
     （位置 `%LOCALAPPDATA%\CodingToolsMCP\workspaces\<id>\`，遷移會改寫它們，
     舊版啟動時會用它們覆蓋 profile，所以也要備份）。
   - 只建立一次，之後不覆蓋。備份失敗只記錄錯誤，不會阻止啟動（fail-soft）。
   - 限制：如果回退後在舊版用了一段時間再升級，備份仍是「最早」那份，期間的修改不在裡面
     （還原時被替換的檔案會留在 `before-rollback-*`）。備份含 `secrets.json`，權限已限制為目前使用者，
     不需要時可手動刪除整個 `pre-global-connection` 資料夾。
   - 程式：`src-tauri/src/data/migrate.rs` 的 `backup_before_global_connection_migration`。
   - 注意：已經升級到 0.1.142 的電腦，遷移已經發生過，沒有這個備份。
3. 新增 `scripts/rollback-desktop.ps1`：
   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\rollback-desktop.ps1 -TargetExe C:\path\ctmcp-0.1.141-win64.exe
   # 若有 pre-global-connection 備份，也還原舊設定：
   powershell -ExecutionPolicy Bypass -File scripts\rollback-desktop.ps1 -TargetExe C:\path\ctmcp-0.1.141-win64.exe -RestorePreMigrationSettings
   ```
   這個腳本不在可攜版 ZIP 裡，請從原始碼倉庫下載。
   它會：先檢查備份是否存在（不存在就直接停止，不會關閉正在執行的程式）→
   結束本 Windows 工作階段中所有 Coding Tools MCP 桌面程序（含 cloudflared/frpc 子程序）→
   （可選）先把目前的檔案留存到 `data\before-rollback-<時間>\` 再還原備份 → 啟動指定的 exe。

### Node Agent parity 說明

- cloudflared / frpc 工作目錄修正：Node Agent 不啟動 cloudflared / frpc，不適用。
- 遷移前備份：Node Agent 的遷移（`packages/node-agent/src/application.ts`）把新設定寫到
  `global-model/` 並「保留舊設定檔不動」（Legacy files remain intact），本來就有回退來源。
  桌面版原本沒有，所以這次只補桌面版。
- 降級確認視窗：只屬於 Windows 桌面單一實例交接（ND-002 Tauri desktop shell，已排除於 parity）。

### 使用者現在可以做的事（不用等新版）

1. 在系統匣（右下角）找到 Coding Tools MCP 圖示 → 右鍵 →「結束程式」。
   或在工作管理員結束 `ctmcp.exe` / `coding-tools-mcp-desktop.exe`。
2. 再開啟舊版 exe（例如 `ctmcp-0.1.141-win64.exe`）。
3. 舊版開啟後，到 MCP 頁面手動重新啟用工作區的 MCP 與隧道（因為 0.1.142 已改寫設定）。
4. 建議：直接使用修正後的 0.1.143+，就不需要回退。

## 5. 驗證方式

- Linux 上：`cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --lib tunnel::`
  （新增 `process_working_dir` 的單元測試）。
- `cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --lib pre_global_connection_backup`（遷移前備份測試）。
- `pwsh -File tests/rollback-desktop.Tests.ps1`（Linux 也可跑）。
- 已知：在 Linux 用 pwsh 跑 `tests/desktop-startup-handoff.Tests.ps1` 時，最後一段
  `Replacement working directory must be preserved` 因路徑分隔符 `/` vs `\\` 失敗，
  這是 Linux 平台差異，與本次修改無關；Windows 上應通過。
- Windows 上（CI `Manual source upgrade (Windows)` workflow 或本機）：
  `./tests/desktop-startup-handoff.Tests.ps1`。
- 手動：用全局連接 + Cloudflare Quick 隧道啟動 → 不再出現 os error 123。

## 6. 進度

- [x] 找到 os error 123 的根本原因（全局連接 `path` 為空 → `current_dir("")`）。
- [x] 找到無法回退的原因（handoff 只允許升級 + 單向資料遷移）。
- [x] 修正 cloudflared / frpc 工作目錄並加測試（`tunnel::process_working_dir_tests`，4 項通過）。
- [x] 遷移前備份 `data\pre-global-connection\` 並加測試（`data::migrate::pre_global_connection_backup_tests`）。
- [x] handoff 腳本支援確認後降級（`Select-DowngradeTargets`，`tests/desktop-startup-handoff.Tests.ps1` 新增 6 項）。
- [x] 新增 `scripts/rollback-desktop.ps1` 與 `tests/rollback-desktop.Tests.ps1`（10 項，Linux pwsh 7.4 通過）。
- [x] Linux：`cargo test --lib -- data:: tunnel::` 98 項通過；`version:check`、`node-agent:parity:check` 通過。
- [x] 推送、開 PR：https://github.com/755287249/coding-tools-mcp/pull/2（base：`coderabbit/sandbox-mcp-setup/bf9588fc`）。
- [x] 內部 review（風險 3/10）：已修正「還原前先檢查備份」、確認視窗中的備份路徑、測試版本號、還原失敗訊息。
- [x] Windows CI `Manual source upgrade (Windows)` 通過（run 38211322270：provider 測試、handoff/rollback 測試、可攜版 exe 建置）。可攜版 exe 在該 run 的 Artifacts（保留 14 天）。
- [ ] 使用者在 Windows 實機驗證：全局連接 + Cloudflare 隧道可啟動。

## 7. 安全提醒

- 使用者在對話中貼出了 GitHub Personal Access Token。這個 token 應該立即到
  https://github.com/settings/tokens 撤銷（revoke）並重新產生。本次修改沒有使用它，
  而是用機器上已設定的 git 憑證。**不要把任何 token 寫進這個倉庫。**

---

# 第二部分：種子庫分配（2026-10-12 使用者回報）

## 使用者回報

1. 種子庫裡看得到已入庫、有效的種子卡片，但新建對話（選好專案/工作目錄後）不再出現
   「從哪裡分配」的三類卡片（帳號 / 來源 / 備註前綴）。舊版有。
2. 對話右上角只有「接入AI」，沒有「種子庫」；新建對話沒有選擇種子庫分配的功能。
3. 種子 AI 連上後，做兩次 `chat_wait` 都是 idle，就停下來問使用者「需要我繼續等待嗎？」，不再持續等待。
4. 需求：右上角改成 [接入AI] [種子庫] 兩個按鈕。接入哪一種，那個按鈕變成 [斷開]；另一個按鈕用來切換。
   斷開或切換時，原本的 AI 如果仍有效，就進入種子庫，歸類到「備用」卡片，成為可分配（自動或手動）的備用種子；
   失效就直接斷開。新建對話「總是顯示選擇」；沒有可分配種子時卡片顯示 0 並變灰。
5. 使用者要求繼續放在 PR #2。

## 根本原因（已確認）

- `SeedStart.svelte` 只有在種子庫設定 `enabled`（「新對話自動分配種子」開關）為 true 時才顯示分配卡片；
  否則只顯示「请接入 AI」標題。
- 後端 `chat.rs` 的 `create` 只有在 `seeds::enabled(root)` 為 true 時才把新對話設成 `seed_auto`，
  `chat_seeds.rs::poll_at` 也只有 `data.enabled` 才分配。所以開關為 false 時，整條種子分配流程都消失，
  右上角也只會顯示「接入AI」（只有 `detail.seed_auto` 時才顯示「種子庫」）。
- 種子庫是「每個專案目錄」各自一份（`<專案目錄>/docs/chat-sessions/seed-library.seeds.json`）。
  0.1.142 把多資料夾工作區拆成多個專案；新專案目錄的種子庫是新的，`enabled` 預設 false。
- 斷開種子對話（`detach`）會讓種子在下次 poll 時被退役（`conversation_detached`），不會回到種子庫。
- 種子 AI 停止等待：入庫提示詞與 idle 指令沒有明確說「不要詢問使用者、不要結束回合、無限期等待」。

## 實作計畫（進度見下）

- A. 使用者在新對話中「明確選擇」種子池時，不再受 `enabled` 開關限制：
  `create` 帶 `seed_filter` 時設 `seed_auto=true`、`seed_explicit=true`；`poll_at` 對 `seed_explicit` 對話照常分配。
  `enabled` 開關只控制「沒有選擇時是否自動分配」。
- B. `Seed` 新增 `standby: bool`（serde default）。過濾條件 `{"standby":true}` 只匹配備用種子；
  `ready_groups` 帶 `standby` 欄位；被重新分配時清除 `standby`。
- C. 斷開/切換時（`detach` 帶 `standby:true`）：
  - 種子對話：種子仍在線（未退役、`last_seen` 在 `SUSPECT_MS` 內、沒有未完成操作）→ 釋放回種子庫並標記 `standby`；否則照舊斷開。
  - 直接接入的 AI：仍在線（lease 有效或正在等待）→ 建立一顆 `source=standby` 的手動種子，
    在記憶體保存「舊 attachment → 種子 URL」；該 AI 下一次 `chat_wait` 收到 `status=standby`
    與新的種子 URL 路徑（`/mcp/seeds/<folder>/<url_key>`），依指示改連種子 URL 並持續等待。失效就直接斷開。
- D. 前端：`SeedStart` 永遠顯示三類卡片與「備用」卡片（0 時變灰）；右上角 [接入AI] [種子庫] 切換邏輯；
  既有對話可從「種子庫」按鈕選擇種子池（`seed_resume_chat` 帶 `seed_filter`）。
- E. 強化種子提示詞、`seed-connect` Skill、idle 指令：不要詢問是否繼續等待、不要結束回合。
- F. Node Agent parity：A、B、C（種子部分）、E 同步到 `packages/node-agent`；
  直接 AI 轉備用種子（C 的第二項）若未同步，要寫入 `docs/todo/node-agent-parity/manifest.json`。

## 第二部分進度

- [x] 找到根本原因。
- [x] A 後端明確選擇不受開關限制（Rust `chat.rs` create / `chat_seeds.rs` poll_at；Node `store.ts` / `seeds.ts`）
- [x] B standby 欄位與過濾、`pool_groups`（含 0 可分配的池）
- [x] C 斷開轉備用：`detach` 帶 `standby:true`；`release_to_standby` / `enroll_direct_standby`；
      記憶體 `STANDBY_HANDOFFS`，`chat_wait`（gateway 與 wait 迴圈）回傳 standby 指令。Rust `standby_tests` 4 項、Node 3 項通過。
- [x] D 前端：`SeedStart` 永遠顯示（0 可分配時卡片變灰）+「備用 AI」卡片；`ChatPanel` 右上角 [接入AI][種子庫]，
      已接入的那個變 [斷開]，另一個切換；種子池選擇對話框（`seed_resume_chat` 帶 `seed_filter`）；斷開一律帶 `standby:true`。
      svelte-check 0 錯誤 0 警告；前端測試 276 項通過。
- [x] E 提示詞（seed-enrollment.ts、seed-connect SKILL.md、idle 指令 Rust/Node）
- [x] F Node parity（含直接 AI 轉備用）
- [x] 內部 review（風險 6/10）後修正：
  - 既有對話選種子池前一律先斷開（也清掉離線 AI 的舊 attachment）；後端只在仍有 attachment 時拒絕，舊的 seed_owner 不再擋住。
  - 回到備用的種子用舊 chat_id/attachment_id 呼叫 chat_wait 時，種子閘道自動移除這兩個參數，繼續等待（新增 gateway 層測試）。
  - 轉備用失敗不再阻止斷開（退回一般斷開並記錄）。
  - 備用 URL 被兌換後立即從記憶體移除；URL 的資料夾 ID 在斷開時記錄（Tauri 指令注入 `_folder_id`，Node 用 `folder_id`）。
  - `release_workspace` 也清除 `seed_explicit`；一小時未連線的備用種子在下次建立備用時退役。
  - 新對話按 [接入AI] 會建立手動對話並開啟接入指引。
  - 修改 seed-connect SKILL.md 後需執行 `pnpm --filter @coding-tools/node-agent run sync:rust-contract` 重新產生 `rustCatalog.generated.ts`。
- [x] 第二輪 review：風險 4/10，可合併；已補「轉備用失敗時顯示提示」與「release_workspace 跳過無法讀取的對話」。
- [x] 瀏覽器端到端測試（node agent + 瀏覽器，簡體中文介面）：9 項全部通過。
      測試中發現：新對話按 [接入AI] 後導航到對話網址，舊的 ChatPanel 被卸載，接入指引沒有打開；
      已改為在網址加 `&connect=1`（`onNavigate(folder, chat, connect)`）。
- [x] 推送、更新 PR #2 描述。
- [x] Windows CI `Manual source upgrade (Windows)` 在 740ec0a 通過（run 38220031640，含可攜版 exe，Artifacts 保留 14 天）。

## 尚未處理（低優先，可後續）

- 直接 AI 轉成的備用種子，除了「備用 AI」卡片，也會以代理名稱（source `standby`）出現在前綴/帳號分類中。
- 種子庫頁面沒有「備用」標籤；重新分配後 `reason` 仍是 `standby`。
- 工作對話的回覆沒有記錄作者，切換 AI 後舊回覆會顯示新 AI 的名稱（舊行為）。
- 尚未在 Windows 桌面實機驗證新的種子流程。

---

# 第三部分：接入流程簡化與目錄重疊（2026-10-12）

## 使用者回報

- 接入AI 與接入種子都失敗：`Project directories overlap. Use separate directory roots before enabling isolated access.`
  （專案目錄 `C:\Users\belly\Documents` 內還有其他專案。）
- 接入AI 提示詞太長（pairing POST、OAuth/PKCE、Client ID、密碼、redirect_uri），部分 AI 會拒絕連線。

## 使用者決定

1. 取消重疊檢查：父目錄包含子目錄、子目錄不包含父目錄。專案目錄內預設可讀寫；
   存取專案外路徑需要使用者確認。輸入框提供三種授權模式：完全授權（自動允許）、
   一般授權（低風險自動、高風險確認）、嚴格授權（有風險就確認）。
2. 接入AI 改成和種子一樣的一次性網址（認證在網址內），提示詞簡短；OAuth 方式可在設定中切換。

## 根本原因

- `src-tauri/src/mcp/listener/gateway.rs::project_context_for_epoch` 與
  `packages/node-agent/src/server/projectGateway.ts::contextFor` 在任一其他專案目錄與本專案目錄
  互相包含時直接拒絕。種子網址也走同一個函式，所以兩種接入都失敗。

## 計畫與進度

- [x] A. 移除重疊拒絕（Rust + Node），保留其餘隔離（每個專案仍以自己的目錄為根）。測試 `nested_project_directories_can_both_connect`。
- [x] B. 接入AI 一次性網址：新的 `seed_direct` 動作替對話建立「綁定此對話」的手動種子
      （`Seed.bound_chat`，對話 `seed_direct=true`、`seed_filter.seed_id`），提示詞只有網址＋一段話；
      連接指引中可切換回 OAuth（偏好存在 localStorage `ctmcp-connect-oauth`）。Node 同步。
      端點不是 https 或 loopback 時自動改用 OAuth 提示詞。斷開時未使用的一次性網址會退役。
      綁定種子不出現在種子池分類中。測試：Rust `direct_url_is_bound_to_its_chat_only`、Node `direct connect URL is bound to its chat only`。
- [x] 內部 review（風險 5/10）後修正：
  - 切換到 OAuth 時先撤回待用的接入網址（detach），避免對話卡在接入網址模式、右上角沒有 [斷開]。
  - 重新產生網址時，退役「已使用但未分配」的舊網址，並刪除從未使用的舊網址列；斷開時同理。
  - 重新打開接入指引時，若舊網址仍未使用且未過期，直接沿用，不會讓已貼出的網址失效。
  - 產生網址時清除過時的 seed_owner。release_workspace 也撤回待用接入網址。
  - 文字更正：網址需在 1 小時內首次連接，連接後 7 天內有效（不是「一次性、1 小時」）。
- [x] 端到端測試（node agent + 瀏覽器）通過：巢狀專案、接入網址、OAuth 切換、重新打開沿用網址、斷開轉備用。
  測試後再修：網址快取改存 sessionStorage（重新整理頁面不會讓已貼出的網址失效）；
  備用種子在同一次 poll 被重新分配時也會移除舊 ID；備用種子只顯示在「備用 AI」卡片，不再重複出現在分類中。
- [x] Windows CI 在 c964c7c 通過（run 38226003382，含可攜版 exe）。
- 已知差異（已告知使用者，後續可改）：
  - 接入網址走種子閘道，工具集比 OAuth 少（沒有 chat_discuss、exec_many、request_permissions、
    history_session_* 等），每個種子最多 4 個進行中操作。
  - 取消重疊檢查後，父專案的 AI 可以寫入子專案的 `docs/chat-sessions/seed-library.seeds.json`，
    理論上可偽造子專案的種子並以子專案的權限設定執行。只有子專案權限比父專案寬鬆時才有影響。
- [ ] C.（下一步，較大）每個對話的授權模式（完全/一般/嚴格）與專案外路徑核准。
      現況：檔案工具的 `Workspace::resolve_*` 一律拒絕絕對路徑與越界路徑；專案隔離會清空
      `sandbox.external_paths`。需要：專案層級的「已核准外部路徑」清單（讀/寫），
      檔案工具與沙箱（AppContainer 已支援 `SandboxPathGrant`）都要讀它；越界時回傳
      `OUTSIDE_PROJECT_REQUIRES_APPROVAL`，聊天介面顯示核准卡片；模式決定是否自動核准。

---

# 第四部分：最大相容接入、專案完全存取、添加種子介面（2026-10-12）

## 使用者回報

- 有的 AI 說「我沒有 MCP 客戶端，沒有 seed_connect / chat_wait 工具」（只有網頁讀取 GET）。
- 有的 AI 拒絕「無人值守的長期待命」：提示詞中的「無人值守 / 不要詢問用戶」反而觸發拒絕。
- 希望「接入AI」與「接入種子」兩個通道相同，只有地址規則略有不同；目標是最大程度相容各家 AI。
- 推進「專案完全存取」：專案目錄內預設可讀寫；存取專案外路徑要使用者確認；
  輸入框可選「完全授權 / 一般授權 / 嚴格授權」。
- 「添加種子」頁面改成卡片式並優化。

## 設計

A. 接入相容（主 session 負責）
  - 兩個通道共用同一個網址格式 `/mcp/seeds/<folder>/<key>` 與同一套提示詞產生器；差別只在綁定對話或進入種子池。
  - 種子閘道的工具集改為與 OAuth 專案範圍相同（`project_scope::allowed`），不再較少。
  - 新增「只能 GET」的相容模式：`GET <網址>` 回傳純文字說明與下一步連結；
    `GET <網址>/wait?n=<序號>`、`GET <網址>/reply?...` 等，每個回應都附上完整的下一個網址（含新序號避免快取），
    讓只有網頁讀取工具的 AI 也能參與。
  - 提示詞改寫：說明這是使用者自己的本機工具、訊息由使用者本人發出；不用「無人值守」字眼；
    明確寫「工具在 MCP 伺服器上，不在你的客戶端；沒有 MCP 客戶端就用 HTTP（POST JSON-RPC）或 GET 相容模式」。

B. 專案完全存取與三種授權模式（build 子代理，分支 `vorflux/permission-modes`）
C. 添加種子頁面卡片化（build 子代理，分支 `vorflux/seed-add-cards`）
完成後合併回 `vorflux/fix-cloudflared-cwd-and-rollback`（PR #2）。

## 第四部分進度

- [x] A 接入相容：
  - 種子閘道工具集 = OAuth 專案範圍（`seed_tool_allowed` / `seedToolAllowed`）。
  - GET 相容模式：`GET <網址>` 回傳純文字說明（要求 JSON 時仍回 JSON）；`/connect`、`/wait`、`/reply`、`/call`、`/tools`
    （Rust `get_action`，Node `handleSeedGet` 透過 `internalRpc` 重用 POST 流程）。每頁附完整下一步網址與新 `n`。
  - 接受缺少 `MCP-Protocol-Version` 標頭及 2024-11-05 / 2025-03-26 / 2025-06-18。
  - 共用提示詞 `connectPrompt(endpoint, 'direct'|'seed')`（src/lib/chat/seed-enrollment.ts）；去掉「無人值守／不要詢問」字眼。
  - 測試：Rust `seed_http_gateway_preserves_oauth_and_scopes_project_tools`（含 GET 頁面）、Node `global seed endpoint ...`。
- [ ] B 授權模式
- [ ] C 添加種子介面
- [ ] 合併、測試、更新 PR

---

# 第五部分：GET 相容強化、提示詞、授權模式、添加種子卡片（Arena Agent 接手，2026-10-12）

> 接手者：從本節「第五部分進度」第一個未打勾項目繼續。分支仍是 `vorflux/fix-cloudflared-cwd-and-rollback`（PR #2）。
> 本機環境（Arena 沙箱）沒有 Rust 工具鏈、記憶體 1GB，Rust 改動只能靠 Windows CI
> `Manual source upgrade (Windows)`（workflow_dispatch，ref 選本分支）驗證。Node 端可本機測：
> `cd packages/node-agent && pnpm run build:server:unlocked && node --import ./test/setup.mjs --test test/<file>.test.mjs`。

## 使用者回報（仍存在）

- 部分 AI 回覆：「我無法直接完成這個連接。我的工具裡沒有能調用 MCP 的客戶端，也沒有 seed_connect、chat_reply 這些方法。
  我能做的只有普通的網頁讀取(GET)，它不能建立MCP會話。」
- 希望接入AI / 接入種子兩通道一致、最大程度相容各家 AI；提示詞再優化；推進「專案完全存取」；添加種子頁面卡片化。

## 分析（為什麼 GET 模式仍可能失敗）

1. 提示詞只給了 MCP 基底網址，GET 入口 `/connect?n=` 要 AI 自己拼。很多網頁讀取工具（Claude web_fetch、ChatGPT 瀏覽等）
   **只允許打開使用者訊息或先前結果中出現過的網址**，自己拼的網址會被拒絕 → AI 以為只能放棄。
   修正：提示詞直接給出完整的「網頁入口」網址；之後每頁都附完整下一步網址（出現在結果中的網址可被打開）。
2. 頁面先輸出最多 24000 字的 JSON、最後才是下一步網址；讀取工具常截斷長頁面 → 看不到下一步。
   修正：下一步網址放在頁面最前面（JSON 後面再重複一次），JSON 縮短。
3. GET 的 `/wait` 預設等 20 秒；很多讀取工具 10~15 秒逾時。修正：GET 預設等 8 秒（可用 `wait=` 調整，上限 25 秒）。
4. 部分讀取工具會先查 `robots.txt`。修正：新增 `/robots.txt`（允許 `/mcp/seeds/`）。
5. 瀏覽器型代理（會點連結、填表單）需要 HTML。修正：`Accept` 含 `text/html` 時回傳簡單 HTML（可點連結 + 回覆表單），
   其他情況維持純文字。
6. 無法解決的限制（如實告知使用者）：只允許打開「已出現網址」的讀取工具，無法把任意回覆文字放進網址 →
   這類 AI 可以讀取/等待訊息，但 `reply` 需要自己拼網址，可能仍被工具拒絕。這是 AI 廠商工具的限制，伺服器端無法繞過。

## 第五部分進度

- [ ] P1 提示詞：直接給「網頁入口」完整網址 + MCP 地址；明確「不需要你自己有這些工具」。（`src/lib/chat/seed-enrollment.ts`）
- [ ] P2 GET 頁面：下一步網址置頂、JSON 縮短、wait 預設 8 秒、HTML 版（Node `routes/seeds.ts`）
- [ ] P3 同步 Rust（`src-tauri/src/mcp/listener/seeds.rs`、`routes.rs` robots.txt）
- [ ] P4 Node 測試 + Rust 測試更新；Windows CI
- [ ] C 添加種子頁面卡片化（`src/lib/components/chat/ManualSeed.svelte` 等）
- [ ] B 授權模式（完全/一般/嚴格）— 見下方 B 設計
- [ ] 更新 PR #2 描述
