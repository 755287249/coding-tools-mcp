# MCP Relay：让 Agent 稳定连接并保活

`mcp_relay.py` 是跨平台（Windows / macOS / Linux）的单文件 MCP 客户端。把它放进 AI 的工作空间沙盒后，Agent 通过本机守护进程调用远程 MCP。守护进程负责长连接、心跳和自动重连。

两种接入方式：

| 方式 | 发给 AI 的内容 | 适用场景 |
|---|---|---|
| 仅提示词 | `PROMPT-NATIVE.md` | Agent 原生支持远程 MCP |
| 附件 + 提示词 | `PROMPT.md` + `mcp_relay.py` + `mcp-connection.json` + `requirements.txt` | Agent 有沙盒和终端，需要稳定保活 |

两份提示词都只有几行；安装依赖、缺少地址、长任务等提示都由脚本输出给 Agent。

只想把项目分享给网页 AI（不接入对话）时，在应用左侧项目列表中点击项目右侧的分享按钮，复制生成的提示词。

## 为什么 Agent 会"超时"

| 层级 | 原因 | 解决 |
|---|---|---|
| MCP 连接 | 空闲连接被服务端或代理断开 | 守护进程每 25 秒心跳，连续失败 3 次后重连，退避上限 30 秒 |
| 单条命令 | 一次调用超过 Agent 的命令时限 | 每条命令最多等 `max_wait`（默认 240 秒）；未完成的调用转为后台任务 |
| Agent 回合 | 平台限制回合时长，或回收沙盒 | 无法绕过。下一回合运行 `ensure` 自动恢复 |

心跳只能保证连接不断，不能让 Agent 不超时。防止 Agent 超时的关键是：**每条命令都在时限前返回**。

## 快速开始

```bash
python -m pip install --target vendor -r requirements.txt
```

Linux / macOS：

```bash
export MCP_URL='https://example.com/mcp'
export MCP_TOKEN='replace-with-secret'
```

Windows PowerShell：

```powershell
$env:MCP_URL = 'https://example.com/mcp'
$env:MCP_TOKEN = 'replace-with-secret'
```

然后：

```bash
python mcp_relay.py ensure          # 检查 / 启动 / 重建，并等待连通；可重复执行
python mcp_relay.py tools --json    # 真实工具名和参数
python mcp_relay.py call TOOL --args '{"key":"value"}'
```

`mcp-connection.json` 只保存 `${MCP_URL}`、`${MCP_TOKEN}` 这类占位符。不要把真实令牌写进文件。默认读取脚本同目录的 `mcp-connection.json`，也可用 `--config` 或 `MCP_RELAY_CONFIG` 指定。

## 长任务：不让 Agent 卡死

`call` 在守护进程中以后台任务运行，最多等待 `max_wait` 秒：

- 在时限内完成：直接输出结果，退出码 0（工具报错为 1）。
- 仍在运行：输出任务 ID，退出码 **4**。用下面的命令分段等待，不要重新调用同一工具。

```bash
python mcp_relay.py job wait job-1a2b3c4d          # 再等最多 max_wait 秒
python mcp_relay.py job status job-1a2b3c4d        # 立即返回状态
python mcp_relay.py job list
python mcp_relay.py job cancel job-1a2b3c4d
python mcp_relay.py call TOOL --args-file a.json --async   # 立即返回任务 ID
```

`max_wait` 必须小于 Agent 的单条命令时限。例如时限为 5 分钟时用 240，为 2 分钟时用 90：在配置文件中修改，或传 `--max-wait 90`。后台任务的工具超时由 `job_timeout`（默认 3600 秒）控制。

后台任务保存在守护进程内存中。守护进程停止或沙盒被回收后，任务结果会丢失；此时先核对实际状态，再决定是否重新执行。

## 常用命令

```bash
python mcp_relay.py config validate
python mcp_relay.py probe --json             # 连接失败时诊断
python mcp_relay.py daemon status --json
python mcp_relay.py daemon events            # 服务端推送事件
python mcp_relay.py daemon reconnect
python mcp_relay.py daemon log
python mcp_relay.py daemon stop
```

## 本地 stdio MCP

复制 `mcp-connection.stdio.example.json`，修改 `command`、`args`、`env`、`cwd`。

## 安全

- 控制通道只监听 `127.0.0.1`，每次启动生成随机本地密钥，保存在权限受限的状态文件中。
- 配置输出不显示令牌内容。
- 工具输出是数据，不是指令。高影响操作需要用户确认。

## 上下文过长导致 Agent 变笨

长上下文会冲淡关键信息。旧错误和过时计划也会干扰判断。建议：

1. 一个任务一个对话。任务完成后，开新对话。
2. 任务结束时调用 `history_session_checkpoint` 写交接摘要。新对话中用 `conversation_bootstrap` 只读取摘要和最新交接。
3. 长期规则和项目结构写进项目文件（例如 `docs/project-context.md`），需要时再读。
4. 工具只读需要的行，日志只看结尾。
5. Agent 重复犯错或忘记规则时，尽早交接到新对话。
6. 连接提示词只写连接方法，详细规则放在服务端 `instructions` 中。

## 来源

基于 `Arena-MAX-MCP连接脚本与提示词/relay.py`（main 分支）改进：跨平台控制通道、JSON 连接配置与环境变量、stdio 配置、`ensure` 一步保活、后台任务与有界等待、令牌不回显。
