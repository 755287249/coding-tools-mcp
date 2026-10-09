import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

async function source(path) {
  return readFile(new URL(`../${path}`, import.meta.url), "utf8");
}

test("sandbox settings stay backend-neutral and fail closed", async () => {
  const [model, sandbox, dockerSbx, wslc, exec, dispatch, types, settings] = await Promise.all([
    source("src-tauri/src/workspace/model.rs"),
    source("src-tauri/src/tools/sandbox.rs"),
    source("src-tauri/src/tools/sandbox/docker_sbx.rs"),
    source("src-tauri/src/tools/sandbox/wslc.rs"),
    source("src-tauri/src/tools/exec.rs"),
    source("src-tauri/src/tools/dispatch.rs"),
    source("src/lib/types.ts"),
    source("src/lib/components/workspace/SandboxSettings.svelte"),
  ]);

  assert.match(model, /pub struct SandboxConfig\s*\{[\s\S]*pub enabled: bool,[\s\S]*pub backend: String,/);
  assert.match(model, /pub external_paths: Vec<SandboxPathGrant>/);
  assert.match(model, /pub enum SandboxPathAccess[\s\S]*ReadOnly[\s\S]*Modify/);
  assert.match(types, /external_paths: SandboxPathGrant\[\]/);
  assert.match(types, /options: Record<string, string>/);
  assert.match(settings, /Read-only external paths/);
  assert.match(settings, /Writable external paths/);

  assert.match(model, /pub sandbox: SandboxConfig/);
  assert.match(sandbox, /pub\(crate\) trait SandboxBackend/);
  assert.match(sandbox, /pub\(crate\) trait PreparedSandbox/);
  assert.match(sandbox, /fn prepare/);
  assert.match(sandbox, /fn normalize_logical_command/);
  assert.match(sandbox, /fn prepare_process/);
  assert.match(sandbox, /fn launch_prepared_process/);
  assert.match(sandbox, /fn environment_overrides/);
  assert.match(sandbox, /fn build_prepared_process_plan/);
  assert.match(sandbox, /pub\(crate\) fn prepare_enabled_backend/);
  assert.match(sandbox, /pub fn backend_descriptors\(\)/);
  assert.match(sandbox, /id: "docker_sbx"\.into\(\)/);
  assert.match(sandbox, /label: "Docker Sandboxes \(sbx\)"\.into\(\)/);
  assert.match(sandbox, /id: "wslc"\.into\(\)/);
  assert.match(sandbox, /label: "Microsoft WSL Containers \(wslc\)"\.into\(\)/);
  assert.match(sandbox, /APPCONTAINER_NETWORK_OPTION_ID/);
  assert.match(sandbox, /supports_wsl: false/);
  assert.match(sandbox, /supports_wsl: true/);
  assert.match(settings, /hasWslFolder/);
  assert.match(settings, /This backend cannot isolate WSL folders/);
  assert.match(sandbox, /static BACKENDS: \[&'static dyn SandboxBackend; 5\]/);
  assert.match(wslc, /DEFAULT_IMAGE/);
  assert.match(wslc, /"run"\.to_string\(\)/);
  assert.match(wslc, /"--rm"\.to_string\(\)/);
  assert.match(wslc, /\.args\(\["--session", session_name, "remove", "-f", name\]\)/);
  assert.match(wslc, /fallback_allowed/);
  assert.match(sandbox, /fn prepare_command/);
  assert.match(dockerSbx, /sbx exec|"exec"\.to_string\(\)/);
  assert.match(dockerSbx, /"create"\.to_string\(\)/);
  assert.match(dockerSbx, /SANDBOX_SBX_SETUP_REQUIRED/);
  assert.match(dockerSbx, /fallback_allowed/);
  assert.match(sandbox, /SANDBOX_BACKEND_UNKNOWN/);
  assert.match(sandbox, /SANDBOX_BACKEND_UNSUPPORTED/);
  assert.match(sandbox, /SANDBOX_BACKEND_NOT_READY/);
  assert.match(sandbox, /SANDBOX_DISABLED/);
  assert.match(sandbox, /"fallback_allowed": false/);
  assert.match(exec, /CommandExecutionBoundary::from_config\(&runtime_config\.sandbox, &ctx\.workspace\)/);
  assert.match(exec, /boundary\.prepare_backend\(&runtime_config\.sandbox, ctx\)/);
  assert.match(exec, /request\.legacy_native\s*&& request\.timeout_contract\.execution_mode != "job"\s*&& boundary\.allows_native_diagnostic\(\)/);
  assert.match(exec, /pub fn exec_health_check[\s\S]*CommandExecutionBoundary::from_config/);
  assert.match(exec, /pub fn exec_health_check[\s\S]*boundary\.prepare_backend/);
  assert.doesNotMatch(exec, /pub fn exec_health_check[\s\S]*CommandExecutionBackend::Native/);
  assert.match(dispatch, /"filesystem_sandbox": \{[\s\S]*"available": sandbox_available/);
  assert.match(dispatch, /"workspace_exec": \{[\s\S]*"sandbox_enforced": sandbox_enforced/);
  assert.match(dispatch, /"boundary": sandbox_boundary/);
  assert.match(types, /export interface SandboxBackendDescriptor/);
  assert.match(settings, /listSandboxBackends\(\)/);
  assert.match(settings, /#each backends as item/);
  assert.match(settings, /selected\.enforcementReady/);
  assert.match(settings, /selected\.options/);
  assert.doesNotMatch(settings, /const\s+backends\s*=\s*\[/);
  assert.match(model, /"appcontainer"\.to_string\(\)/);
  assert.doesNotMatch(settings, /async function saveEnabled/);
  assert.match(settings, /let enabled = \$state\(false\)/);
  assert.match(settings, /await onSave\(\{ enabled, backend, external_paths: pendingExternalPaths, options \}\)/);
  assert.match(settings, /bind:checked=\{enabled\}/);
  assert.match(settings, /enabled && !selected\.enforcementReady/);
  assert.doesNotMatch(settings, /Command sandboxing is always enabled/);
});

test("MCP and Actions share a sandbox selection that can switch live between commands", async () => {
  const [mcpLifecycle, supervisor, workspaceCommand, page, workspaceSettings, hub, appContainer] = await Promise.all([
    source("src-tauri/src/mcp/listener/lifecycle.rs"),
    source("src-tauri/src/runtime/supervisor.rs"),
    source("src-tauri/src/commands/workspace.rs"),
    source("src/routes/workspace/[id]/+page.svelte"),
    source("src/lib/components/workspace/WorkspaceSettings.svelte"),
    source("src-tauri/src/tools/hub.rs"),
    source("src-tauri/src/tools/sandbox/appcontainer/provider.rs"),
  ]);

  assert.match(mcpLifecycle, /runtime\.sandbox\.clone\(\)/);
  assert.match(supervisor, /profile\.runtime\.sandbox\.clone\(\)/);
  assert.doesNotMatch(workspaceCommand, /current\.runtime\.sandbox != profile\.runtime\.sandbox/);
  assert.doesNotMatch(workspaceCommand, /mcp_status\(&current\)\.state == "stopped"/);
  assert.doesNotMatch(workspaceCommand, /actions_status\(&current\)\.state == "stopped"/);
  assert.doesNotMatch(workspaceCommand, /if !profile\.runtime\.sandbox\.enabled/);
  assert.doesNotMatch(workspaceCommand, /命令沙盒不可停用/);
  assert.match(workspaceCommand, /preflight_live_hub\(&profile\)/);
  assert.match(workspaceCommand, /sync_live_hub_after_preflight\(&updated\)/);
  assert.match(hub, /prewarm_sandbox_backend/);
  assert.match(hub, /live_preflight_folders/);
  assert.match(hub, /self\.live_preflight_folders\(folders\)/);
  assert.match(hub, /Arc::strong_count\(context\) > 1/);
  assert.match(hub, /sync_preflighted/);
  assert.doesNotMatch(hub, /current_sandbox\.enabled != runtime\.sandbox\.enabled/);
  assert.match(appContainer, /APPCONTAINER_ACL_COMMAND_TIMEOUT/);
  assert.match(appContainer, /APPCONTAINER_ACL_CLEANUP_TIMEOUT/);
  assert.match(appContainer, /command_status_and_stderr_with_timeout/);
  assert.match(appContainer, /SANDBOX_ACL_GRANT_TIMEOUT/);
  assert.match(appContainer, /revoke_acl_grant_via_handle_direct/);
  assert.match(page, /const sandboxLocked = false/);
  assert.doesNotMatch(page, /const requiresRestart = current\.enabled !== config\.enabled/);
  assert.match(page, /sandbox: config/);
  assert.match(page, /Workspace settings could not be loaded/);
  assert.match(workspaceSettings, /<SandboxSettings/);
  assert.match(workspaceSettings, /locked=\{sandboxLocked\}/);
});

test("disabling the desktop sandbox saves first and automatically starts or restarts MCP", async () => {
  const page = await source("src/routes/workspace/[id]/+page.svelte");

  assert.match(page, /const disablingSandbox = current\.enabled && !config\.enabled;/);
  assert.match(page, /if \(!disablingSandbox\) return;/);
  assert.match(page, /const wasRunning = mcpStatus === "running";/);
  assert.match(page, /wasRunning \? "service\.mcp\.restart\.sandbox-disabled" : "service\.mcp\.start\.sandbox-disabled"/);
  assert.match(page, /\(\) => \(wasRunning \? restartRuntime\(targetWorkspaceId\) : startRuntime\(targetWorkspaceId\)\)/);

  const persisted = page.indexOf('persistProfile("save.workspace.sandbox", next, targetWorkspaceId)');
  const disableGate = page.indexOf('if (!disablingSandbox) return;', persisted);
  const launch = page.indexOf('restartRuntime(targetWorkspaceId) : startRuntime(targetWorkspaceId)', disableGate);
  assert.ok(persisted >= 0, "sandbox settings must be persisted before runtime handling");
  assert.ok(disableGate > persisted, "automatic runtime handling must only run after the sandbox save succeeds");
  assert.ok(launch > disableGate, "MCP start/restart must occur only for an actual enabled-to-disabled transition");
});

test("desktop sandbox restart never force-closes its own Windows listener", async () => {
  const [portRuntime, platform, windowsPlatform, windowsNet, rustHandoff] = await Promise.all([
    source("src-tauri/src/runtime/port.rs"),
    source("src-tauri/src/platform/mod.rs"),
    source("src-tauri/src/platform/windows/mod.rs"),
    source("src-tauri/src/platform/windows/net.rs"),
    source("scripts/switch-rust-desktop-to-latest.ps1"),
  ]);

  assert.doesNotMatch(portRuntime, /try_reclaim_own_port/);
  assert.doesNotMatch(platform, /reclaim_listening_port/);
  assert.doesNotMatch(windowsPlatform, /reclaim_listening_port/);
  assert.doesNotMatch(windowsNet, /SetTcpEntry|MIB_TCP_STATE_DELETE_TCB/);
  assert.match(rustHandoff, /Profile\.bind\.host/);
  assert.match(rustHandoff, /Profile\.bind\.port/);
  assert.match(rustHandoff, /Profile\.host\.desktop\.actions/);
  assert.match(rustHandoff, /rev-parse --path-format=absolute --git-common-dir/);
  assert.match(rustHandoff, /\[System\.IO\.File\]::ReadAllText\(\$Path, \[System\.Text\.Encoding\]::UTF8\) \| ConvertFrom-Json/);
  assert.match(rustHandoff, /\$canonicalDir = Join-Path \$stableWorkspace 'dist-portable\\ctmcp-win64'/);
  assert.match(rustHandoff, /Move-Item -LiteralPath \$canonicalDir -Destination \$canonicalPrevious/);
  assert.match(rustHandoff, /Move-Item -LiteralPath \$canonicalPrevious -Destination \$canonicalDir/);
  assert.doesNotMatch(rustHandoff, /Remove-Item -LiteralPath \$canonicalDir/);
  assert.doesNotMatch(rustHandoff, /\$canonicalDir = Join-Path \(Split-Path -Parent \$PackageZip\)/);
  assert.match(portRuntime, /if !port_free \{\s*handle\.abort\(\);/);
  assert.match(portRuntime, /if !port_free \{\s*let _ = wait_for_port_free_blocking\(port, Duration::from_secs\(2\)\);/);
});
test("Node management preserves manual sandbox enablement and hot-applies sandbox generations", async () => {
  const [config, store, form, tools] = await Promise.all([
    source("packages/node-agent/src/config.ts"),
    source("packages/node-agent/src/management/configStore.ts"),
    source("src/lib/components/workspace/SandboxSettings.svelte"),
    source("packages/node-agent/src/tools.ts"),
  ]);

  assert.match(config, /input\?\.enabled, false/);
  assert.match(config, /enabled: requestedEnabled/);
  assert.match(store, /runtime\.preflightSandbox\(desiredBeforeSave\.sandbox\)/);
  assert.match(store, /result\.applied\.push\('sandbox'\)/);
  assert.doesNotMatch(store, /sandbox enablement changes require an Agent restart/);
  assert.doesNotMatch(store, /sandbox settings are still in use by active or pending process work/);
  assert.match(tools, /config: structuredClone\(ctx\.config\)/);
  assert.match(form, /bind:checked=\{enabled\}/);
  assert.match(form, /Enable command sandbox/);
  assert.doesNotMatch(form, /命令沙盒固定啟用/);
});

test("legacy profiles keep sandbox disabled by default with an extensible backend id", async () => {
  const [model, store, types, settings] = await Promise.all([
    source("src-tauri/src/workspace/model.rs"),
    source("src-tauri/src/data/store.rs"),
    source("src/lib/types.ts"),
    source("src/lib/components/workspace/SandboxSettings.svelte"),
  ]);

  assert.match(model, /fn default_sandbox_enabled\(\) -> bool \{\s*false/);
  assert.match(model, /enabled: false/);
  assert.match(model, /pub fn normalize_sandbox/);
  assert.match(store, /profile\.normalize_sandbox\(\)/);
  assert.match(types, /enabled: false,[\s\S]*\.\.\.runtime\.sandbox/);
  assert.match(types, /backend: "appcontainer"/);
  assert.match(settings, /enabled, backend/);
  assert.match(settings, /type="checkbox"[\s\S]*bind:checked=\{enabled\}/);
  assert.doesNotMatch(settings, /Always on/);
});
