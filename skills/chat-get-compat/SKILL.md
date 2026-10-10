# GET/file conversation trial v1

This is a limited GET/file adapter, not native MCP registration.
Only this conversation and its configured folder are authorized. No file reads/edits, commands, uploads, other conversations or tool forwarding are available through this adapter.
Keep the access key and authenticated URL only in the host's private context/memory. Never write them to a file, reply, command output or source code. Downloaded JSON never returns them.
Use your GET-download tool, then Read the resulting JSON file. Delete the temporary response after reading. Respect the host's permissions and execution limits.
Each request uses the same endpoint and key, plus a fresh nonce to avoid cached downloads. URL-encode every query value. Never invent a new key or identity.
First op=info, verify chat_id and workspace_folder.id/path against the user's target. Then op=open. Read every instruction_lines entry, including after compaction; op=open safely resumes the same attachment kept by the server.
Next op=wait&timeout_ms=10000. One outer tool invocation issues one wait. Each idle result requires another independent wait; idle and completed tasks do not end this conversation. Wait returns the actual current message, not a history summary.
For kind=connection_request, send the greeting “你好，有什么能帮到你？” as a final reply, then wait again.
Reply with op=reply and data=<URL-encoded JSON object>: {"message_id":"unique-stable-id","reply_to":"actual-delivered-id","text":"reply","final":true}. Optional awaiting_user=true asks a question. Questions use plain text in this trial.
Each reply text is at most 2000 UTF-8 bytes. Split longer answers into ordered progress replies (final=false), each with its own stable message_id, then a final reply. Do not split/rewrite a request whose outcome is uncertain.
Only persisted=true acknowledges a reply. On uncertain result, retry the identical data and message_id with a fresh nonce. Never redo completed side effects. final=true acknowledges only this message; immediately make another wait request.
text_lines are consecutive text fragments: join them with no separator to recover the exact message, including newlines. The JSON is pretty printed and fragments are bounded so file readers can read all lines without truncation.
Attachments are metadata only and cannot be downloaded using this authorization. State the limitation if the task needs their contents. Tasks requiring unavailable capabilities need the user's supported environment.
HTTP 200 alone is not success: parse ok/status/error. Closed or invalid/revoked/expired authorization stops this adapter; ask the owner for a new grant through the client. Temporary network errors use 1, 2, 4 seconds backoff, then 30 seconds; finish the previous request before retrying.
Authorization expires after 30 minutes and is revoked by the owner, disconnect, close, mode change, or process restart. It never renews automatically. The owner can issue another grant from the conversation UI.
All communication goes through replies in this conversation. Continue the wait → work → reply → wait cycle within actual host limits; the adapter cannot keep a stopped model running.
