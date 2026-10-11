// ==UserScript==
// @name         Coding Tools MCP 种子库
// @namespace    https://github.com/755287249/coding-tools-mcp
// @version      1.3.2
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
    const state = JSON.parse(page.sessionStorage.getItem('user'))?.state;
    const account = state?.user?.id;
    const access = page.sessionStorage.getItem('accessToken');
    const provider = page.localStorage.getItem('clerkGitProvider') || '';
    if (typeof account !== 'string' || !account.trim() || typeof access !== 'string' || !access.trim()) return null;
    if (provider && !/^[a-z0-9-]{1,80}$/.test(provider)) return null;
    // Identity deliberately excludes the rotating access token. Never persist this object.
    const user=state.user,org=state.currentOrganization;
    return {account, access, provider, identity: JSON.stringify([account, provider]),
      accountName:user.providerLogin||user.userName||account,providerUserId:user.providerUserId||'',
      organization:org?.id||'',workspace:org?.workspace_id||page.localStorage.getItem('clerkSsoWorkspaceId')||'',
      orgName:org?.organization_name||'',providerOrg:org?.provider_organization_id||'',orgType:org?.type||provider};
  } catch { return null; }
}

function pageSelection(page) {
  const login=pageSession(page);
  if(!login)throw Error('请先登录 CodeRabbit 并刷新页面。');
  if(!login.organization||!login.orgName)throw Error('请在 CodeRabbit 选择组织。');
  if(page.location.pathname.replace(/\/$/,'')!=='/code/new')throw Error('请打开 CodeRabbit 的“New coding task”页面并选择仓库。');
  const repoName=page.localStorage.getItem('coding-agent-last-selected-repository')||'';
  const repository=page.document.querySelector('[aria-label^="Repository: "]');
  const branches=[...new Set([...page.document.querySelectorAll('[aria-label^="Branch: "]')].map(e=>e.getAttribute('aria-label').slice(8).trim()))];
  if(!repoName||!repository||repository.getAttribute('aria-label').slice(12).trim()!==(repoName.split('/').filter(Boolean).at(-1)??repoName)||branches.length!==1||!branches[0])throw Error('请在网页中选择仓库和分支，油猴会自动读取。');
  const branch=branches[0];
  return {...login,repoName,branch,selection:JSON.stringify([login.identity,login.organization,login.workspace,repoName,branch])};
}

