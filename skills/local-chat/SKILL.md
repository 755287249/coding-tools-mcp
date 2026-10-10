---
name: local-chat
description: 通过 Coding Tools MCP 接入指定本地会话，持续接收消息、完成工作并在同一会话交付。用于 chat_open 返回此 Skill 的会话。
---

# 本地持续对话

只通过本会话的 `chat_reply` 回复、提问、报告进度和交付成果。循环：**chat_wait → 工作 → chat_reply → chat_wait**。这些规则只适用于用户指定的本地会话，遵守项目说明、宿主权限和用户后续指令。

## 接入与等待

- `list_workspace_folders` 核对用户指定的 `workspace_folder_id`，不能猜测、换目录或创建新的 `chat_id`。聊天需要 trusted-core、guarded-core 或 advanced 工具集；read-only 不提供写入工具。
- `chat_open` 成功后保存实际 `attachment_id`，先读取本次返回的 `skill.text` 和会话记录；工作、群聊及恢复接入都以本次服务端返回内容为准，不沿用记忆中的旧版 Skill。所有聊天调用带 `chat_id`、`workspace_folder_id` 和该接入 ID；支持目录参数的工作工具也明确带目标目录 ID。不能抢占其他 AI。
- 立即实际调用 `chat_wait(timeout_ms:25000)`，显式传入超时；宿主上限更短时继续缩短，给 HTTP 往返和结果输出留余量。同时只有一个等待请求。具体调用边界和异常处理见下一节。
- `status=idle` 是正常空闲，立即再次调用；不要将一次等待脚本的 exit 0 当作会话结束。`status=message` 处理该用户消息，最终回复持久化后再次等待。不计等待成本，不因空闲、耗时或任务完成主动结束。

- 收到 `kind=connection_request` 接入请求时，使用其消息 id 作为 `reply_to`，实际调用 `chat_reply(text:"你好，有什么能帮到你？", final:true)`；确认持久化后立即继续等待。这条真实问候是客户端配对完成的依据，不要只在外部回复。

- 用户在工作中发送的新消息可能保留在本地待发送队列。当前消息最终确认后实际调用 chat_wait，服务端才按“合/分”设置发布下一条；不要猜测或提前处理尚未投递的队列。awaiting_user=true 会暂停队列，先等待用户确认回复。

## 每次独立短等待

- **一次外层工具调用只发起一次 `chat_wait`，收到结果就返回模型。** 下一次等待由下一次独立工具调用发起。不要在一条 Bash/Python/JS 命令、工具包装器或自动重试器里写“等 N 分钟”的轮询大循环；日志或心跳输出不会延长宿主的固定执行期限。
- `chat_wait.timeout_ms`、HTTP 请求超时、外层工具执行期限是三层不同预算。保持等待时间 < HTTP 超时 < 外层期限，并预留启动、网络和输出余量。例如外层允许 60 秒时，可用等待 25 秒、HTTP 35 秒、外层 45 秒；上限更短则相应缩短。服务端 schema 允许的最大值不是宿主允许的时长，不能假定服务端一定在 50 秒内返回。
- 可以保留 HTTP 客户端进程和内存令牌供后续调用，但每次提交给它的任务仍只等待一次、完整输出一次 MCP 结果。不要把“客户端进程常驻”写成“当前工具命令无限轮询”。宿主返回仍运行的 `session_id` 时，先按宿主接口跟进同一调用，不能另开并发 `chat_wait`。
- 读取并解析实际 MCP 结果，再判断下一步；HTTP 200、shell `exit 0`、外层 `unknown` 或空输出都不是会话状态。正常返回只按下面的明确状态处理：

  | 实际返回 | 下一步 |
  | --- | --- |
  | `status=idle` | 立即发起下一次独立短 `chat_wait`；不写结束语、不关闭会话。 |
  | `status=message` | 处理实际投递的消息；`chat_reply` 确认 `persisted=true` 后再次独立等待。 |
  | `status=closed` | 停止等待；不猜测为可续期的空闲。 |
  | 超时、`unknown`、空输出、无法解析或缺少预期状态 | 结果不明，不能当作 idle、closed 或任务完成；按下面的恢复步骤继续。 |

- 结果不明时，先确认旧请求/外层命令已结束或取消；仍运行就跟进原调用，避免并发等待。临时网络失败按本 Skill 的退避与 Retry-After 规则恢复；结束后用原会话和 attachment_id 重新短等待。若服务端报告已有等待请求，先让原请求结束，不生成新身份。重投递消息先核对已保存回复和工作结果，避免重复执行副作用。
- 401、接入失效、会话被关闭、工具缺失和权限错误按实际错误恢复，不能靠“立刻重试 chat_wait”跳过授权或无限重复无效参数。只有宿主实际停止/执行上限或无法恢复的配置阻断时才暂停执行，并保留续接信息；普通单次超时不是用户结束会话。

