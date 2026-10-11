<!-- parity-id: NP-017 -->
<!-- parity-status: done -->
# NP-017 — Canonical workspace path containment

- Priority: P0
- Area: security
- Status: done

## Gap

Node currently performs primarily lexical path containment. Existing direct-path tools can accept an absolute path that is already inside the workspace and can follow a workspace symlink to a target outside the configured root. The audit reproduced both external reads and an external write through `edit_file`.

## Rust evidence

- `src-tauri/src/tools/workspace.rs`
- `src-tauri/src/tools/file.rs`
- `src-tauri/src/tools/patch.rs`
- `src-tauri/src/tools/image_tool.rs`

## Node current state

- `packages/node-agent/src/workspace.ts`
- `packages/node-agent/src/fileTools.ts`
- `packages/node-agent/src/imageCodec.ts`
- `packages/node-agent/src/editRecovery.ts`
- `packages/node-agent/src/fileOpsTools.ts`
- `packages/node-agent/src/formatterTools.ts`
- `packages/node-agent/src/gitTools.ts`
- `packages/node-agent/src/processes.ts`
- `packages/node-agent/src/tools.ts`
- `packages/node-agent/test/basic.test.mjs`
- `packages/node-agent/test/pathContainment.test.mjs`

## Required implementation

Introduce one canonical resolver contract for existing reads, existing writes, and create targets. Reject absolute user paths before normalization, resolve existing ancestors through `realpath`, verify the canonical target remains under the canonical workspace root, and retain safe create semantics when the leaf does not exist. WSL containment must continue to use Linux path semantics and must not regress NP-015.

## Acceptance checklist

- [x] Absolute POSIX, Windows drive, UNC, and extended paths return `ABSOLUTE_PATH_DENIED` even when they point inside the workspace.
- [x] Parent traversal continues to return `PATH_OUTSIDE_WORKSPACE`.
- [x] Direct file and directory symlinks that escape the root return `SYMLINK_ESCAPE`.
- [x] `read_file`, `read_many`, `view_image`, `edit_file`, and `edit_many` cannot read or modify an external symlink target.
- [x] Create and replace operations verify the canonical parent and cannot escape through an ancestor symlink.
- [x] In-workspace symlinks remain usable where Rust permits them.
- [x] Error categories, retryability, and details match the Rust workspace contract.
- [x] Host, case-sensitive WSL, formatter, Git, and walker regressions pass.

## Dependencies

None. This is the first follow-up item because it closes a reproduced write escape.

## Verification

Implemented `pathContainment.test.mjs` with absolute POSIX/drive/UNC/extended paths, parent traversal, direct and ancestor symlinks, safe internal aliases, missing create leaves, patch tools, formatter, Git, cwd, process workdir, and source immutability checks. The complete Node Agent verification, Rust catalog check, Desktop compatibility check, roadmap validation, native dependency scan, and repository test suite pass.

The global connection extension is tracked separately in NP-029.
