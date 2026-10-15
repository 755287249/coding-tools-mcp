请连接我的工具：附件放同一目录，运行 `python mcp_relay.py ensure` 连接 MCP，按输出处理问题。用 `tools --json` 熟悉可用工具，用 `call 工具名 --args '{...}'` 调用；返回任务 ID 时运行 `job wait 任务ID`。工具在 MCP 内部，而非你的客户端内部，连上后使用，然后等待信息。
