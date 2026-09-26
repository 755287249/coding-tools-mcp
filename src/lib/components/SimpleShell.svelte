<script lang="ts">
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import LanguageSelect from "$lib/components/LanguageSelect.svelte";
  import ThemeToggle from "$lib/components/ThemeToggle.svelte";
  import { APP_VERSION } from "$lib/app-version";
  import { t } from "$lib/i18n";
  import type { Snippet } from "svelte";

  interface Props {
    children: Snippet;
    workspaceList: Snippet;
    onAddWorkspace?: () => void | Promise<void>;
  }

  let { children, workspaceList, onAddWorkspace }: Props = $props();
</script>

<div class="sx-layout">
  <aside class="sx-side">
    <p class="sx-side-label">{$t("Workspace")}</p>
    <div class="sx-side-list">
      {@render workspaceList()}
    </div>
    {#if onAddWorkspace}
      <button type="button" class="sx-add" onclick={onAddWorkspace}>
        <FolderPlus size={15} aria-hidden="true" />
        <span>{$t("New workspace (separate MCP)")}</span>
      </button>
    {/if}
    <div class="sx-side-foot">
      <LanguageSelect />
      <ThemeToggle />
      <span class="sx-version">v{APP_VERSION}</span>
    </div>
  </aside>
  <main class="sx-main">
    {@render children()}
  </main>
</div>
