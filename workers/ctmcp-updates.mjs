/** Isolated /ctmcp routes for the existing multi-application Worker.
 * Call before the legacy router; a null result means this is not our route.
 * Never reads or changes legacy REPO, tickets, quotas, or KV state.
 */
export async function ctmcpUpdates(request, env, fetcher = fetch) {
 const url = new URL(request.url), route = url.pathname.replace(/\/+$/, '');
 if (route !== '/ctmcp' && !route.startsWith('/ctmcp/')) return null;
 const json = (body, status = 200) => new Response(JSON.stringify(body), {status, headers:{'content-type':'application/json','cache-control':'no-store','access-control-allow-origin':'*'}});
 if (request.method === 'OPTIONS') return new Response(null,{status:204,headers:{'access-control-allow-origin':'*','access-control-allow-methods':'GET, OPTIONS'}});
 if (request.method !== 'GET') return json({error:'Method not allowed'},405);
 if (!['/ctmcp/latest','/ctmcp/download'].includes(route)) return json({error:'Not found'},404);
 const repo=env.CTMCP_REPO;
 if (typeof repo!=='string'||!/^[-\w.]+\/[-\w.]+$/.test(repo)||repo.split('/').some(p=>p==='.'||p==='..')) return json({error:'Configure CTMCP_REPO as owner/repository'},503);
 const headers={'user-agent':'CodingToolsMCP-Updater','accept':'application/vnd.github+json','x-github-api-version':'2022-11-28'};
 const token=env.CTMCP_GH_TOKEN||env.GH_TOKEN;if(token)headers.authorization=`Bearer ${token}`;
 try {
  const beta=env.CTMCP_CHANNEL==='beta';
  const releaseResponse=await fetcher(`https://api.github.com/repos/${repo}/releases${beta?'?per_page=100':'/latest'}`,{headers,signal:AbortSignal.timeout(20000)});
  if(!releaseResponse.ok)return json({error:releaseResponse.status===404?'Publish a stable Coding Tools release first':'GitHub release metadata unavailable'},releaseResponse.status===404?404:502);
  const payload=await releaseResponse.json();
  const data=beta&&Array.isArray(payload)?payload.filter(r=>!r.draft&&/^(?:client-v|v)?\d+\.\d+\.\d+$/.test(String(r.tag_name))&&r.assets?.some(a=>a.name===`ctmcp-${String(r.tag_name).replace(/^(?:client-v|v)/,'')}-win64.exe`)).sort((a,b)=>{const av=String(a.tag_name).replace(/^(?:client-v|v)/,'').split('.').map(Number),bv=String(b.tag_name).replace(/^(?:client-v|v)/,'').split('.').map(Number);return bv[0]-av[0]||bv[1]-av[1]||bv[2]-av[2]})[0]:payload;
  if(!data)return json({error:'No Coding Tools release in the selected channel'},404);
  const tag=String(data.tag_name??''),version=tag.replace(/^(?:client-v|v)/,'');
  if(data.draft||(!beta&&data.prerelease)||!/^\d+\.\d+\.\d+$/.test(version))return json({error:'Invalid stable Coding Tools version'},502);
  const name=`ctmcp-${version}-win64.exe`,asset=data.assets?.find(a=>a.name===name);
  const sha256=String(asset?.digest??'').replace(/^sha256:/,'').toLowerCase();
  if(!asset||!Number.isSafeInteger(asset.id)||asset.id<=0||!Number.isSafeInteger(asset.size)||asset.size<=0||asset.size>200*1024*1024||!/^sha256:[a-f0-9]{64}$/i.test(String(asset.digest)))return json({error:'Coding Tools Windows EXE and GitHub SHA-256 are required'},502);
  if(route==='/ctmcp/latest')return json({appId:'coding-tools-mcp',version,tag,notes:String(data.body??''),date:data.published_at??'',files:{portable:{name,size:asset.size,sha256}}});
  if(url.searchParams.get('version')!==version||url.searchParams.get('sha256')!==sha256)return json({error:'Release changed; check for updates again'},409);
  const download=await fetcher(`https://api.github.com/repos/${repo}/releases/assets/${asset.id}`,{headers:{...headers,accept:'application/octet-stream'},signal:AbortSignal.timeout(180000)});
  if(!download.ok)return json({error:'GitHub download unavailable'},502);
  const length=download.headers.get('content-length');
  if(length!==null&&Number(length)!==asset.size)return json({error:'Asset size changed'},502);
  return new Response(download.body,{headers:{'content-type':'application/octet-stream','content-disposition':`attachment; filename="${name}"`,'cache-control':'no-store','x-content-type-options':'nosniff','access-control-allow-origin':'*',...(length?{'content-length':length}:{})}});
 }catch{return json({error:'Coding Tools update service temporarily unavailable'},502)}
}
