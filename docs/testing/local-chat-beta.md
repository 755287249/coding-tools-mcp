# 本地 MCP 对话

在工作区顶部选择 **对话**。简洁模式和高级模式共用该页面，Desktop 通过 Tauri IPC，Node Agent 通过已鉴权的管理 API 访问同一套 v1 数据契约。

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

占用有效期为 10 分钟，`chat_wait`、`chat_reply` 和携带原占用标识的 `chat_open` 会续期。长任务应定期报告进度。占用标识丢失时，先找首次返回；找不到可在本地点击“断开 AI”后用原会话重接，或等待占用过期。

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

写入使用临时文件替换和目录锁。短暂争用最多等待 2 秒，超时报告 Chat storage is busy；不再把普通锁争用直接报告为 Windows os error 183。若进程在持锁时崩溃，会停止后续写入；确认所有使用此目录的客户端已停止后，可以删除 `docs/chat-sessions/.lock` 再启动。不会自动抢占可能仍由其他进程持有的锁。JSON 为事实来源，Markdown 写入失败会报告错误，重试会重新生成。

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

会话提示词要求沟通与成果通过 MCP 回传；chat_open 和空闲 chat_wait 也返回持续等待规则。不同 Agent 共用这套协议，实际执行仍受宿主能力与硬性上限约束。

精简提示词仅保留接入、等待、回复、恢复、退出五项核心规则，把会话作为持续循环：无论连续多少次 `idle` 都立即再次等待，不设空闲时长、次数或成本退出阈值。`final=true` 只确认当前消息；任务完成、需要补充信息、任务操作失败或提交/发布受阻后，只要聊天工具可用，就继续实际调用 `chat_wait`，不以宿主最终总结结束循环。

临时网络错误按 1、2、4、8、16、30 秒退避，此后每 30 秒重试，不设累计次数上限；遵守更长的 `Retry-After`，成功后重置退避。前一个等待请求结束或取消后才开始下一个，恢复连接先用原占用标识 `chat_open`。鉴权、目录/会话不存在等配置错误需要报告并修复，不进行无效重试。重复投递先检查结果，写入重试保留全部原字段，长任务在 10 分钟占用到期前续期。

用户明确结束、会话关闭、用户取消、聊天本身受阻或宿主硬性上限实际触发时停止；任务操作受限不等于聊天受限。被迫暂停时明确当前未在等待，保留会话与幂等信息供恢复。不通过提示词取消宿主的执行限制，也不把后台脚本当成持续运行的 AI。生成测试检查协议文字，真实第三方 AI 的长期执行行为仍需实测。

## 消息处理提示

发送后显示“等待 AI 接单”；`chat_wait` 实际领取消息时，两端都会保存首次 `received_at`，页面显示“AI 已接单，思考中…”。进度回复不会清除提示，只有该条消息的最终回复才清除；多条消息按最早未完成消息判断。旧版服务端可用实际进度回复确认接单。租约过期显示等待恢复，关闭会话不再显示处理提示。“思考中”是消息处理状态，不代表能检测模型内部推理或保证连接期间模型始终运行。

### 每条用户消息的已读和工作状态

每条用户消息在页面、自动生成的对话 Markdown 和手动导出中都有“未读 · 排队中”“已读 · 正在处理”“已读 · 待确认”“已读 · 已回复”等标记。收到后续用户回复，原提问标记更新；会话关闭时未完成消息显示已结束，保留已读/未读区别。旧会话有 AI 回复但没有领取时间时仍可判断已读。

“已读”表示消息已通过 `chat_wait` 交付，或已有对应 AI 回复；仅接入会话不会把所有排队消息标为已读，也不代表可以检测模型是否逐字理解内容。工作状态基于最后一次领取/回复，是处理记录；实时连接是否过期仍看会话状态。Markdown 是 JSON 的派生记录，在发送、领取、回复等写入时更新，请勿手动编辑它来驱动聊天。

### 状态图标

页面中的每条消息使用图标表达状态：空心圆表示未读、双勾表示已读、时钟表示排队、旋转圆环表示处理/补充中、问号表示待确认、圆形勾表示回复完成、方框表示会话结束。工具卡片也使用运行/成功/失败图标。悬停可查看文字含义，屏幕阅读器可读取同一标签；减少动态效果偏好会禁用旋转动画。Markdown 导出继续使用明确文字状态。

