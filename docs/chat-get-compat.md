# GET/file conversation trial (v1)

The connection dialog offers **GET/file chat trial → Authorize this conversation**.
The authenticated owner issues an independent 30-minute capability and copies the
instructions. It is never an OAuth password/token or a pairing status ticket.
Use **Revoke GET authorization** to revoke it, including after reopening the dialog.
Disconnect also revokes access. Reissuing after expiry requires disconnecting the
old AI. Restarting the process loses all capabilities.

This adapter does not register native MCP or provide workspace file access,
commands, uploads, general tools, group chats or discussion deliveries. The
original MCP/OAuth flow is unchanged. The host must allow URL downloads to the
server and must itself keep calling tools; this cannot wake a stopped model.

## Contract

Root and configured public-prefix routes expose `/mcp/chat-compat` with GET only.
HEAD and other methods fail before side effects. Required query fields are `key`,
`nonce` (fresh per download), and `op`. Duplicate/unknown fields are rejected.

| op | Extra fields | Result |
| --- | --- | --- |
| info | none | Exact chat ID and configured folder ID/name/path |
| open | none | Server-held attachment/resume and full `instruction_lines` |
| wait | optional `timeout_ms`, integer 0–10000 | idle or current message |
| reply | `data`, URL-encoded JSON | `persisted:true` acknowledgement |

Reply data accepts only `message_id`, `reply_to`, `text` (1–2000 UTF-8 bytes),
`final` (required boolean), and optional boolean `awaiting_user`. Existing chat
idempotency/conflict rules apply. Longer answers use separate progress replies
and one final reply. Retry uncertain writes with the identical data and IDs.
The URL is limited to 16 KiB. One request per grant may run at a time (409 if busy).

Downloaded responses are pretty JSON, with 120-codepoint `text_lines` fragments
joined without separators. They never contain the grant key or authenticated URLs.
Responses use no-store/private, no-referrer, and nosniff. Download files contain
conversation data and should be removed after reading. The copied URL/key remains
a private capability in the host context; URL intermediaries must be trusted.

## Boundary and implementation

Grants live in a bounded in-memory registry, bound to profile, canonical folder
path, folder ID, chat ID and a persisted non-secret generation ID. Management is
available only through authenticated local UI/management. No public route/tool can
issue a grant. Detach, close and mode changes invalidate the generation; deletion
removes the target. A current native attachment cannot be taken over, even after
its heartbeat expires. Native and GET calls use the same chat writer lock and
reply validation. GET calls skip discussion-inbox synchronization and reject
sessions containing discussion deliveries.

Every operation checks the configured folder and exposed chat tool profile.
Native chat operations do not require generic write/exec permissions; the same
rule applies to custom security settings here. Enabled hooks are unsupported in
this trial: argument rewriting must never expand a capability's scope. Neither
hook subprocesses nor generic MCP dispatch are reachable through this adapter.
The shared Markdown instructions are compiled into Rust and generated into Node.

Validation covers actual HTTP GET-to-file/read, auth scope, HEAD, prefixes,
revocation/expiry, concurrent waits, duplicate replies, payload constraints,
Unicode response fragments and matching packaged instructions. Windows CI runs
the Rust transport tests and copied-prompt contract as well as portable builds.