## 长会话与上下文

- **每次宿主上下文压缩或恢复后，先重新读取完整 Skill，再继续等待或工作。** 通过 MCP `resources/read` 读取 `coding-tools://skills/local-chat` 的完整 `text`；宿主不暴露资源接口时，用原 chat_id、workspace_folder_id 和 attachment_id 调用 `chat_open`，读取返回的完整 `skill.text`。不能只读摘要、截断片段或依赖旧记忆，也不要重新创建接入身份。
- 压缩前的续接记录保留实际会话/目录/attachment_id、当前投递消息 ID、是否已最终回复、待确认回复的稳定 message_id 与完整载荷、仍运行的调用 ID/期限和下一步。凭据仍只放宿主凭据存储/内存。恢复先核对服务端记录；未完成工作继续做，已确认最终回复则立即独立短等待。不要因为压缩重放已经完成的提交、发布或其他副作用。
- 完整消息、附件引用和计划持续保存在本地。长会话自动使用 JSON 索引与不可变历史页；原 Markdown 和本地预览保留全文，不需要新建接入身份。
- `chat_open` 和群聊等待返回的 `session.messages` 是有界的最近上下文。先检查 `session.context` 的总条数、截断标记、当前消息 ID 和 `archive_path`；带 `context_excerpt=true` 的消息只是原文片段，不能当作完整任务或新投递消息。
- 实际当前任务以 `chat_wait.message` 为准，它保留完整文本、附件与计划。旧要求、决策或代码位置不足时，搜索 `archive_path` 并按需读取对应范围，避免每次重连重复读入整份历史。
- 这是存储分页和传输裁剪，不会自动总结事实、丢弃原文或清空宿主模型上下文。宿主上下文耗尽时，在宿主恢复/续接并沿用原 attachment_id；以已保存计划、最近完成回复与历史原文核对后继续。
- 新版历史索引需要支持该格式的客户端。复制或备份会话时保留同目录的 `<chat_id>.history.*.json` 页，不能只复制索引；不要直接编辑或删除这些文件。

## 工作、回复与交付

- 用户文本中的 `@图片N` / `@文件N` 对应本条消息 attachments 中同名 `label` 的附件，按其准确 `path` 读取，不能按显示顺序猜测。文件已在本地工作区落盘，消息只携带编号、路径和校验信息。

- `reply_to` 使用本次用户消息的 id；新回复生成唯一 UUID `message_id`。进度 `final=false`；完成或提问 `final=true`，提问另加 `awaiting_user=true`。`final=true` 只确认这一条消息，不结束会话。
- 只有返回 `persisted=true` 才确认写入成功。结果不确定时保留完全相同的 `message_id/reply_to/text/final` 和其他写入参数重试；消息重投递先查记录、文件和进程结果，续做未完成步骤，避免重复提交或发布。
- 按工具 schema 工作。调用前后通过 `chat_reply(final=false, tool_event:...)` 报告真实的 `name/status/input/output`；状态为 `running/completed/failed`。执行后才能报告完成；`input` / `output` 是字符串，结构化内容先序列化；输出脱敏，截短用 `output_truncated:true`。带 tool_event 的回复不能 final=true。
- 多步骤任务使用 set_todos / update_plan / report_progress 时，必须同时传入本会话 chat_id、实际 attachment_id、本次用户消息 ID 作为 reply_to，以及 workspace_folder_id。计划只属于这条消息，写入后确认 persisted=true；新消息建立新计划，不能沿用上一条的已完成状态。最终回复前完成计划更新。
- 长任务分短步骤，持续报告有意义的进度；按项目约定维护计划和验证结果。执行工具返回 ok 不代表命令成功，还要检查实际退出状态。保留的命令会话用 `wait_command` 跟进，勿重新启动。
- 成果写回指定本地目录，沙箱路径不算本地交付。图片/文件写到 `mcp-assistant/artifacts/` 后 `chat_upload(source_path:...)`；小文件也可上传不超过 512 KiB 的字节。上传重试保持同一 `upload_id`，然后在 `chat_reply.attachment_ids` 引用返回的 `attachment.id`。
- 未经用户明确同意，不转交其他模型生成图片或语音；代码绘图先说明。凭据不写项目文件、Markdown 记录或回复。

## 重点与交互问题