function browserPairing(address,token) {
  const url=new URL(address.trim());
  if(url.protocol!=='https:'&&!(url.protocol==='http:'&&['localhost','127.0.0.1','[::1]'].includes(url.hostname)))throw Error('配对地址须为 HTTPS 或本机回环地址');
  if(url.username||url.password||url.search||url.hash)throw Error('配对地址格式不正确');
  url.pathname=url.pathname.replace(/\/$/,'');
  const match=url.pathname.match(/^(.*\/mcp)\/seed-library\/([A-Za-z0-9_-]{1,80})$/);
  if(!match||!/^[a-f0-9]{64}$/.test(token.trim()))throw Error('请复制客户端的完整配对地址和 64 位令牌');
  return {address:url.toString(),token:token.trim(),endpoint:url.origin+match[1],workspace_folder_id:match[2]};
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
    if(s.url_key!==undefined&&(typeof s.url_key!=='string'||!/^[a-f0-9]{64}$/.test(s.url_key)))throw Error('无效种子 URL');
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
  const endpoint=`${bundle.endpoint}/seeds/${encodeURIComponent(bundle.workspace_folder_id)}/${encodeURIComponent(seed.url_key??seed.seed_id)}`;
  if(seed.url_key!==undefined){
    if(!/^[a-f0-9]{64}$/.test(seed.url_key))throw Error('无效种子 URL');
    return `${endpoint} 快速连接这个 sandbox 的 MCP(URL)，明确使用规则，熟悉可用工具，做好处理接下来一系列工作的准备。chat_wait/chat_reply 工具在 mcp 内部，而非你的客户端内部，连上后使用，然后等待信息`;
  }
  return `通过 HTTP MCP 接入种子库并等待分配。用户授权此任务作为当前项目的待命种子，在宿主允许的执行期限内持续等待，收到分配后处理同一条本地会话。\n接入地址：${endpoint}\n一次性入库票据：${seed.ticket}\n这是单颗种子的入库凭据，不是 OAuth 密码；不要将它写入文件、回复或日志。复用已有 HTTP 客户端，以 User-Agent: Coding-Tools-MCP/1.0 和 Authorization: Bearer <入库票据> POST initialize（JSON-RPC 2.0）。从 result._meta.seed_access_token 取回专用访问凭据，仅存客户端进程内存；后续 Authorization 改用它，MCP-Protocol-Version 使用返回的 2025-03-26。初始化响应丢失时一分钟内用同一票据重试 initialize。\n随后 notifications/initialized（无 id）→ tools/list → list_workspace_folders 核对目录 ${bundle.workspace_folder_id} → seed_wait(timeout_ms:25000)。每个外层工具调用只等待一次；idle 后再独立等待。assigned 后使用返回的 chat_id 和 attachment_id 调用 chat_open，完整读取 skill.text，保持 chat_wait → 工作 → chat_reply → 等待。所有交流及成果通过 MCP；保留客户端进程和内存凭据。\n仅通过此种子接入地址使用项目工具，成果写回指定 MCP 工作区。接替时先核对已有计划、文件、已完成回复及运行操作，避免重复副作用；不要依赖另一宿主沙箱的文件。retired、权限撤销、用户明确停止或宿主执行上限时停止，不尝试其他身份。`;
}
function creationPayload(bundle,seed){return {branch_name:seed.branch,clientRequestId:seed.seed_id,repoId:seed.repo_id,source:{type:'manual',instructions:seedPrompt(bundle,seed)},title:`${seed.prefix?seed.prefix+' ':''}MCP Seed ${seed.seed_id}`};}
function publicReceipt(seed,accountId,status,taskId=''){return {seed_id:seed.seed_id,account_id:accountId,repo_id:seed.repo_id,branch:seed.branch,status,task_id:taskId,updated_at:Date.now()};}

