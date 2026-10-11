<script lang="ts">
  import {seedRequest,type SeedTicket} from '$lib/api/seeds';
  import {seedEnrollmentPrompt} from '$lib/chat/seed-enrollment';
  import {pairingAddress} from '$lib/chat/seed-pairing';
  import {copyText} from '$lib/browser-tools';
  import {t} from '$lib/i18n';
  let {workspaceId,folderId,address,onCreated}:{workspaceId:string;folderId:string;address:string;onCreated:()=>void}=$props();
  let source=$state('arena'),prefix=$state(''),account=$state(''),endpoint=$state(''),prompt=$state(''),busy=$state(false),error=$state(''),copied=$state(false),scope=0;
  $effect(()=>{const folder=folderId,ws=workspaceId;scope++;prompt='';error='';copied=false;endpoint=address;});
  async function create(){if(busy)return;busy=true;error='';const generation=scope;try{
    const target=pairingAddress(endpoint,folderId);
    const result=await seedRequest<{batch:SeedTicket[]}>(workspaceId,folderId,{action:'seed_manual',account,source,prefix});
    if(generation!==scope)return;prompt=seedEnrollmentPrompt(target,folderId,result.batch[0]);copied=false;onCreated();
  }catch(e){if(generation===scope)error=String(e);}finally{busy=false;}}
  async function copy(){try{await copyText(prompt);copied=true;}catch(e){error=String(e);}}
</script>
<details class="manual-enrollment"><summary>{$t('seeds.manualEnroll')}</summary><p>{$t('seeds.manualHelp')}</p>
  <form onsubmit={e=>{e.preventDefault();void create();}}>
    <label>{$t('seeds.source')}<input required maxlength="200" bind:value={source} placeholder="arena"/></label>
    <label>{$t('seeds.prefix')}<input maxlength="200" bind:value={prefix} placeholder="Opus / Grok / GPT-6-astra"/></label>
    <label>{$t('seeds.account')}<input required maxlength="200" bind:value={account}/></label>
    <label>{$t('seeds.pairAddress')}<input required type="url" bind:value={endpoint}/></label>
    <button disabled={busy}>{$t('seeds.createManual')}</button>
  </form>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if prompt}<p>{$t('seeds.manualPromptHelp')}</p><textarea aria-label={$t('seeds.manualPrompt')} readonly value={prompt} rows="7"></textarea><button onclick={copy}>{$t(copied?'seeds.copied':'seeds.copyPrompt')}</button>{/if}
</details>
<style>
.manual-enrollment{border:1px solid var(--color-border);padding:16px;border-radius:12px;margin:18px 0}summary{cursor:pointer}p{font-size:12px;color:var(--color-text-muted);line-height:1.7}form{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px}label{display:grid;gap:6px;font-size:12px}input,textarea,button{padding:9px 10px;border:1px solid var(--color-border);border-radius:8px;background:var(--card-bg);font:inherit;min-width:0}textarea{box-sizing:border-box;width:100%;font-size:12px;margin-bottom:10px}button{cursor:pointer}button:disabled{opacity:.5}[role=alert]{color:var(--danger)}@media(max-width:700px){form{grid-template-columns:1fr}}
</style>
