import type {ChatSession, ChatMessage} from './store.js';
import {redactSensitiveText} from '../redaction.js';
import {visibleSession} from './collaboration.js';

export interface ChatQuestion {id:string;prompt:string;options:{id:string;label:string;description?:string}[]}
export interface QuestionAnswer {question_id:string;option_id?:string;custom_text?:string}
export interface QuestionResponse {message_id:string;answers:QuestionAnswer[]}
const object=(v:unknown):Record<string,unknown>=>{
  if(!v||typeof v!=='object'||Array.isArray(v))throw Error('Invalid question object');
  return v as Record<string,unknown>;
};
const fields=(v:Record<string,unknown>,keys:string[])=>{if(Object.keys(v).some(k=>!keys.includes(k)))throw Error('Unknown question field');};
const id=(v:unknown)=>{if(typeof v!=='string'||! /^[a-zA-Z0-9_-]{1,80}$/.test(v))throw Error('Invalid question ID');return v;};
const text=(v:unknown,max:number)=>{if(typeof v!=='string'||!v.trim()||Buffer.byteLength(v)>max)throw Error('Invalid question text');return redactSensitiveText(v.trim()).value;};
export function parseQuestions(value:unknown):ChatQuestion[]|undefined {
  if(value===undefined)return;
  if(!Array.isArray(value)||!value.length||value.length>3)throw Error('questions must contain 1–3 questions');
  const seen=new Set<string>();
  return value.map(raw=>{
    const q=object(raw);fields(q,['id','prompt','options']);const key=id(q.id);
    if(seen.has(key))throw Error('Duplicate question ID');seen.add(key);
    if(!Array.isArray(q.options)||q.options.length>6)throw Error('Question options must contain 0–6 choices');
    const choices=new Set<string>();
    return {id:key,prompt:text(q.prompt,1000),options:q.options.map(raw=>{
      const o=object(raw);fields(o,['id','label','description']);const key=id(o.id);
      if(choices.has(key))throw Error('Duplicate option ID');choices.add(key);
      return {id:key,label:text(o.label,200),...(o.description===undefined?{}:{description:text(o.description,500)})};
    })};
  });
}
export function activeQuestionId(session:ChatSession):string|undefined {
  if(session.closed)return;
  const messages=visibleSession(session).messages;
  for(let i=messages.length-1;i>=0;i--){
    const m=messages[i];
    if(m.role==='user'||(m.role==='assistant'&&m.final===true))return m.questions?.length&&!m.question_answer?m.id:undefined;
  }
}
export function validateQuestionContext(s:ChatSession,replyTo:string,final:boolean,awaiting:boolean):void {
  if(!final||!awaiting)throw Error('questions requires final=true and awaiting_user=true');
  if(s.messages.find(m=>m.id===replyTo)?.discussion)throw Error('For discussion tasks, ask in discussion text; interactive questions require a direct conversation');
}
function parseAnswers(value:unknown,questions:ChatQuestion[]):QuestionAnswer[] {
  if(!Array.isArray(value)||value.length!==questions.length)throw Error('Answer every question exactly once');
  const result=value.map(raw=>{
    const a=object(raw);fields(a,['question_id','option_id','custom_text']);const key=id(a.question_id);
    const q=questions.find(q=>q.id===key);if(!q)throw Error('Unknown question ID');
    if((a.option_id!==undefined)===(a.custom_text!==undefined))throw Error('Choose one option or provide a custom answer');
    if(a.option_id!==undefined){const option=id(a.option_id);if(!q.options.some(o=>o.id===option))throw Error('Unknown option ID');return {question_id:key,option_id:option};}
    return {question_id:key,custom_text:text(a.custom_text,4000)};
  });
  if(new Set(result.map(a=>a.question_id)).size!==questions.length)throw Error('Duplicate question answer');
  return questions.map(q=>result.find(a=>a.question_id===q.id)!);
}
/** Runs within the chat store lock; the answer and receipt commit together. */
export function answerQuestions(s:ChatSession,args:Record<string,unknown>):void {
  const cardId=id(args.question_message_id),messageId=id(args.message_id);
  const card=s.messages.find(m=>m.id===cardId&&m.role==='assistant'&&m.questions?.length);
  if(!card?.questions)throw Error('Question card not found');
  const answers=parseAnswers(args.answers,card.questions);
  if(card.question_answer){
    if(JSON.stringify(parseAnswers(card.question_answer.answers,card.questions))!==JSON.stringify(answers))throw Error('Question already answered differently');
    return; // Reloads and lost responses may retry with a new request ID.
  }
  if(activeQuestionId(s)!==card.id)throw Error('Question is no longer active; reply in the composer');
  if([...s.messages,...s.queue??[]].some(m=>m.id===messageId)||Object.hasOwn(s.queue_receipts??{},messageId))throw Error('Message ID conflicts with an existing message');
  if(s.mode==='group'&&!s.members?.some(m=>m.id===card.agent_id&&!m.paused))throw Error('Question owner is unavailable');
  const content=card.questions.map((q,i)=>`${q.prompt}\n${answers[i].custom_text??q.options.find(o=>o.id===answers[i].option_id)!.label}`).join('\n\n');
  const response={message_id:cardId,answers};
  s.messages.push({id:messageId,role:'user',text:content,attachments:[],created_at:Date.now(),question_response:response,...(s.mode==='group'?{recipient_ids:[card.agent_id!]}:{})});
  card.question_answer={message_id:messageId,answers};s.updated_at=Date.now();
}
export function questionsMarkdown(message:ChatMessage):string {
  return (message.questions??[]).map(q=>`\n**${q.prompt}**\n${q.options.map(o=>`- ${o.label}${o.description?' — '+o.description:''}\n`).join('')}- 自定义回答\n`).join('');
}
