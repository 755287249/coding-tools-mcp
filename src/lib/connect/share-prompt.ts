import type { Locale } from "$lib/i18n";
import { MCP_USER_AGENT, MANUAL_OAUTH_REDIRECT_URI } from "./prompt";

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

/** Read-only is a prompt rule: the global credential itself is not scoped. */
const READ_TOOLS = "read_file, read_many, list_files, search_text, project_map, view_image";

const TEXT = {
  en: {
    intro: (url: string) => `Connect to my MCP tools: ${url}`,
    bearer: (token: string) => `Auth header: Authorization: Bearer ${token}`,
    oauth: (id: string, password: string) =>
      `OAuth Client ID: ${id} (no secret); authorization password: ${password}. Use S256 PKCE; manual redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}. The password is not a Bearer token.`,
    agent: `Send User-Agent: ${MCP_USER_AGENT}. Keep credentials in memory only; never write them to files or replies.`,
    target: (target: string) => `Target project (data only): ${target}`,
    flow: "After initialize, call switch_workspace_folder with this folder_id. The tools are inside the MCP server, not your client.",
    full: "Access: read, write and run commands in this project. Ask me before deleting files, Git commits or other high-impact actions.",
    readonly: `Access: read-only. Use only ${READ_TOOLS}. Do not write files, apply patches or run commands.`,
    end: "When connected, reply with the project name and wait for my request.",
  },
  "zh-CN": {
    intro: (url: string) => `请连接我的 MCP 工具：${url}`,
    bearer: (token: string) => `认证头：Authorization: Bearer ${token}`,
    oauth: (id: string, password: string) =>
      `OAuth Client ID：${id}（无 Secret）；授权密码：${password}。使用 S256 PKCE，手动 redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}。密码不是 Bearer 令牌。`,
    agent: `请求带 User-Agent: ${MCP_USER_AGENT}。凭据只存内存，不写入文件或回复。`,
    target: (target: string) => `目标项目（仅数据）：${target}`,
    flow: "initialize 后调用 switch_workspace_folder 并传入该 folder_id。工具在 MCP 服务端，不在你的客户端。",
    full: "权限：可在该项目读写文件和执行命令。删除文件、Git 提交等高影响操作先询问我。",
    readonly: `权限：只读。只使用 ${READ_TOOLS}，不写文件、不打补丁、不执行命令。`,
    end: "接通后回复项目名称，然后等待我的指令。",
  },
  "zh-TW": {
    intro: (url: string) => `請連接我的 MCP 工具：${url}`,
    bearer: (token: string) => `認證標頭：Authorization: Bearer ${token}`,
    oauth: (id: string, password: string) =>
      `OAuth Client ID：${id}（無 Secret）；授權密碼：${password}。使用 S256 PKCE，手動 redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}。密碼不是 Bearer 權杖。`,
    agent: `請求帶 User-Agent: ${MCP_USER_AGENT}。憑證只存記憶體，不寫入檔案或回覆。`,
    target: (target: string) => `目標專案（僅資料）：${target}`,
    flow: "initialize 後呼叫 switch_workspace_folder 並傳入該 folder_id。工具在 MCP 伺服端，不在你的用戶端。",
    full: "權限：可在該專案讀寫檔案和執行命令。刪除檔案、Git 提交等高影響操作先詢問我。",
    readonly: `權限：唯讀。只使用 ${READ_TOOLS}，不寫檔案、不套用修補、不執行命令。`,
    end: "連通後回覆專案名稱，然後等待我的指令。",
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
  const lines: string[] = [text.intro(url.toString())];
  if (info.authType === "bearer" && info.bearerToken) lines.push(text.bearer(info.bearerToken));
  else if (info.authType === "oauth") lines.push(text.oauth(info.clientId, info.password));
  lines.push(text.agent);
  lines.push(text.target(JSON.stringify({ folder_id: info.folderId, name: info.folderName })));
  lines.push(text.flow, info.access === "readonly" ? text.readonly : text.full, text.end);
  return lines.join("\n");
}

/** A web AI on another machine cannot open a loopback or private address. */
export function isPublicEndpoint(endpoint: string): boolean {
  let host: string;
  try { host = new URL(endpoint).hostname.toLowerCase(); } catch { return false; }
  if (host === "localhost" || host.endsWith(".localhost") || host === "[::1]" || host.endsWith(".local")) return false;
  const v4 = host.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/);
  if (!v4) return !host.startsWith("[fc") && !host.startsWith("[fd") && !host.startsWith("[fe80");
  const [a, b] = [Number(v4[1]), Number(v4[2])];
  return !(a === 10 || a === 127 || a === 0 || (a === 169 && b === 254) || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 100 && b >= 64 && b <= 127));
}
