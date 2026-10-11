<script lang="ts">
  import {copyText} from "$lib/browser-tools";
  import { t } from "$lib/i18n";
  interface Props {
    value: string;
    label?: string;
    onCopy?: () => void;
  }

  let { value, label, onCopy }: Props = $props();
  let copied = $state(false);

  async function copy() {
    await copyText(value);
    copied = true;
    onCopy?.();
    setTimeout(() => {
      copied = false;
    }, 1500);
  }
</script>

<button
  type="button"
  class="tx-btn-ghost shrink-0 px-2.5 py-1 text-xs"
  onclick={copy}
>
  {copied ? $t("Copied") : label ?? $t("Copy")}
</button>
