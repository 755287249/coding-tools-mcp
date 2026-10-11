# Windows AppContainer runtime repair

## Scope

The Desktop provider grants the current sandbox package SID traversal, read-attributes
and synchronize access on canonical workspace ancestors, without inheritance.
This supports Node realpath and Git/MSYS current-directory resolution. It grants
no directory listing, file-content access or writes on those ancestors. In-process ancestor ACL installation and cleanup are serialized across concurrent
provider leases. Temporary grants are held by the process lease and revoked when it drops. Preparation errors
remain fail-closed and identify the ancestor that could not be configured.

GitHub CLI uses the same concrete PATH executable identity check as Git.
A same-name executable elsewhere does not match that trusted runtime.
HOME, USERPROFILE, APPDATA and LOCALAPPDATA remain sandbox-local. Executable
support alone does not provide access to the user's GitHub authentication.

## Node Agent parity

The existing `packages/node-agent/src/sandboxAppContainer.ts`
`GrantAncestorTraversal` already applies Traverse and ReadAttributes with no
inheritance. .NET allow rules add Synchronize; the Rust mask includes it explicitly.
This change brings the Desktop workspace ancestor behavior into alignment.
Node Agent's normal executable launch path already passes resolved programs to
its helper; it does not have the Desktop trusted-runtime whitelist that omitted gh.
No Node implementation change is required for these Desktop repairs.

## Validation status

This is the 0.1.143 source candidate, not a verified Windows release. The running v0.1.142
client has not been replaced.

- Passed: guarded source edits; CRLF preservation.
- Passed: version 0.1.143 consistency via Node 24.15.0 with preserve-symlinks and
  preserve-symlinks-main flags (same version:check script).
- Passed: rustfmt parsing/format checks on added helper and regression test fragments.
  These are syntax/format checks, not Rust compilation or Windows execution.
- Blocked: Cargo and pnpm are not found on the running MCP client's PATH.
- Outstanding: Windows provider tests, Node parity gate, node-agent:verify-repo,
  full source formatting, portable build, personal GitHub identity and push.
- Parity check started through Node but its final result could not be retrieved:
  the gateway returned "Process does not belong to this seed". No pass is claimed.

## Windows validation

Use a prepared Windows development environment with this repository's Node,
pnpm and Rust/MSVC toolchain, plus Git, Node and GitHub CLI on PATH.
The process configuring AppContainer ACLs must have permission to update the
workspace ancestors' DACLs, including the volume root. A normal account may lack
that permission. Do not treat a successful source edit or compilation as proof
that this requirement is met. The new error reports the exact failed path.

From the repository root:

```powershell
pnpm run version:check
pnpm run node-agent:parity:check
pnpm run node-agent:verify-repo
pnpm run rust:fmt:check
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --lib tools::sandbox::appcontainer::provider::tests -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --lib gh_uses_exact_identity_and_isolated_configuration -- --ignored
```

The added tests cover minimal ACL masks, exact gh identity, nested realpath and
parent/sibling isolation, temporary grant removal, Git status in a real repository,
gh launch with isolated configuration, and concurrent lease cleanup. Existing provider tests retain the
repository-metadata and cross-workspace isolation checks.

After successful checks, build using the repository's portable-build procedure.
Only after switching to the tested executable can the MCP runtime be rechecked.
Verify the personally authenticated GitHub account before any external write.
