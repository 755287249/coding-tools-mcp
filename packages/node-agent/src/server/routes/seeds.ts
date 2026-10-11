import {callTool} from '../../tools.js';
import {McpToolCallLifecycle} from '../mcp/lifecycle.js';
import type {IncomingMessage,ServerResponse} from 'node:http';
import type {JsonObject,ToolContext} from '../../types.js';
import {sendJson} from '../../oauth.js';
import {readRequestBody} from '../http.js';
import {currentToolCatalog} from '../catalog.js';
import {dispatchMcpMethod} from '../mcp/dispatcher.js';
import {seedStore} from '../../chat/store.js';
import {seedInitialize,seedAuthenticate,seedCreated,seedPoll,seedBegin,seedFinish,seedPendingProcesses,seedProcessSettled} from '../../chat/seeds.js';
import {localChatSkill} from '../../rustCatalog.generated.js';
import {setTimeout as sleep} from 'node:timers/promises';

// Exclude external tools, workspace switching, permissions replay and other chats.
export const SEED_TOOLS=new Set(['chat_open','chat_wait','chat_reply','chat_upload','chat_close','set_todos','update_plan','report_progress','read_file','list_files','list_directory','search_files','search_text','search_code','grep','glob','view_image','apply_patch','edit','read_many','project_map','patch_check','format_files','edit_file','write_file','file_ops','exec_command','wait_command','send_input','kill_session','read_output','git_status','git_diff','git_log']);
const waitTool={name:'seed_wait',description:'Wait for a conversation assignment. One independent wait at a time; idle is normal. Then chat_open with returned chat_id/attachment_id.',inputSchema:{type:'object',properties:{timeout_ms:{type:'integer',minimum:0,maximum:25000}},required:['timeout_ms'],additionalProperties:false}};
const foldersTool={name:'list_workspace_folders',description:'Show the single project authorized for this seed.',inputSchema:{type:'object',properties:{},additionalProperties:false}};
const waiting=new Set<string>();
function wrapped(result:unknown){return {content:[{type:'text',text:JSON.stringify(result)}],structuredContent:result,isError:false};}
export async function handleSeedRoute(req:IncomingMessage,res:ServerResponse,pathname:string,context:ToolContext):Promise<boolean>{
  const match=pathname.match(/^\/mcp\/seeds\/([A-Za-z0-9_-]{1,80})\/([A-Za-z0-9_-]{1,80})$/);if(!match)return false;
  res.setHeader('cache-control','no-store');
  if(req.method!=='POST'){sendJson(res,405,{error:'POST required'});return true;}
  const folder=context.config.folders.find(f=>f.id===match[1]);if(!folder){sendJson(res,404,{error:'Seed unavailable'});return true;}
  const seedId=match[2],store=seedStore(folder.path),token=String(req.headers.authorization??'').replace(/^Bearer /,'');
  let id:unknown=null;
  try{
    const body=JSON.parse((await readRequestBody(req)).toString()) as JsonObject;id=body.id??null;
    if(body.jsonrpc!=='2.0'||typeof body.method!=='string')throw Error('Invalid JSON-RPC request');
    const method=body.method,params=(body.params??{}) as JsonObject;
    const protocol=req.headers['mcp-protocol-version'];
    if(method!=='initialize'&&protocol!=='2025-03-26')throw Error('MCP-Protocol-Version must match negotiated 2025-03-26');
    if(!currentToolCatalog(context).names.includes('chat_open'))throw Error('Seed access requires a tool profile with local chat enabled');
    if(method==='initialize'){
      const access=seedInitialize(store,seedId,token);
      sendJson(res,200,{jsonrpc:'2.0',id,result:{protocolVersion:'2025-03-26',capabilities:{tools:{},resources:{}},serverInfo:{name:'coding-tools-seed',version:'1'},instructions:'Keep _meta.seed_access_token in client memory and use it as Bearer on this endpoint. List tools and list_workspace_folders, then seed_wait. Assigned work uses only this endpoint and project. Never create another attachment or use another seed identity.',_meta:{seed_access_token:access}}});return true;
    }
    if(method==='seed/created'){seedCreated(store,seedId,token,String(params.task_id??''));sendJson(res,200,{jsonrpc:'2.0',id,result:{ok:true}});return true;}
    seedAuthenticate(store,seedId,token);
    if(method==='notifications/initialized'&&body.id===undefined){res.writeHead(202).end();return true;}
    if(body.id===undefined)throw Error('Request ID required');
    let result:unknown;
    const catalog=currentToolCatalog(context);
    if(method==='tools/list')result={tools:[foldersTool,waitTool,...catalog.tools.filter(t=>SEED_TOOLS.has(t.name))]};
    else if(method==='resources/read'&&params.uri===localChatSkill.uri)result={contents:[localChatSkill]};
    else if(method==='resources/list')result={resources:[{uri:localChatSkill.uri,name:localChatSkill.name,mimeType:localChatSkill.mimeType}]};
    else if(method==='ping')result={};
    else if(method==='tools/call'){
      const name=String(params.name??''),args={...((params.arguments??{}) as JsonObject)};
      if(name==='list_workspace_folders')result=wrapped({ok:true,folders:[{id:folder.id,name:folder.name,path:folder.path,selected:true}],selected_folder_id:folder.id});
      else if(name==='seed_wait'){
        const timeout=Number(args.timeout_ms);if(!Number.isInteger(timeout)||timeout<0||timeout>25000)throw Error('timeout_ms must be 0–25000');
        for(const pending of seedPendingProcesses(store)){
          const check=await callTool(context,'wait_command',{workspace_folder_id:folder.id,session_id:pending.session_id,timeout_ms:0},{'openai/session':`seed:${pending.seed_id}`});
          if(check.ok!==false&&check.process_still_running===false)seedProcessSettled(store,pending.seed_id,pending.session_id);
        }
        const key=folder.path+':'+seedId;if(waiting.has(key))throw Error('A seed_wait is already active');waiting.add(key);
        try{const deadline=Date.now()+timeout;let status;do{status=seedPoll(store,seedId,token);if(status.status!=='idle'||Date.now()>=deadline)break;await sleep(1000);}while(true);result=wrapped(status);}finally{waiting.delete(key);}
      }else{
        if(!SEED_TOOLS.has(name)||!catalog.names.includes(name))throw Error('Tool not available to seeds');
        if(args.workspace_folder_id!==undefined&&args.workspace_folder_id!==folder.id)throw Error('Seed workspace mismatch');
        if(name==='read_output'&&!args.session_id)throw Error('Session ID required');
        const binding=seedBegin(store,seedId,token,name,args,String(id));
        args.workspace_folder_id=folder.id;
        if(name.startsWith('chat_')||['set_todos','update_plan','report_progress'].includes(name)){args.chat_id=binding.chat_id;args.attachment_id=binding.attachment_id;}
        if(name==='exec_command'&&!args.operation_id)args.operation_id=`seed-${seedId}-${binding.call_id.slice(0,20)}`;
        let actual:JsonObject={__unknown:true};const lifecycle=new McpToolCallLifecycle(context,req,res);
        try{
          const request={jsonrpc:'2.0',id,method,params:{name,arguments:args,_meta:{'openai/session':`seed:${seedId}`}}};
          result=await dispatchMcpMethod({catalog,context,method,req,request,protocolVersion:'2025-03-26',startedAt:Date.now(),processLifecycle:lifecycle.process});
          actual=((result as JsonObject).structuredContent??{}) as JsonObject;lifecycle.complete();
        }catch(e){lifecycle.abort();throw e;}finally{lifecycle.dispose();seedFinish(store,seedId,binding.call_id,name,args,actual);}
      }
    }else throw Error('Method not available to seeds');
    sendJson(res,200,{jsonrpc:'2.0',id,result});
  }catch(error){sendJson(res,200,{jsonrpc:'2.0',id,error:{code:-32000,message:error instanceof Error?error.message:'Seed request failed'}});}
  return true;
}
