<script lang="ts">
  import { onMount } from "svelte";
  import Copy from "@lucide/svelte/icons/copy";
  import Minus from "@lucide/svelte/icons/minus";
  import Square from "@lucide/svelte/icons/square";
  import SquareTerminal from "@lucide/svelte/icons/square-terminal";
  import X from "@lucide/svelte/icons/x";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { t } from "$lib/i18n";

  let maximized = $state(false);

  async function syncMaximized() {
    try {
      maximized = await getCurrentWindow().isMaximized();
    } catch {
      maximized = false;
    }
  }

  onMount(() => {
    void syncMaximized();
    let unlisten: (() => void) | undefined;
    void getCurrentWindow()
      .onResized(() => void syncMaximized())
      .then((fn) => (unlisten = fn))
      .catch(() => undefined);
    return () => unlisten?.();
  });

  const minimize = () => void getCurrentWindow().minimize();
  const toggleMaximize = () => void getCurrentWindow().toggleMaximize();
  // Close hides to the tray (handled by the Rust CloseRequested hook).
  const close = () => void getCurrentWindow().close();
</script>

<header class="wt-bar" data-tauri-drag-region>
  <div class="wt-brand" data-tauri-drag-region>
    <SquareTerminal size={14} strokeWidth={2.2} aria-hidden="true" />
    <span data-tauri-drag-region>Coding Tools MCP</span>
  </div>
  <div class="wt-controls">
    <button type="button" class="wt-btn" onclick={minimize} title={$t("Minimize")} aria-label={$t("Minimize")}>
      <Minus size={14} />
    </button>
    <button
      type="button"
      class="wt-btn"
      onclick={toggleMaximize}
      title={maximized ? $t("Restore") : $t("Maximize")}
      aria-label={maximized ? $t("Restore") : $t("Maximize")}
    >
      {#if maximized}<Copy size={12} />{:else}<Square size={11} />{/if}
    </button>
    <button type="button" class="wt-btn wt-btn--close" onclick={close} title={$t("Close")} aria-label={$t("Close")}>
      <X size={15} />
    </button>
  </div>
</header>
