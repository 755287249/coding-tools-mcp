import assert from "node:assert/strict";
import { mkdir, mkdtemp, readdir, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import ts from "typescript";

const root = process.cwd();
const backendDir = path.join(root, "src", "lib", "backend");
const srcDir = path.join(root, "src");

async function listSourceFiles(directory, predicate) {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(
    entries.map(async (entry) => {
      const absolute = path.join(directory, entry.name);
      if (entry.isDirectory()) return listSourceFiles(absolute, predicate);
      return predicate(absolute) ? [absolute] : [];
    }),
  );
  return nested.flat();
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

async function importBackendRuntime() {
  const tmp = await mkdtemp(path.join(os.tmpdir(), "frontend-backend-"));
  const files = [
    "capabilities.ts",
    "errors.ts",
    "tauri.ts",
    "node.ts",
    "node-map.ts",
    "workspace-document.ts",
    "secret-read.ts",
    "index.ts",
  ];
  await Promise.all([
    compileTs(path.join(srcDir, "lib", "chat", "cache-lifecycle.js"), path.join(tmp, "chat", "cache-lifecycle.js")),
    compileTs(path.join(srcDir, "lib", "types.ts"), path.join(tmp, "types.js")),
    ...files.map((name) =>
      compileTs(path.join(backendDir, name), path.join(tmp, "backend", name.replace(/\.ts$/, ".js"))),
    ),
  ]);
  return import(pathToFileURL(path.join(tmp, "backend", "index.js")).href);
}

function fakeDialog() {
  return {
    async open() {
      return null;
    },
    async confirm() {
      return false;
    },
    async message() {},
  };
}

test("desktop and node capabilities encode the product boundary", async () => {
  const { DESKTOP_CAPABILITIES, NODE_CAPABILITIES } = await importBackendRuntime();

  assert.equal(DESKTOP_CAPABILITIES.host, "desktop");
  assert.equal(NODE_CAPABILITIES.host, "node");

  for (const flag of [
    "actions",
    "frpManagement",
    "nativeDirectoryPicker",
    "softwareManagement",
    "rawRuntimeLogs",
    "staticBearerAuth",
    "liveHistoryActivity",
    "wslFolders",
    "runtimeSupervisor",
    "sharedSecretStore",
  ]) {
    assert.equal(DESKTOP_CAPABILITIES[flag], true, `desktop should expose ${flag}`);
    assert.equal(NODE_CAPABILITIES[flag], false, `node should hide ${flag}`);
  }

  assert.equal(DESKTOP_CAPABILITIES.guidedSetup, true, "desktop should expose guided setup");
  assert.equal(NODE_CAPABILITIES.guidedSetup, true, "node should expose guided setup");

  for (const flag of ["agentRestart", "directoryBrowser", "operationLogs", "openNativePath", "workspaceLifecycle", "workspaceFeatureControls"]) {
    assert.equal(NODE_CAPABILITIES[flag], true, `node should expose ${flag}`);
  }
  for (const flag of ["agentRestart", "directoryBrowser", "operationLogs"]) {
    assert.equal(DESKTOP_CAPABILITIES[flag], false, `desktop should hide ${flag}`);
  }
  assert.equal(DESKTOP_CAPABILITIES.workspaceFeatureControls, true, "desktop should expose shared workspace feature controls");
  assert.equal(NODE_CAPABILITIES.workspaceFeatureControls, true, "node should expose shared workspace feature controls");
});

test("TauriBackend workspaces and telemetry go through injected invoke", async () => {
  const { createTauriBackend } = await importBackendRuntime();
  const calls = [];
  const backend = createTauriBackend({
    invoke: async (cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === "list_workspaces") return [{ id: "ws-1" }];
      if (cmd === "read_workspace_telemetry") return { workspace_id: args.id, records: [] };
      return null;
    },
    dialog: fakeDialog(),
  });

  assert.equal(backend.capabilities.host, "desktop");
  assert.deepEqual(await backend.workspaces.list(), [{ id: "ws-1" }]);
  await backend.telemetry.query("ws-1", { limit: 20, errorsOnly: true });

  assert.equal(calls[0].cmd, "list_workspaces");
  assert.equal(calls[1].cmd, "read_workspace_telemetry");
  assert.deepEqual(calls[1].args, {
    id: "ws-1",
    limit: 20,
    errorsOnly: true,
    minDurationMs: undefined,
    sinceTsMs: undefined,
  });
});

test("TauriBackend native dialogs wrap the injected picker and confirm", async () => {
  const { createTauriBackend } = await importBackendRuntime();
  const backend = createTauriBackend({
    invoke: async () => null,
    dialog: {
      async open(options) {
        assert.equal(options.directory, true);
        assert.equal(options.multiple, false);
        return "D:\\repo";
      },
      async confirm(message, options) {
        assert.match(message, /delete/i);
        assert.equal(options.kind, "warning");
        return true;
      },
      async message() {},
    },
  });

  assert.equal(await backend.native.pickDirectory({ multiple: false }), "D:\\repo");
  assert.equal(await backend.native.confirm("Delete workspace?", { kind: "warning" }), true);
});

test("TauriBackend throws CapabilityError for Node-only surfaces", async () => {
  const { CapabilityError, createTauriBackend } = await importBackendRuntime();
  const backend = createTauriBackend({
    invoke: async () => null,
    dialog: fakeDialog(),
  });

  await assert.rejects(() => backend.agent.restart(), (error) => {
    assert.equal(error instanceof CapabilityError, true);
    assert.equal(error.capability, "agentRestart");
    return true;
  });
  await assert.rejects(() => backend.directories.browse(), (error) => {
    assert.equal(error.capability, "directoryBrowser");
    return true;
  });
  await assert.rejects(() => backend.operations.query("ws", {
    folderId: "f",
    status: "all",
    tool: "",
    errorsOnly: false,
    limit: 20,
  }), (error) => {
    assert.equal(error.capability, "operationLogs");
    return true;
  });
});

test("TauriBackend maps shared Skills, Hooks, and MCP controls to Rust commands", async () => {
  const { createTauriBackend } = await importBackendRuntime();
  const calls = [];
  const backend = createTauriBackend({
    invoke: async (cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === "get_workspace_skills") return { ok: true, workspaceId: args.workspaceId, active: true, skills: [], diagnostics: [] };
      if (cmd === "get_workspace_extensions") return { ok: true, workspaceId: args.workspaceId, hooksActive: true, mcpActive: true, hooks: [], mcpServers: [], diagnostics: [] };
      return { ok: true };
    },
    dialog: fakeDialog(),
  });

  assert.equal(backend.capabilities.workspaceFeatureControls, true);
  await backend.workspaceFeatures.skills("ws-1");
  await backend.workspaceFeatures.setSkillsActive("ws-1", false);
  await backend.workspaceFeatures.setSkillEnabled("ws-1", "skill-key", true);
  await backend.workspaceFeatures.extensions("ws-1");
  await backend.workspaceFeatures.setExtensionActive("ws-1", "hook", false);
  await backend.workspaceFeatures.setExtensionEnabled("ws-1", "mcp", "server-key", true);

  assert.deepEqual(calls, [
    { cmd: "get_workspace_skills", args: { workspaceId: "ws-1" } },
    { cmd: "set_workspace_skills_active", args: { workspaceId: "ws-1", active: false } },
    { cmd: "set_workspace_skill_enabled", args: { workspaceId: "ws-1", skillKey: "skill-key", enabled: true } },
    { cmd: "get_workspace_extensions", args: { workspaceId: "ws-1" } },
    { cmd: "set_workspace_extension_active", args: { workspaceId: "ws-1", extensionKind: "hook", active: false } },
    { cmd: "set_workspace_extension_enabled", args: { workspaceId: "ws-1", extensionKind: "mcp", extensionKey: "server-key", enabled: true } },
  ]);
});

