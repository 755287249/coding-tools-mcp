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
    oauthManual: `Manual HTTP OAuth only: redirect_uri=${MANUAL_OAUTH_REDIRECT_URI} (server-supported loopback callback, no client registration needed). Generate random state and an S256 PKCE verifier/challenge; send response_type=code, client_id, redirect_uri, state, code_challenge and code_challenge_method=S256 to the discovered authorization endpoint. Submit the password once; disable redirect following, read code/state from Location and verify state. Exchange code with the identical redirect_uri and verifier at the token endpoint. Native MCP hosts use their own callback. Keep the access token in host credential storage or process memory; reuse it after disconnect/restart until expiry. The one-time password is not the access token; do not reauthorize on every network retry.`,
    bearerReuse: "This fixed Bearer token stays valid across client restarts until you regenerate/delete it. Send it directly on MCP requests; no OAuth callback or authorization password is needed. Keep it in host credential storage or process memory, never project files or replies. A 401 requires checking the token/configuration, not endlessly repeating invalid credentials.",
    intro: "Connect to this MCP server and read the project root:",
    url: "URL: ",
    clientId: (id: string) => `Client ID: ${id} (OAuth + PKCE, no secret)`,
    password: "Authorization password: ",
    bearer: "Auth header: ",
    multi: "Several project folders: call conversation_bootstrap first to pick one.",
    fallback: "Configure this URL as a remote MCP server in your host and complete its authentication flow. If the host cannot add servers or authorize OAuth automatically, explain the exact user action needed. If the platform does not support MCP registration, connect manually over HTTP using the MCP protocol and include a User-Agent header on every request (Cloudflare may block requests without it). Complete the same OAuth + PKCE or Bearer authentication, initialize, list tools and successfully call list_workspace_folders before claiming protocol access; HTTP transport is not host-native MCP registration. If neither route is available, explain the required user setup. Local URLs require a host on the same machine; remote hosts need the published endpoint. Keep credentials out of project files and replies. For local chat, follow the session-specific instructions copied from the Chat page after connecting.",
  },
  "zh-CN": {
    oauthManual: `仅 HTTP 手动 OAuth：redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}（服务端允许的本机回调，无需注册新客户端）。生成随机 state 和 S256 PKCE verifier/challenge，向发现的授权端点传 response_type=code、client_id、redirect_uri、state、code_challenge、code_challenge_method=S256。提交一次授权密码；禁止自动跟随跳转，从 Location 读取 code/state 并校验 state，再用完全相同的 redirect_uri 和 verifier 换令牌。原生 MCP 宿主使用自己的回调。访问令牌保存在宿主凭据存储或进程内存，断线/重启后优先复用至到期；一次性密码不是访问令牌，不要每次网络重试都重新授权。`,
    bearerReuse: "此固定 Bearer Token 在客户端重启后仍可复用，直到你重新生成或删除它。MCP 请求直接携带此头，无需 OAuth 回调或授权密码。仅保存在宿主凭据存储或进程内存，不写入项目文件或回复。401 时检查凭据/配置，不无限重试已失效凭据。",
    intro: "连接 MCP 服务器并读取项目根目录：",
    url: "地址：",
    clientId: (id: string) => `Client ID：${id}（OAuth + PKCE，无 Secret）`,
    password: "授权密码：",
    bearer: "认证头：",
    multi: "有多个项目目录：先调用 conversation_bootstrap 选择目录。",
    fallback: "在宿主中将此地址配置为远程 MCP 服务并完成认证；若无法自动添加服务或完成 OAuth，明确说明用户需要完成的步骤。平台不支持 MCP 就用 HTTP 手动连接，请求需带 User-Agent 头（否则可能被 Cloudflare 拦截）。HTTP 连接也要完成 OAuth + PKCE 或 Bearer 认证、MCP 初始化、工具发现，并成功调用 list_workspace_folders；不能把 HTTP 协议接通说成宿主已注册 MCP。两种方式都不可用时，说明用户需要完成的配置步骤。本地地址仅供同机宿主使用，远程 AI 需要可访问的公开地址。认证信息不能写入项目文件或回复。需要本地持续聊天时，连接后执行从对话页复制的会话接入指令。",
  },
  "zh-TW": {
    oauthManual: `僅 HTTP 手動 OAuth：redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}（伺服器允許的本機回呼，不需註冊新用戶端）。產生隨機 state 與 S256 PKCE verifier/challenge，向探索到的授權端點傳 response_type=code、client_id、redirect_uri、state、code_challenge、code_challenge_method=S256。提交一次授權密碼；禁止自動跟隨重新導向，從 Location 讀取 code/state 並驗證 state，再使用完全相同的 redirect_uri 與 verifier 換取權杖。原生 MCP 宿主使用自己的回呼。存取權杖保存在宿主憑證儲存或程序記憶體，斷線/重啟後優先重用至到期；一次性密碼不是存取權杖，不要每次網路重試都重新授權。`,
    bearerReuse: "此固定 Bearer Token 在用戶端重啟後仍可重用，直到你重新產生或刪除它。MCP 請求直接攜帶此標頭，不需 OAuth 回呼或授權密碼。僅保存在宿主憑證儲存或程序記憶體，不寫入專案檔案或回覆。401 時檢查憑證/設定，不無限重試已失效憑證。",
    intro: "連接 MCP 伺服器並讀取專案根目錄：",
    url: "位址：",
    clientId: (id: string) => `Client ID：${id}（OAuth + PKCE，無 Secret）`,
    password: "授權密碼：",
    bearer: "認證標頭：",
    multi: "有多個專案目錄：先呼叫 conversation_bootstrap 選擇目錄。",
    fallback: "在宿主中將此位址設定為遠端 MCP 服務並完成認證；若無法自動新增服務或完成 OAuth，明確說明使用者需要完成的步驟。平台不支援 MCP 時改用 HTTP 手動連線，每個請求須帶 User-Agent 標頭（否則可能被 Cloudflare 攔截）。HTTP 連線也須完成 OAuth + PKCE 或 Bearer 認證、MCP 初始化、工具探索並成功呼叫 list_workspace_folders；不能將 HTTP 協定連通稱為宿主已註冊 MCP。兩種方式都不可用時，說明使用者需完成的設定步驟。本機位址僅供同機宿主使用，遠端 AI 需要可存取的公開位址。認證資訊不能寫入專案檔案或回覆。需要本機持續聊天時，連線後執行從對話頁複製的會話接入指令。",
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
