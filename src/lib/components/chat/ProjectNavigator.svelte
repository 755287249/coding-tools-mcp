<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { appUrl } from '$lib/app-path';
  import { workspaces } from '$lib/stores/app';
  import { uiMode } from '$lib/stores/ui-mode';
  import { workspaceFolders } from '$lib/types';
  import { localChat, type ChatSession } from '$lib/api/chat';
  import { chatLocation } from '$lib/chat/location';
  import { sessionPresence } from '$lib/chat/navigation';
  import { t } from '$lib/i18n';
  import Folder from '@lucide/svelte/icons/folder';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Plus from '@lucide/svelte/icons/plus';
  import Search from '@lucide/svelte/icons/search';
  import SquarePen from '@lucide/svelte/icons/square-pen';
  import Ellipsis from '@lucide/svelte/icons/ellipsis';
  import Pencil from '@lucide/svelte/icons/pencil';
  import Check from '@lucide/svelte/icons/check';
  import { onMount } from 'svelte';
  let { onAddWorkspace, recent = false, searchOpen = false }: { onAddWorkspace?: () => void | Promise<void>; recent?: boolean; searchOpen?: boolean } = $props();
  let query = $state('');
  let picker = $state<HTMLDivElement>();
  let pickerSearch = $state('');
  let createScope = $state('');
  let creating = $state(false);
  let createError = $state('');
  let collapsed = $state<Record<string, boolean>>({});
  let sessions = $state<Record<string, ChatSession[]>>({});
  let errors = $state<Record<string, string>>({});
  let rename = $state('');
  let title = $state('');
  let saving = $state(false);
  let renameInput=$state<HTMLInputElement>();
  $effect(()=>{if(rename&&renameInput){renameInput.focus();renameInput.select()}});
  let projectsExpanded = $state(true);
  let expandedChats = $state<Record<string,boolean>>({});
  let searchInput = $state<HTMLInputElement>();
  const scope = (workspace: string, folder: string) => JSON.stringify([workspace, folder]);
  const groups = $derived($workspaces.flatMap(workspace => workspaceFolders(workspace).map(folder => ({workspace, folder, key: scope(workspace.id, folder.id)}))));
  const activeGroup = $derived(groups.find(group => group.workspace.id === $page.params.id && group.folder.id === $page.url.searchParams.get('folder')));
  const activeChat = $derived(activeGroup ? sessions[activeGroup.key]?.find(chat => chat.id === $page.url.searchParams.get('chat')) : undefined);
  const pickerItems = $derived(groups.flatMap(group => (sessions[group.key] ?? []).map(chat => ({group, chat})))
    .filter(({group, chat}) => `${chat.title} ${group.workspace.name} ${group.folder.name}`.toLocaleLowerCase().includes(pickerSearch.trim().toLocaleLowerCase()))
    .sort((a, b) => b.chat.updated_at - a.chat.updated_at));
  $effect(() => { const route = $page.url.href; picker?.hidePopover(); });
  function preparePicker() {
    pickerSearch = ''; createError = '';
    createScope = activeGroup?.key ?? groups.find(group => group.workspace.id === $page.params.id)?.key ?? groups[0]?.key ?? '';
  }
  async function createFromPicker() {
    const group = groups.find(item => item.key === createScope);
    if (!group || creating) return;
    creating = true; createError = '';
    try {
      const result = await localChat(group.workspace.id, group.folder.id, {action:'create', title:$t('chat.1')});
      if (!result.session) throw new Error($t('chat.117'));
      const chat = result.session;
      sessions = {...sessions, [group.key]:[chat, ...(sessions[group.key] ?? []).filter(item => item.id !== chat.id)]};
      picker?.hidePopover();
      await goto(appUrl(`${chatLocation(group.workspace.id, group.folder.id, chat.id)}&connect=1`), {noScroll:true});
    } catch (error) { createError = String(error); }
    finally { creating = false; }
  }
  const needle = $derived(query.trim().toLocaleLowerCase());
  const recentItems = $derived(groups.flatMap(group => (sessions[group.key] ?? []).map(chat => ({group, chat}))).filter(item => !needle || `${item.chat.title} ${item.group.workspace.name}`.toLocaleLowerCase().includes(needle)).sort((a,b) => b.chat.updated_at - a.chat.updated_at).slice(0,40));
  const presenceLabels = { online:'chat.98', offline:'chat.22', working:'chat.70', error:'chat.71', closed:'chat.21' } as const;
  onMount(() => { try { const value=JSON.parse(localStorage.getItem('ctmcp-project-collapse') ?? '{}'); if(value && typeof value==='object' && !Array.isArray(value)) collapsed=value; } catch {} });
  $effect(() => { if (searchOpen) searchInput?.focus(); });
  $effect(() => {
    const targets = groups;
    let stopped=false; let timer:ReturnType<typeof setTimeout>;
    async function poll() {
      // Query summaries only; no transcript or authentication material is loaded by navigation.
      for(let offset=0;offset<targets.length;offset+=4) {
        if(stopped) return;
        await Promise.all(targets.slice(offset,offset+4).map(async group => {
          try { const result=await localChat(group.workspace.id,group.folder.id,{action:'list'}); if(!stopped){sessions={...sessions,[group.key]:result.sessions??[]};errors={...errors,[group.key]:''};} }
          catch(error){if(!stopped)errors={...errors,[group.key]:String(error)}}
        }));
      }
      if(!stopped)timer=setTimeout(poll,document.hidden?10000:3000);
    }
    void poll();
    return()=>{stopped=true;clearTimeout(timer)};
  });
  function unread(ws:string,folder:string,chat:ChatSession){try{const saved=JSON.parse(localStorage.getItem(`ctmcp-chat-seen:${ws}:${folder}`)??'{}');return Math.max(0,(chat.assistant_message_count??0)-(Number(saved?.[chat.id])||0))}catch{return 0}}
  function visibleChats(ws:string,folder:string,name:string){
    const list=sessions[scope(ws,folder)]??[];
    if(needle)return list.filter(chat=>name.toLocaleLowerCase().includes(needle)||chat.title.toLocaleLowerCase().includes(needle));
    if(expandedChats[scope(ws,folder)])return list;
    const first=list.slice(0,4);
    const active=$page.params.id===ws&&$page.url.searchParams.get('folder')===folder?list.find(chat=>chat.id===$page.url.searchParams.get('chat')):undefined;
    if(active&&!first.includes(active))first.push(active);
    return first;
  }
  function toggle(id:string){collapsed={...collapsed,[id]:!collapsed[id]};try{localStorage.setItem('ctmcp-project-collapse',JSON.stringify(collapsed))}catch{}}
  function open(workspace:string,folder:string,chat?:string){void goto(appUrl(chatLocation(workspace,folder,chat)),{noScroll:true});}
  function editProject(id:string){uiMode.set('advanced');void goto(appUrl(`/workspace/${encodeURIComponent(id)}?tab=settings`));}
  async function renameChat(workspace:string,folder:string,chat:ChatSession){
    const value=title.trim();if(saving||!value)return;saving=true;
    try{await localChat(workspace,folder,{action:'rename',chat_id:chat.id,title:value});sessions={...sessions,[scope(workspace,folder)]:(sessions[scope(workspace,folder)]??[]).map(item=>item.id===chat.id?{...item,title:value}:item)};rename=''}
    catch(error){errors={...errors,[scope(workspace,folder)]:String(error)}}finally{saving=false}
  }
