import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const scriptUrl = new URL("../scripts/switch-node-agent-to-latest.ps1", import.meta.url);

test("Node Agent handoff is detached and validates the new portable before stopping the old service", async () => {
  const script = await readFile(scriptUrl, "utf8");

  assert.match(script, /\[switch\]\$Worker/);
  assert.match(script, /\[switch\]\$DryRun/);
  assert.match(script, /Assert-CriticalPortableFiles -Root/);
  assert.match(script, /SHA256SUMS\.txt/);
  assert.match(script, /Get-WorkspaceEndpoints -Directory/);
  assert.match(script, /Invoke-CimMethod -ClassName Win32_Process -MethodName Create/);
  assert.match(script, /Start-DetachedProcess -CommandLine \$workerCommandLine/);
  assert.match(script, /Start-DetachedProcess -CommandLine \$commandLine/);
  assert.match(script, /The detached worker will stop the old Agent only after this foreground invocation has returned/);
  assert.match(script, /Start-Sleep -Seconds \(\[Math\]::Max\(0, \$DelaySeconds\)\)/);
  assert.match(script, /Stop-ExistingNodeAgents/);
  assert.match(script, /Start-PortableAgent/);
  assert.match(script, /Assert-PortableSupervisorRunning/);
  assert.match(script, /Wait-NewAgentHealthy/);
  assert.match(script, /health\.version -eq \$ExpectedVersion/);
  assert.match(script, /health\.buildGitSha -eq \$ExpectedGitCommit/);
  assert.match(script, /Get-CimInstance Win32_Process -Filter/);
  assert.match(script, /Portable supervisor exited before handoff completed/);
  assert.match(script, /Start-Process -FilePath 'taskkill\.exe'/);
  assert.match(script, /\$taskkill\.ExitCode -ne 0/);
  assert.match(script, /dist-node-portable/);
  assert.match(script, /Join-Path \$script:ResolvedDataDir 'handoff\.json'/);
});
