<script lang="ts">
  import Languages from "@lucide/svelte/icons/languages";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import { LOCALE_OPTIONS, localePreference, setLocale, t, type LocalePreference } from "$lib/i18n";

  function handleChange(event: Event) {
    setLocale((event.currentTarget as HTMLSelectElement).value as LocalePreference);
  }
</script>

<label class="tx-lang-select">
  <Languages size={14} aria-hidden="true" />
  <span class="sr-only">{$t("Language")}</span>
  <select aria-label={$t("Language")} value={$localePreference} onchange={handleChange}>
    <option value="system">{$t("Follow system")}</option>
    {#each LOCALE_OPTIONS as option}
      <option value={option.value}>{option.label}</option>
    {/each}
  </select>
  <ChevronDown class="pointer-events-none shrink-0 opacity-60" size={12} aria-hidden="true" />
</label>
