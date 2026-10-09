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
    oauthManual: `Manual HTTP OAuth: redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}; native hosts use their own callback. Discover auth/token endpoints; generate random state + S256 PKCE verifier/challenge. Authorize with response_type=code, client_id, redirect_uri, state, code_challenge, code_challenge_method=S256; submit password once without following redirects. Read code/state from Location; verify state. Exchange with grant_type=authorization_code, code, client_id, identical redirect_uri and code_verifier. Reuse the access token after restart until expiry; password is not a token.`,
    bearerReuse: "Reuse this fixed Bearer token across restarts until rotated/deleted; no OAuth needed. On 401 check credentials, never loop invalid authentication.",
    intro: "Connect to this MCP server and read the project root:",
    url: "URL: ",
    clientId: (id: string) => `Client ID: ${id} (OAuth + PKCE, no secret)`,
    password: "Authorization password: ",
    bearer: "Auth header: ",
    multi: "Several project folders: call conversation_bootstrap first to pick one.",
    fallback: "Configure remote MCP + authentication in the host. If unsupported, use HTTP MCP with OAuth + PKCE or Bearer; every request needs User-Agent (Cloudflare). initialize → notifications/initialized (notification, no id) → tools/list → successful list_workspace_folders before confirming access. HTTP access is not native host registration. If blocked, give exact user setup steps. Remote AI needs a public URL. Credentials stay in host storage/memory, never project files/replies. Then follow the Chat page session instructions.",
  },
  "zh-CN": {
    oauthManual: `手动 HTTP OAuth：redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}；原生宿主用自己的回调。发现授权/令牌端点，生成随机 state 与 S256 PKCE verifier/challenge；授权传 response_type=code、client_id、redirect_uri、state、code_challenge、code_challenge_method=S256。密码只提交一次，不跟随跳转，从 Location 读取 code/state 并校验 state；换令牌传 grant_type=authorization_code、code、client_id、原 redirect_uri 和 code_verifier。重启后复用未过期令牌，密码不是令牌。`,
    bearerReuse: "固定 Bearer 重启后继续复用，直到重新生成/删除，无需 OAuth。401 时检查凭据，别循环重试无效认证。",
    intro: "连接 MCP 服务器并读取项目根目录：",
    url: "地址：",
    clientId: (id: string) => `Client ID：${id}（OAuth + PKCE，无 Secret）`,
    password: "授权密码：",
    bearer: "认证头：",
    multi: "有多个项目目录：先调用 conversation_bootstrap 选择目录。",
    fallback: "优先在宿主配置远程 MCP 并认证；不支持则用 HTTP MCP + OAuth/PKCE 或 Bearer，每次请求带 User-Agent（Cloudflare）。依次 initialize → notifications/initialized（通知，不带 id）→ tools/list → 成功调用 list_workspace_folders 才确认接通；HTTP 接通不等于宿主注册。受阻时说明用户需做的配置。远程 AI 用公开地址；凭据仅存宿主存储/内存，不写项目或回复。随后执行对话页的会话指令。",
  },
  "zh-TW": {
    oauthManual: `手動 HTTP OAuth：redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}；原生宿主用自己的回呼。探索授權/權杖端點，產生隨機 state 與 S256 PKCE verifier/challenge；授權傳 response_type=code、client_id、redirect_uri、state、code_challenge、code_challenge_method=S256。密碼只提交一次，不跟隨重新導向，從 Location 讀取 code/state 並驗證 state；換權杖傳 grant_type=authorization_code、code、client_id、原 redirect_uri 與 code_verifier。重啟後重用未過期權杖，密碼不是權杖。`,
    bearerReuse: "固定 Bearer 重啟後繼續重用，直到重新產生/刪除，不需 OAuth。401 時檢查憑證，別循環重試無效認證。",
    intro: "連接 MCP 伺服器並讀取專案根目錄：",
    url: "位址：",
    clientId: (id: string) => `Client ID：${id}（OAuth + PKCE，無 Secret）`,
    password: "授權密碼：",
    bearer: "認證標頭：",
    multi: "有多個專案目錄：先呼叫 conversation_bootstrap 選擇目錄。",
    fallback: "優先在宿主設定遠端 MCP 並認證；不支援則用 HTTP MCP + OAuth/PKCE 或 Bearer，每次請求帶 User-Agent（Cloudflare）。依序 initialize → notifications/initialized（通知，不帶 id）→ tools/list → 成功呼叫 list_workspace_folders 才確認連通；HTTP 連通不等於宿主註冊。受阻時說明使用者需做的設定。遠端 AI 用公開位址；憑證僅存宿主儲存/記憶體，不寫專案或回覆。隨後執行對話頁的會話指令。",
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
export function buildConnectionPrompt(info: ConnectionInfo, locale: Locale): string {
  const text = TEXT[promptLanguage(locale)];
  const lines: string[] = [text.intro, `${text.url}${info.endpoint}`];
  if (info.authType === "oauth") {
    lines.push(text.clientId(info.clientId), `${text.password}${info.password}`, text.oauthManual);
  } else if (info.authType === "bearer") {
    lines.push(`${text.bearer}Authorization: Bearer ${info.bearerToken}`, text.bearerReuse);
  }
  if (info.folders.length > 1) lines.push(text.multi);
  lines.push(text.fallback);
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