test("NodeBackend maps shared observability routes and keeps admin token / same-origin request contract", async () => {
  const { NODE_CAPABILITIES, createNodeBackend } = await importBackendRuntime();
  const calls = [];
  const backend = createNodeBackend({
    request: async (route, init = {}, signal) => {
      calls.push({ route, init, signal });
      if (route.includes("/telemetry")) {
        return {
          scanned_lines: 4,
          matched_lines: 2,
          records: [{ tool: "exec" }],
          warnings: [],
        };
      }
      if (route.includes("/history/3")) {
        return {
          number: 3,
          title: "session",
          records: [],
          content: "# session",
          checkpointCount: 1,
        };
      }
      if (route.includes("/history?")) {
        return {
          folder: { name: "docs/history-session" },
          sessions: [{ number: 3, title: "session", checkpointCount: 1 }],
          integrity: { missingNumbers: [], invalidFiles: [], emptyFiles: [] },
        };
      }
      if (route.endsWith("/health")) {
        return { items: [{ label: "MCP", ok: true, detail: "401", hint: "ok" }] };
      }
      if (route === "/admin/api/restart") {
        return { ok: true, restarting: true };
      }
      return {};
    },
  });

  assert.deepEqual(backend.capabilities, NODE_CAPABILITIES);

  const telemetry = await backend.telemetry.query("ws-1", { errorsOnly: true, limit: 10 });
  assert.equal(telemetry.workspace_id, "ws-1");
  assert.equal(telemetry.scanned_lines, 4);
  assert.equal(telemetry.records[0].tool, "exec");

  const history = await backend.history.list("ws-1", "folder-1");
  assert.equal(history.sessions[0].activityStatus, "completed");
  assert.equal(history.sessions[0].sessionKey, null);

  const detail = await backend.history.read("ws-1", 3, "folder-1");
  assert.equal(detail.content, "# session");

  const health = await backend.health.run("ws-1");
  assert.deepEqual(health, [{ label: "MCP", ok: true, detail: "401", hint: "ok" }]);

  await backend.agent.restart();

  assert.match(calls[0].route, /\/admin\/api\/workspaces\/ws-1\/telemetry\?/);
  assert.match(calls[0].route, /errorsOnly=true/);
  assert.match(calls[1].route, /\/admin\/api\/workspaces\/ws-1\/history\?folderId=folder-1/);
  assert.equal(calls[3].init.method, "POST");
  assert.equal(calls[4].route, "/admin/api/restart");
  assert.equal(calls[4].init.method, "POST");
});

