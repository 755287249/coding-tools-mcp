<script lang="ts">
  import { t } from '$lib/i18n';
  import { randomId } from '$lib/browser-tools';
  import type {ChatMessage, QuestionAnswer} from '$lib/api/chat';
  let {message, disabled=false, onAnswer}: {message:ChatMessage;disabled?:boolean;onAnswer:(id:string,answers:QuestionAnswer[])=>Promise<boolean>}=$props();
  let choices=$state<Record<string,string>>({}), custom=$state<Record<string,string>>({});
  let submitting=$state(false), submitted=$state(false), failed=$state(false);
  let retry:{id:string;answers:QuestionAnswer[]}|null=null;
  const inactive=$derived(!message.questions_active||!!message.question_answer||submitted);
  const ready=$derived((message.questions??[]).every(q=>choices['q:'+q.id]==='custom'?!!custom['q:'+q.id]?.trim()&&new TextEncoder().encode(custom['q:'+q.id].trim()).length<=4000:!!choices['q:'+q.id]));
  function answerFor(id:string){return message.question_answer?.answers.find(a=>a.question_id===id);}
  async function submit(){
    if(disabled||inactive||submitting||!ready)return;
    const answers=(message.questions??[]).map(q=>choices['q:'+q.id]==='custom'?{question_id:q.id,custom_text:custom['q:'+q.id].trim()}:{question_id:q.id,option_id:choices['q:'+q.id].slice(7)});
    if(!retry||JSON.stringify(retry.answers)!==JSON.stringify(answers))retry={id:randomId(),answers};
    submitting=true;failed=false;
    try{submitted=await onAnswer(retry.id,retry.answers);failed=!submitted;}catch{failed=true;}finally{submitting=false;}
  }
</script>
<form class="question-card" onsubmit={e=>{e.preventDefault();void submit();}}>
  {#each message.questions??[] as question (question.id)}
    {@const saved=answerFor(question.id)}
    <fieldset disabled={disabled||inactive||submitting}>
      <legend>{question.prompt}</legend>
      {#each question.options as option (option.id)}
        <label class="choice" class:selected={(saved?.option_id?'option:'+saved.option_id:choices['q:'+question.id])==='option:'+option.id}>
          <input type="radio" name={message.id+question.id} value={option.id} checked={(saved?.option_id?'option:'+saved.option_id:choices['q:'+question.id])==='option:'+option.id} onchange={()=>choices['q:'+question.id]='option:'+option.id}/>
          <span>{option.label}{#if option.description}<small>{option.description}</small>{/if}</span>
        </label>
      {/each}
      <label class="choice" class:selected={!!saved?.custom_text||choices['q:'+question.id]==='custom'}>
        <input type="radio" name={message.id+question.id} checked={!!saved?.custom_text||choices['q:'+question.id]==='custom'} onchange={()=>choices['q:'+question.id]='custom'}/><span>{$t('chat.questionCustom')}</span>
      </label>
      {#if choices['q:'+question.id]==='custom'||saved?.custom_text}
        <textarea aria-label={question.prompt+' · '+$t('chat.questionCustom')} rows="3" maxlength="4000" value={saved?.custom_text??custom['q:'+question.id]??''} oninput={e=>custom['q:'+question.id]=e.currentTarget.value}></textarea>
      {/if}
    </fieldset>
  {/each}
  {#if message.question_answer||submitted}<p role="status">{$t('chat.questionAnswered')}</p>
  {:else if inactive}<p>{$t('chat.questionExpired')}</p>
  {:else}<button type="submit" disabled={disabled||submitting||!ready}>{$t(submitting?'chat.questionSending':failed?'chat.questionRetry':'chat.questionSubmit')}</button>{/if}
  {#if failed}<p class="question-error" role="alert">{$t('chat.questionFailed')}</p>{/if}
</form>
<style>
.question-card{margin-top:16px;padding:16px;border:1px solid var(--color-border);border-radius:14px;background:var(--surface-2);color:var(--color-text);display:grid;gap:16px;min-width:0}.question-card fieldset{padding:0;border:0;display:grid;gap:8px;min-width:0}.question-card legend{font-weight:600;padding:0;margin-bottom:10px;white-space:pre-wrap;overflow-wrap:anywhere}.choice{display:flex;gap:10px;align-items:flex-start;padding:10px 12px;border:1px solid var(--color-border);border-radius:9px;cursor:pointer;overflow-wrap:anywhere}.choice input{flex:none;margin-top:4px;accent-color:var(--primary)}.choice.selected{border-color:var(--primary);background:var(--chat-user-bg);color:var(--chat-user-text)}.choice span{min-width:0}.choice small{display:block;font-size:12px;line-height:1.5;opacity:.8;margin-top:3px}.question-card textarea{box-sizing:border-box;width:100%;resize:vertical;min-height:70px;padding:10px;border:1px solid var(--color-border);border-radius:9px;background:var(--chat-bg);color:var(--color-text)}.question-card button{justify-self:start;padding:9px 16px;border-radius:9px;background:var(--primary);color:var(--on-primary,#fff)}.question-card button:disabled{opacity:.55}.question-card p{margin:0;font-size:12px}.question-error{color:var(--danger)}.question-card fieldset:disabled .choice{cursor:default}
</style>