// Current browser account only. The saved pairing capability cannot run project tools.
const CONNECTION='ctmcp-seed-connection-v2',RECENT='ctmcp-seed-recent-v2',RECEIPT='ctmcp-seed-receipt-v2:';
const pageFetch=unsafeWindow.fetch.bind(unsafeWindow);
let connection=null,context=null,selection='',contextVersion=0,running=false,binding=false,stopped=false,job=null;
const panel=document.createElement('div');panel.style.cssText='position:fixed;right:20px;bottom:20px;z-index:2147483647;pointer-events:auto';
const shadow=panel.attachShadow({mode:'closed'});
shadow.innerHTML=`<style>:host{font:14px system-ui;color:#e5e7eb}button,input{font:inherit}button{cursor:pointer;border:1px solid #475569;border-radius:8px;background:#1e293b;color:#f8fafc;padding:8px 11px}button:disabled{opacity:.5;cursor:default}.box{display:none;background:#0f172a;border:1px solid #334155;border-radius:14px;padding:18px;width:min(440px,85vw);box-shadow:0 20px 60px #0007;max-height:80vh;overflow:auto}.box.open{display:block}.toggle{margin-top:8px;float:right;background:#166534}h2{margin:0 0 12px;font-size:18px}p{font-size:12px;line-height:1.7;color:#cbd5e1}label{display:grid;gap:5px;margin:12px 0}input{box-sizing:border-box;background:#1e293b;color:#fff;border:1px solid #475569;border-radius:8px;padding:9px;width:100%}.actions{display:flex;gap:8px;flex-wrap:wrap}.log{white-space:pre-wrap;overflow-wrap:anywhere;font-size:12px;line-height:1.7;margin-top:12px}.primary{background:#166534}.context{background:#1e293b;border-radius:9px;padding:12px;white-space:pre-wrap;overflow-wrap:anywhere}summary{cursor:pointer}progress{width:100%;margin-top:12px}</style><section class="box"><h2>种子库 · CodeRabbit</h2><details id="connection" open><summary>连接客户端</summary><label>配对地址<input id="address" type="url" placeholder="粘贴客户端的配对地址"></label><label>配对令牌<input id="token" type="password" autocomplete="off" placeholder="粘贴客户端令牌"><button id="show-token" type="button" aria-pressed="false">显示令牌</button></label><div class="actions"><button id="bind" class="primary">连接并保存</button><button id="forget">忘记连接</button></div><p>只保存配对地址和令牌，用于向该项目补充种子。网页登录令牌和入库票据不额外保存。</p></details><p id="connection-status">尚未连接客户端。</p><p id="context-status" class="context" role="status">正在识别当前页面…</p><button id="refresh-context">刷新账号和仓库</button><label>模型前缀 / 备注<input id="prefix" maxlength="200" placeholder="例如 GPT-6-astra"></label><label>创建数量<input id="count" type="number" min="1" max="10000" value="3"></label><div class="actions"><button id="start" class="primary">创建并入库</button><button id="stop" disabled>完成当前请求后停止</button><button id="discard">结束本轮</button><button id="history">创建记录</button></div><progress id="progress" value="0" max="1"></progress><div class="log" role="status"></div><p>使用新建任务页当前选中的账号、仓库和分支。种子逐个创建，实际并发与额度由 CodeRabbit 决定。</p></section><button class="toggle">种子库</button>`;
const $=selector=>shadow.querySelector(selector),say=text=>{$('.log').textContent=text;};
function headers(login){return {authorization:`Bearer ${login.access}`,'content-type':'application/json','x-trpc-source':'react',...(login.provider?{'x-clerk-git-provider':login.provider}:{}),'x-coderabbitai-organization':login.organization,...(login.workspace?{'x-coderabbitai-workspace':login.workspace}:{})};}
async function refreshContext(force=false){
  let page;try{page=pageSelection(unsafeWindow);}catch(e){if(selection){selection='';contextVersion++;}context=null;$('#context-status').textContent=e.message;return;}
  if(!force&&page.selection===selection)return;
  selection=page.selection;context=null;const version=++contextVersion;
  $('#context-status').textContent=`账号：${page.accountName}\n仓库：${page.repoName}\n正在核对仓库 ID…`;
  try{
    const input={provider:page.provider||'github',orgIds:page.providerOrg,orgName:page.orgName,type:page.orgType||'github',perPage:100,page:1,searchQuery:page.repoName,skipInstallEligibility:true};
    const response=await pageFetch('https://app.coderabbit.ai/trpc/repositories.getAllRepos?batch=1&input='+encodeURIComponent(JSON.stringify({'0':input})),{credentials:'include',headers:headers(page),signal:AbortSignal.timeout(20000)});
    if(!response.ok)throw Error(`仓库读取失败（HTTP ${response.status}），请刷新网页后重试。`);
    const data=unwrapTrpc(await response.json());if(data.loadFailed)throw Error('仓库列表加载失败，请在网页重新选择仓库。');
    const repos=(data.repos||[]).filter(r=>r.name===page.repoName);
    if(repos.length!==1||!repos[0].id)throw Error('无法唯一识别当前仓库，请在网页重新选择仓库后刷新识别。');
    if(version!==contextVersion||pageSelection(unsafeWindow).selection!==page.selection)return;
    const {access,...publicPage}=page;
    context={...publicPage,repoId:String(repos[0].id)};
    $('#context-status').textContent=`账号：${page.accountName}${page.providerUserId?' · GitHub ID '+page.providerUserId:''}\n仓库：${page.repoName} · ${context.repoId}\n分支：${page.branch}\n组织：${page.organization}${page.workspace?'\n工作区：'+page.workspace:''}`;
  }catch(e){if(version===contextVersion)$('#context-status').textContent=String(e.message||e);}
}
function bridge(link,args){return new Promise((resolve,reject)=>GM_xmlhttpRequest({method:'POST',url:link.address,headers:{'Content-Type':'application/json',Authorization:`Bearer ${link.token}`},data:JSON.stringify(args),timeout:20000,onload:r=>{try{const body=JSON.parse(r.responseText);if(r.status!==200||body.error)throw Error(body.error||`配对请求失败（HTTP ${r.status}）`);if(body.workspace_folder_id!==link.workspace_folder_id)throw Error('客户端返回了其他项目，已停止');resolve(body);}catch(e){reject(e);}},onerror:()=>reject(Error('客户端不可达：请检查完整配对地址、客户端是否在线，以及油猴是否已允许访问该地址；远程浏览器不能使用另一台电脑的 127.0.0.1。')),onabort:()=>reject(Error('配对请求已取消，请重试')),ontimeout:()=>reject(Error('客户端请求超时；本轮配对请求可用相同编号重试'))}));}
async function bindConnection(){
  if(running||binding)return;
  if(job){say('请先结束本轮，再更换连接。');return;}
  binding=true;controls(true);$('#bind').textContent='连接中…';
  $('#connection-status').textContent='正在验证配对地址和令牌…';say('正在连接客户端…');
  try{
    const link=browserPairing($('#address').value,$('#token').value);
    const state=await bridge(link,{action:'status'});
    if(state.ok!==true)throw Error('客户端未确认连接');
    GM_setValue(CONNECTION,{address:link.address,token:link.token,count:Number($('#count').value)||3,prefix:$('#prefix').value.trim()});
    connection=link;
    $('#connection-status').textContent=`已连接项目 ${link.workspace_folder_id} · ${state.total} / ${state.capacity} 颗种子`;
    say('连接已保存。确认上方账号、仓库和分支后即可创建。');
    await refreshContext(true);
  }catch(e){connection=null;$('#connection-status').textContent='连接失败，请检查下方提示。';say(String(e.message||e));}
  finally{binding=false;controls(running);$('#bind').textContent='连接并保存';}
}
function receipt(id){return GM_getValue(RECEIPT+id,null);}
function remember(value){GM_setValue(RECEIPT+value.seed_id,value);const ids=GM_getValue(RECENT,[]).filter(id=>id!==value.seed_id);ids.push(value.seed_id);GM_setValue(RECENT,ids.slice(-200));}
function mcpCreated(bundle,seed,taskId){return new Promise((resolve,reject)=>GM_xmlhttpRequest({method:'POST',url:`${bundle.endpoint}/seeds/${bundle.workspace_folder_id}/${seed.seed_id}`,headers:{'Content-Type':'application/json','User-Agent':'Coding-Tools-MCP/1.0','MCP-Protocol-Version':'2025-03-26',Authorization:`Bearer ${seed.ticket}`},data:JSON.stringify({jsonrpc:'2.0',id:'created-'+seed.seed_id,method:'seed/created',params:{task_id:taskId}}),timeout:15000,onload:r=>{try{const body=JSON.parse(r.responseText);if(r.status!==200||body.error||body.result?.ok!==true)throw Error();resolve();}catch{reject(Error('宿主任务已创建，但客户端登记未确认；已停止，勿重复创建。'));}},onerror:()=>reject(Error('宿主任务已创建，客户端暂不可达；勿重复创建。')),ontimeout:()=>reject(Error('宿主任务已创建，客户端登记超时；勿重复创建。'))}));}
function currentLogin(expected){const login=pageSelection(unsafeWindow);if(login.selection!==expected.selection)throw Error('账号、组织、工作区、仓库或分支已切换，停止剩余创建。');return login;}
function controls(value){for(const id of ['start','bind','forget','address','token','count','prefix','refresh-context','discard'])$('#'+id).disabled=value;$('#stop').disabled=!running;}
$('#start').onclick=async()=>{
  if(running||binding)return;
  try{
    if(!connection)throw Error('请先连接客户端。');
    if(!context)throw Error('请等待账号和仓库识别完成，或点击“刷新账号和仓库”。');
    currentLogin(context);
    const prefix=$('#prefix').value.trim();if(/[\r\n\0]/.test(prefix))throw Error('前缀不能包含换行。');
    const count=Number($('#count').value);if(!Number.isInteger(count)||count<1||count>10000)throw Error('数量须为 1–10000。');
    if(job&&job.prefix!==prefix)throw Error('本轮前缀已经固定，请先结束本轮再更改。');
    if(job?.blocked)throw Error('上一轮存在结果不明的任务，请先核对创建记录；结束本轮后再添加新种子。');
    if(job&&job.context.selection!==context.selection)throw Error('上一轮属于另一个账号或仓库，请先结束本轮。');
    if(!window.confirm(`账号：${context.accountName}\n仓库：${context.repoName}\n分支：${context.branch}\n${job?'继续剩余 '+job.remaining:'创建 '+count} 颗种子？`))return;
    currentLogin(context);
    job??={context:{...context},prefix,remaining:count,total:count,queue:[],pending:null,blocked:false};
    GM_setValue(CONNECTION,{address:connection.address,token:connection.token,count,prefix});
    running=true;stopped=false;controls(true);
    $('#progress').max=job.total;
    while((job.remaining>0||job.registration)&&!stopped){
      if(job.registration){await mcpCreated(connection,job.registration.seed,job.registration.taskId);job.registration=null;if(!job.remaining)break;}
      currentLogin(job.context);
      if(!job.queue.length){
        job.pending??={action:'batch',prefix:job.prefix,request_id:crypto.randomUUID(),count:Math.min(10,job.remaining),account:job.context.accountName,repo_id:job.context.repoId,branch:job.context.branch,metadata:{account_id:job.context.account,provider_user_id:String(job.context.providerUserId),repo_name:job.context.repoName,organization_id:job.context.organization,workspace_id:job.context.workspace}};
        const response=await bridge(connection,job.pending);currentLogin(job.context);
        const bundle=parseBundle(JSON.stringify({version:1,endpoint:connection.endpoint,workspace_folder_id:connection.workspace_folder_id,seeds:response.batch}));
        if(bundle.seeds.length!==job.pending.count||bundle.seeds.some(s=>s.repo_id!==job.context.repoId||s.branch!==job.context.branch||(s.prefix??'')!==job.prefix))throw Error('客户端批次信息不匹配，已停止');
        job.queue=bundle.seeds;job.pending=null;
      }
      if(stopped)break;
      const seed=job.queue[0],login=currentLogin(job.context);
      if(receipt(seed.seed_id)){job.blocked=true;throw Error('这颗种子已有提交记录，请核对后结束本轮；不会重复提交。');}
      if(seed.expires_at<=Date.now())throw Error('本轮票据已过期，请结束本轮后重新添加。');
      remember(publicReceipt(seed,login.account,'submitting'));
      let taskId;
      try{
        const response=await pageFetch('https://app.coderabbit.ai/trpc/codingAgent.enqueueCodingTask?batch=1',{method:'POST',credentials:'include',headers:headers(login),body:JSON.stringify({'0':creationPayload(connection,seed)}),signal:AbortSignal.timeout(30000)});
        if(!response.ok)throw Error();taskId=createdTaskId(unwrapTrpc(await response.json()));
      }catch{job.blocked=true;remember(publicReceipt(seed,login.account,'unknown'));throw Error(`种子 ${seed.seed_id} 创建结果不明，已停止。请在 CodeRabbit 列表核对，不要重复创建。`);}
      remember(publicReceipt(seed,login.account,'created',taskId));job.queue.shift();job.remaining--;
      $('#progress').value=job.total-job.remaining;
      say(`已创建 ${job.total-job.remaining} / ${job.total}\n最近任务：${taskId}\n实际接入状态请查看客户端种子库。`);
      job.registration={seed,taskId};await mcpCreated(connection,seed,taskId);job.registration=null;
      if(job.remaining&&!stopped)await new Promise(resolve=>setTimeout(resolve,1000));
    }
    if(!job.remaining){say(`本轮 ${job.total} 颗种子已创建并登记。等待宿主接入，客户端会自动刷新。`);job=null;}
    else say(`已暂停，剩余 ${job.remaining} 颗。保持当前页面，可点击继续。`);
  }catch(e){say(String(e.message||e));}
  finally{running=false;controls(false);$('#start').textContent=job?'继续本轮':'创建并入库';}
};
$('#bind').onclick=bindConnection;
$('#show-token').onclick=()=>{const input=$('#token'),visible=input.type==='password';input.type=visible?'text':'password';$('#show-token').textContent=visible?'隐藏令牌':'显示令牌';$('#show-token').setAttribute('aria-pressed',String(visible));};
$('#forget').onclick=()=>{if(running)return;if(job){say('请先结束本轮，再忘记连接。');return;}GM_setValue(CONNECTION,null);connection=null;$('#token').value='';$('#connection').open=true;$('#connection-status').textContent='连接已从油猴移除。';};
$('#stop').onclick=()=>{stopped=true;say('完成当前请求后停止。');};
$('#discard').onclick=()=>{if(!running){job=null;$('#start').textContent='创建并入库';say('本轮进度已清除；已创建任务保留，未使用票据稍后过期。');}};
$('#refresh-context').onclick=()=>void refreshContext(true);
$('#history').onclick=()=>{const rows=GM_getValue(RECENT,[]).map(receipt).filter(Boolean);say(rows.reverse().map(r=>`${r.seed_id}\n${r.account_id} · ${r.status}${r.task_id?' · '+r.task_id:''}`).join('\n\n')||'暂无创建记录');};
$('.toggle').onclick=()=>$('.box').classList.toggle('open');
const saved=GM_getValue(CONNECTION,null);if(saved){$('#address').value=saved.address||'';$('#token').value=saved.token||'';$('#count').value=saved.count||3;$('#prefix').value=saved.prefix||'';try{connection=browserPairing(saved.address,saved.token);$('#connection-status').textContent='已载入保存的连接；创建时会核验令牌。';}catch{}}
setInterval(()=>void refreshContext(),1000);void refreshContext();
// Keep native input/click defaults, but do not send panel interactions to page
// shortcuts or document-level focus traps through the closed shadow boundary.
for(const type of ['focusin','focusout','keydown','keyup','keypress','pointerdown','pointerup','click','dblclick','contextmenu']){
  shadow.addEventListener(type,event=>event.stopPropagation());
}
function mount(){
  if(!document.body)return;
  // Native dialogs make siblings inert; app dialogs may trap focus or pointers.
  // Retain the same panel/inputs when a dialog opens, closes or is replaced.
  const dialogs=[...document.querySelectorAll('dialog[open], [role="dialog"][aria-modal="true"], [role="alertdialog"][aria-modal="true"]')];
  const target=dialogs.filter(node=>node.getAttribute('data-state')!=='closed'&&node.getClientRects().length).at(-1)||document.body;
  if(panel.parentNode!==target){const focused=shadow.activeElement;target.append(panel);focused?.focus({preventScroll:true});}
}
const mountObserver=new MutationObserver(()=>mount());
mountObserver.observe(document.documentElement,{childList:true,subtree:true,attributes:true,attributeFilter:['open','data-state','hidden','aria-hidden']});
if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',mount,{once:true});else mount();
GM_registerMenuCommand('打开 MCP 种子库',()=>{mount();$('.box').classList.add('open');});

})();
