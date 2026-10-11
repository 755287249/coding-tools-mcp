import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const workspacePagePath = new URL(
  "../src/routes/workspace/[id]/+page.svelte",
  import.meta.url,
);

test("MCP tool and command policy changes hot-apply without a restart prompt", async () => {
  const page = await readFile(workspacePagePath, "utf8");
  const start = page.indexOf("async function saveMcpPolicy");
  const end = page.indexOf("async function saveActionsPolicy", start);
  assert.ok(start >= 0 && end > start, "saveMcpPolicy function should be present");
  const savePolicy = page.slice(start, end);
  assert.match(savePolicy, /persistProfile\("save\.mcp\.policy"/);
  assert.doesNotMatch(savePolicy, /promptServiceRestart|restartRuntime/);
  for(const field of ["allowedCommands","workspaceLocalEntries","workspaceScriptExtensions","securityPolicy"]) assert.ok(savePolicy.includes(`draft.${field}`));
});
