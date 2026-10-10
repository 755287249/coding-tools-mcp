<script lang="ts">
  import { tick } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { appUrl } from '$lib/app-path';
  import { chatLocation } from '$lib/chat/location';
  import { t } from '$lib/i18n';
  import { workspaces } from '$lib/stores/app';
  import { connectionActions } from '$lib/stores/ui-mode';
  import { workspaceFolders } from '$lib/types';
  import { localChat, type ChatFile } from '$lib/api/chat';
  import { getBackend, type ExtensionInventoryPayload, type McpServerInventoryItem } from '$lib/backend';
  import Folder from '@lucide/svelte/icons/folder';
  import FolderPlus from '@lucide/svelte/icons/folder-plus';
  import Library from '@lucide/svelte/icons/library';
  import Puzzle from '@lucide/svelte/icons/puzzle';
  import Monitor from '@lucide/svelte/icons/monitor';
  import Search from '@lucide/svelte/icons/search';
  import File from '@lucide/svelte/icons/file';
  import FileImage from '@lucide/svelte/icons/file-image';
  import FileCode from '@lucide/svelte/icons/file-code';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Check from '@lucide/svelte/icons/check';
  import Settings from '@lucide/svelte/icons/settings';
  let {workspaceId,folderId,selectedFolderName,disabled=false,onOpenChange,onBeforeNavigate,onSelectFolder,onAttach,onChooseLocal,onReference,onPlugins}: {
    workspaceId:string;folderId:string;selectedFolderName:string;disabled?:boolean;onOpenChange:(open:boolean)=>void;onBeforeNavigate:()=>void;onSelectFolder:(id:string)=>void;onAttach:(file:ChatFile,chatId:string)=>void|Promise<void>;onChooseLocal:()=>void;onReference:()=>void;onPlugins?: (folderId:string)=>void;
  }=$props();
  type Menu='projects'|'files'|'plugins'|'local';
  let menu=$state<Menu>('projects'), popup=$state<HTMLDivElement>(), search=$state<HTMLInputElement>(), query=$state(''), open=$state(false), loading=$state(false), saving=$state(false), error=$state(''), left=$state(0), top=$state(0);
  let anchor:HTMLElement|null=null, generation=0;
  type Entry={file:ChatFile;chatId:string};
  let files=$state<Entry[]>([]), plugins=$state<ExtensionInventoryPayload|null>(null);
  const capabilities=getBackend().capabilities;
  const groups=$derived($workspaces.flatMap(workspace=>workspaceFolders(workspace).map(folder=>({workspace,folder}))));
  const needle=$derived(query.trim().toLocaleLowerCase());
  const matchingGroups=$derived(groups.filter(g=>`${g.workspace.name} ${g.folder.name} ${g.folder.path}`.toLocaleLowerCase().includes(needle)));
  const matchingFiles=$derived(files.filter(item=>`${item.file.name} ${item.file.path}`.toLocaleLowerCase().includes(needle)));
  const matchingPlugins=$derived((plugins?.mcpServers??[]).filter(item=>(!item.folderId||item.folderId===folderId)&&`${item.name} ${item.provider}`.toLocaleLowerCase().includes(needle)));
  function position(){if(!anchor||!popup?.matches(':popover-open'))return;const r=anchor.getBoundingClientRect();left=Math.max(8,Math.min(r.left,innerWidth-popup.offsetWidth-8));const below=r.bottom+6;top=below+popup.offsetHeight<=innerHeight-8?below:Math.max(8,r.top-popup.offsetHeight-6);}
  function close(){popup?.hidePopover();}
  function toggled(){open=!!popup?.matches(':popover-open');onOpenChange(open);if(!open){generation++;loading=false;}else position();}
  $effect(()=>{const scope=[workspaceId,folderId];close();files=[];plugins=null;error='';generation++;});
  $effect(()=>{const contents=[query,files.length,plugins,loading,error];if(open)void tick().then(position);});
  async function show(next:Menu,event:MouseEvent){
    if(disabled)return;
    if(open&&menu===next){close();return;}
    menu=next;query='';error='';anchor=event.currentTarget as HTMLElement;
    await tick();popup?.showPopover();open=true;onOpenChange(true);position();search?.focus({preventScroll:true});
    const gen=++generation,ws=workspaceId,folder=folderId;
    if(next==='files'){
      loading=true;files=[];
      try{
        const result=await localChat(ws,folder,{action:'list'});const found=new Map<string,Entry>();
        for(const summary of result.sessions??[]){
          if(gen!==generation)return;
          const {session}=await localChat(ws,folder,{action:'read',chat_id:summary.id});
          if(gen!==generation)return;
          for(const message of session?.messages??[])for(const file of message.attachments??[])found.set(file.path,{file,chatId:summary.id});
          files=[...found.values()];
        }
      }catch(e){if(gen===generation)error=String(e)}finally{if(gen===generation)loading=false}
    }else if(next==='plugins'){
      loading=true;
      try{const inventory=await getBackend().workspaceFeatures.extensions(ws);if(gen===generation)plugins=inventory;}
      catch(e){if(gen===generation)error=String(e)}finally{if(gen===generation)loading=false}
    }
  }
  function chooseProject(ws:string,folder:string){close();onBeforeNavigate();if(ws===workspaceId)onSelectFolder(folder);else void goto(appUrl(chatLocation(ws,folder)),{noScroll:true});}
  async function attach(entry:Entry){close();await onAttach(entry.file,entry.chatId);}
  async function togglePlugin(server?:McpServerInventoryItem){
    if(saving||!plugins)return;const gen=generation,ws=workspaceId;saving=true;error='';
    try{const api=getBackend().workspaceFeatures;if(server)await api.setExtensionEnabled(ws,'mcp',server.key,!server.selected);else await api.setExtensionActive(ws,'mcp',!plugins.mcpActive);const inventory=await api.extensions(ws);if(gen===generation)plugins=inventory;}
    catch(e){if(gen===generation)error=String(e)}finally{saving=false}
  }
  function browseAssets(){close();onBeforeNavigate();const url=new URL($page.url);url.searchParams.set('panel','assets');url.searchParams.set('folder',folderId);if(!url.pathname.includes('/workspace/')){url.pathname=appUrl(`/workspace/${encodeURIComponent(workspaceId)}`);url.searchParams.set('tab','chat');}void goto(url,{noScroll:true});}
