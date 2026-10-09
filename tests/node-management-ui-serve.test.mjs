import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import ts from "typescript";
import { extractInlineAssets } from "../scripts/extract-sveltekit-inline.mjs";

const root = process.cwd();

async function compileTs(sourcePath) {
  const source = await readFile(sourcePath, "utf8");
  const compiled = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.ESNext,
      target: ts.ScriptTarget.ES2022,
    },
  }).outputText;
  const tmp = await mkdtemp(path.join(os.tmpdir(), "management-ui-"));
  const dest = path.join(tmp, "managementUi.js");
  await writeFile(dest, compiled);
  return import(pathToFileURL(dest).href);
}

function listen(server) {
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve(server.address().port));
  });
}

test("extractInlineAssets moves SvelteKit inline scripts and styles to hashed files", async () => {
  const outDir = await mkdtemp(path.join(os.tmpdir(), "svelte-inline-"));
  const original = `<!doctype html>
<html>
<head>
<style>body{color:red}</style>
</head>
<body>
<script>
{
  const element = document.currentScript.parentElement;
  Promise.all([import("/ui/_app/start.js")]).then(([kit]) => kit.start(element));
}
</script>
</body>
</html>`;
  await writeFile(path.join(outDir, "index.html"), original);
  const html = await extractInlineAssets(outDir, "/ui");
  assert.doesNotMatch(html, /<style\b/);
  assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)/);
  assert.match(html, /src="\/ui\/_app\/immutable\/chunks\/inline-[0-9a-f]+\.js"/);
  assert.match(html, /href="\/ui\/_app\/immutable\/chunks\/inline-[0-9a-f]+\.css"/);
  const scriptName = html.match(/src="\/ui\/_app\/immutable\/chunks\/(inline-[0-9a-f]+\.js)"/)[1];
  const script = await readFile(path.join(outDir, "_app", "immutable", "chunks", scriptName), "utf8");
  assert.match(script, /document\.currentScript/);
  assert.equal(script.includes("<script"), false);
});

test("handleManagementUiRequest serves the built Node Svelte artifact under /ui/", async () => {
  const assetRoot = path.join(root, "packages", "node-agent", "dist", "ui");
  let index;
  try {
    index = await readFile(path.join(assetRoot, "index.html"), "utf8");
  } catch {
    return;
  }
  assert.doesNotMatch(index, /<script(?![^>]*\bsrc=)/);
  assert.match(index, /\/ui\/_app\//);
  const { handleManagementUiRequest } = await compileTs(
    path.join(root, "packages", "node-agent", "src", "managementUi.ts"),
  );
  const token = "tok_built_artifact_1";
  const server = http.createServer((req, res) => {
    const pathname = new URL(req.url, "http://127.0.0.1").pathname;
    void handleManagementUiRequest(req, res, pathname, token, assetRoot);
  });
  const port = await listen(server);
  const base = `http://127.0.0.1:${port}`;
  try {
    const redirected = await fetch(`${base}/`, { redirect: "manual" });
    assert.equal(redirected.status, 302);
    assert.equal(redirected.headers.get("location"), "/ui/");
    const page = await fetch(`${base}/ui/`);
    assert.equal(page.status, 200);
    const csp = page.headers.get("content-security-policy") ?? "";
    assert.match(csp, /default-src 'none'/);
    assert.match(csp, /img-src 'self' blob: data:/);
    assert.match(csp, /script-src 'self'/);
    assert.doesNotMatch(csp, /unsafe-inline/);
    const html = await page.text();
    assert.match(html, new RegExp(`content="${token}"`));
    assert.match(html, /data-ui-framework="svelte"/);
    assert.match(html, /\/ui\/_app\//);
    assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)/);
    const js = html.match(/src="(\/ui\/[^"]+\.js)"/)?.[1];
    assert.ok(js);
    const asset = await fetch(`${base}${js}`);
    assert.equal(asset.status, 200);
    assert.match(asset.headers.get("content-type") ?? "", /javascript/);
    const bundle = await asset.text();
    assert.match(bundle, /document\.currentScript|__sveltekit_/);
    const jsFiles = [];
    async function collect(dir) {
      for (const entry of await readdir(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) await collect(full);
        else if (entry.name.endsWith(".js")) jsFiles.push(full);
      }
    }
    await collect(path.join(assetRoot, "_app"));
    const joined = (await Promise.all(jsFiles.map((file) => readFile(file, "utf8")))).join("\n");
    assert.doesNotMatch(joined, /__TAURI_INTERNALS__/);
    assert.match(joined, /host:`node`|host:"node"/);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});

