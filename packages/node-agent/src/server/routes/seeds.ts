import {callTool} from '../../tools.js';
import {McpToolCallLifecycle} from '../mcp/lifecycle.js';
import type {IncomingMessage,ServerResponse} from 'node:http';
import type {JsonObject,ToolContext,ToolDefinition} from '../../types.js';
import {sendJson} from '../../oauth.js';
import {readRequestBody} from '../http.js';
import {currentToolCatalog} from '../catalog.js';
import {dispatchMcpMethod} from '../mcp/dispatcher.js';
import {seedStore,standbyHandoff} from '../../chat/store.js';
import {seedUrl,seedProvision,seedInitialize,seedAuthenticate,seedCreated,seedPoll,seedBegin,seedFinish,seedPendingProcesses,seedProcessSettled} from '../../chat/seeds.js';
import {seedConnectSkill,localChatSkill} from '../../rustCatalog.generated.js';
import {setTimeout as sleep} from 'node:timers/promises';
import {Readable} from 'node:stream';
import {EventEmitter} from 'node:events';
import {createHash, randomUUID} from 'node:crypto';
import {projectScopedToolNames} from '../../rustCatalog.generated.js';

// Exclude external tools, workspace switching, permissions replay and other chats.
export const SEED_TOOLS=new Set(['chat_open','chat_wait','chat_reply','chat_upload','chat_close','set_todos','update_plan','report_progress','read_file','list_files','list_directory','search_files','search_text','search_code','grep','glob','view_image','apply_patch','edit','read_many','project_map','patch_check','format_files','edit_file','write_file','file_ops','exec_command','wait_command','send_input','kill_session','read_output','git_status','git_diff','git_log']);
const waitTool={name:'seed_wait',description:'Wait for a conversation assignment. One independent wait at a time; idle is normal. Then chat_open with returned chat_id/attachment_id.',inputSchema:{type:'object',properties:{timeout_ms:{type:'integer',minimum:0,maximum:25000}},required:['timeout_ms'],additionalProperties:false}};
const foldersTool={name:'list_workspace_folders',description:'Show the single project authorized for this seed.',inputSchema:{type:'object',properties:{},additionalProperties:false}};
const connectTool={name:'seed_connect',description:'Register this seed as ready; return its only workspace, full connection Skill and assignment state. Then chat_wait(timeout_ms:25000).',inputSchema:{type:'object',properties:{},additionalProperties:false}};
function scopedTool(tool: ToolDefinition): ToolDefinition {
  const copy=JSON.parse(JSON.stringify(tool));
  if(Array.isArray(copy.inputSchema.required))copy.inputSchema.required=copy.inputSchema.required.filter((key:string)=>!['chat_id','attachment_id','workspace_folder_id'].includes(key));
  if(copy.name==='chat_wait')copy.inputSchema.properties.timeout_ms.maximum=25000;
  if(copy.name==='chat_wait')copy.description='Wait for seed assignment or a chat message. First assignment returns status=assigned with full Skill/session; read it then wait again. Identity is bound by this endpoint.';
  return copy;
}
const waiting=new Set<string>();
const probeTimes=new Map<string,number>();
/** Same tool set as an OAuth project connection: both channels are equal. */
const PROJECT_TOOLS=new Set<string>(projectScopedToolNames);
function seedToolAllowed(name:string):boolean {return name!=='switch_workspace_folder'&&(SEED_TOOLS.has(name)||PROJECT_TOOLS.has(name));}
const SUPPORTED_PROTOCOLS=['2024-11-05','2025-03-26','2025-06-18'];
function wrapped(result:unknown){return {content:[{type:'text',text:JSON.stringify(result)}],structuredContent:result,isError:false};}
export async function handleSeedRoute(req:IncomingMessage,res:ServerResponse,pathname:string,context:ToolContext):Promise<boolean>{
  const pairing=pathname.match(/^\/mcp\/seed-library\/([A-Za-z0-9_-]{1,80})$/);
  if(pairing){
    res.setHeader('cache-control','no-store');
    if(req.method!=='POST'){sendJson(res,405,{error:'POST required'});return true;}
    const folder=context.config.folders.find(f=>f.id===pairing[1]);
    if(!folder||!currentToolCatalog(context).names.includes('chat_open')){sendJson(res,404,{error:'Seed library unavailable'});return true;}
    try{
      const body=await readRequestBody(req,16384);
      const args=JSON.parse(body.toString());if(!args||Array.isArray(args)||typeof args!=='object')throw Error('Invalid pairing request');
      const result=seedProvision(seedStore(folder.path),String(req.headers.authorization??'').replace(/^Bearer /,''),args);
      sendJson(res,200,{...result,workspace_folder_id:folder.id});
    }catch(error){sendJson(res,400,{error:error instanceof Error?error.message:'Pairing request failed'});}
    return true;
  }
  const action=pathname.match(/^\/mcp\/seeds\/([A-Za-z0-9_-]{1,80})\/([a-f0-9]{64})\/([a-z]{1,16})$/);
  if(action&&req.method==='GET'){await handleSeedGet(req,res,pathname,action[3],context);return true;}
  const match=pathname.match(/^\/mcp\/seeds\/([A-Za-z0-9_-]{1,80})\/([A-Za-z0-9_-]{1,80})$/);if(!match)return false;
  res.setHeader('cache-control','no-store');
  const folder=context.config.folders.find(f=>f.id===match[1]);if(!folder){sendJson(res,404,{error:'Seed unavailable'});return true;}
  const store=seedStore(folder.path),urlMode=/^[a-f0-9]{64}$/.test(match[2]);
  let seedId=match[2],token=String(req.headers.authorization??'').replace(/^Bearer /,'');
  if(req.method==='GET'){
    if(req.headers.accept==='text/event-stream'){res.setHeader('allow','POST');sendJson(res,405,{error:'SSE stream not provided; use POST'});return true;}
    try{seedUrl(store,match[2]);if(!currentToolCatalog(context).names.includes('chat_open'))throw Error();res.setHeader('referrer-policy','no-referrer');if(String(req.headers.accept??'').includes('application/json'))sendJson(res,200,{name:'coding-tools-seed',transport:'streamable-http',instructions:seedConnectSkill.text});else sendPage(req,res,guideText(baseUrl(req,pathname)),baseUrl(req,pathname));}
    catch{sendJson(res,404,{error:'Seed unavailable'});}return true;
  }
  if(req.method!=='POST'){sendJson(res,405,{error:'POST required'});return true;}
  let id:unknown=null,waitKey:string|undefined;
  try{
    const body=JSON.parse((await readRequestBody(req)).toString()) as JsonObject;id=body.id??null;
    if(body.jsonrpc!=='2.0'||typeof body.method!=='string')throw Error('Invalid JSON-RPC request');
    const method=body.method,params=(body.params??{}) as JsonObject;
    const protocol=req.headers['mcp-protocol-version'];
    // Accept a missing header (curl users, older clients) and every published revision.
    if(method!=='initialize'&&protocol!==undefined&&!SUPPORTED_PROTOCOLS.includes(String(protocol)))throw Error('Unsupported MCP-Protocol-Version');
    const negotiated=SUPPORTED_PROTOCOLS.includes(String((body.params as JsonObject|undefined)?.protocolVersion))?String((body.params as JsonObject).protocolVersion):'2025-03-26';
    if(!currentToolCatalog(context).names.includes('chat_open'))throw Error('Seed access requires a tool profile with local chat enabled');
    if(urlMode){seedId=seedUrl(store,match[2],method==='initialize');token=match[2];}
    if(method==='initialize'){
      const access=urlMode?'':seedInitialize(store,seedId,token);
      sendJson(res,200,{jsonrpc:'2.0',id,result:{protocolVersion:negotiated,capabilities:{tools:{},resources:{}},serverInfo:{name:'coding-tools-seed',version:'1'},instructions:seedConnectSkill.text,...(urlMode?{}:{_meta:{seed_access_token:access}})}});return true;
    }
    if(method==='seed/created'){seedCreated(store,seedId,token,String(params.task_id??''));sendJson(res,200,{jsonrpc:'2.0',id,result:{ok:true}});return true;}
    seedAuthenticate(store,seedId,token);
    if(method==='notifications/initialized'&&body.id===undefined){res.writeHead(202).end();return true;}
    if(body.id===undefined)throw Error('Request ID required');
    let result:unknown;
    const catalog=currentToolCatalog(context);
    if(method==='tools/list')result={tools:[connectTool,foldersTool,waitTool,...catalog.tools.filter(t=>seedToolAllowed(t.name)).map(scopedTool)]};
    else if(method==='resources/read'&&params.uri===seedConnectSkill.uri)result={contents:[seedConnectSkill]};
    else if(method==='resources/read'&&params.uri===localChatSkill.uri)result={contents:[localChatSkill]};
    else if(method==='resources/list')result={resources:[seedConnectSkill,localChatSkill].map(({uri,name,mimeType})=>({uri,name,mimeType}))};
    else if(method==='ping')result={};
    else if(method==='tools/call'){
      let name=String(params.name??'');
      const rawArgs=params.arguments??{};if(!rawArgs||typeof rawArgs!=='object'||Array.isArray(rawArgs))throw Error('Arguments must be an object');
      const args={...(rawArgs as JsonObject)};
      if(args.workspace_folder_id!==undefined&&args.workspace_folder_id!==folder.id)throw Error('Seed workspace mismatch');
      if(name==='seed_connect'){
        const status=seedPoll(store,seedId,token);
        sendJson(res,200,{jsonrpc:'2.0',id,result:wrapped({...status,registered:status.status!=='retired',seed_id:seedId,workspace_folder_id:folder.id,workspace:folder,skill:seedConnectSkill,instruction:'Read skill.text, then immediately chat_wait(timeout_ms:25000). Keep calling chat_wait one at a time until assigned. The user writes in the app at any time; idle only means no new message yet, so just wait again.'})});return true;
      }
      let autoOpen=false;
      const chatWait=name==='chat_wait';
      if(chatWait){waitKey=folder.path+':'+seedId;if(waiting.has(waitKey)){waitKey=undefined;throw Error('A seed_wait is already active');}waiting.add(waitKey);const timeout=Number(args.timeout_ms);if(!Number.isInteger(timeout)||timeout<0||timeout>25000)throw Error('timeout_ms must be 0–25000');const status=seedPoll(store,seedId,token);if(status.status==='retired'){sendJson(res,200,{jsonrpc:'2.0',id,result:wrapped({...status,registered:false,instruction:'Seed retired. Stop waiting and do not switch identities.'})});return true;}if(standbyHandoff(args)){delete args.chat_id;delete args.attachment_id;}if(args.chat_id!==undefined&&args.chat_id!==status.chat_id)throw Error('Seed may only access its assigned chat');if(args.attachment_id!==undefined&&args.attachment_id!==status.attachment_id)throw Error('Seed attachment mismatch');if(status.status!=='assigned')name='seed_wait';else if(status.needs_open){name='chat_open';autoOpen=true;}}

      if(name==='list_workspace_folders')result=wrapped({ok:true,folders:[{id:folder.id,name:folder.name,path:folder.path,selected:true}],selected_folder_id:folder.id});
      else if(name==='seed_wait'){
        const timeout=Number(args.timeout_ms);if(!Number.isInteger(timeout)||timeout<0||timeout>25000)throw Error('timeout_ms must be 0–25000');
        const probeAt=Date.now(),probeDue=probeAt-(probeTimes.get(folder.path)??0)>=5000;
        if(probeDue)probeTimes.set(folder.path,probeAt);
        for(const pending of probeDue?seedPendingProcesses(store):[]){
          const check=await callTool(context,'wait_command',{workspace_folder_id:folder.id,session_id:pending.session_id,timeout_ms:0},{'openai/session':`seed:${pending.seed_id}`});
          if(check.ok!==false&&check.process_still_running===false)seedProcessSettled(store,pending.seed_id,pending.session_id);
        }
        const key=folder.path+':'+seedId;if(waitKey!==key){if(waiting.has(key))throw Error('A seed_wait is already active');waiting.add(key);}
        try{const deadline=Date.now()+timeout;let status;do{status=seedPoll(store,seedId,token);if(status.status!=='idle'||Date.now()>=deadline)break;await sleep(Math.min(5000,Math.max(0,deadline-Date.now())));}while(true);if(chatWait&&status.status==='assigned'){
          const binding=seedBegin(store,seedId,token,'chat_open',args,String(id));
          const openArgs={chat_id:binding.chat_id,attachment_id:binding.attachment_id,workspace_folder_id:folder.id};
          const opened=await callTool(context,'chat_open',openArgs,{'openai/session':`seed:${seedId}`});
          seedFinish(store,seedId,binding.call_id,'chat_open',openArgs,opened);
          if(opened.ok!==true)throw Error('Could not open assigned chat; retry chat_wait');
          result=wrapped({...opened,status:'assigned',chat_id:binding.chat_id,workspace_folder_id:folder.id});
        }else result=wrapped(chatWait?{...status,registered:status.status!=='retired',instruction:status.status==='retired'?'Seed retired. Stop waiting and do not switch identities.':'Immediately call chat_wait(timeout_ms:25000) again. Idle is normal. The user writes in the app at any time; idle only means no new message yet, so just wait again.'}:status);}finally{waiting.delete(key);}
      }else{
        if(!seedToolAllowed(name)||!catalog.names.includes(name))throw Error('Tool not available to seeds');
        if(args.workspace_folder_id!==undefined&&args.workspace_folder_id!==folder.id)throw Error('Seed workspace mismatch');
        if(name==='read_output'&&!args.session_id)throw Error('Session ID required');
        const binding=seedBegin(store,seedId,token,name,args,String(id));
        if(autoOpen)delete args.timeout_ms;
        args.workspace_folder_id=folder.id;
        if(name.startsWith('chat_')||['set_todos','update_plan','report_progress'].includes(name)){args.chat_id=binding.chat_id;args.attachment_id=binding.attachment_id;}
        if(name==='exec_command'&&!args.operation_id)args.operation_id=`seed-${seedId}-${binding.call_id.slice(0,20)}`;
        let actual:JsonObject={__unknown:true};const lifecycle=new McpToolCallLifecycle(context,req,res);
        try{
          const request={jsonrpc:'2.0',id,method,params:{name,arguments:args,_meta:{'openai/session':`seed:${seedId}`}}};
          result=await dispatchMcpMethod({catalog,context,method,req,request,protocolVersion:'2025-03-26',startedAt:Date.now(),processLifecycle:lifecycle.process});
          actual=((result as JsonObject).structuredContent??{}) as JsonObject;if(autoOpen&&actual.ok===true)result=wrapped({...actual,status:'assigned',chat_id:binding.chat_id,workspace_folder_id:folder.id});lifecycle.complete();
        }catch(e){lifecycle.abort();throw e;}finally{lifecycle.dispose();seedFinish(store,seedId,binding.call_id,name,args,actual);}
      }
    }else throw Error('Method not available to seeds');
    sendJson(res,200,{jsonrpc:'2.0',id,result});
  }catch(error){sendJson(res,200,{jsonrpc:'2.0',id,error:{code:-32000,message:error instanceof Error?error.message:'Seed request failed'}});}finally{if(waitKey)waiting.delete(waitKey);}
  return true;
}

