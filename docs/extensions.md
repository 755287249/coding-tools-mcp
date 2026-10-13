# Hook and MCP extensions

The Node Agent can discover Hook and MCP configuration from Claude Code and Codex, show the discovered entries in the management UI, and let the user explicitly enable individual entries for this Agent workspace profile.

## Safety model

Hook and MCP execution is **opt-in in Node Agent**. Discovery alone never runs a Hook, starts an MCP process, or connects to a remote MCP endpoint. Enabling or disabling an item only changes the Node Agent configuration; the source Claude/Codex file is never rewritten.

The persisted Node Agent state is:

```json
{
  "extensions": {
    "hooks": { "active": true, "enabled": ["opaque-hook-key"] },
    "mcp": { "active": true, "enabled": ["opaque-mcp-key"] }
  }
}
```

Enable keys are bounded opaque identifiers and do not embed Hook commands, URLs, credentials, or absolute user-home paths.

`extensions.hooks.active` and `extensions.mcp.active` are independent master switches and default to `true`. They do not opt discovered entries in by themselves: individual Hooks and MCP servers still require their opaque key in the corresponding `enabled` list. Turning a master switch off preserves that list and the discovered inventory. Turning Hooks off stops Hook execution; turning MCP off disconnects active external servers and removes their dynamic tools. Re-enabling restores the previously selected entries.

## Hook discovery

Node Agent scans these sources:

| Provider | Scope | Sources |
| --- | --- | --- |
| Claude Code | user | `~/.claude/settings.json` |
| Claude Code | workspace | `.claude/settings.json` |
| Claude Code | local | `.claude/settings.local.json` |
| Codex | user | `~/.codex/hooks.json`, `~/.codex/config.toml` |
| Codex | workspace | `.codex/hooks.json`, `.codex/config.toml` |

Source-level disable controls are preserved. For example, Claude `disableAllHooks` and Codex `features.hooks = false` prevent Node Agent from enabling affected Hooks.

Node Agent currently executes these Hook lifecycle events:

- `SessionStart`: runs once for each conversation/workspace pair when that workspace first becomes active for a tool call. Session matchers receive `startup` as the source.
- `SessionEnd`: runs for active conversation/workspace pairs when the Node Agent extension registry shuts down. This is a runtime-shutdown boundary because MCP clients do not provide a portable conversation-close signal.
- `PreToolUse`: may allow, rewrite tool input, add context, or block a tool call.
- `PostToolUse`: runs after a successful tool call and may return feedback/context.
- `PostToolUseFailure`: runs after a failed tool call and may return feedback/context.

Supported Hook handler types are `command` and `http`. Other discovered handler types or events remain visible in inventory but are marked unsupported and cannot be enabled by Node Agent.

Command Hooks receive a JSON event payload on stdin. HTTP Hooks receive the JSON event payload with `POST`. `SessionStart` and `SessionEnd` payloads include the conversation session id, workspace root cwd, event name, and source. Hook execution is bounded by timeout and output limits.

## MCP discovery

Node Agent scans these MCP sources:

| Provider | Scope | Sources |
| --- | --- | --- |
| Claude Code | user | `~/.claude.json` `mcpServers` |
| Claude Code | local | matching `~/.claude.json` project `mcpServers` |
| Claude Code | workspace | `.mcp.json` |
| Codex | user | `~/.codex/config.toml` `mcp_servers` |
| Codex | workspace | `.codex/config.toml` `mcp_servers` |

Claude MCP precedence is local, then workspace/project, then user for the same server name within a workspace folder. Codex workspace configuration similarly shadows user configuration for the same server name. Claude and Codex servers are namespaced separately so equal names from different providers can coexist.

Node Agent currently proxies:

- `stdio` MCP servers using persistent JSON-RPC child processes. On Windows, explicitly enabled `.cmd` and `.bat` launchers are started through the command shell; native executables remain shell-free.
- Streamable HTTP MCP servers using HTTP POST, including JSON and `text/event-stream` responses and MCP session IDs.

Legacy SSE and WebSocket entries may be discovered but are currently marked unsupported.

### Adding MCP servers from the UI

Customize → Plugins → Add → "MCP servers" accepts pasted JSON or a picked file in any of these shapes: `{"mcpServers":{...}}` (Claude/Cursor), `{"servers":{...}}` (VS Code), `{"mcp":{"servers":{...}}}`, a bare `{"name":{"command"|"url"...}}` map, or one server object `{"command"|"url"...}` (a name field appears for this case). The servers are merged into the selected project folder's `.mcp.json` (`mcpServers`; other entries are kept, an invalid existing file is never overwritten), then the MCP master switch and the new servers are enabled automatically.

Backends: desktop `add_workspace_mcp_servers` (Tauri command) and Node Agent `POST /admin/api/workspaces/{id}/extensions/mcp` with `{folderId, servers}`. The desktop inventory now connects enabled servers (up to 25 s) before reporting status, so connection errors and tool counts appear in the detail view; when the workspace MCP service is stopped the server shows a "start the workspace" hint.

When an MCP server is enabled, Node Agent initializes it, reads `tools/list`, and merges those tools into the Agent's MCP `tools/list`. Proxy tool names are namespaced by provider, folder/scope, server, and tool name. The Agent toolset revision changes when the enabled external tool catalog changes, so clients can refresh their catalog.

External MCP tools are conservatively disabled in Node Agent `read-only` permission mode. Enabling a server is an explicit trust decision for other modes; Hooks still run around proxied tool calls.

## Management UI and privacy

Workspace management exposes separate **Hooks** and **MCP** tabs. Each tab has its own master switch. Each discovered item shows its provider, scope, privacy-safe source path, supported state, and individual selection switch. When a master switch is off, the inventory and saved individual selections remain visible but the individual switches are temporarily disabled. MCP entries also show transport, connection state, and discovered tool count.

The management API does not return MCP environment values, HTTP header values, bearer tokens, complete Hook command lines, or absolute user-home paths. User source paths are represented with `~/...`; command display is reduced to the executable basename, and remote endpoints are reduced to their origin.

## Refresh behavior

Discovery is refreshed while the Agent is running. Hook individual or master enable changes take effect immediately. MCP individual or master enable changes connect or disconnect immediately, and edits to an enabled MCP source configuration cause the existing connection to be replaced so the new command, endpoint, or arguments are used.

Workspace-folder hot apply also updates extension discovery roots. No Agent restart is required solely for a Hook or MCP individual/master enable change.
