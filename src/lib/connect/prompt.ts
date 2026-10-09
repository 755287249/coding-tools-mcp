import type { Locale } from "$lib/i18n";

export interface ConnectionInfo {
  workspaceName: string;
  endpoint: string;
  authType: string;
  clientId: string;
  password: string;
  bearerToken: string;
  folders: string[];
}

export const MANUAL_OAUTH_REDIRECT_URI = "http://127.0.0.1:8765/callback";
export const MCP_USER_AGENT = "Coding-Tools-MCP/1.0";

type PromptLanguage = "en" | "zh-CN" | "zh-TW";

function promptLanguage(locale: Locale): PromptLanguage {
  if (locale === "zh-CN" || locale === "zh-TW") return locale;
  return "en";
}

/**
 * Host setup and a successful tool call are separate connection requirements.
 * Keepalive: the authorization password is reusable and tokens are long-lived,
 * so the AI should save tokens and refresh instead of re-authorizing.
 */
const TEXT = {
  en: {
    oauthCompact: `Auth: OAuth + S256 PKCE, public client. Native MCP hosts: add the URL as a remote server and enter the password on the login page. Manual HTTP: discover endpoints from /.well-known/oauth-authorization-server, redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}. Check state. The password is reusable but is not a token; keep access_token + refresh_token and refresh on 401.`,
    compactFlow: `Steps: 1) connect via native MCP, or HTTP MCP with a User-Agent header; 2) initialize → notifications/initialized (no id) → tools/list; 3) list_workspace_folders must succeed — only then you are connected. HTTP access is not host registration. Never put credentials in files or replies.`,
    oauthManual: `Auth: OAuth + S256 PKCE, public client (native MCP hosts use their own callback and show a login page — enter the password there). Manual HTTP flow: discover auth/token endpoints from /.well-known/oauth-authorization-server; redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}. Generate a random state and a PKCE verifier/challenge. POST the authorize form with response_type=code, client_id, redirect_uri, state, code_challenge, code_challenge_method=S256 and the password; do not follow the redirect — read code and state from Location and verify state. Exchange with grant_type=authorization_code, code, client_id, the same redirect_uri and code_verifier. Tokens are long-lived: save access_token and refresh_token, reuse them, and on 401 use grant_type=refresh_token. The password is reusable but is not a token.`,
    bearerReuse: `Auth: fixed Bearer token, no OAuth. It stays valid until regenerated; on 401 ask the user for a fresh prompt.`,
    intro: "Connect to my Coding Tools MCP server, then follow the steps below.",
    url: "URL: ",
    clientId: (id: string) => `Client ID: ${id} (no client secret)`,
    password: "Authorization password: ",
    bearer: "Auth header: ",
    multi: `Several project folders exist: use the supplied workspace_folder_id; otherwise conversation_bootstrap returns the choices.`,
    fallback: `Steps:
1) Prefer adding this as a remote MCP server in your host. If the host cannot, call it over HTTP MCP (OAuth/PKCE or Bearer) and always send a User-Agent header (Cloudflare rejects requests without one).
2) initialize → notifications/initialized (no id) → tools/list.
3) Call list_workspace_folders; only a successful result counts as connected. HTTP access is not host registration — if setup is blocked, tell the user the exact steps instead of guessing.
4) Remote AIs need a public URL. Keep credentials in host storage or memory only, never in files or replies.
5) Then follow the session instructions below, if supplied.`,
  },
  "zh-CN": {
    oauthCompact: `认证：OAuth + S256 PKCE，公共客户端。原生 MCP 宿主：把地址添加为远程服务器，在登录页输入密码。手动 HTTP：从 /.well-known/oauth-authorization-server 发现端点，redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}，校验 state。密码可重复使用但不是令牌；保存 access_token 和 refresh_token，401 时用刷新令牌续期。`,
    compactFlow: `步骤：1）用原生 MCP 连接，或用 HTTP MCP 并带 User-Agent 请求头；2）initialize → notifications/initialized（无 id）→ tools/list；3）list_workspace_folders 成功才算接通。HTTP 接通不等于宿主注册。凭据不写入文件或回复。`,
    oauthManual: `认证：OAuth + S256 PKCE，公共客户端（原生 MCP 宿主用自身回调，会弹出登录页，在那里输入密码）。手动 HTTP 流程：从 /.well-known/oauth-authorization-server 发现授权/令牌端点；redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}。生成随机 state 和 PKCE verifier/challenge；POST 授权表单，带 response_type=code、client_id、redirect_uri、state、code_challenge、code_challenge_method=S256 和密码；不要跟随跳转，从 Location 读取 code 和 state 并校验 state。再用 grant_type=authorization_code、code、client_id、原 redirect_uri、code_verifier 换取令牌。令牌长期有效：保存并复用 access_token 和 refresh_token，401 时用 grant_type=refresh_token 续期。密码可重复使用，但不是令牌。`,
    bearerReuse: `认证：固定 Bearer，无需 OAuth；在重新生成前一直有效，401 时请用户重新复制提示词。`,
    intro: "请连接我的 Coding Tools MCP 服务器，然后按下面的步骤执行。",
    url: "地址：",
    clientId: (id: string) => `Client ID：${id}（无 Client Secret）`,
    password: "授权密码：",
    bearer: "认证头：",
    multi: `有多个项目目录：优先使用给定的 workspace_folder_id；否则 conversation_bootstrap 会返回可选目录。`,
    fallback: `步骤：
1）优先在宿主里把它添加为远程 MCP 服务器；宿主不支持时改用 HTTP MCP（OAuth/PKCE 或 Bearer），并始终带 User-Agent 请求头（Cloudflare 会拒绝没有它的请求）。
2）initialize → notifications/initialized（无 id）→ tools/list。
3）调用 list_workspace_folders，成功才算接通。HTTP 接通不等于宿主注册；配置受阻时告诉用户具体步骤，不要猜。
4）远程 AI 需要公网地址。凭据只存在宿主或内存中，绝不写入文件或回复。
5）然后执行下面附带的会话指令（如有）。`,
  },
  "zh-TW": {
    oauthCompact: `認證：OAuth + S256 PKCE，公開用戶端。原生 MCP 宿主：把位址新增為遠端伺服器，在登入頁輸入密碼。手動 HTTP：從 /.well-known/oauth-authorization-server 探索端點，redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}，驗證 state。密碼可重複使用但不是權杖；保存 access_token 和 refresh_token，401 時用重新整理權杖續期。`,
    compactFlow: `步驟：1）用原生 MCP 連接，或用 HTTP MCP 並帶 User-Agent 標頭；2）initialize → notifications/initialized（無 id）→ tools/list；3）list_workspace_folders 成功才算連通。HTTP 連通不等於宿主註冊。憑證不寫入檔案或回覆。`,
    oauthManual: `認證：OAuth + S256 PKCE，公開用戶端（原生 MCP 宿主用自身回呼，會顯示登入頁，在那裡輸入密碼）。手動 HTTP 流程：從 /.well-known/oauth-authorization-server 探索授權/權杖端點；redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}。產生隨機 state 和 PKCE verifier/challenge；POST 授權表單，帶 response_type=code、client_id、redirect_uri、state、code_challenge、code_challenge_method=S256 和密碼；不要跟隨重新導向，從 Location 讀取 code 和 state 並驗證 state。再用 grant_type=authorization_code、code、client_id、原 redirect_uri、code_verifier 換取權杖。權杖長期有效：保存並重用 access_token 和 refresh_token，401 時用 grant_type=refresh_token 續期。密碼可重複使用，但不是權杖。`,
    bearerReuse: `認證：固定 Bearer，不需 OAuth；在重新產生前一直有效，401 時請使用者重新複製提示詞。`,
    intro: "請連接我的 Coding Tools MCP 伺服器，然後依照下面的步驟執行。",
    url: "位址：",
    clientId: (id: string) => `Client ID：${id}（無 Client Secret）`,
    password: "授權密碼：",
    bearer: "認證標頭：",
    multi: `有多個專案目錄：優先使用給定的 workspace_folder_id；否則 conversation_bootstrap 會回傳可選目錄。`,
    fallback: `步驟：
1）優先在宿主裡把它新增為遠端 MCP 伺服器；宿主不支援時改用 HTTP MCP（OAuth/PKCE 或 Bearer），並始終帶 User-Agent 標頭（Cloudflare 會拒絕沒有它的請求）。
2）initialize → notifications/initialized（無 id）→ tools/list。
3）呼叫 list_workspace_folders，成功才算連通。HTTP 連通不等於宿主註冊；設定受阻時告訴使用者具體步驟，不要猜。
4）遠端 AI 需要公開位址。憑證只存在宿主或記憶體中，絕不寫入檔案或回覆。
5）然後執行下面附帶的會話指令（如有）。`,
  },
} as const;

