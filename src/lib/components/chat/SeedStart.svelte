<script lang="ts">
  import {untrack} from 'svelte';
  import {Layers,UserRound,Boxes,Ellipsis,ArrowUpRight,X,Search} from '@lucide/svelte';
  import {seedRequest,type ReadySeedGroup,type SeedFilter} from '$lib/api/seeds';
  import {availablePools,type PoolMode,type SeedPool} from '$lib/chat/seed-pools';
  import {t} from '$lib/i18n';
  import SeedLoading from './SeedLoading.svelte';
  let {workspaceId,folderId,onChoose,onEnabled}:{workspaceId:string;folderId:string;onChoose:(filter:SeedFilter|null)=>Promise<boolean>;onEnabled:(enabled:boolean)=>void}=$props();
  let groups=$state<ReadySeedGroup[]>([]),enabled=$state(false),loading=$state(true),choosing=$state(false),error=$state('');
  let mode=$state<PoolMode>('prefix'),more=$state(false),source=$state(''),search=$state(''),page=$state(0),dialog:HTMLDialogElement;
  let version=0;
  const modes=[{id:'prefix' as const,key:'seeds.byPrefix' as const,icon:Layers},{id:'account' as const,key:'seeds.byAccount' as const,icon:UserRound},{id:'source' as const,key:'seeds.bySource' as const,icon:Boxes}];
  const pools=$derived(availablePools(groups,mode));
  const filtered=$derived(availablePools(groups,mode,search,source));
  const sources=$derived([...new Set(groups.filter(g=>g.ready>0).map(g=>g.source))].sort());
  const pages=$derived(Math.max(1,Math.ceil(filtered.length/30)));
  const currentPage=$derived(Math.min(page,pages-1));
  function category(value:PoolMode){mode=value;page=0;search='';source='';}
  function browse(){search='';source='';page=0;more=true;}
  async function choose(filter:SeedFilter|null){
    if(choosing)return;const generation=version;more=false;choosing=true;error='';
    try{if(!await onChoose(filter)&&generation===version)choosing=false;}
    catch(e){if(generation===version){error=String(e);choosing=false;}}
  }
  async function load(ws:string,folder:string,generation:number){
    try{const result=await seedRequest(ws,folder,{action:'seed_list',limit:1});if(generation!==version)return;
      groups=result.ready_groups??[];enabled=result.enabled;onEnabled(enabled);error='';
    }catch(e){if(generation===version)error=String(e);}
    finally{if(generation===version)loading=false;}
  }
  $effect(()=>{
    const ws=workspaceId,folder=folderId,generation=++version;let running=false;
    const refresh=async()=>{if(running)return;running=true;try{await load(ws,folder,generation);}finally{running=false;}};
    untrack(()=>{groups=[];loading=true;choosing=false;more=false;category('prefix');onEnabled(true);void refresh();});
    const timer=setInterval(()=>{if(!document.hidden&&!choosing)void refresh();},5000);
    return()=>{version++;clearInterval(timer);};
  });
  $effect(()=>{if(more)dialog?.showModal();else dialog?.close();});