test("handleManagementUiRequest serves injected Svelte HTML and hashed assets under /ui/", async () => {
  const { handleManagementUiRequest } = await compileTs(
    path.join(root, "packages", "node-agent", "src", "managementUi.ts"),
  );
  const assetRoot = await mkdtemp(path.join(os.tmpdir(), "node-ui-artifact-"));
  const jsName = `chunk-${createHash("sha256").update("ok").digest("hex").slice(0, 8)}.js`;
  await mkdir(path.join(assetRoot, "_app"), { recursive: true });
  await writeFile(
    path.join(assetRoot, "index.html"),
    `<!doctype html>
<html>
<head>
<meta name="ctmcp-admin-token" content="">
<meta name="description" content="Coding Tools MCP Headless Agent management interface">
<link rel="stylesheet" href="/ui/_app/${jsName.replace(".js", ".css")}">
</head>
<body data-ui-framework="svelte">
<script src="/ui/_app/${jsName}"></script>
</body>
</html>`,
  );
  await writeFile(path.join(assetRoot, "_app", jsName), "console.log('svelte-ui');");
  await writeFile(path.join(assetRoot, "_app", jsName.replace(".js", ".css")), ".svelte-app{display:contents}");

  const token = "tok_test_admin_1";
  const server = http.createServer((req, res) => {
    const pathname = new URL(req.url, "http://127.0.0.1").pathname;
    void handleManagementUiRequest(req, res, pathname, token, assetRoot);
  });
  const port = await listen(server);
  const base = `http://127.0.0.1:${port}`;
  try {
    const redirected = await fetch(`${base}/`, { redirect: "manual" });
    assert.equal(redirected.status, 302);
    assert.equal(redirected.headers.get("location"), "/ui/");

    const page = await fetch(`${base}/ui/`);
    assert.equal(page.status, 200);
    const csp = page.headers.get("content-security-policy");
    assert.match(csp, /default-src 'none'/);
    assert.match(csp, /img-src 'self' blob: data:/);
    assert.match(csp, /script-src 'self'/);
    assert.doesNotMatch(csp, /unsafe-inline/);
    const html = await page.text();
    assert.match(html, new RegExp(`content="${token}"`));
    assert.match(html, /data-ui-framework="svelte"/);
    assert.match(html, new RegExp(`/ui/_app/${jsName}`));
    assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)/);

    const asset = await fetch(`${base}/ui/_app/${jsName}`);
    assert.equal(asset.status, 200);
    assert.match(asset.headers.get("content-type"), /javascript/);
    assert.equal(await asset.text(), "console.log('svelte-ui');");

    const rejected = await fetch(`${base}/ui/_app/${jsName}`, { method: "POST" });
    assert.equal(rejected.status, 405);

    const unprefixed = await fetch(`${base}/workspace/10ff0a7d2cfe4acc8b4b9b30a8a92bfd?tab=overview`, {
      redirect: "manual",
    });
    assert.equal(unprefixed.status, 302);
    assert.equal(
      unprefixed.headers.get("location"),
      "/ui/workspace/10ff0a7d2cfe4acc8b4b9b30a8a92bfd?tab=overview",
    );
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});
