<script lang="ts">
 import { page } from '$app/stores';
 import { goto } from '$app/navigation';
 import { appUrl } from '$lib/app-path';
 import { workspaces } from '$lib/stores/app';
 import { workspaceFolders } from '$lib/types';
 import { navigationSessions } from '$lib/chat/navigation-state';
 import { isConnectedConversation, conversationOrder, sessionPresence } from '$lib/chat/navigation';
 import { chatLocation } from '$lib/chat/location';
 import { discussionLocation } from '$lib/chat/discussion-navigation';
 import { t } from '$lib/i18n';
 import ChevronDown from '@lucide/svelte/icons/chevron-down';
 import Check from '@lucide/svelte/icons/check';
 let {id}:{id:string}=$props();
 let picker=$state<HTMLDivElement>(),trigger=$state<HTMLButtonElement>();
 let left=$state(8),top=$state(64);
 const groups=$derived($workspaces.flatMap(workspace=>workspaceFolders(workspace).map(folder=>({workspace,folder,key:JSON.stringify([workspace.id,folder.id])}))));
 const activeId=$derived($page.url.searchParams.get('chat')??($page.url.searchParams.get('discussion')?'discussion:'+$page.url.searchParams.get('discussion'):''));
 const activeGroup=$derived(groups.find(g=>g.workspace.id===$page.params.id&&g.folder.id===$page.url.searchParams.get('folder')));
 const active=$derived(activeGroup?$navigationSessions[activeGroup.key]?.find(c=>c.id===activeId):undefined);
 const presenceLabels={online:'chat.98',offline:'chat.22',working:'chat.70',error:'chat.71',closed:'chat.21',queued:'chat.81',awaiting_user:'chat.73'} as const;
 const items=$derived(groups.flatMap(group=>($navigationSessions[group.key]??[]).filter(isConnectedConversation).map(chat=>({group,chat}))).sort((a,b)=>conversationOrder(a.chat,b.chat)));
 function position(){if(!picker?.matches(':popover-open')||!trigger)return;const r=trigger.getBoundingClientRect();left=Math.max(8,Math.min(r.left,innerWidth-picker.offsetWidth-8));top=Math.max(8,Math.min(r.bottom+6,innerHeight-picker.offsetHeight-8));}
 $effect(()=>{const route=$page.url.href;picker?.hidePopover()});
</script>
<svelte:window onresize={position} onscrollcapture={position}/>
<button class="session-picker-trigger" bind:this={trigger} popovertarget={id} title={$t('chat.113')} aria-label={$t('chat.113')}><strong>{active?.title??$t('chat.99')}</strong><ChevronDown size={15}/></button>
<div {id} class="conversation-picker" popover="auto" bind:this={picker} ontoggle={position} style:left={`${left}px`} style:top={`${top}px`}>
 {#each items as {group,chat} (JSON.stringify([group.key,chat.id]))}{@const selected=activeGroup?.key===group.key&&active?.id===chat.id}
  <button class="picker-chat" aria-current={selected?'page':undefined} onclick={()=>{picker?.hidePopover();goto(appUrl(chat.discussionId?discussionLocation(group.workspace.id,group.folder.id,chat.discussionId):chatLocation(group.workspace.id,group.folder.id,chat.id)))}}><span><strong>{chat.title}{#if chat.note}<span class="conversation-note">{chat.note}</span>{/if}</strong><small>{group.workspace.name} · {$t(presenceLabels[sessionPresence(chat)])}</small></span>{#if selected}<Check size={16}/>{/if}</button>
 {:else}<p>{$t('chat.noConnected')}</p>{/each}
</div>
<style>
.conversation-note{margin-left:7px;font-size:11px;color:var(--color-text-muted)}.picker-chat[aria-current=page]{background:var(--surface-hover)}
.session-picker-trigger{display:flex;align-items:center;gap:7px;min-width:0;max-width:calc(100% - 28px);padding:6px;text-align:left;border-radius:6px;cursor:pointer}.session-picker-trigger strong{font-size:18px;font-weight:600;line-height:26px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.session-picker-trigger :global(svg){flex:none}.conversation-picker{position:fixed;inset:auto;margin:0;width:min(286px,calc(100vw - 16px));max-height:50dvh;overflow:auto;padding:6px;border:1px solid var(--color-border);border-radius:12px;background:var(--surface-2);color:var(--color-text);box-shadow:0 10px 32px #0005}.picker-chat{display:flex;align-items:center;gap:10px;width:100%;text-align:left;padding:10px;border-radius:8px;cursor:pointer}.picker-chat:hover,.session-picker-trigger:hover{background:var(--surface-hover)}.picker-chat>span{flex:1;min-width:0}.picker-chat strong,.picker-chat small{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.picker-chat strong{font-size:14px;font-weight:500}.picker-chat small{font-size:11px;color:var(--color-text-muted);margin-top:4px}p{padding:10px;font-size:12px;color:var(--color-text-muted)}button:focus-visible{outline:2px solid var(--primary)}
</style>
