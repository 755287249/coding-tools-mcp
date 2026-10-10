<script lang="ts">
  import { onMount, tick, setContext, type Snippet } from 'svelte';
  import { writable } from 'svelte/store';
  import { MOBILE_CHAT_HEADER, type MobileChatHeader } from '$lib/chat/mobile-header';
  import DiscussionGroups from '$lib/components/chat/DiscussionGroups.svelte';
  import Users from '@lucide/svelte/icons/users';
  import BrowserShareDialog from './BrowserShareDialog.svelte';
  import Globe from '@lucide/svelte/icons/globe';
  import {localDesktop} from '$lib/api/distribution';
  import MobileSheet from './MobileSheet.svelte';
  import { mobileViewport } from '$lib/chat/mobile-viewport';
  import Menu from '@lucide/svelte/icons/menu';
  import Search from '@lucide/svelte/icons/search';
  import SquarePen from '@lucide/svelte/icons/square-pen';
  import Settings from '@lucide/svelte/icons/settings';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { appUrl, routePath } from '$lib/app-path';
  import Home from '@lucide/svelte/icons/house';
  import PanelLeft from '@lucide/svelte/icons/panel-left';
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import ArrowRight from '@lucide/svelte/icons/arrow-right';
  import Images from '@lucide/svelte/icons/images';
  import Clock from '@lucide/svelte/icons/clock';
  import Library from '@lucide/svelte/icons/library';
  import Blocks from '@lucide/svelte/icons/blocks';
  import Ellipsis from '@lucide/svelte/icons/ellipsis';
  import WandSparkles from '@lucide/svelte/icons/wand-sparkles';
  import FolderPlus from '@lucide/svelte/icons/folder-plus';
  import ProjectNavigator from '$lib/components/chat/ProjectNavigator.svelte';
  import ConversationSearch from '$lib/components/chat/ConversationSearch.svelte';
  import AssetLibrary from '$lib/components/chat/AssetLibrary.svelte';
  import ScheduledTasks from '$lib/components/chat/ScheduledTasks.svelte';
  import WorkspaceFeatureControls from '$lib/components/workspace/WorkspaceFeatureControls.svelte';
  import LanguageSelect from '$lib/components/LanguageSelect.svelte';
  import ThemeToggle from '$lib/components/ThemeToggle.svelte';
  import WindowTitlebar from '$lib/components/WindowTitlebar.svelte';
  import { navigationWidth, chatLocation } from '$lib/chat/location';
  import { workspaces } from '$lib/stores/app';
  import { workspaceFolders } from '$lib/types';
  import { localChat } from '$lib/api/chat';
  import { getBackend } from '$lib/backend';
  import { isDesktopWindow } from '$lib/stores/glass';
  import { newAppWindow, closeAppWindow, quitApp, toggleFullscreen } from '$lib/backend/window';
  import { startScheduleRunner } from '$lib/chat/schedules';
  import { t } from '$lib/i18n';
  import type { MessageKey } from '$lib/i18n/catalog';
  let { children, settingsNav, onAddWorkspace, onQuickSetup }: { children: Snippet; settingsNav?: Snippet; onQuickSetup?: () => void; onAddWorkspace?: () => void | Promise<void> } = $props();
  let shareOpen=$state(false);
  let pinned=$state(true), hovered=$state(false), recent=$state(false), searchOpen=$state(false), settingsOpen=$state(false);
  let assetSearchRequest=$state(0);
  let width=$state(280), error=$state(''), creating=$state(false);
  let drag: {x:number;width:number}|null=null;
  let mobileNav=$state(false),mobileScreen=$state(false),mobileMore=$state(false);
  const mobileChat=writable<MobileChatHeader|null>(null);
  setContext(MOBILE_CHAT_HEADER,{header:mobileChat,newChat:()=>newChat()});
  let menu=$state(''), menuElement=$state<HTMLDivElement>(), menuLeft=$state(0), menuTop=$state(0);
  let editor:HTMLElement|null=null;
  let editorSelection:{start:number;end:number}|null=null;
  let zoom=$state(100);
  const desktop=isDesktopWindow();
  const groups=$derived($workspaces.flatMap(workspace=>workspaceFolders(workspace).map(folder=>({workspace,folder,key:JSON.stringify([workspace.id,folder.id])}))));
  const group=$derived(groups.find(g=>g.workspace.id===$page.params.id&&g.folder.id===$page.url.searchParams.get('folder'))??groups.find(g=>g.workspace.id===$page.params.id)??groups[0]);
  const panel=$derived($page.url.searchParams.get('panel')??'');
  const customizing=$derived(panel==='plugins'||panel==='skills');
  const entries=[{id:'discussions',icon:Users},{id:'assets',icon:Images},{id:'scheduled',icon:Clock},{id:'skills',icon:Library},{id:'plugins',icon:Blocks}];
  onMount(()=>{const media=matchMedia('(max-width:700px)');const resize=()=>{mobileScreen=media.matches;if(!media.matches)mobileNav=false};resize();media.addEventListener('change',resize);try{pinned=localStorage.getItem('ctmcp-nav-pinned')!=='0';width=navigationWidth(localStorage.getItem('ctmcp-nav-width'));if(media.matches)pinned=false;}catch{}const stop=startScheduleRunner();return()=>{media.removeEventListener('change',resize);stop()}});
  $effect(()=>{const route=$page.url.href;mobileNav=false;mobileMore=false;settingsOpen=false;menuElement?.hidePopover();if(typeof window!=='undefined'&&window.innerWidth<700){pinned=false;hovered=false}});
  function pin(){pinned=!pinned;hovered=false;try{localStorage.setItem('ctmcp-nav-pinned',pinned?'1':'0')}catch{}}
  function resize(value:number){width=navigationWidth(value);try{localStorage.setItem('ctmcp-nav-width',String(width))}catch{}}
  async function newChat(connect=false){
    if(!group){await onAddWorkspace?.();return}if(creating)return;
    creating=true;error='';const target=group;
    try{let id:string|undefined;if(connect){const result=await localChat(target.workspace.id,target.folder.id,{action:'create',title:$t('chat.1')});if(!result.session)throw new Error($t('chat.117'));id=result.session.id}
      await goto(appUrl(chatLocation(target.workspace.id,target.folder.id,id)+(connect?'&connect=1':'')),{noScroll:true});
    }catch(e){error=String(e)}finally{creating=false}
  }
  function openPanel(id:string){hovered=false;void goto(appUrl(group?`/workspace/${encodeURIComponent(group.workspace.id)}?folder=${encodeURIComponent(group.folder.id)}&panel=${id}`:`/?panel=${id}`))}
  function openFeatureTab(tab:'skills'|'hooks'|'mcp'){
    const url=new URL($page.url);url.searchParams.set('panel',tab==='skills'?'skills':'plugins');url.searchParams.delete('feature');
    if(tab==='hooks')url.searchParams.set('featureTab','hooks');else url.searchParams.delete('featureTab');
    void goto(url.pathname+url.search);
  }
  function selectGroup(key:string){const next=groups.find(g=>g.key===key);if(next)void goto(appUrl(`/workspace/${encodeURIComponent(next.workspace.id)}?folder=${encodeURIComponent(next.folder.id)}&panel=${panel}`))}
  function captureEditor(){
    const active=document.activeElement;
    if(active instanceof HTMLElement&&(active.matches('input,textarea')||active.isContentEditable)){editor=active;editorSelection=active instanceof HTMLInputElement||active instanceof HTMLTextAreaElement?{start:active.selectionStart??0,end:active.selectionEnd??0}:null;}
  }
  async function edit(command:string){
    if(!editor?.isConnected)throw new Error($t('shell.selectEditor'));
    editor.focus();if(editorSelection&&(editor instanceof HTMLInputElement||editor instanceof HTMLTextAreaElement))editor.setSelectionRange(editorSelection.start,editorSelection.end);
    if(command==='paste'){const value=await navigator.clipboard.readText();if(!document.execCommand('insertText',false,value))throw new Error($t('shell.useShortcut'));}
    else if(!document.execCommand(command))throw new Error($t('shell.useShortcut'));
  }
  type Item={key:MessageKey;shortcut?:string;run:()=>unknown;disabled?:boolean}|null;
  const menus=$derived<Record<string,Item[]>>({
    file:[{key:'shell.newWindow',run:newAppWindow},{key:'shell.newChat',shortcut:'Ctrl+N',run:()=>newChat(),disabled:creating},{key:'shell.newConversation',run:()=>newChat(true),disabled:creating},null,{key:'shell.openFolder',run:()=>onAddWorkspace?.(),disabled:!onAddWorkspace},null,{key:'Close',run:closeAppWindow,disabled:!desktop},null,{key:'shell.quit',run:quitApp,disabled:!desktop}],
    edit:[...['undo','redo','cut','copy','paste','delete','selectAll'].map(id=>({key:`shell.${id}` as MessageKey,run:()=>edit(id)})),null,{key:'Settings',run:()=>settingsOpen=true}],
    view:[{key:'Collapse sidebar',shortcut:'Ctrl+Shift+S',run:pin},{key:'chat.100',shortcut:'Ctrl+K',run:()=>{searchOpen=!searchOpen}},{key:'chat.103',run:()=>{pinned=true;recent=!recent}},null,{key:'shell.zoomIn',run:()=>zoom=Math.min(150,zoom+10)},{key:'shell.zoomOut',run:()=>zoom=Math.max(70,zoom-10)},{key:'shell.resetZoom',run:()=>zoom=100},{key:'shell.fullscreen',run:toggleFullscreen}],
    help:[{key:'shell.documentation',run:()=>window.open('https://github.com/755287249/coding-tools-mcp#readme','_blank','noopener')},{key:'shell.shortcuts',run:()=>getBackend().native.alert('Ctrl+[  /  Ctrl+]\nCtrl+Shift+S\nCtrl+N\nCtrl+K',{title:$t('shell.shortcuts')})},{key:'shell.about',run:()=>getBackend().native.alert('Coding Tools MCP',{title:$t('shell.about')})}]
  });
  async function openMenu(id:string,event:MouseEvent){menu=id;const rect=(event.currentTarget as HTMLElement).getBoundingClientRect();menuLeft=rect.left;menuTop=rect.bottom+3;menuElement?.showPopover();await tick();menuElement?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();}
  async function act(item:NonNullable<Item>){menuElement?.hidePopover();try{await item.run()}catch(e){error=String(e)}}
  function menuKeys(event:KeyboardEvent){if(!['ArrowDown','ArrowUp','Home','End'].includes(event.key))return;event.preventDefault();const buttons=[...menuElement!.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];const index=buttons.indexOf(document.activeElement as HTMLButtonElement);buttons[event.key==='Home'?0:event.key==='End'?buttons.length-1:(index+(event.key==='ArrowDown'?1:-1)+buttons.length)%buttons.length]?.focus();}
  function shortcuts(event:KeyboardEvent){if(!event.ctrlKey||event.altKey||event.isComposing)return;const key=event.key.toLowerCase();if(key==='['){event.preventDefault();history.back()}else if(key===']'){event.preventDefault();history.forward()}else if(key==='s'&&event.shiftKey){event.preventDefault();pin()}else if(key==='n'&&!event.shiftKey){event.preventDefault();void newChat()}else if(key==='k'){event.preventDefault();searchOpen=!searchOpen}}
