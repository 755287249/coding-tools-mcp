---
name: node-agent-portable-packager
description: Build, verify, version, tag, and package the Coding Tools MCP Node Agent as two Windows x64 portable ZIP editions: bundled-node with embedded Node.js and system-node without Node.js. Use when creating or reviewing Node Agent portable releases, enforcing release tag-to-commit provenance, changing launchers or archive layout, validating either artifact, or deciding which independent Node Agent, Portable, or Skill version to increment.
---

# Node Agent Portable Packager

Operate on the Coding Tools MCP repository that contains `packages/node-agent`.

## Version model

Keep these sources independent:

- Read the Node Agent application version from `packages/node-agent/package.json#version`.
- Read the Portable wrapper version from `packages/node-agent/portable-version.json#version`.
- Read this Skill version from `VERSION` beside this file.
- Derive the release tag as `node-agent-v<agent>-portable-v<portable>`; this tag is release provenance, not a fourth independent version.

Never synchronize or bulk-bump them. Increment only the component whose behavior changed. Read [references/packaging-rules.md](references/packaging-rules.md) before changing version, edition, or archive policy.

## Editions

- `bundled-node`: includes `runtime/node.exe` and `runtime/NODE-LICENSE.txt`; must not require system Node.js or a package manager after extraction.
- `system-node`: excludes the runtime directory and Node.js license; requires Windows x64 Node.js 22 or later as `node.exe` on `PATH`; must not require a package manager after extraction.

Both editions contain the same compiled application and production dependencies and use the same default per-user data directory.

## Workflow

1. Inspect Git status and preserve unrelated changes and generated artifacts.
2. Review the changes being released and choose independent version increments.
3. Run the normal Node Agent verification before creating the release identity.
4. Commit all release source and version changes. Require a clean worktree after the commit.
5. Create an annotated `node-agent-v<agent>-portable-v<portable>` tag on that exact commit.
6. Run `scripts/check-release-versions.ps1 -RepositoryRoot <repo>` from this Skill. Require the expected annotated tag to resolve to `HEAD`, a clean worktree, valid versions, and expected artifact names.
7. Run `pnpm run node-agent:portable` from the repository root to build both editions from the tagged commit. Use the focused `:bundled` or `:system` commands only when explicitly requested.
8. Inspect both ZIP names and manifests; require Agent version, Portable version, edition, release tag, and full Git commit in each artifact identity.
9. Confirm common application contents are present in both editions. Confirm only `bundled-node` contains `runtime/node.exe` and the Node license.
10. Extract both ZIPs to paths containing spaces. Start each with `start-node-agent.bat --no-browser` on isolated ports and data directories, then verify `/health`, `/ui`, and static UI assets.
11. Stop both smoke-test processes cleanly, validate every checksum entry, and compute both archive SHA-256 values.
12. Deliver both Portable ZIPs separately from the Skill ZIP.

## Release constraints

- Require Windows x64 and Node.js 22 or later for building and for the `system-node` runtime.
- Release packaging must run from a clean `HEAD` with an annotated `node-agent-v<agent>-portable-v<portable>` tag resolving to that exact commit. Do not use Desktop `v*` tags for Node portable releases.
- Deploy production staging once with `pnpm --filter @coding-tools/node-agent deploy --prod <staging>`, then derive both editions from the same common staging tree.
- Never copy source `node_modules` into an archive.
- Never include dev dependencies, build caches, repository history, credentials, user data, or existing configuration.
- Default runtime data to `%LOCALAPPDATA%\CodingToolsMCPNode` unless `CTMCP_DATA_DIR` is already set.
- Preserve support for `CTMCP_PORT`, command-line arguments, supervised restart exit code `75`, and `--no-browser` in both editions.
- The bundled launcher must use only its bundled runtime. The system launcher must validate the runtime found on `PATH` and must not claim to bundle Node.js.
- Generate checksums independently after each edition is finalized, including the root `update-handoff.ps1` used for verified live-update handoff and rollback.
- Place portable package contents directly at the ZIP root. Do not wrap them in an additional top-level directory.
- Do not embed Portable ZIPs or `node.exe` in this Skill; keep the Skill under the upload size limit.

## Outputs

Report:

- Node Agent version
- Portable version
- Skill version
- Release tag and full Git commit
- Build Node.js version and architecture
- Both edition names, ZIP paths, byte sizes, and SHA-256 values
- Runtime presence/absence checks for each edition
- Verification and smoke-test results for both editions
- Any intentionally skipped check
