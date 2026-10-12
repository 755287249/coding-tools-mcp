<script lang="ts">
  import {seedEnrollmentPrompt} from '$lib/chat/seed-enrollment';
  import {showToast} from '$lib/stores/toast';
  import ManualSeed from './ManualSeed.svelte';
  import {seedRequest,type SeedInventory,type SeedRow,type SeedTicket} from '$lib/api/seeds';
  import {copyText} from '$lib/browser-tools';
  import {t} from '$lib/i18n';
  import type {MessageKey} from '$lib/i18n/catalog';
  import {workspaces,mcpConnection} from '$lib/stores/app';
  import {APP_VERSION} from '$lib/app-version';
  import {appUrl} from '$lib/app-path';
  import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
  import type {Snippet} from 'svelte';
  import RotateCw from '@lucide/svelte/icons/rotate-cw';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import X from '@lucide/svelte/icons/x';
  import Pencil from '@lucide/svelte/icons/pencil';
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import Bot from '@lucide/svelte/icons/bot';
  import Puzzle from '@lucide/svelte/icons/puzzle';
  import {untrack,onMount} from 'svelte';
  import {flip} from 'svelte/animate';
  import {slide} from 'svelte/transition';
  let motionMs=$state(0);
  onMount(()=>{const query=matchMedia('(prefers-reduced-motion: reduce)');const update=()=>motionMs=query.matches?0:140;update();query.addEventListener('change',update);return()=>query.removeEventListener('change',update);});
  import CodeRabbitPlugin from './CodeRabbitPlugin.svelte';
  import {installedAppPlugins,setAppPlugin} from '$lib/plugins/registry';
  import Sprout from '@lucide/svelte/icons/sprout';
  import {pairingKey,pairingAddress,pairingWebhook,loadPairing,savePairing,newPairingToken} from '$lib/chat/seed-pairing';
  let {workspaceId,folderId,workspacePicker}:{workspaceId:string;folderId:string;workspacePicker?:Snippet}=$props();
  const empty=():SeedInventory=>({enabled:false,seeds:[],retired_count:0});
  let inventory=$state<SeedInventory>(empty()),address=$state(''),token=$state('');
  let busy=$state(false),error=$state(''),notice=$state(''),history=$state(false),filter=$state(''),status=$state(''),search=$state(''),page=$state(0),sourceFilter=$state(''),keyword=$state('');
  let addMethod=$state<'manual'|'pair'|'rabbit'>('manual'),view=$state<'accounts'|'seeds'>('accounts'),loading=$state(true);
  let addOpen=$state(false),refreshing=$state(false),updatedAt=$state(0),refreshKey=$state(0),debouncedSearch=$state(''),debouncedKeyword=$state('');
  let refreshingAccount=$state('');
  function refreshAccount(account:string){if(busy||refreshing)return;refreshingAccount=account;refreshKey++;}
  function accountHue(account:string){let hash=0;for(const char of account)hash=(Math.imul(hash,31)+char.charCodeAt(0))>>>0;return [215,270,155,32,335,185][hash%6];}
  function editSeed(event:MouseEvent){event.preventDefault();event.stopPropagation();const row=(event.currentTarget as HTMLElement).closest('details');if(row){row.open=true;row.querySelector<HTMLInputElement>('.row-labels input')?.focus();}}
  let moreFilters=$state(false),selected=$state<string[]>([]),batchSource=$state(''),batchPrefix=$state(''),reissuedPrompt=$state('');
  let confirmation=$state<{args:Record<string,unknown>;label:MessageKey;count:number}|null>(null);
  const selectedRows=$derived(inventory.seeds.filter(seed=>selected.includes(seed.id)));
  const unsettled=$derived(selectedRows.some(seed=>seed.unresolved_operations>0));
  function ask(args:Record<string,unknown>,label:MessageKey,count=1){confirmation={args,label,count};}
  function toggle(id:string,checked:boolean){selected=checked?[...selected.filter(item=>item!==id),id]:selected.filter(item=>item!==id);}
  async function reissue(seed:SeedRow){
    if(busy||refreshing)return;busy=true;error='';const generation=scope;
    try{
      const endpoint=pairingAddress(address,folderId);
      const result=await seedRequest<{batch:SeedTicket[]}>(workspaceId,folderId,{action:'seed_reissue',seed_id:seed.id});
      if(generation===scope){reissuedPrompt=seedEnrollmentPrompt(endpoint,folderId,result.batch[0]);refreshKey++;}
    }catch(e){error=e instanceof Error?e.message:String(e);}finally{busy=false;}
  }
  function openModal(node:HTMLDialogElement){node.showModal();return {destroy:()=>node.close()};}
  let scope=0;
  const accounts=$derived(inventory.accounts??[]),counts=$derived(inventory.counts??{});
  const pageCount=$derived(Math.max(1,Math.ceil((inventory.filtered_total??0)/50)));
  $effect(()=>{const key=pairingKey(workspaceId,folderId);untrack(()=>{
    const saved=loadPairing(key),profile=$mcpConnection;
    address=saved.address;token=saved.token;
    // Prefer the current public address: a changed tunnel URL must not leave a stale webhook.
    if(profile?.tunnel.public_url){try{address=pairingAddress(profile.tunnel.public_url.replace(/\/$/,'').replace(/\/mcp$/,'')+'/mcp',folderId);}catch{}}
    addMethod='manual';view='accounts';addOpen=false;updatedAt=0;selected=[];confirmation=null;reissuedPrompt='';
    inventory=empty();error='';notice='';history=false;filter='';status='';search='';page=0;sourceFilter='';keyword='';
  });});
  $effect(()=>{const value=search,label=keyword;const timer=setTimeout(()=>{debouncedSearch=value;debouncedKeyword=label;page=0;},300);return()=>clearTimeout(timer);});
  $effect(()=>{const ws=workspaceId,folder=folderId,query={action:'seed_list',limit:50,offset:page*50,account:filter,status,search:debouncedSearch,source:sourceFilter,keyword:debouncedKeyword,include_archived:history};refreshKey;
    const generation=++scope;selected=[];loading=untrack(()=>updatedAt===0);
    let stopped=false,failures=0,timer:ReturnType<typeof setTimeout>;
    const refresh=async()=>{
      if(stopped)return;
      if(document.hidden){timer=setTimeout(refresh,5000);return;}
      refreshing=true;
      try{const result=await seedRequest(ws,folder,query);if(!stopped&&scope===generation){inventory=result;selected=selected.filter(id=>result.seeds.some(seed=>seed.id===id));updatedAt=Date.now();error='';failures=0;const last=Math.max(0,Math.ceil((result.filtered_total??0)/50)-1);if(page>last)page=last;}}
      catch(e){if(!stopped&&scope===generation){error=e instanceof Error?e.message:String(e);failures++;}}
      finally{if(!stopped&&scope===generation){loading=false;refreshing=false;refreshingAccount='';timer=setTimeout(refresh,Math.min(60000,5000*2**failures));}}
    };
    void refresh();return()=>{stopped=true;clearTimeout(timer);};
  });
  function query(){return {limit:50,offset:page*50,account:filter,status,search:debouncedSearch,source:sourceFilter,keyword:debouncedKeyword,include_archived:history};}
  async function action(args:Record<string,unknown>){if(busy||refreshing)return;busy=true;error='';const generation=scope;if(args.action==='seed_delete_account'&&filter===args.account)filter='';try{const result=await seedRequest(workspaceId,folderId,{...query(),...args});if(generation===scope){notice=result.affected_count===undefined?$t('seeds.actionSaved'):$t('seeds.affected',{count:result.affected_count});selected=[];showToast(notice,{kind:'success'});refreshKey++;}}catch(e){if(generation===scope){error=e instanceof Error?e.message:String(e);showToast(error,{kind:'error'});}}finally{busy=false;}}
  const webhook=$derived(inventory.pairing_enabled?pairingWebhook(address,token):'');
  /** Issue a new webhook token; the previous webhook stops working at once. */
  async function connect(){if(busy||refreshing)return;busy=true;error='';notice='';const generation=scope,ws=workspaceId,folder=folderId;try{
    const saved={address:pairingAddress(address,folder),token:newPairingToken()};
    if(!/^[a-f0-9]{64}$/.test(saved.token))throw Error($t('seeds.tokenInvalid'));
    const result=await seedRequest(ws,folder,{...query(),action:'seed_pairing',token:saved.token});
    const stored=savePairing(pairingKey(ws,folder),saved);
    if(generation===scope){address=saved.address;token=saved.token;inventory=result;notice=$t(stored?'seeds.webhookSaved':'seeds.pairStorage');}
  }catch(e){if(generation===scope)error=String(e);}finally{busy=false;}}
  async function copy(value:string){try{await copyText(value);notice=$t('seeds.copied');showToast(notice,{kind:'success'});}catch(e){error=String(e);showToast(error,{kind:'error'});}}
  const date=(value:number)=>value?new Date(value).toLocaleString():$t('seeds.never');
  /** Rarely needed fields, shown behind the "More info" toggle of an open row. */
  function advanced(seed:SeedRow):[string,string][]{
    const rows:[string,string|number|undefined][]=[['ID',seed.id],[$t('seeds.source'),seed.source],[$t('seeds.prefix'),seed.prefix||'—'],
      [$t('seeds.repository'),seed.metadata?.repo_name||seed.repo_id],[$t('seeds.branch'),seed.branch],
      ...Object.entries(seed.metadata??{}).filter(([key,value])=>value&&key!=='repo_name') as [string,string][],
      [$t('seeds.created'),date(seed.created_at)],[$t('seeds.hostTask'),seed.host_task_id||'—'],[$t('seeds.conversation'),seed.chat_id||'—'],
      [$t('seeds.generation'),seed.generation]];
    return rows.filter(([,value])=>value!==undefined&&value!=='').map(([label,value])=>[label,String(value)]);
  }
  function details(seed:SeedRow){return [seed.account,`${$t('seeds.source')}: ${seed.source}`,`${$t('seeds.prefix')}: ${seed.prefix||'—'}`,seed.metadata?.repo_name||seed.repo_id,seed.branch,`ID: ${seed.id}`,...Object.entries(seed.metadata??{}).filter(([,v])=>v).map(([k,v])=>`${k}: ${v}`),`${$t('seeds.created')}: ${date(seed.created_at)}`,`${$t('seeds.lastSeen')}: ${date(seed.last_seen)}`,`${$t('seeds.hostTask')}: ${seed.host_task_id||'—'}`,`${$t('seeds.conversation')}: ${seed.chat_id||'—'}`,`${$t('seeds.generation')}: ${seed.generation}`,`${$t('seeds.unresolved')}: ${seed.unresolved_operations}`,seed.reason?$t(`seeds.reason.${seed.reason}` as MessageKey):''].filter(Boolean).join('\n');}
