<script lang="ts">
  import {untrack} from 'svelte';
  import {t} from '$lib/i18n';
  import type {ChatSession} from '$lib/api/chat';
  import {currentChatTask} from '$lib/chat/task-state';
  import Check from '@lucide/svelte/icons/check';
  import X from '@lucide/svelte/icons/x';
  import Maximize2 from '@lucide/svelte/icons/maximize-2';
  import Minimize2 from '@lucide/svelte/icons/minimize-2';
  import LoaderCircle from '@lucide/svelte/icons/loader-circle';
  let {width=$bindable(350),session,expanded=false,onToggleExpanded,onClose}:{width?:number;session:ChatSession|null;expanded?:boolean;onToggleExpanded:()=>void;onClose:()=>void}=$props();
  const task=$derived(currentChatTask(session));
  const steps=$derived(task?.plan?.todos??[]);
  const done=$derived(steps.filter(t=>t.status==='completed').length);
  const labels={queued:'chat.81',processing:'chat.82',awaiting_user:'chat.73',completed:'Completed',closed:'chat.4',interrupted:'chat.71'} as const;
  let panel=$state<HTMLElement>();let maxWidth=$state(700);
  let drag:{x:number;width:number;id:number}|null=null;
  function setWidth(value:number){width=Math.max(Math.min(280,maxWidth),Math.min(maxWidth,value));}
  $effect(()=>{const wide=expanded;untrack(()=>setWidth(wide?600:350))});
  $effect(()=>{const parent=panel?.parentElement;if(!parent)return;const observer=new ResizeObserver(()=>{maxWidth=Math.max(180,Math.min(900,parent.clientWidth>=800?parent.clientWidth*.6:parent.clientWidth-24));setWidth(width)});observer.observe(parent);return()=>observer.disconnect()});
  function begin(event:PointerEvent){if(event.button!==0)return;event.preventDefault();(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);drag={x:event.clientX,width,id:event.pointerId};}
  function move(event:PointerEvent){if(drag?.id===event.pointerId)setWidth(drag.width+drag.x-event.clientX);}
  function end(event:PointerEvent){if(drag?.id!==event.pointerId)return;drag=null;const el=event.currentTarget as HTMLElement;if(el.hasPointerCapture(event.pointerId))el.releasePointerCapture(event.pointerId);}
