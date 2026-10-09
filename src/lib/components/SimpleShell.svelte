<script lang="ts">
  import { onMount, type Snippet } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { appUrl, routePath } from '$lib/app-path';
  import Home from '@lucide/svelte/icons/house';
  import PanelLeft from '@lucide/svelte/icons/panel-left';
  import Search from '@lucide/svelte/icons/search';
  import Clock from '@lucide/svelte/icons/clock';
  import WandSparkles from '@lucide/svelte/icons/wand-sparkles';
  import Settings from '@lucide/svelte/icons/settings';
  import FolderPlus from '@lucide/svelte/icons/folder-plus';
  import ProjectNavigator from '$lib/components/chat/ProjectNavigator.svelte';
  import LanguageSelect from '$lib/components/LanguageSelect.svelte';
  import ThemeToggle from '$lib/components/ThemeToggle.svelte';
  import { navigationWidth } from '$lib/chat/location';
  import { t } from '$lib/i18n';
  let { children, settingsNav, onAddWorkspace, onQuickSetup }: { children: Snippet; settingsNav?: Snippet; onQuickSetup?: () => void; onAddWorkspace?: () => void | Promise<void> } = $props();
  let pinned=$state(true), hovered=$state(false), recent=$state(false), searchOpen=$state(false), settingsOpen=$state(false);
  let width=$state(280);
  let drag: {x:number;width:number}|null=null;
  onMount(()=>{try{pinned=localStorage.getItem('ctmcp-nav-pinned')!=='0';width=navigationWidth(localStorage.getItem('ctmcp-nav-width'));if(innerWidth<700)pinned=false;}catch{}});
  $effect(()=>{const route=$page.url.href;settingsOpen=false;if(typeof window!=='undefined'&&window.innerWidth<700){pinned=false;hovered=false}});
  function pin(){pinned=!pinned;try{localStorage.setItem('ctmcp-nav-pinned',pinned?'1':'0')}catch{}}
  function resize(value:number){width=navigationWidth(value);try{localStorage.setItem('ctmcp-nav-width',String(width))}catch{}}
  function showRecent(){recent=!recent;pinned=true;}
</script>
<svelte:window onresize={()=>{if(innerWidth<700){pinned=false;hovered=false}}}/>
<div class="sx-layout unified-shell">
  <div class="navigation-area" class:pinned class:peek={hovered} style:--nav-width={`${width}px`} onmouseenter={()=>hovered=true} onmouseleave={()=>hovered=false} role="presentation">
    <nav class="app-rail" aria-label={$t('chat.106')}>
      <button class:active={routePath($page.url.pathname)==='/'} title={$t('chat.107')} aria-label={$t('chat.107')} onclick={()=>goto(appUrl('/'))}><Home size={18}/></button>
      <button title={$t(pinned?'Collapse sidebar':'Expand sidebar')} aria-label={$t(pinned?'Collapse sidebar':'Expand sidebar')} aria-expanded={pinned} onclick={pin}><PanelLeft size={17}/></button>
      <button title={$t('chat.100')} aria-label={$t('chat.100')} onclick={()=>{pinned=true;searchOpen=!searchOpen}}><Search size={17}/></button>
      <button class:active={recent} title={$t('chat.103')} aria-label={$t('chat.103')} onclick={showRecent}><Clock size={17}/></button>
      <div class="rail-spacer"></div>
      {#if onQuickSetup}<button title={$t("Quick setup")} aria-label={$t("Quick setup")} onclick={onQuickSetup}><WandSparkles size={17}/></button>{/if}
      {#if onAddWorkspace}<button title={$t('New workspace (separate MCP)')} aria-label={$t('New workspace (separate MCP)')} onclick={onAddWorkspace}><FolderPlus size={17}/></button>{/if}
      <button title={$t('Settings')} aria-label={$t('Settings')} aria-expanded={settingsOpen} onclick={()=>settingsOpen=!settingsOpen}><Settings size={17}/></button>
    </nav>
    <aside class="project-sidebar" inert={!pinned&&!hovered}>
      <ProjectNavigator {onAddWorkspace} {recent} {searchOpen}/>
      <button class="resize-handle" aria-label={$t('chat.108')} title={$t('chat.108')} onpointerdown={event=>{drag={x:event.clientX,width};event.currentTarget.setPointerCapture(event.pointerId)}} onpointermove={event=>{if(drag)resize(drag.width+event.clientX-drag.x)}} onpointerup={()=>drag=null} onpointercancel={()=>drag=null} onkeydown={event=>{if(event.key==='ArrowLeft'||event.key==='ArrowRight'){event.preventDefault();resize(width+(event.key==='ArrowRight'?20:-20))}}}></button>
    </aside>
  </div>
  <main class="sx-main">{@render children()}</main>
  {#if settingsOpen}<div class="shell-settings"><div class="settings-heading"><strong>{$t('Settings')}</strong><button onclick={()=>settingsOpen=false} aria-label={$t('Close')}>×</button></div>{#if settingsNav}{@render settingsNav()}{/if}<div class="appearance"><LanguageSelect/><ThemeToggle/></div></div>{/if}
</div>
<style>
.unified-shell{position:relative;background:var(--color-bg)}.navigation-area{position:relative;flex:none;width:52px;display:flex;z-index:40}.navigation-area.pinned{width:calc(52px + var(--nav-width))}.app-rail{width:52px;flex:none;display:flex;flex-direction:column;align-items:center;gap:8px;padding:12px 6px;background:var(--sidebar-bg);border-right:1px solid var(--color-border)}.app-rail button{display:grid;place-items:center;width:35px;height:35px;border-radius:9px;color:var(--color-text-muted);cursor:pointer}.app-rail button:hover,.app-rail button.active{background:var(--surface-hover);color:var(--color-text)}.app-rail button:focus-visible{outline:2px solid var(--primary)}.rail-spacer{flex:1}.project-sidebar{position:absolute;left:52px;top:0;bottom:0;width:var(--nav-width);background:var(--sidebar-bg);border-right:1px solid var(--color-border);transform:translateX(-12px);opacity:0;visibility:hidden;transition:transform .16s,opacity .16s}.pinned .project-sidebar,.peek .project-sidebar{transform:none;opacity:1;visibility:visible}.peek:not(.pinned) .project-sidebar{box-shadow:16px 0 32px #0004}.resize-handle{position:absolute;top:0;right:-3px;bottom:0;width:6px;cursor:col-resize;touch-action:none;z-index:2}.resize-handle:hover,.resize-handle:focus-visible{background:var(--color-border);outline:none}.sx-main{background:var(--card-bg)}.shell-settings{position:absolute;left:12px;bottom:12px;z-index:80;min-width:240px;padding:12px;border:1px solid var(--color-border);border-radius:12px;background:var(--card-bg);box-shadow:0 10px 30px #0005}.settings-heading{display:flex;justify-content:space-between;margin-bottom:12px;font-size:13px}.settings-heading button{cursor:pointer}.appearance{display:flex;gap:8px;margin-top:12px;border-top:1px solid var(--color-border);padding-top:12px}@media(max-width:700px){.navigation-area.pinned{width:52px}.project-sidebar{width:min(var(--nav-width),calc(100vw - 70px));box-shadow:16px 0 32px #0004}.app-rail{padding:8px 4px}}@media(prefers-reduced-motion:reduce){.project-sidebar{transition:none}}
</style>
