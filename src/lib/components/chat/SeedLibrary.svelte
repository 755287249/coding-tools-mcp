<script lang="ts">
  import {seedEnrollmentPrompt} from '$lib/chat/seed-enrollment';
  import {showToast} from '$lib/stores/toast';
  import ManualSeed from './ManualSeed.svelte';
  import {seedRequest,type SeedInventory,type SeedRow,type SeedTicket} from '$lib/api/seeds';
  import {copyText} from '$lib/browser-tools';
  import {t} from '$lib/i18n';
  import type {MessageKey} from '$lib/i18n/catalog';
  import {workspaces} from '$lib/stores/app';
  import {APP_VERSION} from '$lib/app-version';
  import {appUrl} from '$lib/app-path';
  import {untrack,onMount} from 'svelte';
  import {flip} from 'svelte/animate';
  import {slide} from 'svelte/transition';
  let motionMs=$state(0);
  onMount(()=>{const query=matchMedia('(prefers-reduced-motion: reduce)');const update=()=>motionMs=query.matches?0:140;update();query.addEventListener('change',update);return()=>query.removeEventListener('change',update);});
  import {pairingKey,pairingAddress,loadPairing,savePairing,newPairingToken} from '$lib/chat/seed-pairing';
  let {workspaceId,folderId}:{workspaceId:string;folderId:string}=$props();
  const empty=():SeedInventory=>({enabled:false,seeds:[],retired_count:0});
  let inventory=$state<SeedInventory>(empty()),address=$state(''),token=$state('');
  let busy=$state(false),error=$state(''),notice=$state(''),history=$state(false),filter=$state(''),status=$state(''),search=$state(''),page=$state(0),sourceFilter=$state(''),keyword=$state('');
  let pairingOpen=$state(true),showToken=$state(false),view=$state<'accounts'|'seeds'>('accounts'),loading=$state(true);
  let addOpen=$state(false),refreshing=$state(false),updatedAt=$state(0),refreshKey=$state(0),debouncedSearch=$state(''),debouncedKeyword=$state('');
  let moreFilters=$state(false),selected=$state<string[]>([]),batchSource=$state(''),batchPrefix=$state(''),reissuedPrompt=$state('');
  let confirmation=$state<{args:Record<string,unknown>;label:MessageKey;count:number}|null>(null);
  const selectedRows=$derived(inventory.seeds.filter(seed=>selected.includes(seed.id)));
  const unsettled=$derived(selectedRows.some(seed=>seed.unresolved_operations>0));
  function ask(args:Record<string,unknown>,label:MessageKey,count=1){confirmation={args,label,count};}
  function toggle(id:string,checked:boolean){selected=checked?[...selected.filter(item=>item!==id),id]:selected.filter(item=>item!==id);}
  async function reissue(seed:SeedRow){
    if(busy)return;busy=true;error='';const generation=scope;
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
    const saved=loadPairing(key),profile=$workspaces.find(w=>w.id===workspaceId);
    address=saved.address;token=saved.token;
    if(!address&&profile?.tunnel.public_url){try{address=pairingAddress(profile.tunnel.public_url.replace(/\/$/,'').replace(/\/mcp$/,'')+'/mcp',folderId);}catch{}}
    pairingOpen=true;showToken=false;view='accounts';addOpen=false;updatedAt=0;selected=[];confirmation=null;reissuedPrompt='';
    inventory=empty();error='';notice='';history=false;filter='';status='';search='';page=0;sourceFilter='';keyword='';
  });});
  $effect(()=>{const value=search,label=keyword;const timer=setTimeout(()=>{debouncedSearch=value;debouncedKeyword=label;page=0;},300);return()=>clearTimeout(timer);});
  $effect(()=>{const ws=workspaceId,folder=folderId,query={action:'seed_list',limit:50,offset:page*50,account:filter,status,search:debouncedSearch,source:sourceFilter,keyword:debouncedKeyword,include_archived:history};refreshKey;
    const generation=++scope;loading=untrack(()=>updatedAt===0);
    let stopped=false,failures=0,timer:ReturnType<typeof setTimeout>;
    const refresh=async()=>{
      if(stopped)return;
      if(document.hidden){timer=setTimeout(refresh,5000);return;}
      refreshing=true;
      try{const result=await seedRequest(ws,folder,query);if(!stopped&&scope===generation){inventory=result;selected=selected.filter(id=>result.seeds.some(seed=>seed.id===id));updatedAt=Date.now();error='';failures=0;const last=Math.max(0,Math.ceil((result.filtered_total??0)/50)-1);if(page>last)page=last;}}
      catch(e){if(!stopped&&scope===generation){error=e instanceof Error?e.message:String(e);failures++;}}
      finally{if(!stopped&&scope===generation){loading=false;refreshing=false;timer=setTimeout(refresh,Math.min(60000,5000*2**failures));}}
    };
    void refresh();return()=>{stopped=true;clearTimeout(timer);};
  });
  function query(){return {limit:50,offset:page*50,account:filter,status,search:debouncedSearch,source:sourceFilter,keyword:debouncedKeyword,include_archived:history};}
  async function action(args:Record<string,unknown>){if(busy)return;busy=true;error='';const generation=scope;try{const result=await seedRequest(workspaceId,folderId,{...query(),...args});if(generation===scope){notice=result.affected_count===undefined?$t('seeds.actionSaved'):$t('seeds.affected',{count:result.affected_count});selected=[];showToast(notice,{kind:'success'});refreshKey++;}}catch(e){if(generation===scope){error=e instanceof Error?e.message:String(e);showToast(error,{kind:'error'});}}finally{busy=false;}}
  async function connect(){if(busy)return;busy=true;error='';notice='';const generation=scope,ws=workspaceId,folder=folderId;try{
    const saved={address:pairingAddress(address,folder),token:token.trim()};
    if(!/^[a-f0-9]{64}$/.test(saved.token))throw Error($t('seeds.tokenInvalid'));
    const result=await seedRequest(ws,folder,{...query(),action:'seed_pairing',token:saved.token});
    const stored=savePairing(pairingKey(ws,folder),saved);
    if(generation===scope){address=saved.address;token=saved.token;inventory=result;notice=$t(stored?'seeds.pairSaved':'seeds.pairStorage');}
  }catch(e){if(generation===scope)error=String(e);}finally{busy=false;}}
  async function copy(value:string){try{await copyText(value);notice=$t('seeds.copied');showToast(notice,{kind:'success'});}catch(e){error=String(e);showToast(error,{kind:'error'});}}
  const date=(value:number)=>value?new Date(value).toLocaleString():$t('seeds.never');
  function details(seed:SeedRow){return [seed.account,`${$t('seeds.source')}: ${seed.source}`,`${$t('seeds.prefix')}: ${seed.prefix||'—'}`,seed.metadata?.repo_name||seed.repo_id,seed.branch,`ID: ${seed.id}`,...Object.entries(seed.metadata??{}).filter(([,v])=>v).map(([k,v])=>`${k}: ${v}`),`${$t('seeds.created')}: ${date(seed.created_at)}`,`${$t('seeds.lastSeen')}: ${date(seed.last_seen)}`,`${$t('seeds.hostTask')}: ${seed.host_task_id||'—'}`,`${$t('seeds.conversation')}: ${seed.chat_id||'—'}`,`${$t('seeds.generation')}: ${seed.generation}`,`${$t('seeds.unresolved')}: ${seed.unresolved_operations}`,seed.reason?$t(`seeds.reason.${seed.reason}` as MessageKey):''].filter(Boolean).join('\n');}
