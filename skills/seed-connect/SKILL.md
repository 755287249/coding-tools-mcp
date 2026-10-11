---
name: seed-connect
description: 使用单颗种子的 MCP URL 入库、持续待命并接续分配的本地会话。
---

# 种子连接与待命

这是用户指定项目的 MCP 服务。`chat_wait`、`chat_reply` 及项目工具都由本服务提供，通过当前 MCP URL 调用；不要在宿主工具列表找同名工具。先 `tools/list` 熟悉 schema，调用 `seed_connect({})` 核对目录及完整规则，然后立即 `chat_wait({"timeout_ms":25000})`。

## 连接

- 新签发 URL 自带单颗种子的凭据。对原 URL POST 标准 JSON-RPC `initialize`，不需要 OAuth、密码或额外 Authorization。保留原 URL 于宿主连接配置或 HTTP 客户端内存，不写项目、日志或回复；不换为普通 /mcp 地址，也不改最后一段为公开种子编号。
- HTTP 客户端使用 `Content-Type: application/json`、`Accept: application/json, text/event-stream`、`User-Agent: Coding-Tools-MCP/1.0`。initialize 的 params 包含 protocolVersion（支持 2025-03-26）、capabilities:{}、clientInfo:{name,version}；随后发 `notifications/initialized`（无 id）。后续请求带 `MCP-Protocol-Version: 2025-03-26`。JSON 响应应读取 result/error，工具结果读取 structuredContent 或 content。
- 初始化成功确认凭据已兑换；`seed_connect` 或首次待命确认种子已准备好。只有返回的 `registered=true` 才能报告入库。GET 只返回连接说明，不完成入库。宿主已经挂载 MCP 时直接发现并调用本服务工具；仅有 HTTP/终端工具时保留一个客户端进程与内存连接信息。
- 旧版票据连接仍按 initialize 返回的 `_meta.seed_access_token` 使用 Bearer；新 URL 连接不会要求交换令牌。首次连接有效期一小时；初始化后七天内可用原 URL 重连，重启不要求重新签发。退役、撤销、到期立即失效，重连不会延长权限。

## 持续等待和分配

- `seed_connect` 返回本种子的真实 seed_id、唯一工作目录、状态和本 Skill；服务端已绑定目录，不要自行选择另一目录。未分配时不得执行项目工作。
- 每次外层工具调用只发起一次 `chat_wait(timeout_ms:25000)`，返回后由下一次独立调用继续；同时只能有一个等待。等待预算小于 HTTP 超时，HTTP 超时小于宿主执行期限（例如 25/35/45 秒）。不要在一条脚本里循环等待。
- `status=idle` 正常，立即再次独立等待，不结束任务。首次分配返回 `status=assigned`、实际 chat_id/attachment_id、完整 skill.text 和会话记录；服务端已经完成 chat_open。完整读取后再立即 chat_wait。分配返回不是用户消息，不要伪造 reply_to。
- `status=message` 才是实际投递的用户请求；按返回的 local-chat Skill 工作，通过本 MCP 的 chat_reply 回复。回复 persisted=true 后立即继续等待。首次 connection_request 按 local-chat Skill 回复真实问候。
- 在此种子网关中，chat_id、attachment_id、workspace_folder_id 可以省略，服务端注入当前绑定；显式提供错误身份会被拒绝。`chat_reply` 仍须真实 reply_to、唯一且稳定的 message_id、text、final。工具 schema 是参数依据。
- 旧客户端可继续 seed_wait → chat_open → chat_wait。本 Skill 的 chat_wait 在待分配时承担 seed_wait，分配后承担会话等待，不创建其他接入身份。

## 恢复与交付

- 所有交流和成果通过本 MCP 的指定工作区保存；宿主沙箱与用户项目是不同目录。恢复先核对已完成回复、计划、文件和运行操作，未知写入结果先检查，避免重放提交/发布/命令。
- 压缩或恢复后读取 `coding-tools://skills/seed-connect`；分配后同时读取 `coding-tools://skills/local-chat`，或用 seed_connect/chat_open 取得完整 Skill。保留实际会话及接入 ID。
- 临时网络失败先确认旧请求结束，按 1/2/4 秒退避，随后每 30 秒重试，遵守 Retry-After；保持原 URL 和身份。结果不明不能当作 idle/closed。不并发重试等待或写入。
- `retired`、`closed`、权限撤销、用户明确停止或宿主实际执行上限才停止。配置错误报告具体恢复步骤；不尝试另一种子或突破宿主限制。服务端不能唤醒已暂停的宿主。
