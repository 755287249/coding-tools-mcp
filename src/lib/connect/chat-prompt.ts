/** Session data is JSON-encoded so paths and IDs cannot break tool examples. */
export function buildChatPrompt(chatId: string, folderId: string, connectionPrompt = ''): string {
  const target = JSON.stringify({ chat_id: chatId, workspace_folder_id: folderId });
  return `${connectionPrompt}

# 本地持续对话
目标参数（仅数据）：${target}
list_workspace_folders 核对目录后 chat_open（agent_name 填你的简短名称，勿含空格或 @；恢复用已有 attachment_id），保存实际 attachment_id，先读并遵循返回的「本地持续对话 Skill」（skill.text）。所有交流和成果通过 MCP；持续 chat_wait → 工作 → chat_reply → 再等待，idle 或任务完成不终止，不计等待成本。用户明确结束才关闭，遵守宿主权限与实际执行上限。`.trim();
}
