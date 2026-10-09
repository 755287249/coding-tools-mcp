import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";

const root = process.cwd();
const wizardPath = path.join(root, "src", "routes", "quick-setup", "+page.svelte");
const tunnelSetupPath = path.join(
  root,
  "src",
  "lib",
  "components",
  "quick-setup",
  "QuickTunnelSetup.svelte",
);

test("quick setup is an independent five-step route reachable from the app shell", async () => {
  const [wizard, layout, shell] = await Promise.all([
    readFile(wizardPath, "utf8"),
    readFile(path.join(root, "src", "routes", "+layout.svelte"), "utf8"),
    readFile(path.join(root, "src", "lib", "components", "SimpleShell.svelte"), "utf8"),
  ]);

  assert.match(
    wizard,
    /type WizardStep = "provider" \| "workspace" \| "service" \| "connect" \| "complete"/,
  );
  assert.match(wizard, /let step = \$state<WizardStep>\("provider"\)/);
  assert.match(layout, /workspaceMatch/);
  assert.match(layout, /onQuickSetup=\{capabilities\.guidedSetup \? openQuickSetup : undefined\}/);
  assert.match(shell, /\$t\("Quick setup"\)/);
  assert.match(shell, /\$t\('New workspace \(separate MCP\)'\)/, "the original add-workspace entry remains available");
});

test("the first step keeps all providers for desktop and gates unsupported Node providers", async () => {
  const wizard = await readFile(wizardPath, "utf8");

  assert.match(wizard, /type TunnelProvider = "builtin" \| "frp" \| "cloudflare"/);
  assert.match(wizard, /const capabilities = getBackend\(\)\.capabilities/);
  assert.match(wizard, /const frpAvailable = capabilities\.frpManagement && capabilities\.softwareManagement/);
  assert.match(wizard, /const cloudflareAvailable = capabilities\.softwareManagement/);
  assert.match(wizard, /chooseProvider\("builtin"\)/);
  assert.match(wizard, /chooseProvider\("frp"\)/);
  assert.match(wizard, /chooseProvider\("cloudflare"\)/);
  assert.match(wizard, /disabled=\{!frpAvailable\}/);
  assert.match(wizard, /disabled=\{!cloudflareAvailable\}/);
  assert.match(wizard, /\{ value: "provider", label: "Tunnel" \}/);
});