// GET compatibility mode: AIs that can only read web pages follow plain-text pages.
// Every page lists the complete next URL with a fresh `n` so fetch caches never replay.
// The next-step URLs come FIRST: fetch tools often truncate long pages.
// Browsers and browser agents (Accept: text/html) get the same content as HTML
// with clickable links and a reply form.
export const GET_WAIT_DEFAULT_MS=8000;
/** Seed URLs are capabilities, not public pages; fetch tools that honor robots.txt may open them. */
export const ROBOTS_TXT='User-agent: *\nAllow: /\n';
const PAGE_JSON_LIMIT=6000;
function wantsHtml(req:IncomingMessage):boolean {
  const accept=String(req.headers.accept??'');
  return accept.includes('text/html')&&!accept.startsWith('text/plain');
}
const escapeHtml=(text:string)=>text.replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c] as string));
/** Linkify whole-token URLs only; templates that still contain <placeholders> stay text. */
function linkify(text:string):string {
  return text.split(/(\s+)/).map(token=>/^https?:\/\/[^\s<>"]+$/.test(token)?`<a href="${escapeHtml(token)}">${escapeHtml(token)}</a>`:escapeHtml(token)).join('');
}
export function htmlPage(text:string,base:string,replyTo?:string):string {
  const form=replyTo?`<form method="get" action="${escapeHtml(base)}/reply"><input type="hidden" name="n" value="${nonce()}"><input type="hidden" name="to" value="${escapeHtml(replyTo)}"><input type="hidden" name="final" value="1"><textarea name="text" rows="6" cols="80" placeholder="回复 Reply"></textarea><br><button type="submit">发送回复 Send reply</button></form>`:'';
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="robots" content="noindex"><title>Coding Tools MCP</title></head><body><pre style="white-space:pre-wrap">${linkify(text)}</pre>${form}</body></html>`;
}
function sendPage(req:IncomingMessage,res:ServerResponse,text:string,base:string,replyTo?:string):void {
  const html=wantsHtml(req);
  res.writeHead(200,{'content-type':html?'text/html; charset=utf-8':'text/plain; charset=utf-8','cache-control':'no-store','referrer-policy':'no-referrer','x-content-type-options':'nosniff','x-robots-tag':'noindex, nofollow'}).end(html?htmlPage(text,base,replyTo):text);
}
const nonce=()=>randomUUID().replace(/-/g,'').slice(0,12);
function baseUrl(req:IncomingMessage,path:string):string {
  const host=String(req.headers['x-forwarded-host']??req.headers.host??'127.0.0.1');
  const loopback=/^(127\.|localhost|\[::1\])/.test(host);
  const proto=String(req.headers['x-forwarded-proto']??(loopback?'http':'https'));
  return `${proto}://${host}${path.replace(/\/$/,'')}`;
}
export function guideText(base:string):string {
  return `Coding Tools MCP — 用户本人的本机项目连接 (the user's own local coding tool)

这个网址由用户在自己的 Coding Tools MCP 应用里生成，用来让你访问用户指定的项目并在应用的对话里和用户交流。
你不需要自己有 MCP 客户端或 seed_connect / chat_wait / chat_reply 工具：它们都在这个服务器上。
You do NOT need an MCP client or these tools yourself — they run on this server.

最简单 Easiest (web page reading is enough): 打开 Open ${base}/connect?n=${nonce()}
之后每个页面都会给出下一步的完整网址，照着打开即可。Each page gives the complete next URL.

其他方式 Other ways:
- MCP 客户端 MCP client: 把 ${base} 添加为 Streamable HTTP MCP 服务器（不需要额外认证），然后调用 seed_connect，再 chat_wait。
- HTTP POST (curl/python): POST JSON-RPC 到 ${base}，Content-Type: application/json，Accept: application/json, text/event-stream；
  先 initialize {"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"ai","version":"1"}}，
  再 notifications/initialized，之后请求加 MCP-Protocol-Version: 2025-03-26；tools/call seed_connect，然后 chat_wait {"timeout_ms":25000}。

收到消息后按说明完成工作并回复；回复后继续等待下一条消息。等待返回 idle 只表示用户还没发消息，再等一次即可。
`;
}
export function nextLinks(base:string,replyTo?:string):string {
  const n=nonce(),to=replyTo?encodeURIComponent(replyTo):'<message.id>';
  return `下一步 / Next (open the complete URL with your web/fetch tool):
- 等待用户消息 Wait for the next message: ${base}/wait?n=${n}
- 回复用户 Reply (append your URL-encoded text): ${base}/reply?n=${n}&to=${to}&final=1&text=
- 进度回复（未完成）Progress: same as Reply with final=0；长回复可分几次 final=0，最后一次 final=1
- 调用项目工具 Call a tool: ${base}/call?n=${n}&tool=<name>&args=<URL-encoded JSON>  (e.g. ${base}/call?n=${n}&tool=read_file&args=%7B%22path%22%3A%22README.md%22%7D)
- 工具列表 Tool list: ${base}/tools?n=${n}`;
}
/** Returns the page text and, when the result carries a user message, its id. */
export function summarize(action:string,result:JsonObject,base:string):{text:string,replyTo?:string} {
  const data=(result.structuredContent??result) as JsonObject;
  let out=`action: ${action}\nstatus: ${String(data.status??(data.ok===true?'ok':'-'))}\n`;
  let replyTo:string|undefined;
  if(data.status==='message'){const m=(data.message??{}) as JsonObject;replyTo=String(m.id??'')||undefined;out+=`\n用户消息 User message (id=${String(m.id??'')}):\n${String(m.text??'')}\n`;}
  if(typeof data.instruction==='string')out+=`\ninstruction: ${data.instruction}\n`;
  if(data.status==='idle')out+='\n用户还没发新消息，打开下面的「等待」网址继续等。No new message yet — open the Wait URL again.\n';
  const links=nextLinks(base,replyTo);
  let body=JSON.stringify(data,null,2);if(body.length>PAGE_JSON_LIMIT)body=body.slice(0,PAGE_JSON_LIMIT)+'\n…(truncated; use /call for full tool output)';
  return {text:`${out}\n${links}\n\nresult:\n${body}\n\n${links}\n`,replyTo};
}
/** Run one JSON-RPC request through the POST handler without a network round trip. */
async function internalRpc(path:string,context:ToolContext,body:JsonObject):Promise<JsonObject|undefined> {
  const req=Object.assign(Readable.from([Buffer.from(JSON.stringify(body))]),{method:'POST',headers:{'mcp-protocol-version':'2025-03-26','content-type':'application/json'}}) as unknown as IncomingMessage;
  let text='';
  const res=Object.assign(new EventEmitter(),{writableEnded:false,statusCode:200,setHeader(){return res;},writeHead(){return res;},end(chunk?:unknown){text=chunk===undefined?'':String(chunk);res.writableEnded=true;queueMicrotask(()=>res.emit('close'));return res;}});
  await handleSeedRoute(req,res as unknown as ServerResponse,path,context);
  if(!text)return undefined;
  const parsed=JSON.parse(text) as JsonObject;
  if(parsed.error)throw Error(String((parsed.error as JsonObject).message??'Seed request failed'));
  return parsed.result as JsonObject;
}
async function handleSeedGet(req:IncomingMessage,res:ServerResponse,pathname:string,action:string,context:ToolContext):Promise<void> {
  const seedPath=pathname.replace(/\/[a-z]{1,16}$/,''),base=baseUrl(req,seedPath);
  const query=new URL(req.url??'/','http://local').searchParams,n=query.get('n')??'';
  const call=(name:string,args:JsonObject)=>internalRpc(seedPath,context,{jsonrpc:'2.0',id:`get-${n}`,method:'tools/call',params:{name,arguments:args}});
  try{
    // Every page works as the first page: redeeming is idempotent.
    await internalRpc(seedPath,context,{jsonrpc:'2.0',id:'get-init',method:'initialize',params:{protocolVersion:'2025-03-26'}});
    let page:{text:string,replyTo?:string};
    if(action==='connect')page=summarize(action,(await call('seed_connect',{}))??{},base);
    else if(action==='wait'){
      // Fetch tools usually time out after 10–30 s; keep GET waits short by default.
      const requested=Number(query.get('wait')??GET_WAIT_DEFAULT_MS);
      const wait=Math.min(Number.isFinite(requested)&&requested>=0?requested:GET_WAIT_DEFAULT_MS,25000);
      page=summarize(action,(await call('chat_wait',{timeout_ms:wait}))??{},base);
    }
    else if(action==='reply'){
      const to=query.get('to')??'',reply=query.get('text')??'';if(!n||!to||!reply.trim())throw Error('reply needs n, to and text');
      const message_id=`get-${createHash('sha256').update(`${n}\0${to}`).digest('hex').slice(0,24)}`;
      page=summarize(action,(await call('chat_reply',{reply_to:to,message_id,text:reply,final:query.get('final')!=='0',...(query.get('ask')==='1'?{awaiting_user:true}:{})}))??{},base);
    }else if(action==='call'){
      const tool=query.get('tool')??'';if(!tool)throw Error('call needs tool');
      const raw=query.get('args');let args:JsonObject={};if(raw&&raw.trim()){try{args=JSON.parse(raw) as JsonObject;}catch(e){throw Error(`args must be JSON: ${e instanceof Error?e.message:e}`);}}
      page=summarize(action,(await call(tool,args))??{},base);
    }else if(action==='tools'){
      const listed=(await internalRpc(seedPath,context,{jsonrpc:'2.0',id:'get-tools',method:'tools/list'}))??{};
      page={text:`${nextLinks(base)}\n\ntools:\n`+((listed.tools??[]) as JsonObject[]).map(t=>`- ${String(t.name)}: ${String(t.description??'').slice(0,160)}`).join('\n')+'\n'};
    }else throw Error('Unknown action. Use connect, wait, reply, call or tools.');
    sendPage(req,res,page.text,base,page.replyTo);
  }catch(error){sendPage(req,res,`error: ${error instanceof Error?error.message:'Seed request failed'}\n\n${nextLinks(base,query.get('to')??undefined)}`,base);}
}
