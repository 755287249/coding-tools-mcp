<script lang="ts">
 import {t} from '$lib/i18n';
 import type {ChatSession} from '$lib/api/chat';
 import {chatOperations,operationSummary} from '$lib/chat/operations';
 import {parseUnifiedDiff,formatDuration} from '$lib/activity/diff';
 import DiffView from '$lib/components/activity/DiffView.svelte';
 let {session,request}:{session:ChatSession|null;request?:string}=$props();
 let scope=$state('all');let filter=$state('all');let limit=$state(40);
 const rows=$derived(chatOperations(session,scope==='current'?request:undefined));
 const stats=$derived(operationSummary(rows));
 let diffMode=$state<'unified'|'split'>('unified');
 const visible=$derived(rows.filter(e=>filter==='all'||(filter==='failed'?(e.status==='failed'||e.status==='interrupted'):e.kind===filter)));
 const labels={read:'chat.opRead',search:'Search',edit:'chat.opEdit',exec:'chat.opExec',other:'chat.opOther'} as const;
 const status={running:'Running',completed:'Completed',failed:'Failed',interrupted:'chat.opInterrupted'} as const;
 const filterScope=$derived(JSON.stringify([session?.id,scope,filter,scope==='current'?request:'']));
 $effect(()=>{filterScope;limit=40;});
