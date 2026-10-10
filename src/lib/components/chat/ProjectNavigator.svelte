<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { appUrl } from '$lib/app-path';
  import { workspaces } from '$lib/stores/app';
  import { uiMode } from '$lib/stores/ui-mode';
  import { workspaceFolders } from '$lib/types';
  import { localChat, type ChatSession } from '$lib/api/chat';
  import { chatLocation } from '$lib/chat/location';
  import { sessionPresence, isConnectedConversation } from '$lib/chat/navigation';
  import { t } from '$lib/i18n';
  import Folder from '@lucide/svelte/icons/folder';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Plus from '@lucide/svelte/icons/plus';
  import Search from '@lucide/svelte/icons/search';
  import SquarePen from '@lucide/svelte/icons/square-pen';
  import Ellipsis from '@lucide/svelte/icons/ellipsis';
  import Pin from '@lucide/svelte/icons/pin';
  import ListFilter from '@lucide/svelte/icons/list-filter';
  import Pencil from '@lucide/svelte/icons/pencil';
  import Check from '@lucide/svelte/icons/check';
  import Archive from '@lucide/svelte/icons/archive';
  import ArchiveRestore from '@lucide/svelte/icons/archive-restore';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import { innerPopover } from '$lib/chat/popover';
  import { onMount, tick } from 'svelte';
  let { onAddWorkspace, recent = false, onSearch }: { onAddWorkspace?: () => void | Promise<void>; recent?: boolean; onSearch: () => void } = $props();
  let projectMenu = $state<HTMLDivElement>();
  let menuWorkspace = $state('');
  let menuAnchor: HTMLElement | null = null;
  let menuLeft=$state(0),menuTop=$state(0);
  function positionProjectMenu() {
    if(!menuAnchor || !projectMenu?.matches(':popover-open'))return;
    const pos=innerPopover(menuAnchor.getBoundingClientRect(),{width:projectMenu.offsetWidth,height:projectMenu.offsetHeight},{width:innerWidth,height:innerHeight});
    menuLeft=pos.left;menuTop=pos.top;
  }
  async function showProjectMenu(id:string,event:MouseEvent) {
    if(menuWorkspace===id&&projectMenu?.matches(':popover-open')){projectMenu.hidePopover();return;}
    menuWorkspace=id;menuAnchor=event.currentTarget as HTMLElement;
    await tick();projectMenu?.showPopover();positionProjectMenu();
  }
  $effect(()=>{const route=$page.url.href;projectMenu?.hidePopover();});
  let chatMenu = $state<HTMLDivElement>();
  let chatMenuTarget = $state<{workspace:string;folder:string;chat:ChatSession} | null>(null);
  let chatMenuLeft=$state(0),chatMenuTop=$state(0);
  let chatMenuAnchor: HTMLElement | null=null;
  let pinSaving=$state('');
  function positionChatMenu(){if(!chatMenuAnchor||!chatMenu?.matches(':popover-open'))return;const pos=innerPopover(chatMenuAnchor.getBoundingClientRect(),{width:chatMenu.offsetWidth,height:chatMenu.offsetHeight},{width:innerWidth,height:innerHeight});chatMenuLeft=pos.left;chatMenuTop=pos.top;}
  async function showChatMenu(workspace:string,folder:string,chat:ChatSession,event:MouseEvent){deleteArmed=false;chatMenuTarget={workspace,folder,chat};chatMenuAnchor=event.currentTarget as HTMLElement;await tick();chatMenu?.showPopover();positionChatMenu();}
  async function togglePin(workspace:string,folder:string,chat:ChatSession){
    if(pinSaving)return;pinSaving=scope(scope(workspace,folder),chat.id);
    try{const result=await localChat(workspace,folder,{action:'pin',chat_id:chat.id,pinned:!chat.pinned});if(result.session)sessions={...sessions,[scope(workspace,folder)]:(sessions[scope(workspace,folder)]??[]).map(item=>item.id===chat.id?result.session!:item).sort((a,b)=>Number(!!a.archived)-Number(!!b.archived)||Number(!!b.pinned)-Number(!!a.pinned)||b.updated_at-a.updated_at)};chatMenu?.hidePopover();}
    catch(error){errors={...errors,[scope(workspace,folder)]:String(error)}}finally{pinSaving=''}
  }
  $effect(()=>{const route=$page.url.href;chatMenu?.hidePopover();});
  const sortChats=(list:ChatSession[])=>[...list].sort((a,b)=>Number(!!a.archived)-Number(!!b.archived)||Number(!!b.pinned)-Number(!!a.pinned)||b.updated_at-a.updated_at);
  let deleteArmed=$state(false);
  let showArchived=$state<Record<string,boolean>>({});
  async function toggleArchive(workspace:string,folder:string,chat:ChatSession){
    if(pinSaving)return;pinSaving=scope(scope(workspace,folder),chat.id);
    try{const result=await localChat(workspace,folder,{action:'archive',chat_id:chat.id,archived:!chat.archived});if(result.session)sessions={...sessions,[scope(workspace,folder)]:sortChats((sessions[scope(workspace,folder)]??[]).map(item=>item.id===chat.id?result.session!:item))};chatMenu?.hidePopover();}
    catch(error){errors={...errors,[scope(workspace,folder)]:String(error)}}finally{pinSaving=''}
  }
  async function deleteChat(workspace:string,folder:string,chat:ChatSession){
    if(pinSaving)return;
    if(!deleteArmed){deleteArmed=true;return;}
    pinSaving=scope(scope(workspace,folder),chat.id);
    try{
      if(chat.status==='connected'||chat.status==='waiting')await localChat(workspace,folder,{action:'detach',chat_id:chat.id});
      const result=await localChat(workspace,folder,{action:'delete',chat_id:chat.id});
      if(result.deleted!==true)throw new Error('Conversation deletion was not confirmed.');
      sessions={...sessions,[scope(workspace,folder)]:(sessions[scope(workspace,folder)]??[]).filter(item=>item.id!==chat.id)};
      chatMenu?.hidePopover();
      if($page.params.id===workspace&&$page.url.searchParams.get('folder')===folder&&$page.url.searchParams.get('chat')===chat.id)open(workspace,folder);
    }
    catch(error){errors={...errors,[scope(workspace,folder)]:String(error)}}finally{pinSaving='';deleteArmed=false}
  }
  let picker = $state<HTMLDivElement>();
  let pickerTrigger=$state<HTMLButtonElement>();
  let pickerLeft=$state(62), pickerTop=$state(90);
  let collapsed = $state<Record<string, boolean>>({});
  let sessions = $state<Record<string, ChatSession[]>>({});
  let errors = $state<Record<string, string>>({});
  let rename = $state('');
  let title = $state('');
  let saving = $state(false);
  let renameInput=$state<HTMLInputElement>();
  $effect(()=>{if(rename&&renameInput){renameInput.focus();renameInput.select()}});
  let projectsExpanded = $state(true);
  let recentExpanded = $state(true);
  let recentFilter = $state<'all'|'work'|'group'>('all');
  let recentProjectNames = $state(false);
  let recentMenu = $state<HTMLDivElement>();
  let recentMenuKind = $state<'more'|'filter'>('filter');
  let recentMenuAnchor: HTMLElement | null = null;
  let recentMenuLeft=$state(0), recentMenuTop=$state(0);
  function positionRecentMenu() {
    if(!recentMenuAnchor || !recentMenu?.matches(':popover-open'))return;
    const pos=innerPopover(recentMenuAnchor.getBoundingClientRect(),{width:recentMenu.offsetWidth,height:recentMenu.offsetHeight},{width:innerWidth,height:innerHeight});
    recentMenuLeft=pos.left;recentMenuTop=pos.top;
  }
  async function showRecentMenu(kind:'more'|'filter',event:MouseEvent) {
    if(recentMenuKind===kind&&recentMenu?.matches(':popover-open')){recentMenu.hidePopover();return;}
    recentMenuKind=kind;recentMenuAnchor=event.currentTarget as HTMLElement;
    await tick();recentMenu?.showPopover();positionRecentMenu();
  }
  $effect(()=>{const route=$page.url.href;recentMenu?.hidePopover();});
  let expandedChats = $state<Record<string,boolean>>({});
  const scope = (workspace: string, folder: string) => JSON.stringify([workspace, folder]);
  const groups = $derived($workspaces.flatMap(workspace => workspaceFolders(workspace).map(folder => ({workspace, folder, key: scope(workspace.id, folder.id)}))));
  const activeGroup = $derived(groups.find(group => group.workspace.id === $page.params.id && group.folder.id === $page.url.searchParams.get('folder')));
  const activeChat = $derived(activeGroup ? sessions[activeGroup.key]?.find(chat => chat.id === $page.url.searchParams.get('chat')) : undefined);
  const pickerItems = $derived(groups.flatMap(group => (sessions[group.key] ?? []).filter(isConnectedConversation).map(chat => ({group, chat})))
    .sort((a, b) => b.chat.updated_at - a.chat.updated_at));
  $effect(() => { const route = $page.url.href; picker?.hidePopover(); });
  function positionPicker() {
    if(!pickerTrigger||!picker?.matches(':popover-open'))return;
    const anchor=pickerTrigger.getBoundingClientRect();
    pickerLeft=Math.max(8,Math.min(anchor.left,innerWidth-picker.offsetWidth-8));
    pickerTop=Math.max(8,Math.min(anchor.bottom+6,innerHeight-picker.offsetHeight-8));
  }
  const recentItems = $derived(groups.flatMap(group => (sessions[group.key] ?? []).filter(chat => !chat.archived && (recentFilter==='all' || (chat.mode??'work')===recentFilter)).map(chat => ({group, chat}))).sort((a,b) => Number(!!b.chat.pinned)-Number(!!a.chat.pinned)||b.chat.updated_at-a.chat.updated_at).slice(0,40));
  const presenceLabels = { online:'chat.98', offline:'chat.22', working:'chat.70', error:'chat.71', closed:'chat.21' } as const;
  onMount(() => { try { const value=JSON.parse(localStorage.getItem('ctmcp-project-collapse') ?? '{}'); if(value && typeof value==='object' && !Array.isArray(value)) collapsed=value; } catch {} });
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
  function visibleChats(ws:string,folder:string){
    const list=(sessions[scope(ws,folder)]??[]).filter(chat=>!chat.archived);
    if(expandedChats[scope(ws,folder)])return list;
    const first=list.slice(0,4);
    const active=$page.params.id===ws&&$page.url.searchParams.get('folder')===folder?list.find(chat=>chat.id===$page.url.searchParams.get('chat')):undefined;
    if(active&&!first.includes(active))first.push(active);
    return first;
  }
  function archivedChats(ws:string,folder:string){return (sessions[scope(ws,folder)]??[]).filter(chat=>chat.archived);}
  function toggle(id:string){collapsed={...collapsed,[id]:!collapsed[id]};try{localStorage.setItem('ctmcp-project-collapse',JSON.stringify(collapsed))}catch{}}
  function open(workspace:string,folder:string,chat?:string){void goto(appUrl(chatLocation(workspace,folder,chat)),{noScroll:true});}
  function editProject(id:string){uiMode.set('advanced');void goto(appUrl(`/workspace/${encodeURIComponent(id)}?tab=settings`));}
  async function renameChat(workspace:string,folder:string,chat:ChatSession){
    const value=title.trim();if(saving||!value)return;saving=true;
    try{await localChat(workspace,folder,{action:'rename',chat_id:chat.id,title:value});sessions={...sessions,[scope(workspace,folder)]:(sessions[scope(workspace,folder)]??[]).map(item=>item.id===chat.id?{...item,title:value}:item)};rename=''}
    catch(error){errors={...errors,[scope(workspace,folder)]:String(error)}}finally{saving=false}
  }
