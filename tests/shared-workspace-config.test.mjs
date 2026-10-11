import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import ts from "typescript";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const fixtureDir = path.join(root, "docs", "specs", "shared-workspace-config", "fixtures");
const libSrc = path.join(root, "src", "lib");

async function loadFixture(name) {
  return JSON.parse(await readFile(path.join(fixtureDir, name), "utf8"));
}

function rewriteRelativeImports(code) {
  return code.replaceAll(/from\s+["'](\.[^"']+)["']/g, (_, specifier) => {
    const withExt = specifier.endsWith(".js") ? specifier : `${specifier}.js`;
    return `from "${withExt}"`;
  });
}

async function compileTs(sourcePath, destPath) {
  const source = await readFile(sourcePath, "utf8");
  const compiled = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.ESNext,
      target: ts.ScriptTarget.ES2022,
    },
  }).outputText;
  await mkdir(path.dirname(destPath), { recursive: true });
  await writeFile(destPath, rewriteRelativeImports(compiled));
}

async function loadWorkspaceDocument() {
  const tmp = await mkdtemp(path.join(os.tmpdir(), "workspace-document-"));
  await compileTs(path.join(libSrc, "types.ts"), path.join(tmp, "types.js"));
  await compileTs(
    path.join(libSrc, "backend", "workspace-document.ts"),
    path.join(tmp, "backend", "workspace-document.js"),
  );
  return import(pathToFileURL(path.join(tmp, "backend", "workspace-document.js")).href);
}

function secretText(value) {
  return JSON.stringify(value);
}

test("canonical fixtures parse and roundtrip without secrets", async () => {
  const {
    parseCanonicalWorkspace,
    serializeCanonicalWorkspace,
    migrateNodeV1Document,
    migrateDesktopProfile,
  } = await loadWorkspaceDocument();

  const minimal = parseCanonicalWorkspace(await loadFixture("minimal.json"));
  assert.equal(minimal.id, "ws-minimal");
  assert.equal(minimal.bind.port, 3789);
  assert.equal(minimal.folders.length, 1);

  const full = parseCanonicalWorkspace(await loadFixture("full.json"));
  const roundtrip = parseCanonicalWorkspace(serializeCanonicalWorkspace(full));
  assert.equal(roundtrip.id, full.id);
  assert.deepEqual(roundtrip.folders, full.folders);
  assert.deepEqual(roundtrip.policy.allowedCommands, ["pytest", "cargo"]);
  assert.equal(roundtrip.tunnel.builtin.enabled, true);

  const desktop = parseCanonicalWorkspace(await loadFixture("desktop-extras.json"));
  assert.equal(desktop.host.desktop.tunnel.type, "frp");
  const desktopAgain = parseCanonicalWorkspace(serializeCanonicalWorkspace(desktop));
  assert.equal(desktopAgain.host.desktop.actions.oauthClientId, "actions-client");

  const node = parseCanonicalWorkspace(await loadFixture("node-extras.json"));
  assert.equal(node.host.node.management.enabled, true);
  assert.match(node.host.node.dataDir, /CodingToolsMCPNode/);
});

test("v1 Node and Desktop profiles migrate into canonical documents", async () => {
  const { migrateNodeV1Document, migrateDesktopProfile, serializeCanonicalWorkspace } =
    await loadWorkspaceDocument();

  const migrated = migrateNodeV1Document(await loadFixture("from-node-v1.json"), {
    id: "ws-v1",
    name: "V1",
  });
  assert.equal(migrated.id, "ws-v1");
  assert.equal(migrated.auth.oauthClientId, "chatgpt-v1");
  assert.equal(migrated.auth.type, "oauth");
  assert.equal(migrated.tunnel.builtin.publicUrl, "https://example.test/builtin/clients/v1/mcp");
  assert.equal(migrated.host.node.dataDir, "C:\\data\\node-v1");
  const serialized = secretText(serializeCanonicalWorkspace(migrated));
  assert.equal(serialized.includes("must-not-survive-migrate"), false);
  assert.equal(serialized.includes("enroll/SECRET"), false);

  const desktop = migrateDesktopProfile(await loadFixture("from-desktop-profile.json"));
  assert.equal(desktop.id, "desktop-1");
  assert.equal(desktop.bind.port, 18790);
  assert.equal(desktop.tunnel.builtin.enabled, true);
  assert.deepEqual(desktop.policy.allowedCommands, ["pytest", "cargo"]);
  assert.equal(desktop.auth.type, "oauth");
});

test("desktop profile roundtrip restores host-only fields", async () => {
  const { migrateDesktopProfile, canonicalToWorkspaceProfile } = await loadWorkspaceDocument();
  const source = await loadFixture("from-desktop-profile.json");
  source.auth.type = "bearer";
  source.auth.use_shared_secrets = true;
  source.runtime.transport_mode = "legacy-json";
  source.runtime.runtime_command = "custom-runtime";
  source.tunnel.type = "frp";
  source.tunnel.public_url = "https://dev.example.test/mcp";
  source.tunnel.frp_server = "frp.example";
  source.tunnel.frp_subdomain = "dev";
  source.tunnel.frp_profile_id = "profile-1";
  source.tunnel.frp_server_port = 7000;
  source.tunnel.use_proxy = false;
  source.folders[0].execution = { kind: "wsl", distro: "Ubuntu", linux_path: "/home/repo" };
  const restored = canonicalToWorkspaceProfile(migrateDesktopProfile(source));
  assert.equal(restored.auth.type, "bearer");
  assert.equal(restored.auth.use_shared_secrets, true);
  assert.equal(restored.runtime.transport_mode, "legacy-json");
  assert.equal(restored.runtime.runtime_command, "custom-runtime");
  assert.equal(restored.tunnel.type, "frp");
  assert.equal(restored.tunnel.public_url, "https://dev.example.test/mcp");
  assert.equal(restored.tunnel.frp_server, "frp.example");
  assert.equal(restored.folders[0].execution.kind, "wsl");
  assert.equal(restored.folders[0].execution.distro, "Ubuntu");
  assert.equal(restored.actions.oauth_client_id, "actions-desktop");
  assert.equal(restored.actions.local_port, 18791);
});

test('global connection round-trips zero folders while project documents require a root', async () => {
  const {parseCanonicalWorkspace,migrateNodeV1Document,migrateDesktopProfile,canonicalToWorkspaceProfile} = await loadWorkspaceDocument();
  const id='00000000000000000000000000000001';
  const canonical=migrateNodeV1Document({folders:[],oauth:{clientId:'global-client'}},{id,name:'MCP'});
  assert.deepEqual(canonical.folders,[]);
  const desktop=canonicalToWorkspaceProfile(canonical);
  assert.deepEqual(migrateDesktopProfile(desktop).folders,[]);
  assert.throws(()=>parseCanonicalWorkspace({...canonical,id:'project'}),/at least one folder/);
  assert.throws(()=>parseCanonicalWorkspace({...canonical,folders:[{id:'p',name:'p',path:'/project'}]}),/must not contain project folders/);
});
