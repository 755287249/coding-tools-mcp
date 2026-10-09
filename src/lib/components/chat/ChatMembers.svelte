<script lang="ts">
 import type {ChatMember,ChatAction} from '$lib/api/chat';
 import {t} from '$lib/i18n';
 let {members=[],busy=false,onMention,onConnect,onChange}:{members?:ChatMember[];busy?:boolean;onMention:(name:string)=>void;onConnect:()=>void;onChange:(args:ChatAction)=>void}=$props();
 let editing=$state('');let name=$state('');
</script>
<div class="members">
 <div class="chips" aria-label={$t('chat.groupMembers')}>
 {#each members.filter(m=>m.status!=='offline') as member(member.id)}<button type="button" disabled={busy} onclick={()=>onMention(member.name)} title={member.role==='coordinator'?$t('chat.coordinator'):$t('chat.collaborator')}><i class:waiting={member.status==='waiting'}></i>{member.name}{#if member.role==='coordinator'}<small>{$t('chat.coordinator')}</small>{/if}</button>{/each}
 {#if !members.some(m=>m.status!=='offline')}<span>{$t('chat.noMembers')}</span>{/if}
 <button type="button" disabled={busy} onclick={onConnect}>＋ {$t('chat.connect')}</button>
 </div>
 {#if members.length}<details class="manage"><summary>{$t('chat.manageMembers')}</summary><div class="member-menu">
 {#each members as m(m.id)}<div class="member-row"><span><b>{m.name}</b><small>{m.role==='coordinator'?$t('chat.coordinator'):$t('chat.collaborator')} · {m.status==='offline'?$t('chat.memberOffline'):$t('Connected')}</small></span>
 <button type="button" disabled={busy} onclick={()=>{editing=m.id;name=m.name}}>{$t('chat.renameMember')}</button>
 <button type="button" disabled={busy} onclick={()=>onChange({action:m.paused?'resume_member':'detach_member',member_id:m.id})}>{$t(m.paused?'shell.resume':'shell.pause')}</button>
 {#if m.role!=='coordinator'}<button type="button" disabled={busy} onclick={()=>onChange({action:'set_coordinator',member_id:m.id})}>{$t('chat.makeCoordinator')}</button>{/if}
 </div>{#if editing===m.id}<form onsubmit={e=>{e.preventDefault();onChange({action:'rename_member',member_id:m.id,name});editing='';}}><input aria-label={$t('chat.agentRemark')} bind:value={name} maxlength="80"/><button disabled={busy||!name.trim()}>{$t('Save')}</button><button type="button" onclick={()=>editing=''}>{$t('Cancel')}</button></form>{/if}{/each}
 </div></details>{/if}
</div>
<style>
.members{max-width:760px;margin:16px auto 0;font-size:12px;position:relative;color:#aaa}.chips{display:flex;flex-wrap:wrap;align-items:center;justify-content:center;gap:8px}.chips button{display:flex;align-items:center;gap:7px;padding:7px 11px;border:1px solid #ffffff20;border-radius:20px;color:#d5d5d5}.chips i{width:5px;height:5px;border-radius:50%;background:#829887}.chips i.waiting{background:#6fbe98}.chips small{font-size:9px;color:#999}.chips span{font-size:11px}.manage{margin-top:10px;text-align:center}.manage summary{cursor:pointer;font-size:10px;color:#888}.member-menu{position:absolute;z-index:35;bottom:28px;left:0;right:0;padding:12px;border-radius:12px;background:#252525;border:1px solid #ffffff25;box-shadow:0 12px 28px #0007;text-align:left;max-height:300px;overflow:auto}.member-row{display:flex;align-items:center;gap:10px;padding:8px 0}.member-row>span{flex:1;min-width:0;overflow-wrap:anywhere}.member-row b{font-weight:500;color:#ddd}.member-row small{display:block;margin-top:4px;font-size:10px}.member-row button,form button{font-size:10px;padding:5px;border-radius:5px}button{cursor:pointer}button:hover{background:#ffffff0a}button:disabled{opacity:.4;cursor:default}button:focus-visible,input:focus-visible,summary:focus-visible{outline:2px solid #79b5ff}form{display:flex;gap:8px;padding:8px 0}input{flex:1;min-width:0;background:#181818;padding:6px;border:1px solid #ffffff20;border-radius:5px}
</style>
