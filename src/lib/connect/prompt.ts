import type { Locale } from "$lib/i18n";

export type PromptTarget = "chatgpt" | "generic";

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

const TEXT = {
  en: {
    intro: (name: string) =>
      `Please connect to my local coding tools through MCP (Model Context Protocol) so you can read and edit my project "${name}" directly.`,
    info: "Connection details:",
    endpoint: "MCP server URL (Streamable HTTP)",
    folders: "Project folders on my computer",
    authOauth: "Authentication: OAuth 2.1 (authorization code + PKCE, no client secret)",
    clientId: "OAuth Client ID",
    password: "Authorization password (one-time, enter it on the authorization page)",
    authBearer: "Authentication: HTTP header",
    authNone: "Authentication: none",
    doTitle: "What I need you to do:",
    chatgptSteps: [
      "If you can add a remote MCP server / connector yourself, add it with the details above.",
      "Otherwise, guide me step by step. In ChatGPT: Settings → Apps & Connectors → Advanced settings → turn on Developer mode → Create connector. Paste the MCP server URL, choose OAuth, open the advanced OAuth settings, enter the Client ID, leave Client Secret empty, then click Connect and enter the authorization password on the page that opens.",
      "After connecting, list the available tools and read the project root to confirm everything works, then tell me the result.",
    ],
    genericSteps: [
      "If your platform or client can add a remote MCP server (Streamable HTTP), add it with the details above. The server supports automatic OAuth client registration, so most clients (Claude, Claude Code, Cursor, VS Code…) only need the URL; if a Client ID is requested, use the one above and leave Client Secret empty. When the authorization page opens, enter the authorization password.",
      "If I need to do something manually, tell me exactly where to click and what to fill in, one step at a time. For example, in Claude: Settings → Connectors → Add custom connector → paste the MCP server URL (optionally enter the Client ID under Advanced settings) → Connect → enter the authorization password.",
      "After connecting, list the available tools and read the project root to confirm everything works, then tell me the result.",
    ],
    oneTime:
      "Note: the authorization password works only once and is replaced automatically after a successful sign-in. If authorization fails, remind me to copy a fresh prompt from Coding Tools MCP.",
    temporary:
      "Note: this is a temporary Cloudflare address; it changes when the tunnel restarts. If the connection stops working, remind me to copy a fresh prompt.",
  },
  "zh-CN": {
    intro: (name: string) =>
      `请通过 MCP（Model Context Protocol）连接我电脑上的编程工具服务「Coding Tools MCP」，这样你就可以直接读取和修改我的项目「${name}」。`,
    info: "连接信息：",
    endpoint: "MCP 服务地址（Streamable HTTP）",
    folders: "我电脑上的项目文件夹",
    authOauth: "认证方式：OAuth 2.1（授权码 + PKCE，无需 Client Secret）",
    clientId: "OAuth Client ID",
    password: "授权密码（一次性，在授权页面输入）",
    authBearer: "认证方式：HTTP 请求头",
    authNone: "认证方式：无",
    doTitle: "请你这样做：",
    chatgptSteps: [
      "如果你可以自己添加远程 MCP 服务器 / 连接器，请直接用上面的信息完成添加。",
      "如果需要我手动操作，请一步一步告诉我。在 ChatGPT 中：设置 → 应用与连接器 → 高级设置 → 打开「开发者模式」→ 创建连接器；填入 MCP 服务地址，认证选择 OAuth，展开高级 OAuth 设置填入 Client ID，Client Secret 留空；点击连接，在弹出的授权页面输入授权密码。",
      "连接成功后，先列出可用工具，再读取项目根目录确认连接正常，然后告诉我结果。",
    ],
    genericSteps: [
      "如果你的平台或客户端支持添加远程 MCP 服务器（Streamable HTTP），请直接用上面的信息完成添加。服务器支持 OAuth 客户端自动注册，大多数客户端（Claude、Claude Code、Cursor、VS Code 等）只需填写服务地址；如果要求填写 Client ID，就用上面的 Client ID，Client Secret 留空。弹出授权页面时输入授权密码。",
      "如果需要我手动操作，请一步一步告诉我在哪里点、填什么。例如在 Claude 中：设置 → 连接器 → 添加自定义连接器 → 填入 MCP 服务地址（可在高级设置里填入 Client ID）→ 连接 → 输入授权密码。",
      "连接成功后，先列出可用工具，再读取项目根目录确认连接正常，然后告诉我结果。",
    ],
    oneTime:
      "注意：授权密码只能使用一次，授权成功后会自动更换。如果授权失败，请提醒我回到 Coding Tools MCP 重新复制提示词。",
    temporary:
      "注意：这是 Cloudflare 临时地址，隧道重启后会变化。如果连接失效，请提醒我重新复制提示词。",
  },
  "zh-TW": {
    intro: (name: string) =>
      `請透過 MCP（Model Context Protocol）連接我電腦上的程式工具服務「Coding Tools MCP」，這樣你就可以直接讀取和修改我的專案「${name}」。`,
    info: "連線資訊：",
    endpoint: "MCP 服務位址（Streamable HTTP）",
    folders: "我電腦上的專案資料夾",
    authOauth: "認證方式：OAuth 2.1（授權碼 + PKCE，不需要 Client Secret）",
    clientId: "OAuth Client ID",
    password: "授權密碼（一次性，在授權頁面輸入）",
    authBearer: "認證方式：HTTP 標頭",
    authNone: "認證方式：無",
    doTitle: "請你這樣做：",
    chatgptSteps: [
      "如果你可以自己新增遠端 MCP 伺服器 / 連接器，請直接用上面的資訊完成新增。",
      "如果需要我手動操作，請一步一步告訴我。在 ChatGPT 中：設定 → 應用程式與連接器 → 進階設定 → 開啟「開發者模式」→ 建立連接器；填入 MCP 服務位址，認證選擇 OAuth，展開進階 OAuth 設定填入 Client ID，Client Secret 留空；點擊連接，在彈出的授權頁面輸入授權密碼。",
      "連線成功後，先列出可用工具，再讀取專案根目錄確認連線正常，然後告訴我結果。",
    ],
    genericSteps: [
      "如果你的平台或用戶端支援新增遠端 MCP 伺服器（Streamable HTTP），請直接用上面的資訊完成新增。伺服器支援 OAuth 用戶端自動註冊，大多數用戶端（Claude、Claude Code、Cursor、VS Code 等）只需填寫服務位址；如果要求填寫 Client ID，就用上面的 Client ID，Client Secret 留空。彈出授權頁面時輸入授權密碼。",
      "如果需要我手動操作，請一步一步告訴我在哪裡點、填什麼。例如在 Claude 中：設定 → 連接器 → 新增自訂連接器 → 填入 MCP 服務位址（可在進階設定裡填入 Client ID）→ 連接 → 輸入授權密碼。",
      "連線成功後，先列出可用工具，再讀取專案根目錄確認連線正常，然後告訴我結果。",
    ],
    oneTime:
      "注意：授權密碼只能使用一次，授權成功後會自動更換。如果授權失敗，請提醒我回到 Coding Tools MCP 重新複製提示詞。",
    temporary:
      "注意：這是 Cloudflare 臨時位址，通道重啟後會變化。如果連線失效，請提醒我重新複製提示詞。",
  },
} as const;

