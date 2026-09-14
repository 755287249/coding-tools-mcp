# Node Agent Windows Portable

The Node Agent portable release produces two Windows x64 ZIP editions from one verified build and one production-dependency staging tree.

| Edition | Node.js included | Runtime requirement | Recommended use |
| --- | --- | --- | --- |
| `bundled-node` | Yes, `runtime/node.exe` and its license | None after extraction | Default distribution for users who want a self-contained package |
| `system-node` | No | Windows x64 Node.js 22 or later available as `node.exe` on `PATH` | Smaller package for managed machines that already provide Node.js |

Neither edition requires pnpm after extraction. Both include compiled server/UI assets, production-only dependencies, checksums, the project license, and double-click BAT launchers.

## Independent versions

| Component | Source of truth | Change when |
| --- | --- | --- |
| Node Agent | `packages/node-agent/package.json#version` | Agent behavior or shipped application code changes |
| Portable wrapper | `packages/node-agent/portable-version.json#version` | Runtime selection, launcher, edition set, archive layout, or portable-only behavior changes |
| Packaging Skill | `skills/node-agent-portable-packager/VERSION` | Skill instructions, validation rules, or release workflow changes |

Never synchronize these three versions automatically. A source-only Agent fix normally increments the Node Agent version. A launcher or archive-edition change normally increments only the Portable version. A Skill workflow change increments only the Skill version.

## Build

From the repository root, build both editions:

```powershell
pnpm run node-agent:portable
```

Build only one edition when needed:

```powershell
pnpm run node-agent:portable:bundled
pnpm run node-agent:portable:system
```

Equivalent package-level commands are `pnpm run portable`, `pnpm run portable:bundled`, and `pnpm run portable:system` from `packages/node-agent`.

To rebuild both editions while deliberately skipping the full Node Agent verification suite:

```powershell
pnpm --filter @coding-tools/node-agent run portable -- --SkipVerify
```

The default outputs are:

```text
dist-node-portable/ctnode-<agent-version>-p<portable-version>-win64.zip
dist-node-portable/ctnode-<agent-version>-p<portable-version>-sys-win64.zip
dist-node-portable/ctnode-win64/
dist-node-portable/ctnode-sys-win64/
```

ZIP filenames and their top-level directories remain versioned release artifacts. The two expanded directories are stable latest-build locations: every successful build replaces the matching directory instead of accumulating versioned expanded copies. Release packaging rejects package-relative paths longer than 180 characters to leave headroom for normal Windows extraction destinations.

## Archive contents

Both editions include:

```text
app/dist/                    compiled Node Agent and Management UI
app/node_modules/            production dependencies only
app/package.json
LICENSE.txt                  project license
start-node-agent.bat         validate runtime, start Agent, and open UI after health is ready
open-management-ui.bat       open the configured local UI port
update-handoff.ps1           verified whole-portable live-update handoff with health rollback
portable-manifest.json       edition/application/runtime/build metadata
SHA256SUMS.txt               per-file checksums
README-PORTABLE.txt          edition-specific operator instructions
```

Only `bundled-node` also includes:

```text
runtime/node.exe             bundled Windows x64 Node.js runtime
runtime/NODE-LICENSE.txt     Node.js license
```

The `system-node` launcher finds `node.exe` on `PATH` and rejects a runtime that is not Windows x64 or is older than the minimum major version. It does not silently fall back to a bundled runtime.

## Data directory

Both launchers use `%LOCALAPPDATA%\CodingToolsMCPNode` by default. This matches the Node Agent's Windows per-user default and lets a newly extracted portable version or a switch between editions reuse the existing configuration, encrypted secret store, primary/recovery key pair, tunnel identity, Harness state, history, and logs. The Agent recreates a missing `agent-secrets.key` from `agent-secrets.key.backup` only after the backup successfully decrypts the existing store.

An existing process, user, or machine `CTMCP_DATA_DIR` environment variable takes precedence. Set it to another absolute directory when isolation is required; point it at the extracted package's `data` directory for package-local behavior.

## Manifest contract

`portable-manifest.json` identifies the archive unambiguously with:

- `nodeAgentVersion` and `portableVersion`
- `edition`: `bundled-node` or `system-node`
- `nodeRuntimeBundled`
- `nodeRuntimeVersion` for the bundled edition, or `null` for the system edition
- `buildNodeVersion`, platform, architecture, minimum Node major, archive name, Git commit, and build timestamp

## Release checks

1. Run the Node Agent verification suite once before staging unless there is an explicit reason to use `-SkipVerify`.
2. Deploy the Node Agent workspace package into temporary common staging with `pnpm --filter @coding-tools/node-agent deploy --prod <staging>`.
3. Confirm `ws`, `pngjs`, and `jpeg-js` resolve from the deployed production staging tree.
4. Require the build runtime to be Windows x64 at or above the minimum major version.
5. Confirm `bundled-node` contains `runtime/node.exe` and its license.
6. Confirm `system-node` contains neither `runtime/node.exe` nor `runtime/NODE-LICENSE.txt`.
7. Confirm `update-handoff.ps1` exists at ZIP root and is covered by each edition's `SHA256SUMS.txt`.
8. Generate and validate a separate `SHA256SUMS.txt` for each edition, then report both ZIP SHA-256 values.
9. Extract each ZIP to a path containing spaces and smoke-test its launcher, `/health`, `/ui`, and static UI assets on isolated ports and data directories.
10. Do not put either Portable ZIP or a Node runtime inside the Skill ZIP; the Skill remains a small rules/workflow package.
