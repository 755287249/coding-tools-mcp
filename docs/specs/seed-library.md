# Seed library v1

The desktop and Node agent own a per-folder registry. Browser provisioning is a
replaceable adapter. A seed serves at most one local conversation during its
lifetime; replacement retains the local conversation and all durable history.

## Contract

- Management uses authenticated local-chat `seed_*` actions. Batch credentials
  are returned once to the UI, copied into the browser adapter and kept in memory.
  Only hashes are persisted. The account label, exact repository ID and branch
  are mandatory; a missing repository never falls back to another repository.
- `/mcp/seeds/{folder_id}/{seed_id}` is a separate, capability authenticated MCP
  endpoint. Enrollment expires after one hour. Initialization exchanges it for a
  seed-specific access credential (seven days); a one-minute retry window makes
  lost initialization responses recoverable. Ordinary MCP OAuth is unchanged.
- Before assignment only discovery and `seed_wait` are available. A successful
  wait establishes readiness. Assigned calls are restricted to the configured
  folder, current local chat and attachment. Replaced/retired credentials cannot
  run tools. The normal tool profile and permission checks still apply.
- Registry and chat mutations share the existing cross-process chat lock.
  Assignment is recorded in the chat first and reconciled from that owner on
  recovery. No arbitrary JSON/Markdown edits by the AI are part of the protocol.
- Allocation leaves the chat offline until the assigned seed acknowledges with
  `chat_open`; project calls before acknowledgement are rejected.
- A folder is bound to one exact host repository ID and branch in v1, including
  retired history. Use a separate folder for a different repository/branch.
- New work-mode conversations opt into auto assignment when the library is
  enabled. Explicit detach disables auto assignment; close/archive/group mode
  and waiting for a user decision prevent automatic takeover.
- A seed is suspected after ten minutes without activity and eligible for
  replacement after another two minutes, only with no in-flight or retained
  operation. Timeout retirement is recorded as a timeout, not proven host death.
  One-way retired identities cannot reconnect and resume writes.
- Unresolved operations survive restart and block automatic replacement. The
  UI reports the reason. Successful terminal process polling clears its marker;
  unknown execution outcomes are never silently retried.
- The standby seed's independent waits drive allocation and recovery even when
  the UI is closed. Zero standby seeds means visibly waiting for capacity.
- Recovery returns the existing chat and its normal bounded context plus a
  reminder to inspect history, files and retained operations before resuming.
  Host sandbox memory/uncommitted files are not transferred. Work must be saved
  through the selected MCP workspace. Host execution limits remain applicable.

## Browser adapter

Use the current browser account, exact repository and branch. Create sequentially
with stable client request IDs and validate tRPC error envelopes and task IDs.
Uncertain network results stop that batch for reconciliation; never automatically
recreate an uncertain task. Store only nonsecret task receipts. Do not implement
account registration, eligibility activation or automatic billing/consent.

## Required verification

Both runtimes: batch bounds, credential exchange/expiry, project isolation,
readiness, concurrent nonduplicate assignment, preserved chat/history, explicit
pause, no takeover during operations, old-owner rejection, cleanup statistics,
corrupt/symlink storage, retry behavior. Browser adapter: tRPC errors, missing IDs,
exact repository payload, uncertain submission and credential-free receipts.
Run UI checks/build, repository mandatory parity/version gates and Windows build.
Live host capacity and private webpage API compatibility require a browser account;
synthetic tests must not be reported as a live fifty-task verification.

The exchange retry receipt exists only in server memory. Restart after redemption
requires the already received access token; a lost initialization response across
restart requires a new enrollment. The access token is independently random and
cannot be derived from the enrollment ticket. Retained process markers are cleared
only after the server observes a terminal process result in the original seed's
execution context. Missing sessions and ambiguous in-flight markers fail closed.
