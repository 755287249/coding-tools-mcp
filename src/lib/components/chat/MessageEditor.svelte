<script lang="ts">
  import {onMount} from 'svelte';
  import {randomId} from '$lib/browser-tools';
  import {messageBytes} from '$lib/chat/autogrow';
  import {t} from '$lib/i18n';
  let {text,hasAttachments,onSend,onCancel}:{text:string;hasAttachments:boolean;onSend:(text:string,id:string)=>Promise<boolean>;onCancel:()=>void}=$props();
  let value=$state(''),saving=$state(false),editor:HTMLTextAreaElement;
  let retry:{text:string;id:string}|null=null;
  onMount(()=>{value=text;editor.focus();});
  async function submit(){
    if(saving||messageBytes(value)>32000||(!value.trim()&&!hasAttachments))return;
    if(!retry||retry.text!==value)retry={text:value,id:randomId()};
    saving=true;try{if(await onSend(value,retry.id))onCancel();}finally{saving=false;}
  }
</script>
<div class="message-editor">
  <textarea bind:this={editor} bind:value aria-label={$t('chat.editMessage')} disabled={saving} onkeydown={event=>{if(event.isComposing)return;if(event.key==='Escape'&&!saving)onCancel();if(event.key==='Enter'&&(event.ctrlKey||event.metaKey)){event.preventDefault();void submit();}}}></textarea>
  {#if messageBytes(value)>32000}<p role="alert">{$t('chat.tooLong')}</p>{/if}
  <div><button type="button" disabled={saving} onclick={onCancel}>{$t('Cancel')}</button><button type="button" class="resend" disabled={saving||messageBytes(value)>32000||(!value.trim()&&!hasAttachments)} onclick={submit}>{$t(saving?'Working…':'chat.resendMessage')}</button></div>
</div>
<style>.message-editor{width:100%;min-width:min(480px,100%)}textarea{display:block;width:100%;height:260px;max-height:45dvh;min-height:120px;resize:vertical;border:0;background:transparent;color:inherit;font:13px/1.7 system-ui;outline:none}.message-editor>div{display:flex;justify-content:flex-end;gap:8px;margin-top:12px}button{padding:6px 12px;border:1px solid #ffffff30;border-radius:8px;font-size:12px;cursor:pointer}.resend{background:#eee;color:#222}button:disabled{opacity:.5;cursor:default}button:focus-visible{outline:2px solid var(--primary);outline-offset:2px}p{font-size:12px;color:var(--danger)}@media(max-width:700px){textarea{font-size:16px}}</style>
