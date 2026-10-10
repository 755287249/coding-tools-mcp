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

/** Host setup and a successful tool call are separate connection requirements. */
const TEXT = {
  en: {
    oauthCompact: `OAuth/S256 PKCE: discover endpoints, verify state; manual redirect_uri=${MANUAL_OAUTH_REDIRECT_URI} (native hosts use theirs). Password is not a Bearer token.`,
    compactFlow: `User-Agent: ${MCP_USER_AGENT}. Use native/HTTP MCP; initialize and execute returned instructions. Keep credentials only in host/memory, never files/replies.`,
    oauthManual: `Manual OAuth (native hosts use their callback): discover auth/token endpoints; redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}. Generate random state + PKCE verifier/challenge. Authorize: response_type=code, client_id, redirect_uri, state, code_challenge, code_challenge_method=S256; submit password once, do not follow redirects. Verify state from Location, then exchange code with grant_type=authorization_code, client_id, same redirect_uri, code_verifier. Reuse unexpired tokens; retain any refresh_token in host storage/memory and use grant_type=refresh_token, client_id and refresh_token on expiry. Reuse the authorization password if credentials were lost; keep the live client and its tokens in memory across calls. Password is not a token.`,
    bearerReuse: `Reuse Bearer until rotated/deleted; no OAuth. On 401 check credentials.`,
    intro: "Connect to this MCP server and read the project root:",
    url: "URL: ",
    clientId: (id: string) => `Client ID: ${id} (no secret)`,
    password: "Authorization password: ",
    bearer: "Auth header: ",
    multi: `Use the supplied workspace_folder_id; otherwise conversation_bootstrap returns folder choices.`,
    fallback: `Configure remote MCP in the host; if unsupported, use HTTP MCP (OAuth/PKCE or Bearer). Send User-Agent (Cloudflare). initialize → notifications/initialized (no id) → tools/list → list_workspace_folders must succeed. HTTP access is not host registration; if blocked, give setup steps. Remote AI needs a public URL. Keep credentials in host storage/memory, never files/replies. Follow the session instructions below, if supplied.`,
  },
  "zh-CN": {
    oauthCompact: `OAuth/S256 PKCE：发现端点并校验state；手动redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}（宿主用自身回调）。密码不是Bearer令牌。`,
    compactFlow: `User-Agent: ${MCP_USER_AGENT}。用宿主/HTTP客户端按MCP初始化，读取并执行instructions；凭据只存宿主/内存，勿写文件/回复。`,
    oauthManual: `手动 OAuth（原生宿主用自身回调）：发现授权/令牌端点，redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}。生成随机 state 和 PKCE verifier/challenge；授权传 response_type=code、client_id、redirect_uri、state、code_challenge、code_challenge_method=S256。密码提交一次，不跟随跳转；校验 Location 的 state，再以 grant_type=authorization_code、code、client_id、原 redirect_uri、code_verifier 换令牌。复用未过期令牌；如返回 refresh_token，仅存宿主/内存，过期以 grant_type=refresh_token、client_id、refresh_token 刷新。凭据丢失时可用原授权密码重新授权；保留客户端进程和内存令牌供后续调用。密码不是令牌。`,
    bearerReuse: `固定 Bearer 复用至重新生成/删除，无需 OAuth；401 时检查凭据。`,
    intro: "连接 MCP 服务器并读取项目根目录：",
    url: "地址：",
    clientId: (id: string) => `Client ID：${id}（无 Secret）`,
    password: "授权密码：",
    bearer: "认证头：",
    multi: `有目标 workspace_folder_id 就使用它；否则 conversation_bootstrap 返回目录选择。`,
    fallback: `优先宿主配置；不支持则用 HTTP MCP（OAuth/PKCE 或 Bearer），带 User-Agent（Cloudflare）。initialize → notifications/initialized（无 id）→ tools/list → list_workspace_folders 成功才确认接通。后续 MCP-Protocol-Version 使用协商版本。HTTP 接通不等于宿主注册；受阻给出配置步骤。远程 AI 用公开地址，凭据仅存宿主/内存，不写文件或回复。若附有会话指令，立即执行。`,
  },
  "zh-TW": {
    oauthCompact: `OAuth/S256 PKCE：探索端點並驗證state；手動redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}（宿主用自身回呼）。密碼不是Bearer權杖。`,
    compactFlow: `User-Agent: ${MCP_USER_AGENT}。用宿主/HTTP用戶端按MCP初始化，讀取並執行instructions；憑證只存宿主/記憶體，勿寫檔案/回覆。`,
    oauthManual: `手動 OAuth（原生宿主用自身回呼）：探索授權/權杖端點，redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}。產生隨機 state 和 PKCE verifier/challenge；授權傳 response_type=code、client_id、redirect_uri、state、code_challenge、code_challenge_method=S256。密碼提交一次，不跟隨重新導向；驗證 Location 的 state，再以 grant_type=authorization_code、code、client_id、原 redirect_uri、code_verifier 換權杖。重用未過期權杖；若回傳 refresh_token，僅存宿主/記憶體，過期以 grant_type=refresh_token、client_id、refresh_token 更新。憑證遺失時可用原授權密碼重新授權；保留用戶端行程和記憶體權杖供後續呼叫。密碼不是權杖。`,
    bearerReuse: `固定 Bearer 重用至重新產生/刪除，不需 OAuth；401 時檢查憑證。`,
    intro: "連接 MCP 伺服器並讀取專案根目錄：",
    url: "位址：",
    clientId: (id: string) => `Client ID：${id}（無 Secret）`,
    password: "授權密碼：",
    bearer: "認證標頭：",
    multi: `有目標 workspace_folder_id 就使用它；否則 conversation_bootstrap 回傳目錄選擇。`,
    fallback: `優先宿主設定；不支援則用 HTTP MCP（OAuth/PKCE 或 Bearer），帶 User-Agent（Cloudflare）。initialize → notifications/initialized（無 id）→ tools/list → list_workspace_folders 成功才確認連通。後續 MCP-Protocol-Version 使用協商版本。HTTP 連通不等於宿主註冊；受阻給出設定步驟。遠端 AI 用公開位址，憑證僅存宿主/記憶體，不寫檔案或回覆。若附有會話指令，立即執行。`,
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
  const lines: string[] = compact ? [`${text.url}${info.endpoint}`] : [text.intro, `${text.url}${info.endpoint}`];
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