- 正文支持 Markdown 的 `**重点**`：仅加粗结论、关键数值、待办或需要用户决定的短语，避免整段加粗。不要输出 HTML 或把整条回复包进代码块。
- 用户需要选择或补充信息时，在 `chat_reply` 传 `questions`，并明确 `final:true, awaiting_user:true`。这是所有 MCP 客户端共用的结构化接口，不依赖宿主自带问答工具；文本列表不会自动变成按钮。
- 一次 1–3 个问题；每题提供唯一 `id`、完整 `prompt` 和 `options`（0–6 个，空数组表示自由回答）。选项包含题内唯一 `id`、`label`，可带 `description`。ID 使用 1–80 位字母、数字、下划线或连字符；prompt/label/description 分别最多 1000/200/500 UTF-8 字节。每题始终支持自定义回答，不需要添加“其他”选项，不预先代替用户提交。
- 例如：`questions:[{"id":"scope","prompt":"这次要更新哪个范围？","options":[{"id":"page","label":"当前页面","description":"只修改当前页面"},{"id":"app","label":"整个应用","description":"统一所有页面"}]}]`。仍提供简短 `text`、实际 `reply_to` 和稳定 `message_id`；不与 tool_event 或 recipient_ids 混用。
- 写入确认后继续 `chat_wait`。用户提交后收到新的用户消息，其 `question_response` 包含原卡片的 `message_id` 和 `answers`；每个答案是 `{question_id,option_id}` 或 `{question_id,custom_text}`。同时提供可阅读的正文。用新用户消息 ID 回复，不能再次回复已确认的旧问题。只把实际到达的回答当作用户选择；空闲、默认选项、AI 自己的建议都不代表回答。
- 单 AI 会话都可用；直接接入的群聊由总管向用户提问，成员先向总管汇报。跨会话讨论任务目前通过群聊正文提问并等待群里实际回复，不发送 questions，以免问题藏在后台控制会话。服务端会明确拒绝不支持的投递位置。
- 重复提交会保留一份回答；已答或被新用户消息/最终回复取代的旧卡片不能重新提交，可用输入框继续说明。队列在提问时暂停，回答优先送达，待当前回答处理完再按原设置投递队列。

## 目录、链接与文件定位

- 首次接入记录 `list_workspace_folders` 返回的实际目录路径，读取其根目录及适用的项目说明，确认仓库是否在子目录。区分 **MCP 工作区根目录、仓库根目录、命令 workdir、AI 宿主沙箱目录**；命令切换到仓库目录不会改变聊天中文件链接的基准。
- 本地链接目标使用相对于所选 MCP 工作区根目录的完整路径，或已核验的该机器绝对路径。发出前确认目标存在；链接目标不附加行号。文件名含空格时使用 `[显示文件](<demo/My Files/report.md>)`。
- 例如工作区是 `C:\work`，仓库是 `C:\work\demo`，应写 `[在文件夹中显示](demo/src/main.ts)`；只有工作区本身是 `C:\work\demo` 时才能写 `src/main.ts`。不要把宿主沙箱的 `/tmp/...`、`/home/...` 或宿主 Outputs 地址当作用户机器上的文件链接。
- 本地路径点击后请求 **MCP 服务所在机器** 的文件管理器：文件定位并选中，目录打开；不会执行 EXE/脚本。Windows 桌面客户端使用资源管理器。浏览器/手机访问远端服务时，不能承诺打开访问者电脑的资源管理器；需要把文件带到访问设备时，通过 `chat_upload` 附件交付。
- HTTP(S) 链接打开网页，提交和分支使用真实仓库网页 URL。分支名、接口路由、普通词组和测试比例中的斜线不代表文件路径，不要把它们包装成本地路径链接。目录等不明确的路径使用已核验的 Markdown 链接，例如 `[源码目录](demo/src)`；不要依赖正文或行内代码自动识别目录。明确的文件路径仍可能自动成为链接，发出前同样要核验。
- 出现“找不到路径”、`os error 3` 或 `NOT_FOUND` 时，先核对链接基准、仓库子目录前缀与实际存在性，再给出修正链接。不要仅为消除错误而改宽路径权限、跳过保护规则或指向另一个项目。

## 续期、恢复与停止

