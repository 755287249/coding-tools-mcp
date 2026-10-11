import {callTool} from '../../tools.js';
import {McpToolCallLifecycle} from '../mcp/lifecycle.js';
import type {IncomingMessage,ServerResponse} from 'node:http';
import type {JsonObject,ToolContext,ToolDefinition} from '../../types.js';
import {sendJson} from '../../oauth.js';
import {readRequestBody} from '../http.js';
import {currentToolCatalog} from '../catalog.js';
import {dispatchMcpMethod} from '../mcp/dispatcher.js';
import {seedStore} from '../../chat/store.js';
import {seedUrl,seedProvision,seedInitialize,seedAuthenticate,seedCreated,seedPoll,seedBegin,seedFinish,seedPendingProcesses,seedProcessSettled} from '../../chat/seeds.js';
import {seedConnectSkill,localChatSkill} from '../../rustCatalog.generated.js';
import {setTimeout as sleep} from 'node:timers/promises';

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
  const match=pathname.match(/^\/mcp\/seeds\/([A-Za-z0-9_-]{1,80})\/([A-Za-z0-9_-]{1,80})$/);if(!match)return false;
  res.setHeader('cache-control','no-store');
  const folder=context.config.folders.find(f=>f.id===match[1]);if(!folder){sendJson(res,404,{error:'Seed unavailable'});return true;}
  const store=seedStore(folder.path),urlMode=/^[a-f0-9]{64}$/.test(match[2]);
  let seedId=match[2],token=String(req.headers.authorization??'').replace(/^Bearer /,'');
  if(req.method==='GET'){
    if(req.headers.accept==='text/event-stream'){res.setHeader('allow','POST');sendJson(res,405,{error:'SSE stream not provided; use POST'});return true;}
    try{seedUrl(store,match[2]);if(!currentToolCatalog(context).names.includes('chat_open'))throw Error();res.setHeader('referrer-policy','no-referrer');sendJson(res,200,{name:'coding-tools-seed',transport:'streamable-http',instructions:seedConnectSkill.text});}
    catch{sendJson(res,404,{error:'Seed unavailable'});}return true;
  }
  if(req.method!=='POST'){sendJson(res,405,{error:'POST required'});return true;}
  let id:unknown=null,waitKey:string|undefined;
  try{
    const body=JSON.parse((await readRequestBody(req)).toString()) as JsonObject;id=body.id??null;
    if(body.jsonrpc!=='2.0'||typeof body.method!=='string')throw Error('Invalid JSON-RPC request');
    const method=body.method,params=(body.params??{}) as JsonObject;
    const protocol=req.headers['mcp-protocol-version'];
    if(method!=='initialize'&&protocol!=='2025-03-26')throw Error('MCP-Protocol-Version must match negotiated 2025-03-26');
    if(!currentToolCatalog(context).names.includes('chat_open'))throw Error('Seed access requires a tool profile with local chat enabled');
    if(urlMode){seedId=seedUrl(store,match[2],method==='initialize');token=match[2];}
    if(method==='initialize'){
      const access=urlMode?'':seedInitialize(store,seedId,token);
      sendJson(res,200,{jsonrpc:'2.0',id,result:{protocolVersion:'2025-03-26',capabilities:{tools:{},resources:{}},serverInfo:{name:'coding-tools-seed',version:'1'},instructions:seedConnectSkill.text,...(urlMode?{}:{_meta:{seed_access_token:access}})}});return true;
    }
    if(method==='seed/created'){seedCreated(store,seedId,token,String(params.task_id??''));sendJson(res,200,{jsonrpc:'2.0',id,result:{ok:true}});return true;}
    seedAuthenticate(store,seedId,token);
    if(method==='notifications/initialized'&&body.id===undefined){res.writeHead(202).end();return true;}
    if(body.id===undefined)throw Error('Request ID required');
    let result:unknown;
    const catalog=currentToolCatalog(context);
    if(method==='tools/list')result={tools:[connectTool,foldersTool,waitTool,...catalog.tools.filter(t=>SEED_TOOLS.has(t.name)).map(scopedTool)]};
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
        sendJson(res,200,{jsonrpc:'2.0',id,result:wrapped({...status,registered:status.status!=='retired',seed_id:seedId,workspace_folder_id:folder.id,workspace:folder,skill:seedConnectSkill,instruction:'Read skill.text, then immediately chat_wait(timeout_ms:25000).'})});return true;
      }
      let autoOpen=false;
      const chatWait=name==='chat_wait';
      if(chatWait){waitKey=folder.path+':'+seedId;if(waiting.has(waitKey)){waitKey=undefined;throw Error('A seed_wait is already active');}waiting.add(waitKey);const timeout=Number(args.timeout_ms);if(!Number.isInteger(timeout)||timeout<0||timeout>25000)throw Error('timeout_ms must be 0–25000');const status=seedPoll(store,seedId,token);if(status.status==='retired'){sendJson(res,200,{jsonrpc:'2.0',id,result:wrapped({...status,registered:false,instruction:'Seed retired. Stop waiting and do not switch identities.'})});return true;}if(args.chat_id!==undefined&&args.chat_id!==status.chat_id)throw Error('Seed may only access its assigned chat');if(args.attachment_id!==undefined&&args.attachment_id!==status.attachment_id)throw Error('Seed attachment mismatch');if(status.status!=='assigned')name='seed_wait';else if(status.needs_open){name='chat_open';autoOpen=true;}}

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
        }else result=wrapped(chatWait?{...status,registered:status.status!=='retired',instruction:status.status==='retired'?'Seed retired. Stop waiting and do not switch identities.':'Immediately call chat_wait(timeout_ms:25000) again. Idle is normal.'}:status);}finally{waiting.delete(key);}
      }else{
        if(!SEED_TOOLS.has(name)||!catalog.names.includes(name))throw Error('Tool not available to seeds');
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
