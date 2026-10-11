<script lang="ts">
  import { confirmFolderRemoval } from "$lib/workspace/confirm-folder-removal";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import { pickDirectory } from "$lib/api/native";
  import { getBackend } from "$lib/backend";
  import DirectoryPicker from "$lib/components/DirectoryPicker.svelte";
  import WslFolderDialog from "$lib/components/WslFolderDialog.svelte";
  import {
    addWorkspaceFolder,
    addWslWorkspaceFolder,
    listWslDistributions,
    openWorkspaceDirectory,
    removeWorkspaceFolder,
  } from "$lib/api/workspaces";
  import { showToast } from "$lib/stores/toast";
  import { workspaceFolders, type WorkspaceProfile } from "$lib/types";
  import { t } from "$lib/i18n";

  interface Props {
    profile: WorkspaceProfile;
    onChanged: (profile: WorkspaceProfile) => void | Promise<void>;
    onFoldersChanged?: (profile: WorkspaceProfile) => void | Promise<void>;
  }

  let { profile, onChanged, onFoldersChanged }: Props = $props();
  const capabilities = getBackend().capabilities;
  let busyAction = $state("");
  let wslDialogOpen = $state(false);
  let wslDistributions = $state<string[]>([]);
  let wslError = $state("");
  let directoryPickerOpen = $state(false);

  const folders = $derived(workspaceFolders(profile));

  function normalizePath(value: string): string {
    const trimmed = value.trim();
    if (/^[A-Za-z]:[\\/]$/.test(trimmed) || trimmed === "/") return trimmed;
    return trimmed.replace(/[\\/]+$/, "");
  }

  async function addFolderFromPath(selected: string) {
    const path = normalizePath(selected);
    if (!path) return;
    busyAction = "add";
    try {
      const updated = await addWorkspaceFolder(profile.id, path);
      await onChanged(updated);
      await onFoldersChanged?.(updated);
      showToast($t("Folder added to the workspace. MCP stays connected; running Actions will prompt for a restart to refresh the manifest."), { kind: "success" });
    } catch (error) {
      showToast(String(error), { kind: "error", title: $t("Could not add folder") });
    } finally {
      busyAction = "";
    }
  }

  async function chooseFolder() {
    if (busyAction) return;
    if (!capabilities.nativeDirectoryPicker) {
      directoryPickerOpen = true;
      return;
    }
    busyAction = "add";
    try {
      const selected = await pickDirectory({
        multiple: false,
        defaultPath: profile.path || undefined,
      });
      if (!selected || Array.isArray(selected)) return;
      await addFolderFromPath(selected);
    } catch (error) {
      showToast(String(error), { kind: "error", title: $t("Could not add folder") });
      busyAction = "";
    }
  }

  async function chooseWslFolder() {
    if (busyAction) return;
    busyAction = "wsl-open";
    try {
      wslError = "";
      wslDistributions = await listWslDistributions();
      if (wslDistributions.length === 0) {
        throw new Error($t("No WSL distributions are installed."));
      }
      wslDialogOpen = true;
    } catch (error) {
      showToast(String(error), {
        kind: "error",
        title: $t("Failed to open WSL"),
        duration: 8000,
      });
    } finally {
      busyAction = "";
    }
  }

  async function addWslFolder(distro: string, linuxPath: string, name?: string) {
    if (busyAction) return;
    busyAction = "wsl-add";
    wslError = "";
    try {
      const updated = await addWslWorkspaceFolder(profile.id, distro, linuxPath, name);
      await onChanged(updated);
      await onFoldersChanged?.(updated);
      wslDialogOpen = false;
      showToast($t("Folder added to the workspace. MCP stays connected; running Actions will prompt for a restart to refresh the manifest."), { kind: "success" });
    } catch (error) {
      wslError = String(error);
    } finally {
      busyAction = "";
    }
  }

  async function openFolder(path: string, folderId: string) {
    if (busyAction) return;
    busyAction = `open:${folderId}`;
    try {
      await openWorkspaceDirectory(path);
    } catch (error) {
      showToast(String(error), { kind: "error", title: $t("Could not open directory") });
    } finally {
      busyAction = "";
    }
  }

  async function removeFolder(folderId: string) {
    if (busyAction || folders.length <= 1) return;
    const folder=folders.find(item=>item.id===folderId);
    if(!folder)return;
    busyAction = `remove:${folderId}`;
    try {
      if(!await confirmFolderRemoval(profile.id,folder))return;
      const updated = await removeWorkspaceFolder(profile.id, folderId);
      await onChanged(updated);
      await onFoldersChanged?.(updated);
      showToast($t("Folder removed from the workspace. Restart running Actions to refresh the manifest."), { kind: "success" });
    } catch (error) {
      showToast(String(error), { kind: "error", title: $t("Could not remove folder") });
    } finally {
      busyAction = "";
    }
  }
