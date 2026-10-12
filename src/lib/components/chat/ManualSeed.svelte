<script lang="ts">
  import {seedRequest,type SeedTicket} from '$lib/api/seeds';
  import {seedEnrollmentPrompt} from '$lib/chat/seed-enrollment';
  import {pairingAddress} from '$lib/chat/seed-pairing';
  import {copyText} from '$lib/browser-tools';
  import {t} from '$lib/i18n';
  import Copy from '@lucide/svelte/icons/copy';
  import Check from '@lucide/svelte/icons/check';
  let {workspaceId,folderId,address,onCreated}:{workspaceId:string;folderId:string;address:string;onCreated:()=>void}=$props();
  /** Common AI hosts as one-click source chips; any other value can be typed. */
  const SOURCES=['arena','chatgpt','claude','gemini','grok','kimi','doubao','qwen','deepseek'];
  let source=$state('arena'),prefix=$state(''),account=$state(''),endpoint=$state(''),prompt=$state(''),busy=$state(false),error=$state(''),copied=$state(false),scope=0;
  $effect(()=>{const folder=folderId,ws=workspaceId;scope++;prompt='';error='';copied=false;endpoint=address;});
  async function create(){if(busy)return;busy=true;error='';const generation=scope;try{
    const target=pairingAddress(endpoint,folderId);
    const result=await seedRequest<{batch:SeedTicket[]}>(workspaceId,folderId,{action:'seed_manual',account:account.trim()||source,source,prefix});
    if(generation!==scope)return;prompt=seedEnrollmentPrompt(target,folderId,result.batch[0]);copied=false;onCreated();
  }catch(e){if(generation===scope)error=String(e);}finally{busy=false;}}
  async function copy(){try{await copyText(prompt);copied=true;}catch(e){error=String(e);}}
</script>
<section class="manual-card" aria-label={$t('seeds.manualEnroll')}>
  {#if !prompt}
  <form onsubmit={e=>{e.preventDefault();void create();}}>
    <fieldset class="step">
      <legend>{$t('seeds.stepSource')}</legend>
      <div class="chips" role="radiogroup" aria-label={$t('seeds.source')}>
        {#each SOURCES as item}<button type="button" role="radio" aria-checked={source===item} class:active={source===item} onclick={()=>source=item}>{item}</button>{/each}
      </div>
      <label>{$t('seeds.source')}<input required maxlength="200" bind:value={source} placeholder="arena"/></label>
    </fieldset>
    <fieldset class="step two">
      <legend>{$t('seeds.stepLabel')}</legend>
      <label>{$t('seeds.prefix')}<input maxlength="200" bind:value={prefix} placeholder="Opus / Grok / GPT-6-astra"/></label>
      <label>{$t('seeds.account')}<input maxlength="200" bind:value={account} placeholder={source}/></label>
    </fieldset>
    <details class="advanced"><summary>{$t('seeds.advanced')}</summary>
      <label>{$t('seeds.pairAddress')}<input required type="url" bind:value={endpoint}/></label>
    </details>
    <p class="hint">{$t('seeds.worksWith')}</p>
    <button class="primary" disabled={busy||!endpoint}>{$t(busy?'Working…':'seeds.createManual')}</button>
  </form>
  {:else}
  <div class="result">
    <p class="hint">{$t('seeds.manualPromptHelp')}</p>
    <textarea aria-label={$t('seeds.manualPrompt')} readonly value={prompt} rows="8"></textarea>
    <div class="actions">
      <button class="primary" onclick={copy}>{#if copied}<Check size={15}/>{:else}<Copy size={15}/>{/if}{$t(copied?'seeds.copied':'seeds.copyPrompt')}</button>
      <button onclick={()=>{prompt='';copied=false;}}>{$t('seeds.generateAnother')}</button>
    </div>
    <p class="hint">{$t('seeds.worksWith')}</p>
  </div>
  {/if}
  {#if error}<p role="alert">{error}</p>{/if}
</section>
<style>
.manual-card{display:grid;gap:14px}
form{display:grid;gap:14px}
.step{border:1px solid var(--color-border);border-radius:12px;padding:12px 14px 14px;margin:0;display:grid;gap:10px;background:var(--card-bg)}
.step.two{grid-template-columns:repeat(2,minmax(0,1fr))}
.step.two legend{grid-column:1/-1}
legend{font-size:12px;font-weight:600;padding:0 6px}
.chips{display:flex;flex-wrap:wrap;gap:6px}
.chips button{padding:5px 11px;border-radius:999px;font-size:12px}
.chips button.active{border-color:var(--primary,#2563eb);background:color-mix(in srgb,var(--primary,#2563eb) 14%,transparent);font-weight:600}
label{display:grid;gap:6px;font-size:12px}
input,textarea,button{padding:9px 10px;border:1px solid var(--color-border);border-radius:8px;background:var(--card-bg);font:inherit;min-width:0;color:inherit}
textarea{box-sizing:border-box;width:100%;font-size:12px;font-family:var(--font-mono,monospace);resize:vertical}
button{cursor:pointer;display:inline-flex;align-items:center;gap:6px;justify-content:center}
button:disabled{opacity:.5;cursor:default}
button.primary{background:var(--primary,#2563eb);border-color:var(--primary,#2563eb);color:#fff;font-weight:600}
.advanced{font-size:12px}.advanced summary{cursor:pointer;color:var(--color-text-muted)}.advanced label{margin-top:8px}
.hint{margin:0;font-size:12px;color:var(--color-text-muted);line-height:1.7}
.result{display:grid;gap:10px}.actions{display:flex;gap:8px;flex-wrap:wrap}
[role=alert]{color:var(--danger);font-size:12px;margin:0}
@media(max-width:700px){.step.two{grid-template-columns:1fr}}
</style>
