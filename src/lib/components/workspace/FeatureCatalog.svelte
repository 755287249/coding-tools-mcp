<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { copyText } from '$lib/browser-tools';
  import { t } from '$lib/i18n';
  import type { Snippet } from 'svelte';
  import type { SkillInventoryPayload, ExtensionInventoryPayload, ExtensionKind } from '$lib/backend';
  import Puzzle from '@lucide/svelte/icons/puzzle';
  import BookOpen from '@lucide/svelte/icons/book-open';
  import Search from '@lucide/svelte/icons/search';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import Settings from '@lucide/svelte/icons/settings';
  import Plus from '@lucide/svelte/icons/plus';
  import Check from '@lucide/svelte/icons/check';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Copy from '@lucide/svelte/icons/copy';
  import X from '@lucide/svelte/icons/x';
  import { featureRows, filterFeatureRows, type FeatureRow } from '$lib/workspace/feature-catalog';
  let { workspaceId, activeTab, skills, extensions, loading, error, isBusy, onRefresh, onTab, onSkill, onExtension, onSkillsActive, onExtensionActive, workspacePicker }: {
    workspaceId:string;activeTab:'skills'|'hooks'|'mcp';skills:SkillInventoryPayload|null;extensions:ExtensionInventoryPayload|null;loading:boolean;error:string;
    isBusy:(key:string)=>boolean;onRefresh:()=>void;onTab:(tab:'skills'|'hooks'|'mcp')=>void;
    onSkill:(key:string,enabled:boolean)=>void;onExtension:(kind:ExtensionKind,key:string,enabled:boolean)=>void;
    onSkillsActive:(active:boolean)=>void;onExtensionActive:(kind:ExtensionKind,active:boolean)=>void;workspacePicker?:Snippet;
  }=$props();
  let query=$state(''), scope=$state<'all'|'workspace'|'user'>('all');
  let settingsDialog:HTMLDialogElement, addDialog:HTMLDialogElement;
  let feedback=$state('');
  const rows=$derived(featureRows(activeTab,skills,extensions));
  const filtered=$derived(filterFeatureRows(rows,query,scope));
  const enabledRows=$derived(filtered.filter(row=>row.selected));
  const availableRows=$derived(filtered.filter(row=>!row.selected));
  const selected=$derived(rows.find(row=>row.key===$page.url.searchParams.get('feature')));
  const master=$derived(activeTab==='skills'?skills?.active:activeTab==='hooks'?extensions?.hooksActive:extensions?.mcpActive);
  const heading=$derived(activeTab==='skills'?$t('shell.skills'):activeTab==='hooks'?$t('Hooks'):$t('shell.plugins'));
  const description=$derived($t(activeTab==='skills'?'features.skillsDescription':activeTab==='hooks'?'features.hooksDescription':'features.pluginsDescription'));
  const configured=$derived(extensions?.mcpServers??[]);
  $effect(()=>{const id=workspaceId,tab=activeTab;query='';scope='all';feedback='';settingsDialog?.close();addDialog?.close();});
  function detail(key?:string){const url=new URL($page.url);if(key)url.searchParams.set('feature',key);else url.searchParams.delete('feature');void goto(url.pathname+url.search,{noScroll:true});}
  function toggle(row:FeatureRow){if(row.kind==='skill')onSkill(row.key,!row.selected);else onExtension(row.kind,row.key,!row.selected);}
  function disabled(row:FeatureRow){return loading||!master||!row.supported||!row.sourceEnabled||isBusy(`${row.kind}:${row.key}`);}
  async function copy(value:string){try{await copyText(value);feedback=$t('Copied')}catch(e){feedback=String(e)}}
  function sidePlugin(key:string){const url=new URL($page.url);url.searchParams.delete('featureTab');url.searchParams.set('panel','plugins');url.searchParams.set('feature',key);void goto(url.pathname+url.search,{noScroll:true});}
  const sample=$derived(activeTab==='skills'?'---\nname: my-skill\ndescription: Describe when to use this skill.\n---\n\n# My skill\n\nWrite the steps the assistant should follow.\n':activeTab==='hooks'?'~/.claude/settings.json\n.codex/hooks.json':'{\n  "mcpServers": {\n    "my-server": {\n      "type": "http",\n      "url": "https://example.com/mcp"\n    }\n  }\n}');