### 回复状态与工具详情

- `final=false` 的普通回复显示“补充中…”，同一用户消息的最终回复出现后移除该标记；最终回复显示“回复完成”。
- 提问/确认使用 `awaiting_user:true, final:true`，显示“待确认”，确认当前消息后继续 `chat_wait`。用户发来下一条消息后，原提问标记为“已回复”。这只表示用户有回复，不代替对回复内容或权限的判断。`awaiting_user:true` 与 `final:false` 不能组合。
- `tool_event` 增加可选 `input`（最多 8,000 UTF-8 字节）、`output`（最多 16,000 UTF-8 字节）、`output_truncated`。命令、参数和结果经脱敏后持久化，在卡片内按纯文本展开；超长输出应明确标记节选并在正文提供完整日志位置。新增字段纳入同 ID 重试冲突校验，也保存在本地 Markdown 和页面导出中。
- 工具卡片由 AI 按提示词显式回传，包括 Bash；客户端不能自动获取外部宿主未回传的工具调用、完整日志或内部推理。旧 schema 不支持新字段时在正文使用状态标记和工具详情。

## 附件和工具进度（0.1.61）

- 添加附件：支持在聊天输入框直接粘贴剪贴板图片，粘贴后加入待发送附件；纯文本粘贴保持原行为。每条消息最多 5 个，每个 2 MiB；每个会话最多 32 个上传文件、合计 32 MiB。移出输入框不会删除已经上传的文件，上传计入会话配额。
- 支持纯附件消息。文件存于 `docs/chat-sessions/<chat-id>-<file-id>.<ext>`，JSON/Markdown 仅记录元数据与路径，不内嵌二进制内容。图片按文件签名识别 PNG/JPEG/GIF/WebP；其他格式提供下载，SVG/HTML 不作为网页执行。
- 附件读取使用已鉴权的聊天管理入口，校验会话归属、路径和 SHA-256；MCP 消息包含附件 path，可通过已有文件/图片工具读取。附件不会自动成为模型原生视觉输入，需要宿主具备图片工具。
- `chat_reply` 可选 `tool_event: {name, status}`，status 为 running/completed/failed，必须 `final=false`。调用者显式回传工具用途、结果或错误，页面显示可展开卡片；不自动拦截其他工具、不采集模型内部推理。最终答复仍单独用 `final=true` 确认用户消息。
- 旧会话兼容；旧版工具不支持 tool_event 时，接入指令要求使用普通进度文本。

## 代码和图片预览（0.1.61）

HTML、SVG、JS 回复代码块以及上传的 HTML/JS 文件提供“代码 / 运行”切换和重新运行。JS 运行区显示 console 输出及运行错误；普通 UTF-8 文本、JSON、Markdown、Python、Rust 等附件可查看源码。预览最多 256 KiB，较大文件仍可下载。Node.js、TypeScript/JSX 编译以及项目依赖安装不由此浏览器预览器提供。

运行仅在用户点击后发生；切回代码会销毁 iframe。独立 runner 使用不带 allow-same-origin 的 sandbox，脚本不能读取父页面和管理令牌，资源加载/网络 API 受 CSP 限制。Node 通过独立 `/ui/chat-preview.html` 路由返回无管理令牌的 runner；Desktop 使用专用 chatpreview 协议，主应用脚本 CSP 未放宽。预览不具备 Tauri 或本地文件工具权限。

图片支持点击放大、25%–400% 缩放、适应窗口和关闭。预览本身不保存编辑；附件原文件和聊天记录保持原样。当前验证包括 Web UI 实际运行及 Native 编译，Windows WebView2 实机运行仍需新版打包后验证。


## HTTP 兜底与升级恢复

宿主不支持添加原生 MCP 服务时，可通过 HTTP 按 MCP 协议手动连接；每个请求都带 `User-Agent`，避免缺失该头时被 Cloudflare 拦截。HTTP 方式仍须完成 OAuth + PKCE/Bearer 认证、初始化、工具发现和实际目录调用，不能把普通 HTTP 可达或401响应当作认证成功，也不能声称宿主已注册原生 MCP。