</script>
<div class="seed-library" aria-busy={busy||refreshing}>
  <header class="library-header"><h2>{$t('seeds.library')}</h2><button disabled={refreshing||busy} onclick={()=>refreshKey++}>{$t(refreshing?'seeds.refreshing':'seeds.refresh')}</button><span class="muted">{$t('seeds.updated')}: {date(updatedAt)}</span><label><input type="checkbox" checked={inventory.enabled} disabled={busy||loading} onchange={e=>action({action:'seed_settings',enabled:e.currentTarget.checked})}/>{$t('seeds.auto')}</label><button class="primary" onclick={()=>addOpen=true}>{$t('seeds.addSeeds')}</button></header>
  <div class="overview" aria-label={$t('seeds.globalSummary')}>
    {#each ['ready','assigned','preparing','offline','retired'] as state}<button class:active={status===state} onclick={()=>{status=status===state?'':state;view='seeds';page=0;}}><span>{$t(`seeds.${state}` as MessageKey)}</span><strong>{counts[state]??0}</strong></button>{/each}
  </div>
  <div class="toolbar"><button disabled={busy||loading} onclick={()=>ask({action:'seed_cleanup'},'seeds.cleanup',0)}>{$t('seeds.cleanup')}</button><span class="muted">{$t('seeds.globalSummary')} · {$t('seeds.total')}: {inventory.total??0} / {inventory.capacity??10000}</span></div>
  {#if busy}<p class="notice" role="status">{$t('Working…')}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  {#if addOpen}<dialog class="add-dialog" use:openModal onclose={()=>addOpen=false} aria-label={$t('seeds.addSeeds')}><header class="library-header"><h2>{$t('seeds.addSeeds')}</h2><button onclick={()=>addOpen=false}>{$t('Close')}</button></header>
  <details class="pairing" bind:open={pairingOpen}>
    <summary>{$t('seeds.browserPair')} <span class:connected={inventory.pairing_enabled}>{$t(inventory.pairing_enabled?'seeds.pairEnabled':'seeds.pairDisabled')}</span></summary>
    <p class="notice">{$t('seeds.pairHelp')}</p>
    <form onsubmit={e=>{e.preventDefault();void connect();}}>
      <label>{$t('seeds.pairAddress')}<div class="input-row"><input bind:value={address} type="url" required placeholder="https://example.com/mcp"/><button type="button" onclick={()=>copy(address)}>{$t('seeds.copy')}</button></div></label>
      <label>{$t('seeds.pairToken')}<div class="input-row"><input bind:value={token} type={showToken?'text':'password'} autocomplete="off" maxlength="64" required/><button type="button" aria-pressed={showToken} onclick={()=>showToken=!showToken}>{$t(showToken?'sharing.hidePassword':'sharing.showPassword')}</button><button type="button" onclick={()=>copy(token)}>{$t('seeds.copy')}</button><button type="button" disabled={busy} onclick={()=>{token=newPairingToken();notice=$t('seeds.tokenUnsaved');}}>{$t('seeds.generateToken')}</button></div></label>
      <div class="toolbar"><button class="primary" disabled={busy}>{$t('seeds.savePair')}</button><button type="button" disabled={busy||!inventory.pairing_enabled} onclick={()=>action({action:'seed_pairing',enabled:false})}>{$t('seeds.revokePair')}</button><a href={`https://github.com/755287249/coding-tools-mcp/releases/download/v${APP_VERSION}/coderabbit-seeds.user.js`} target="_blank" rel="noreferrer">{$t('seeds.install')}</a></div>
    </form>
  </details>
  <ManualSeed {workspaceId} {folderId} {address} onCreated={()=>refreshKey++}/>
  </dialog>{/if}
  <div class="toolbar tabs" aria-label={$t('seeds.view')}><button aria-pressed={view==='accounts'} onclick={()=>{view='accounts';filter='';page=0;}}>{$t('seeds.byAccount')}</button><button aria-pressed={view==='seeds'} onclick={()=>{view='seeds';filter='';page=0;}}>{$t('seeds.allSeeds')}</button></div>
  <div class="toolbar filters">
    <input class="search" type="search" aria-label={$t('seeds.search')} placeholder={$t('seeds.search')} bind:value={search}/>
    <select aria-label={$t('seeds.source')} bind:value={sourceFilter} onchange={()=>page=0}><option value="">{$t('seeds.allSources')}</option>{#each inventory.sources??[] as source}<option value={source}>{source}</option>{/each}</select>

    <select aria-label={$t('seeds.account')} bind:value={filter} onchange={()=>{page=0;view='seeds';}}><option value="">{$t('seeds.allAccounts')}</option>{#each accounts as a}<option value={a}>{a}</option>{/each}</select>
    <select aria-label={$t('seeds.allStates')} bind:value={status} onchange={()=>page=0}><option value="">{$t('seeds.allStates')}</option>{#each ['ready','assigned','working','preparing','offline','retired'] as state}<option value={state}>{$t(`seeds.${state}` as MessageKey)}</option>{/each}</select>
    <button aria-expanded={moreFilters} onclick={()=>moreFilters=!moreFilters}>{$t('seeds.morePools')}</button>
    {#if moreFilters}<input aria-label={$t('seeds.keyword')} placeholder={$t('seeds.keyword')} bind:value={keyword}/><label><input type="checkbox" bind:checked={history} onchange={()=>page=0}/>{$t('seeds.history')}</label>{/if}
  </div>
  {#if filter}
    <div class="toolbar account-heading"><button onclick={()=>{filter='';view='accounts';page=0;}}>{$t('seeds.backAccounts')}</button><h2>{filter}</h2></div>
  {/if}
  <div class="list-head muted"><span>{$t(view==='seeds'?'seeds.hoverDetails':'seeds.accountSummaryHelp')}</span><span>{inventory.filtered_total??0} {$t('seeds.items')}</span></div>
  {#if loading}
    <div class="empty skeleton" role="status" aria-label={$t('seeds.inventoryLoading')}><span></span><span></span><span></span></div>
  {:else if view==='accounts'}
    <div class="account-grid">
      {#each inventory.account_summaries??[] as account(account.account)}
        <article class="account-card" animate:flip={{duration:motionMs}} transition:slide={{duration:motionMs}}>
          <header><span class="dot" class:alive={account.valid>0}></span><h2 title={account.account}>{account.account}</h2></header>
          <div class="account-ratio"><strong>{account.valid}</strong><span>/ {account.total}</span></div>
          <p class="ratio-label" title={$t('seeds.validHelp')}>{$t('seeds.validTotal')}</p>
          <dl class="account-stats">
            <div><dt>{$t('seeds.ready')}</dt><dd>{account.counts.ready??0}</dd></div>
            <div><dt>{$t('seeds.assigned')}</dt><dd>{(account.counts.assigned??0)+(account.counts.working??0)}</dd></div>
            <div><dt>{$t('seeds.preparing')}</dt><dd>{account.counts.preparing??0}</dd></div>
            <div><dt>{$t('seeds.offline')}</dt><dd>{account.counts.offline??0}</dd></div>
            <div><dt>{$t('seeds.retired')}</dt><dd>{account.counts.retired??0}</dd></div>
          </dl>
          <button class="account-manage" onclick={()=>{filter=account.account;view='seeds';page=0;}}>{$t('seeds.manageAccount')}</button>
        </article>
      {:else}<div class="empty">{$t('seeds.noMatches')}</div>{/each}
    </div>
  {:else}

  <div class="toolbar selection-bar"><label><input type="checkbox" checked={inventory.seeds.length>0&&selected.length===inventory.seeds.length} onchange={e=>selected=e.currentTarget.checked?inventory.seeds.map(seed=>seed.id):[]}/>{$t('seeds.selectPage')}</label><span class="muted">{$t('seeds.selected',{count:selected.length})}</span>
    {#if selected.length}<button disabled={busy||unsettled} onclick={()=>ask({action:'seed_retire',seed_ids:selected},'seeds.retire',selected.length)}>{$t('seeds.retire')}</button><button disabled={busy||unsettled||selectedRows.some(seed=>!seed.retired_at)} onclick={()=>ask({action:'seed_purge',seed_ids:selected},'seeds.purge',selected.length)}>{$t('seeds.purge')}</button>{/if}
  </div>
  {#if selected.length}<form class="batch-labels" onsubmit={e=>{e.preventDefault();void action({action:'seed_label',seed_ids:selected,source:batchSource,prefix:batchPrefix});}}><label>{$t('seeds.source')}<input bind:value={batchSource} maxlength="200" placeholder={$t('seeds.keepSource')}/></label><label>{$t('seeds.prefix')}<input bind:value={batchPrefix} maxlength="200"/></label><button disabled={busy}>{$t('seeds.batchLabel')}</button><span class="muted">{$t('seeds.batchLabelHelp')}</span></form>{/if}
  {#if unsettled}<p class="notice">{$t('seeds.unresolvedHelp')}</p>{/if}
  <div class="seed-list">{#each inventory.seeds as seed(seed.id)}
    <details class="seed-row" animate:flip={{duration:motionMs}} transition:slide={{duration:motionMs}}>
      <summary title={details(seed)}><input type="checkbox" aria-label={`${$t('seeds.select')} ${seed.id}`} checked={selected.includes(seed.id)} onclick={e=>e.stopPropagation()} onchange={e=>toggle(seed.id,e.currentTarget.checked)}/><span class="dot" class:alive={seed.status==='ready'||seed.status==='assigned'||seed.status==='working'}></span><strong>{seed.account}</strong><span class="identity">{seed.source} · {seed.prefix||seed.metadata?.repo_name||seed.repo_id||$t('seeds.unlabelled')}</span><code>{seed.id.slice(0,8)}</code><span class="badge">{$t(`seeds.${seed.status}` as MessageKey)}</span></summary>
      <div class="row-details"><pre>{details(seed)}</pre>
        <form class="row-labels" onsubmit={e=>{e.preventDefault();const fields=new FormData(e.currentTarget);void action({action:'seed_label',seed_id:seed.id,source:fields.get('source'),prefix:fields.get('prefix')});}}>
          <label>{$t('seeds.source')}<input name="source" required value={seed.source} maxlength="200"/></label>
          <label>{$t('seeds.prefix')}<input name="prefix" value={seed.prefix} maxlength="200"/></label>
          <button disabled={busy}>{$t('seeds.saveLabel')}</button>
        </form><div class="toolbar">
        {#if seed.chat_id}<a href={appUrl(`/workspace/${encodeURIComponent(workspaceId)}?folder=${encodeURIComponent(folderId)}&chat=${encodeURIComponent(seed.chat_id)}`)}>{$t('seeds.conversation')}</a>{/if}
        {#if seed.host_task_id}<a href={`https://app.coderabbit.ai/code/tasks/${encodeURIComponent(seed.host_task_id)}`} target="_blank" rel="noreferrer">{$t('seeds.hostTask')}</a>{/if}
        {#if !seed.retired_at}<button disabled={busy||seed.unresolved_operations>0} onclick={()=>ask({action:'seed_retire',seed_id:seed.id},'seeds.retire')}>{$t('seeds.retire')}</button>{/if}
        {#if seed.unresolved_operations>0}<span class="muted">{$t('seeds.unresolvedHelp')}</span>{/if}
        {#if seed.retired_at}<button disabled={busy||seed.unresolved_operations>0} onclick={()=>ask({action:'seed_purge',seed_id:seed.id},'seeds.purge')}>{$t('seeds.purge')}</button>{/if}
        {#if !seed.retired_at&&seed.chat_id}<button disabled={busy||seed.unresolved_operations>0} onclick={()=>ask({action:'seed_release',seed_id:seed.id},'seeds.release')}>{$t('seeds.release')}</button>{/if}
        {#if !seed.retired_at&&!seed.redeemed_at&&seed.enrollment_until&&seed.enrollment_until<updatedAt}<button disabled={busy||seed.unresolved_operations>0} onclick={()=>void reissue(seed)}>{$t('seeds.reissue')}</button>{/if}
      </div></div>
    </details>
  {:else}<div class="empty">{$t('seeds.empty')}</div>{/each}</div>
  <div class="toolbar pagination"><button disabled={page===0} onclick={()=>page--}>{$t('seeds.previous')}</button><span>{page+1} / {pageCount}</span><button disabled={page+1>=pageCount} onclick={()=>page++}>{$t('seeds.next')}</button></div>
  {/if}
</div>
{#if confirmation}<dialog class="add-dialog" use:openModal onclose={()=>confirmation=null} aria-label={$t(confirmation.label)}><h2>{$t(confirmation.label)}{#if confirmation.count>0} · {confirmation.count}{/if}</h2><p class="notice">{$t(confirmation.args.action==='seed_purge'?'seeds.purgeConfirm':confirmation.args.action==='seed_release'?'seeds.releaseConfirm':'seeds.retireConfirm')}</p><div class="toolbar"><button onclick={()=>confirmation=null}>{$t('Cancel')}</button><button class="primary" disabled={busy} onclick={()=>{const args=confirmation?.args;confirmation=null;if(args)void action(args);}}>{$t('seeds.confirm')}</button></div></dialog>{/if}
{#if reissuedPrompt}<dialog class="add-dialog" use:openModal onclose={()=>reissuedPrompt=''} aria-label={$t('seeds.reissue')}><h2>{$t('seeds.reissue')}</h2><p class="notice">{$t('seeds.reissueHelp')}</p><textarea readonly value={reissuedPrompt} rows="7" aria-label={$t('seeds.manualPrompt')}></textarea><div class="toolbar"><button onclick={()=>copy(reissuedPrompt)}>{$t('seeds.copyPrompt')}</button><button onclick={()=>reissuedPrompt=''}>{$t('Close')}</button></div></dialog>{/if}
<style>
.skeleton{display:grid;gap:14px}.skeleton span{display:block;height:44px;border-radius:8px;background:var(--color-border)}

.selection-bar{margin:14px 0}.batch-labels{display:flex;align-items:end;flex-wrap:wrap;margin:12px 0}.add-dialog textarea{width:100%;padding:12px;background:var(--card-bg);border:1px solid var(--color-border);border-radius:8px;font-size:12px}

.library-header{display:flex;gap:10px;align-items:center;flex-wrap:wrap}.library-header h2{margin:0 auto 0 0;font-size:17px}.library-header label{display:flex;gap:6px;align-items:center;font-size:12px}.tabs{margin-top:18px}.tabs [aria-pressed=true]{border-color:var(--primary);color:var(--primary)}.add-dialog{margin:auto;width:min(740px,calc(100vw - 32px));max-height:85dvh;overflow:auto;padding:24px;border:1px solid var(--color-border);border-radius:16px;background:var(--surface-1);color:var(--color-text);box-shadow:var(--dialog-shadow)}.add-dialog::backdrop{background:var(--backdrop)}

.account-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(min(100%,270px),1fr));gap:14px}.account-card{border:1px solid var(--color-border);border-radius:12px;background:var(--card-bg);padding:20px;min-width:0}.account-card header{display:flex;align-items:center;gap:9px}.account-card h2,.account-heading h2{font-size:14px;margin:0;overflow-wrap:anywhere}.account-card h2{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.account-ratio{display:flex;align-items:baseline;gap:8px;margin-top:22px;font-variant-numeric:tabular-nums}.account-ratio strong{font-size:34px;line-height:1.2}.account-ratio span{font-size:20px;color:var(--color-text-muted)}.ratio-label{font-size:12px;color:var(--color-text-muted);margin:6px 0 18px}.account-stats{display:grid;grid-template-columns:repeat(3,1fr);gap:14px 10px;border-top:1px solid var(--color-border);padding-top:16px;margin:0 0 18px}.account-stats dt{font-size:11px;color:var(--color-text-muted)}.account-stats dd{font-size:15px;font-weight:600;margin:4px 0 0;font-variant-numeric:tabular-nums}.account-manage{width:100%;font-size:12px}.account-heading{margin:18px 0 12px}button:focus-visible{outline:2px solid #6366f1;outline-offset:3px}
.seed-library{max-width:1180px;margin:auto}.overview{display:flex;flex-wrap:nowrap;overflow-x:auto;gap:8px;margin:16px 0;padding-bottom:4px}.overview>button{flex:1;min-width:88px;text-align:left}.overview>button.active{border-color:var(--primary)}.overview>button{border:1px solid var(--color-border);border-radius:10px;padding:10px;display:grid;gap:4px;background:var(--card-bg)}.overview span,.muted,.notice,.identity{font-size:12px;color:var(--color-text-muted);line-height:1.7}.overview strong{font-size:21px}.toolbar,.input-row{display:flex;gap:10px;align-items:center;flex-wrap:wrap}.toolbar label{display:flex;align-items:center;gap:6px}.notice{margin:12px 0}.pairing{border:1px solid var(--color-border);padding:16px;border-radius:12px;margin:18px 0}.pairing summary{display:flex;justify-content:space-between}.pairing summary span{font-size:12px;color:var(--color-text-muted)}.pairing summary span.connected{color:#16a34a}summary{cursor:pointer}form{display:grid;gap:14px;margin-top:16px}form label{display:grid;gap:6px;font-size:12px}.input-row input{flex:1;min-width:160px}input:not([type=checkbox]),select{padding:9px 10px;border:1px solid var(--color-border);border-radius:8px;background:var(--card-bg);min-width:0}button{padding:8px 12px;border:1px solid var(--color-border);border-radius:8px;cursor:pointer;white-space:nowrap}button:disabled{opacity:.5;cursor:default}.primary{background:#2563eb;color:white}.filters{margin:18px 0}.search{flex:1;min-width:180px!important}.list-head{display:flex;justify-content:space-between;margin:8px 0}.seed-list{border:1px solid var(--color-border);border-radius:12px;overflow:hidden}.seed-row+ .seed-row{border-top:1px solid var(--color-border)}.seed-row summary{display:flex;align-items:center;gap:12px;padding:13px 16px;background:var(--card-bg)}.seed-row summary:hover{background:var(--color-border)}.seed-row strong{font-size:13px;max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.identity{flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dot{width:7px;height:7px;border-radius:50%;background:#94a3b8;flex-shrink:0}.dot.alive{background:#22c55e}.badge{font-size:11px;border-radius:12px;padding:3px 8px;background:var(--color-border);white-space:nowrap}code{font-size:11px;color:var(--color-text-muted)}.row-labels{display:flex;align-items:end;flex-wrap:wrap;margin-bottom:14px}.row-details{padding:0 16px 16px}.row-details pre{font:12px/1.8 system-ui;white-space:pre-wrap;overflow-wrap:anywhere;margin:12px 0;color:var(--color-text-muted)}.empty{padding:36px;color:var(--color-text-muted);text-align:center}.pagination{justify-content:center;margin:18px 0;font-size:12px}[role=alert]{color:var(--danger);overflow-wrap:anywhere}a{color:#3b82f6;cursor:pointer;font-size:12px}@media(max-width:700px){.overview strong{font-size:18px}.seed-row summary{gap:7px;padding:12px 8px}.seed-row code{display:none}.seed-row strong{max-width:110px}.input-row button{padding:8px}.identity{font-size:11px}}
</style>
