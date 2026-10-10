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
list_workspace_folders 核对目录后 chat_open（agent_name 填你的简短名称，勿含空格或 @；恢复用已有 attachment_id），保存实际 attachment_id，先读并遵循返回的「本地持续对话 Skill」（skill.text）。所有交流和成果通过 MCP；持续 chat_wait → 工作 → chat_reply → 再等待，idle 或任务完成不终止，不计等待成本。用户明确结束才关闭，遵守宿主权限与实际执行上限。`.trim();
}