export function isTemporaryEndpoint(endpoint: string): boolean {
  try {
    return new URL(endpoint).hostname.endsWith(".trycloudflare.com");
  } catch {
    return false;
  }
}

/** Build a copy-paste prompt any AI assistant can follow to connect to this MCP server. */
export function buildConnectionPrompt(info: ConnectionInfo, target: PromptTarget, locale: Locale): string {
  const text = TEXT[promptLanguage(locale)];
  const lines: string[] = [text.intro(info.workspaceName), "", text.info];
  lines.push(`- ${text.endpoint}: ${info.endpoint}`);

  if (info.authType === "oauth") {
    lines.push(`- ${text.authOauth}`);
    lines.push(`- ${text.clientId}: ${info.clientId}`);
    lines.push(`- ${text.password}: ${info.password}`);
  } else if (info.authType === "bearer") {
    lines.push(`- ${text.authBearer}: Authorization: Bearer ${info.bearerToken}`);
  } else {
    lines.push(`- ${text.authNone}`);
  }

  if (info.folders.length > 0) {
    lines.push(`- ${text.folders}: ${info.folders.join(", ")}`);
  }

  lines.push("", text.doTitle);
  const steps = target === "chatgpt" ? text.chatgptSteps : text.genericSteps;
  steps.forEach((step, index) => lines.push(`${index + 1}. ${step}`));

  const notes: string[] = [];
  if (info.authType === "oauth") notes.push(text.oneTime);
  if (isTemporaryEndpoint(info.endpoint)) notes.push(text.temporary);
  if (notes.length > 0) lines.push("", ...notes);

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
