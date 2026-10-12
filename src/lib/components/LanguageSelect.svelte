<script lang="ts">
  import Languages from "@lucide/svelte/icons/languages";
  import MenuSelect from "./MenuSelect.svelte";
  import { LOCALE_OPTIONS, localePreference, setLocale, t, type LocalePreference } from "$lib/i18n";
  const options = $derived([{ value: "system" as LocalePreference, label: $t("Follow system") }, ...LOCALE_OPTIONS.map(o => ({ value: o.value as LocalePreference, label: o.label }))]);
</script>

<MenuSelect value={$localePreference} {options} label={$t("Language")} onChange={setLocale}>
  {#snippet icon()}<Languages size={14} aria-hidden="true" />{/snippet}
</MenuSelect>