</script>

<section class="tx-card p-4">
  <div class="flex flex-wrap items-start justify-between gap-3">
    <div>
      <h3 class="text-sm font-semibold text-[var(--color-text-primary)]">{$t("Folder list")}</h3>
      <p class="mt-1 max-w-3xl text-xs leading-5 text-[var(--color-text-muted)]">
        {$t("All folders share the same MCP service, authentication, port, and tunnel.")}
        {$t("Each folder keeps independent tool context and conversation history under")}
        <span class="tx-mono">docs/history-session</span>.
        {$t("MCP and Actions require an explicit folder selection; no default folder is used.")}
      </p>
    </div>
    <div class="flex shrink-0 flex-wrap items-center gap-2">
      {#if capabilities.wslFolders}
        <button
          type="button"
          class="tx-btn-ghost"
          disabled={Boolean(busyAction)}
          onclick={() => void chooseWslFolder()}
        >
          <FolderPlus size={15} class="inline-block" />
          <span class="ml-1">{$t("Add WSL folder")}</span>
        </button>
      {/if}
      <button
        type="button"
        class="tx-btn-primary"
        disabled={Boolean(busyAction)}
        onclick={() => void chooseFolder()}
      >
        <FolderPlus size={15} class="inline-block" />
        <span class="ml-1">{busyAction === "add" ? $t("Selecting…") : $t("Add folder")}</span>
      </button>
    </div>
  </div>

  <div class="mt-4 grid gap-2">
    {#each folders as folder (folder.id)}
      <article class="rounded-[12px] border border-[var(--color-border)] px-3 py-3">
        <div class="flex min-w-0 flex-wrap items-center justify-between gap-3">
          <div class="min-w-0 flex-1">
            <div class="flex min-w-0 items-center gap-2">
              <span class="block truncate text-sm font-medium text-[var(--color-text-primary)]">
                {folder.name}
              </span>
              <span class="shrink-0 rounded-full border border-[var(--color-border)] px-2 py-0.5 text-[10px] text-[var(--color-text-muted)]">
                {#if folder.execution?.kind === "wsl"}
                  {$t("WSL")} · {folder.execution.distro}
                {:else}
                  {$t("Local")}
                {/if}
              </span>
            </div>
            <p class="tx-mono mt-1 truncate text-xs text-[var(--color-text-muted)]" title={folder.path}>
              {folder.path}
            </p>
          </div>

          <div class="flex shrink-0 flex-wrap items-center gap-1.5">
            {#if capabilities.openNativePath}
              <button
                type="button"
                class="tx-btn-ghost px-2.5 py-1.5 text-xs"
                disabled={Boolean(busyAction)}
                onclick={() => void openFolder(folder.path, folder.id)}
              >
                <FolderOpen size={14} class="inline-block" />
                <span class="ml-1">{$t("Open")}</span>
              </button>
            {/if}
            <button
              type="button"
              class="tx-btn-ghost px-2.5 py-1.5 text-xs text-[var(--danger)]"
              disabled={Boolean(busyAction) || folders.length <= 1}
              title={folders.length <= 1 ? $t("A workspace must keep at least one folder") : $t("Remove from workspace")}
              onclick={() => void removeFolder(folder.id)}
            >
              <Trash2 size={14} class="inline-block" />
              <span class="ml-1">{$t("Remove")}</span>
            </button>
          </div>
        </div>
      </article>
    {/each}
  </div>
</section>

<WslFolderDialog
  open={wslDialogOpen}
  distributions={wslDistributions}
  busy={busyAction === "wsl-add"}
  error={wslError}
  onClose={() => (wslDialogOpen = false)}
  onSubmit={addWslFolder}
/>

<DirectoryPicker
  open={directoryPickerOpen}
  workspaceId={profile.id}
  initialPath={profile.path}
  onCancel={() => (directoryPickerOpen = false)}
  onSelect={async (path) => {
    directoryPickerOpen = false;
    await addFolderFromPath(path);
  }}
/>
