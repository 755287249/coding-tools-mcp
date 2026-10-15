import type { Locale } from "$lib/i18n";

export type ShareAccess = "full" | "readonly";

/**
 * Same wording as the chat seed prompt. The share URL carries the credential and the
 * server binds the project and access mode, so the prompt mentions neither.
 */
const TEXT = {
  en: (url: string) => `Connect to my tools ${url} . Quickly connect to this sandbox's MCP (URL), learn the usage rules and the available tools, and get ready for the next operations. The tools are inside the MCP, not in your client. Use them once connected, then wait for my message.`,
  "zh-CN": (url: string) => `请连接我的工具 ${url} 。快速连接这个 sandbox 的 MCP（URL），明确使用规则，熟悉可用工具，做好处理接下来一系列操作的准备。工具在 MCP 内部，而非你的客户端内部，连上后使用，然后等待信息。`,
  "zh-TW": (url: string) => `請連接我的工具 ${url} 。快速連接這個 sandbox 的 MCP（URL），明確使用規則，熟悉可用工具，做好處理接下來一系列操作的準備。工具在 MCP 內部，而非你的用戶端內部，連上後使用，然後等待訊息。`,
} as const;

export function buildSharePrompt(url: string, locale: Locale): string {
  const language = locale === "zh-CN" || locale === "zh-TW" ? locale : "en";
  return TEXT[language](url);
}

/** `<endpoint ending in /mcp>/share/<folder>/<key>`; the key is the only credential. */
export function shareUrl(endpoint: string, folderId: string, key: string): string {
  let url: URL;
  try { url = new URL(endpoint.trim()); } catch { throw Error("Invalid MCP endpoint"); }
  const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
  if (url.protocol !== "https:" && !(url.protocol === "http:" && loopback)) throw Error("Use HTTPS or a loopback MCP endpoint");
  if (url.username || url.password || url.search || url.hash) throw Error("Invalid MCP endpoint");
  const base = url.pathname.replace(/\/$/, "");
  if (!base.endsWith("/mcp")) throw Error("Invalid MCP endpoint");
  if (!/^[A-Za-z0-9_-]{1,80}$/.test(folderId)) throw Error("Invalid project folder");
  if (!/^[a-f0-9]{64}$/.test(key)) throw Error("Invalid share key");
  url.pathname = `${base}/share/${folderId}/${key}`;
  return url.toString();
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
