<script lang="ts">
  import {t} from '$lib/i18n';
  import type {ChatApproval} from '$lib/api/chat';
  /** Strict mode: the AI's pending operations wait for the user's decision here. */
  let {approvals,busy=false,onResolve}:{approvals:ChatApproval[];busy?:boolean;onResolve:(id:string,approve:boolean)=>void}=$props();
</script>
{#if approvals.length}
<div class="approvals" role="region" aria-label={$t('chat.access.approvals')}>
  {#each approvals as approval (approval.id)}
  <div class="approval" role="alertdialog" aria-label={$t('chat.access.approvalTitle')}>
    <div class="body"><strong>{$t('chat.access.approvalTitle')}</strong><pre>{approval.summary}</pre></div>
    <div class="actions">
      <button type="button" class="deny" disabled={busy} onclick={()=>onResolve(approval.id,false)}>{$t('chat.access.deny')}</button>
      <button type="button" class="allow" disabled={busy} onclick={()=>onResolve(approval.id,true)}>{$t('chat.access.allow')}</button>
    </div>
  </div>
  {/each}
</div>
{/if}
<style>
.approvals{display:grid;gap:8px;max-width:760px;margin:0 auto 10px}
.approval{display:flex;gap:12px;align-items:flex-start;justify-content:space-between;padding:10px 12px;border:1px solid color-mix(in srgb,#d97706 45%,var(--color-border));border-radius:12px;background:color-mix(in srgb,#d97706 8%,var(--surface-2))}
.body{min-width:0;display:grid;gap:4px;font-size:12px}
pre{margin:0;font:12px/1.6 var(--font-mono,monospace);white-space:pre-wrap;overflow-wrap:anywhere;max-height:96px;overflow:auto;color:var(--color-text-secondary,var(--color-text-muted))}
.actions{display:flex;gap:6px;flex:none}
button{padding:6px 12px;border:1px solid var(--color-border);border-radius:8px;font-size:12px;cursor:pointer;background:var(--surface-2);color:var(--color-text)}
button:disabled{opacity:.5;cursor:wait}
.allow{background:var(--primary,#2563eb);border-color:var(--primary,#2563eb);color:#fff}
.deny:hover{color:var(--danger)}
</style>
