import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const workspacePagePath = new URL("../src/routes/workspace/[id]/+page.svelte", import.meta.url);
const workspaceSettingsPath = new URL(
  "../src/lib/components/workspace/WorkspaceSettings.svelte",
  import.meta.url,
);
const workspaceFolderManagerPath = new URL(
  "../src/lib/components/WorkspaceFolderManager.svelte",
  import.meta.url,
);
const mcpPanelPath = new URL(
  "../src/lib/components/workspace/McpWorkspacePanel.svelte",
  import.meta.url,
);
const actionsPanelPath = new URL(
  "../src/lib/components/workspace/ActionsWorkspacePanel.svelte",
  import.meta.url,
);
const tabsPath = new URL("../src/lib/components/Tabs.svelte", import.meta.url);
const quickCopyPath = new URL("../src/lib/components/GptQuickCopy.svelte", import.meta.url);
const sessionPromptPath = new URL(
  "../src/lib/components/ChatGptSessionPrompt.svelte",
  import.meta.url,
);
const packagePath = new URL("../package.json", import.meta.url);

async function read(path) {
  return readFile(path, "utf8");
}

test("工作區頂層分頁保存於 URL 並按需載入", async () => {
  const source = await read(workspacePagePath);

  assert.match(
    source,
    /type WorkspaceTab = "overview" \| "history" \| "telemetry" \| "logs" \| "health" \| "features" \| "mcp" \| "actions" \| "settings"/,
  );
  assert.match(source, /searchParams\.set\("tab", tab\)/);
  assert.match(source, /searchParams\.set\("section", mcpSection\)/);
  assert.match(source, /searchParams\.set\("section", actionsSection\)/);
  assert.match(source, /replaceState: false, noScroll: true, keepFocus: true/);
  assert.match(source, /import\("\$lib\/components\/workspace\/WorkspaceOverview\.svelte"\)/);
  assert.match(source, /import\("\$lib\/components\/workspace\/WorkspaceSettings\.svelte"\)/);
  assert.match(source, /import\("\$lib\/components\/workspace\/WorkspaceFeatureControls\.svelte"\)/);
  assert.match(source, /import\("\$lib\/components\/workspace\/McpWorkspacePanel\.svelte"\)/);
  assert.match(source, /import\("\$lib\/components\/workspace\/ActionsWorkspacePanel\.svelte"\)/);
});

test("初始資料平行載入且儲存設定不再全面 reload", async () => {
  const source = await read(workspacePagePath);

  assert.match(source, /listWorkspaces\(\)/);
  assert.match(source, /capabilities\.frpManagement \? listFrpProfiles\(\)/);
  assert.match(source, /async function refreshMcpRuntime/);
  assert.match(source, /async function refreshActionsRuntime/);
  assert.match(source, /persistProfile\("save\.mcp\.port"/);
  assert.match(source, /persistProfile\("save\.actions\.policy"/);
  assert.doesNotMatch(source, /await load\(\)/);
  assert.match(source, /console\.debug\(`\[workspace:/);
});

test("工作區設定分頁集中名稱、資料夾與會話恢復入口", async () => {
  const pageSource = await read(workspacePagePath);
  const settingsSource = await read(workspaceSettingsPath);

  assert.match(pageSource, /loadSettingsPanel\(\)/);
  assert.match(settingsSource, /<WorkspaceMetaForm/);
  assert.match(settingsSource, /profile\.path/);
  assert.doesNotMatch(settingsSource, /<WorkspaceFolderManager/);
  assert.match(settingsSource, /<ChatGptSessionPrompt \/>/);
});

test("資料夾清單不再提供預設資料夾功能", async () => {
  const source = await read(workspaceFolderManagerPath);

  assert.match(source, /no default folder is used/);
  assert.doesNotMatch(source, /setActiveWorkspaceFolder|selectDefault|Set as default/);
  assert.doesNotMatch(source, /\$t\("Default"\)/);
});

test("MCP 與 Actions 配置拆為獨立服務面板", async () => {
  const [mcpSource, actionsSource] = await Promise.all([read(mcpPanelPath), read(actionsPanelPath)]);

  for (const source of [mcpSource, actionsSource]) {
    for (const value of ["service", "tunnel", "auth", "logs", "health"]) {
      assert.match(source, new RegExp(`\\{ value: "${value}", label:`));
    }
    assert.match(source, /role="tabpanel"/);
    assert.match(source, /aria-labelledby=/);
  }

  assert.match(mcpSource, /idPrefix="mcp-tabs"/);
  assert.match(mcpSource, /ariaLabel=\{\$t\("MCP features"\)\}/);
  assert.match(actionsSource, /idPrefix="actions-tabs"/);
  assert.match(actionsSource, /ariaLabel=\{\$t\("Actions features"\)\}/);
});

test("Tabs 支援方向鍵、Home End 與完整 ARIA 關聯", async () => {
  const source = await read(tabsPath);

  assert.match(source, /case "ArrowLeft"/);
  assert.match(source, /case "ArrowRight"/);
  assert.match(source, /case "Home"/);
  assert.match(source, /case "End"/);
  assert.match(source, /aria-controls=\{panelId\(item\.value\)\}/);
  assert.match(source, /tabindex=\{value === item\.value \? 0 : -1\}/);
  assert.match(source, /aria-orientation="horizontal"/);
});

test("快速驗證會平行執行前端檢查與測試，完整 build 保持獨立", async () => {
  const pkg = JSON.parse(await read(packagePath));

  assert.equal(pkg.scripts["verify:fast"], "node scripts/verify-frontend.mjs");
  assert.equal(pkg.scripts.verify, "pnpm run verify:fast && pnpm run build");
});

test("GPT 配置卡片不再重複展示會話恢復入口", async () => {
  const source = await read(quickCopyPath);
  assert.doesNotMatch(source, /ChatGptSessionPrompt/);
});

test("會話恢復快捷入口預設緊湊，並可展開完整提示詞", async () => {
  const source = await read(sessionPromptPath);

  assert.match(source, /let expanded = \$state\(false\)/);
  assert.match(source, /aria-expanded=\{expanded\}/);
  assert.match(source, /\$t\("View full prompt"\)/);
  assert.match(source, /\{#if expanded\}[\s\S]*<pre/);
});

test("複製和展開操作保留可觸達尺寸與狀態回饋", async () => {
  const source = await read(sessionPromptPath);

  assert.ok((source.match(/min-h-11/g) ?? []).length >= 2, "兩個操作按鈕都應至少為 44px 高");
  assert.match(source, /aria-live="polite"/);
  assert.match(source, /\$t\("Copy full prompt"\)/);
  assert.match(source, /\$t\("Copied"\)/);
});