</script>
{#snippet poolCard(pool:SeedPool)}
  <button class="pool-card" onclick={()=>choose(pool.filter)} disabled={choosing} title={pool.label||$t('seeds.unlabelled')}>
    <span class="pool-name">{pool.label||$t('seeds.unlabelled')}</span>
    <span class="pool-count"><strong>{pool.ready}</strong><span>{$t('seeds.ready')}</span></span>
    <ArrowUpRight class="pool-arrow" size={16}/>
  </button>
{/snippet}
{#if loading||choosing}<SeedLoading/>
{:else if enabled}
  <section class="seed-choice" aria-label={$t('seeds.choosePool')}>
    <h2>{$t('seeds.choosePool')}</h2><p class="intro">{$t('seeds.chooseCleanHelp')}</p>
    <div class="categories" role="group" aria-label={$t('seeds.poolMode')}>
      {#each modes as item}<button aria-pressed={mode===item.id} onclick={()=>category(item.id)}><item.icon size={15}/>{$t(item.key)}</button>{/each}
    </div>
    {#if error}<p role="alert">{error}<button class="text-button" onclick={()=>load(workspaceId,folderId,version)}>{$t('seeds.retry')}</button></p>{/if}
    {#if !pools.length}<p class="no-ready">{$t('seeds.noReadyPools')}</p>{/if}
    <div class="featured-pools">
      {#each pools.slice(0,2) as pool(pool.key)}{@render poolCard(pool)}{/each}
      <button class="more-card" onclick={browse}><Ellipsis size={24}/><strong>{$t('seeds.morePools')}</strong><span>{pools.length} {$t('seeds.poolKinds')}</span></button>
    </div>
    <button class="manual" onclick={()=>choose(null)}>{$t('seeds.manualChat')}</button>
  </section>
{:else if error}<div role="alert">{error}<button onclick={()=>load(workspaceId,folderId,version)}>{$t('seeds.retry')}</button></div>
{:else}<h2>{$t('chat.connectTitle')}</h2>{/if}
<dialog class="pool-dialog" bind:this={dialog} aria-label={$t('seeds.browsePools')} onclose={()=>more=false} oncancel={()=>more=false}>
  <header><h2>{$t('seeds.browsePools')}</h2><button class="close" aria-label={$t('Close')} onclick={()=>more=false}><X size={18}/></button></header>
  <div class="categories" role="group" aria-label={$t('seeds.poolMode')}>
    {#each modes as item}<button aria-pressed={mode===item.id} onclick={()=>category(item.id)}><item.icon size={15}/>{$t(item.key)}</button>{/each}
  </div>
  <div class="pool-filters"><label><Search size={16}/><input type="search" aria-label={$t('seeds.searchPools')} placeholder={$t('seeds.searchPools')} bind:value={search} oninput={()=>page=0}/></label>
    <select aria-label={$t('seeds.source')} bind:value={source} onchange={()=>page=0}><option value="">{$t('seeds.allSources')}</option>{#each sources as value}<option value={value}>{value}</option>{/each}</select>
  </div>
  <div class="all-pools">{#each filtered.slice(currentPage*30,(currentPage+1)*30) as pool(pool.key)}{@render poolCard(pool)}{:else}<p class="no-ready">{$t('seeds.noReadyPools')}</p>{/each}</div>
  {#if pages>1}<footer><button disabled={currentPage===0} onclick={()=>page=currentPage-1}>{$t('seeds.previous')}</button><span>{currentPage+1} / {pages}</span><button disabled={currentPage+1>=pages} onclick={()=>page=currentPage+1}>{$t('seeds.next')}</button></footer>{/if}
</dialog>
<style>
.seed-choice{max-width:700px;width:100%;margin:auto;text-align:center}h2{font-size:22px;font-weight:600;letter-spacing:-.02em;margin:0}.intro,.no-ready{font-size:12px;line-height:1.7;color:var(--color-text-muted);margin:10px 0 20px}.categories{display:inline-flex;gap:4px;padding:4px;border:1px solid var(--color-border);border-radius:11px;background:var(--card-bg)}button,input,select{font:inherit;color:inherit;min-width:0}button{cursor:pointer;border:1px solid var(--color-border);background:var(--card-bg)}.categories button{display:flex;align-items:center;justify-content:center;gap:7px;padding:8px 17px;font-size:12px;border:0;border-radius:7px;background:transparent;color:var(--color-text-muted)}.categories button[aria-pressed=true]{background:var(--color-border);color:var(--color-text);font-weight:600}.featured-pools{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:12px;margin:20px 0 12px}.pool-card,.more-card{position:relative;min-height:138px;padding:20px;border-radius:12px;text-align:left}.pool-card{display:flex;flex-direction:column;justify-content:space-between;gap:22px}.pool-card:hover,.more-card:hover{border-color:var(--primary,#6366f1);background:var(--surface-hover,var(--card-bg))}.pool-name{font-size:14px;font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:100%}.pool-count{display:flex;align-items:baseline;gap:7px}.pool-count strong{font-size:29px;font-weight:600;font-variant-numeric:tabular-nums}.pool-count>span{font-size:11px;color:var(--color-text-muted)}:global(.pool-arrow){position:absolute;bottom:23px;right:16px;color:var(--color-text-muted)}.more-card{display:flex;align-items:center;justify-content:center;flex-direction:column;gap:8px;border-style:dashed;color:var(--color-text-muted);text-align:center}.more-card strong{font-size:13px;font-weight:500}.more-card span{font-size:11px}.manual{font-size:12px;padding:10px;border:0;background:transparent;color:var(--color-text-muted)}.pool-dialog{width:min(760px,calc(100vw - 28px));max-height:85dvh;overflow:auto;margin:auto;padding:24px;border:1px solid var(--color-border);border-radius:16px;background:var(--surface-2,var(--card-bg));color:var(--color-text)}.pool-dialog::backdrop{background:#0009}.pool-dialog header{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-bottom:20px}.pool-dialog h2{font-size:18px}.close{display:grid;place-items:center;border:0;border-radius:6px;padding:6px;background:transparent}.pool-filters{display:flex;gap:10px;margin:18px 0}.pool-filters label{display:flex;align-items:center;gap:8px;flex:1;border:1px solid var(--color-border);border-radius:9px;padding:0 10px;color:var(--color-text-muted);min-width:0}.pool-filters input{width:100%;padding:11px 0;background:transparent;border:0;outline:none}.pool-filters select{max-width:40%;padding:9px;border:1px solid var(--color-border);border-radius:9px;background:var(--card-bg);font-size:12px}.all-pools{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:10px;max-height:48dvh;overflow:auto;padding:3px}.all-pools .no-ready{grid-column:1/-1;text-align:center}footer{display:flex;align-items:center;justify-content:center;gap:12px;font-size:12px;margin-top:18px}footer button{border-radius:7px;padding:7px 12px}.text-button{border:0;background:transparent;padding:6px;font-size:12px}button:disabled{opacity:.5;cursor:default}button:focus-visible,select:focus-visible{outline:2px solid var(--primary,#6366f1);outline-offset:3px}.pool-filters label:focus-within{outline:2px solid var(--primary,#6366f1);outline-offset:2px}[role=alert]{color:var(--danger);font-size:12px;overflow-wrap:anywhere}@media(max-width:540px){h2{font-size:19px}.featured-pools{gap:8px}.pool-card,.more-card{padding:13px;min-height:125px}.pool-name{font-size:12px}.pool-count{gap:4px}.pool-count strong{font-size:25px}.pool-count>span{font-size:10px}:global(.pool-arrow){display:none}.categories button{padding:8px 12px}.pool-dialog{padding:16px}.all-pools{grid-template-columns:repeat(2,minmax(0,1fr))}}
</style>
