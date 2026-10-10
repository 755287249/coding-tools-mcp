<script lang="ts">
  import {untrack} from 'svelte';
  import {t} from '$lib/i18n';
  import {getBackend} from '$lib/backend';
  const projectActivityAvailable=getBackend().capabilities.liveHistoryActivity;
  import type {ChatSession} from '$lib/api/chat';
  import {currentChatTask} from '$lib/chat/task-state';
  import ChatOperations from './ChatOperations.svelte';
  import ActivityDrawer from '../activity/ActivityDrawer.svelte';
  import Check from '@lucide/svelte/icons/check';
  import X from '@lucide/svelte/icons/x';
  import Maximize2 from '@lucide/svelte/icons/maximize-2';
  import Minimize2 from '@lucide/svelte/icons/minimize-2';
  import LoaderCircle from '@lucide/svelte/icons/loader-circle';
  let {workspaceId,width=$bindable(350),session,expanded=false,onToggleExpanded,onClose}:{workspaceId:string;width?:number;session:ChatSession|null;expanded?:boolean;onToggleExpanded:()=>void;onClose:()=>void}=$props();
  let view=$state<'conversation'|'workspace'>('conversation');
  let requestId=$state('');
  const requests=$derived((session?.messages??[]).filter(m=>m.role==='user'&&m.kind!=='connection_request'));
  const sessionId=$derived(session?.id);
  $effect(()=>{sessionId;requestId='';});
  const task=$derived(currentChatTask(session,requestId||undefined));
  const steps=$derived(task?.plan?.todos??[]);
  const done=$derived(steps.filter(t=>t.status==='completed').length);
  const labels={queued:'chat.81',processing:'chat.82',awaiting_user:'chat.73',completed:'Completed',closed:'chat.4',interrupted:'chat.71'} as const;
  let panel=$state<HTMLElement>();let maxWidth=$state(700);
  let drag:{x:number;width:number;id:number}|null=null;
  function setWidth(value:number){width=Math.max(Math.min(280,maxWidth),Math.min(maxWidth,value));}
  $effect(()=>{const wide=expanded;untrack(()=>setWidth(wide?600:350))});
  $effect(()=>{const parent=panel?.parentElement;if(!parent)return;const observer=new ResizeObserver(()=>{maxWidth=Math.max(180,Math.min(900,parent.clientWidth>=800?parent.clientWidth*.6:parent.clientWidth*.85));setWidth(width)});observer.observe(parent);return()=>observer.disconnect()});
  function begin(event:PointerEvent){if(event.button!==0)return;event.preventDefault();(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);drag={x:event.clientX,width,id:event.pointerId};}
  function move(event:PointerEvent){if(drag?.id===event.pointerId)setWidth(drag.width+drag.x-event.clientX);}
  function end(event:PointerEvent){if(drag?.id!==event.pointerId)return;drag=null;const el=event.currentTarget as HTMLElement;if(el.hasPointerCapture(event.pointerId))el.releasePointerCapture(event.pointerId);}
