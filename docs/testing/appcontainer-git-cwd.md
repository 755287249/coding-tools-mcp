# Git current directory inside Windows AppContainer

Git for Windows queries `GetFinalPathNameByHandleW(VOLUME_NAME_DOS)` in
`mingw_getcwd`. AppContainer can deny the mount manager lookup even when the
workspace itself is accessible. Git then uses `GetLongPathNameW`, which can need
to enumerate otherwise private ancestor directories. Traverse/attribute ACEs
alone do not make that fallback work.

The Desktop provider and Node helper give a directly launched Git executable a
temporary DOS drive pointing at its workspace. Only the actual process cwd is
translated; command admission, workspace identity and ACL grants use physical
paths. Nested cwd remains nested under the alias. This lets Git's fallback query
only directories within the granted workspace. No ancestor listing permissions
are added.

The alias is a temporary Object Manager symbolic link created without
`OBJ_PERMANENT`, `OBJ_OPENIF` or handle inheritance. Its handle is held until the
sandboxed process lifetime ends. Closing the handle, including owner process
termination, removes the name. Allocation skips occupied drives and atomically
rejects name collisions; it never replaces another mapping. The host checks the
alias resolves to the expected physical workspace before launch, and rejects
cwd outside that workspace.

## Limits

- Local drive workspaces only; UNC workspaces need a separate network contract.
- Free drive letters are finite; exhaustion returns an explicit launch error.
- This adapter covers direct Git launches. Git started indirectly by another
  program does not get a separate alias.
- Git commands that print absolute paths can print the temporary drive. Those
  paths are valid only during the command's process lifetime; use the configured
  physical workspace path for later MCP file calls.
- Windows runtime validation is required; Linux compilation cannot establish
  Object Manager or AppContainer behavior.

## Verification

The Windows source upgrade workflow runs the Node alias lifecycle test, Rust
alias exclusivity/containment/drop/owner-termination tests, the existing nested
Git status regression, and the full provider isolation suite. Its existing NUL
capability fixture is confined to the disposable CI runner. The gh, startup,
and portable-build gates remain required.

Source references:

- Git for Windows `compat/mingw.c`, `mingw_getcwd`.
- [Deno AppContainer canonicalization report](https://github.com/denoland/deno/issues/35543).
- [Stock Git cwd drive-alias investigation](https://github.com/unslothai/unsloth/pull/12164).
