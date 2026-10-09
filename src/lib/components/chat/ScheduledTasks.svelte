<script lang="ts">
  import { randomId } from "$lib/browser-tools";
  import { localChat, type ChatSession } from '$lib/api/chat';
  import { scheduledChats, scheduleError, changeSchedule, type ScheduledChat } from '$lib/chat/schedules';
  import { t } from '$lib/i18n';
  let {workspaceId,folderId}:{workspaceId:string;folderId:string}=$props();
  let chats=$state<ChatSession[]>([]), chatId=$state(''), text=$state(''), due=$state(''), error=$state(''), saving=$state(false);
  const tasks=$derived($scheduledChats.filter(task=>task.workspaceId===workspaceId&&task.folderId===folderId));
  $effect(()=>{const ws=workspaceId,folder=folderId;let cancelled=false;chats=[];chatId='';error='';
    void localChat(ws,folder,{action:'list'}).then(result=>{if(!cancelled)chats=(result.sessions??[]).filter(chat=>!chat.closed)}).catch(e=>{if(!cancelled)error=String(e)});
    return()=>{cancelled=true};
  });
  async function save(){
    const chat=chats.find(c=>c.id===chatId), when=new Date(due).getTime();
    if(saving||!chat||!text.trim()||!Number.isFinite(when)||when<=Date.now()){error=$t('shell.scheduleInvalid');return}
    saving=true;error='';
    const task:ScheduledChat={id:randomId(),workspaceId,folderId,chatId,title:chat.title,text:text.trim(),due:when,status:'pending'};
    try{await changeSchedule(tasks=>[...tasks,task]);text='';due=''}catch(e){error=String(e)}finally{saving=false}
  }
  async function update(id:string,status?:ScheduledChat['status']) {try{await changeSchedule(tasks=>status?tasks.map(task=>task.id===id?{...task,status}:task):tasks.filter(task=>task.id!==id))}catch(e){error=String(e)}}
</script>
<div class="schedule-page"><p class="notice">{$t('shell.scheduleNotice')}</p>
{#if error||$scheduleError}<p role="alert">{error||$scheduleError}</p>{/if}
<form onsubmit={event=>{event.preventDefault();void save()}}>
<label>{$t('shell.conversation')}<select bind:value={chatId} required><option value="">{$t('shell.chooseConversation')}</option>{#each chats as chat}<option value={chat.id}>{chat.title}</option>{/each}</select></label>
<label>{$t('shell.sendAt')}<input type="datetime-local" bind:value={due} required/></label>
<label>{$t('shell.taskMessage')}<textarea rows="3" bind:value={text} maxlength="32000" required></textarea></label>
<button disabled={saving||!chats.length}>{$t('shell.schedule')}</button>
</form>
<div class="tasks">{#each tasks as task (task.id)}<article><strong>{task.title}</strong><time>{new Date(task.due).toLocaleString()}</time><p>{task.text}</p><small>{$t(`shell.task.${task.status}`)}</small>{#if task.error}<p role="alert">{task.error}</p>{/if}<div>{#if task.status==='pending'}<button onclick={()=>update(task.id,'paused')}>{$t('shell.pause')}</button>{:else if task.status==='paused'||task.status==='error'}<button onclick={()=>update(task.id,'pending')}>{$t('shell.resume')}</button>{/if}<button disabled={task.status==='sending'} onclick={()=>update(task.id)}>{$t('Delete')}</button></div></article>{:else}<p>{$t('shell.noTasks')}</p>{/each}</div>
</div>
<style>.schedule-page{max-width:800px;margin:auto}.notice{font-size:12px;line-height:1.7;color:var(--color-text-muted);margin-bottom:20px}form{display:grid;gap:14px}label{display:grid;gap:6px;font-size:12px}select,input,textarea{padding:10px;border:1px solid var(--color-border);border-radius:8px;background:var(--card-bg);min-width:0}button{padding:8px 14px;border:1px solid var(--color-border);border-radius:8px;cursor:pointer}form>button{justify-self:start;background:#2563eb;color:#fff}.tasks{display:grid;gap:12px;margin-top:24px}article{border:1px solid var(--color-border);padding:15px;border-radius:12px}article p{white-space:pre-wrap;overflow-wrap:anywhere;font-size:13px;margin:12px 0}time,small{display:block;font-size:11px;color:var(--color-text-muted);margin:8px 0}article>div{display:flex;gap:8px}[role=alert]{color:var(--danger);overflow-wrap:anywhere}button:disabled{opacity:.5}</style>