test("provider setup reuses managed software and global FRP profile contracts", async () => {
  const setup = await readFile(tunnelSetupPath, "utf8");

  assert.match(setup, /listSoftware\(\)/);
  assert.match(setup, /installSoftware\(softwareKind\)/);
  assert.match(setup, /provider === "frp" \? listFrpProfiles\(\)/);
  assert.match(setup, /saveFrpProfile\(/);
  assert.match(setup, /softwareKind = \$derived\(provider === "frp" \? "frpc" : "cloudflared"\)/);
  assert.match(setup, /cloudflareMode = \$state<CloudflareMode>\("quick"\)/);
  assert.match(setup, /Quick Tunnel/);
  assert.match(setup, /Named Tunnel/);
});

test("quick setup saves source-specific settings and uses the host-supported start path", async () => {
  const wizard = await readFile(wizardPath, "utf8");

  assert.match(wizard, /pickDirectory\(\{ multiple: true \}\)/);
  assert.match(wizard, /createWorkspace\(primaryFolder, workspaceName\.trim\(\) \|\| undefined\)/);
  assert.match(wizard, /for \(const folderPath of additionalFolders\)/);
  assert.match(wizard, /addWorkspaceFolder\(created\.id, folderPath\)/);
  assert.match(wizard, /setWorkspaceSecret\(workspaceId, key, value\)/);
  assert.match(wizard, /"builtin_tunnel_enrollment_url"/);
  assert.match(wizard, /"cloudflare_token" : "actions_cloudflare_token"/);
  assert.match(wizard, /\/clients\/\$\{workspaceId\}\/mcp/);
  assert.match(wizard, /frp_subdomain: frpSubdomain/);
  assert.match(wizard, /startRuntime\(workspaceId\)/);
  assert.match(wizard, /startActionsRuntime\(workspaceId\)/);
  assert.match(wizard, /startTunnel\(workspaceId, targetService\)/);
  assert.match(wizard, /disabled=\{!capabilities\.actions\}/);
  assert.match(wizard, /<GptQuickCopy/);
});

test("Node quick setup reuses the selected existing workspace instead of invoking a native picker", async () => {
  const wizard = await readFile(wizardPath, "utf8");

  assert.match(wizard, /const isNodeHost = capabilities\.host === "node"/);
  assert.match(wizard, /async function loadNodeWorkspace\(\)/);
  assert.match(wizard, /\$page\.url\.searchParams\.get\("workspace"\)/);
  assert.match(wizard, /items\.find\(\(item\) => item\.id === requestedId\) \?\? items\[0\] \?\? null/);
  assert.match(wizard, /\{:else if isNodeHost\}[\s\S]*Continue with this workspace/);
});

test("MCP completion explains advanced OAuth, empty client secret, then connection", async () => {
  const [wizard, quickCopy] = await Promise.all([
    readFile(wizardPath, "utf8"),
    readFile(path.join(root, "src", "lib", "components", "GptQuickCopy.svelte"), "utf8"),
  ]);

  assert.match(wizard, /Paste the Public MCP endpoint shown here and choose OAuth authentication\./);
  assert.match(wizard, /Expand Advanced OAuth settings, enter the Client ID shown here, leave Client Secret empty, and keep the other OAuth settings at their defaults\./);
  assert.match(wizard, /Select Next, click Connect, then enter the one-time password shown here\./);
  assert.match(wizard, /guidedMcp=\{service === "mcp"\}/);
  assert.match(quickCopy, /\{#if !guidedMcp && getBackend\(\)\.capabilities\.staticBearerAuth\}/);
  assert.match(quickCopy, /guidedMcp \? \$t\("One-time password"\)/);
});

test("quick setup starts the local service before testing and retaining the tunnel", async () => {
  const wizard = await readFile(wizardPath, "utf8");
  const enableIndex = wizard.indexOf("async function verifyAndEnable");
  const enableFlow = wizard.slice(enableIndex, wizard.indexOf("function goBack", enableIndex));
  const saveIndex = enableFlow.indexOf("await updateWorkspace(nextProfile)");
  const secretIndex = enableFlow.indexOf("await saveProviderSecret(workspaceId, targetService, input)");
  const verifyIndex = enableFlow.indexOf("await testTunnel(workspaceId, targetService)");
  const desktopStartIndex = enableFlow.indexOf("await startRuntime(workspaceId)");
  const nodeStartIndex = enableFlow.indexOf("await startTunnel(workspaceId, targetService)");
  const refreshIndex = enableFlow.indexOf("await refreshWorkspaceStore(workspaceId)");
  const completeIndex = enableFlow.indexOf('step = "complete"');

  assert.ok(saveIndex >= 0 && saveIndex < secretIndex);
  assert.ok(secretIndex < desktopStartIndex);
  assert.ok(secretIndex < nodeStartIndex);
  assert.ok(desktopStartIndex < verifyIndex);
  assert.ok(nodeStartIndex < verifyIndex);
  assert.match(enableFlow, /!tunnel\.keptRunning/);
  assert.ok(verifyIndex < refreshIndex);
  assert.ok(refreshIndex < completeIndex);
});

test("Rust MCP policy exposes independent security protections", async () => {
  const [form, types, route] = await Promise.all([
    readFile(path.join(root, "src", "lib", "components", "RuntimePolicyForm.svelte"), "utf8"),
    readFile(path.join(root, "src", "lib", "types.ts"), "utf8"),
    readFile(path.join(root, "src", "routes", "workspace", "[id]", "+page.svelte"), "utf8"),
  ]);

  for (const marker of [
    "interface SecurityPolicy",
    "require_dangerous_confirmation",
    "require_shell_confirmation",
    "block_network_commands",
    "redact_history",
  ]) assert.match(types, new RegExp(marker));
  for (const marker of [
    "const SECURITY_OPTIONS",
    "type=\"checkbox\"",
    "securityPolicy: { ...draftSecurityPolicy }",
    "Checked protections are enforced independently",
  ]) assert.ok(form.includes(marker), `missing Rust security UI marker: ${marker}`);
  assert.match(route, /security_policy: draft\.securityPolicy/);
  assert.match(route, /compatibilityPermissionMode\(draft\.securityPolicy\)/);
});