test("NodeBackend maps Skills, Hooks, and external MCP workspace controls", async () => {
  const { createNodeBackend } = await importBackendRuntime();
  const calls = [];
  const backend = createNodeBackend({
    request: async (route, init = {}) => {
      calls.push({ route, init });
      if (route.endsWith("/skills") && !init.method) {
        return { ok: true, workspaceId: "ws-1", active: true, skills: [], diagnostics: [] };
      }
      if (route.endsWith("/extensions") && !init.method) {
        return { ok: true, workspaceId: "ws-1", hooksActive: true, mcpActive: true, hooks: [], mcpServers: [], diagnostics: [] };
      }
      return { ok: true, workspaceId: "ws-1", restartRequired: false };
    },
  });

  await backend.workspaceFeatures.skills("ws-1");
  await backend.workspaceFeatures.extensions("ws-1");
  await backend.workspaceFeatures.setSkillsActive("ws-1", false);
  await backend.workspaceFeatures.setSkillEnabled("ws-1", "skill-a", true);
  await backend.workspaceFeatures.setExtensionActive("ws-1", "hook", false);
  await backend.workspaceFeatures.setExtensionEnabled("ws-1", "mcp", "server-a", true);

  assert.equal(calls[0].route, "/admin/api/workspaces/ws-1/skills");
  assert.equal(calls[1].route, "/admin/api/workspaces/ws-1/extensions");
  assert.deepEqual(JSON.parse(calls[2].init.body), { active: false });
  assert.deepEqual(JSON.parse(calls[3].init.body), { key: "skill-a", enabled: true });
  assert.deepEqual(JSON.parse(calls[4].init.body), { kind: "hook", active: false });
  assert.deepEqual(JSON.parse(calls[5].init.body), { kind: "mcp", key: "server-a", enabled: true });
  for (const call of calls.slice(2)) assert.equal(call.init.method, "PUT");
});

test("NodeBackend keeps desktop-only product features behind CapabilityError", async () => {
  const { CapabilityError, createNodeBackend } = await importBackendRuntime();
  const backend = createNodeBackend({
    request: async () => {
      throw new Error("request should not run for capability-gated methods");
    },
  });

  await assert.rejects(() => backend.software.list(), (error) => {
    assert.equal(error instanceof CapabilityError, true);
    assert.equal(error.capability, "softwareManagement");
    return true;
  });
  await assert.rejects(() => backend.workspaces.startRuntime("ws"), (error) => {
    assert.equal(error.capability, "runtimeSupervisor");
    return true;
  });
  await assert.rejects(() => backend.native.pickDirectory(), (error) => {
    assert.equal(error.capability, "nativeDirectoryPicker");
    return true;
  });
  await assert.rejects(() => backend.workspaces.startActionsRuntime("ws"), (error) => {
    assert.equal(error.capability, "actions");
    return true;
  });
});

