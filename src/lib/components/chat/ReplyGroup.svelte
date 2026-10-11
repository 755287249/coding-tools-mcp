<script lang="ts">
  import {t} from "$lib/i18n";
  import type {ChatMessage,QuestionAnswer} from '$lib/api/chat';
  import type {ReplyGroup} from '$lib/chat/reply-groups';
  import {combinedReply} from '$lib/chat/reply-groups';
  import MessageText from './MessageText.svelte';
  import ChatToolActivity from './ChatToolActivity.svelte';
  import ChatAttachment from './ChatAttachment.svelte';
  import QuestionCard from './QuestionCard.svelte';
  import Copy from '@lucide/svelte/icons/copy';
  import Check from '@lucide/svelte/icons/check';
  import Share2 from '@lucide/svelte/icons/share-2';
  let {group,workspaceId,folderId,chatId,copied,disabled=false,onCopy,onShare,onAnswer}:{group:ReplyGroup;workspaceId:string;folderId:string;chatId:string;copied:string;disabled?:boolean;onCopy:(m:ChatMessage)=>void;onShare:(m:ChatMessage)=>void;onAnswer:(m:ChatMessage,id:string,answers:QuestionAnswer[])=>Promise<boolean>}=$props();
  const last=$derived(group.messages.at(-1)!);
  const first=$derived(group.messages[0]);
  const complete=$derived(last.final===true&&!last.awaiting_user);
  const combined=$derived(combinedReply(group,{tool:$t('chat.92'),input:$t('chat.76'),output:$t('chat.77'),truncated:$t('chat.78')}));
  const started=$derived(group.request?.received_at??first.created_at);
  const duration=$derived(Math.max(0,Math.floor((last.created_at-started)/1000)));
  const elapsed=$derived($t('reply.elapsed').replace('{minutes}',String(Math.floor(duration/60))).replace('{seconds}',String(duration%60)));
  function time(value:number){return new Date(value).toLocaleTimeString([], {hour:'2-digit',minute:'2-digit',second:'2-digit',hour12:false})}
</script>
<article class="assistant reply-group" data-reply-group={group.id}>
  <header class="reply-author"><strong>{group.name}</strong><time title={$t('reply.started')} datetime={new Date(first.created_at).toISOString()}>{time(first.created_at)}</time></header>
  <div class="reply-content">
    {#each group.messages as m (m.id)}
      {#if m.tool_event}<div class="reply-tools"><ChatToolActivity messages={[m]} settled={last.final===true} inline showTimes={false} {workspaceId} {folderId} {chatId}/></div>
      {:else}<section class="reply-segment">
        {#if (m.attachments??[]).some(f=>f.mime.startsWith('image/'))}<div class="reply-images">{#each (m.attachments??[]).filter(f=>f.mime.startsWith('image/')) as file (file.id)}<ChatAttachment {workspaceId} {folderId} {chatId} {file}/>{/each}</div>{/if}
        <MessageText text={m.text} attachments={m.attachments??[]} {workspaceId} {folderId} {chatId}/>
        {#each (m.attachments??[]).filter(f=>!f.mime.startsWith('image/')) as file (file.id)}<ChatAttachment {workspaceId} {folderId} {chatId} {file}/>{/each}
        {#if m.questions?.length}<QuestionCard message={m} {disabled} onAnswer={(id,answers)=>onAnswer(m,id,answers)}/>{/if}
      </section>{/if}
    {/each}
  </div>
  <footer class="reply-footer">
    <div class="reply-timing">{#if complete}<Check size={13}/><span>{$t('reply.completed')} <time datetime={new Date(last.created_at).toISOString()}>{time(last.created_at)}</time><span class="separator">·</span>{$t('reply.duration')} {elapsed}</span>{:else}<span>{$t(last.awaiting_user?'chat.73':'reply.updated')} <time datetime={new Date(last.created_at).toISOString()}>{time(last.created_at)}</time></span>{/if}</div>
    {#if last.final===true}<div class="reply-actions"><button aria-label={$t('reply.copy')} title={$t('reply.copy')} onclick={()=>onCopy(combined)}>{#if copied===group.id}<Check size={15}/>{:else}<Copy size={15}/>{/if}</button><button aria-label={$t('reply.share')} title={$t('reply.share')} onclick={()=>onShare(combined)}><Share2 size={15}/></button></div>{/if}
  </footer>
</article>
<style>
.reply-group{margin-bottom:26px}.reply-author{display:flex;align-items:center;gap:10px;font-size:11px;margin-bottom:14px}.reply-author strong{font-weight:600}.reply-author time{font-size:9px;color:var(--color-text-muted)}.reply-content{display:grid;gap:18px}.reply-segment{min-width:0}.reply-images{display:flex;gap:10px;overflow:auto;margin-bottom:10px}.reply-tools{padding:0}.reply-footer{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-top:18px;min-height:28px;color:var(--color-text-muted)}.reply-timing{display:flex;align-items:center;gap:6px;font-size:10px;line-height:1.6}.reply-timing :global(svg){color:#78c8a1;flex:none}.separator{margin:0 6px}.reply-actions{display:flex;gap:5px;flex:none}.reply-actions button{display:grid;place-items:center;width:29px;height:28px;border-radius:6px;cursor:pointer}.reply-actions button:hover{background:var(--surface-hover);color:var(--color-text)}.reply-actions button:focus-visible{outline:2px solid var(--primary);outline-offset:2px}@media(max-width:700px){.reply-content{gap:16px}.reply-footer{gap:6px}.reply-timing{font-size:10px}}
</style>
