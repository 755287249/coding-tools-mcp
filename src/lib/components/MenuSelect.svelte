<script lang="ts" generics="T extends string">
  import type { Component, Snippet } from 'svelte';
  import { tick } from 'svelte';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Check from '@lucide/svelte/icons/check';
  /** Compact popover select that matches the app's menus (replaces native <select>). */
  interface Option { value: T; label: string; icon?: Component<any> }
  let { value, options, label, onChange, icon }: { value: T; options: Option[]; label: string; onChange: (value: T) => void; icon?: Snippet } = $props();
  const id = `menu-select-${Math.random().toString(36).slice(2, 10)}`;
  let open = $state(false), active = $state(0), trigger = $state<HTMLButtonElement>(), menu = $state<HTMLDivElement>(), place = $state('');
  const current = $derived(options.find(option => option.value === value) ?? options[0]);
  function position() {
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect(), width = Math.max(rect.width, 170);
    const left = Math.max(8, Math.min(rect.left, window.innerWidth - width - 8));
    const below = window.innerHeight - rect.bottom;
    place = below > options.length * 36 + 24 ? `left:${left}px;top:${rect.bottom + 6}px;min-width:${width}px` : `left:${left}px;bottom:${window.innerHeight - rect.top + 6}px;min-width:${width}px`;
  }
  const items = () => menu?.querySelectorAll<HTMLElement>('[role=menuitemradio]') ?? [];
  async function show() { active = Math.max(0, options.findIndex(o => o.value === value)); position(); open = true; await tick(); items()[active]?.focus(); }
  function hide(focus = true) { open = false; if (focus) trigger?.focus(); }
  function choose(next: T) { hide(); if (next !== value) onChange(next); }
  function move(step: number) { active = (active + step + options.length) % options.length; items()[active]?.focus(); }
  function keys(event: KeyboardEvent) {
    if (event.key === 'Escape') { event.preventDefault(); hide(); }
    else if (event.key === 'ArrowDown') { event.preventDefault(); move(1); }
    else if (event.key === 'ArrowUp') { event.preventDefault(); move(-1); }
    else if (event.key === 'Tab') hide(false);
  }
  function outside(event: PointerEvent) { const target = event.target as Node; if (open && !menu?.contains(target) && !trigger?.contains(target)) hide(false); }
</script>
<svelte:window onpointerdown={outside} onresize={() => { if (open) position(); }} />
<button bind:this={trigger} type="button" class="menu-select" class:open aria-haspopup="menu" aria-expanded={open} aria-controls={open ? id : undefined}
  aria-label={`${label}: ${current?.label ?? ''}`} title={`${label}: ${current?.label ?? ''}`}
  onclick={() => open ? hide() : void show()} onkeydown={e => { if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && !open) { e.preventDefault(); void show(); } }}>
  {#if current?.icon}<current.icon size={14} />{:else if icon}{@render icon()}{/if}
  <span class="value">{current?.label}</span><ChevronDown size={12} class="chevron" />
</button>
{#if open}
  <div bind:this={menu} {id} class="menu-popover" role="menu" tabindex="-1" aria-label={label} style={place} onkeydown={keys}>
    {#each options as option, index}
      <button type="button" role="menuitemradio" aria-checked={option.value === value} tabindex={index === active ? 0 : -1} class:selected={option.value === value} onclick={() => choose(option.value)} onfocus={() => active = index}>
        {#if option.icon}<option.icon size={15} />{/if}<span>{option.label}</span><span class="check">{#if option.value === value}<Check size={14} />{/if}</span>
      </button>
    {/each}
  </div>
{/if}
<style>
  .menu-select{display:inline-flex;align-items:center;gap:6px;min-width:0;max-width:100%;height:32px;padding:0 9px;border:1px solid var(--color-border);border-radius:9px;background:var(--surface-1,var(--card-bg));color:var(--color-text);font:inherit;font-size:12.5px;cursor:pointer;transition:background .15s,border-color .15s}
  .menu-select:hover,.menu-select.open{background:var(--surface-hover);border-color:var(--border-strong,var(--color-border))}
  .menu-select:focus-visible{outline:2px solid var(--primary,#2563eb);outline-offset:2px}
  .value{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1;text-align:left}
  .menu-select :global(.chevron){flex:none;opacity:.6;transition:transform .15s}.menu-select.open :global(.chevron){transform:rotate(180deg)}
  .menu-popover{position:fixed;z-index:1000;display:flex;flex-direction:column;gap:1px;padding:5px;border:1px solid var(--color-border);border-radius:12px;background:var(--card-bg,var(--surface-2));color:var(--color-text);box-shadow:var(--dialog-shadow,0 14px 36px rgb(0 0 0 / .18))}
  .menu-popover button{display:grid;grid-template-columns:auto 1fr 16px;align-items:center;gap:9px;padding:7px 9px;border:0;border-radius:8px;background:transparent;color:inherit;font:inherit;font-size:13px;text-align:left;cursor:pointer;white-space:nowrap}
  .menu-popover button:not(:has(:global(svg:first-child))){grid-template-columns:1fr 16px}
  .menu-popover button:hover,.menu-popover button:focus-visible{background:var(--surface-hover);outline:none}
  .menu-popover button.selected{font-weight:600}
  .check{display:inline-flex;color:var(--primary,#2563eb)}
  :global(.sidebar-collapsed) .menu-select .value,:global(.sidebar-collapsed) .menu-select :global(.chevron){display:none}
  @media(max-width:700px){.menu-select{height:40px;font-size:14px}}
</style>