更新便携版时：先下载并解压新版到新目录，保留旧版以便回退；备份本地配置和项目的整个 `docs/chat-sessions/`（含 JSON、Markdown、附件）。使用同一系统用户，退出旧进程后启动新版 `ctmcp.exe`，保持原工作区、目录 ID、公开地址和认证配置，不点击“结束会话”，不清空数据或重新创建工作区。确认 MCP 服务与隧道恢复后，Agent 重建传输连接、刷新工具、核对目录，并用原 chat_id/attachment_id 重新 chat_open 再等待。

替换正在运行的进程会中断现有请求；本版提供的是原会话恢复条件与退避重连协议，不是零停机热更新。保持原签名密钥与地址时，尚未过期的 OAuth token 可继续验证；更换密钥/公开域名、令牌到期或宿主丢失令牌时需要重新认证。临时隧道地址变化需重新复制连接信息。外部 AI 宿主若已暂停，需要用户重新触发恢复；客户端无法强制唤醒宿主。


## 固定认证与手动 OAuth

桌面连接页提供“OAuth / PKCE”和“固定 Token / HTTP”。固定模式复用已有 Bearer 认证和本地凭据存储；服务或客户端重启不轮换 Token，只有手动重新生成/删除或更换配置才失效。切换会重启正在运行的服务，需要重新复制连接指令；使用固定域名能让 AI 继续用原地址退避重连。高级设置仍可轮换 Token。只接受 OAuth 的宿主应保留 OAuth 模式。

HTTP 手动 OAuth 的复制指令包含 `redirect_uri=http://127.0.0.1:8765/callback`，使用服务端已允许的本机回调规则，不依赖动态客户端注册。手动客户端禁止跟随授权 POST 的跳转，读取 Location 并校验 state，再用同一 redirect_uri/verifier 换取访问令牌；原生宿主使用自己的回调。令牌应保存在宿主凭据存储或进程内存，网络重试/服务重启优先复用；一次性授权密码不能用作长期重连凭据。401、已轮换 Token 或丢失凭据需恢复认证，不能靠无限重试解决。

共享 Svelte 提示词同时适用于 Desktop/Node。Node 本次对齐 Desktop 的 HTTP loopback 回调支持，但既有 Node 后端不提供静态 Bearer 认证，所以该入口按 capability 隐藏，继续使用 OAuth。静态 Bearer 不属于本次新增后端契约；当前差异明确保留。断网退避重连由仍在运行的 AI 宿主执行，客户端无法唤醒已停止的模型。


## 会话重命名

左侧每个会话的铅笔按钮可编辑名称，Enter 保存、Esc 或取消按钮放弃；名称最多240个UTF-8字节。名称同步保存到原JSON与Markdown，不改变会话ID、附件、消息、接入占用或归档路径；已结束会话也可重命名。手动名称通过可选 title_custom 标志保留，不被首条用户消息覆盖。Rust与Node本地聊天管理接口均支持rename，远程MCP工具schema不变。

侧栏卡片按实际状态显示：正在chat_wait为淡绿、只有有效接入占用为淡黄、离线为灰、结束变淡。选中状态保留状态色，提示文字提供非颜色识别。用户消息淡蓝、AI消息淡绿。精简接入提示词保留PKCE/state、令牌复用、无id初始化通知、实际等待、幂等回复与租约恢复；强调首次open立即保存attachment_id，避免重复无ID接入导致自锁。

若AI丢失attachment_id，可在本地对话页点击“断开 AI”释放接入，再用原会话重新连接；保留消息/附件/归档且不结束会话。此按钮不终止工作进程，也不保证其他仍运行的AI不会再次申请接入。远程工具不能借此抢占他人。

新回复角标：侧栏卡片右上角显示该会话未读AI回复数，聊天页右上角显示总数并可跳转。只有页面可见且读到会话底部时标已读；已读计数按工作区/目录存本机localStorage，不改变AI接入或远端消息确认状态。用户消息和幂等重试不增加回复计数。bash执行可通过现有tool_event卡片汇报命令与结果；不会自动截获外部宿主的工具日志。

持续对话提示词整理为六条：接入、等待循环、回复确认、工具反馈、恢复、退出；认证细节只保留在连接段，避免重复。

## AI 图片与文件回传