test("NodeBackend creates workspaces and opens folders through management routes", async () => {
  const { createNodeBackend } = await importBackendRuntime();
  const calls = [];
  const snapshot = {
    primaryWorkspaceId: "ws-1",
    workspaces: [
      {
        id: "ws-1",
        name: "Demo",
        effective: {
          host: "127.0.0.1",
          port: 3789,
          publicBaseUrl: "",
          dataDir: "D:\\\\data",
          permissionMode: "trusted",
          toolProfile: "core",
          activeToolProfile: "trusted-core",
          securityPolicy: {},
          management: { enabled: true },
          sandbox: { enabled: false, backend: "appcontainer", externalPaths: [], options: {} },
          oauth: { clientId: "client", passwordConfigured: true, clientSecretConfigured: false, tokenSecretSource: "generated" },
          policy: { allowedCommands: [], workspaceLocalEntries: true, workspaceScriptExtensions: [], maxPatchBytes: 1000 },
          folders: [{ id: "folder-1", name: "repo", path: "D:\\\\repo" }],
          limits: {
            blockingConcurrency: 8,
            processConcurrency: 4,
            globalBlockingConcurrency: 16,
            globalProcessConcurrency: 8,
            activeSessionLimit: 16,
            maxOutputBytes: 4096,
            commandTimeoutMaxMs: 1000,
          },
          tunnel: { enabled: true, publicUrl: "", enrollmentConfigured: false },
        },
        saved: null,
      },
    ],
  };
  snapshot.workspaces[0].saved = snapshot.workspaces[0].effective;
  const created = {
    ...snapshot.workspaces[0],
    id: "ws-2",
    name: "Other",
    effective: {
      ...snapshot.workspaces[0].effective,
      port: 3790,
      folders: [{ id: "folder-2", name: "other", path: "D:\\\\other" }],
    },
  };
  created.saved = created.effective;
  const backend = createNodeBackend({
    request: async (route, init = {}) => {
      calls.push({ route, init });
      if (route === "/admin/api/workspaces" && init.method === "POST") {
        snapshot.workspaces.push(created);
        return { id: "ws-2", name: "Other", restartRequired: true };
      }
      if (route === "/admin/api/config") return snapshot;
      if (route === "/admin/api/directories/open") return { ok: true, path: "D:\\\\repo" };
      if (route.endsWith("/tunnel/start")) return { state: "running", publicUrl: "https://example.test" };
      throw new Error(`unexpected route ${route}`);
    },
  });

  const workspace = await backend.workspaces.create("D:\\\\other", "Other");
  assert.equal(workspace.id, "ws-2");
  await backend.workspaces.openDirectory("D:\\\\repo");
  const tunnel = await backend.tunnel.start("ws-2", "mcp");
  assert.equal(tunnel.state, "running");
  assert.equal(calls[0].route, "/admin/api/workspaces");
  assert.equal(calls[2].route, "/admin/api/directories/open");
});

test("NodeBackend maps /admin/api/config workspaces onto WorkspaceProfile", async () => {
  const { createNodeBackend } = await importBackendRuntime();
  const calls = [];
  const snapshot = {
    primaryWorkspaceId: "ws-1",
    workspaces: [
      {
        id: "ws-1",
        name: "Demo",
        effective: {
          host: "127.0.0.1",
          port: 8788,
          publicBaseUrl: "https://example.test",
          dataDir: "D:\\\\data",
          permissionMode: "trusted",
          toolProfile: "advanced",
          activeToolProfile: "advanced",
          securityPolicy: { restrictToolCatalog: true, redactHistory: true },
          management: { enabled: true },
          sandbox: { enabled: false, backend: "appcontainer", externalPaths: [], options: {} },
          oauth: { clientId: "client", passwordConfigured: true, clientSecretConfigured: false, tokenSecretSource: "generated" },
          policy: { allowedCommands: ["pytest"], workspaceLocalEntries: true, workspaceScriptExtensions: [".exe"], maxPatchBytes: 1000 },
          folders: [{ id: "folder-1", name: "repo", path: "D:\\\\repo" }],
          limits: {
            blockingConcurrency: 8,
            processConcurrency: 4,
            globalBlockingConcurrency: 16,
            globalProcessConcurrency: 8,
            activeSessionLimit: 16,
            maxOutputBytes: 4096,
            commandTimeoutMaxMs: 1000,
          },
          tunnel: { enabled: true, publicUrl: "https://example.test", enrollmentConfigured: true },
        },
        saved: null,
      },
    ],
  };
  snapshot.workspaces[0].saved = {
    ...snapshot.workspaces[0].effective,
    folders: [
      ...snapshot.workspaces[0].effective.folders,
      { id: "folder-2", name: "linux", path: "/workspace/linux" },
    ],
  };
  const backend = createNodeBackend({
    request: async (route, init = {}) => {
      calls.push({ route, init });
      if (route === "/admin/api/config") return snapshot;
      throw new Error(`unexpected route ${route}`);
    },
  });
  const [workspace] = await backend.workspaces.list();
  assert.equal(workspace.id, "ws-1");
  assert.equal(workspace.name, "Demo");
  assert.equal(workspace.folders[0].path, "D:\\\\repo");
  assert.equal(workspace.folders[1].path, "/workspace/linux");
  assert.equal(workspace.auth.type, "oauth");
  assert.equal(workspace.runtime.local_port, 8788);
  assert.equal(await backend.settings.getLastWorkspaceId(), "ws-1");
  assert.equal(calls[0].route, "/admin/api/config");
});

