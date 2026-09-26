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

type PromptLanguage = "en" | "zh-CN" | "zh-TW";

function promptLanguage(locale: Locale): PromptLanguage {
  if (locale === "zh-CN" || locale === "zh-TW") return locale;
  return "en";
}

/** Deliberately minimal: tested to work with ChatGPT, Claude and generic agents. */
const TEXT = {
  en: {
    intro: "Connect to this MCP server and read the project root:",
    url: "URL: ",
    clientId: (id: string) => `Client ID: ${id} (OAuth + PKCE, no secret)`,
    password: "Authorization password: ",
    bearer: "Auth header: ",
    multi: "Several project folders: call conversation_bootstrap first to pick one.",
    fallback: "If your platform lacks MCP support, connect manually over HTTP and send a User-Agent header (Cloudflare may block default script clients).",
  },
  "zh-CN": {
    intro: "连接 MCP 服务器并读取项目根目录：",
    url: "地址：",
    clientId: (id: string) => `Client ID：${id}（OAuth + PKCE，无 Secret）`,
    password: "授权密码：",
    bearer: "认证头：",
    multi: "有多个项目目录：先调用 conversation_bootstrap 选择目录。",
    fallback: "平台不支持 MCP 就用 HTTP 手动连接，请求需带 User-Agent 头（否则可能被 Cloudflare 拦截）。",
  },
  "zh-TW": {
    intro: "連接 MCP 伺服器並讀取專案根目錄：",
    url: "位址：",
    clientId: (id: string) => `Client ID：${id}（OAuth + PKCE，無 Secret）`,
    password: "授權密碼：",
    bearer: "認證標頭：",
    multi: "有多個專案目錄：先呼叫 conversation_bootstrap 選擇目錄。",
    fallback: "平台不支援 MCP 就用 HTTP 手動連接，請求需帶 User-Agent 標頭（否則可能被 Cloudflare 攔截）。",
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
    lines.push(text.clientId(info.clientId), `${text.password}${info.password}`);
  } else if (info.authType === "bearer") {
    lines.push(`${text.bearer}Authorization: Bearer ${info.bearerToken}`);
  }
  if (info.folders.length > 1) lines.push(text.multi);
  lines.push(text.fallback);
  return lines.join("\n");
}

/** JSON config for MCP clients that accept a remote URL with static headers. */
export function buildClientConfigJson(info: ConnectionInfo): string {
  const server: Record<string, unknown> = { type: "http", url: info.endpoint };
  if (info.authType === "bearer" && info.bearerToken) {
    server.headers = { Authorization: `Bearer ${info.bearerToken}` };
  }
  return JSON.stringify({ mcpServers: { "coding-tools": server } }, null, 2);
}
