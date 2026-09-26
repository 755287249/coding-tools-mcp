<script lang="ts">
  import { t, type MessageKey } from "$lib/i18n";
  import type { SecurityPolicy } from "$lib/types";

  type SecurityPolicyKey = keyof SecurityPolicy;

  export interface RuntimePolicyDraft {
    transportMode: string;
    securityPolicy: SecurityPolicy;
    allowedCommands: string;
    workspaceLocalEntries: boolean;
    workspaceScriptExtensions: string;
    blockingAdmissionLimit: number;
    processAdmissionLimit: number;
    globalBlockingAdmissionLimit: number;
    globalProcessAdmissionLimit: number;
    activeSessionLimit: number;
  }

  interface Props {
    transportMode: string;
    securityPolicy: SecurityPolicy;
    allowedCommands: string;
    workspaceLocalEntries: boolean;
    workspaceScriptExtensions: string;
    blockingAdmissionLimit: number;
    processAdmissionLimit: number;
    globalBlockingAdmissionLimit: number;
    globalProcessAdmissionLimit: number;
    activeSessionLimit: number;
    onSave: (draft: RuntimePolicyDraft) => void | Promise<void>;
  }

  const TRANSPORT_MODE_OPTIONS = [
    { value: "streamable-http", label: "Standard Streamable HTTP (recommended)" },
    { value: "legacy-json", label: "Legacy JSON compatibility mode" },
  ] as const;

  const SECURITY_OPTIONS: ReadonlyArray<{
    key: SecurityPolicyKey;
    title: string;
    description: string;
    highRisk?: boolean;
  }> = [
    { key: "restrict_tool_catalog", title: "Restrict tool catalog", description: "Only expose the core tool catalog; disabling this exposes the advanced tool set." },
    { key: "enforce_command_allowlist", title: "Command allowlist", description: "Only allow default or manually configured executables." },
    { key: "require_dangerous_confirmation", title: "Dangerous command confirmation", description: "Require confirm=true for destructive commands such as rm -rf and git reset --hard." },
    { key: "require_shell_confirmation", title: "Shell execution confirmation", description: "Require confirm=true for PowerShell, cmd, sh, pipes, redirects, and command chains." },
    { key: "block_network_commands", title: "Block network commands", description: "Block curl, wget, ssh, HTTP clients, and other network-looking commands." },
    { key: "enforce_workspace_boundary", title: "Workspace boundary", description: "Keep execution paths, working directories, and executables inside the configured Workspace.", highRisk: true },
    { key: "protect_repository_metadata", title: "Protect .git / .github", description: "Block direct overwrite, move, or recursive deletion of Git metadata and workflows.", highRisk: true },
    { key: "block_symlink_escape", title: "Block symlink escape", description: "Reject reads and writes that escape the Workspace through symbolic links.", highRisk: true },
    { key: "protect_environment_variables", title: "Protect process environment", description: "Prevent overriding PATH, COMSPEC, LD_PRELOAD, DYLD, and other loader variables.", highRisk: true },
    { key: "enforce_harness_baseline", title: "Harness baseline checks", description: "Scan the HEAD and file baseline before tracked writes or executions." },
    { key: "require_write_confirmation", title: "Write and delete confirmation", description: "Require confirmation for overwrites, critical deletes, Git restore, and project-wide formatting." },
    { key: "verify_write_conflicts", title: "Write conflict checks", description: "Verify hashes, versions, and file contents before applying changes.", highRisk: true },
    { key: "enforce_resource_limits", title: "General resource limits", description: "Apply command, output, payload, file-count, and concurrency limits." },
    { key: "redact_sensitive_output", title: "Sensitive value redaction", description: "Redact tokens, passwords, private keys, Authorization headers, and likely credentials.", highRisk: true },
    { key: "withhold_sensitive_source_output", title: "Withhold sensitive source output", description: "Hide complete content and stdout when reading .env, SSH keys, credentials, and similar sources.", highRisk: true },
    { key: "redact_telemetry", title: "Telemetry redaction", description: "Redact arguments, results, and error content before persisting tool usage records.", highRisk: true },
    { key: "redact_history", title: "History redaction", description: "Remove sensitive values before writing development checkpoints and history Markdown.", highRisk: true },
  ];

  let {
    transportMode,
    securityPolicy,
    allowedCommands,
    workspaceLocalEntries,
    workspaceScriptExtensions,
    blockingAdmissionLimit,
    processAdmissionLimit,
    globalBlockingAdmissionLimit,
    globalProcessAdmissionLimit,
    activeSessionLimit,
    onSave,
  }: Props = $props();

  let draftTransportMode = $state("streamable-http");
  let draftSecurityPolicy = $state<SecurityPolicy>({
    restrict_tool_catalog: true,
    enforce_command_allowlist: true,
    require_dangerous_confirmation: true,
    require_shell_confirmation: true,
    block_network_commands: false,
    enforce_workspace_boundary: true,
    protect_repository_metadata: true,
    block_symlink_escape: true,
    protect_environment_variables: true,
    enforce_harness_baseline: true,
    require_write_confirmation: true,
    verify_write_conflicts: true,
    enforce_resource_limits: true,
    redact_sensitive_output: true,
    withhold_sensitive_source_output: true,
    redact_telemetry: true,
    redact_history: true,
  });
  let draftCommands = $state("");
  let draftLocalEntries = $state(true);
  let draftExtensions = $state(".exe,.bat,.cmd,.ps1");
  let draftBlockingLimit = $state(128);
  let draftProcessLimit = $state(64);
  let draftGlobalBlockingLimit = $state(1024);
  let draftGlobalProcessLimit = $state(512);
  let draftActiveSessions = $state(512);
  let saving = $state(false);

  const dirty = $derived(
    draftTransportMode !== transportMode ||
      JSON.stringify(draftSecurityPolicy) !== JSON.stringify(securityPolicy) ||
      draftCommands !== allowedCommands ||
      draftLocalEntries !== workspaceLocalEntries ||
      draftExtensions !== workspaceScriptExtensions ||
      draftBlockingLimit !== blockingAdmissionLimit ||
      draftProcessLimit !== processAdmissionLimit ||
      draftGlobalBlockingLimit !== globalBlockingAdmissionLimit ||
      draftGlobalProcessLimit !== globalProcessAdmissionLimit ||
      draftActiveSessions !== activeSessionLimit,
  );

  $effect(() => {
    draftTransportMode = transportMode;
    draftSecurityPolicy = { ...securityPolicy };
    draftCommands = allowedCommands;
    draftLocalEntries = workspaceLocalEntries;
    draftExtensions = workspaceScriptExtensions;
    draftBlockingLimit = blockingAdmissionLimit;
    draftProcessLimit = processAdmissionLimit;
    draftGlobalBlockingLimit = globalBlockingAdmissionLimit;
    draftGlobalProcessLimit = globalProcessAdmissionLimit;
    draftActiveSessions = activeSessionLimit;
  });

  function updateSecurityOption(key: SecurityPolicyKey, checked: boolean) {
    draftSecurityPolicy = { ...draftSecurityPolicy, [key]: checked };
  }

  async function save() {
    if (saving || !dirty) return;
    saving = true;
    try {
      await onSave({
        transportMode: draftTransportMode,
        securityPolicy: { ...draftSecurityPolicy },
        allowedCommands: draftCommands.trim(),
        workspaceLocalEntries: draftLocalEntries,
        workspaceScriptExtensions: draftExtensions.trim(),
        blockingAdmissionLimit: Math.min(65535, Math.max(1, draftBlockingLimit)),
        processAdmissionLimit: Math.min(65535, Math.max(1, draftProcessLimit)),
        globalBlockingAdmissionLimit: Math.min(65535, Math.max(1, draftGlobalBlockingLimit)),
        globalProcessAdmissionLimit: Math.min(65535, Math.max(1, draftGlobalProcessLimit)),
        activeSessionLimit: Math.min(65535, Math.max(1, draftActiveSessions)),
      });
    } finally {
      saving = false;
    }
  }
