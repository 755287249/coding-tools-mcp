import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

// SeedLibrary renders `seeds.reason.${seed.reason}`; a reason the backend writes
// without a catalog entry used to throw and freeze the "Manage seeds" view.
async function catalog() {
  const source = await readFile("src/lib/i18n/catalog.ts", "utf8");
  const compiled = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
  return (await import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`)).MESSAGES;
}

test("every seed reason written by Node or Rust has a translation", async () => {
  const messages = await catalog();
  const node = await readFile("packages/node-agent/src/chat/seeds.ts", "utf8");
  const rust = await readFile("src-tauri/src/tools/chat_seeds.rs", "utf8");
  const reasons = new Set([
    ...[...node.matchAll(/reason\s*[=:]\s*'([a-z_]+)'/g)].map((m) => m[1]),
    ...[...rust.matchAll(/reason\s*=\s*"([a-z_]+)"/g)].map((m) => m[1]),
    ...[...rust.matchAll(/retire[a-z_]*\([^)]*?"([a-z_]+)"/g)].map((m) => m[1]).filter((r) => r !== "id"),
    ...[...node.matchAll(/retirement\(\s*\w+\s*,\s*'([a-z_]+)'/g)].map((m) => m[1]),
  ]);
  assert.ok(reasons.has("standby"), "the scan should find the standby reason");
  for (const reason of reasons) assert.ok(messages[`seeds.reason.${reason}`], `missing seeds.reason.${reason}`);
});

test("translation of an unknown key returns the key instead of throwing", async () => {
  const source = await readFile("src/lib/i18n/index.ts", "utf8");
  assert.match(source, /hasOwnProperty\.call\(MESSAGES, key\)/);
  assert.match(source, /if \(!messages\) return interpolate\(String\(key\), values\);/);
});