</script>

<div class="feature-catalog">
  <aside class="custom-sidebar" aria-label={$t('features.customize')}>
    <h2>{$t('features.customize')}</h2>
    <button class:active={activeTab==='mcp'} onclick={()=>onTab('mcp')}><Puzzle size={17}/>{$t('shell.plugins')}</button>
    <button class:active={activeTab==='skills'} onclick={()=>onTab('skills')}><BookOpen size={17}/>{$t('shell.skills')}</button>
    <p class="sidebar-label">{$t('features.configured')}</p>
    {#each configured as plugin (plugin.key)}<button class="sidebar-plugin" class:active={activeTab==='mcp'&&selected?.key===plugin.key} onclick={()=>sidePlugin(plugin.key)} title={plugin.name}><Puzzle size={14}/><span>{plugin.name}</span>{#if plugin.selected}<i class:connected={plugin.connected}></i>{/if}</button>{:else}<p class="sidebar-empty">{$t('No MCP servers discovered.')}</p>{/each}
    <div class="sidebar-workspace">{#if workspacePicker}{@render workspacePicker()}{/if}</div>
  </aside>
  <main class="catalog-content">
    {#if selected}
      <nav class="feature-breadcrumb" aria-label={$t('features.details')}><button onclick={()=>detail()}>{heading}</button><ChevronRight size={13}/><span>{selected.name}</span></nav>
      <article class="feature-detail">
        <span class="feature-icon large" class:skill={selected.kind==='skill'}>{#if selected.kind==='skill'}<BookOpen size={30}/>{:else}<Puzzle size={30}/>{/if}</span>
        <header class="detail-heading"><div><h1>{selected.name}</h1><p>{selected.description||description}</p></div><div class="detail-actions"><button class="secondary" onclick={()=>copy($page.url.href)}><Copy size={14}/>{$t('features.copyLink')}</button><button class="primary" disabled={disabled(selected)} onclick={()=>toggle(selected)}>{#if selected.selected}<Check size={15}/>{/if}{$t(selected.selected?'features.disable':'features.enable')}</button></div></header>
        <div class="feature-hero"><span class="hero-icon">{#if selected.kind==='skill'}<BookOpen size={58}/>{:else}<Puzzle size={58}/>{/if}</span><div><strong>{selected.name}</strong><p>{selected.kind==='mcp'?$t('{count} tools',{count:selected.toolCount}):selected.description||description}</p><span>{$t(selected.enabled?'features.active':selected.selected?'features.selectedInactive':'features.available')}</span></div></div>
        {#if selected.error}<p class="catalog-error" role="alert">{selected.error}</p>{/if}
        {#if !master}<p class="catalog-notice">{$t('features.masterOff')} <button onclick={()=>settingsDialog.showModal()}>{$t('Settings')}</button></p>{/if}
        <h3>{$t('features.information')}</h3>
        <dl><div><dt>{$t('features.source')}</dt><dd>{selected.provider}</dd></div><div><dt>{$t('features.scope')}</dt><dd>{$t(selected.scope==='user'?'features.personal':'features.project')}</dd></div><div><dt>{$t('features.location')}</dt><dd><code>{selected.path}</code><button aria-label={$t('Copy')} onclick={()=>copy(selected.path)}><Copy size={14}/></button></dd></div>
        {#if selected.kind==='mcp'}<div><dt>{$t('features.connection')}</dt><dd>{selected.transport} · {$t(selected.connected?'Connected':'features.disconnected')}</dd></div><div><dt>{$t('features.tools')}</dt><dd>{selected.toolCount}</dd></div>{/if}
        {#if selected.version}<div><dt>{$t('features.version')}</dt><dd>{selected.version}</dd></div>{/if}
        <div><dt>{$t('Status')}</dt><dd>{$t(!selected.supported?'Unsupported':!selected.sourceEnabled?'Source disabled':selected.enabled?'features.active':selected.selected?'features.selectedInactive':'features.available')}</dd></div></dl>
        <button class="secondary" onclick={()=>detail()}>{$t('features.backToList')}</button>
      </article>
    {:else}
      <section class="catalog-home">
        <header class="catalog-heading"><div><h1>{heading}</h1><p>{description}</p></div><div class="catalog-toolbar"><label class="catalog-search"><Search size={15}/><input bind:value={query} aria-label={$t(activeTab==='skills'?'features.searchSkills':'chat.searchPlugins')} placeholder={$t(activeTab==='skills'?'features.searchSkills':'chat.searchPlugins')}/></label><button class="icon-button" aria-label={$t('Refresh')} title={$t('Refresh')} disabled={loading} onclick={onRefresh}><RefreshCw size={16}/></button><button class="icon-button" aria-label={$t('Settings')} title={$t('Settings')} onclick={()=>settingsDialog.showModal()}><Settings size={16}/></button><button class="primary" onclick={()=>addDialog.showModal()}>{$t('features.add')}<Plus size={13}/></button></div></header>
        <div class="catalog-scopes">{#each ['all','workspace','user'] as value}<button class:active={scope===value} aria-pressed={scope===value} onclick={()=>scope=value as typeof scope}>{$t(value==='all'?'chat.allModes':value==='user'?'features.personal':'features.project')}</button>{/each}</div>
        {#if !master&&!loading}<div class="catalog-notice">{$t('features.masterOff')}<button onclick={()=>settingsDialog.showModal()}>{$t('Settings')}</button></div>{/if}
        {#if loading&&!rows.length}<div class="feature-empty" role="status"><RefreshCw size={30}/><p>{$t('Loading…')}</p></div>
        {:else if !filtered.length}<div class="feature-empty"><span class="feature-icon large">{#if activeTab==='skills'}<BookOpen size={32}/>{:else}<Puzzle size={32}/>{/if}</span><h3>{$t(query?'features.noMatches':'features.empty')}</h3><p>{$t(query?'features.searchHint':'features.addHint')}</p>{#if !query}<button class="primary" onclick={()=>addDialog.showModal()}>{$t('features.add')}</button>{/if}</div>
        {:else}
          {#if enabledRows.length}<section class="catalog-section"><h3>{$t('features.selected')} <span>{enabledRows.length}</span></h3><div class="feature-grid">{#each enabledRows as row (row.key)}{@render featureCard(row)}{/each}</div></section>{/if}
          {#if availableRows.length}<section class="catalog-section"><h3>{$t('features.available')} <span>{availableRows.length}</span></h3><div class="feature-grid">{#each availableRows as row (row.key)}{@render featureCard(row)}{/each}</div></section>{/if}
        {/if}
      </section>
    {/if}
    {#if error}<p class="catalog-error" role="alert">{error}<button onclick={onRefresh}>{$t('Try again')}</button></p>{/if}
    {#if feedback}<p class="catalog-feedback" role="status">{feedback}</p>{/if}
  </main>
</div>

{#snippet featureCard(row:FeatureRow)}
  <div class="feature-card" class:unavailable={!row.supported||!row.sourceEnabled}>
    <button class="feature-summary" onclick={()=>detail(row.key)}><span class="feature-icon" class:skill={row.kind==='skill'}>{#if row.kind==='skill'}<BookOpen size={24}/>{:else}<Puzzle size={24}/>{/if}</span><span class="feature-card-text"><strong>{row.name}</strong><small>{row.description||(row.kind==='mcp'?`${row.provider} · ${$t('{count} tools',{count:row.toolCount})}`:row.provider)}</small>{#if row.error||!row.supported||!row.sourceEnabled}<em>{row.error||$t(!row.supported?'Unsupported':'Source disabled')}</em>{/if}</span></button>
    <button class="feature-toggle" role="switch" aria-checked={row.selected} aria-label={`${$t(row.selected?'features.disable':'features.enable')}: ${row.name}`} title={$t(row.selected?'features.disable':'features.enable')} disabled={disabled(row)} onclick={()=>toggle(row)}>{#if isBusy(`${row.kind}:${row.key}`)}<RefreshCw size={17}/>{:else if row.selected}<Check size={18}/>{:else}<Plus size={20}/>{/if}</button>
  </div>
{/snippet}

<dialog class="catalog-dialog" bind:this={settingsDialog} aria-label={$t('Settings')}><header><h2>{$t('Settings')}</h2><button class="icon-button" aria-label={$t('Close')} onclick={()=>settingsDialog.close()}><X size={18}/></button></header>
  <label><span>{$t('Enable all skills')}</span><input type="checkbox" checked={skills?.active??false} disabled={!skills||loading||isBusy('skills:master')} onchange={event=>onSkillsActive(event.currentTarget.checked)}/></label>
  <label><span>{$t('Enable all MCP servers')}</span><input type="checkbox" checked={extensions?.mcpActive??false} disabled={!extensions||loading||isBusy('mcp:master')} onchange={event=>onExtensionActive('mcp',event.currentTarget.checked)}/></label>
  <label><span>{$t('Enable all hooks')}</span><input type="checkbox" checked={extensions?.hooksActive??false} disabled={!extensions||loading||isBusy('hook:master')} onchange={event=>onExtensionActive('hook',event.currentTarget.checked)}/></label>
  <button class="secondary" onclick={()=>{settingsDialog.close();onTab('hooks')}}>{$t('features.manageHooks')}</button>
  {#if skills?.diagnostics.length||extensions?.diagnostics.length}<details><summary>{$t('Diagnostics')}</summary>{#each [...skills?.diagnostics??[],...extensions?.diagnostics??[]] as diagnostic}<p>{diagnostic.message}</p>{/each}</details>{/if}
  {#if error}<p class="catalog-error" role="alert">{error}</p>{/if}
</dialog>
<dialog class="catalog-dialog" bind:this={addDialog} aria-label={$t('features.add')}><header><h2>{$t(activeTab==='skills'?'features.addSkill':activeTab==='hooks'?'features.addHook':'features.addPlugin')}</h2><button class="icon-button" aria-label={$t('Close')} onclick={()=>addDialog.close()}><X size={18}/></button></header>
  <p>{$t(activeTab==='skills'?'features.addSkillHelp':activeTab==='hooks'?'features.addHookHelp':'features.addPluginHelp')}</p>
  <code class="discovery-path">{activeTab==='skills'?'skills / .agents/skills / .claude/skills':activeTab==='hooks'?'.claude/settings.json / .codex/hooks.json':'.mcp.json / .codex/config.toml'}</code>
  <pre>{sample}</pre><p class="catalog-notice">{$t('features.templateHint')}</p>
  <footer><button class="secondary" onclick={()=>copy(sample)}><Copy size={14}/>{$t('features.copyExample')}</button><button class="primary" disabled={loading} onclick={()=>{addDialog.close();onRefresh()}}><RefreshCw size={14}/>{$t('features.refreshDiscovery')}</button></footer>
  {#if feedback}<p role="status">{feedback}</p>{/if}
</dialog>

<style>
.feature-catalog{display:grid;grid-template-columns:280px minmax(0,1fr);height:100%;min-height:0;color:var(--color-text)}.custom-sidebar{padding:20px 12px;border-right:1px solid var(--color-border);background:var(--sidebar-bg);overflow:auto;display:flex;flex-direction:column;gap:3px;min-width:0}.custom-sidebar h2{font-size:18px;font-weight:650;padding:0 8px 16px}.custom-sidebar>button{display:flex;align-items:center;gap:10px;min-height:34px;padding:7px 10px;border-radius:9px;font-size:14px;text-align:left;cursor:pointer}.custom-sidebar .active{background:var(--surface-hover)}.sidebar-label{font-size:13px;font-weight:600;color:var(--color-text-muted);margin:22px 8px 7px}.sidebar-plugin span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1}.sidebar-plugin i{width:5px;height:5px;background:var(--color-text-muted);border-radius:50%}.sidebar-plugin i.connected{background:var(--success)}.sidebar-empty{padding:8px;font-size:12px;color:var(--color-text-muted)}.sidebar-workspace{margin-top:auto;padding:24px 8px 4px}.sidebar-workspace :global(select){width:100%;min-width:0;padding:8px;font-size:12px;border:1px solid var(--color-border);border-radius:8px;background:var(--surface-2)}.catalog-content{overflow:auto;padding:32px clamp(20px,5vw,80px);min-width:0}.catalog-home{max-width:950px;margin:auto}.catalog-heading{display:flex;justify-content:space-between;align-items:flex-start;gap:24px;margin-bottom:22px}.catalog-heading h1,.detail-heading h1{font-size:24px;font-weight:650;margin:0 0 10px}.catalog-heading p,.detail-heading p{font-size:13px;color:var(--color-text-secondary);line-height:1.65}.catalog-toolbar{display:flex;gap:12px;align-items:center;flex-wrap:wrap}.catalog-search{display:flex;align-items:center;gap:7px;border:1px solid var(--color-border);border-radius:24px;padding:7px 12px;color:var(--color-text-muted);width:220px}.catalog-search input{background:transparent;border:0;outline:0;min-width:0;width:100%;font-size:12px;color:var(--color-text)}.icon-button{display:inline-grid;place-items:center;padding:6px;border-radius:7px;color:var(--color-text-muted);cursor:pointer}.primary,.secondary{display:inline-flex;align-items:center;justify-content:center;gap:7px;border-radius:20px;padding:7px 13px;font-size:12px;cursor:pointer;white-space:nowrap}.primary{background:var(--color-text);color:var(--primary-contrast)}.secondary{background:var(--surface-hover);color:var(--color-text)}.catalog-scopes{display:flex;gap:6px;margin-bottom:22px}.catalog-scopes button{padding:8px 16px;font-size:13px;border-radius:22px;color:var(--color-text-secondary);cursor:pointer}.catalog-scopes .active{background:var(--surface-hover);color:var(--color-text);font-weight:600}.catalog-section{margin-bottom:35px}.catalog-section h3{font-size:15px;font-weight:600;border-bottom:1px solid var(--color-border);padding-bottom:12px;margin-bottom:12px}.catalog-section h3 span{font-size:12px;color:var(--color-text-muted);font-weight:400;margin-left:5px}.feature-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));column-gap:30px;row-gap:10px}.feature-card{display:flex;align-items:center;gap:10px;border-radius:12px;min-width:0;padding:7px 0}.feature-card:hover{background:var(--surface-hover)}.feature-summary{display:flex;align-items:center;gap:12px;min-width:0;flex:1;padding:6px 8px;text-align:left;cursor:pointer}.feature-icon{width:40px;height:40px;flex:none;border:1px solid var(--color-border);border-radius:11px;background:#25596933;color:#92cedb;display:inline-grid;place-items:center}.feature-icon.skill{background:#79623533;color:#e4c793}.feature-icon.large{width:56px;height:56px;border-radius:14px}.feature-card-text{display:grid;gap:6px;min-width:0}.feature-card-text strong{font-size:13px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.feature-card-text small{font-size:12px;color:var(--color-text-secondary);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.feature-card-text em{font-size:10px;font-style:normal;color:var(--danger);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.feature-toggle{display:grid;place-items:center;width:30px;height:32px;flex:none;margin-right:7px;border-radius:8px;cursor:pointer;color:var(--color-text-muted)}.feature-toggle:hover:enabled{background:var(--surface-hover);color:var(--color-text)}button:disabled{opacity:.45;cursor:default}.feature-empty{display:flex;flex-direction:column;align-items:center;gap:15px;padding:90px 15px;text-align:center}.feature-empty h3{font-size:18px;font-weight:600}.feature-empty p{font-size:13px;color:var(--color-text-muted);max-width:400px;line-height:1.7}.feature-breadcrumb{display:flex;align-items:center;gap:10px;font-size:12px;color:var(--color-text-muted);margin-bottom:34px}.feature-breadcrumb button{cursor:pointer}.feature-breadcrumb span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--color-text)}.feature-detail{max-width:730px;margin:auto}.detail-heading{display:flex;align-items:center;justify-content:space-between;gap:20px;margin:18px 0 24px}.detail-heading>div:first-child{min-width:0}.detail-heading h1{overflow-wrap:anywhere;font-size:22px}.detail-actions{display:flex;gap:8px;flex-wrap:wrap}.feature-hero{display:flex;align-items:center;gap:28px;padding:34px;border-radius:18px;background:linear-gradient(130deg,#224a6b88,#67567566);min-height:200px;margin-bottom:28px}.hero-icon{color:#dae8ff88}.feature-hero strong{font-size:20px}.feature-hero p{font-size:13px;margin:12px 0;line-height:1.7}.feature-hero span{font-size:12px}.feature-detail h3{font-size:15px;font-weight:600;margin:24px 0 12px}dl{border-top:1px solid var(--color-border);margin-bottom:26px}dl>div{display:grid;grid-template-columns:110px minmax(0,1fr);gap:18px;padding:14px 0;border-bottom:1px solid var(--color-border);font-size:12px}dt{color:var(--color-text-muted)}dd{min-width:0;overflow-wrap:anywhere;display:flex;gap:10px;align-items:center}dd code{min-width:0;overflow-wrap:anywhere}dd button{flex:none;cursor:pointer}.catalog-notice{font-size:12px;line-height:1.7;color:var(--color-text-secondary);padding:12px;border:1px solid var(--color-border);border-radius:10px;margin-bottom:20px}.catalog-notice button{margin-left:10px;text-decoration:underline;cursor:pointer}.catalog-error{font-size:12px;color:var(--danger);overflow-wrap:anywhere;padding:14px;max-width:950px;margin:12px auto}.catalog-error button{margin-left:12px;text-decoration:underline;cursor:pointer}.catalog-feedback{font-size:12px;padding:10px;max-width:950px;margin:auto}.catalog-dialog{margin:auto;width:min(540px,calc(100vw - 32px));max-height:calc(100dvh - 48px);overflow:auto;padding:22px;background:var(--surface-2);color:var(--color-text);border:1px solid var(--color-border);border-radius:18px;box-shadow:0 20px 70px #0007}.catalog-dialog::backdrop{background:#0006}.catalog-dialog header{display:flex;justify-content:space-between;align-items:center;margin-bottom:20px}.catalog-dialog h2{font-size:18px;font-weight:600}.catalog-dialog label{display:flex;align-items:center;justify-content:space-between;gap:16px;padding:14px 0;font-size:13px}.catalog-dialog p{font-size:13px;line-height:1.8;margin:12px 0}.catalog-dialog details{margin-top:22px;font-size:12px}.catalog-dialog pre{font-size:11px;background:var(--surface-1);padding:14px;border-radius:10px;overflow:auto;margin:16px 0}.discovery-path{font-size:12px;overflow-wrap:anywhere}.catalog-dialog footer{display:flex;gap:12px;justify-content:space-between}.catalog-dialog .catalog-notice{font-size:12px}button:focus-visible,input:focus-visible{outline:2px solid var(--primary);outline-offset:2px}@media(max-width:1200px){.feature-catalog{grid-template-columns:230px minmax(0,1fr)}.catalog-heading{flex-direction:column;gap:16px}.catalog-toolbar{width:100%}.catalog-search{flex:1}.catalog-content{padding:28px}.feature-grid{column-gap:15px}}@media(max-width:850px){.feature-grid{grid-template-columns:1fr}.detail-heading{flex-direction:column;align-items:flex-start}.feature-hero{padding:24px;gap:16px}}@media(max-width:650px){.feature-catalog{display:flex;flex-direction:column}.custom-sidebar{flex:none;padding:12px;border-right:0;border-bottom:1px solid var(--color-border);display:grid;grid-template-columns:1fr 1fr;gap:6px}.custom-sidebar h2{grid-column:1/-1;padding:0 4px 6px;font-size:16px}.sidebar-label,.sidebar-plugin,.sidebar-empty{display:none!important}.sidebar-workspace{grid-column:1/-1;padding:4px;margin:0}.catalog-content{padding:22px 16px}.catalog-toolbar{gap:7px}.catalog-search{width:auto;min-width:120px}.feature-hero{min-height:160px}.hero-icon{display:none}dl>div{grid-template-columns:75px minmax(0,1fr)}.catalog-heading h1{font-size:22px}}
</style>
