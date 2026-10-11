/** Match the app's session transport; Clerk's SDK token is not its API token. */
export function pageSession(page) {
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

export function pageSelection(page) {
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

export function browserPairing(address,token) {
  const url=new URL(address.trim());
  if(url.protocol!=='https:'&&!(url.protocol==='http:'&&['localhost','127.0.0.1','[::1]'].includes(url.hostname)))throw Error('配对地址须为 HTTPS 或本机回环地址');
  if(url.username||url.password||url.search||url.hash)throw Error('配对地址格式不正确');
  url.pathname=url.pathname.replace(/\/$/,'');
  const match=url.pathname.match(/^(.*\/mcp)\/seed-library\/([A-Za-z0-9_-]{1,80})$/);
  if(!match||!/^[a-f0-9]{64}$/.test(token.trim()))throw Error('请复制客户端的完整配对地址和 64 位令牌');
  return {address:url.toString(),token:token.trim(),endpoint:url.origin+match[1],workspace_folder_id:match[2]};
}

/** Capture only routing identifiers from the page's own same-origin tRPC calls. */
export function routingContext(url, headers, origin = 'https://app.coderabbit.ai') {
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
export function observeRouting(page, onContext) {
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
