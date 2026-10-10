<script lang="ts">
  import {localChat} from '$lib/api/chat';
  import {t} from '$lib/i18n';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  let {workspaceId,folderId,chatId,path,label,iconOnly=false}: {workspaceId:string;folderId:string;chatId:string;path:string;label?:string;iconOnly?:boolean}=$props();
  let busy=$state(false),error=$state('');
  async function reveal(){if(busy)return;busy=true;error='';try{await localChat(workspaceId,folderId,{action:'reveal_path',chat_id:chatId,source_path:path});}catch(e){error=String(e)}finally{busy=false}}
</script>
<button type="button" class="local-path-link" class:icon-only={iconOnly} title={`${$t('chat.showInFolder')}: ${path}`} aria-label={iconOnly?`${$t('chat.showInFolder')}: ${path}`:undefined} onclick={reveal} disabled={busy}>{#if iconOnly}<FolderOpen size={16}/>{:else}{label??path}{/if}</button>
{#if error}<span class="path-error" role="alert">{error}</span>{/if}
<style>.local-path-link{display:inline;color:var(--chat-link);text-decoration:underline;text-underline-offset:3px;text-align:left;cursor:pointer;overflow-wrap:anywhere}.local-path-link:disabled{opacity:.5}.local-path-link.icon-only{display:grid;place-items:center;width:32px;height:32px;flex:none;border:1px solid var(--color-border);border-radius:8px;color:var(--color-text-muted);text-decoration:none}.icon-only:hover{background:var(--surface-hover);color:var(--color-text)}.icon-only:focus-visible{outline:2px solid var(--primary);outline-offset:2px}.path-error{display:block;color:var(--danger);font-size:11px;overflow-wrap:anywhere}</style>
