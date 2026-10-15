import type { Locale } from "$lib/i18n";
import { MANUAL_OAUTH_REDIRECT_URI } from "./prompt";

export type ShareAccess = "full" | "readonly";

export interface ShareInfo {
  endpoint: string;
  authType: string;
  clientId: string;
  password: string;
  bearerToken: string;
  folderId: string;
  folderName: string;
  access: ShareAccess;
}

/**
 * Same style as the chat seed prompt: one short paragraph, connect first.
 * The server's initialize instructions carry the detailed rules.
 */
const TEXT = {
  en: {
    intro: (url: string) => `Connect to my tools ${url} .`,
    bearer: (token: string) => `Header: Authorization: Bearer ${token}.`,
    oauth: (id: string, password: string) => `OAuth: Client ID ${id}, password ${password}, S256 PKCE, redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}.`,
    body: (args: string) => `Quickly connect to this sandbox's MCP (URL), then call switch_workspace_folder ${args}. Learn the rules and the available tools, and get ready for the next operations. The tools are inside the MCP, not in your client. Use them once connected, then wait for my message.`,
    readonly: "Read-only: only read and search files.",
  },
  "zh-CN": {
    intro: (url: string) => `请连接我的工具 ${url} 。`,
    bearer: (token: string) => `请求头 Authorization: Bearer ${token} 。`,
    oauth: (id: string, password: string) => `OAuth：Client ID ${id}，密码 ${password}，S256 PKCE，redirect_uri=${MANUAL_OAUTH_REDIRECT_URI} 。`,
    body: (args: string) => `快速连接这个 sandbox 的 MCP（URL），连上后调用 switch_workspace_folder ${args}，明确使用规则，熟悉可用工具，做好处理接下来一系列操作的准备。工具在 MCP 内部，而非你的客户端内部，连上后使用，然后等待信息。`,
    readonly: "只读：只读取和搜索文件。",
  },
  "zh-TW": {
    intro: (url: string) => `請連接我的工具 ${url} 。`,
    bearer: (token: string) => `請求標頭 Authorization: Bearer ${token} 。`,
    oauth: (id: string, password: string) => `OAuth：Client ID ${id}，密碼 ${password}，S256 PKCE，redirect_uri=${MANUAL_OAUTH_REDIRECT_URI} 。`,
    body: (args: string) => `快速連接這個 sandbox 的 MCP（URL），連上後呼叫 switch_workspace_folder ${args}，明確使用規則，熟悉可用工具，做好處理接下來一系列操作的準備。工具在 MCP 內部，而非你的用戶端內部，連上後使用，然後等待訊息。`,
    readonly: "唯讀：只讀取和搜尋檔案。",
  },
} as const;

function language(locale: Locale): keyof typeof TEXT {
  return locale === "zh-CN" || locale === "zh-TW" ? locale : "en";
}

/** Short prompt that lets an external web AI use one project without joining a chat. */
export function buildSharePrompt(info: ShareInfo, locale: Locale): string {
  let url: URL;
  try { url = new URL(info.endpoint); } catch { throw Error("Invalid MCP endpoint"); }
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) throw Error("Invalid MCP endpoint");
  if (!info.folderId) throw Error("Missing project folder");
  const text = TEXT[language(locale)];
  const parts: string[] = [text.intro(url.toString())];
  if (info.authType === "bearer" && info.bearerToken) parts.push(text.bearer(info.bearerToken));
  else if (info.authType === "oauth") parts.push(text.oauth(info.clientId, info.password));
  parts.push(text.body(JSON.stringify({ folder_id: info.folderId })));
  if (info.access === "readonly") parts.push(text.readonly);
  return parts.join(language(locale) === "en" ? " " : "");
}

/** A web AI on another machine cannot open a loopback or private address. */
export function isPublicEndpoint(endpoint: string): boolean {
  let host: string;
  try { host = new URL(endpoint).hostname.toLowerCase(); } catch { return false; }
  if (host === "localhost" || host.endsWith(".localhost") || host.endsWith(".local")) return false;
  if (host.startsWith("[")) {
    // Conservative: only global unicast IPv6 (2000::/3) is public; ::, ::1, mapped IPv4, ULA and link-local are not.
    return /^\[[23][0-9a-f]{0,3}:/.test(host);
  }
  const v4 = host.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/);
  if (!v4) return true;
  const [a, b] = [Number(v4[1]), Number(v4[2])];
  return !(a === 10 || a === 127 || a === 0 || (a === 169 && b === 254) || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 100 && b >= 64 && b <= 127));
}
