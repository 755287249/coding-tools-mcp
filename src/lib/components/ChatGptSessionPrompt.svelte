<script lang="ts">
  import {copyText} from "$lib/browser-tools";
  import Check from "@lucide/svelte/icons/check";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import Copy from "@lucide/svelte/icons/copy";
  import History from "@lucide/svelte/icons/history";
  import { onDestroy } from "svelte";
  import { showToast } from "$lib/stores/toast";
  import { t } from "$lib/i18n";

  const sessionPrompt = $derived($t("Session bootstrap prompt"));

  let copying = $state(false);
  let copied = $state(false);
  let expanded = $state(false);
  let errorMessage = $state("");
  let resetTimer: ReturnType<typeof setTimeout> | undefined;

  async function copyPrompt() {
    if (copying) return;
    copying = true;
    copied = false;
    errorMessage = "";
    if (resetTimer) clearTimeout(resetTimer);
    try {
      await copyText(sessionPrompt);
      copied = true;
      showToast($t("The new-session prompt was copied. Paste it into ChatGPT."), {
        title: $t("Copy successful"),
        kind: "success",
        duration: 2500,
      });
      resetTimer = setTimeout(() => {
        copied = false;
      }, 2000);
    } catch (error) {
      errorMessage = $t("Copy failed. Select the prompt and copy it manually.");
      showToast(String(error), {
        title: $t("Could not copy prompt"),
        kind: "error",
        duration: 6000,
      });
    } finally {
      copying = false;
    }
  }

  onDestroy(() => {
    if (resetTimer) clearTimeout(resetTimer);
  });
</script>

<section
  class="rounded-[12px] border border-[var(--color-border)] bg-[var(--card-bg)] px-3 py-2.5 sm:px-4"
  aria-labelledby="chatgpt-session-prompt-title"
>
  <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between sm:gap-4">
    <div class="flex min-w-0 items-center gap-3">
      <span
        class="flex size-9 shrink-0 items-center justify-center rounded-[10px] bg-[var(--primary-soft)] text-[var(--primary)]"
        aria-hidden="true"
      >
        <History size={16} />
      </span>
      <div class="min-w-0">
        <h3 id="chatgpt-session-prompt-title" class="text-sm font-semibold text-[var(--color-text)]">
          {$t("ChatGPT new-session prompt")}
        </h3>
        <p class="mt-0.5 text-xs leading-5 text-[var(--color-text-muted)]">
          {$t("Choose the target folder first, then initialize or resume its independent history.")}
        </p>
      </div>
    </div>

    <div class="flex shrink-0 flex-wrap items-center gap-2 sm:flex-nowrap">
      <button
        type="button"
        class="tx-btn-primary min-h-11 shrink-0 px-3 py-2 text-xs active:scale-[0.98] disabled:cursor-not-allowed disabled:opacity-50"
        disabled={copying}
        aria-label={$t("Copy ChatGPT new-session prompt")}
        onclick={() => void copyPrompt()}
      >
        {#if copied}
          <Check size={14} aria-hidden="true" />
          <span>{$t("Copied")}</span>
        {:else}
          <Copy size={14} aria-hidden="true" />
          <span>{copying ? $t("Copying…") : $t("Copy full prompt")}</span>
        {/if}
      </button>

      <button
        type="button"
        class="tx-btn-ghost min-h-11 shrink-0 gap-1.5 px-3 py-2 text-xs active:scale-[0.98]"
        aria-expanded={expanded}
        aria-controls="chatgpt-session-prompt-content"
        onclick={() => (expanded = !expanded)}
      >
        <span>{expanded ? $t("Hide prompt") : $t("View full prompt")}</span>
        <ChevronDown
          size={14}
          class={`transition-transform duration-200 motion-reduce:transition-none ${expanded ? "rotate-180" : ""}`}
          aria-hidden="true"
        />
      </button>
    </div>
  </div>

  {#if expanded}
    <div id="chatgpt-session-prompt-content" class="mt-3 border-t border-[var(--color-border)] pt-3">
      <pre
        class="tx-mono whitespace-pre-wrap break-words rounded-[10px] bg-[var(--surface-hover)] p-3 leading-5 text-[var(--color-text-secondary)]"
      >{sessionPrompt}</pre>
      <p class="mt-2 text-[11px] leading-5 text-[var(--color-text-muted)]">
        {$t("Paste it into a new ChatGPT conversation that uses this workspace's MCP connector.")}
      </p>
    </div>
  {/if}

  {#if errorMessage}
    <p class="mt-2 text-xs text-[var(--danger)]" role="alert">{errorMessage}</p>
  {/if}
  <span class="sr-only" aria-live="polite">{copied ? $t("Prompt copied") : ""}</span>
</section>