</script>

<svelte:window onresize={()=>{positionProjectMenu();positionChatMenu();positionPicker();positionRecentMenu()}} onscrollcapture={()=>{positionProjectMenu();positionChatMenu();positionPicker();positionRecentMenu()}}/>
<div bind:this={chatMenu} class="chat-actions-menu" popover="auto" style:left={`${chatMenuLeft}px`} style:top={`${chatMenuTop}px`}>
  {#if chatMenuTarget}{@const target=chatMenuTarget}<button onclick={()=>{rename=scope(scope(target.workspace,target.folder),target.chat.id);title=target.chat.title;chatMenu?.hidePopover()}}><Pencil size={15}/>{$t('chat.85')}</button><button disabled={!!pinSaving} onclick={()=>togglePin(target.workspace,target.folder,target.chat)}><Pin size={15} class={target.chat.pinned?'pin-icon is-pinned':'pin-icon'}/>{$t(target.chat.pinned?'chat.unpin':'chat.pin')}</button><button disabled={!!pinSaving} onclick={()=>toggleArchive(target.workspace,target.folder,target.chat)}>{#if target.chat.archived}<ArchiveRestore size={15}/>{$t('chat.unarchive')}{:else}<Archive size={15}/>{$t('chat.archive')}{/if}</button><hr/><button class="danger" class:armed={deleteArmed} disabled={!!pinSaving} onclick={()=>deleteChat(target.workspace,target.folder,target.chat)}><Trash2 size={15}/>{deleteArmed?$t(target.chat.status==='connected'||target.chat.status==='waiting'?'chat.deleteConfirmConnected':'chat.deleteConfirm'):$t('chat.delete')}</button>{/if}
</div>
<div bind:this={projectMenu} class="project-info-card" popover="auto" style:left={`${menuLeft}px`} style:top={`${menuTop}px`}>
  {#if menuWorkspace}{@const workspace=$workspaces.find(w=>w.id===menuWorkspace)}{#if workspace}<strong>{workspace.name}</strong>{#each workspaceFolders(workspace) as folder}<p>{folder.path}</p>{/each}<button onclick={()=>{projectMenu?.hidePopover();editProject(workspace.id)}}>{$t('Workspace settings')}</button>{/if}{/if}
</div>
<div class="project-nav">
  <header><button class="session-picker-trigger" bind:this={pickerTrigger} popovertarget="conversation-picker" title={$t('chat.113')} aria-label={$t('chat.113')}><strong>{activeChat?.title ?? $t('chat.99')}</strong><ChevronDown size={15}/></button><button type="button" title={$t('chat.100')} aria-label={$t('chat.100')} onclick={onSearch}><Search size={15}/></button></header>
  <div id="conversation-picker" class="conversation-picker" popover="auto" bind:this={picker} ontoggle={positionPicker} style:left={`${pickerLeft}px`} style:top={`${pickerTop}px`}>
    <div class="picker-list" aria-label={$t('chat.113')}>
      {#each pickerItems as {group,chat} (scope(group.key,chat.id))}
        {@const active=activeGroup?.key===group.key && activeChat?.id===chat.id}
        <button class="picker-chat" aria-current={active?'page':undefined} onclick={()=>{picker?.hidePopover();open(group.workspace.id,group.folder.id,chat.id)}}>
          <span><strong>{chat.title}</strong><small>{group.workspace.name} · {$t(presenceLabels[sessionPresence(chat)])}</small></span>{#if active}<Check size={16}/>{/if}
        </button>
      {:else}<p class="muted">{$t('chat.noConnected')}</p>{/each}
    </div>
  </div>
  <button class="new-conversation" onclick={() => goto(appUrl('/?new=1'))}><SquarePen size={18}/>{$t('chat.1')}</button>
  <div class="nav-scroll">
    {#if !recent}
      <div class="section-heading"><button onclick={() => projectsExpanded=!projectsExpanded} aria-expanded={projectsExpanded}>{$t('chat.101')}<ChevronDown size={13}/></button>{#if onAddWorkspace}<button title={$t('New workspace (separate MCP)')} aria-label={$t('New workspace (separate MCP)')} onclick={onAddWorkspace}><Plus size={15}/></button>{/if}</div>
      {#if projectsExpanded}
        {#each $workspaces as workspace (workspace.id)}
          {@const projectGroups=groups.filter(group=>group.workspace.id===workspace.id)}
            <div class="project-group">
              <div class="project-heading" class:current={$page.params.id===workspace.id}>
                <button class="project-name" onclick={() => toggle(workspace.id)} aria-expanded={!collapsed[workspace.id]} title={workspace.name}><Folder size={16}/><span>{workspace.name}</span></button>
                <button class="project-info-trigger" aria-label={`${$t('chat.105')}: ${workspace.name}`} title={$t('chat.105')} onclick={event=>showProjectMenu(workspace.id,event)}><Ellipsis size={15}/></button>
                <button title={$t('chat.1')} aria-label={`${$t('chat.1')}: ${workspace.name}`} onclick={() => {const folder=projectGroups[0]?.folder;if(folder)open(workspace.id,folder.id)}}><SquarePen size={16}/></button>
              </div>
              {#if !collapsed[workspace.id]}
                {#each projectGroups as group (group.key)}
                  {@const activeCount=(sessions[group.key]??[]).filter(chat=>!chat.archived).length}
                  {@const archived=archivedChats(workspace.id,group.folder.id)}
                  {#if projectGroups.length>1}<button class="folder-name" onclick={() => open(workspace.id,group.folder.id)}><Folder size={12}/>{group.folder.name}</button>{/if}
                  {#each visibleChats(workspace.id,group.folder.id) as chat (chat.id)}
                    {@render chatRow(workspace.id,group.folder.id,chat,false)}
                  {:else}<button class="empty-project" onclick={() => open(workspace.id,group.folder.id)}>{$t('chat.104')}</button>{/each}
                  {#if activeCount>4}<button class="empty-project" onclick={()=>expandedChats={...expandedChats,[group.key]:!expandedChats[group.key]}}>{$t(expandedChats[group.key]?'chat.112':'chat.111')} ({activeCount})</button>{/if}
                  {#if archived.length}
                    <button class="empty-project archived-toggle" aria-expanded={!!showArchived[group.key]} onclick={()=>showArchived={...showArchived,[group.key]:!showArchived[group.key]}}><Archive size={12}/>{$t('chat.archived')} ({archived.length})<ChevronDown size={12}/></button>
                    {#if showArchived[group.key]}{#each archived as chat (chat.id)}{@render chatRow(workspace.id,group.folder.id,chat,false)}{/each}{/if}
                  {/if}
                  {#if errors[group.key]}<p class="load-error" role="status">{errors[group.key]}</p>{/if}
                {/each}
              {/if}
            </div>
        {/each}
      {/if}
    {/if}
    <section class="recent-section" aria-label={$t('chat.recent')}>
      <div class="section-heading recent-heading">
        <button onclick={()=>recentExpanded=!recentExpanded} aria-expanded={recentExpanded}>{$t('chat.recent')}<ChevronDown size={14} class={recentExpanded?'':'collapsed-chevron'}/></button>
        <div class="recent-actions">
          <button aria-label={$t('chat.recentOptions')} title={$t('chat.recentOptions')} onclick={event=>showRecentMenu('more',event)}><Ellipsis size={16}/></button>
          <button class:filtered={recentFilter!=='all'} aria-label={$t('chat.recentFilter')} title={$t('chat.recentFilter')} onclick={event=>showRecentMenu('filter',event)}><ListFilter size={16}/></button>
          <button aria-label={$t('chat.1')} title={$t('chat.1')} onclick={()=>goto(appUrl('/?new=1'))}><SquarePen size={16}/></button>
        </div>
      </div>
      {#if recentExpanded}
        {#each recentItems as {group,chat} (scope(group.key,chat.id))}
          {@render chatRow(group.workspace.id,group.folder.id,chat,recentProjectNames)}
        {:else}<p class="muted">{$t('chat.noRecentMatches')}</p>{/each}
      {/if}
    </section>
  </div>
</div>
<div class="recent-menu" role="menu" bind:this={recentMenu} popover="auto" style:left={`${recentMenuLeft}px`} style:top={`${recentMenuTop}px`}>
  {#if recentMenuKind==='filter'}
    {#each ['all','work','group'] as filter}
      <button role="menuitemradio" aria-checked={recentFilter===filter} onclick={()=>{recentFilter=filter as typeof recentFilter;recentExpanded=true;recentMenu?.hidePopover()}}><span>{$t(filter==='all'?'chat.allModes':filter==='work'?'chat.work':'chat.group')}</span>{#if recentFilter===filter}<Check size={15}/>{/if}</button>
    {/each}
  {:else}
    <button role="menuitemcheckbox" aria-checked={recentProjectNames} onclick={()=>{recentProjectNames=!recentProjectNames;recentMenu?.hidePopover()}}><span>{$t('chat.showProjectNames')}</span>{#if recentProjectNames}<Check size={15}/>{/if}</button>
    <button onclick={()=>{recentExpanded=!recentExpanded;recentMenu?.hidePopover()}}>{$t(recentExpanded?'chat.collapseRecent':'chat.expandRecent')}</button>
  {/if}
</div>

{#snippet chatRow(ws:string,folder:string,chat:ChatSession,showProject:boolean)}
  {@const key=scope(scope(ws,folder),chat.id)}
  {@const active=$page.params.id===ws && $page.url.searchParams.get('folder')===folder && $page.url.searchParams.get('chat')===chat.id}
  {@const presence=sessionPresence(chat)}
  {@const newCount=unread(ws,folder,chat)}
  <div class="tree-chat" class:active class:archived-row={chat.archived} data-presence={presence}>
    {#if rename===key}
      <form onsubmit={event=>{event.preventDefault();void renameChat(ws,folder,chat)}}><input bind:this={renameInput} aria-label={$t('chat.86')} bind:value={title} maxlength="240" disabled={saving} onkeydown={event=>{if(event.key==='Escape')rename=''}}/><button disabled={saving||!title.trim()}>{$t('Save')}</button></form>
    {:else}
      <button class="chat-link" onclick={()=>open(ws,folder,chat.id)} aria-current={active?'page':undefined} title={`${chat.title} · ${$t(presenceLabels[presence])}`}><i title={$t(presenceLabels[presence])}></i><span>{chat.title}{#if showProject}<small>{$workspaces.find(item=>item.id===ws)?.name}</small>{/if}</span><em class="chat-mode-tag">#{$t(chat.mode==='group'?'chat.group':'chat.work')}</em>{#if newCount}<b class="unread-count" aria-label={`${$t('chat.90')}: ${newCount}`}>{newCount>99?'99+':newCount}</b>{/if}</button>
      <div class="chat-row-actions"><button title={$t('chat.conversationActions')} aria-label={`${$t('chat.conversationActions')}: ${chat.title}`} onclick={event=>showChatMenu(ws,folder,chat,event)}><Ellipsis size={15}/></button><button class:pinned={chat.pinned} disabled={!!pinSaving} title={$t(chat.pinned?'chat.unpin':'chat.pin')} aria-label={`${$t(chat.pinned?'chat.unpin':'chat.pin')}: ${chat.title}`} onclick={()=>togglePin(ws,folder,chat)}><Pin size={14} class={chat.pinned?'pin-icon is-pinned':'pin-icon'}/></button></div>
    {/if}
  </div>
{/snippet}
<style>
.recent-section{margin-top:18px}.recent-actions{display:flex;gap:2px}.recent-actions button{width:26px;height:26px;justify-content:center}.recent-actions .filtered{color:var(--primary);background:var(--surface-hover)}.recent-heading{padding-bottom:4px}.recent-heading :global(.collapsed-chevron){transform:rotate(-90deg)}
.recent-menu{position:fixed;inset:auto;margin:0;width:200px;max-width:calc(100vw - 16px);padding:6px;border:1px solid var(--color-border);border-radius:12px;background:var(--surface-2);color:var(--color-text);box-shadow:0 12px 32px #0007}.recent-menu button{display:flex;align-items:center;justify-content:space-between;width:100%;gap:10px;padding:9px 10px;font-size:13px;text-align:left}
:global(.pin-icon){transform:rotate(35deg)}:global(.pin-icon.is-pinned){transform:none;fill:currentColor}

.chat-mode-tag{display:none;font-size:11px;font-style:normal;color:var(--color-text-muted);white-space:nowrap;flex:none}.tree-chat:hover .chat-mode-tag,.tree-chat:focus-within .chat-mode-tag{display:inline}

.session-picker-trigger{display:flex;align-items:center;gap:7px;min-width:0;max-width:calc(100% - 25px);padding:6px;text-align:left}.session-picker-trigger strong{font-size:18px;font-weight:600;line-height:26px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.session-picker-trigger :global(svg){flex:none}.conversation-picker{position:fixed;inset:auto;margin:0;width:min(260px,calc(100vw - 16px));max-height:calc(100dvh - 72px);overflow:auto;padding:6px;border:1px solid var(--color-border);border-radius:12px;background:var(--color-bg);color:var(--color-text);box-shadow:0 10px 32px #0005}.picker-list{max-height:50dvh;overflow:auto}.picker-chat{display:flex;align-items:center;gap:10px;width:100%;text-align:left;padding:10px;border-radius:8px}.picker-chat[aria-current=page]{background:var(--surface-hover)}.picker-chat>span{flex:1;min-width:0}.picker-chat strong,.picker-chat small{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.picker-chat strong{font-size:14px;font-weight:500}.picker-chat small{font-size:11px;color:var(--color-text-muted);margin-top:4px}

.project-nav{height:100%;display:flex;flex-direction:column;padding:14px 10px;color:var(--color-text);min-width:0}header{display:flex;align-items:center;justify-content:space-between;padding:2px 9px 16px;font-size:15px}button{cursor:pointer}button{border-radius:6px}button:hover{background:var(--surface-hover)}button:focus-visible,input:focus-visible{outline:2px solid var(--primary);outline-offset:1px}.new-conversation{display:flex;align-items:center;gap:10px;padding:9px;margin-bottom:14px;text-align:left;font-size:12px}.nav-scroll{flex:1;min-height:0;overflow-y:auto;overflow-x:hidden}.section-heading{display:flex;align-items:center;justify-content:space-between;color:var(--color-text-muted);padding:0 6px 8px;font-size:14px;font-weight:600}.section-heading button{display:flex;align-items:center;gap:5px;padding:4px}.project-group{margin-bottom:16px}.project-heading{display:flex;align-items:center;gap:3px;padding:0 4px}.project-heading.current{background:color-mix(in srgb,var(--color-text) 5%,transparent);border-radius:7px}.project-heading>button:last-child{padding:4px;flex:none}.project-name{display:flex;align-items:center;gap:8px;min-width:0;flex:1;text-align:left;padding:8px 4px;font-size:12px}.project-name span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.tree-chat{display:flex;align-items:center;margin:2px 0 2px 22px;border-radius:7px;min-width:0}.tree-chat.active{background:var(--surface-hover)}.chat-link{display:flex;gap:7px;align-items:center;min-width:0;flex:1;text-align:left;padding:8px 6px;font-size:12px}.chat-link>span{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1}.chat-link small{display:block;font-size:9px;color:var(--color-text-muted)}.chat-link i{width:5px;height:5px;border-radius:50%;background:#888;flex:none}.tree-chat[data-presence=online] i{background:#31a56c}.tree-chat[data-presence=working] i{background:#6a9bd4}.tree-chat[data-presence=error] i{background:#db7969}.tree-chat form{display:flex;gap:3px;width:100%;padding:5px;font-size:11px}.tree-chat input{width:0;flex:1;min-width:0;background:var(--card-bg);border:1px solid var(--color-border);padding:4px}.folder-name,.empty-project{display:flex;align-items:center;gap:5px;margin-left:28px;font-size:10px;color:var(--color-text-muted);padding:6px}.load-error{font-size:10px;color:var(--danger);overflow-wrap:anywhere;padding:7px}.muted{font-size:11px;color:var(--color-text-muted);padding:8px}
.unread-count{min-width:14px;border-radius:8px;background:#55749a;color:white;font:9px/14px system-ui;text-align:center;padding:0 3px}

.project-info-trigger{padding:4px;flex:none}.project-info-card{position:fixed;inset:auto;margin:0;width:min(340px,calc(100vw - 16px));max-height:calc(100dvh - 16px);overflow:auto;padding:15px;border:1px solid var(--color-border);border-radius:12px;background:var(--color-bg);color:var(--color-text);box-shadow:0 12px 36px #0007;font-size:12px}.project-info-card p{overflow-wrap:anywhere;color:var(--color-text-muted);margin:9px 0}.project-info-card button{padding:7px;background:var(--surface-hover)}

.project-name,.chat-link,.new-conversation{font-size:14px}.project-name{padding-top:7px;padding-bottom:7px}.project-group{margin-bottom:10px}.tree-chat{margin:2px 0 2px 4px}.chat-link{padding:7px 8px;gap:10px}.chat-link i{width:6px;height:6px}.project-heading .project-info-trigger,.project-heading>button:last-child,.chat-row-actions{opacity:0;pointer-events:none}.project-heading:hover .project-info-trigger,.project-heading:hover>button:last-child,.project-heading:focus-within .project-info-trigger,.project-heading:focus-within>button:last-child,.tree-chat:hover .chat-row-actions,.tree-chat:focus-within .chat-row-actions{opacity:1;pointer-events:auto}.chat-row-actions{display:flex;align-items:center;flex:none;gap:1px;margin-right:4px}.chat-row-actions button{display:grid;place-items:center;width:24px;height:26px;color:var(--color-text-muted)}.chat-row-actions button.pinned{color:#88b5f8}.chat-actions-menu{position:fixed;inset:auto;margin:0;width:210px;max-width:calc(100vw - 16px);padding:5px;border:1px solid var(--color-border);border-radius:11px;background:var(--color-bg);color:var(--color-text);box-shadow:0 12px 32px #0007}.chat-actions-menu button{display:flex;align-items:center;gap:10px;padding:10px;width:100%;font-size:13px;text-align:left}
.tree-chat{transition:background-color .12s}.tree-chat:hover,.tree-chat:focus-within{background:var(--surface-hover)}.tree-chat button:hover{background:transparent}.tree-chat:hover .chat-link,.tree-chat:hover .chat-row-actions button{color:var(--color-text)}.chat-row-actions button:hover{color:var(--color-text);background:color-mix(in srgb,var(--color-text) 10%,transparent)}.tree-chat.archived-row{opacity:.72}
.chat-actions-menu hr{border:0;border-top:1px solid var(--color-border);margin:4px 2px}.chat-actions-menu button.danger{color:#e5786d}.chat-actions-menu button.danger.armed{background:#e5786d22;font-weight:600}.archived-toggle{display:flex;align-items:center;gap:6px}
@media(hover:none){.project-heading .project-info-trigger,.project-heading>button:last-child,.chat-row-actions{opacity:1;pointer-events:auto}}
</style>