AI 生成文件后调用 `chat_upload`（chat_id、attachment_id、唯一upload_id、原始文件名name、data_base64、目标workspace_folder_id），得到attachment.id，再用`chat_reply`的attachment_ids引用并附正文。远端单文件最多512KiB（符合1MiB请求体上限；本地UI上传仍为2MiB），每条回复最多5个，不再限制会话累计附件数量和总文件大小；大文件可用下文的本地路径引用。重试保持upload_id、名称和字节不变。图片缩略图自动显示，可放大/下载；非图片复用附件预览与下载。新附件存储在项目mcp-assistant/chat-assets，旧附件继续保留在docs/chat-sessions，按内容哈希验证；生成能力来自AI宿主，测试图仅验证传输，不冒充生图输出。0.1.63不提供chat_upload，需升级后刷新工具。

工具过程按reply_to汇总：运行中的工具组展开，全部结束后自动折叠一行，失败保留摘要提示；手动可展开原始报告。中间夹文字进度不会拆组。并行同名工具按未完成数量保持进行态，原始命令/输出按消息顺序保留，不猜测配对关系。

会话背景按ID生成稳定低饱和度色相；每次切换和刷新保持颜色。文本草稿、待发送附件和发送重试ID按工作区/目录/会话保存到本机localStorage，切换后恢复，成功发送才清理对应草稿；存储不可用时保留当前应用内存并提示刷新风险。提示词禁止未经用户明确同意转交其他模型生成图片/语音，代码绘图先说明。

### 本地文件资料夹与路径索引

新附件的二进制文件写入所选项目的 `mcp-assistant/chat-assets/<chat_id>/`，会话 JSON/Markdown 只保存文件名、路径、大小和 SHA-256 等索引。取消单会话累计 32 个附件和 32 MiB 的配额；旧版 `docs/chat-sessions/<chat_id>-<upload_id>.*` 文件继续读取，不迁移或删除历史文件。

AI 可以将成果先写入所选项目的 `mcp-assistant/artifacts/`，再调用 `chat_upload`，提供 `source_path`（例如 `mcp-assistant/artifacts/result.png`），与 `data_base64` 二选一。这会登记该本地文件的路径与哈希，不复制文件内容、不把文件放入 MCP 请求体，也没有单文件传输配额。该目录属于会话资料，不应当作可自动清理的临时目录；登记后修改或删除文件会使原引用的完整性检查失败。

用户可在对话输入区点“本地文件”，粘贴此资料目录内的项目相对路径。普通文件选择和图片粘贴继续自动存入资料夹。每条消息最多5个文件；base64 上传的单次传输限制仍为本地2MiB/MCP512KiB，较大的文件请先保存到 `mcp-assistant/artifacts/` 再引用。2MiB以内文件继续原有预览与下载，大文件显示可复制的本地路径，避免把整个文件载入聊天页面；文本预览仍限制256KiB。

目录总量由本地磁盘容量决定。会话消息/索引文件自身的2MiB与500条消息保护仍在，不能承诺无限磁盘或无限历史。路径必须在明确的成果目录内，拒绝绝对路径、父目录跳转、ADS、符号链接；已登记文件每次读取均核验SHA-256。上传/引用失败时保留用户原文件，重试保持upload_id、文件名、路径/字节不变。

## Shared desktop-style shell

- The title row provides Back/Forward, sidebar toggle, and File/Edit/View/Help menus. Shortcuts: Ctrl+[, Ctrl+], Ctrl+Shift+S, Ctrl+N and Ctrl+K. Desktop Close hides the main window to the tray; Quit exits the application. The browser UI cannot close arbitrary browser tabs, so those entries are disabled there.
- Home is the only rail hover target for the floating conversation sidebar. Assets, Scheduled tasks, Skills, Plugins and More open functioning pages/controls for the selected project. Assets index persisted chat attachments and `mcp-assistant/artifacts/` references; they do not delete or move files.
- Scheduled tasks in this test version are one-time local message deliveries. Keep the client open; overdue tasks are delivered when it reopens. An attached AI is required to process them. Schedules are local to this browser/client profile. Web Locks serialize windows; stable message IDs make uncertain-response retries idempotent. Failed deliveries require explicit retry.
- Disconnect AI has a ten-second confirmation. It releases the attachment while retaining the conversation, files and ability to reconnect. Cancelling changes nothing. It does not permanently close the conversation.
- Windows 11 (build 22000+) uses Mica, showing the wallpaper rather than underlying application windows. Unsupported builds or effect errors use an opaque fallback. The message feed is opaque; surrounding chrome is tinted. Hover or keyboard-focus the bottom-right glass button to reveal opacity controls. Browser previews cannot verify native wallpaper behavior.

