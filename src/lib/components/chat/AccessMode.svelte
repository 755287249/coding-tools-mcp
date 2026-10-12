<script lang="ts">
  import {t} from '$lib/i18n';
  import type {MessageKey} from '$lib/i18n/catalog';
  import ShieldCheck from '@lucide/svelte/icons/shield-check';
  import ShieldOff from '@lucide/svelte/icons/shield-off';
  import ShieldAlert from '@lucide/svelte/icons/shield-alert';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Check from '@lucide/svelte/icons/check';
  import {tick} from 'svelte';
  /** Per-chat authorization mode: full / normal / strict (完全 / 一般 / 严格授权). */
  type Mode='full'|'normal'|'strict';
  let {mode='normal',disabled=false,onChange}:{mode?:Mode;disabled?:boolean;onChange:(mode:Mode)=>void}=$props();
  const MODES:Mode[]=['full','normal','strict'];
  const ICONS={full:ShieldOff,normal:ShieldCheck,strict:ShieldAlert};
  const id=`access-mode-${Math.random().toString(36).slice(2,10)}`;
  let open=$state(false),active=$state(0),trigger=$state<HTMLButtonElement>(),menu=$state<HTMLDivElement>(),place=$state('');
  const Current=$derived(ICONS[mode]);
  /** The menu is fixed-positioned above the trigger so the composer's overflow cannot clip it. */
  function position(){
    if(!trigger)return;const rect=trigger.getBoundingClientRect(),width=Math.min(288,window.innerWidth-16);
    const left=Math.max(8,Math.min(rect.right-width,window.innerWidth-width-8));
    place=rect.top>220?`left:${left}px;bottom:${window.innerHeight-rect.top+8}px;width:${width}px`:`left:${left}px;top:${rect.bottom+8}px;width:${width}px`;
  }
  async function show(){if(disabled)return;active=Math.max(0,MODES.indexOf(mode));position();open=true;await tick();menu?.querySelectorAll<HTMLElement>('[role=menuitemradio]')[active]?.focus();}
  function hide(focus=true){open=false;if(focus)trigger?.focus();}
  function choose(item:Mode){hide();if(item!==mode)onChange(item);}
  function move(step:number){active=(active+step+MODES.length)%MODES.length;menu?.querySelectorAll<HTMLElement>('[role=menuitemradio]')[active]?.focus();}
  function keys(event:KeyboardEvent){
    if(event.key==='Escape'){event.preventDefault();hide();}
    else if(event.key==='ArrowDown'){event.preventDefault();move(1);}
    else if(event.key==='ArrowUp'){event.preventDefault();move(-1);}
    else if(event.key==='Home'){event.preventDefault();move(-active);}
    else if(event.key==='End'){event.preventDefault();move(MODES.length-1-active);}
    else if(event.key==='Tab')hide(false);
  }
  function outside(event:PointerEvent){const target=event.target as Node;if(open&&!menu?.contains(target)&&!trigger?.contains(target))hide(false);}
  $effect(()=>{if(disabled&&open)hide(false);});
</script>
<svelte:window onpointerdown={outside} onresize={()=>{if(open)position();}}/>
<button bind:this={trigger} type="button" class="access-mode" class:full={mode==='full'} class:strict={mode==='strict'} class:open {disabled}
  aria-haspopup="menu" aria-expanded={open} aria-controls={open?id:undefined}
  aria-label={`${$t('chat.access.label')}: ${$t(`chat.access.${mode}` as MessageKey)}`} title={$t(`chat.access.${mode}Hint` as MessageKey)}
  onclick={()=>open?hide():void show()} onkeydown={e=>{if((e.key==='ArrowDown'||e.key==='ArrowUp')&&!open){e.preventDefault();void show();}}}>
  <Current size={14}/><span>{$t(`chat.access.${mode}` as MessageKey)}</span><ChevronDown size={13} class="chevron"/>