test("NodeBackend persists the built-in tunnel enrollment secret through the management API", async () => {
  const { createNodeBackend } = await importBackendRuntime();
  const calls = [];
  const backend = createNodeBackend({
    request: async (route, init = {}) => {
      calls.push({ route, init });
      return { ok: true };
    },
  });

  await backend.secrets.setWorkspaceSecret(
    "ws-1",
    "builtin_tunnel_enrollment_url",
    "https://example.test/enroll/once",
  );

  assert.equal(calls.length, 1);
  assert.equal(calls[0].route, "/admin/api/workspaces/ws-1/secrets/builtin-tunnel-enrollment-url");
  assert.equal(calls[0].init.method, "PUT");
  assert.equal(calls[0].init.headers?.["content-type"], "application/json");
  assert.deepEqual(JSON.parse(calls[0].init.body), { value: "https://example.test/enroll/once" });
});

test("loadMcpAuthSecrets drives NodeBackend with only oauth-password for MCP OAuth", async () => {
  const { createNodeBackend, loadMcpAuthSecrets } = await importBackendRuntime();
  const calls = [];
  const backend = createNodeBackend({
    request: async (route) => {
      calls.push(route);
      if (route.endsWith("/secrets/oauth-password")) return { value: "one-time-pw" };
      throw new Error(`unexpected route ${route}`);
    },
  });

  const secrets = await loadMcpAuthSecrets(backend, "ws-1", {
    type: "oauth",
    oauth_client_id: "client-from-profile",
    use_shared_secrets: true,
  });

  assert.equal(secrets.oauth_client_id, "client-from-profile");
  assert.equal(secrets.oauth_client_secret, "");
  assert.equal(secrets.oauth_password, "one-time-pw");
  assert.equal(secrets.bearer_token, "");
  assert.deepEqual(calls, ["/admin/api/workspaces/ws-1/secrets/oauth-password"]);
});

