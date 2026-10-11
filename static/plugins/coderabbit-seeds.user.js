// ==UserScript==
// @name         Coding Tools MCP 种子库
// @namespace    https://github.com/755287249/coding-tools-mcp
// @version      1.1.1
// @description  在当前 CodeRabbit 账号下批量创建并接入 MCP 项目种子库
// @match        https://app.coderabbit.ai/*
// @run-at       document-start
// @grant        unsafeWindow
// @grant        GM_xmlhttpRequest
// @grant        GM_getValue
// @grant        GM_setValue
// @grant        GM_registerMenuCommand
// @connect      *
// ==/UserScript==

(function(){'use strict';
/** Match the app's session transport; Clerk's SDK token is not its API token. */
function pageSession(page) {
  try {
    const account = JSON.parse(page.sessionStorage.getItem('user'))?.state?.user?.id;
    const access = page.sessionStorage.getItem('accessToken');
    const provider = page.localStorage.getItem('clerkGitProvider') || '';
    if (typeof account !== 'string' || !account.trim() || typeof access !== 'string' || !access.trim()) return null;
    if (provider && !/^[a-z0-9-]{1,80}$/.test(provider)) return null;
    // Identity deliberately excludes the rotating access token. Never persist this object.
    return {account, access, provider, identity: JSON.stringify([account, provider])};
  } catch { return null; }
}

/** Capture only routing identifiers from the page's own same-origin tRPC calls. */
function routingContext(url, headers, origin = 'https://app.coderabbit.ai') {
  let target;
  try { target = new URL(url, origin); } catch { return null; }
  if (target.origin !== origin || !target.pathname.startsWith('/trpc/')) return null;
  let h;
  try { h = new Headers(headers); } catch { return null; }
  const organization = h.get('x-coderabbitai-organization')?.trim();
  const workspace = h.get('x-coderabbitai-workspace')?.trim() || '';
  const valid = value => /^[A-Za-z0-9_-]{1,200}$/.test(value);
  if (!organization || !valid(organization) || (workspace && !valid(workspace))) return null;
  return {organization, workspace};
}
function observeRouting(page, onContext) {
  const originalFetch = page.fetch;
  page.fetch = function(input, init) {
    try {
      const context = routingContext(typeof input === 'string' || input instanceof URL ? String(input) : input.url,
        init?.headers !== undefined ? init.headers : input?.headers);
      if (context) onContext(context);
    } catch { /* Observation must never break the page's request. */ }
    return Reflect.apply(originalFetch, this, arguments);
  };
  const prototype = page.XMLHttpRequest?.prototype;
  if (prototype) {
    const requests = new WeakMap(), open = prototype.open, header = prototype.setRequestHeader, send = prototype.send;
    prototype.open = function(method, url) {
      requests.set(this, {url: String(url), headers: new Headers()});
      return Reflect.apply(open, this, arguments);
    };
    prototype.setRequestHeader = function(name, value) {
      const key = String(name).toLowerCase();
      if (key === 'x-coderabbitai-organization' || key === 'x-coderabbitai-workspace') requests.get(this)?.headers.append(name, value);
      return Reflect.apply(header, this, arguments);
    };
    prototype.send = function() {
      try { const request = requests.get(this); const context = request && routingContext(request.url, request.headers); if (context) onContext(context); } catch {}
      return Reflect.apply(send, this, arguments);
    };
  }
  // Use the original transport for our requests, so manual overrides cannot masquerade as page observations.
  return originalFetch.bind(page);
}

/** Pure provisioning contract; used by the userscript and fixture tests. */
function parseBundle(raw,now=Date.now()){
  const bundle=JSON.parse(raw);
  if(bundle.version!==1||!Array.isArray(bundle.seeds)||bundle.seeds.length<1||bundle.seeds.length>50)throw Error('批次须包含 1–50 颗种子');
  const url=new URL(bundle.endpoint);
  if(url.protocol!=='https:'&&!(url.protocol==='http:'&&['localhost','127.0.0.1','[::1]'].includes(url.hostname)))throw Error('接入地址须为 HTTPS 或本机回环地址');
  if(url.username||url.password||url.search||url.hash||!url.pathname.endsWith('/mcp'))throw Error('MCP 地址格式不正确');
  const valid=s=>typeof s==='string'&&/^[A-Za-z0-9_-]{1,80}$/.test(s);
  if(!valid(bundle.workspace_folder_id))throw Error('缺少目录 ID');
  const ids=new Set();
  for(const s of bundle.seeds){
    if(!valid(s.seed_id)||ids.has(s.seed_id)||typeof s.ticket!=='string'||!/^[a-f0-9]{64}$/.test(s.ticket)||!Number.isFinite(s.expires_at)||s.expires_at<=now||!s.repo_id||!s.branch||typeof s.repo_id!=='string'||typeof s.branch!=='string')throw Error('批次包含过期或无效种子');
    ids.add(s.seed_id);
  }
  return bundle;
}
function unwrapTrpc(body){
  const row=Array.isArray(body)?body[0]:body;
  if(!row||row.error)throw Error('CodeRabbit 返回接口错误；检查登录、账号权限、仓库和额度');
  if(!row.result||!Object.hasOwn(row.result,'data'))throw Error('CodeRabbit 接口格式变化，停止创建');
  const data=row.result.data;return data&&typeof data==='object'&&Object.hasOwn(data,'json')?data.json:data;
}
function createdTaskId(result){
  const candidates=[result?.taskId,result?.id,result?.task?.id,result?.task?.taskId];
  const id=candidates.find(v=>typeof v==='string'&&/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i.test(v));
  if(!id)throw Error('创建结果没有真实任务 ID；状态未知，请核对任务列表后再继续');return id;
}
function seedPrompt(bundle,seed){
  const endpoint=`${bundle.endpoint}/seeds/${encodeURIComponent(bundle.workspace_folder_id)}/${encodeURIComponent(seed.seed_id)}`;
  return `通过 HTTP MCP 接入种子库并等待分配。用户授权此任务作为当前项目的待命种子，在宿主允许的执行期限内持续等待，收到分配后处理同一条本地会话。\n接入地址：${endpoint}\n一次性入库票据：${seed.ticket}\n这是单颗种子的入库凭据，不是 OAuth 密码；不要将它写入文件、回复或日志。复用已有 HTTP 客户端，以 User-Agent: Coding-Tools-MCP/1.0 和 Authorization: Bearer <入库票据> POST initialize（JSON-RPC 2.0）。从 result._meta.seed_access_token 取回专用访问凭据，仅存客户端进程内存；后续 Authorization 改用它，MCP-Protocol-Version 使用返回的 2025-03-26。初始化响应丢失时一分钟内用同一票据重试 initialize。\n随后 notifications/initialized（无 id）→ tools/list → list_workspace_folders 核对目录 ${bundle.workspace_folder_id} → seed_wait(timeout_ms:25000)。每个外层工具调用只等待一次；idle 后再独立等待。assigned 后使用返回的 chat_id 和 attachment_id 调用 chat_open，完整读取 skill.text，保持 chat_wait → 工作 → chat_reply → 等待。所有交流及成果通过 MCP；保留客户端进程和内存凭据。\n仅通过此种子接入地址使用项目工具，成果写回指定 MCP 工作区。接替时先核对已有计划、文件、已完成回复及运行操作，避免重复副作用；不要依赖另一宿主沙箱的文件。retired、权限撤销、用户明确停止或宿主执行上限时停止，不尝试其他身份。`;
}
function creationPayload(bundle,seed){return {branch_name:seed.branch,clientRequestId:seed.seed_id,repoId:seed.repo_id,source:{type:'manual',instructions:seedPrompt(bundle,seed)},title:`MCP Seed ${seed.seed_id}`};}
function publicReceipt(seed,accountId,status,taskId=''){return {seed_id:seed.seed_id,account_id:accountId,repo_id:seed.repo_id,branch:seed.branch,status,task_id:taskId,updated_at:Date.now()};}

// Browser adapter: current authenticated account only; no registration or trial activation.
const RECEIPTS='ctmcp-seed-receipts-v1';
let stopped=false,running=false,detected=null,observedAccount='',contextRevision=0;
const pageFetch=observeRouting(unsafeWindow,context=>{
  const account=pageSession(unsafeWindow)?.identity;
  if(account){if(!detected||detected.account!==account||detected.organization!==context.organization||detected.workspace!==context.workspace)contextRevision++;detected={...context,account};updateContext();}
});
const panel=document.createElement('div');panel.style.cssText='position:fixed;right:20px;bottom:20px;z-index:2147483647';
const shadow=panel.attachShadow({mode:'closed'});
shadow.innerHTML=`<style>:host{font:14px system-ui;color:#e5e7eb}button,input,textarea{font:inherit}button{cursor:pointer;border:1px solid #475569;border-radius:8px;background:#1e293b;color:#f8fafc;padding:9px 12px}button:disabled{opacity:.5;cursor:default}.box{display:none;background:#0f172a;border:1px solid #334155;border-radius:14px;padding:18px;width:min(440px,85vw);box-shadow:0 20px 60px #0007;max-height:80vh;overflow:auto}.box.open{display:block}.toggle{margin-top:8px;float:right;background:#166534}h2{margin:0 0 12px;font-size:18px}p{font-size:12px;line-height:1.6;color:#cbd5e1}label{display:grid;gap:5px;margin:12px 0}input,textarea{box-sizing:border-box;background:#1e293b;color:#fff;border:1px solid #475569;border-radius:8px;padding:9px;width:100%}.actions{display:flex;gap:8px;flex-wrap:wrap}.log{white-space:pre-wrap;overflow-wrap:anywhere;font-size:12px;line-height:1.7;margin-top:12px}.primary{background:#166534}.error{color:#fca5a5}</style><section class="box"><h2>种子库 · CodeRabbit</h2><p>在 MCP 客户端选择项目并生成批次，粘贴到下面。使用当前登录账号逐个创建，只有客户端收到真实接入后才会显示可分配。页面关闭会停止创建；票据只保存在内存。</p><label>批次接入数据<textarea id="batch" rows="5" placeholder="粘贴客户端生成的 JSON"></textarea></label><p id="context-status" role="status">正在从当前页面自动识别组织和工作区…</p><details><summary>连接信息（自动识别，可手动调整）</summary><label><span><input id="manual" type="checkbox" style="width:auto"> 手动填写</span></label><label>组织 ID<input id="org" autocomplete="off" readonly placeholder="自动识别"></label><label>Workspace ID<input id="workspace" autocomplete="off" readonly placeholder="自动识别；当前账号未使用时留空"></label><button id="refresh-context">刷新页面重新识别</button></details><div class="actions"><button id="start" class="primary">检查并开始创建</button><button id="stop" disabled>完成当前请求后停止</button><button id="clear">清除接入数据</button><button id="history">查看创建记录</button></div><div class="log" role="status"></div></section><button class="toggle">种子库</button>`;
const $=selector=>shadow.querySelector(selector),log=$('.log');
const say=text=>{log.textContent=text;};
function updateContext(){
  const account=pageSession(unsafeWindow)?.identity||'';
  if(account!==observedAccount){
    observedAccount=account;contextRevision++;
    if(detected?.account!==account)detected=null;
    $('#manual').checked=false;$('#org').value='';$('#workspace').value='';
  }
  const manual=$('#manual').checked;
  $('#org').readOnly=!manual;$('#workspace').readOnly=!manual;
  if(!manual){$('#org').value=detected?.organization||'';$('#workspace').value=detected?.workspace||'';}
  $('#context-status').textContent=!account?'请先登录 CodeRabbit。':manual?'正在使用手动连接信息。':detected?`已自动识别当前账号的组织 ${detected.organization}，${detected.workspace?'工作区 '+detected.workspace:'当前页面未指定工作区'}。`:'等待页面连接信息；请刷新 CodeRabbit 页面，或切换一次组织/工作区，无需打开开发者工具。';
}
function selectedContext(){
  updateContext();
  const organization=$('#org').value.trim(),workspace=$('#workspace').value.trim();
  if(!organization)throw Error('尚未识别组织。请刷新页面后重试，或展开连接信息手动填写。');
  if(!/^[A-Za-z0-9_-]{1,200}$/.test(organization)||(workspace&&!/^[A-Za-z0-9_-]{1,200}$/.test(workspace)))throw Error('组织或工作区 ID 格式不正确');
  return {organization,workspace};
}
$('#manual').onchange=updateContext;
$('#refresh-context').onclick=()=>{if(!running&&(!$('#batch').value||window.confirm('刷新会清除当前粘贴的批次，客户端页面仍可重新复制。继续？')))unsafeWindow.location.reload();};
setInterval(updateContext,1000);updateContext();
$('.toggle').onclick=()=>$('.box').classList.toggle('open');
$('#clear').onclick=()=>{if(!running){$('#batch').value='';say('接入数据已从插件界面清除。');}};
$('#history').onclick=()=>{const rows=GM_getValue(RECEIPTS,[]);say(rows.map(r=>`${r.seed_id.slice(0,8)} · ${r.account_id} · ${r.status}${r.task_id?' · '+r.task_id:''}`).join('\n')||'暂无创建记录');};
$('#stop').onclick=()=>{stopped=true;say('将完成当前请求并记录结果，然后停止。');};
function remember(receipt){const rows=GM_getValue(RECEIPTS,[]).filter(r=>r.seed_id!==receipt.seed_id);rows.push(receipt);GM_setValue(RECEIPTS,rows.slice(-1000));}
function mcpCreated(bundle,seed,taskId){return new Promise((resolve,reject)=>GM_xmlhttpRequest({method:'POST',url:`${bundle.endpoint}/seeds/${bundle.workspace_folder_id}/${seed.seed_id}`,headers:{'Content-Type':'application/json','User-Agent':'Coding-Tools-MCP/1.0','MCP-Protocol-Version':'2025-03-26',Authorization:`Bearer ${seed.ticket}`},data:JSON.stringify({jsonrpc:'2.0',id:'created-'+seed.seed_id,method:'seed/created',params:{task_id:taskId}}),timeout:15000,onload:r=>{try{const body=JSON.parse(r.responseText);if(r.status!==200||body.error||body.result?.ok!==true)throw Error('客户端未确认宿主任务记录');resolve();}catch{reject(Error('客户端任务登记未确认；不要重复创建宿主任务'));}},onerror:()=>reject(Error('客户端不可达；宿主任务已创建，勿重复创建')),ontimeout:()=>reject(Error('客户端登记超时；宿主任务已创建，勿重复创建'))}));}
$('#start').onclick=async()=>{
  if(running)return;
  let bundle;
  try{
    bundle=parseBundle($('#batch').value);
    const login=pageSession(unsafeWindow);
    if(!login)throw Error('CodeRabbit 页面登录会话尚未就绪。请先登录并刷新页面后重试。');
    const accountId=login.account,identity=login.identity;
    const {organization:org,workspace}=selectedContext(),revision=contextRevision;
    const existing=GM_getValue(RECEIPTS,[]);
    const work=bundle.seeds.filter(seed=>!existing.some(r=>r.seed_id===seed.seed_id));
    if(work.length!==bundle.seeds.length)throw Error('批次中已有提交记录。请在创建记录和宿主页面核对；不会重复提交');
    if(!window.confirm(`当前账号 ${accountId}\n组织 ${org}\n将创建 ${work.length} 个宿主任务。\n仓库：${[...new Set(work.map(s=>s.repo_id))].join(', ')}\n分支：${[...new Set(work.map(s=>s.branch))].join(', ')}\n任务会按宿主的额度和并发规则运行。开始？`))return;
    running=true;stopped=false;$('#start').disabled=true;$('#stop').disabled=false;$('#clear').disabled=true;$('#manual').disabled=true;$('#org').disabled=true;$('#workspace').disabled=true;$('#refresh-context').disabled=true;
    const lines=[];
    for(const seed of work){
      if(stopped)break;
      const session=pageSession(unsafeWindow);
      if(!session||session.identity!==identity)throw Error('当前登录会话已退出或账号已切换，停止剩余批次');
      if(seed.expires_at<=Date.now())throw Error('剩余入库票据已过期，请在客户端重新准备');
      const context=selectedContext();
      if(contextRevision!==revision||context.organization!==org||context.workspace!==workspace)throw Error('账号、组织或工作区已切换，停止剩余批次；已提交任务保留。');
      remember(publicReceipt(seed,accountId,'submitting'));
      lines.push(`${seed.seed_id.slice(0,8)} · 提交中`);say(lines.join('\n'));
      let taskId;
      try{
        const response=await pageFetch('https://app.coderabbit.ai/trpc/codingAgent.enqueueCodingTask?batch=1',{method:'POST',credentials:'include',headers:{authorization:`Bearer ${session.access}`,'content-type':'application/json','x-trpc-source':'react',...(session.provider?{'x-clerk-git-provider':session.provider}:{}),'x-coderabbitai-organization':org,...(workspace?{'x-coderabbitai-workspace':workspace}:{})},body:JSON.stringify({'0':creationPayload(bundle,seed)}),signal:AbortSignal.timeout(30000)});
        if(!response.ok)throw Error(`HTTP ${response.status}`);
        taskId=createdTaskId(unwrapTrpc(await response.json()));
      }catch{
        remember(publicReceipt(seed,accountId,'unknown'));
        throw Error(`种子 ${seed.seed_id.slice(0,8)} 创建结果不明，已停止。请在 CodeRabbit 任务列表搜索完整种子 ID 核对；不会自动重试或重建。`);
      }
      remember(publicReceipt(seed,accountId,'created',taskId));
      lines[lines.length-1]=`${seed.seed_id.slice(0,8)} · 已创建 · ${taskId}`;
      try{await mcpCreated(bundle,seed,taskId);}catch(e){lines.push(String(e.message));}say(lines.join('\n')+'\n等待宿主接入；实际就绪状态见 MCP 种子库。');
      await new Promise(resolve=>setTimeout(resolve,1000));
    }
    $('#batch').value='';say(lines.join('\n')+(stopped?'\n批次已停止；未提交的票据稍后过期。':'\n本批次创建流程完成。客户端收到种子握手后才会计入可分配数量。'));
  }catch(e){say(String(e.message??e));}
  finally{running=false;$('#start').disabled=false;$('#stop').disabled=true;$('#clear').disabled=false;$('#manual').disabled=false;$('#org').disabled=false;$('#workspace').disabled=false;$('#refresh-context').disabled=false;bundle=undefined;}
};
function mount(){if(document.body&&!panel.isConnected)document.body.append(panel);}
if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',mount,{once:true});else mount();
GM_registerMenuCommand('打开 MCP 种子库',()=>{mount();$('.box').classList.add('open');});

})();
