// Browser adapter: current authenticated account only; no registration or trial activation.
const RECEIPTS='ctmcp-seed-receipts-v1';
let stopped=false,running=false;
const panel=document.createElement('div');panel.style.cssText='position:fixed;right:20px;bottom:20px;z-index:2147483647';
const shadow=panel.attachShadow({mode:'closed'});
shadow.innerHTML=`<style>:host{font:14px system-ui;color:#e5e7eb}button,input,textarea{font:inherit}button{cursor:pointer;border:1px solid #475569;border-radius:8px;background:#1e293b;color:#f8fafc;padding:9px 12px}button:disabled{opacity:.5;cursor:default}.box{display:none;background:#0f172a;border:1px solid #334155;border-radius:14px;padding:18px;width:min(440px,85vw);box-shadow:0 20px 60px #0007;max-height:80vh;overflow:auto}.box.open{display:block}.toggle{margin-top:8px;float:right;background:#166534}h2{margin:0 0 12px;font-size:18px}p{font-size:12px;line-height:1.6;color:#cbd5e1}label{display:grid;gap:5px;margin:12px 0}input,textarea{box-sizing:border-box;background:#1e293b;color:#fff;border:1px solid #475569;border-radius:8px;padding:9px;width:100%}.actions{display:flex;gap:8px;flex-wrap:wrap}.log{white-space:pre-wrap;overflow-wrap:anywhere;font-size:12px;line-height:1.7;margin-top:12px}.primary{background:#166534}.error{color:#fca5a5}</style><section class="box"><h2>种子库 · CodeRabbit</h2><p>在 MCP 客户端选择项目并生成批次，粘贴到下面。使用当前登录账号逐个创建，只有客户端收到真实接入后才会显示可分配。页面关闭会停止创建；票据只保存在内存。</p><label>批次接入数据<textarea id="batch" rows="5" placeholder="粘贴客户端生成的 JSON"></textarea></label><label>组织 ID<input id="org" autocomplete="off" placeholder="当前 CodeRabbit 组织 ID"></label><label>Workspace ID（如账号需要）<input id="workspace" autocomplete="off"></label><div class="actions"><button id="start" class="primary">检查并开始创建</button><button id="stop" disabled>完成当前请求后停止</button><button id="clear">清除接入数据</button><button id="history">查看创建记录</button></div><div class="log" role="status"></div></section><button class="toggle">种子库</button>`;
const $=selector=>shadow.querySelector(selector),log=$('.log');
const say=text=>{log.textContent=text;};
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
    const clerk=unsafeWindow.Clerk;
    const accountId=clerk?.user?.id;
    if(!accountId||!clerk?.session)throw Error('先在 CodeRabbit 页面登录；当前页面未提供可用的 Clerk 会话');
    const org=$('#org').value.trim(),workspace=$('#workspace').value.trim();
    if(!org)throw Error('请填写当前组织 ID，避免创建到错误的账号空间');
    const existing=GM_getValue(RECEIPTS,[]);
    const work=bundle.seeds.filter(seed=>!existing.some(r=>r.seed_id===seed.seed_id));
    if(work.length!==bundle.seeds.length)throw Error('批次中已有提交记录。请在创建记录和宿主页面核对；不会重复提交');
    if(!window.confirm(`当前账号 ${accountId}\n组织 ${org}\n将创建 ${work.length} 个宿主任务。\n仓库：${[...new Set(work.map(s=>s.repo_id))].join(', ')}\n分支：${[...new Set(work.map(s=>s.branch))].join(', ')}\n任务会按宿主的额度和并发规则运行。开始？`))return;
    running=true;stopped=false;$('#start').disabled=true;$('#stop').disabled=false;$('#clear').disabled=true;
    const lines=[];
    for(const seed of work){
      if(stopped)break;
      if(clerk.user?.id!==accountId)throw Error('当前账号已切换，停止剩余批次');
      if(seed.expires_at<=Date.now())throw Error('剩余入库票据已过期，请在客户端重新准备');
      const access=await clerk.session.getToken();if(!access)throw Error('登录失效，请重新登录');
      remember(publicReceipt(seed,accountId,'submitting'));
      lines.push(`${seed.seed_id.slice(0,8)} · 提交中`);say(lines.join('\n'));
      let taskId;
      try{
        const response=await unsafeWindow.fetch('https://app.coderabbit.ai/trpc/codingAgent.enqueueCodingTask?batch=1',{method:'POST',credentials:'include',headers:{authorization:`Bearer ${access}`,'content-type':'application/json','x-trpc-source':'react','x-clerk-git-provider':'github','x-coderabbitai-organization':org,...(workspace?{'x-coderabbitai-workspace':workspace}:{})},body:JSON.stringify({'0':creationPayload(bundle,seed)}),signal:AbortSignal.timeout(30000)});
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
  finally{running=false;$('#start').disabled=false;$('#stop').disabled=true;$('#clear').disabled=false;bundle=undefined;}
};
function mount(){if(document.body&&!panel.isConnected)document.body.append(panel);}
if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',mount,{once:true});else mount();
GM_registerMenuCommand('打开 MCP 种子库',()=>{mount();$('.box').classList.add('open');});
