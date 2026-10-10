<script lang="ts">
  import { mobileViewport } from "$lib/chat/mobile-viewport";
  import { onMount, tick } from 'svelte';
  import { goto } from '$app/navigation';
  import { appUrl } from '$lib/app-path';
  import { workspaces } from '$lib/stores/app';
  import { workspaceFolders } from '$lib/types';
  import { localChat } from '$lib/api/chat';
  import { chatLocation } from '$lib/chat/location';
  import { searchConversations, type SearchConversation } from '$lib/chat/search';
  import { t } from '$lib/i18n';
  import Search from '@lucide/svelte/icons/search';
  import SquarePen from '@lucide/svelte/icons/square-pen';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import X from '@lucide/svelte/icons/x';
  let { onClose, onNewChat, onOpenWorkspace, onSearchFiles, canSearchFiles }: {
    onClose: () => void; onNewChat: () => void | Promise<void>;
    onOpenWorkspace?: () => void | Promise<void>; onSearchFiles: () => void; canSearchFiles: boolean;
  } = $props();
  let dialog: HTMLDialogElement;
  let input: HTMLInputElement;
  let query=$state(''), mode=$state<'all'|'work'|'group'>('all');
  let items=$state<SearchConversation[]>([]), loading=$state(true), failures=$state<string[]>([]), retry=$state(0);
  let active=$state(0), error=$state('');
  const groups=$derived($workspaces.flatMap(workspace=>workspaceFolders(workspace).map(folder=>({workspace,folder}))));
  const matches=$derived(searchConversations(items,query,mode));
  const actions=$derived([
    {key:'chat.1' as const, icon:SquarePen, shortcut:'Ctrl+N', run:onNewChat, enabled:true},
    {key:'chat.openWorkspace' as const, icon:FolderOpen, shortcut:'Ctrl+O', run:onOpenWorkspace, enabled:!!onOpenWorkspace},
    {key:'chat.searchFiles' as const, icon:Search, shortcut:'Ctrl+P', run:onSearchFiles, enabled:canSearchFiles},
  ].filter(action=>action.enabled));
  onMount(()=>{
    const returnFocus=document.activeElement instanceof HTMLElement?document.activeElement:null;
    dialog.showModal();input.focus();
    return()=>{if(dialog.open)dialog.close();queueMicrotask(()=>{if(returnFocus?.isConnected)returnFocus.focus()});};
  });
  $effect(()=>{
    const targets=groups, attempt=retry;let cancelled=false;
    items=[];failures=[];loading=true;
    void (async()=>{
      for(let offset=0;offset<targets.length;offset+=4){
        if(cancelled)return;
        const results=await Promise.all(targets.slice(offset,offset+4).map(async ({workspace,folder})=>{
          try {const result=await localChat(workspace.id,folder.id,{action:'list'});return {items:(result.sessions??[]).map(chat=>({workspaceId:workspace.id,workspaceName:workspace.name,folderId:folder.id,folderName:folder.name,chat})),failure:''};}
          catch {return {items:[],failure:`${workspace.name} · ${folder.name}`};}
        }));
        if(cancelled)return;
        items=[...items,...results.flatMap(result=>result.items)];failures=[...failures,...results.map(result=>result.failure).filter(Boolean)];
      }
      if(!cancelled)loading=false;
    })();
    return()=>{cancelled=true};
  });
  $effect(()=>{const text=query, filter=mode;active=0;});
  $effect(()=>{const count=matches.length+actions.length;if(active>=count)active=Math.max(0,count-1);});
  async function choose(item:SearchConversation) {
    try {await goto(appUrl(chatLocation(item.workspaceId,item.folderId,item.chat.id)),{noScroll:true});onClose();}
    catch(e){error=String(e);}
  }
  async function runAction(index:number) {
    const action=actions[index];if(!action?.run)return;
    // Dismiss the modal before opening a native picker or navigating to another surface.
    onClose();await action.run();
  }
  function selectActive(){if(active<matches.length)void choose(matches[active]);else void runAction(active-matches.length);}
  async function keys(event:KeyboardEvent) {
    if(event.isComposing)return;
    if(event.key==='Escape'){event.preventDefault();event.stopPropagation();onClose();return;}
    if((event.ctrlKey||event.metaKey)&&!event.altKey){
      const key=event.key.toLowerCase();
      if(key==='k'){event.preventDefault();event.stopPropagation();onClose();return;}
      const index=actions.findIndex(action=>action.shortcut.toLowerCase()===`ctrl+${key}`);
      if(index>=0){event.preventDefault();event.stopPropagation();void runAction(index);return;}
    }
    if(event.altKey && /^[1-9]$/.test(event.key)){
      const item=matches[Number(event.key)-1];if(item){event.preventDefault();event.stopPropagation();void choose(item);}return;
    }
    if(event.key==='ArrowDown'||event.key==='ArrowUp'){
      event.preventDefault();event.stopPropagation();const count=matches.length+actions.length;
      if(count){active=(active+(event.key==='ArrowDown'?1:-1)+count)%count;await tick();dialog.querySelector<HTMLElement>(`[data-choice="${active}"]`)?.scrollIntoView({block:'nearest'});}return;
    }
    if(event.key==='Enter' && event.target===input){event.preventDefault();event.stopPropagation();selectActive();}
  }
