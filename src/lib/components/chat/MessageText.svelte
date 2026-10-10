<script lang="ts">
  import type {ChatFile} from '$lib/api/chat';
  import {messagePreview} from '$lib/chat/message-actions';
  import {t} from '$lib/i18n';
  import ChatMarkdown from './ChatMarkdown.svelte';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  let {text,attachments,workspaceId,folderId,chatId,collapsible=false}:{text:string;attachments:ChatFile[];workspaceId:string;folderId:string;chatId:string;collapsible?:boolean}=$props();
  let expanded=$state(false);
  const preview=$derived(collapsible?messagePreview(text):text);
</script>
<ChatMarkdown text={expanded?text:preview} {attachments} {workspaceId} {folderId} {chatId}/>
{#if preview!==text}<button type="button" class="message-expand" aria-expanded={expanded} onclick={()=>expanded=!expanded}>{$t(expanded?'chat.showLess':'chat.showMore')}<ChevronDown size={14} style={expanded?'transform:rotate(180deg)':undefined}/></button>{/if}
<style>.message-expand{display:flex;align-items:center;gap:5px;font-size:12px;color:inherit;opacity:.7;margin-top:10px;cursor:pointer}.message-expand:hover{opacity:1}.message-expand:focus-visible{outline:2px solid var(--primary);outline-offset:3px}</style>
