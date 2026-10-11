import type {ChatMessage} from '../api/chat';
export interface ReplyGroup {kind:'reply';id:string;messages:ChatMessage[];request?:ChatMessage;name:string}
export function groupReplyTurns(messages:ChatMessage[],fallbackName='AI') {
  const feed:({kind:'message';id:string;message:ChatMessage}|ReplyGroup)[]=[];
  const groups=new Map<string,ReplyGroup>();
  const byId=new Map(messages.map(message=>[message.id,message]));
  const names=new Map<string|undefined,Set<string>>();
  for(const message of messages){if(message.role==='assistant'&&!message.agent_id&&message.agent_name){const group=names.get(message.reply_to)??new Set<string>();group.add(message.agent_name);names.set(message.reply_to,group)}}
  for(const message of messages){
    if(message.role!=='assistant'){feed.push({kind:'message',id:'message:'+message.id,message});continue;}
    const peerNames=names.get(message.reply_to);
    const name=message.agent_name??(!message.agent_id&&peerNames?.size===1?[...peerNames][0]:fallbackName);
    const key=JSON.stringify([message.reply_to??message.id,message.agent_id??name]);
    let group=groups.get(key);
    if(!group){group={kind:'reply',id:'reply:'+message.id,messages:[],name,request:message.reply_to?byId.get(message.reply_to):undefined};groups.set(key,group);feed.push(group)}
    group.messages.push(message);
  }
  return feed;
}
export function combinedReply(group:ReplyGroup,labels={tool:'Tool',input:'Input',output:'Output',truncated:'Output truncated'}):ChatMessage {
  const last=group.messages.at(-1)!;
  const files=new Map(group.messages.flatMap(m=>(m.attachments??[]).map(f=>[f.id,f] as const)));
  const text=group.messages.map(m=>m.tool_event?[
    `### ${labels.tool}: ${m.tool_event.name} (${m.tool_event.status})`,
    m.text,
    m.tool_event.input?labels.input+':\n'+m.tool_event.input:'',
    m.tool_event.output?labels.output+':\n'+m.tool_event.output:'',
    m.tool_event.output_truncated?labels.truncated:''
  ].filter(Boolean).join('\n\n'):[m.text,...(m.questions??[]).map(q=>`**${q.prompt}**\n${q.options.map(o=>`- ${o.label}${o.description?' — '+o.description:''}`).join('\n')}`)].filter(Boolean).join('\n\n')).join('\n\n');
  return {id:group.id,role:'assistant',text,created_at:group.messages[0].created_at,agent_name:group.name,reply_to:last.reply_to,final:last.final,attachments:[...files.values()]};
}