</script>

<dialog use:mobileViewport class="conversation-search" bind:this={dialog} aria-label={$t('chat.100')} onclose={onClose} oncancel={event=>{event.preventDefault();onClose()}} onkeydown={keys} onclick={event=>{if(event.target===dialog){const r=dialog.getBoundingClientRect();if(event.clientX<r.left||event.clientX>r.right||event.clientY<r.top||event.clientY>r.bottom)onClose();}}}>
  <div class="search-input-row"><input bind:this={input} bind:value={query} aria-label={$t('chat.100')} placeholder={$t('chat.searchConversations')} autocomplete="off"/><button type="button" class="close-search" onclick={onClose} aria-label={$t('Close')}><X size={16}/></button></div>
  <div class="search-results-heading"><span>{$t('chat.conversations')}</span><div class="search-modes" aria-label={$t('chat.recentFilter')}>
    {#each ['all','work','group'] as value}<button type="button" class:chosen={mode===value} aria-pressed={mode===value} onclick={()=>{mode=value as typeof mode;input.focus()}}>{$t(value==='all'?'chat.allModes':value==='work'?'chat.work':'chat.group')}</button>{/each}
  </div></div>
  <div class="search-results" aria-label={$t('chat.conversations')}>
    {#each matches as item,index (JSON.stringify([item.workspaceId,item.folderId,item.chat.id]))}
      <button type="button" class="search-result" class:active={active===index} data-choice={index} onpointermove={()=>active=index} onfocus={()=>active=index} onclick={()=>choose(item)} title={`${item.chat.title} · ${item.workspaceName} · ${item.folderName}`}>
        <span class="result-title">{item.chat.title}</span><span class="result-mode">{$t(item.chat.archived?'chat.archived':item.chat.mode==='group'?'chat.group':'chat.work')}</span><span class="result-project">{item.workspaceName}</span>{#if index<9}<kbd>{`Alt+${index+1}`}</kbd>{/if}
      </button>
    {:else}{#if !loading}<p class="search-empty">{$t('chat.noSearchMatches')}</p>{/if}{/each}
  </div>
  {#if loading}<p class="search-status" role="status">{$t('Loading…')}</p>{/if}
  {#if failures.length}<div class="search-status" role="status">{$t('chat.searchPartial')} <span title={failures.join(', ')}>{failures.length}</span><button onclick={()=>retry++}>{$t('Try again')}</button></div>{/if}
  {#if error}<p class="search-status" role="alert">{error}</p>{/if}
  <p class="quick-heading">{$t('chat.quickActions')}</p>
  <div class="search-actions">
    {#each actions as action,index}<button type="button" class:active={active===matches.length+index} data-choice={matches.length+index} onpointermove={()=>active=matches.length+index} onfocus={()=>active=matches.length+index} onclick={()=>runAction(index)}><action.icon size={16}/><span>{$t(action.key)}</span><kbd>{action.shortcut}</kbd></button>{/each}
  </div>
</dialog>

<style>
.conversation-search{margin:auto;width:min(540px,calc(100vw - 32px));max-height:calc(100dvh - 48px);overflow:auto;padding:7px;border:1px solid var(--color-border);border-radius:20px;background:color-mix(in srgb,var(--surface-3) 94%,var(--color-text) 6%);color:var(--color-text);box-shadow:0 24px 80px #0007}.conversation-search::backdrop{background:#0005}.search-input-row{display:flex;align-items:center;padding:5px 6px 8px;gap:8px}.search-input-row input{flex:1;min-width:0;background:transparent;border:0;padding:7px 0;font-size:14px;outline:none;color:var(--color-text)}.close-search{display:grid;place-items:center;padding:5px;color:var(--color-text-muted);border-radius:6px;cursor:pointer}.search-results-heading{display:flex;justify-content:space-between;align-items:center;padding:0 7px 4px;color:var(--color-text-muted);font-size:12px}.search-modes{display:flex;gap:2px}.search-modes button{font-size:11px;padding:3px 7px;border-radius:6px;cursor:pointer}.search-modes .chosen{background:var(--surface-hover);color:var(--color-text)}.search-results{max-height:min(330px,45dvh);overflow:auto}.search-result,.search-actions button{display:flex;align-items:center;width:100%;gap:9px;padding:7px 9px;min-height:32px;border-radius:11px;text-align:left;cursor:pointer;font-size:13px}.search-result.active,.search-actions .active,.search-result:hover,.search-actions button:hover{background:color-mix(in srgb,var(--color-text) 9%,transparent)}.result-title{min-width:0;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.result-mode{font-size:10px;color:var(--color-text-muted);flex:none}.result-project{max-width:90px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:12px;color:var(--color-text-muted)}kbd{flex:none;border-radius:5px;background:color-mix(in srgb,var(--color-text) 9%,transparent);color:var(--color-text-secondary);padding:0 5px;font:11px/17px system-ui}.quick-heading{font-size:12px;color:var(--color-text-muted);padding:12px 7px 5px}.search-actions button>span{flex:1}.search-actions :global(svg){flex:none;color:var(--color-text-secondary)}.search-empty,.search-status{padding:10px 7px;font-size:12px;color:var(--color-text-muted);overflow-wrap:anywhere}.search-status button{margin-left:8px;text-decoration:underline;cursor:pointer}button:focus-visible{outline:2px solid var(--primary);outline-offset:-2px}@media(max-width:500px){.conversation-search{border-radius:16px}.result-project{max-width:65px}.result-mode{display:none}}

@media(max-width:700px){.conversation-search{position:fixed;inset:var(--mobile-viewport-top,0px) 0 auto;width:100%;height:var(--mobile-viewport-height,100dvh);max-height:var(--mobile-viewport-height,100dvh);max-width:100%;margin:0;padding:16px 14px max(14px,env(safe-area-inset-bottom));border:0;border-radius:0;background:var(--surface-2)}.conversation-search[open]{display:flex;flex-direction:column}.search-input-row{order:6;flex:none;margin-top:auto;border:1px solid var(--color-border);background:var(--surface-2);border-radius:30px;padding:8px 8px 8px 17px}.search-input-row input{font-size:16px;padding:6px 0}.close-search{width:34px;height:34px;border-radius:50%}.search-results-heading{padding:8px 4px 12px;font-size:12px}.search-modes button{padding:7px 12px;font-size:12px}.search-results{flex:1;max-height:none;min-height:0}.search-result{min-height:43px;font-size:13px}.result-project{max-width:85px;font-size:11px}.search-actions{padding-bottom:14px}.search-actions button{min-height:44px;font-size:13px}kbd{display:none}.quick-heading{font-size:11px}.search-empty{padding:60px 20px;text-align:center}}
</style>