</button>
{#if open}
  <div bind:this={menu} {id} class="access-menu" role="menu" tabindex="-1" aria-label={$t('chat.access.label')} style={place} onkeydown={keys}>
    <p class="access-menu-title">{$t('chat.access.label')}</p>
    {#each MODES as item,index}
      {@const Icon=ICONS[item]}
      <button type="button" role="menuitemradio" aria-checked={item===mode} tabindex={index===active?0:-1} class={`access-option ${item}`} class:selected={item===mode} onclick={()=>choose(item)} onfocus={()=>active=index}>
        <span class="access-icon"><Icon size={16}/></span>
        <span class="access-text"><strong>{$t(`chat.access.${item}` as MessageKey)}</strong><small>{$t(`chat.access.${item}Hint` as MessageKey)}</small></span>
        <span class="access-check" aria-hidden="true">{#if item===mode}<Check size={15}/>{/if}</span>
      </button>
    {/each}
  </div>
{/if}
<style>
.access-mode{--tone:var(--color-text-muted);display:inline-flex;align-items:center;gap:5px;height:30px;padding:0 9px 0 10px;border:1px solid var(--color-border);border-radius:999px;background:var(--surface-2);color:var(--tone);font:inherit;font-size:12px;font-weight:500;cursor:pointer;white-space:nowrap;transition:background var(--duration-fast,.15s),border-color var(--duration-fast,.15s),color var(--duration-fast,.15s)}
.access-mode span{color:var(--color-text-secondary,var(--color-text))}
.access-mode:hover:not(:disabled),.access-mode.open{background:var(--surface-hover,var(--surface-3));border-color:var(--border-strong,var(--color-border))}
.access-mode:focus-visible{outline:2px solid var(--action,#2563eb);outline-offset:2px}
.access-mode:disabled{cursor:default;opacity:.55}
.access-mode.full{--tone:#d97706;border-color:color-mix(in srgb,#d97706 40%,var(--color-border))}
.access-mode.strict{--tone:#16a34a;border-color:color-mix(in srgb,#16a34a 40%,var(--color-border))}
.access-mode :global(.chevron){color:var(--color-text-muted);transition:transform var(--duration-fast,.15s)}
.access-mode.open :global(.chevron){transform:rotate(180deg)}
.access-menu{position:fixed;z-index:1000;display:flex;flex-direction:column;gap:2px;padding:6px;border:1px solid var(--color-border);border-radius:14px;background:var(--card-bg,var(--surface-2));color:var(--color-text);box-shadow:var(--dialog-shadow,0 16px 40px rgb(0 0 0 / .18));animation:access-in .12s var(--ease-out,ease-out)}
.access-menu-title{margin:4px 8px 6px;font-size:11px;font-weight:600;letter-spacing:.02em;color:var(--color-text-muted)}
.access-option{--tone:#2563eb;display:grid;grid-template-columns:32px minmax(0,1fr) 18px;align-items:center;gap:10px;width:100%;padding:8px;border:0;border-radius:10px;background:transparent;color:inherit;font:inherit;text-align:left;cursor:pointer}
.access-option.full{--tone:#d97706}.access-option.strict{--tone:#16a34a}
.access-option:hover,.access-option:focus-visible{background:var(--surface-hover,var(--surface-2));outline:none}
.access-option.selected{background:color-mix(in srgb,var(--tone) 10%,transparent)}
.access-icon{display:inline-flex;align-items:center;justify-content:center;width:32px;height:32px;border-radius:9px;background:color-mix(in srgb,var(--tone) 14%,transparent);color:var(--tone)}
.access-text{display:flex;flex-direction:column;gap:2px;min-width:0}
.access-text strong{font-size:13px;font-weight:600}
.access-text small{font-size:11.5px;line-height:1.35;color:var(--color-text-muted)}
.access-check{display:inline-flex;color:var(--tone)}
@keyframes access-in{from{opacity:0;transform:translateY(4px)}to{opacity:1;transform:none}}
@media (prefers-reduced-motion:reduce){.access-menu{animation:none}.access-mode :global(.chevron){transition:none}}
</style>