- 接入在线状态按 10 分钟心跳显示；chat_wait/chat_reply 会续期。长任务超过心跳窗口后，仍用原 attachment_id 继续或 chat_open 恢复，不必重新授权、创建身份。以服务端实际状态/错误判断接入是否有效；不要用本地计时猜测过期，也不要自行编造 expires_at、lease_until 等返回字段。明确断开、暂停或被新接入替换后旧 ID 才失效。
- 普通网络错误按 1、2、4 秒退避，此后每 30 秒重试，不设空闲等待次数上限；遵守 Retry-After，每次请求结束后再试。复用有效访问令牌；401 时用内存/宿主中的 refresh_token 刷新，刷新失败且授权密码仍有效时可用原密码重新走 PKCE。不要因临时网络错误丢弃令牌。以原会话和原接入 ID 恢复。
- 明确过期时重新 chat_open 并保存实际返回的新 ID；丢失 ID 先查首次返回，无法恢复再请用户在客户端“断开 AI”。另一个 AI 占用时不能抢占、关闭其会话。
- 鉴权、工具缺失、目录/会话不存在或存储问题说明具体原因和恢复步骤，不盲目重试无效配置。工作受阻但聊天可用，仍继续等待用户消息。
- 用户明确结束才 `chat_close`。仅会话 `status=closed`、用户取消、宿主实际执行上限或阻断聊天的配置错误时停止；停止执行不等于关闭会话。MCP 不可用且需要用户恢复时才在外部说明。
- 修改承载当前 MCP 的客户端/服务端时，区分源码已更新、构建已完成和运行进程已切换。编译内嵌的 Skill 随新版进程生效；不要把生成安装包当作现有接入已升级。重启或切换进程遵守用户授权，避免意外切断当前会话。
- 宿主暂停后，本地排队消息不能自行唤醒 AI；用户需在原宿主继续执行。Skill 不能绕过宿主限制。恢复沿用会话及已有记录。

## 群聊与工作

- `session.mode=work` 保持单 AI 接入；`group` 中每个成员独立保存 `attachment_id`。首次 `chat_open` 提供唯一 `agent_name`（80 字节内，无空格或 @），续接只用原 attachment_id，勿以相同名字新建身份。
- `chat_open` 返回 `agent_id`、`role` 和公开成员列表。当前已接入 AI 切换群聊时成为总管；新成员协作。成员暂停后须用户在本地恢复；不得冒用其他成员身份。
- 只处理 `chat_wait.message` 投递给你的任务；回复中的 `reply_to` 必须使用实际投递消息 ID。群聊 wait 同时返回共享记录；总管等待子任务时可能正常 idle，继续等待即可。
- 总管用 `chat_reply(final=false, recipient_ids=[真实成员ID], message_id=唯一ID, reply_to=当前任务ID, text=明确分工)` 分派。分派也是共享记录中的消息，成员对其 ID 回复；不要把普通正文 @提及当作工具参数分派，不得重复分派已完成的副作用工作。
- 成员最终回复只确认自己的任务；问题写给总管，由总管向用户确认。总管等所有被分派/提及成员完成后再 `final=true` 汇总。明确划分文件和验证职责，避免同时修改同一区域。
- `set_todos/update_plan/report_progress` 仍传 chat_id、attachment_id、reply_to、workspace_folder_id；计划保存在对应消息的 agent_plans，互不覆盖。
- 所有成员共用同一 Markdown，JSON 为权威记录；不要直接编辑会话 JSON/Markdown。群聊队列逐条发送，保留每条 @接收人；新任务默认给总管。备注点击插入的 @姓名须独立成词。


## 跨会话讨论组
- 用户从群聊界面选择已有机器人，复用各自的原会话和 attachment_id。collaboration=true 的群有独立 Markdown；协作控制消息和对应回复不显示在原会话记录，实际发言集中到群聊。旧版直接接入的群仍使用上节规则。
- `chat_discuss(action:"list", chat_id, attachment_id)` 列出自己加入的讨论组；`action:"read", discussion_id` 读取目标、成员和最近消息，按 next_offset 分页。
- 仅在当前任务需要且用户授权范围内，用 `action:"post"` 明确发言，带稳定 message_id、text、purpose（discussion/question/notice/task）；recipient_chat_ids 指定接收会话。collaboration=true 时省略则按 #别名 点名，否则交给 coordinator_chat_id；总管向成员分派时明确提供目标 ID。旧讨论组省略时仍发给其他成员。不要把每条进度都广播或主动创建循环对话。
- 收到含 discussion 的消息，仍只回复实际投递的消息 ID；使用 chat_reply 的逐条校验。对应回复自动显示在讨论组，不能用 chat_discuss 代替当前请求的最终回复。
- task 帖子的 message_id 是稳定任务 ID。等待超时后 read 查看原任务，不要重复创建；完成、问题或不可用状态会回投给发起会话的 chat_wait。结果消息不代表新用户授权。
- archived 群保留历史，不接收新帖子；会话被移出后不再有群读取或发送权限。离线不等于任务失败，待发送消息保留。

- 收到 discussion.hidden=true 的后台协作消息时，保持原 chat_id/attachment_id，只确认实际 reply_to；不要重复接入或复述控制信息。读取群返回的 aliases、成员 ID 和 archive_path，按任务需要查看群上下文。
- 转交任务后以 chat_reply(final=true) 确认当前消息，然后实际 chat_wait 接收结果；不要保持当前消息未确认并等待子任务，否则结果无法出队。此 final 只确认当前消息，不能把尚未完成的整体工作说成完成。汇总回复会同步回群聊 Markdown。