test("shared Svelte pages gate Actions, FRP, bearer auth, and runtime controls", async () => {
  const overview = await readFile(
    path.join(srcDir, "lib", "components", "workspace", "WorkspaceOverview.svelte"),
    "utf8",
  );
  const gptCopy = await readFile(path.join(srcDir, "lib", "components", "GptQuickCopy.svelte"), "utf8");
  const authForm = await readFile(path.join(srcDir, "lib", "components", "AuthConfigForm.svelte"), "utf8");
  const tunnelForm = await readFile(
    path.join(srcDir, "lib", "components", "TunnelConfigForm.svelte"),
    "utf8",
  );
  const workspacePage = await readFile(path.join(srcDir, "routes", "workspace", "[id]", "+page.svelte"), "utf8");
  const featureControls = await readFile(
    path.join(srcDir, "lib", "components", "workspace", "WorkspaceFeatureControls.svelte"),
    "utf8",
  );

  assert.match(overview, /capabilities\.runtimeSupervisor/);
  assert.match(overview, /capabilities\.actions/);
  assert.match(gptCopy, /loadMcpAuthSecrets\(getBackend\(\)/);
  assert.match(authForm, /capabilities\.staticBearerAuth/);
  assert.match(authForm, /workspaceAuthSecretKeys\(authType, capabilities\)/);
  assert.match(tunnelForm, /capabilities\.frpManagement/);
  assert.match(tunnelForm, /if \(!capabilities\.frpManagement\) return;/);
  assert.match(workspacePage, /capabilities\.workspaceFeatureControls/);
  assert.match(workspacePage, /WorkspaceFeatureControls/);
  assert.match(featureControls, /backend\.setSkillsActive/);
  assert.match(featureControls, /backend\.setSkillEnabled/);
  assert.match(featureControls, /backend\.setExtensionActive/);
  assert.match(featureControls, /backend\.setExtensionEnabled/);
  assert.match(featureControls, /import Tabs from "\$lib\/components\/Tabs\.svelte"/);
  assert.match(featureControls, /idPrefix="workspace-feature-tabs"/);
  assert.match(featureControls, /activeTab === "skills"/);
  assert.match(featureControls, /activeTab === "hooks"/);
  assert.match(featureControls, /activeTab === "mcp"/);
});

test("Svelte UI talks to FrontendBackend instead of importing Tauri plugins", async () => {
  const files = await listSourceFiles(srcDir, (file) => /\.(ts|svelte)$/.test(file));
  const tauriImports = [];

  for (const file of files) {
    const source = await readFile(file, "utf8");
    const relative = path.relative(root, file).replaceAll("\\", "/");
    if (/from\s+["']@tauri-apps\//.test(source)) {
      tauriImports.push(relative);
    }
  }

  assert.deepEqual(tauriImports, ["src/lib/backend/desktop.ts"]);

  const apiFiles = await listSourceFiles(path.join(srcDir, "lib", "api"), (file) => file.endsWith(".ts"));
  for (const file of apiFiles) {
    const source = await readFile(file, "utf8");
    if (path.basename(file) === "native.ts") {
      assert.match(source, /getBackend\(\)\.native/);
      continue;
    }
    assert.match(source, /getBackend\(\)/);
    assert.doesNotMatch(source, /@tauri-apps/);
  }
});

test("app bootstrap installs the desktop backend before UI code runs", async () => {
  const layoutSvelte = await readFile(path.join(srcDir, "routes", "+layout.svelte"), "utf8");
  const folderManager = await readFile(
    path.join(srcDir, "lib", "components", "WorkspaceFolderManager.svelte"),
    "utf8",
  );

  assert.match(layoutSvelte, /installHostBackend\(\)/);
  assert.match(layoutSvelte, /pickDirectory\(/);
  assert.match(layoutSvelte, /capabilities\.frpManagement/);
  assert.match(layoutSvelte, /capabilities\.softwareManagement/);
  assert.match(layoutSvelte, /goto\(appUrl\(`\/workspace\/\$\{/);
  assert.match(folderManager, /capabilities\.wslFolders/);
  assert.match(folderManager, /pickDirectory\(/);
});

test("appUrl prefixes SvelteKit paths.base so Node /ui/ navigations stay in the app", async () => {
  const tmp = await mkdtemp(path.join(os.tmpdir(), "app-path-"));
  await writeFile(path.join(tmp, "paths.js"), 'export const base = "/ui";\n');
  const source = (await readFile(path.join(srcDir, "lib", "app-path.ts"), "utf8")).replace(
    "$app/paths",
    "./paths.js",
  );
  const compiled = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.ESNext,
      target: ts.ScriptTarget.ES2022,
    },
  }).outputText;
  await writeFile(path.join(tmp, "app-path.js"), compiled);
  const { withAppBase, withoutAppBase, appUrl, routePath } = await import(
    pathToFileURL(path.join(tmp, "app-path.js")).href
  );

  assert.equal(withAppBase("/workspace/abc", ""), "/workspace/abc");
  assert.equal(withAppBase("/", ""), "/");
  assert.equal(withAppBase("/workspace/abc", "/ui"), "/ui/workspace/abc");
  assert.equal(withAppBase("/", "/ui"), "/ui/");
  assert.equal(withoutAppBase("/ui/workspace/abc", "/ui"), "/workspace/abc");
  assert.equal(withoutAppBase("/ui/", "/ui"), "/");
  assert.equal(withoutAppBase("/ui", "/ui"), "/");
  assert.equal(appUrl("/workspace/abc"), "/ui/workspace/abc");
  assert.equal(routePath("/ui/workspace/abc"), "/workspace/abc");
});