</script>
<div class="seed-library" aria-busy={busy||refreshing}>
  <header class="library-header"><h2>{$t('seeds.library')}</h2>{#if workspacePicker}{@render workspacePicker()}{/if}<button disabled={refreshing||busy} onclick={()=>refreshKey++}>{$t(refreshing?'seeds.refreshing':'seeds.refresh')}</button><span class="muted">{$t('seeds.updated')}: {date(updatedAt)}</span><label><input type="checkbox" checked={inventory.enabled} disabled={busy||refreshing||loading} onchange={e=>action({action:'seed_settings',enabled:e.currentTarget.checked})}/>{$t('seeds.auto')}</label></header>
  <div class="overview" aria-label={$t('seeds.globalSummary')}>
    {#each ['ready','assigned','preparing','offline','retired'] as state}<button class:active={status===state} onclick={()=>{status=status===state?'':state;view='seeds';page=0;}}><span>{$t(`seeds.${state}` as MessageKey)}</span><strong>{counts[state]??0}</strong></button>{/each}
  </div>
  <div class="toolbar library-actions"><span class="muted">{$t('seeds.globalSummary')} · {$t('seeds.total')}: {inventory.total??0} / {inventory.capacity??10000}</span><button disabled={busy||refreshing||loading} onclick={()=>ask({action:'seed_cleanup'},'seeds.cleanup',0)}>{$t('seeds.cleanup')}</button><button class="primary" onclick={()=>addOpen=true}>{$t('seeds.addSeeds')}</button></div>
  {#if busy}<p class="notice" role="status">{$t('Working…')}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  {#if addOpen}<dialog class="add-dialog" use:openModal onclose={()=>addOpen=false} aria-label={$t('seeds.addSeeds')}><header class="library-header dialog-head"><h2>{$t('seeds.addSeeds')}</h2><button class="dialog-close" aria-label={$t('Close')} title={$t('Close')} onclick={()=>addOpen=false}><X size={18}/></button></header>
  <p class="add-intro">{$t('seeds.addIntro')}</p>
  <div class="method-cards" role="tablist" aria-label={$t('seeds.addSeeds')}>
    <button role="tab" aria-selected={addMethod==='manual'} class:active={addMethod==='manual'} onclick={()=>addMethod='manual'}>
      <span class="method-icon"><Bot size={20}/></span>
      <span class="method-text"><strong>{$t('seeds.manualEnroll')}<em>{$t('seeds.recommended')}</em></strong><small>{$t('seeds.methodManualDesc')}</small></span>
    </button>
    <button role="tab" aria-selected={addMethod==='pair'} class:active={addMethod==='pair'} onclick={()=>addMethod='pair'}>
      <span class="method-icon"><Puzzle size={20}/></span>
      <span class="method-text"><strong>{$t('seeds.browserPair')}<span class="pair-state" class:connected={inventory.pairing_enabled}>{$t(inventory.pairing_enabled?'seeds.pairEnabled':'seeds.pairDisabled')}</span></strong><small>{$t('seeds.methodPairDesc')}</small></span>
    </button>
    {#if $installedAppPlugins.includes('coderabbit-seeds')}<button role="tab" aria-selected={addMethod==='rabbit'} class:active={addMethod==='rabbit'} onclick={()=>addMethod='rabbit'}>
      <span class="method-icon"><Sprout size={20}/></span>
      <span class="method-text"><strong>{$t('rabbit.title')}</strong><small>{$t('rabbit.desc')}</small></span>
    </button>{:else}<button role="tab" aria-selected="false" class="install-card" onclick={()=>{setAppPlugin('coderabbit-seeds',true);addMethod='rabbit';}}>
      <span class="method-icon"><Sprout size={20}/></span>
      <span class="method-text"><strong>{$t('rabbit.title')}</strong><small>{$t('plugins.installHere')}</small></span>
    </button>{/if}
  </div>
  <div class="method-panel" role="tabpanel">
  {#if addMethod==='rabbit'}
  <CodeRabbitPlugin {workspaceId} {folderId}/>
  {:else if addMethod==='manual'}
  <ManualSeed {workspaceId} {folderId} {address} onCreated={()=>refreshKey++}/>
  {:else}
  <div class="pairing">
    <p class="notice">{$t('seeds.webhookHelp')}</p>
    {#if !$mcpConnection?.tunnel.public_url}<label>{$t('seeds.pairAddress')}<input bind:value={address} type="url" placeholder="https://example.com/mcp"/></label>{/if}
    <label>{$t('seeds.webhook')}<div class="input-row"><input readonly value={webhook} placeholder={$t('seeds.webhookEmpty')} onfocus={e=>e.currentTarget.select()}/>
      <button type="button" disabled={!webhook} onclick={()=>copy(webhook)}>{$t('seeds.copy')}</button>
      <button type="button" class:primary={!webhook} disabled={busy||refreshing||!address} title={$t('seeds.webhookRefreshHint')} onclick={()=>void connect()}>{$t('seeds.refresh')}</button></div></label>
    <a class="install-link" href={`https://github.com/755287249/coding-tools-mcp/releases/download/v${APP_VERSION}/coderabbit-seeds.user.js`} target="_blank" rel="noreferrer">{$t('seeds.install')}</a>
  </div>
  {/if}
  </div>
  </dialog>{/if}

  <div class="toolbar filters">
    <input class="search" type="search" aria-label={$t('seeds.search')} placeholder={$t('seeds.search')} bind:value={search}/>
    <select aria-label={$t('seeds.source')} bind:value={sourceFilter} onchange={()=>page=0}><option value="">{$t('seeds.allSources')}</option>{#each inventory.sources??[] as source}<option value={source}>{source}</option>{/each}</select>

    <select aria-label={$t('seeds.account')} bind:value={filter} onchange={()=>{page=0;view='seeds';}}><option value="">{$t('seeds.allAccounts')}</option>{#each accounts as a}<option value={a}>{a}</option>{/each}</select>
    <select aria-label={$t('seeds.allStates')} bind:value={status} onchange={()=>page=0}><option value="">{$t('seeds.allStates')}</option>{#each ['ready','assigned','working','preparing','offline','retired'] as state}<option value={state}>{$t(`seeds.${state}` as MessageKey)}</option>{/each}</select>
    <button aria-expanded={moreFilters} onclick={()=>moreFilters=!moreFilters}>{$t('seeds.morePools')}</button>
    {#if moreFilters}<input aria-label={$t('seeds.keyword')} placeholder={$t('seeds.keyword')} bind:value={keyword}/><label><input type="checkbox" bind:checked={history} onchange={()=>page=0}/>{$t('seeds.history')}</label>{/if}
  </div>
  {#if view==='seeds'}
    <div class="toolbar account-heading"><button class="back-accounts" onclick={()=>{filter='';status='';view='accounts';page=0;}}><ArrowLeft size={16}/>{$t('seeds.backAccounts')}</button><h2>{filter}</h2></div>
  {/if}
  <div class="list-head muted"><span>{$t(view==='seeds'?'seeds.hoverDetails':'seeds.accountSummaryHelp')}</span><span>{inventory.filtered_total??0} {$t('seeds.items')}</span></div>
  {#if loading}
    <div class="empty skeleton" role="status" aria-label={$t('seeds.inventoryLoading')}><span></span><span></span><span></span></div>
  {:else if view==='accounts'}
    <div class="account-grid">
      {#each inventory.account_summaries??[] as account(account.account)}
        <article class="account-card" style={`--account-hue:${accountHue(account.account)}`} animate:flip={{duration:motionMs}} transition:slide={{duration:motionMs}}>
          <header><span class="dot" class:alive={account.valid>0}></span><h2 title={account.account}>{account.account}</h2><button class="account-manage-icon" title={$t('seeds.manageAccount')} aria-label={`${$t('seeds.manageAccount')} ${account.account}`} onclick={()=>{filter=account.account;view='seeds';page=0;}}><SlidersHorizontal size={15}/></button><button class="account-refresh" disabled={busy||refreshing} aria-label={`${$t('seeds.refresh')} ${account.account}`} title={$t('seeds.refresh')} onclick={()=>refreshAccount(account.account)}><RotateCw size={15} class={refreshingAccount===account.account?'refresh-spin':''}/></button><button class="account-delete" disabled={busy||refreshing} aria-label={`${$t('seeds.deleteAccount')} ${account.account}`} title={$t('seeds.deleteAccount')} onclick={()=>ask({action:'seed_delete_account',account:account.account},'seeds.deleteAccount',account.total)}><Trash2 size={15}/></button></header>
          <div class="account-summary"><div class="account-total"><div class="account-ratio"><strong>{account.valid}</strong><span>/ {account.total}</span></div>
          <p class="ratio-label" title={$t('seeds.validHelp')}>{$t('seeds.validTotal')}</p>
          </div><dl class="account-stats">
            <div><dt>{$t('seeds.ready')}</dt><dd>{account.counts.ready??0}</dd></div>
            <div><dt>{$t('seeds.assigned')}</dt><dd>{(account.counts.assigned??0)+(account.counts.working??0)}</dd></div>
            <div><dt>{$t('seeds.preparing')}</dt><dd>{account.counts.preparing??0}</dd></div>
            <div><dt>{$t('seeds.offline')}</dt><dd>{account.counts.offline??0}</dd></div>
            <div><dt>{$t('seeds.retired')}</dt><dd>{account.counts.retired??0}</dd></div>
          </dl></div>

        </article>
      {:else}<div class="empty">{$t('seeds.noMatches')}</div>{/each}
    </div>
  {:else}

  <div class="toolbar selection-bar"><label><input type="checkbox" disabled={busy||refreshing} checked={inventory.seeds.length>0&&selected.length===inventory.seeds.length} onchange={e=>selected=e.currentTarget.checked?inventory.seeds.map(seed=>seed.id):[]}/>{$t('seeds.selectPage')}</label><span class="muted">{$t('seeds.selected',{count:selected.length})}</span>
    {#if selected.length}<button disabled={busy||refreshing||unsettled} onclick={()=>ask({action:'seed_retire',seed_ids:selected},'seeds.retire',selected.length)}>{$t('seeds.retire')}</button><button disabled={busy||refreshing||unsettled||selectedRows.some(seed=>!seed.retired_at)} onclick={()=>ask({action:'seed_purge',seed_ids:selected},'seeds.purge',selected.length)}>{$t('seeds.purge')}</button>{/if}
  </div>
  {#if selected.length}<form class="batch-labels" onsubmit={e=>{e.preventDefault();void action({action:'seed_label',seed_ids:selected,source:batchSource,prefix:batchPrefix});}}><label>{$t('seeds.source')}<input bind:value={batchSource} maxlength="200" placeholder={$t('seeds.keepSource')}/></label><label>{$t('seeds.prefix')}<input bind:value={batchPrefix} maxlength="200"/></label><button disabled={busy||refreshing}>{$t('seeds.batchLabel')}</button><span class="muted">{$t('seeds.batchLabelHelp')}</span></form>{/if}
  {#if unsettled}<p class="notice">{$t('seeds.unresolvedHelp')}</p>{/if}
  <div class="seed-list">{#each inventory.seeds as seed(seed.id)}
    <details class="seed-row" animate:flip={{duration:motionMs}} transition:slide={{duration:motionMs}}>
      <summary title={details(seed)}><input type="checkbox" disabled={busy||refreshing} aria-label={`${$t('seeds.select')} ${seed.id}`} checked={selected.includes(seed.id)} onclick={e=>e.stopPropagation()} onchange={e=>toggle(seed.id,e.currentTarget.checked)}/><span class="dot" class:alive={seed.status==='ready'||seed.status==='assigned'||seed.status==='working'}></span><strong>{seed.account}</strong><span class="identity">{seed.source} · {seed.prefix||seed.metadata?.repo_name||seed.repo_id||$t('seeds.unlabelled')}</span><code>{seed.id.slice(0,8)}</code><span class="badge">{$t(`seeds.${seed.status}` as MessageKey)}</span><button class="seed-edit" disabled={busy||refreshing} aria-label={`${$t('shell.edit')} ${seed.id}`} title={$t('shell.edit')} onclick={editSeed}><Pencil size={14}/><span>{$t('shell.edit')}</span></button></summary>
      <div class="row-details">
        <dl class="row-facts">
          {#if seed.reason}<div class="wide"><dt>{$t('seeds.stateReason')}</dt><dd>{$t(`seeds.reason.${seed.reason}` as MessageKey)}</dd></div>{/if}
          <div><dt>{$t('seeds.lastSeen')}</dt><dd>{date(seed.last_seen)}</dd></div>
          {#if seed.unresolved_operations>0}<div><dt>{$t('seeds.unresolved')}</dt><dd>{seed.unresolved_operations}</dd></div>{/if}
        </dl>
        <form class="row-labels" onsubmit={e=>{e.preventDefault();const fields=new FormData(e.currentTarget);void action({action:'seed_label',seed_id:seed.id,source:fields.get('source'),prefix:fields.get('prefix')});}}>
          <label>{$t('seeds.source')}<input name="source" required value={seed.source} maxlength="200"/></label>
          <label>{$t('seeds.prefix')}<input name="prefix" value={seed.prefix} maxlength="200"/></label>
          <button disabled={busy||refreshing}>{$t('seeds.saveLabel')}</button>
        </form><div class="toolbar">
        {#if seed.chat_id}<a href={appUrl(`/workspace/${encodeURIComponent(workspaceId)}?folder=${encodeURIComponent(folderId)}&chat=${encodeURIComponent(seed.chat_id)}`)}>{$t('seeds.conversation')}</a>{/if}
        {#if seed.host_task_id}<a href={`https://app.coderabbit.ai/code/tasks/${encodeURIComponent(seed.host_task_id)}`} target="_blank" rel="noreferrer">{$t('seeds.hostTask')}</a>{/if}
        {#if !seed.retired_at}<button disabled={busy||refreshing||seed.unresolved_operations>0} onclick={()=>ask({action:'seed_retire',seed_id:seed.id},'seeds.retire')}>{$t('seeds.retire')}</button>{/if}
        {#if seed.unresolved_operations>0}<span class="muted">{$t('seeds.unresolvedHelp')}</span>{/if}
        {#if seed.retired_at}<button disabled={busy||refreshing||seed.unresolved_operations>0} onclick={()=>ask({action:'seed_purge',seed_id:seed.id},'seeds.purge')}>{$t('seeds.purge')}</button>{/if}
        {#if !seed.retired_at&&seed.chat_id}<button disabled={busy||refreshing||seed.unresolved_operations>0} onclick={()=>ask({action:'seed_release',seed_id:seed.id},'seeds.release')}>{$t('seeds.release')}</button>{/if}
        {#if !seed.retired_at&&!seed.redeemed_at&&seed.enrollment_until&&seed.enrollment_until<updatedAt}<button disabled={busy||refreshing||seed.unresolved_operations>0} onclick={()=>void reissue(seed)}>{$t('seeds.reissue')}</button>{/if}
      </div>
        <details class="row-more"><summary>{$t('seeds.moreInfo')}</summary>
          <dl class="row-facts">{#each advanced(seed) as [label,value]}<div><dt>{label}</dt><dd>{value}</dd></div>{/each}</dl>
        </details>
      </div>
    </details>
  {:else}<div class="empty">{$t('seeds.empty')}</div>{/each}</div>
  <div class="toolbar pagination"><button disabled={page===0} onclick={()=>page--}>{$t('seeds.previous')}</button><span>{page+1} / {pageCount}</span><button disabled={page+1>=pageCount} onclick={()=>page++}>{$t('seeds.next')}</button></div>
  {/if}
</div>
{#if confirmation}<dialog class="add-dialog" use:openModal onclose={()=>confirmation=null} aria-label={$t(confirmation.label)}><h2>{$t(confirmation.label)}{#if confirmation.count>0} · {confirmation.count}{/if}</h2><p class="notice">{$t(confirmation.args.action==='seed_delete_account'?'seeds.deleteAccountConfirm':confirmation.args.action==='seed_purge'?'seeds.purgeConfirm':confirmation.args.action==='seed_release'?'seeds.releaseConfirm':'seeds.retireConfirm',{account:String(confirmation.args.account??'')})}</p><div class="toolbar confirm-actions"><button onclick={()=>confirmation=null}>{$t('Cancel')}</button><button class={confirmation.args.action==='seed_delete_account'||confirmation.args.action==='seed_purge'?'danger':'primary'} disabled={busy||refreshing} onclick={()=>{const args=confirmation?.args;confirmation=null;if(args)void action(args);}}>{$t(confirmation.args.action==='seed_delete_account'?'seeds.deleteAccount':'seeds.confirm')}</button></div></dialog>{/if}
{#if reissuedPrompt}<dialog class="add-dialog" use:openModal onclose={()=>reissuedPrompt=''} aria-label={$t('seeds.reissue')}><header class="library-header dialog-head"><h2>{$t('seeds.reissue')}</h2><button class="dialog-close" aria-label={$t('Close')} title={$t('Close')} onclick={()=>reissuedPrompt=''}><X size={18}/></button></header><p class="notice">{$t('seeds.reissueHelp')}</p><textarea readonly value={reissuedPrompt} rows="7" aria-label={$t('seeds.manualPrompt')}></textarea><div class="toolbar"><button class="primary" onclick={()=>copy(reissuedPrompt)}>{$t('seeds.copyPrompt')}</button></div></dialog>{/if}
<style>
.account-manage-icon{display:inline-flex;align-items:center;justify-content:center;padding:7px;flex:none}.account-summary{margin-bottom:0!important}.library-header :global(select){max-width:190px;padding:7px;font-size:12px}.library-header{margin-bottom:6px}.library-header h2{margin-right:0!important}.library-header :global(select){margin-right:auto}.account-card header{gap:7px!important}
@media(max-width:700px){.library-actions>.muted{flex-basis:100%}.library-header h2{display:none}.library-header .muted{display:none}.library-header :global(select){max-width:140px}.account-manage-icon,.account-refresh{min-width:36px;min-height:36px}}
.library-actions{justify-content:flex-end}.library-actions>.muted{margin-right:auto}
.overview>button:nth-child(1){background:color-mix(in srgb,#73c599 14%,var(--card-bg));border-color:#73c59955}
.overview>button:nth-child(2){background:color-mix(in srgb,#79a7e5 14%,var(--card-bg));border-color:#79a7e555}
.overview>button:nth-child(3){background:color-mix(in srgb,#e6c178 14%,var(--card-bg));border-color:#e6c17855}
.overview>button:nth-child(4){background:color-mix(in srgb,#b7a0d8 14%,var(--card-bg));border-color:#b7a0d855}
.overview>button:nth-child(5){background:color-mix(in srgb,#d49494 14%,var(--card-bg));border-color:#d4949455}
.account-grid .account-card{padding:16px}.account-summary{display:flex;align-items:center;gap:18px;margin:16px 0}.account-total{flex:none}.account-summary .account-ratio{margin:0;gap:4px}.account-summary .account-ratio strong{font-size:30px}.account-summary .account-ratio span{font-size:17px}.account-summary .ratio-label{font-size:10px;margin:4px 0 0}.account-summary .account-stats{flex:1;gap:10px 9px;border:0;border-left:1px solid var(--color-border);padding:0 0 0 14px;margin:0}.account-summary .account-stats dt{font-size:10px}.account-summary .account-stats dd{font-size:13px;margin:2px 0 0}.account-grid{grid-template-columns:repeat(auto-fill,minmax(min(100%,300px),1fr))!important}

.account-refresh{display:inline-flex;align-items:center;justify-content:center;padding:7px;background:color-mix(in srgb,hsl(var(--account-hue) 65% 65%) 14%,var(--card-bg));flex-shrink:0}
.back-accounts{display:inline-flex;gap:7px;align-items:center;background:#2563eb;color:#fff;border-color:#3b82f6;box-shadow:0 2px 8px #2563eb26}
.seed-row .seed-edit{display:inline-flex;gap:5px;align-items:center;padding:5px 8px;background:var(--surface-1);font-size:12px;opacity:0;pointer-events:none;transition:opacity 120ms}
.seed-row:hover .seed-edit,.seed-row:focus-within .seed-edit,.seed-row[open] .seed-edit{opacity:1;pointer-events:auto}
:global(.refresh-spin){animation:seed-refresh-spin 900ms linear infinite}
@keyframes seed-refresh-spin{to{transform:rotate(360deg)}}
@media(pointer:coarse),(max-width:700px){.seed-row .seed-edit{opacity:1;pointer-events:auto}}
@media(max-width:700px){.seed-edit span{display:none}}
@media(prefers-reduced-motion:reduce){.seed-edit{transition:none}:global(.refresh-spin){animation:none}}

.skeleton{display:grid;gap:14px}.skeleton span{display:block;height:44px;border-radius:8px;background:var(--color-border)}

.selection-bar{margin:14px 0}.batch-labels{display:flex;align-items:end;flex-wrap:wrap;margin:12px 0}.add-dialog textarea{width:100%;padding:12px;background:var(--card-bg);border:1px solid var(--color-border);border-radius:8px;font-size:12px}

.library-header{display:flex;gap:10px;align-items:center;flex-wrap:wrap}.library-header h2{margin:0 auto 0 0;font-size:17px}.library-header label{display:flex;gap:6px;align-items:center;font-size:12px}.add-dialog{margin:auto;width:min(740px,calc(100vw - 32px));max-height:85dvh;overflow:auto;padding:24px;border:1px solid var(--color-border);border-radius:16px;background:var(--surface-1);color:var(--color-text);box-shadow:var(--dialog-shadow)}.add-dialog::backdrop{background:var(--backdrop)}

.account-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(min(100%,270px),1fr));gap:14px}.account-card{border:1px solid color-mix(in srgb,hsl(var(--account-hue) 65% 65%) 30%,var(--color-border));border-radius:12px;background:color-mix(in srgb,hsl(var(--account-hue) 65% 65%) 9%,var(--card-bg));padding:20px;min-width:0}.account-card header{display:flex;align-items:center;gap:9px}.account-card h2,.account-heading h2{font-size:14px;margin:0;overflow-wrap:anywhere}.account-card h2{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.account-ratio{display:flex;align-items:baseline;gap:8px;margin-top:22px;font-variant-numeric:tabular-nums}.account-ratio strong{font-size:34px;line-height:1.2}.account-ratio span{font-size:20px;color:var(--color-text-muted)}.ratio-label{font-size:12px;color:var(--color-text-muted);margin:6px 0 18px}.account-stats{display:grid;grid-template-columns:repeat(3,1fr);gap:14px 10px;border-top:1px solid var(--color-border);padding-top:16px;margin:0 0 18px}.account-stats dt{font-size:11px;color:var(--color-text-muted)}.account-stats dd{font-size:15px;font-weight:600;margin:4px 0 0;font-variant-numeric:tabular-nums}.account-heading{margin:18px 0 12px}button:focus-visible{outline:2px solid #6366f1;outline-offset:3px}
.seed-library{max-width:1180px;margin:auto}.overview{display:flex;flex-wrap:nowrap;overflow-x:auto;gap:8px;margin:16px 0;padding-bottom:4px}.overview>button{flex:1;min-width:88px;text-align:left}.overview>button.active{border-color:var(--primary)}.overview>button{border:1px solid var(--color-border);border-radius:10px;padding:10px;display:grid;gap:4px;background:var(--card-bg)}.overview span,.muted,.notice,.identity{font-size:12px;color:var(--color-text-muted);line-height:1.7}.overview strong{font-size:21px}.toolbar,.input-row{display:flex;gap:10px;align-items:center;flex-wrap:wrap}.toolbar label{display:flex;align-items:center;gap:6px}.notice{margin:12px 0}.pairing{border:1px solid var(--color-border);padding:16px;border-radius:12px;margin:18px 0}summary{cursor:pointer}form{display:grid;gap:14px;margin-top:16px}form label{display:grid;gap:6px;font-size:12px}.input-row input{flex:1;min-width:160px}input:not([type=checkbox]),select{padding:9px 10px;border:1px solid var(--color-border);border-radius:8px;background:var(--card-bg);min-width:0}button{padding:8px 12px;border:1px solid var(--border-strong);border-radius:8px;cursor:pointer;white-space:nowrap;background:var(--surface-3);color:var(--color-text)}button:not(:disabled):hover{border-color:color-mix(in srgb,var(--action) 55%,var(--border-strong))}button:disabled{opacity:.5;cursor:default}.primary{background:var(--action)!important;border-color:var(--action-border)!important;color:var(--action-text)!important;font-weight:600}.primary:not(:disabled):hover{background:var(--action-hover)!important}.danger{background:var(--danger);border-color:var(--danger);color:#fff;font-weight:600}.filters{margin:18px 0}.search{flex:1;min-width:180px!important}.list-head{display:flex;justify-content:space-between;margin:8px 0}.seed-list{border:1px solid var(--color-border);border-radius:12px;overflow:hidden}.seed-row+ .seed-row{border-top:1px solid var(--color-border)}.seed-row summary{display:flex;align-items:center;gap:12px;padding:13px 16px;background:var(--card-bg)}.seed-row summary:hover{background:var(--color-border)}.seed-row strong{font-size:13px;max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.identity{flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dot{width:7px;height:7px;border-radius:50%;background:#94a3b8;flex-shrink:0}.dot.alive{background:#22c55e}.badge{font-size:11px;border-radius:12px;padding:3px 8px;background:var(--color-border);white-space:nowrap}code{font-size:11px;color:var(--color-text-muted)}.row-labels{display:flex;align-items:end;flex-wrap:wrap;margin-bottom:14px}.row-details{padding:0 16px 16px}.empty{padding:36px;color:var(--color-text-muted);text-align:center}.pagination{justify-content:center;margin:18px 0;font-size:12px}[role=alert]{color:var(--danger);overflow-wrap:anywhere}a{color:#3b82f6;cursor:pointer;font-size:12px}@media(max-width:700px){.overview strong{font-size:18px}.seed-row summary{gap:7px;padding:12px 8px}.seed-row code{display:none}.seed-row strong{max-width:110px}.input-row button{padding:8px}.identity{font-size:11px}}
.add-intro{margin:6px 0 14px;font-size:12px;color:var(--color-text-muted)}.method-cards{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:10px;margin-bottom:16px}.method-cards>button{display:flex;gap:12px;align-items:flex-start;text-align:left;white-space:normal;padding:14px;border-radius:12px;background:var(--card-bg);transition:border-color .12s,box-shadow .12s}.method-cards>button:hover{border-color:color-mix(in srgb,var(--action) 50%,var(--color-border))}.method-cards>button.active{border-color:var(--action);box-shadow:0 0 0 1px var(--action) inset}.method-icon{display:grid;place-items:center;width:36px;height:36px;border-radius:10px;flex-shrink:0;background:color-mix(in srgb,var(--action) 14%,transparent);color:var(--action)}.method-text{display:grid;gap:4px;min-width:0}.method-text strong{font-size:13px;display:flex;gap:8px;align-items:center;flex-wrap:wrap}.method-text em{font-style:normal;font-size:10px;font-weight:600;padding:2px 7px;border-radius:999px;background:color-mix(in srgb,#16a34a 16%,transparent);color:#16a34a}.method-text small{font-size:12px;line-height:1.6;color:var(--color-text-muted)}.pair-state{font-size:11px;font-weight:400;color:var(--color-text-muted)}.pair-state.connected{color:#16a34a}.method-panel .pairing{margin:0}@media(max-width:700px){.method-cards{grid-template-columns:1fr}}
.dialog-head{margin-bottom:12px}.dialog-head h2{display:block!important;margin-right:auto!important}.dialog-close{display:inline-flex;align-items:center;justify-content:center;width:34px;height:34px;padding:0;margin-left:auto;border-radius:999px;background:transparent;border-color:transparent;color:var(--color-text-muted)}.dialog-close:hover{background:var(--surface-hover);color:var(--color-text);border-color:var(--border-strong)}
.account-delete{display:inline-flex;align-items:center;justify-content:center;padding:7px;flex:none;color:var(--color-text-muted)}.account-delete:not(:disabled):hover{color:var(--danger);border-color:color-mix(in srgb,var(--danger) 60%,transparent)}
.confirm-actions{justify-content:flex-end;margin-top:16px}
@media(max-width:700px){.account-delete{min-width:36px;min-height:36px}}
.row-facts{display:grid;grid-template-columns:repeat(auto-fill,minmax(180px,1fr));gap:8px 20px;margin:0 0 12px}.row-facts div{min-width:0}.row-facts .wide{grid-column:1/-1}.row-facts dt{font-size:11px;color:var(--color-text-muted);margin-bottom:2px}.row-facts dd{margin:0;font-size:13px;overflow-wrap:anywhere}
.row-more{margin-top:12px;border-top:1px solid var(--color-border);padding-top:10px}.row-more>summary{cursor:pointer;font-size:12px;color:var(--color-text-muted);width:max-content;list-style:none;display:inline-flex;gap:6px;align-items:center}.row-more>summary::-webkit-details-marker{display:none}.row-more>summary::before{content:'›';display:inline-block;transition:transform 120ms}.row-more[open]>summary::before{transform:rotate(90deg)}.row-more[open]>summary{margin-bottom:10px}.row-more .row-facts{margin:0}
.install-link{display:inline-block;margin-top:10px;font-size:12px}
.install-card{border-style:dashed!important;opacity:.85}
</style>
