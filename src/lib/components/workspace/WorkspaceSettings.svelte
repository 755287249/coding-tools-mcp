<script lang="ts">
  import ChatGptSessionPrompt from "$lib/components/ChatGptSessionPrompt.svelte";
  import SandboxSettings from "$lib/components/workspace/SandboxSettings.svelte";
  import WorkspaceFolderManager from "$lib/components/WorkspaceFolderManager.svelte";
  import WorkspaceMetaForm from "$lib/components/WorkspaceMetaForm.svelte";
  import { t } from "$lib/i18n";
  import { sandboxConfig, type SandboxConfig, type WorkspaceFolder, type WorkspaceProfile } from "$lib/types";

  interface Props {
    profile: WorkspaceProfile;
    onSaveName: (name: string) => void | Promise<void>;
    onProfileChanged: (profile: WorkspaceProfile) => void | Promise<void>;
    onFoldersChanged: () => void | Promise<void>;
    onSaveSandbox: (config: SandboxConfig) => void | Promise<void>;
    sandboxLocked: boolean;
  }

  let {
    profile,
    onSaveName,
    onProfileChanged,
    onFoldersChanged,
    onSaveSandbox,
    sandboxLocked,
  }: Props = $props();

  function folderLooksLikeWsl(folder: WorkspaceFolder): boolean {
    if (folder.execution?.kind === "wsl") return true;
    return /^\\\\wsl(?:\.localhost|\$)\\/i.test(folder.path);
  }

  const hasWslFolder = $derived((profile.folders ?? []).some(folderLooksLikeWsl));
</script>

<div class="grid gap-5">
  <section class="tx-card p-5">
    <p class="tx-section-label">{$t("Basic information")}</p>
    <div class="mt-3">
      <WorkspaceMetaForm name={profile.name} onSave={onSaveName} />
    </div>
  </section>

  <section class="tx-card p-5">
    <p class="tx-section-label">{$t("Project folders")}</p>
    <div class="mt-3">
      <p class="break-all">{profile.path}</p><p class="mt-2 text-xs text-[var(--color-text-muted)]">{$t('connection.fixedDirectoryHelp')}</p>
    </div>
  </section>

  <section class="tx-card p-5">
    <p class="tx-section-label">{$t("Sandbox")}</p>
    <div class="mt-3">
      <SandboxSettings
        config={sandboxConfig(profile.runtime)}
        locked={sandboxLocked}
        projectIsolation
        {hasWslFolder}
        onSave={onSaveSandbox}
      />
    </div>
  </section>

  <section class="tx-card p-5">
    <p class="tx-section-label">{$t("ChatGPT session")}</p>
    <div class="mt-3">
      <ChatGptSessionPrompt />
    </div>
  </section>
</div>
