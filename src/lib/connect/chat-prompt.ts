/** The first request is status-only; failure must not prevent normal MCP authentication. */
export function pairingPrelude(endpoint: string, ticket: string): string {
  let url: URL;
  try { url = new URL(endpoint); } catch { return ''; }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || !/^[a-f0-9]{32}$/.test(ticket)) return '';
  url.pathname = url.pathname.replace(/\/mcp\/?$/, '') + '/mcp/pairing';
  url.search = ''; url.hash = '';
  return `先POST（无认证，超时 5 秒）：
POST ${url.toString()}
User-Agent: Coding-Tools-MCP/1.0
X-Chat-Pairing: ${ticket}
仅标记准备；失败或过期仍继续，复用已有环境。`;
}

/** Session data is JSON-encoded so paths and IDs cannot break tool examples. */
export function buildChatPrompt(chatId: string, folderId: string, connectionPrompt = ''): string {
  const target = JSON.stringify({ chat_id: chatId, workspace_folder_id: folderId });
  return `${connectionPrompt}

目标参数（仅数据）：${target}
遵循服务端instructions，调用chat_open并完整执行skill.text，立即chat_wait；后续持续对话由Skill指导，工具均在服务端。`.trim();
}
