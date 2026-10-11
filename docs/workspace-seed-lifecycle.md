# Workspace removal and retained seed pools

Removal stops MCP and Actions without holding the runtime mutex across shutdown. Duplicate removal and stale runtime starts are fenced. Tunnel lock acquisition has a 10-second deadline; child cleanup retains ownership and uses the existing per-child shutdown deadlines. A failed removal keeps the profile available for retry. Node stops only the secondary workspace being removed; its primary management workspace is not removable.

Seed pools are folder scoped. Removal pauses automatic assignment, detaches the seed-owned conversation attachment, and clears the seed assignment. Conversations, messages and project files stay on disk. Existing credentials retain their original expiration; they are not renewed. Unresolved tool/process ownership prevents release. Explicit conversation detach/retirement semantics stay unchanged.

Retained seeds are **offline**, not advertised as reachable standby. Restoring the project folder and its original reachable endpoint/folder identity can reconnect eligible seeds; enable automatic assignment when ready. A new workspace/folder ID or endpoint needs a new enrollment URL. This is not a global route-independent pool. Routine inactive cleanup preserves the retained pool; users can explicitly retire and purge records.

UI removal fences delayed runtime/navigation responses and lets the dialog close while cleanup continues. Failure is reported with retry available from the preserved workspace profile. Disk persistence failures may leave services stopped or some folder assignments released; retry is intentional and safe.
