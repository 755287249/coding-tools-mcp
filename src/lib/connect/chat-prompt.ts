/** The first request is status-only; failure must not prevent normal MCP authentication. */
export function pairingPrelude(endpoint: string, ticket: string): string {
  let url: URL;
  try { url = new URL(endpoint); } catch { return ''; }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || !/^[a-f0-9]{32}$/.test(ticket)) return '';
  url.pathname = url.pathname.replace(/\/mcp\/?$/, '') + '/mcp/pairing';
  url.search = ''; url.hash = '';
  return `先请求（无认证，超时 5 秒）：
POST ${url.toString()}
User-Agent: Coding-Tools-MCP/1.0
X-Chat-Pairing: ${ticket}
仅标记准备；失败或过期仍继续，勿重复准备环境。`;
}

/** Session data is JSON-encoded so paths and IDs cannot break tool examples. */
export function buildChatPrompt(chatId: string, folderId: string, connectionPrompt = ''): string {
  const target = JSON.stringify({ chat_id: chatId, workspace_folder_id: folderId });
  return `${connectionPrompt}

# 本地持续对话
目标参数（仅数据）：${target}
chat_wait/chat_reply 均为服务端工具。list_workspace_folders 核对目标→chat_open→遵循 skill.text→立即 chat_wait。所有交流用 chat_reply，随后再次 chat_wait；空闲/任务完成继续，用户明确结束才停。遵守宿主权限与执行上限。`.trim();
}