</script>
<section class="operations">
 <h4>{$t('chat.operationDetails')} <span>{rows.length}</span></h4>
 <div class="filters">
  <select aria-label={$t('chat.operationScope')} bind:value={scope}><option value="all">{$t('chat.thisConversation')}</option><option value="current" disabled={!request}>{$t('chat.selectedTask')}</option></select>
  <select aria-label={$t('chat.operationFilter')} bind:value={filter}><option value="all">{$t('All')}</option><option value="read">{$t('chat.opRead')}</option><option value="edit">{$t('chat.opEdit')}</option><option value="exec">{$t('chat.opExec')}</option><option value="search">{$t('Search')}</option><option value="failed">{$t('Failed')}</option></select>
 </div>
 <div class="operation-stats" aria-label={$t('chat.operationStats')}>
  <div><strong>{stats.successRate===null?'–':stats.successRate+'%'}</strong><span>{$t('Success rate')}</span></div><div><strong>{stats.total}</strong><span>{$t('Calls')}</span></div>
  <div><strong>{stats.average===null?'–':formatDuration(stats.average)}</strong><span>{$t('Avg response')}</span></div><div><strong>{stats.p95===null?'–':formatDuration(stats.p95)}</strong><span>{$t('chat.p95')}</span></div>
 </div>
 <div class="counters"><span>{$t('Reads')} {stats.reads}</span><span>{$t('Search')} {stats.searches}</span><span>{$t('Edits')} {stats.edits}</span><span>{$t('Commands')} {stats.execs}</span><span>{$t('chat.editedPaths')} {stats.files}</span><span>{$t('Completed')} {stats.completed}</span><span>{$t('Failed')} {stats.failed}</span><span>{$t('Running')} {stats.running}</span>{#if stats.unknown}<span>{$t('chat.opInterrupted')} {stats.unknown}</span>{/if}</div>
 <p class="hint">{$t('chat.operationStatsHint')} {stats.reported ? `${$t('chat.opReported')}: ${stats.reported}` : ''}</p>
 {#if session?.operations_error}<p class="failed">{$t('chat.opUnavailable')}</p>{/if}
 {#if !visible.length}<p class="empty">{$t('chat.opEmpty')}</p>{/if}
 {#each visible.slice(0,limit) as event(event.id)}
  <details class="operation" data-status={event.status}>
   <summary><div class="actor"><strong>{event.agent_name}</strong><time>{new Date(event.started_at).toLocaleTimeString()}</time></div><div class="action"><span>{$t(labels[event.kind])} · {event.tool}</span><small>{$t(status[event.status])}</small></div>{#if event.paths.length}<div class="paths" title={event.paths.join('\n')}>{event.paths.join(' · ')}</div>{/if}</summary>
   <div class="evidence"><span>{$t(event.source==='mcp'?'chat.opObserved':'chat.opReported')}</span>{#if event.duration_ms!==undefined}<span>{event.duration_ms} {$t('chat.opMilliseconds')}</span>{/if}{#if event.dry_run}<span>{$t('chat.opDryRun')}</span>{/if}</div>
   <p class="request" title={event.task}>{event.task}</p>
   {#each event.paths as path(path)}<code class="path">{path}</code>{/each}
   {#if event.input}<pre>{event.input}</pre>{/if}
   {#if event.output}<pre>{event.output}</pre>{/if}
   {#if event.diff}<div class="diff-controls" role="group" aria-label={$t('Code diff')}><button type="button" aria-pressed={diffMode==='unified'} onclick={()=>diffMode='unified'}>{$t('Unified')}</button><button type="button" aria-pressed={diffMode==='split'} onclick={()=>diffMode='split'}>{$t('Side by side')}</button></div>{#each parseUnifiedDiff(event.diff) as file,index(index)}<div class="diff"><DiffView {file} mode={diffMode}/></div>{/each}{/if}
   {#if event.truncated}<small class="notice">{$t('chat.opTruncated')}</small>{/if}
   {#if event.status==='running'}<small class="notice">{$t('chat.opRunning')}</small>{/if}
  </details>
 {/each}
 {#if visible.length>limit}<button type="button" onclick={()=>limit+=40}>{$t('chat.opMore')} ({visible.length-limit})</button>{/if}
 <p class="hint">{$t('chat.opHint')}</p>
</section>
<style>
.operation-stats{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px;margin:16px 0}.operation-stats>div{padding:12px;background:var(--color-border);border:1px solid var(--color-border);border-radius:8px;display:flex;flex-direction:column;gap:5px}.operation-stats strong{font-size:18px;font-weight:500;color:var(--color-text)}.operation-stats span{font-size:10px;color:var(--color-text-muted)}.counters{display:flex;flex-wrap:wrap;gap:8px 12px;color:var(--color-text-secondary);font-size:11px}.diff-controls{display:flex;gap:6px;margin-top:10px}.diff-controls [aria-pressed=true]{background:var(--color-border);color:var(--color-text)}

.operations{border-top:1px solid var(--color-border);padding-top:18px;margin-top:20px}h4{font-size:12px;font-weight:500;color:var(--color-text-secondary);display:flex;justify-content:space-between;margin-bottom:12px}h4 span{color:var(--color-text-muted);font-size:11px}.filters{display:flex;gap:8px;margin-bottom:12px}select{min-width:0;flex:1;border:1px solid var(--color-border);border-radius:6px;background:var(--surface-2);padding:6px;color:var(--color-text-secondary);font-size:11px}.operation{border:1px solid var(--color-border);border-radius:8px;margin:8px 0;padding:10px}.operation[data-status=failed]{border-color:#b367564f}.operation[data-status=interrupted]{border-color:#a190674f}summary{cursor:pointer;list-style:none}.actor,.action,.evidence{display:flex;align-items:center;justify-content:space-between;gap:8px}.actor strong{font-weight:500;color:var(--color-text);font-size:11px}.actor time{color:var(--color-text-muted);font-size:10px}.action{margin:6px 0;color:var(--color-text-secondary);font-size:11px}.action small{font-size:10px;color:var(--color-text-muted);white-space:nowrap}.paths{color:var(--chat-link);font-size:10px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.path{display:block;white-space:pre-wrap;overflow-wrap:anywhere;color:var(--chat-link);font-size:10px;margin:6px 0}.evidence{justify-content:flex-start;flex-wrap:wrap;color:var(--color-text-secondary);font-size:9px;border-top:1px solid var(--color-border);margin-top:10px;padding-top:8px}.request{max-height:65px;overflow:auto;font-size:10px;color:var(--color-text-muted);line-height:1.6;margin-top:8px;white-space:pre-wrap;overflow-wrap:anywhere}pre{font-size:10px;line-height:1.65;color:var(--color-text-secondary);background:var(--input-bg);padding:8px;border-radius:5px;white-space:pre-wrap;overflow-wrap:anywhere;max-height:240px;overflow:auto;margin-top:8px}.diff{max-height:300px;overflow:auto;margin-top:10px}.hint,.empty{font-size:10px;line-height:1.7;color:var(--color-text-muted);margin:12px 0}.notice{display:block;font-size:10px;color:var(--warning);margin-top:8px}.failed{color:var(--danger)}button{width:100%;padding:7px;border-radius:5px;background:var(--color-border);color:var(--color-text-secondary);font-size:11px}summary:focus-visible,button:focus-visible,select:focus-visible{outline:2px solid var(--chat-link);outline-offset:2px}
</style>
