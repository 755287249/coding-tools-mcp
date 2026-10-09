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

/** Host setup and a successful tool call are separate connection requirements. */
const TEXT = {
  en: {
    intro: "Connect to this MCP server and read the project root:",
    url: "URL: ",
    clientId: (id: string) => `Client ID: ${id} (OAuth + PKCE, no secret)`,
    password: "Authorization password: ",
    bearer: "Auth header: ",
    multi: "Several project folders: call conversation_bootstrap first to pick one.",
    fallback: "Configure this URL as a remote MCP server in your host and complete its authentication flow. If the host cannot add servers or authorize OAuth automatically, explain the exact user action needed. Refresh tools and call list_workspace_folders to verify access before claiming success. Local URLs require a host on the same machine; remote hosts need the published endpoint. Keep credentials out of project files and replies. For local chat, follow the session-specific instructions copied from the Chat page after connecting.",
  },
  "zh-CN": {
    intro: "连接 MCP 服务器并读取项目根目录：",
    url: "地址：",
    clientId: (id: string) => `Client ID：${id}（OAuth + PKCE，无 Secret）`,
    password: "授权密码：",
    bearer: "认证头：",
    multi: "有多个项目目录：先调用 conversation_bootstrap 选择目录。",
    fallback: "在宿主中将此地址配置为远程 MCP 服务并完成认证；若无法自动添加服务或完成 OAuth，明确说明用户需要完成的步骤。刷新工具并成功调用 list_workspace_folders 后才能确认连接成功。本地地址仅供同机宿主使用，远程 AI 需要可访问的公开地址。认证信息不能写入项目文件或回复。需要本地持续聊天时，连接后执行从对话页复制的会话接入指令。",
  },
  "zh-TW": {
    intro: "連接 MCP 伺服器並讀取專案根目錄：",
    url: "位址：",
    clientId: (id: string) => `Client ID：${id}（OAuth + PKCE，無 Secret）`,
    password: "授權密碼：",
    bearer: "認證標頭：",
    multi: "有多個專案目錄：先呼叫 conversation_bootstrap 選擇目錄。",
    fallback: "在宿主中將此位址設定為遠端 MCP 服務並完成認證；若無法自動新增服務或完成 OAuth，明確說明使用者需要完成的步驟。重新整理工具並成功呼叫 list_workspace_folders 後才能確認連線成功。本機位址僅供同機宿主使用，遠端 AI 需要可存取的公開位址。認證資訊不能寫入專案檔案或回覆。需要本機持續聊天時，連線後執行從對話頁複製的會話接入指令。",
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
