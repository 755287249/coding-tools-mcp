# 自製應用插件

Coding Tools MCP 插件是一個由 Vite 打包的單檔 JSON：`{format:1, manifest, code}`。程式在 `sandbox="allow-scripts"` iframe 與嚴格 CSP 中執行，不能直接讀取宿主頁面、檔案系統或網路；所有能力都由 `@coding-tools/plugin-sdk` 的 `postMessage` RPC 提供，宿主會再次按 manifest 權限檢查。

## 建立與編譯

可複製 `plugins/template`：

```bash
pnpm install
pnpm --dir plugins/template build
```

產物是 `plugins/template/dist/hello-plugin.ctplugin.json`。在「自定義 → 插件 → 添加 → 匯入插件檔案」選取它即可。插件 ID 只能使用小寫字母、數字與連字號。

```json
{
  "id": "hello-plugin",
  "name": "Hello plugin",
  "version": "1.0.0",
  "description": "Example",
  "permissions": {
    "hosts": ["api.example.com"],
    "storage": true,
    "seedLibrary": false,
    "progress": false
  }
}
```

## SDK

- `plugin.http(...)`：只允許公开 HTTPS，且主機必須列在 `permissions.hosts`。代理不跟隨重新導向，會回傳 `location` 與 `set-cookie`。
- `CookieJar`：維護插件自己的 Cookie，適合需要多次表單/OAuth 請求的協議登入。
- `plugin.storage.get/set`：每個插件獨立，資料存在應用資料目錄的 `plugins/<id>.data.json`。
- `plugin.seedLibrary(args)`：需要 `seedLibrary:true`，作用於目前插件槽所在的專案。
- `plugin.progress(value)`：需要 `progress:true`。`null` 關閉進度；使用者按「停止」時插件收到 `plugin.on('stop', ...)`。
- `plugin.context()` / `plugin.on('context', ...)`：取得目前 `workspaceId`、`folderId`、語言與主題。插件 iframe 在切換頁面時不銷毀，長工作可繼續執行。

不要把密碼或權杖寫進插件包；如確實需要保存登入狀態，使用插件私有 storage，並在介面清楚告知使用者。
