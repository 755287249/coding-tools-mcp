<script lang="ts">
  import {t} from '$lib/i18n';
  // seed: waiting for a pool seed; direct: preparing [接入AI]; list: reading the seed library.
  let {variant='seed'}:{variant?:'seed'|'direct'|'list'}=$props();
  const title=$derived(variant==='direct'?'seeds.directLoading':variant==='list'?'Loading…':'seeds.loading');
  const help=$derived(variant==='direct'?'seeds.directLoadingHelp':variant==='list'?'':'seeds.loadingHelp');
</script>
<div class="seed-loading" role="status" aria-live="polite">
  <h2>{$t(title)}</h2>
  <div class="track" role="progressbar" aria-label={$t(title)}><span></span></div>
  {#if help}<p>{$t(help)}</p>{/if}
</div>
<style>
.seed-loading{width:min(100%,400px);margin:auto;text-align:center}h2{font-size:23px;font-weight:600}.track{height:4px;background:var(--color-border);border-radius:5px;overflow:hidden;margin:18px 24px}.track span{display:block;height:100%;width:40%;background:#5798ed;border-radius:5px;animation:seed-pulse 1.6s ease-in-out infinite}p{font-size:12px;color:var(--color-text-muted);line-height:1.7}@keyframes seed-pulse{0%{transform:translateX(-110%);opacity:.35}50%{opacity:1}100%{transform:translateX(360%);opacity:.35}}@media(prefers-reduced-motion:reduce){.track span{animation:none;width:100%;opacity:.55}}
</style>