</script>
{#if shareOpen}<BrowserShareDialog onClose={()=>shareOpen=false}/>{/if}
{#if searchOpen}<ConversationSearch onClose={()=>searchOpen=false} onNewChat={async()=>{await newChat()}} onOpenWorkspace={onAddWorkspace} canSearchFiles={!!group} onSearchFiles={()=>{assetSearchRequest++;openPanel('assets')}}/>{/if}
<svelte:window onkeydown={shortcuts} onresize={()=>{if(innerWidth<700){pinned=false;hovered=false}}}/>
{#if mobileMore}<MobileSheet title={$t('mobile.chatActions')} onClose={()=>mobileMore=false}><nav class="mobile-destinations"><button onclick={()=>{mobileMore=false;void newChat()}}><SquarePen size={19}/>{$t('shell.newChat')}</button><button onclick={()=>{mobileMore=false;searchOpen=true}}><Search size={19}/>{$t('chat.100')}</button><button onclick={()=>{mobileMore=false;settingsOpen=true}}><Settings size={19}/>{$t('Settings')}</button></nav></MobileSheet>{/if}
{#if mobileNav}<MobileSheet title="Coding Tools" edge="left" onClose={()=>mobileNav=false}>
  <nav class="mobile-destinations" aria-label={$t('chat.106')}>
    <button onclick={()=>{mobileNav=false;searchOpen=true}}><Search size={19}/>{$t('chat.100')}</button>
    <button onclick={()=>{mobileNav=false;void newChat()}}><SquarePen size={19}/>{$t('shell.newChat')}</button>
    {#each entries.filter(entry=>entry.id!=='skills') as entry}<button onclick={()=>{mobileNav=false;openPanel(entry.id)}}><entry.icon size={19}/>{$t(entry.id==='plugins'?'features.customize':`shell.${entry.id}` as MessageKey)}</button>{/each}
  </nav>
  <div class="mobile-projects"><ProjectNavigator {onAddWorkspace} {recent} onSearch={()=>{mobileNav=false;searchOpen=true}}/></div>
  {#if localDesktop()}<button class="mobile-settings" onclick={()=>{mobileNav=false;shareOpen=true}}><Globe size={20} strokeWidth={1.5}/>{$t('sharing.title')}</button>{/if}
  <button class="mobile-settings" onclick={()=>{mobileNav=false;settingsOpen=true}}><Settings size={18}/>{$t('Settings')}</button>
</MobileSheet>{/if}
<div class="shell-frame" use:mobileViewport>
<header class="mobile-topbar"><button aria-label={$t('mobile.navigation')} onclick={()=>mobileNav=true}><Menu size={21}/></button><div class="mobile-title"><strong title={$mobileChat?.title}>{$mobileChat?.title??(customizing?$t('features.customize'):panel==='assets'?$t('shell.assets'):panel==='scheduled'?$t('shell.scheduled'):'Coding Tools')}</strong>{#if $mobileChat}<span class="mobile-status"><i class:online={$mobileChat.online}></i>{$mobileChat.status}</span>{/if}</div><button aria-label={$t('mobile.chatActions')} onclick={()=>{$mobileChat?$mobileChat.openMenu():mobileMore=true}}><Ellipsis size={21}/></button></header>
<header class="shell-topbar" data-tauri-drag-region>
  <button title={`${$t('shell.back')} (Ctrl+[)`} aria-label={$t('shell.back')} onclick={()=>history.back()}><ArrowLeft size={15}/></button>
  <button title={`${$t('shell.forward')} (Ctrl+])`} aria-label={$t('shell.forward')} onclick={()=>history.forward()}><ArrowRight size={15}/></button>
  <button title={`${$t(pinned?'Collapse sidebar':'Expand sidebar')} (Ctrl+Shift+S)`} aria-label={$t(pinned?'Collapse sidebar':'Expand sidebar')} aria-expanded={pinned} onclick={pin}><PanelLeft size={16}/></button>
  {#each ['file','edit','view','help'] as id}<button class="menu-trigger" aria-haspopup="menu" onpointerdown={captureEditor} onclick={event=>openMenu(id,event)}>{$t(`shell.${id}` as MessageKey)}</button>{/each}
  <div class="title-space" data-tauri-drag-region><img src={appUrl('/app-icon.svg')} width="15" height="15" alt="" data-tauri-drag-region/><span data-tauri-drag-region>Coding Tools MCP</span></div>
  {#if desktop}<WindowTitlebar/>{/if}
</header>
<div class="menu-popup" bind:this={menuElement} popover="auto" role="menu" tabindex="-1" style:left={`${menuLeft}px`} style:top={`${menuTop}px`} onkeydown={menuKeys}>{#each menus[menu]??[] as item}{#if item}<button role="menuitem" disabled={item.disabled} onclick={()=>act(item)}><span>{$t(item.key)}</span>{#if item.shortcut}<kbd>{item.shortcut}</kbd>{/if}</button>{:else}<hr/>{/if}{/each}</div>
<div class="sx-layout unified-shell" style:zoom={`${zoom}%`}>
  <div class="navigation-area" class:pinned={pinned&&!customizing} class:peek={hovered&&!customizing} style:--nav-width={`${width}px`} onmouseleave={()=>hovered=false} role="presentation">
    <nav class="app-rail" aria-label={$t('chat.106')}>
      <button class:active={!panel&&routePath($page.url.pathname)==='/'} title={$t('chat.107')} aria-label={$t('chat.107')} onmouseenter={()=>hovered=true} onfocus={()=>hovered=true} onclick={()=>goto(appUrl('/'))}><Home size={18}/></button>
      {#each entries.filter(entry=>entry.id!=='skills') as entry}<button class:active={panel===entry.id||(entry.id==='plugins'&&panel==='skills')} title={$t(entry.id==='plugins'?'features.customize':`shell.${entry.id}` as MessageKey)} aria-label={$t(entry.id==='plugins'?'features.customize':`shell.${entry.id}` as MessageKey)} onclick={()=>openPanel(entry.id)}><entry.icon size={18}/></button>{/each}
      <button title={$t('shell.more')} aria-label={$t('shell.more')} aria-expanded={settingsOpen} onclick={()=>settingsOpen=!settingsOpen}><Ellipsis size={19}/></button>
      {#if localDesktop()}<button class="share-globe" title={$t('sharing.title')} aria-label={$t('sharing.title')} onclick={()=>shareOpen=true}><Globe size={20} strokeWidth={1.5}/></button>{/if}
    </nav>
    {#if !customizing&&!mobileScreen}<aside class="project-sidebar" inert={!pinned&&!hovered}>
      <ProjectNavigator {onAddWorkspace} {recent} onSearch={()=>searchOpen=true}/>
      <button class="resize-handle" aria-label={$t('chat.108')} title={$t('chat.108')} onpointerdown={event=>{drag={x:event.clientX,width};event.currentTarget.setPointerCapture(event.pointerId)}} onpointermove={event=>{if(drag)resize(drag.width+event.clientX-drag.x)}} onpointerup={()=>drag=null} onpointercancel={()=>drag=null} onkeydown={event=>{if(event.key==='ArrowLeft'||event.key==='ArrowRight'){event.preventDefault();resize(width+(event.key==='ArrowRight'?20:-20))}}}></button>
    </aside>{/if}
  </div>
  <main class="sx-main">
  {#if error}<div class="shell-error" role="alert">{error}<button onclick={()=>error=''} aria-label={$t('Close')}>×</button></div>{/if}
  {#if customizing}
    {#if group}<WorkspaceFeatureControls workspaceId={group.workspace.id} initialTab={panel==='skills'?'skills':$page.url.searchParams.get('featureTab')==='hooks'?'hooks':'mcp'} catalog onCatalogTab={openFeatureTab}>
      {#snippet workspacePicker()}<select aria-label={$t('chat.109')} value={group?.key??''} onchange={event=>selectGroup(event.currentTarget.value)}>{#each groups as g}<option value={g.key}>{g.workspace.name} · {g.folder.name}</option>{/each}</select>{/snippet}
    </WorkspaceFeatureControls>{:else}<section class="library-page"><h1>{$t('features.customize')}</h1><p>{$t('chat.110')}</p>{#if onAddWorkspace}<button onclick={onAddWorkspace}>{$t('chat.openWorkspace')}</button>{/if}</section>{/if}
  {:else if entries.some(entry=>entry.id===panel)}<section class="library-page"><header><h1>{$t(`shell.${panel}` as MessageKey)}</h1><select aria-label={$t('chat.109')} value={group?.key??''} onchange={event=>selectGroup(event.currentTarget.value)}>{#each groups as g}<option value={g.key}>{g.workspace.name} · {g.folder.name}</option>{/each}</select></header>
    {#if group}{#if panel==='discussions'}{#key group.key}<DiscussionGroups workspaceId={group.workspace.id} folderId={group.folder.id}/>{/key}{:else if panel==='assets'}<AssetLibrary workspaceId={group.workspace.id} folderId={group.folder.id} focusSearchRequest={assetSearchRequest}/>{:else if panel==='scheduled'}<ScheduledTasks workspaceId={group.workspace.id} folderId={group.folder.id}/>{:else}<WorkspaceFeatureControls workspaceId={group.workspace.id} initialTab={panel==='plugins'?'mcp':'skills'}/>{/if}
    {:else}<p>{$t('chat.110')}</p>{#if onAddWorkspace}<button onclick={onAddWorkspace}>{$t('shell.openFolder')}</button>{/if}{/if}
  </section>{:else}{@render children()}{/if}
  </main>
  {#if settingsOpen}
    <div class="shell-settings">
      <div class="settings-heading"><strong>{$t('shell.more')}</strong><button type="button" onclick={()=>settingsOpen=false} aria-label={$t('Close')}>×</button></div>
      <div class="settings-items">
        {#if onQuickSetup}<button type="button" class="tx-settings-link" onclick={onQuickSetup}><WandSparkles size={15} aria-hidden="true"/><span class="tx-sidebar-text">{$t("Quick setup")}</span></button>{/if}
        {#if onAddWorkspace}<button type="button" class="tx-settings-link" onclick={onAddWorkspace}><FolderPlus size={15} aria-hidden="true"/><span class="tx-sidebar-text">{$t('New workspace (separate MCP)')}</span></button>{/if}
        {#if settingsNav}{@render settingsNav()}{/if}
      </div>
      <div class="appearance"><LanguageSelect/><ThemeToggle/></div>
    </div>
  {/if}
</div>
</div>
<style>
.app-rail .share-globe{margin-top:auto}
.shell-frame{display:flex;flex-direction:column;min-height:0;height:100%;width:100%}.shell-topbar{height:38px;flex:none;display:flex;align-items:center;gap:2px;border-bottom:1px solid var(--color-border);padding-left:8px;background:var(--sidebar-bg);user-select:none}.shell-topbar>button{display:grid;place-items:center;min-width:28px;height:28px;border-radius:5px;color:var(--color-text-muted);cursor:pointer}.shell-topbar .menu-trigger{padding:0 9px;font-size:12px}.shell-topbar button:hover{background:var(--surface-hover);color:var(--color-text)}.title-space{flex:1;display:flex;justify-content:center;align-items:center;gap:7px;font-size:11px;color:var(--color-text-muted);min-width:0;overflow:hidden;white-space:nowrap}
.menu-popup{margin:0;position:fixed;width:240px;padding:5px;border:1px solid var(--color-border);border-radius:9px;background:var(--color-bg);color:var(--color-text);box-shadow:0 12px 35px #0005}.menu-popup button{display:flex;justify-content:space-between;align-items:center;width:100%;padding:7px 10px;border-radius:5px;font-size:12px;cursor:pointer}.menu-popup button:hover,.menu-popup button:focus-visible{background:var(--surface-hover);outline:none}.menu-popup button:disabled{opacity:.4;cursor:default}.menu-popup kbd{font-size:10px;color:var(--color-text-muted)}.menu-popup hr{border:0;border-top:1px solid var(--color-border);margin:5px}
.unified-shell{position:relative;background:transparent;flex:1;min-height:0;height:auto}.navigation-area{position:relative;flex:none;width:48px;display:flex;z-index:40}.navigation-area.pinned{width:calc(48px + var(--nav-width))}.app-rail{width:48px;flex:none;display:flex;flex-direction:column;align-items:center;gap:8px;padding:12px 5px;background:var(--sidebar-bg);border-right:1px solid var(--color-border)}.app-rail button{display:grid;place-items:center;width:34px;height:34px;border-radius:9px;color:var(--color-text-muted);cursor:pointer}.app-rail button:hover,.app-rail button.active{background:var(--surface-hover);color:var(--color-text)}.app-rail button:focus-visible{outline:2px solid var(--primary)}.project-sidebar{position:absolute;left:48px;top:0;bottom:0;width:var(--nav-width);background:var(--sidebar-bg);border-right:1px solid var(--color-border);transform:translateX(-12px);opacity:0;visibility:hidden;transition:transform .16s,opacity .16s}.pinned .project-sidebar,.peek .project-sidebar{transform:none;opacity:1;visibility:visible}.peek:not(.pinned) .project-sidebar{box-shadow:16px 0 32px #0004;background:var(--color-bg)}.resize-handle{position:absolute;top:0;right:-3px;bottom:0;width:6px;cursor:col-resize;touch-action:none;z-index:2}.resize-handle:hover,.resize-handle:focus-visible{background:var(--color-border);outline:none}.sx-main{background:transparent;min-width:0}.shell-settings{position:absolute;left:12px;top:255px;z-index:80;max-height:calc(100% - 270px);overflow:auto;min-width:240px;padding:12px;border:1px solid var(--color-border);border-radius:12px;background:var(--color-bg);box-shadow:0 10px 30px #0005}.settings-heading{display:flex;justify-content:space-between;margin-bottom:12px;font-size:13px}.settings-heading button{cursor:pointer}.appearance{display:flex;gap:8px;margin-top:12px;border-top:1px solid var(--color-border);padding-top:12px}.library-page{height:100%;overflow:auto;padding:28px}.library-page>header{display:flex;gap:16px;align-items:center;justify-content:space-between;margin-bottom:28px}.library-page h1{font-size:24px;font-weight:600}.library-page select{min-width:0;max-width:60%;padding:8px;border:1px solid var(--color-border);border-radius:7px;background:var(--card-bg);font-size:12px}.shell-error{padding:10px;display:flex;justify-content:space-between;color:var(--danger);font-size:12px;overflow-wrap:anywhere}
@media(max-width:700px){.navigation-area.pinned{width:48px}.project-sidebar{width:min(var(--nav-width),calc(100vw - 70px));box-shadow:16px 0 32px #0004}.title-space{display:none}.shell-topbar .menu-trigger{padding:0 5px}.library-page{padding:18px}.library-page>header{align-items:flex-start;flex-direction:column}.library-page select{max-width:100%}.shell-settings{top:12px;max-height:calc(100% - 24px)}}@media(prefers-reduced-motion:reduce){.project-sidebar{transition:none}}

  .shell-settings{width:272px;min-width:0;max-width:calc(100vw - 24px);color:var(--color-text);font-size:13px;line-height:20px}
  :global(html.is-glass) .shell-settings{background:rgb(var(--glass-base))}
  .settings-heading{align-items:center;margin:0 0 8px;padding-left:10px;font-size:13px;line-height:20px}
  .settings-heading strong{font-weight:600}
  .settings-heading button{width:28px;height:28px;border-radius:6px;color:var(--color-text-muted)}
  .settings-heading button:hover{background:var(--surface-hover);color:var(--color-text)}
  .settings-items{display:grid;gap:2px}
  .settings-items :global(.tx-settings-link){display:flex;align-items:center;gap:10px;width:100%;height:auto;min-height:36px;padding:8px 10px;font-size:13px;font-weight:500;line-height:20px;color:var(--color-text);text-align:left}
  .settings-items :global(.tx-settings-link svg){width:15px;height:15px;flex:none;color:inherit;opacity:.85}
  .settings-items :global(.tx-sidebar-text){white-space:normal;overflow-wrap:anywhere}
  .settings-items :global(.tx-settings-link:hover){background:var(--surface-hover)}
  .settings-items :global(.tx-settings-link:focus-visible),.settings-heading button:focus-visible{outline:2px solid var(--primary);outline-offset:-2px}
  .appearance{align-items:center;margin-top:10px;padding-top:10px}
  .appearance :global(.tx-lang-select){flex:1;min-width:0}
  .appearance :global(select){font-size:13px;color:var(--color-text);line-height:20px}

.mobile-topbar{display:none}.mobile-destinations{display:grid;gap:3px;padding:4px 12px 16px;border-bottom:1px solid var(--color-border)}.mobile-destinations button,.mobile-settings{display:flex;align-items:center;gap:14px;min-height:44px;padding:10px 12px;border-radius:12px;font-size:14px;text-align:left;cursor:pointer}.mobile-destinations button:hover,.mobile-settings:hover{background:var(--surface-hover)}.mobile-projects{flex:1;min-height:0;overflow:auto}.mobile-projects :global(.project-nav){height:auto;min-height:100%}.mobile-projects :global(.project-nav>header),.mobile-projects :global(.new-conversation){display:none}.mobile-settings{flex:none;margin:10px 14px}.mobile-destinations button:focus-visible,.mobile-settings:focus-visible{outline:2px solid var(--primary);outline-offset:-2px}
@media(max-width:700px){
 .shell-frame{background:rgb(var(--glass-base));position:fixed;top:var(--mobile-viewport-top,0px);left:0;right:0;height:var(--mobile-viewport-height,100dvh);max-height:100dvh;border-radius:0!important}
 .shell-topbar,.navigation-area{display:none}.mobile-topbar{display:flex;flex:none;align-items:center;justify-content:space-between;gap:10px;padding:8px 12px;padding-top:max(8px,env(safe-area-inset-top));min-height:54px;background:var(--color-bg)}
 .mobile-topbar strong{font-size:14px;font-weight:600;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.mobile-topbar button{width:40px;height:40px;display:grid;place-items:center;flex:none;border-radius:50%;background:var(--surface-hover);cursor:pointer}.mobile-topbar button:focus-visible{outline:2px solid var(--primary)}
 .unified-shell{width:100%;zoom:1!important}.sx-main{width:100%}.shell-settings{top:62px;left:12px;max-height:calc(100% - 78px);width:calc(100% - 24px)}
}

.mobile-title{display:grid;gap:3px;min-width:0;flex:1;text-align:center}.mobile-title strong{display:block;max-width:100%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.mobile-status{display:flex;align-items:center;justify-content:center;gap:5px;min-width:0;font-size:10px;line-height:14px;color:var(--color-text-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.mobile-status i{width:5px;height:5px;border-radius:50%;background:var(--color-text-muted);flex:none}.mobile-status i.online{background:#49a476}
</style>
