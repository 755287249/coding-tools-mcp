# 本地 MCP 对话测试版

在工作区顶部选择 **对话 · Beta**。简洁模式和高级模式共用该页面，Desktop 通过 Tauri IPC，Node Agent 通过已鉴权的管理 API 访问同一套 v1 数据契约。

## 启动

沿用项目已配置的安装及运行环境：

```sh
# Desktop 开发环境（需要系统桌面依赖）
pnpm run desktop

# Node Agent：共用同一个 Svelte 对话页面
pnpm run node-agent:build
pnpm run node-agent:start
```

Node Agent 默认本地入口为 `http://127.0.0.1:3789/ui`，以实际启动输出为准。工作目录、OAuth 和端口沿用现有客户端配置，聊天功能无需额外模型 API Key。

## 连接外部 AI

1. 在现有客户端配置目标目录，启动 MCP 服务。
2. 确认外部 AI 宿主支持 MCP；远程 AI 需要可访问的公开地址，本机地址只能由同机宿主访问。
3. 打开本地“对话”，选择工作目录，点击“新建对话”。
4. 点击“复制 AI 接入指令”，发给外部 AI。复制的完整指令包含当前连接地址、认证信息、本次会话 ID、明确的 `workspace_folder_id` 和恢复流程。妥善保管，不要发布到日志或仓库。AI 按指令检查工具和目录、加入会话并实际等待；宿主不能自动添加 MCP 或完成 OAuth 时仍需要手动配置。页面展开的协议不显示认证秘密；每次复制会重新读取凭据。
5. 一次性密码失效或临时地址变化时重新复制；接入失败先处理报告的错误。当界面显示“AI 正在待命”时，在本地输入需求。
6. AI 通过现有文件、命令等工具工作，通过 `chat_reply` 回传回复或进度，再调用 `chat_wait`。

聊天工具在 `trusted-core`、`guarded-core`、`advanced` 中提供；`read-only` 不提供聊天写入工具。兼容工具目录仍按现有项目契约发布。

## 工具契约

| 工具 | 参数与行为 |
| --- | --- |
| `chat_open` | `chat_id`，可选旧 `attachment_id`；加入 UI 创建的会话，返回占用标识、现有消息及循环说明。 |
| `chat_wait` | `chat_id`、`attachment_id`、`timeout_ms`；默认 120 秒，上限 180 秒。返回 `message`、`idle` 或 `closed`。 |
| `chat_reply` | `chat_id`、`attachment_id`、`message_id`、`reply_to`、`text`、`final`；`final=false` 保存进度，`final=true` 保存最终回复并确认本条用户消息。 |
| `chat_close` | `chat_id`、`attachment_id`；结束会话，仅在用户明确要求时使用。 |

所有工具支持 `workspace_folder_id`，不依赖某家平台专属的 conversation 元数据。后续执行文件/命令工具时也应传入该目录，或通过现有工作区选择工具绑定目录。

成功回复返回 `persisted=true`。失败时使用相同 `message_id` 重试；同 ID 不同正文会被拒绝。用户消息按顺序交付，直到最终回复确认前仍可再次交付，AI 应核对已有工作结果，不能把重投递当作“副作用尚未执行”。会话、消息 ID 不接受路径分隔符。

占用有效期为 10 分钟，`chat_wait`、`chat_reply` 和携带原占用标识的 `chat_open` 会续期。长任务应定期报告进度。占用标识丢失时，等待过期后重接，或在 UI 结束旧会话并新建会话。

## 状态与持久化

- **正在待命**：本进程内确实有有效的挂起等待请求。
- **已接入 · 当前未等待**：占用仍有效；不代表模型此刻正在推理或客户端仍在线。
- **等待 AI 接入**：没有有效占用。
- **已结束**：不再接受消息，正在等待的请求返回 `closed`。

结束会话不会自动终止已启动的命令进程。请使用现有进程控制功能处理这些进程。

每个工作目录下保存：

```text
docs/chat-sessions/
  <chat-id>.json   # 版本、消息、占用期限；内部事实来源
  <chat-id>.md     # 用户/AI 文本及显式进度，自动生成
```

页面也支持下载 Markdown。原始 HTML 不会作为页面代码执行；支持标题、列表、粗体、行内代码和代码块。文本写入前使用项目已有脱敏规则。每条文本最多 32,000 UTF-8 字节，会话最多 500 条消息且 JSON 最大 2 MiB。达到限制时请新建会话。

写入使用临时文件替换和目录锁。若进程在持锁时崩溃，会停止后续写入；确认所有使用此目录的客户端已停止后，可以删除 `docs/chat-sessions/.lock` 再启动。不会自动抢占可能仍由其他进程持有的锁。JSON 为事实来源，Markdown 写入失败会报告错误，重试会重新生成。

聊天 Markdown 记录显式回传的回复/进度；已有工具执行历史和开发检查点仍通过 Telemetry/History 查看。外部 AI 界面中未回传的文本或其他工具执行不会自动被本项目捕获。

## 兼容性范围

- 已实现 Rust 与 Node 的相同磁盘/消息契约及共享 UI。
- MCP 等待请求不占用普通工具执行额度，取消请求清除待命标记。
- 客户端仍可能限制工具调用时长或连续执行轮数；不能保证所有 AI 客户端永久待命。可调整 `timeout_ms`，必要时重新发送接入指令。
- 首版通过 UI 每 1.5 秒读取状态刷新消息，不提供模型 token 自动流式转发；AI 可显式发布进度。
- 本版实现参考等待/回复循环的设计思想，未复制 VSIX 的混淆代码、IDE 注入或平台账户逻辑。

## 聚焦检查

```sh
pnpm run check
pnpm run ui:build:node
pnpm --filter @coding-tools/node-agent run build:server
node --import ./packages/node-agent/test/setup.mjs --test packages/node-agent/test/chat.test.mjs
node --import ./packages/node-agent/test/setup.mjs --test --test-name-pattern='local chat management' packages/node-agent/test/management.test.mjs
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --lib tools::chat::tests
pnpm run node-agent:contract
```

浏览器人工验收：创建会话 → 复制指令 → 本地发送 → 外部 MCP 领取并回传 → 刷新保留记录 → 等待/断开状态变化 → 导出 Markdown → 结束会话。协议模拟客户端的联调回复应明确标注来源，不能当作真实模型回复。

提示词要求空闲返回后继续等待、最终回复确认 persisted=true 后再等待；网络错误最多连续重试三次，重复投递先检查结果，长任务在 10 分钟占用到期前续期。不会通过提示词取消宿主的执行限制。提示词行为已做生成测试，真实第三方 AI 的长期待命仍需实测。
