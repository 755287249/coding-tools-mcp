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
