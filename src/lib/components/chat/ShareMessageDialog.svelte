<script lang="ts">
  import {onMount} from 'svelte';
  import {get} from 'svelte/store';
  import {workspaces} from '$lib/stores/app';
  import {workspaceFolders} from '$lib/types';
  import {localChat,type ChatMessage,type ChatSession} from '$lib/api/chat';
  import {createMessageTransfer,forwardMessage,type MessageTransfer} from '$lib/chat/message-actions';
  import {randomId} from '$lib/browser-tools';
  import {messageBytes} from '$lib/chat/autogrow';
  import {t} from '$lib/i18n';
  import X from '@lucide/svelte/icons/x';
  import MessageSquare from '@lucide/svelte/icons/message-square';
  let {message,workspaceId,folderId,chatId,onClose}:{message:ChatMessage;workspaceId:string;folderId:string;chatId:string;onClose:()=>void}=$props();
  type Target={key:string;workspace:string;folder:string;project:string;session:ChatSession};
  let dialog:HTMLDialogElement;
  let search=$state<HTMLInputElement>();
  let targets=$state<Target[]>([]),query=$state(''),loading=$state(true),sending=$state(false),error=$state(''),selected=$state(''),done=$state(false);
  const filtered=$derived(targets.filter(target=>`${target.project} ${target.session.title} ${target.session.agent_name??''}`.toLowerCase().includes(query.toLowerCase())));
  const transfers=new Map<string,MessageTransfer>();
  onMount(()=>{
    const opener=document.activeElement;let cancelled=false;dialog.showModal();search?.focus();
    const groups=get(workspaces).flatMap(workspace=>workspaceFolders(workspace).map(folder=>({workspace:workspace.id,folder:folder.id,project:`${workspace.name} · ${folder.name}`})));
    if(!groups.some(g=>g.workspace===workspaceId&&g.folder===folderId))groups.unshift({workspace:workspaceId,folder:folderId,project:''});
    void (async()=>{try{for(const group of groups){if(cancelled)return;try{const result=await localChat(group.workspace,group.folder,{action:'list'});if(cancelled)return;targets=[...targets,...(result.sessions??[]).filter(s=>!s.closed&&!s.archived&&!(group.workspace===workspaceId&&group.folder===folderId&&s.id===chatId)).map(session=>({...group,session,key:JSON.stringify([group.workspace,group.folder,session.id])}))];}catch(e){if(!cancelled)error=String(e)}}}finally{if(!cancelled)loading=false}})();
    return()=>{cancelled=true;dialog.close();if(opener instanceof HTMLElement&&opener.isConnected)opener.focus({preventScroll:true})};
  });
  async function share(){
    const target=targets.find(t=>t.key===selected);if(!target||sending||done)return;
    if(messageBytes(message.text)>32000){error=$t('chat.tooLong');return;}
    sending=true;error='';
    try{let transfer=transfers.get(target.key);if(!transfer){transfer=createMessageTransfer(randomId);transfers.set(target.key,transfer);}await forwardMessage(message,chatId,target.session.id,args=>localChat(workspaceId,folderId,args),args=>localChat(target.workspace,target.folder,args),transfer,randomId);done=true;}
    catch(e){error=String(e)}finally{sending=false;}
  }
</script>
<dialog class="share-message" bind:this={dialog} aria-label={$t('chat.shareMessage')} oncancel={event=>{event.preventDefault();if(!sending)onClose();}}>
  <header><strong>{$t('chat.shareMessage')}</strong><button type="button" aria-label={$t('Close')} onclick={onClose} disabled={sending}><X size={18}/></button></header>
  {#if done}<p role="status">{$t('chat.messageShared')}</p><footer><button type="button" onclick={onClose}>{$t('Close')}</button></footer>
  {:else}<input bind:this={search} bind:value={query} aria-label={$t('chat.findConversation')} placeholder={$t('chat.findConversation')} disabled={sending}/>
    <div class="share-targets">{#each filtered as target (target.key)}<button type="button" class:selected={selected===target.key} aria-pressed={selected===target.key} disabled={sending} onclick={()=>selected=target.key}><MessageSquare size={17}/><span><strong>{target.session.title}</strong><small>{target.project}{target.session.agent_name?' · '+target.session.agent_name:''}</small></span></button>{:else}{#if !loading}<p>{$t('chat.noShareTargets')}</p>{/if}{/each}</div>
    {#if loading}<p role="status">{$t('Loading…')}</p>{/if}{#if error}<p class="error" role="alert">{error}</p>{/if}
    <footer><button type="button" disabled={sending} onclick={onClose}>{$t('Cancel')}</button><button type="button" disabled={!selected||sending} onclick={share}>{$t(sending?'Working…':'chat.shareMessage')}</button></footer>
  {/if}
</dialog>
<style>.share-message{margin:auto;width:min(480px,calc(100vw - 28px));max-height:85dvh;overflow:auto;border:1px solid var(--color-border);border-radius:18px;padding:18px;background:var(--surface-2);color:var(--color-text)}.share-message::backdrop{background:#0008}header,footer{display:flex;align-items:center;justify-content:space-between;gap:12px}header{margin-bottom:16px}header button{padding:6px}input{width:100%;padding:10px 12px;border:1px solid var(--color-border);border-radius:9px;background:var(--card-bg);font-size:14px}button{cursor:pointer}button:disabled{opacity:.5;cursor:default}.share-targets{display:grid;gap:5px;max-height:42dvh;overflow:auto;margin:12px 0}.share-targets button{display:flex;align-items:center;gap:12px;padding:12px;border-radius:9px;text-align:left;border:1px solid transparent}.share-targets button:hover,.share-targets button.selected{background:var(--surface-hover);border-color:var(--color-border)}.share-targets span{min-width:0}.share-targets strong,.share-targets small{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.share-targets strong{font-size:13px}.share-targets small{font-size:11px;color:var(--color-text-muted);margin-top:4px}footer{justify-content:flex-end;margin-top:14px}footer button{padding:8px 14px;border:1px solid var(--color-border);border-radius:8px;font-size:13px}footer button:last-child{background:var(--primary);color:white}p{font-size:13px;padding:10px 0}.error{color:var(--danger);overflow-wrap:anywhere}button:focus-visible,input:focus-visible{outline:2px solid var(--primary);outline-offset:2px}</style>