### Clickable local image references

Chat prose, bullet lists, inline code and Markdown links recognize PNG/JPEG/GIF/WebP paths under `mcp-assistant/artifacts/`. Clicking opens the zoomable viewer through the local UI `read_artifact` action. This read-only operation also works in closed chats, does not register attachments, validates workspace containment/symlink rejection and image signatures, and limits preview reads to 2 MiB. The artifact may change on disk; each click reads its current content. Existing attachment previews retain their immutable hash checks.

The cumulative attachment cap removal and these UI changes require rebuilding and upgrading the running client. Updating source files alone cannot remove the old installed server's `Session attachment limit reached` error. Historical manifests/files must not be deleted to work around it.

## Portable 0.1.66 startup and build timing

The requested release bumps once to 0.1.66. Windows chat-storage checks run alongside the production build; deliver the package only after the entire workflow succeeds. Compiler timing HTML is uploaded separately, and the portable script prints frontend/Rust/packaging seconds. This shortens the serial test/build path; a new runner or cache miss can still require a full compile.

Double-clicking a newer **release** `ctmcp.exe` while an older desktop owns the single-instance mutex schedules an embedded Windows update worker. It only selects known executable names with the exact product name, an older numeric version, a different path, the same Windows session, and a rechecked process identity. Equal/newer or unidentifiable instances are left alone. The worker preserves enabled MCP/Actions IDs, stops the old process tree, starts the new executable, verifies saved service versions, and attempts to restart the old executable if startup fails. Old files and workspace data are retained. Diagnostics are stored in the application's data directory as `startup-handoff.log`. Debug builds do not replace a running release.

`tests/desktop-startup-handoff.Tests.ps1` tests selection/snapshots without starting or stopping processes. The separate `.Integration.ps1` fixture refuses to run outside GitHub Actions and refuses runners with an existing desktop process. It compiles temporary dummy products to exercise process replacement and rollback. Actual wallpaper, window interactions and restored external tunnels still require the packaged Windows client.

### Clickable local paths

The chat renderer recognizes Windows drive paths, project-relative paths, inline-code paths with spaces, and labeled Markdown paths. File clicks reveal the file in Explorer on Windows (Finder on macOS); directories open directly. Linux opens the containing directory. Images retain preview and provide **Show in folder**, also available in the image dialog and attachment paths. HTTP(S) links are ordinary web links.

The local UI `reveal_path` action is scoped to a configured folder and an existing chat (including closed chats); it is not an MCP tool. Relative paths cannot traverse or follow a symlink outside that folder. Explicit absolute local paths can be revealed, matching the local directory browser. UNC/network paths, device paths, URI schemes and alternate streams are rejected by the backend. Clicking a script reveals it; it does not run the script. The file manager opens on the machine running the desktop/Node host.

Checks: `node --test tests/chat-local-links.test.mjs tests/chat-artifact-links.test.mjs`; Node `test/chat-reveal.test.mjs`; Rust `platform::reveal::tests`. Native Windows Explorer selection still requires an updated desktop binary; UI fixtures only verify the request, modal behavior and error presentation.

### Standalone EXE and personal Releases publishing

`desktop:portable` also emits `dist-portable/ctmcp-<version>-win64.exe` from the same production custom-protocol build. This file runs directly and does not require unpacking. ZIP output remains available for compatibility. The upgrade selector recognizes versioned and browser-numbered EXE filenames. Shutdown checks captured process identities and the single-instance mutex, not taskkill stderr/exit code alone. Previously responding local services must recover; saved but already unavailable services do not force rollback. Diagnostics are timestamped in `data/startup-handoff.log`.

