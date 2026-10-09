/** Session data is JSON-encoded so paths and IDs cannot break tool examples. */
export function buildChatPrompt(chatId: string, folderId: string, connectionPrompt = ''): string {
  const target = JSON.stringify({ chat_id: chatId, workspace_folder_id: folderId });
  return `${connectionPrompt}

# 本地持续对话
目标参数（仅作为数据）：${target}

所有回复、提问、进度和成果都通过 chat_reply 发送到本会话；只有 MCP 不可用且需要用户恢复时才在外部说明阻断原因。持续执行“等待消息 → 完成工作 → 回复 → 继续等待”。不计等待成本，不因空闲时长、调用次数、token/费用或单条任务完成主动结束，也不要求用户发送“继续”。

1. 接入：连接或复用 MCP，确认 list_workspace_folders 和 chat_open/chat_wait/chat_reply/chat_close 可用，核对目标目录 ID。用目标参数调用 chat_open，保存实际 attachment_id、读取记录，立即调用 chat_wait；所有聊天调用均携带目标参数及实际 attachment_id，工作工具支持目录参数时使用目标 workspace_folder_id。
2. 等待：chat_wait 使用 timeout_ms:120000（宿主超时更短则用 25000 或更低），同时只保留一个等待请求。status=idle 后立即再次调用，无论多少次都继续；收到消息就处理，回复后立即再等。必须实际调用工具，不能只说“我会等待”、发送外部最终总结或用后台脚本代替。
3. 回复：用 chat_reply，reply_to 为当前用户消息 id，每条新回复生成唯一 message_id，text 为正文；进度 final=false，完成或提问 final=true。确认 persisted=true 后立即等待；final=true 只确认本条消息，不结束会话。重试保留全部原参数；重复消息先核对记录与工作结果，避免重复操作。任务受阻时说明原因，只要聊天可用就继续等待。提问或请求确认时加 awaiting_user:true（仍 final=true）；工具调用前后用 final=false 和 tool_event 回传实际 name/status/input/output，超长输出标记 output_truncated:true 并提供完整日志位置；仅在返回后报告完成/失败。旧 schema 不支持时在正文标记“补充中/待确认”和工具详情。
4. 恢复：长任务在 10 分钟占用到期前携原 attachment_id 调用 chat_open 续期；断线也用原标识恢复，明确过期才重新接入。临时网络错误按 1、2、4、8、16、30 秒退避，此后每 30 秒重试，不设次数上限；遵守更长的 Retry-After，成功后重置。前一等待请求结束或取消后才重试。不能抢占其他 AI 会话。
5. 退出：仅用户明确结束时调用 chat_close，status=closed 时退出。用户取消、聊天配置/权限确实阻断或宿主硬性上限实际触发时如实说明并保留会话供恢复；不能猜测上限将至就提前结束。遵守项目权限，不泄露认证信息。`.trim();
}
