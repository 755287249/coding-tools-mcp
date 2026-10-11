<script lang="ts">
  import {untrack} from 'svelte';
  import {seedRequest,type SeedGroup,type SeedFilter} from '$lib/api/seeds';
  import {t} from '$lib/i18n';
  import SeedLoading from './SeedLoading.svelte';
  let {workspaceId,folderId,onChoose,onEnabled}:{workspaceId:string;folderId:string;onChoose:(filter:SeedFilter|null)=>Promise<boolean>;onEnabled:(enabled:boolean)=>void}=$props();
  let groups=$state<SeedGroup[]>([]),enabled=$state(false),loading=$state(true),choosing=$state(false),error=$state(''),source=$state(''),keyword=$state('');
  let version=0;
  const sources=$derived([...new Set(groups.map(g=>g.source))]);
  const count=$derived(groups.filter(g=>(!source||g.source===source)&&(!keyword||g.prefix.toLowerCase().includes(keyword.trim().toLowerCase()))).reduce((n,g)=>n+g.ready,0));
  async function choose(filter:SeedFilter|null){if(choosing)return;choosing=true;try{if(!await onChoose(filter)){choosing=false;}}catch(e){error=String(e);choosing=false;}}
  async function load(ws:string,folder:string,generation:number){loading=true;error='';try{
    const result=await seedRequest(ws,folder,{action:'seed_list',limit:1});if(generation!==version)return;
    groups=result.groups??[];enabled=result.enabled;onEnabled(enabled);loading=false;
    if(enabled&&groups.length<=1)await choose({source:groups[0]?.source??'',keyword:groups[0]?.prefix??''});
  }catch(e){if(generation===version){error=String(e);loading=false;}}}
  $effect(()=>{const ws=workspaceId,folder=folderId,generation=++version;untrack(()=>{choosing=false;source='';keyword='';onEnabled(true);void load(ws,folder,generation);});return()=>{version++;};});
</script>
{#if loading||choosing}<SeedLoading/>
{:else if error}<div role="alert">{error}<button onclick={()=>load(workspaceId,folderId,version)}>{$t('seeds.retry')}</button></div>
{:else if enabled}
  <div class="seed-choice"><h2>{$t('seeds.choosePool')}</h2><p>{$t('seeds.chooseHelp')}</p>
    <div class="pool-options">{#each groups.filter(g=>(!source||g.source===source)&&(!keyword||g.prefix.toLowerCase().includes(keyword.trim().toLowerCase()))).slice(0,50) as group}<button onclick={()=>choose({source:group.source,keyword:group.prefix})}><strong>{group.prefix||$t('seeds.unlabelled')}</strong><span>{group.source} · {group.ready} {$t('seeds.ready')}</span></button>{/each}</div>
    <form onsubmit={e=>{e.preventDefault();void choose({source,keyword:keyword.trim()});}}>
      <select aria-label={$t('seeds.source')} bind:value={source}><option value="">{$t('seeds.allSources')}</option>{#each sources as item}<option value={item}>{item}</option>{/each}</select>
      <input aria-label={$t('seeds.keyword')} placeholder={$t('seeds.keyword')} bind:value={keyword}/>
      <button class="primary">{$t('seeds.startPool')} · {count}</button>
    </form>
    <button class="manual" onclick={()=>choose(null)}>{$t('seeds.manualChat')}</button>
  </div>
{:else}<h2>{$t('chat.connectTitle')}</h2>{/if}
<style>
.seed-choice{max-width:620px;width:100%;margin:auto}h2{font-size:23px;font-weight:600}p{font-size:12px;line-height:1.7;color:var(--color-text-muted)}.pool-options{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:10px;margin:18px 0;max-height:240px;overflow:auto}.pool-options button{display:grid;gap:5px;text-align:left}.pool-options span{font-size:11px;color:var(--color-text-muted)}button,input,select{padding:10px;border:1px solid var(--color-border);border-radius:9px;background:var(--card-bg);font:inherit;min-width:0}button{cursor:pointer}form{display:flex;gap:8px;flex-wrap:wrap}input{flex:1;min-width:120px}.primary{background:#2563eb;color:white}.manual{font-size:12px;margin-top:12px;border:0;color:var(--color-text-muted)}
</style>