The chat CI can publish the verified EXE as a GitHub prerelease after both Windows jobs pass. Add a repository Actions secret named `PERSONAL_RELEASE_TOKEN`, authorized for Contents read/write in this repository. The publishing script verifies `/user` is the repository owner and a human User before any write. No default GitHub/installation token fallback is used. With no secret, CI retains the artifact and prints a notice; no Release is published. The release starts as a draft, verifies/uploads exactly one versioned EXE, then becomes public. Its SHA-256 is in the release body; no ZIP or checksum file is uploaded to Releases. ZIP and checksum files remain internal Actions artifacts. Unexpected existing assets stop publication for manual inspection; the script never deletes them. Existing tags targeting another commit and existing assets with different/unverifiable digests cause failure instead of replacement.

### Concise connection and tool guidance (0.1.67)

The copied instructions lead with one loop: connect and verify the folder, open the supplied chat, wait for a message, work locally, persist the reply, and wait again. MCP initialize/discover and Desktop/Node chat responses use the same model-independent guidance. Tool descriptions state the action, essential guards and next call; detailed parameters remain in the schemas.

Progress uses `final=false`; completion or a question uses `final=true` (questions also `awaiting_user=true`). A final reply acknowledges only that user message. Idle waits and completed work continue the loop while the host allows execution. Deliverables must exist in the target workspace; attach them with `chat_upload` and reference the returned ID. Authentication secrets never enter project files or replies.

Check: `node --test tests/local-chat-prompt.test.mjs` covers JSON-safe IDs, OAuth/Bearer connection details and shared runtime instruction parity.

### Built-in local chat Skill

`skills/local-chat/SKILL.md` is the single maintained session guide. Rust embeds it in the binary and the Rust catalog exporter generates the same text for the Node package. `chat_open` returns it in `skill.text`; `resources/list` advertises `coding-tools://skills/local-chat`, and authenticated `resources/read` returns the identical Markdown. No workspace Skill installation is required. LF normalization keeps Windows and Linux exports identical.

The Chat page copies a short instruction referencing that returned Skill plus compact connection details. The Connection page retains the complete manual OAuth tutorial. The session-only copy is about 316 characters with UUID-sized IDs; total length depends on locale/auth/URL. The guide still consumes model context when read. Changes require upgrading the running server/client.

An idle wait is not a disconnection: the assistant must issue the next wait. If the original AI host stops executing, local queued messages cannot wake it; continue in that host with the same chat/folder and saved attachment ID, or obtain a new attachment after expiration. Do not attach another AI merely to diagnose an existing conversation.

### Conversation switching

Selecting a conversation now reads it immediately instead of waiting for the 1.5-second poll/list sequence. Revisited conversations render their in-memory snapshot immediately while refreshing. A first visit shows a loading state rather than the new-chat welcome screen. The component-local LRU cache is scoped by workspace/folder/chat and bounded to 12 entries and 4 Mi serialized characters; it does not persist message history in browser storage.

Read sequence guards discard late results after navigation or a successful mutation. Drafts remain scoped independently. Cached content stays visible during transient read failures. Verify with `node --test tests/chat-session-cache.test.mjs tests/chat-drafts.test.mjs`; browser checks also cover delayed reads, rapid A→C→A, identical chat IDs in separate folders, draft restoration and a send while an old read is pending. Synthetic 350ms reads measured about 368ms cold and 9ms cached, with zero welcome-screen frames, versus about 2s in the old polling path. These are fixture measurements, not Windows performance guarantees.

### 接入配对与聊天布局（0.1.67）

