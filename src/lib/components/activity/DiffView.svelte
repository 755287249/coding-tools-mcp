<script lang="ts">
  import { toSplitRows, type DiffFile } from "$lib/activity/diff";
  import { t } from "$lib/i18n";

  interface Props {
    file: DiffFile;
    mode: "unified" | "split";
  }

  let { file, mode }: Props = $props();

  const STEP = 800;
  let limit = $state(STEP);

  const unifiedLines = $derived(file.lines.slice(0, limit));
  const splitRows = $derived(mode === "split" ? toSplitRows(file.lines) : []);
  const visibleSplit = $derived(splitRows.slice(0, limit));
  const total = $derived(mode === "split" ? splitRows.length : file.lines.length);
</script>

<div class="dv-wrap">
  {#if file.lines.length === 0}
    <p class="dv-empty">{$t("No line changes (binary or empty file).")}</p>
  {:else if mode === "unified"}
    <table class="dv-table dv-unified">
      <tbody>
        {#each unifiedLines as line, index (index)}
          {#if line.type === "hunk"}
            <tr class="dv-hunk"><td colspan="3">{line.text}</td></tr>
          {:else if line.type === "meta"}
            <tr class="dv-meta"><td colspan="3">{line.text}</td></tr>
          {:else}
            <tr class="dv-line dv-{line.type}">
              <td class="dv-no">{line.oldNo ?? ""}</td>
              <td class="dv-no">{line.newNo ?? ""}</td>
              <td class="dv-code"><span class="dv-mark">{line.type === "add" ? "+" : line.type === "del" ? "−" : " "}</span>{line.text}</td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  {:else}
    <table class="dv-table dv-split">
      <colgroup>
        <col class="dv-col-no" />
        <col />
        <col class="dv-col-no" />
        <col />
      </colgroup>
      <tbody>
        {#each visibleSplit as row, index (index)}
          {#if row.hunk}
            <tr class="dv-hunk"><td colspan="4">{row.hunk}</td></tr>
          {:else}
            <tr class="dv-line">
              <td class="dv-no">{row.left?.oldNo ?? ""}</td>
              <td class="dv-code dv-half" class:dv-del={row.left?.type === "del"} class:dv-void={!row.left}>{row.left?.text ?? ""}</td>
              <td class="dv-no">{row.right?.newNo ?? ""}</td>
              <td class="dv-code dv-half" class:dv-add={row.right?.type === "add"} class:dv-void={!row.right}>{row.right?.text ?? ""}</td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  {/if}
  {#if total > limit}
    <button type="button" class="dv-more" onclick={() => (limit += STEP)}>
      {$t("Show more")} ({total - limit})
    </button>
  {/if}
</div>