</script>
<aside bind:this={panel} class="chat-task-panel" style:width={`${width}px`} aria-label={$t('Task panel')}>
  <button type="button" class="resize-edge" title={$t('chat.resizeTasks')} aria-label={$t('chat.resizeTasks')}
    onpointerdown={begin} onpointermove={move} onpointerup={end} onpointercancel={end} onlostpointercapture={()=>drag=null}
    onkeydown={event=>{if(event.key==='ArrowLeft'||event.key==='ArrowRight'){event.preventDefault();setWidth(width+(event.key==='ArrowLeft'?20:-20))}}}></button>
  <header><div><strong>{$t('Task panel')}</strong><p title={session?.title}>{session?.title??$t('chat.taskEmpty')}</p></div><button type="button" title={$t(expanded?'Restore':'Maximize')} aria-label={$t(expanded?'Restore':'Maximize')} onclick={onToggleExpanded}>{#if expanded}<Minimize2 size={15}/>{:else}<Maximize2 size={15}/>{/if}</button><button type="button" aria-label={$t('chat.67')} title={$t('chat.67')} onclick={onClose}><X size={17}/></button></header>
  <div class="panel-tabs" role="tablist" aria-label={$t('chat.operationScope')}><button type="button" role="tab" aria-selected={view==='conversation'} onclick={()=>view='conversation'}>{$t('chat.thisConversation')}</button><button type="button" role="tab" aria-selected={view==='workspace'} onclick={()=>view='workspace'}>{$t('chat.projectActivity')}</button></div>
  {#if view==='workspace'}
    <div class="workspace-activity"><p class="scope-note">{$t('chat.projectActivityHint')}</p>{#if projectActivityAvailable}{#key workspaceId}<ActivityDrawer embedded {workspaceId} live={session?.status==='connected'||session?.status==='waiting'} {expanded} {onToggleExpanded} {onClose}/>{/key}{:else}<p class="scope-note" role="status">{$t('chat.projectActivityDesktop')}</p><button type="button" class="scope-back" onclick={()=>view='conversation'}>{$t('chat.backConversation')}</button>{/if}</div>
  {:else}
  <div class="panel-content">
    {#if requests.length}<label class="request-select">{$t('chat.taskHistory')}<select bind:value={requestId}><option value="">{$t('chat.currentTask')}</option>{#each requests.toReversed() as request(request.id)}<option value={request.id}>{request.text.slice(0,90)||request.id}</option>{/each}</select></label>{/if}
    {#if task}
      <section class="current-task">
        <div class="task-heading"><span>{$t('Build goal')}</span><span class="task-state" data-state={task.status}>{#if task.status==='processing'}<LoaderCircle size={12} class="chat-task-spinner"/>{:else if task.status==='completed'}<Check size={12}/>{/if}{$t(labels[task.status])}</span></div>
        <h3>{task.plan?.goal||task.message.text}</h3>
        {#if steps.length}
          <div class="steps-heading"><span>{$t('chat.taskSteps')}</span><span>{done} / {steps.length}</span></div>
          <div class="step-track" aria-hidden="true"><div style:width={`${done/steps.length*100}%`}></div></div>
          <ol>{#each steps as step(step.id)}<li data-state={step.status} aria-current={step.status==='in_progress'?'step':undefined}><span class="step-icon">{#if step.status==='completed'}<Check size={13}/>{:else if step.status==='in_progress'&&task.status==='processing'}<LoaderCircle size={13} class="chat-task-spinner"/>{:else}<i></i>{/if}</span><span>{step.title}</span></li>{/each}</ol>
        {:else if !task.agentTasks.length}<p class="empty-plan">{$t('chat.noTaskSteps')}</p>{/if}
      </section>
      {#if task.agentTasks.some(item=>!item.plan||item.plan!==task.plan)}<section class="agent-tasks"><h4>{$t('chat.groupMembers')}</h4>{#each task.agentTasks.filter(item=>!item.plan||item.plan!==task.plan) as item(item.id)}<details open><summary>{item.name}<small>{item.complete?$t('Completed'):item.accepted?$t('chat.82'):$t('chat.81')}</small></summary><p>{item.plan?.goal??item.text}</p>{#if item.plan}<ol>{#each item.plan.todos as todo(todo.id)}<li>{todo.status==='completed'?'✓':todo.status==='in_progress'?'◉':'○'} {todo.title}</li>{/each}</ol>{#if item.plan.progress}<p>{item.plan.progress.message}</p>{/if}{/if}</details>{/each}</section>{/if}
      {#if task.progress}<section class="latest-progress"><h4>{$t('chat.taskProgress')}</h4><p>{task.progress}</p>{#if task.plan?.progress?.percent!==undefined}<small>{$t('Agent estimate')} · {task.plan.progress.percent}%</small>{/if}</section>{/if}

    {:else}<p class="panel-empty">{$t('chat.taskEmpty')}</p>{/if}
    <ChatOperations {session} request={task?.message.id}/>
  </div>{/if}
  <footer><span>{$t('chat.thisConversation')}</span><span>{$t('Completed')} {task?.completed??0}</span>{#if session?.queued_messages?.length}<span>{$t('chat.outbox')} {session.queued_messages.length}</span>{/if}</footer>
</aside>
<style>
.scope-back{margin:8px 18px;padding:10px;border:1px solid var(--color-border);border-radius:6px}.panel-tabs{display:flex;border-bottom:1px solid var(--color-border);padding:6px 12px;gap:6px}.panel-tabs button{flex:1;padding:9px;color:var(--color-text-muted);border-radius:6px}.panel-tabs [aria-selected=true]{background:var(--color-border);color:var(--color-text)}.workspace-activity{min-height:0;flex:1;display:flex;flex-direction:column;overflow:hidden}.scope-note{padding:10px 18px;color:var(--color-text-muted);font-size:11px;line-height:1.5}.request-select{display:flex;flex-direction:column;gap:7px;color:var(--color-text-muted);font-size:11px;margin-bottom:20px}.request-select select{width:100%;min-width:0;padding:8px;background:var(--surface-2);color:var(--color-text);border:1px solid var(--color-border);border-radius:6px}

/* Docked split pane: full height on the right, separated from the conversation by a divider line (not a floating card). */
.chat-task-panel{position:absolute;z-index:30;top:0;right:0;bottom:0;max-width:100%;display:flex;flex-direction:column;border:0;border-left:1px solid var(--color-border);border-radius:0;background:var(--surface-1);color:var(--color-text);box-shadow:none;min-height:180px;font-size:12px;text-align:left}
header{display:flex;gap:4px;align-items:center;padding:15px 12px 14px 18px;border-bottom:1px solid var(--color-border)}header>div{min-width:0;flex:1}header strong{font-size:14px;font-weight:550}header p{font-size:11px;color:var(--color-text-muted);margin-top:5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}header button{display:grid;place-items:center;width:28px;height:28px;color:var(--color-text-secondary);border-radius:6px}button{cursor:pointer}button:hover{background:var(--color-border)}button:focus-visible,summary:focus-visible{outline:2px solid var(--chat-link);outline-offset:1px}
.panel-content{flex:1;min-height:0;overflow:auto;padding:20px 18px}.task-heading{display:flex;justify-content:space-between;align-items:center;gap:10px;color:var(--color-text-muted);font-size:11px}.task-state{display:flex;align-items:center;gap:5px;color:var(--color-text-secondary);white-space:nowrap}.task-state[data-state=processing]{color:var(--chat-link)}.task-state[data-state=completed]{color:var(--success)}.task-state[data-state=awaiting_user]{color:var(--warning)}h3{max-height:120px;overflow:auto;font-size:14px;font-weight:500;line-height:1.7;margin:12px 0 22px;white-space:pre-wrap;overflow-wrap:anywhere}.steps-heading{display:flex;justify-content:space-between;color:var(--color-text-secondary);font-size:11px}.step-track{height:3px;background:var(--color-border);border-radius:2px;overflow:hidden;margin:10px 0 16px}.step-track>div{height:100%;background:#a9b6c9;transition:width .2s}ol{display:flex;flex-direction:column;gap:15px}li{display:flex;gap:10px;line-height:1.6;color:var(--color-text-secondary);overflow-wrap:anywhere}li[data-state=in_progress]{color:var(--color-text)}li[data-state=completed]{color:var(--color-text-muted)}.step-icon{display:grid;place-items:center;flex:none;width:16px;height:19px}.step-icon i{width:6px;height:6px;border:1px solid var(--color-text-muted);border-radius:50%}.empty-plan,.panel-empty{color:var(--color-text-muted);line-height:1.8;margin:16px 0}.latest-progress{border-top:1px solid var(--color-border);margin-top:24px;padding-top:20px}h4{font-size:11px;color:var(--color-text-muted);margin-bottom:12px;font-weight:400}.latest-progress p{max-height:160px;overflow:auto;font-size:12px;line-height:1.8;white-space:pre-wrap;overflow-wrap:anywhere}.latest-progress small{display:block;color:var(--color-text-muted);font-size:10px;margin-top:12px}footer{display:flex;gap:10px;flex-wrap:wrap;border-top:1px solid var(--color-border);padding:12px 18px;font-size:10px;color:var(--color-text-muted)}.resize-edge{position:absolute;left:-4px;top:0;bottom:0;width:8px;cursor:ew-resize;touch-action:none;border-radius:6px}.resize-edge:hover{background:var(--color-border)}:global(.chat-task-spinner){animation:chat-task-spin 1s linear infinite}@keyframes chat-task-spin{to{transform:rotate(360deg)}}@media(prefers-reduced-motion:reduce){:global(.chat-task-spinner){animation:none}}
.agent-tasks{border-top:1px solid var(--color-border);padding-top:18px;margin-top:18px}.agent-tasks details{border:1px solid var(--color-border);border-radius:9px;padding:10px;margin:10px 0}.agent-tasks summary{display:flex;justify-content:space-between;gap:10px;cursor:pointer}.agent-tasks small{font-size:10px;color:var(--color-text-muted)}.agent-tasks p{font-size:11px;white-space:pre-wrap;overflow-wrap:anywhere;margin:10px 0;color:var(--color-text-secondary)}.agent-tasks ol{gap:7px}
</style>