</script>

<form
  class="grid gap-3"
  onsubmit={(event) => {
    event.preventDefault();
    void save();
  }}
>
  <label class="grid gap-1">
    <span class="text-xs text-[var(--color-text-muted)]">{$t("HTTP transport mode")}</span>
    <select
      class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
      bind:value={draftTransportMode}
    >
      {#each TRANSPORT_MODE_OPTIONS as option}
        <option value={option.value}>{$t(option.label as MessageKey)}</option>
      {/each}
    </select>
    <span class="text-xs text-[var(--color-text-muted)]">
      {$t("Standard mode supports ChatGPT and current MCP clients. Use legacy mode only to preserve older connection behavior.")}
    </span>
  </label>
  <div class="grid grid-cols-1 gap-3 sm:grid-cols-3">
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("Workspace blocking concurrency")}</span>
      <input type="number" min="1" max="65535" class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm" bind:value={draftBlockingLimit} />
    </label>
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("Workspace process concurrency")}</span>
      <input type="number" min="1" max="65535" class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm" bind:value={draftProcessLimit} />
    </label>
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("Active command session limit")}</span>
      <input type="number" min="1" max="65535" class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm" bind:value={draftActiveSessions} />
    </label>
  </div>
  <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("Global blocking concurrency")}</span>
      <input type="number" min="1" max="65535" class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm" bind:value={draftGlobalBlockingLimit} />
    </label>
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("Global process concurrency")}</span>
      <input type="number" min="1" max="65535" class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm" bind:value={draftGlobalProcessLimit} />
    </label>
  </div>
  <span class="text-xs text-[var(--color-text-muted)]">{$t("Defaults are intentionally generous; 65535 is only the configuration format boundary. Workspace capacity is acquired before global capacity to prevent one workspace from occupying the global queue.")}</span>
  <section class="grid gap-3 rounded-lg border border-[var(--color-border)] p-3">
    <div>
      <p class="text-sm font-semibold text-[var(--color-text)]">{$t("Custom security policy")}</p>
      <p class="mt-1 text-xs leading-5 text-[var(--color-text-muted)]">{$t("Checked protections are enforced independently. Disabling a high-risk protection gives tools broader host access or stores less-redacted data.")}</p>
    </div>
    <div class="grid grid-cols-1 gap-3 md:grid-cols-2">
      {#each SECURITY_OPTIONS as option}
        <label class={`grid cursor-pointer gap-2 rounded-md border p-3 ${option.highRisk && !draftSecurityPolicy[option.key] ? "border-[var(--color-danger)]" : "border-[var(--color-border)]"}`}>
          <span class="flex items-start gap-2">
            <input
              class="mt-0.5 h-4 w-4"
              type="checkbox"
              checked={draftSecurityPolicy[option.key]}
              onchange={(event) => updateSecurityOption(option.key, (event.currentTarget as HTMLInputElement).checked)}
            />
            <span class="text-sm font-medium">{$t(option.title as MessageKey)}</span>
          </span>
          <span class="pl-6 text-xs leading-5 text-[var(--color-text-muted)]">{$t(option.description as MessageKey)}</span>
          {#if option.highRisk && !draftSecurityPolicy[option.key]}
            <span class="pl-6 text-xs font-medium text-[var(--color-danger)]">{$t("High risk: disabled")}</span>
          {/if}
        </label>
      {/each}
    </div>
  </section>
  <label class="grid gap-1">
    <span class="text-xs text-[var(--color-text-muted)]">{$t("System commands (comma separated)")}</span>
    <input type="text" class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-sm" placeholder="python,git,curl,powershell,..." bind:value={draftCommands} />
  </label>
  <label class="flex items-center gap-2 text-sm">
    <input type="checkbox" bind:checked={draftLocalEntries} />
    <span>{$t("Allow local workspace entry points")}</span>
  </label>
  <label class="grid gap-1">
    <span class="text-xs text-[var(--color-text-muted)]">{$t("Local script extensions (comma separated)")}</span>
    <input type="text" class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-sm" placeholder=".exe,.bat,.cmd,.ps1" bind:value={draftExtensions} disabled={!draftLocalEntries} />
  </label>
  <p class="text-xs text-[var(--color-text-muted)]">
    {$t("Local workspace entry points resolve from the current working directory. System commands and script types are configurable per project. The execution boundary remains policy_only.")}
  </p>
  <div class="flex justify-end pt-1">
    <button
      type="submit"
      class="rounded-md bg-[var(--color-accent)] px-3 py-1.5 text-sm font-medium text-[var(--primary-contrast)] disabled:opacity-50"
      disabled={saving || !dirty}
    >
      {saving ? $t("Saving…") : $t("Save policy")}
    </button>
  </div>
</form>
