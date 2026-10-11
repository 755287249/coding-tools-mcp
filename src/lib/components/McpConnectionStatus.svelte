<script lang="ts">
  import {t} from '$lib/i18n';
  import {onMount} from 'svelte';
  import {goto} from '$app/navigation';
  import {appUrl} from '$lib/app-path';
  import Globe from '@lucide/svelte/icons/globe';
  import X from '@lucide/svelte/icons/x';
  import {connectionHealth,acquireConnectionHealth,refreshConnectionHealth} from '$lib/stores/connection-health';
  let {mobile=false,onSharing}:{mobile?:boolean;onSharing?:()=>void}=$props();
  let dialog:HTMLDialogElement;
  let now=$state(Date.now());
  const stale=$derived(Boolean($connectionHealth.error)||Boolean($connectionHealth.receivedAt&&now-$connectionHealth.receivedAt>25000));
  const phase=$derived(stale?'stale':$connectionHealth.snapshot?.state??'connecting');
  const states={online:{label:'connection.online' as const,color:'#78c8a1'},offline:{label:'connection.offline' as const,color:'#969aa5'},error:{label:'connection.error' as const,color:'#f0aa78'},connecting:{label:'connection.connecting' as const,color:'#b1a0ec'},stale:{label:'connection.stale' as const,color:'#d4ba79'}};
  const status=$derived(states[phase]);
  const labels=$derived<Record<string,string>>({running:$t('connection.running'),stopped:$t('connection.stopped'),starting:$t('connection.starting'),stopping:$t('connection.stopping'),error:$t('connection.failed'),online:$t('connection.reachable'),local:$t('connection.local')});
  function close(){dialog?.close();}
  function configure(){close();void goto(appUrl('/connection'));}
  onMount(()=>{const release=acquireConnectionHealth();const timer=setInterval(()=>now=Date.now(),1000);return()=>{release();clearInterval(timer);close();};});
</script>
<button class="connection-trigger" class:mobile style:--status-color={status.color} title={`${$t('connection.global')} · ${$t(status.label)}`} aria-label={`${$t('connection.global')}: ${$t(status.label)}`} onclick={()=>dialog.showModal()}>
  <Globe size={20} strokeWidth={1.5}/><span class="connection-label" aria-live="polite">{$t(status.label)}</span>{#if mobile}<span class="mobile-caption">{$t('connection.global')}</span>{/if}
</button>
<dialog bind:this={dialog} class="connection-dialog" aria-label={$t('connection.dialog')} oncancel={event=>{event.preventDefault();close()}}>
  <header><div><p>{$t('connection.state')}</p><h2>{$t('connection.global')}</h2></div><button class="close" aria-label={$t('connection.close')} onclick={close}><X size={20}/></button></header>
  <div class="status-summary" style:--status-color={status.color}><Globe size={26}/><div><strong>{$t(status.label)}</strong><p>{stale?$t('connection.staleHelp'):$connectionHealth.snapshot?.detail||$t('connection.probeHelp')}</p></div></div>
  <dl><div><dt>{$t('connection.service')}</dt><dd>{stale?$t('connection.retryPending'):labels[$connectionHealth.snapshot?.service??'']??$t('connection.checking')}</dd></div><div><dt>{$t('connection.entry')}</dt><dd>{stale?$t('connection.retryPending'):labels[$connectionHealth.snapshot?.entry??'']??$t('connection.checking')}</dd></div><div><dt>{$t('connection.scope')}</dt><dd>{$t('connection.allProjects')}</dd></div></dl>
  <p class="scope-note">{$t('connection.scopeHelp')}</p>
  {#if $connectionHealth.error}<p class="scope-note">{$connectionHealth.error}</p>{/if}
  <footer><span>{$connectionHealth.receivedAt?$t('connection.updated',{time:new Date($connectionHealth.receivedAt).toLocaleTimeString()}):$t('connection.noStatus')}</span><button onclick={()=>void refreshConnectionHealth()}>{$t('connection.recheck')}</button></footer>
  <footer><button onclick={configure}>{$t('connection.settings')}</button>{#if onSharing}<button onclick={()=>{close();onSharing?.()}}>{$t('connection.sharing')}</button>{/if}</footer>
</dialog>
<style>
.connection-trigger{display:flex;flex-direction:column;align-items:center;justify-content:center;gap:4px;width:36px;min-height:51px;border-radius:9px;color:var(--status-color);cursor:pointer;flex:none}.connection-trigger:hover{background:var(--surface-hover)}.connection-trigger:focus-visible,.close:focus-visible,footer button:focus-visible{outline:2px solid var(--primary);outline-offset:2px}.connection-label{font-size:10px;line-height:13px;white-space:nowrap}.connection-trigger.mobile{margin:6px 14px 12px;flex-direction:row;width:calc(100% - 28px);gap:9px;justify-content:flex-start;padding:8px 12px;min-height:44px;border:1px solid var(--color-border)}.mobile .connection-label{font-size:12px;order:2;margin-left:auto}.mobile-caption{font-size:12px;color:var(--color-text-muted)}
.connection-dialog{position:fixed;margin:auto;width:min(430px,calc(100vw - 28px));max-height:88dvh;overflow:auto;padding:24px;border:1px solid var(--color-border);border-radius:18px;background:rgb(var(--glass-base));color:var(--color-text);box-shadow:0 24px 80px #0005}.connection-dialog::backdrop{background:#0007}header{display:flex;justify-content:space-between;align-items:flex-start}header p{font-size:11px;color:var(--color-text-muted);margin-bottom:5px}h2{font-size:20px;font-weight:600}.close{padding:5px;border-radius:7px;cursor:pointer;color:var(--color-text-muted)}.status-summary{display:flex;gap:14px;align-items:center;margin:24px 0;color:var(--status-color)}.status-summary strong{font-size:17px;font-weight:600}.status-summary p{font-size:12px;color:var(--color-text-muted);margin-top:5px}dl{border-top:1px solid var(--color-border);border-bottom:1px solid var(--color-border);padding:10px 0}dl div{display:flex;justify-content:space-between;gap:14px;font-size:12px;padding:8px 0}dt{color:var(--color-text-muted)}.scope-note{font-size:12px;line-height:1.8;color:var(--color-text-muted);margin-top:16px}footer{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-top:22px;font-size:10px;color:var(--color-text-muted)}footer button{font-size:12px;border:1px solid var(--color-border);padding:6px 10px;border-radius:7px;cursor:pointer;white-space:nowrap}
</style>
