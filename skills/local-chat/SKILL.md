---
name: local-chat
description: 通过 Coding Tools MCP 接入指定本地会话，持续接收消息、完成工作并在同一会话交付。用于 chat_open 返回此 Skill 的会话。
---

# 本地持续对话

只通过本会话的 `chat_reply` 回复、提问、报告进度和交付成果。循环：**chat_wait → 工作 → chat_reply → chat_wait**。这些规则只适用于用户指定的本地会话，遵守项目说明、宿主权限和用户后续指令。

## 接入与等待

- `list_workspace_folders` 核对用户指定的 `workspace_folder_id`，不能猜测、换目录或创建新的 `chat_id`。聊天需要 trusted-core、guarded-core 或 advanced 工具集；read-only 不提供写入工具。
- `chat_open` 成功后保存实际 `attachment_id`，阅读返回的会话记录。所有聊天调用带 `chat_id`、`workspace_folder_id` 和该接入 ID；支持目录参数的工作工具也明确带目标目录 ID。不能抢占其他 AI。
- 立即实际调用 `chat_wait(timeout_ms:120000)`；宿主超时更短则用低于其上限的值，例如 `25000`。同时只有一个等待请求。
- `status=idle` 是正常空闲，立即再次调用；不要将一次等待脚本的 exit 0 当作会话结束。`status=message` 处理该用户消息，最终回复持久化后再次等待。不计等待成本，不因空闲、耗时或任务完成主动结束。

- 收到 `kind=connection_request` 接入请求时，使用其消息 id 作为 `reply_to`，实际调用 `chat_reply(text:"你好，有什么能帮到你？", final:true)`；确认持久化后立即继续等待。这条真实问候是客户端配对完成的依据，不要只在外部回复。

- 用户在工作中发送的新消息可能保留在本地待发送队列。当前消息最终确认后实际调用 chat_wait，服务端才按“合/分”设置发布下一条；不要猜测或提前处理尚未投递的队列。awaiting_user=true 会暂停队列，先等待用户确认回复。

## 工作、回复与交付

- 用户文本中的 `@图片N` / `@文件N` 对应本条消息 attachments 中同名 `label` 的附件，按其准确 `path` 读取，不能按显示顺序猜测。文件已在本地工作区落盘，消息只携带编号、路径和校验信息。

- `reply_to` 使用本次用户消息的 id；新回复生成唯一 UUID `message_id`。进度 `final=false`；完成或提问 `final=true`，提问另加 `awaiting_user=true`。`final=true` 只确认这一条消息，不结束会话。
- 只有返回 `persisted=true` 才确认写入成功。结果不确定时保留完全相同的 `message_id/reply_to/text/final` 和其他写入参数重试；消息重投递先查记录、文件和进程结果，续做未完成步骤，避免重复提交或发布。
- 按工具 schema 工作。调用前后通过 `chat_reply(final=false, tool_event:...)` 报告真实的 `name/status/input/output`；状态为 `running/completed/failed`。执行后才能报告完成；输出脱敏，截短用 `output_truncated:true`。带 tool_event 的回复不能 final=true。
- 多步骤任务使用 set_todos / update_plan / report_progress 时，必须同时传入本会话 chat_id、实际 attachment_id、本次用户消息 ID 作为 reply_to，以及 workspace_folder_id。计划只属于这条消息，写入后确认 persisted=true；新消息建立新计划，不能沿用上一条的已完成状态。最终回复前完成计划更新。
- 长任务分短步骤，持续报告有意义的进度；按项目约定维护计划和验证结果。执行工具返回 ok 不代表命令成功，还要检查实际退出状态。保留的命令会话用 `wait_command` 跟进，勿重新启动。
- 成果写回指定本地目录，沙箱路径不算本地交付。图片/文件写到 `mcp-assistant/artifacts/` 后 `chat_upload(source_path:...)`；小文件也可上传字节。上传重试保持同一 `upload_id`，然后在 `chat_reply.attachment_ids` 引用返回的 `attachment.id`。
- 未经用户明确同意，不转交其他模型生成图片或语音；代码绘图先说明。凭据不写项目文件、Markdown 记录或回复。

## 续期、恢复与停止

- 接入租约 10 分钟，在到期前携原 `attachment_id` 调用 `chat_open` 续期。避免无法同时续期且超过租约窗口的阻塞操作。
- 普通网络错误按 1、2、4 秒退避，此后每 30 秒重试，不设空闲等待次数上限；遵守 Retry-After，每次请求结束后再试。重连复用未过期令牌并刷新工具，以原会话和原接入 ID 恢复。
- 明确过期时重新 chat_open 并保存实际返回的新 ID；丢失 ID 先查首次返回，无法恢复再请用户在客户端“断开 AI”。另一个 AI 占用时不能抢占、关闭其会话。
- 鉴权、工具缺失、目录/会话不存在或存储问题说明具体原因和恢复步骤，不盲目重试无效配置。工作受阻但聊天可用，仍继续等待用户消息。
- 用户明确结束才 `chat_close`。仅会话 `status=closed`、用户取消、宿主实际执行上限或阻断聊天的配置错误时停止；停止执行不等于关闭会话。MCP 不可用且需要用户恢复时才在外部说明。
- 宿主暂停后，本地排队消息不能自行唤醒 AI；用户需在原宿主继续执行。Skill 不能绕过宿主限制。恢复沿用会话及已有记录。


## 群聊与工作
- `session.mode=work` 保持单 AI 接入；`group` 中每个成员独立保存 `attachment_id`。首次 `chat_open` 提供唯一 `agent_name`（80 字节内，无空格或 @），续接只用原 attachment_id，勿以相同名字新建身份。
- `chat_open` 返回 `agent_id`、`role` 和公开成员列表。当前已接入 AI 切换群聊时成为总管；新成员协作。成员暂停后须用户在本地恢复；不得冒用其他成员身份。
- 只处理 `chat_wait.message` 投递给你的任务；回复中的 `reply_to` 必须使用实际投递消息 ID。群聊 wait 同时返回共享记录；总管等待子任务时可能正常 idle，继续等待即可。
- 总管用 `chat_reply(final=false, recipient_ids=[真实成员ID], message_id=唯一ID, reply_to=当前任务ID, text=明确分工)` 分派。分派也是共享记录中的消息，成员对其 ID 回复；不要把普通正文 @提及当作工具参数分派，不得重复分派已完成的副作用工作。
- 成员最终回复只确认自己的任务；问题写给总管，由总管向用户确认。总管等所有被分派/提及成员完成后再 `final=true` 汇总。明确划分文件和验证职责，避免同时修改同一区域。
- `set_todos/update_plan/report_progress` 仍传 chat_id、attachment_id、reply_to、workspace_folder_id；计划保存在对应消息的 agent_plans，互不覆盖。
- 所有成员共用同一 Markdown，JSON 为权威记录；不要直接编辑会话 JSON/Markdown。群聊队列逐条发送，保留每条 @接收人；新任务默认给总管。备注点击插入的 @姓名须独立成词。