</script>
<aside bind:this={panel} class="chat-task-panel" style:width={`${width}px`} aria-label={$t('Task panel')}>
  <button type="button" class="resize-edge" title={$t('chat.resizeTasks')} aria-label={$t('chat.resizeTasks')}
    onpointerdown={begin} onpointermove={move} onpointerup={end} onpointercancel={end} onlostpointercapture={()=>drag=null}
    onkeydown={event=>{if(event.key==='ArrowLeft'||event.key==='ArrowRight'){event.preventDefault();setWidth(width+(event.key==='ArrowLeft'?20:-20))}}}></button>
  <header><div><strong>{$t('Task panel')}</strong><p title={session?.title}>{session?.title??$t('chat.taskEmpty')}</p></div><button type="button" title={$t(expanded?'Restore':'Maximize')} aria-label={$t(expanded?'Restore':'Maximize')} onclick={onToggleExpanded}>{#if expanded}<Minimize2 size={15}/>{:else}<Maximize2 size={15}/>{/if}</button><button type="button" aria-label={$t('chat.67')} title={$t('chat.67')} onclick={onClose}><X size={17}/></button></header>
  <div class="panel-content">
    {#if task}
      <section class="current-task">
        <div class="task-heading"><span>{$t('chat.currentTask')}</span><span class="task-state" data-state={task.status}>{#if task.status==='processing'}<LoaderCircle size={12} class="chat-task-spinner"/>{:else if task.status==='completed'}<Check size={12}/>{/if}{$t(labels[task.status])}</span></div>
        <h3>{task.plan?.goal||task.message.text}</h3>
        {#if steps.length}
          <div class="steps-heading"><span>{$t('chat.taskSteps')}</span><span>{done} / {steps.length}</span></div>
          <div class="step-track" aria-hidden="true"><div style:width={`${done/steps.length*100}%`}></div></div>
          <ol>{#each steps as step(step.id)}<li data-state={step.status} aria-current={step.status==='in_progress'?'step':undefined}><span class="step-icon">{#if step.status==='completed'}<Check size={13}/>{:else if step.status==='in_progress'&&task.status==='processing'}<LoaderCircle size={13} class="chat-task-spinner"/>{:else}<i></i>{/if}</span><span>{step.title}</span></li>{/each}</ol>
        {:else if !task.agentTasks.length}<p class="empty-plan">{$t('chat.noTaskSteps')}</p>{/if}
      </section>
      {#if task.agentTasks.length}<section class="agent-tasks"><h4>{$t('chat.groupMembers')}</h4>{#each task.agentTasks as item(item.id)}<details open><summary>{item.name}<small>{item.complete?$t('Completed'):item.accepted?$t('chat.82'):$t('chat.81')}</small></summary><p>{item.plan?.goal??item.text}</p>{#if item.plan}<ol>{#each item.plan.todos as todo(todo.id)}<li>{todo.status==='completed'?'✓':todo.status==='in_progress'?'◉':'○'} {todo.title}</li>{/each}</ol>{#if item.plan.progress}<p>{item.plan.progress.message}</p>{/if}{/if}</details>{/each}</section>{/if}
      {#if task.progress}<section class="latest-progress"><h4>{$t('chat.taskProgress')}</h4><p>{task.progress}</p>{#if task.plan?.progress?.percent!==undefined}<small>{$t('Agent estimate')} · {task.plan.progress.percent}%</small>{/if}</section>{/if}
      {#if task.tools.length}<section class="task-tools"><h4>{$t('chat.taskTools')}</h4>{#each task.tools.slice(-12).reverse() as message(message.id)}<details><summary><span class:failed={message.tool_event?.status==='failed'}>{message.tool_event?.name}</span><small>{message.tool_event?.status==='running'?$t('Running'):message.tool_event?.status==='completed'?$t('Completed'):$t('Failed')}</small></summary><p>{message.text}</p>{#if message.tool_event?.input}<pre>{message.tool_event.input}</pre>{/if}{#if message.tool_event?.output}<pre>{message.tool_event.output}</pre>{/if}</details>{/each}</section>{/if}
    {:else}<p class="panel-empty">{$t('chat.taskEmpty')}</p>{/if}
  </div>
  <footer><span>{$t('chat.thisConversation')}</span><span>{$t('Completed')} {task?.completed??0}</span>{#if session?.queued_messages?.length}<span>{$t('chat.outbox')} {session.queued_messages.length}</span>{/if}</footer>
</aside>
<style>
.chat-task-panel{position:absolute;z-index:30;top:76px;right:12px;bottom:12px;max-width:calc(100% - 24px);display:flex;flex-direction:column;border:1px solid #ffffff20;border-radius:14px;background:#202020;color:#eee;box-shadow:0 16px 44px #0006;min-height:180px;font-size:12px;text-align:left}
header{display:flex;gap:4px;align-items:center;padding:15px 12px 14px 18px;border-bottom:1px solid #ffffff10}header>div{min-width:0;flex:1}header strong{font-size:14px;font-weight:550}header p{font-size:11px;color:#999;margin-top:5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}header button{display:grid;place-items:center;width:28px;height:28px;color:#aaa;border-radius:6px}button{cursor:pointer}button:hover{background:#ffffff0c}button:focus-visible,summary:focus-visible{outline:2px solid #7aa8e8;outline-offset:1px}
.panel-content{flex:1;min-height:0;overflow:auto;padding:20px 18px}.task-heading{display:flex;justify-content:space-between;align-items:center;gap:10px;color:#999;font-size:11px}.task-state{display:flex;align-items:center;gap:5px;color:#b1b1b1;white-space:nowrap}.task-state[data-state=processing]{color:#98bff1}.task-state[data-state=completed]{color:#9ebea4}.task-state[data-state=awaiting_user]{color:#d7ba83}h3{font-size:14px;font-weight:500;line-height:1.7;margin:12px 0 22px;white-space:pre-wrap;overflow-wrap:anywhere}.steps-heading{display:flex;justify-content:space-between;color:#aaa;font-size:11px}.step-track{height:3px;background:#ffffff0d;border-radius:2px;overflow:hidden;margin:10px 0 16px}.step-track>div{height:100%;background:#a9b6c9;transition:width .2s}ol{display:flex;flex-direction:column;gap:15px}li{display:flex;gap:10px;line-height:1.6;color:#aaa;overflow-wrap:anywhere}li[data-state=in_progress]{color:#e9e9e9}li[data-state=completed]{color:#828282}.step-icon{display:grid;place-items:center;flex:none;width:16px;height:19px}.step-icon i{width:6px;height:6px;border:1px solid #777;border-radius:50%}.empty-plan,.panel-empty{color:#888;line-height:1.8;margin:16px 0}.latest-progress,.task-tools{border-top:1px solid #ffffff10;margin-top:24px;padding-top:20px}h4{font-size:11px;color:#999;margin-bottom:12px;font-weight:400}.latest-progress p{font-size:12px;line-height:1.8;white-space:pre-wrap;overflow-wrap:anywhere}.latest-progress small{display:block;color:#888;font-size:10px;margin-top:12px}.task-tools details{border:1px solid #ffffff12;border-radius:8px;margin:7px 0;padding:9px 10px}.task-tools summary{display:flex;justify-content:space-between;gap:8px;cursor:pointer;font-size:11px;list-style:none}.task-tools small{color:#999;white-space:nowrap}.task-tools p,.task-tools pre{white-space:pre-wrap;overflow-wrap:anywhere;font-size:11px;line-height:1.6;margin-top:8px;color:#aaa}.failed{color:#db9b91}footer{display:flex;gap:10px;flex-wrap:wrap;border-top:1px solid #ffffff10;padding:12px 18px;font-size:10px;color:#888}.resize-edge{position:absolute;left:-4px;top:14px;bottom:14px;width:8px;cursor:ew-resize;touch-action:none;border-radius:6px}.resize-edge:hover{background:#ffffff15}:global(.chat-task-spinner){animation:chat-task-spin 1s linear infinite}@keyframes chat-task-spin{to{transform:rotate(360deg)}}@media(prefers-reduced-motion:reduce){:global(.chat-task-spinner){animation:none}}@media(max-width:560px){.chat-task-panel{top:64px;right:8px;bottom:8px;max-width:calc(100% - 16px)}}
.agent-tasks{border-top:1px solid #ffffff10;padding-top:18px;margin-top:18px}.agent-tasks details{border:1px solid #ffffff15;border-radius:9px;padding:10px;margin:10px 0}.agent-tasks summary{display:flex;justify-content:space-between;gap:10px;cursor:pointer}.agent-tasks small{font-size:10px;color:#999}.agent-tasks p{font-size:11px;white-space:pre-wrap;overflow-wrap:anywhere;margin:10px 0;color:#aaa}.agent-tasks ol{gap:7px}
</style>