</script>
<svelte:window onresize={position} onscrollcapture={position}/>
{#if workspaceId}<button class="composer-tool project-tool" type="button" disabled={disabled} aria-expanded={open&&menu==='projects'} onclick={event=>show('projects',event)}><Folder size={14}/><span>{selectedFolderName||$t('chat.project')}</span><ChevronDown size={12}/></button>{/if}
<button class="composer-tool" type="button" disabled={disabled||!folderId} aria-expanded={open&&menu==='files'} onclick={event=>show('files',event)}><Library size={14}/>{$t('chat.files')}<ChevronDown size={12}/></button>
<button class="composer-tool" type="button" disabled={disabled||!workspaceId||!capabilities.workspaceFeatureControls} aria-expanded={open&&menu==='plugins'} onclick={event=>show('plugins',event)}><Puzzle size={14}/>{$t('chat.plugins')}<ChevronDown size={12}/></button>
<button class="composer-tool local-tool" type="button" disabled={disabled||!folderId} title={$t('chat.122')} aria-label={$t('chat.122')} aria-expanded={open&&menu==='local'} onclick={event=>show('local',event)}><Monitor size={15}/></button>
<div class="composer-menu" popover="auto" bind:this={popup} ontoggle={toggled} style:left={`${left}px`} style:top={`${top}px`}>
  {#if menu!=='local'}<label class="tool-search"><Search size={14}/><input bind:this={search} bind:value={query} aria-label={$t(menu==='projects'?'chat.searchProjects':menu==='plugins'?'chat.searchPlugins':'shell.findAssets')} placeholder={$t(menu==='projects'?'chat.searchProjects':menu==='plugins'?'chat.searchPlugins':'shell.findAssets')}/></label>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
  {#if menu==='projects'}
    {#if capabilities.workspaceLifecycle}<button type="button" onclick={()=>{close();onBeforeNavigate();connectionActions.newConnection?.()}}><FolderPlus size={15}/>{$t('Choose a project folder')}</button>{/if}
    <div class="menu-items">{#each matchingGroups as g (g.workspace.id+g.folder.id)}<button type="button" class:chosen={g.workspace.id===workspaceId&&g.folder.id===folderId} onclick={()=>chooseProject(g.workspace.id,g.folder.id)}><Folder size={15}/><span>{g.folder.name}<small>{g.workspace.name}</small></span>{#if g.workspace.id===workspaceId&&g.folder.id===folderId}<Check size={14}/>{/if}</button>{:else}<p>{$t('chat.selectWorkspace')}</p>{/each}</div>
  {:else if menu==='files'}
    <div class="menu-items">{#each matchingFiles as item (item.file.path)}<button type="button" onclick={()=>attach(item)} title={item.file.path}>{#if item.file.mime.startsWith('image/')}<FileImage size={15}/>{:else if /\.(rs|py|[cm]?[jt]sx?|html|css|json)$/.test(item.file.name)}<FileCode size={15}/>{:else}<File size={15}/>{/if}<span>{item.file.name}</span></button>{:else}{#if !loading}<p>{$t('shell.noAssets')}</p>{/if}{/each}</div>
    <hr/><button type="button" onclick={browseAssets}><Library size={15}/>{$t('chat.browseAssets')}</button>
  {:else if menu==='plugins'}
    {#if plugins}<button type="button" role="switch" aria-checked={plugins.mcpActive} disabled={saving} onclick={()=>togglePlugin()}><Puzzle size={15}/><span>{$t('MCP servers')}</span>{#if plugins.mcpActive}<Check size={14}/>{/if}</button><hr/>{/if}
    <div class="menu-items">{#each matchingPlugins as plugin (plugin.key)}<button type="button" role="switch" aria-checked={plugin.selected} disabled={saving||!plugins?.mcpActive||!plugin.supported||!plugin.sourceEnabled} onclick={()=>togglePlugin(plugin)}><Puzzle size={15}/><span>{plugin.name}<small>{plugin.provider}</small></span>{#if plugin.selected}<Check size={14}/>{/if}</button>{:else}{#if !loading}<p>{$t('chat.noPlugins')}</p>{/if}{/each}</div>
    {#if onPlugins}<hr/><button type="button" onclick={()=>{close();onBeforeNavigate();onPlugins?.(folderId)}}><Settings size={15}/>{$t('chat.managePlugins')}</button>{/if}
  {:else}
    <button type="button" onclick={()=>{close();onChooseLocal()}}><File size={15}/>{$t('chat.chooseLocalFile')}</button><button type="button" onclick={()=>{close();onReference()}}><Folder size={15}/>{$t('chat.122')}</button>
  {/if}
  {#if loading}<p role="status">{$t('Loading…')}</p>{/if}
</div>
<style>
.composer-tool{display:flex;align-items:center;gap:6px;padding:5px 8px;border-radius:6px;min-height:28px;cursor:pointer;color:inherit;font-size:11px}.composer-tool:hover:enabled,.composer-tool[aria-expanded=true]{background:#ffffff12}.composer-tool:disabled{opacity:.45;cursor:default}.project-tool{min-width:0;max-width:40%}.project-tool span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.local-tool{margin-left:auto}.composer-menu{position:fixed;inset:auto;margin:0;width:min(285px,calc(100vw - 16px));max-height:min(380px,calc(100dvh - 16px));overflow:auto;padding:6px;background:#282828;color:#ddd;border:1px solid #ffffff20;border-radius:13px;box-shadow:0 12px 30px #0007;font-size:12px}.tool-search{display:flex;align-items:center;gap:8px;padding:9px 8px;color:#999}.tool-search input{min-width:0;flex:1;background:transparent;border:0;font-size:12px;color:#eee}.composer-menu button{width:100%;display:flex;align-items:center;gap:8px;min-height:34px;padding:7px 9px;border-radius:7px;text-align:left;cursor:pointer}.composer-menu button:disabled{opacity:.45;cursor:default}.composer-menu button:hover:enabled,.composer-menu button.chosen{background:#ffffff12}.composer-menu button>span{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.composer-menu small{display:block;color:#999;font-size:10px;margin-top:3px}.composer-menu :global(svg),.composer-tool :global(svg){flex:none}.menu-items{max-height:240px;overflow:auto}.composer-menu p{font-size:11px;color:#aaa;padding:10px;overflow-wrap:anywhere}.composer-menu [role=alert]{color:#ef9090}.composer-menu hr{border:0;border-top:1px solid #ffffff15;margin:5px}.composer-tool:focus-visible,.composer-menu button:focus-visible,.tool-search input:focus-visible{outline:2px solid var(--primary);outline-offset:-1px}

@media(max-width:700px){.composer-menu{left:12px!important;top:auto!important;bottom:calc(env(safe-area-inset-bottom) + 16px);width:calc(100vw - 24px);max-height:55dvh;border-radius:22px;padding:10px;background:var(--surface-2);color:var(--color-text);border-color:var(--color-border)}.composer-menu button{min-height:44px;font-size:13px}.composer-menu small,.composer-menu p{color:var(--color-text-muted)}.tool-search input{font-size:16px;color:var(--color-text)}.composer-menu hr{border-color:var(--color-border)}.composer-menu button:hover:enabled,.composer-menu button.chosen{background:var(--surface-hover)}:global(.mobile-attach-options) .composer-tool{max-width:none;width:100%;min-height:48px;padding:9px 12px;font-size:14px;gap:15px;color:var(--color-text)}:global(.mobile-attach-options) .composer-tool :global(svg:first-child){width:22px;height:22px}:global(.mobile-attach-options) .local-tool{display:none}}
</style>