- 新对话仅显示“我们要做什么？”和居中输入框；对话标题、AI 状态、断开/接入、任务面板和设置合并在同一个顶部栏。
- 接入弹窗提供完整可复制提示词。点击“已发送”通过本地 `request_connection` 写入 `kind=connection_request` 的控制消息，展示无确定百分比的等待条。
- 控制消息保留在 JSON/Markdown（标记“接入请求”），从普通消息列表/大纲隐藏；不会生成假 AI 回复，也不占用第一条真实用户消息的自动标题。
- 配对只接受对应控制消息的实际最终回复“你好，有什么能帮到你？”。连接状态、其他会话/旧请求的回复及 final=false 都不算完成。异常问候可重新发起；关闭弹窗不关闭会话，重新打开可恢复未完成配对。保持 FIFO，接入请求不打断已经在执行的消息。
- `request_connection` 使用稳定 message_id，重试不重复写入；不同 ID 的并发请求复用未确认的控制消息，不改变现有 AI 租约。
- 主聊天区域（包括输入框外围）统一为不透明底色。原有 Windows Mica 保留；导航和顶部栏使用比会话侧栏高 12 个百分点的不透明度。Windows 壁纸效果仍需原生安装包实机确认。
- 任务面板四周留空、圆角阴影，可拖动左边缘调整宽度；聚焦边缘按钮后左右方向键每次调整 20px，Home/End 调整到边界。窄窗口使用受限宽度的悬浮层。
- 回归：`node --test tests/chat-connection.test.mjs tests/chat-session-cache.test.mjs tests/chat-drafts.test.mjs tests/local-chat-prompt.test.mjs`，Node chat.test.mjs 和 Rust tools::chat::tests 覆盖幂等/真实 MCP 回信及标题；浏览器检查配对重开/错误回复重试、窄屏、拖宽与切换缓存。

### 附件编号、引用与本地分块传输（0.1.67）

- 附件在会话内分别编号为 `图片1` / `文件1`，同名文件也有独立编号。编号依据不可删除的附件清单顺序稳定生成；移除草稿卡片不会删除已落盘文件或重新编号。
- 输入 `@` 显示当前草稿和已发送附件，`@图片` / `@文件` 或文件名筛选；方向键选择，Enter/Tab 插入。手动输入完整 `@图片N` 也会标蓝，发送时将对应附件元数据一并交付。IME 组合输入的 Enter 不发送。
- 草稿附件在文本上方显示缩略图和编号，右上角 × 移除；预览、删除和图片查看器按钮明确 `type=button`，不能触发发送。发送后的引用可打开对应图片或本地文件。
- 本地文件使用 `upload_chunk` 按 512 KiB 分块写入工作区 `mcp-assistant/chat-assets/<chat_id>/`，移除每条 5 个和每个 2 MiB 的本地上传限制；单次传输仍有边界，聊天 JSON/Markdown 只保存编号、路径、大小、SHA-256 等元数据。MCP `chat_upload` 的内联字节传输限制仍为 512 KiB，大型 AI 成果用 `source_path` 引用本地文件。
- 分块使用固定上传 ID、字节偏移和总大小。重试核对已写入部分，不追加重复字节；不同内容/名称/大小冲突拒绝，乱序偏移拒绝。完整落盘后流式计算校验并加入清单；未完成的 `.part`/`.upload.json` 不作为可发送附件。中断的大文件可留下这些临时文件。
- `read_attachment_chunk` 分块读取，浏览器显示前校验整文件摘要。为避免浏览器一次载入超大内容，超过 32 MiB 的文件通过“在文件夹中显示”本地打开；这是浏览器预览预算，不限制本地上传大小。磁盘容量、已有会话元数据总量和宿主资源上限仍适用。
- 项目详情改用 top-layer popover，位于按钮右侧；底部空间不足则向上，窄视口向内限制边界。项目卡片、会话切换和大纲浮层使用 .97 底色，保证文字可读。
- 验证：`tests/chat-mentions.test.mjs`、`tests/chat-drafts.test.mjs`、Rust `tools::chat::tests` 与 Node `chat.test.mjs`；浏览器用真实 Node chatUi 的隔离临时目录确认 3 MiB 图片、7 个附件、复制/手动引用、预览/删除无误发送和菜单位置。

### Release audit corrections (0.1.67)

- Group confirmation status uses the coordinator's final reply after collaborators finish, in the shared UI and both Markdown writers.
- Node folder opening waits for the spawn event and returns a structured HTTP error if no file manager exists. This repairs Node-specific child-process event handling; the desktop native opener already returns launch errors.
- Node startup errors retain the actual attempt diagnostics, and harness telemetry includes phases measured as zero milliseconds. Rust uses its own native startup/phase recording; no protocol or schema change is needed.
- Git test fixtures isolate their local author identity from host environment overrides. Frontend verification supports both JavaScript package-manager launchers and pnpm 12 native executables.
- Releases upload only `ctmcp-0.1.67-win64.exe`. SHA-256 stays in the release description. Automatic GitHub source archives may still be displayed by GitHub; they are not uploaded release assets.
