# Packaging Rules

## Contents

1. Version ownership
2. Git release identity
3. Required edition layout
4. Build invariants
5. Launcher contract
6. Verification contract
7. Release decision examples

## 1. Version ownership

Treat all versions as semantic versions but keep them independent.

| Version | Source | Owns |
| --- | --- | --- |
| Node Agent | `packages/node-agent/package.json#version` | Server, tools, contracts, Management UI, and application behavior |
| Portable | `packages/node-agent/portable-version.json#version` | Runtime editions, BAT launch behavior, staging process, archive layout, manifest, and checksums |
| Skill | Skill root `VERSION` | ChatGPT workflow, review rules, validation guidance, and bundled helper scripts |

Do not change the Desktop Client version as part of a Node Agent portable release. Do not change `codingTools.clientVersion` unless shared Rust/Desktop contracts were deliberately synchronized.

## 2. Git release identity

Every deliverable Node portable release must be tied to one immutable repository commit.

- Derive the expected tag from both independent package versions: `node-agent-v<nodeAgentVersion>-portable-v<portableVersion>`.
- Reserve plain `v*` tags for Desktop releases; never use a Desktop tag as Node portable provenance.
- Commit all release source and version changes first, then create an **annotated** Node portable release tag on that exact commit.
- Require a clean worktree before packaging. Generated ignored output may exist, but tracked or untracked source/config changes are not allowed.
- Require `refs/tags/<releaseTag>` to be an annotated tag object and require its peeled commit (`<releaseTag>^{commit}`) to equal `HEAD`.
- Use the same release tag and full 40-character commit SHA for `bundled-node` and `system-node` artifacts.
- Record both `gitTag` and `gitCommit` in `portable-manifest.json`; neither value may be `unknown` for a deliverable release.
- If either Node Agent or Portable version changes, create a new tag. Never move or reuse an existing release tag.

## 3. Required edition layout

A normal release produces two ZIPs. The portable contents must live directly at the ZIP root; do not add a wrapper directory around the package.

Both editions require these ZIP-root entries:

```text
app/dist/
app/node_modules/
app/package.json
LICENSE.txt
data/
logs/
start-node-agent.bat
open-management-ui.bat
update-handoff.ps1
README-PORTABLE.txt
portable-manifest.json
SHA256SUMS.txt
```

Only `bundled-node` requires:

```text
runtime/node.exe
runtime/NODE-LICENSE.txt
```

The `system-node` edition must not contain either runtime file.

## 4. Build invariants

- Build deliverable ZIPs only from the clean, tagged `HEAD` defined by the Git release identity contract.
- Compress the contents of each finalized edition directory directly into the ZIP root; do not archive the edition directory itself as a wrapper folder.
- Build `dist` once before staging.
- Create common staging in a new temporary directory.
- Deploy the workspace package once with `pnpm --filter @coding-tools/node-agent deploy --prod <staging>` to create common production staging.
- Resolve `ws`, `pngjs`, and `jpeg-js` from the staged application.
- Derive both edition directories from the same common staging content.
- Bundle the selected Windows x64 `node.exe` and its license only in `bundled-node`.
- Include the repository license in both editions.
- Use manifest schema version 3 and record edition, runtime-bundled status, Git release tag, full Git commit, build timestamp, build/runtime versions, architecture, minimum Node major, archive name, and all version-source paths in each manifest.
- Compute independent file checksums before compression and independent ZIP checksums after compression.
- Do not use an existing portable output folder as staging because a running executable may be locked.
- Publish the versioned ZIP before refreshing the stable expanded folder; if that expanded folder is locked by a running Agent, keep the current ZIP and warn instead of failing packaging.

## 5. Launcher contract

Both `start-node-agent.bat` variants must:

- Resolve all paths relative to `%~dp0`.
- Default `CTMCP_DATA_DIR` to `%LOCALAPPDATA%\CodingToolsMCPNode` without overriding an existing value.
- Use a launcher-only `CTMCP_LAUNCH_PORT` default of `3789` for health checks, port preflight, and browser opening. If the caller explicitly supplies `CTMCP_PORT`, mirror that value into `CTMCP_LAUNCH_PORT`, but do **not** synthesize `CTMCP_PORT` in the launcher: it is an Agent configuration override and would otherwise force every saved workspace onto the same port.
- Pass `--restart-supervised` explicitly to `app\dist\cli.js` and set the child-scoped `CTMCP_RESTART_SUPERVISOR=active-v1` contract in the launcher. The Agent enables Web UI restart only when both are present, so a detached/direct Node process cannot accidentally advertise restart support from the flag alone.
- Forward remaining command-line arguments to `app\dist\cli.js` after the supervision flag.
- Restart when the Agent exits with code `75`.
- Support `--no-browser` as a launcher-only first argument.
- Open `/ui` only after `/health` is ready.

Edition-specific runtime rules:

- `bundled-node` must use only `%PORTABLE_ROOT%\runtime\node.exe` and fail clearly when it is missing.
- `system-node` must locate `node.exe` on `PATH`, require Windows x64 Node.js at or above the configured minimum major, and fail clearly otherwise.

`open-management-ui.bat` must respect `CTMCP_PORT` and perform no server mutation.

## 6. Verification contract

Before delivery:

1. Confirm `HEAD` is clean and the expected annotated `node-agent-v<agent>-portable-v<portable>` tag peels to exactly `HEAD`.
2. Confirm the normal Node Agent verification suite passed once, or identify why it was skipped.
3. Inspect both expanded artifacts and reject unexpected dev dependencies, `.git`, source trees, credentials, or user data.
4. Confirm both manifests contain the same expected `gitTag` and 40-character `gitCommit`.
5. Confirm runtime files are present only in `bundled-node`.
6. Extract both archives to fresh directories whose paths contain spaces.
7. Start each with `--no-browser` on a distinct unused port and isolated data directory.
8. Poll `/health`, require HTTP success, and confirm the reported Agent version.
9. Fetch `/ui` and its JavaScript/CSS assets successfully for both editions.
10. Confirm `update-handoff.ps1` is present at ZIP root and covered by `SHA256SUMS.txt` in both editions.
11. Stop both processes and verify no child or test port remains.
12. Validate every line in each `SHA256SUMS.txt`.
13. Record both final ZIP sizes and SHA-256 values.

## 7. Release decision examples

- Agent bug fix only: increment Node Agent patch; leave Portable and Skill unchanged.
- BAT quoting fix affecting both editions: increment Portable patch; leave Node Agent unchanged.
- Add or remove a runtime edition: increment Portable minor or major according to compatibility impact and update the Skill independently.
- New bundled Node major: increment Portable minor; Node Agent changes only when its runtime requirement changes.
- Improved Skill checklist only: increment Skill patch; rebuild neither application nor Portable unless explicitly requested.
- Breaking archive or launcher contract: increment Portable major and update the Skill independently.