export function isTemporaryEndpoint(endpoint: string): boolean {
  try {
    return new URL(endpoint).hostname.endsWith(".trycloudflare.com");
  } catch {
    return false;
  }
}

/** Build the copy-paste prompt any AI assistant can follow to connect. */
export function buildConnectionPrompt(info: ConnectionInfo, locale: Locale, compact = false): string {
  const text = TEXT[promptLanguage(locale)];
  const lines: string[] = [text.intro, `${text.url}${info.endpoint}`];
  if (info.authType === "oauth") {
    lines.push(text.clientId(info.clientId), `${text.password}${info.password}`, compact ? text.oauthCompact : text.oauthManual);
  } else if (info.authType === "bearer") {
    lines.push(`${text.bearer}Authorization: Bearer ${info.bearerToken}`, text.bearerReuse);
  }
  if (!compact && info.folders.length > 1) lines.push(text.multi);
  lines.push(compact ? text.compactFlow : text.fallback);
  return lines.join("\n");
}

/** JSON config for MCP clients that accept a remote URL with static headers. */
export function buildClientConfigJson(info: ConnectionInfo): string {
  const headers: Record<string, string> = { "User-Agent": MCP_USER_AGENT };
  const server: Record<string, unknown> = { type: "http", url: info.endpoint, headers };
  if (info.authType === "bearer" && info.bearerToken) {
    headers.Authorization = `Bearer ${info.bearerToken}`;
  }
  return JSON.stringify({ mcpServers: { "coding-tools": server } }, null, 2);
}
