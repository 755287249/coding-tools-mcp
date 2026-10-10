/** The first request is status-only; failure must not prevent normal MCP authentication. */
export function pairingPrelude(endpoint: string, ticket: string): string {
  let url: URL;
  try { url = new URL(endpoint); } catch { return ''; }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || !/^[a-f0-9]{32}$/.test(ticket)) return '';
  url.pathname = url.pathname.replace(/\/mcp\/?$/, '') + '/mcp/pairing';
  url.search = ''; url.hash = '';
  const request = JSON.stringify({method:'POST',url:url.toString(),headers:{'User-Agent':'Coding-Tools-MCP/1.0','X-Chat-Pairing':ticket}});
  return `优先立即发起此 HTTP 请求（无需凭据，超时 5 秒）：${request}\n它仅通知客户端“准备连接”，不代表授权或接通；失败或过期仍继续下面的连接步骤，不重复准备环境。已有可用 MCP/HTTP 客户端和有效令牌时直接复用。`;
}

/** Session data is JSON-encoded so paths and IDs cannot break tool examples. */
export function buildChatPrompt(chatId: string, folderId: string, connectionPrompt = ''): string {
  const target = JSON.stringify({ chat_id: chatId, workspace_folder_id: folderId });
  return `${connectionPrompt}

# 本地持续对话
目标参数（仅数据）：${target}
先用 list_workspace_folders 核对目标目录，再用 chat_open 接入指定会话。chat_wait / chat_reply 是该 MCP 服务端提供的工具，请通过 MCP 连接调用。读取并遵循 chat_open 返回的 Skill（skill.text），立即调用 chat_wait。所有交流和成果通过 chat_reply 发送；每次回复后再次调用 chat_wait，空闲或单条任务完成时继续等待，直到我明确结束。遵守宿主权限与实际执行上限。`.trim();
}