</script>

<div class="project-nav">
  <header><button class="session-picker-trigger" popovertarget="conversation-picker" onclick={preparePicker} title={$t('chat.113')} aria-label={$t('chat.113')}><strong>{activeChat?.title ?? $t('chat.99')}</strong><ChevronDown size={15}/></button><button type="button" title={$t('chat.100')} aria-label={$t('chat.100')} onclick={() => searchInput?.focus()}><Search size={15}/></button></header>
  <div id="conversation-picker" class="conversation-picker" popover="auto" bind:this={picker}>
    <h2>{$t('chat.113')}</h2>
    <label class="project-search"><Search size={14}/><input bind:value={pickerSearch} placeholder={$t('chat.100')} aria-label={$t('chat.114')}/></label>
    <div class="picker-list">
      {#each pickerItems as {group,chat} (scope(group.key,chat.id))}
        {@const active=activeGroup?.key===group.key && activeChat?.id===chat.id}
        <button class="picker-chat" aria-current={active?'page':undefined} onclick={()=>{picker?.hidePopover();open(group.workspace.id,group.folder.id,chat.id)}}>
          <span><strong>{chat.title}</strong><small>{group.workspace.name} · {group.folder.name} · {$t(presenceLabels[sessionPresence(chat)])}</small></span>{#if active}<Check size={16}/>{/if}
        </button>
      {:else}<p class="muted">{$t('chat.104')}</p>{/each}
    </div>
    <form class="picker-create" onsubmit={event=>{event.preventDefault();void createFromPicker()}}>
      <label for="picker-project">{$t('chat.109')}</label>
      <select id="picker-project" bind:value={createScope} disabled={creating}>{#each groups as group (group.key)}<option value={group.key}>{group.workspace.name} · {group.folder.name}</option>{/each}</select>
      <button type="submit" disabled={creating||!createScope}><Plus size={16}/>{$t(creating?'chat.116':'chat.115')}</button>
      <p>{$t('chat.118')}</p>
      {#if createError}<p class="load-error" role="alert">{createError}</p>{/if}
    </form>
  </div>
  <button class="new-conversation" onclick={() => goto(appUrl('/?new=1'))}><SquarePen size={16}/>{$t('chat.1')}</button>
  <label class="project-search"><Search size={13}/><input bind:this={searchInput} bind:value={query} placeholder={$t('chat.100')} aria-label={$t('chat.100')}/></label>
  <div class="nav-scroll">
    {#if recent}
      <p class="section-title">{$t('chat.103')}</p>
      {#each recentItems as {group,chat} (scope(group.key,chat.id))}
        {@render chatRow(group.workspace.id,group.folder.id,chat,true)}
      {:else}<p class="muted">{$t('chat.104')}</p>{/each}
    {:else}
      <div class="section-heading"><button onclick={() => projectsExpanded=!projectsExpanded} aria-expanded={projectsExpanded}>{$t('chat.101')}<ChevronDown size={13}/></button>{#if onAddWorkspace}<button title={$t('New workspace (separate MCP)')} aria-label={$t('New workspace (separate MCP)')} onclick={onAddWorkspace}><Plus size={15}/></button>{/if}</div>
      {#if projectsExpanded}
        {#each $workspaces as workspace (workspace.id)}
          {@const projectGroups=groups.filter(group=>group.workspace.id===workspace.id)}
          {@const matching=projectGroups.some(group=>(sessions[group.key]??[]).some(chat=>chat.title.toLocaleLowerCase().includes(needle)))}
          {#if !needle || workspace.name.toLocaleLowerCase().includes(needle) || matching}
            <div class="project-group">
              <div class="project-heading" class:current={$page.params.id===workspace.id}>
                <button class="project-name" onclick={() => toggle(workspace.id)} aria-expanded={!collapsed[workspace.id]} title={workspace.name}><Folder size={14}/><span>{workspace.name}</span></button>
                <details class="project-info"><summary aria-label={`${$t('chat.105')}: ${workspace.name}`} title={$t('chat.105')}><Ellipsis size={15}/></summary><div><strong>{workspace.name}</strong>{#each projectGroups as group}<p>{group.folder.path}</p>{/each}<button onclick={() => editProject(workspace.id)}>{$t('Workspace settings')}</button></div></details>
                <button title={$t('chat.1')} aria-label={`${$t('chat.1')}: ${workspace.name}`} onclick={() => {const folder=projectGroups[0]?.folder;if(folder)open(workspace.id,folder.id)}}><SquarePen size={14}/></button>
              </div>
              {#if !collapsed[workspace.id] || needle}
                {#each projectGroups as group (group.key)}
                  {#if projectGroups.length>1}<button class="folder-name" onclick={() => open(workspace.id,group.folder.id)}><Folder size={12}/>{group.folder.name}</button>{/if}
                  {#each visibleChats(workspace.id,group.folder.id,workspace.name) as chat (chat.id)}
                    {@render chatRow(workspace.id,group.folder.id,chat,false)}
                  {:else}<button class="empty-project" onclick={() => open(workspace.id,group.folder.id)}>{$t('chat.104')}</button>{/each}
                  {#if !needle && (sessions[group.key]?.length??0)>4}<button class="empty-project" onclick={()=>expandedChats={...expandedChats,[group.key]:!expandedChats[group.key]}}>{$t(expandedChats[group.key]?'chat.112':'chat.111')} ({sessions[group.key]?.length})</button>{/if}
                  {#if errors[group.key]}<p class="load-error" role="status">{errors[group.key]}</p>{/if}
                {/each}
              {/if}
            </div>
          {/if}
        {/each}
      {/if}
    {/if}
  </div>
</div>

{#snippet chatRow(ws:string,folder:string,chat:ChatSession,showProject:boolean)}
  {@const key=scope(scope(ws,folder),chat.id)}
  {@const active=$page.params.id===ws && $page.url.searchParams.get('folder')===folder && $page.url.searchParams.get('chat')===chat.id}
  {@const presence=sessionPresence(chat)}
  {@const newCount=unread(ws,folder,chat)}
  <div class="tree-chat" class:active data-presence={presence}>
    {#if rename===key}
      <form onsubmit={event=>{event.preventDefault();void renameChat(ws,folder,chat)}}><input bind:this={renameInput} aria-label={$t('chat.86')} bind:value={title} maxlength="240" disabled={saving} onkeydown={event=>{if(event.key==='Escape')rename=''}}/><button disabled={saving||!title.trim()}>{$t('Save')}</button></form>
    {:else}
      <button class="chat-link" onclick={()=>open(ws,folder,chat.id)} aria-current={active?'page':undefined} title={`${chat.title} · ${$t(presenceLabels[presence])}`}><span>{chat.title}{#if showProject}<small>{$workspaces.find(item=>item.id===ws)?.name}</small>{/if}</span><i title={$t(presenceLabels[presence])}></i>{#if newCount}<b class="unread-count" aria-label={`${$t('chat.90')}: ${newCount}`}>{newCount>99?'99+':newCount}</b>{/if}</button>
      <button class="rename-chat" title={$t('chat.85')} aria-label={`${$t('chat.85')}: ${chat.title}`} onclick={()=>{rename=key;title=chat.title}}><Pencil size={12}/></button>
    {/if}
  </div>
{/snippet}
<style>
.session-picker-trigger{display:flex;align-items:center;gap:7px;min-width:0;max-width:calc(100% - 25px);padding:6px;text-align:left}.session-picker-trigger strong{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.session-picker-trigger :global(svg){flex:none}.conversation-picker{position:fixed;inset:52px auto auto 62px;margin:0;width:min(350px,calc(100vw - 76px));max-height:calc(100dvh - 72px);overflow:auto;padding:12px;border:1px solid var(--color-border);border-radius:12px;background:var(--card-bg);color:var(--color-text);box-shadow:0 10px 32px #0005}.conversation-picker h2{font-size:13px;margin:2px 5px 12px}.picker-list{max-height:38dvh;overflow:auto}.picker-chat{display:flex;align-items:center;gap:10px;width:100%;text-align:left;padding:10px;border-radius:8px}.picker-chat[aria-current=page]{background:var(--surface-hover)}.picker-chat>span{flex:1;min-width:0}.picker-chat strong,.picker-chat small{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.picker-chat strong{font-size:12px;font-weight:500}.picker-chat small{font-size:10px;color:var(--color-text-muted);margin-top:4px}.picker-create{display:flex;flex-direction:column;gap:8px;border-top:1px solid var(--color-border);padding:12px 3px 2px;margin-top:8px;font-size:11px}.picker-create select{width:100%;padding:8px;border:1px solid var(--color-border);border-radius:6px;background:var(--card-bg);color:var(--color-text)}.picker-create button{display:flex;align-items:center;justify-content:center;gap:7px;padding:9px;background:var(--surface-hover)}.picker-create button:disabled{opacity:.5;cursor:default}.picker-create p{font-size:10px;color:var(--color-text-muted);line-height:1.6}.picker-create .load-error{color:var(--danger)}

.project-nav{height:100%;display:flex;flex-direction:column;padding:14px 10px;color:var(--color-text);min-width:0}header{display:flex;align-items:center;justify-content:space-between;padding:2px 9px 16px;font-size:15px}button,summary{cursor:pointer}button{border-radius:6px}button:hover{background:var(--surface-hover)}button:focus-visible,summary:focus-visible,input:focus-visible{outline:2px solid var(--primary);outline-offset:1px}.new-conversation{display:flex;align-items:center;gap:10px;padding:9px;margin-bottom:14px;text-align:left;font-size:12px}.project-search{display:flex;align-items:center;gap:7px;border:1px solid var(--color-border);border-radius:7px;padding:7px;color:var(--color-text-muted);margin-bottom:14px}.project-search input{min-width:0;width:100%;background:transparent;border:0;font-size:11px}.nav-scroll{flex:1;min-height:0;overflow-y:auto;overflow-x:hidden}.section-heading{display:flex;align-items:center;justify-content:space-between;color:var(--color-text-muted);padding:0 6px 8px;font-size:11px}.section-heading button{display:flex;align-items:center;gap:5px;padding:4px}.section-title{font-size:11px;color:var(--color-text-muted);padding:8px}.project-group{margin-bottom:16px}.project-heading{display:flex;align-items:center;gap:3px;padding:0 4px}.project-heading.current{background:color-mix(in srgb,var(--color-text) 5%,transparent);border-radius:7px}.project-heading>button:last-child{padding:4px;flex:none}.project-name{display:flex;align-items:center;gap:8px;min-width:0;flex:1;text-align:left;padding:8px 4px;font-size:12px}.project-name span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.project-info{position:relative;flex:none}.project-info summary{list-style:none;padding:4px}.project-info summary::-webkit-details-marker{display:none}.project-info>div{position:fixed;left:64px;top:140px;width:min(340px,calc(100vw - 80px));padding:15px;z-index:80;background:var(--card-bg);border:1px solid var(--color-border);border-radius:10px;box-shadow:0 8px 28px #0005;font-size:12px}.project-info p{overflow-wrap:anywhere;color:var(--color-text-muted);margin:9px 0}.project-info button{padding:7px;background:var(--surface-hover)}.tree-chat{display:flex;align-items:center;margin:2px 0 2px 22px;border-radius:7px;min-width:0}.tree-chat.active{background:var(--surface-hover)}.chat-link{display:flex;gap:7px;align-items:center;min-width:0;flex:1;text-align:left;padding:8px 6px;font-size:12px}.chat-link>span{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1}.chat-link small{display:block;font-size:9px;color:var(--color-text-muted)}.chat-link i{width:5px;height:5px;border-radius:50%;background:#888;flex:none}.tree-chat[data-presence=online] i{background:#31a56c}.tree-chat[data-presence=working] i{background:#6a9bd4}.tree-chat[data-presence=error] i{background:#db7969}.rename-chat{padding:6px;opacity:0;color:var(--color-text-muted)}.tree-chat:hover .rename-chat,.tree-chat:focus-within .rename-chat{opacity:1}.tree-chat form{display:flex;gap:3px;width:100%;padding:5px;font-size:11px}.tree-chat input{width:0;flex:1;min-width:0;background:var(--card-bg);border:1px solid var(--color-border);padding:4px}.folder-name,.empty-project{display:flex;align-items:center;gap:5px;margin-left:28px;font-size:10px;color:var(--color-text-muted);padding:6px}.load-error{font-size:10px;color:var(--danger);overflow-wrap:anywhere;padding:7px}.muted{font-size:11px;color:var(--color-text-muted);padding:8px}
.unread-count{min-width:14px;border-radius:8px;background:#55749a;color:white;font:9px/14px system-ui;text-align:center;padding:0 3px}
</style>
