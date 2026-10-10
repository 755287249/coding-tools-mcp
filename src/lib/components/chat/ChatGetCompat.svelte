<script lang="ts">
  import { localChat } from '$lib/api/chat';
  import { copyText, randomId } from '$lib/browser-tools';
  import { buildCompatPrompt } from '$lib/chat/compat-prompt';
  import { t } from '$lib/i18n';
  let {workspaceId,folderId,chatId,endpoint}: {workspaceId:string;folderId:string;chatId:string;endpoint:string}=$props();
  let busy=$state(false), error=$state(''), prompt=$state(''), copied=$state(false), expires=$state(0);
  let attempt='';
  async function issue() {
    busy=true;error='';copied=false;attempt ||= randomId();
    try {
      const result=await localChat(workspaceId,folderId,{action:'prepare_compat',chat_id:chatId,message_id:attempt});
      if(!result.compat)throw new Error('Missing compatibility authorization');
      prompt=buildCompatPrompt(endpoint,result.compat.key,chatId,folderId);expires=result.compat.expires_at;
    } catch(e){error=String(e);}finally{busy=false;}
  }
  async function copy() {try{await copyText(prompt);copied=true;}catch(e){error=String(e);}}
  async function revoke() {
    busy=true;error='';
    try{await localChat(workspaceId,folderId,{action:'revoke_compat',chat_id:chatId});prompt='';expires=0;attempt='';copied=false;}catch(e){error=String(e);}finally{busy=false;}
  }
</script>
<details class="get-compat">
  <summary>{$t('chat.getTrial')}</summary>
  <p>{$t('chat.getTrialHint')}</p>
  <button onclick={issue} disabled={busy||!endpoint}>{$t('chat.getAuthorize')}</button>
  {#if prompt}
    <button onclick={copy} disabled={busy}>{$t(copied?'chat.25':'chat.24')}</button>
    <p>{$t('chat.getExpires')} {new Date(expires).toLocaleTimeString()}</p>
  {/if}
  <button onclick={revoke} disabled={busy}>{$t('chat.getRevoke')}</button>
  {#if error}<p role="alert">{error}</p>{/if}
</details>
<style>
  .get-compat{border:1px solid var(--border-color,#7776);border-radius:8px;padding:10px;margin:10px 0;}
  summary{cursor:pointer;font-weight:600;}p{font-size:.85rem;line-height:1.5;}button{margin:4px 8px 4px 0;padding:7px 10px;border:1px solid var(--border-color,#7776);border-radius:6px;}
</style>
