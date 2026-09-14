import { createHash, randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import type { EditProposalRecord, JsonObject, ToolContext } from './types.js';
import { decodeTextBuffer, TextDecodingError } from './textCodec.js';
import {
  relativeInside, rejectDirectWriteSymlink, resolveExistingWritePath, resolveWritePath,
  rootAndCwd, validateWorkspaceUserPath, WorkspacePathError
} from './workspace.js';
import { currentFolderRuntime } from './folderRuntime.js';

export const EDIT_PROPOSAL_TTL_MS = 300_000;
export const MAX_EDIT_PROPOSALS = 200;
export const MAX_PROPOSAL_PATCH_BYTES = 64 * 1024;
export const MAX_PROPOSAL_REPLACEMENT_BYTES = 128 * 1024;
export const MAX_PROPOSAL_PREVIEW_BYTES = 128 * 1024;
export const SMALL_PROPOSAL_REPLACEMENT_BYTES = 8 * 1024;
export const PATCH_EFFICIENCY_PERCENT = 80;

export class EditContractError extends Error {
  readonly code: string;
  readonly category: string;
  readonly retryable: boolean;
  readonly details: JsonObject;

  constructor(code: string, message: string, category = 'validation', retryable = false, details: JsonObject = {}) {
    super(message);
    this.name = 'EditContractError';
    this.code = code;
    this.category = category;
    this.retryable = retryable;
    this.details = details;
  }
}

export function editFailure(error: unknown, fallbackCode = 'EDIT_FAILED'): JsonObject {
  if (error instanceof EditContractError) {
    return {
      ok: false,
      error: {
        code: error.code,
        message: error.message,
        category: error.category,
        retryable: error.retryable,
        details: error.details
      }
    };
  }
  return {
    ok: false,
    error: {
      code: fallbackCode,
      message: error instanceof Error ? error.message : String(error),
      category: 'tool',
      retryable: false,
      details: {}
    }
  };
}

export function errorValue(error: EditContractError): JsonObject {
  return {
    code: error.code,
    message: error.message,
    category: error.category,
    retryable: error.retryable,
    details: error.details
  };
}

export function editRecoveryActions(file: string, actualSha256: string, reason: string): JsonObject[] {
  return [
    {
      action: 'read_current_file',
      tool: 'read_file',
      arguments: { path: file },
      reason
    },
    {
      action: 'rebuild_guarded_edit',
      tool: 'edit',
      arguments: { files: [{ path: file, expected_sha256: actualSha256 }] },
      required_arguments: ['files[0].edits'],
      reason: 'rebuild_from_fresh_content'
    }
  ];
}

export function fileVersionMismatch(file: string, expected: string, actual: string): EditContractError {
  return new EditContractError(
    'FILE_VERSION_MISMATCH',
    `File changed since it was read: ${file}`,
    'conflict',
    true,
    {
      path: file,
      expected_sha256: expected,
      actual_sha256: actual,
      suggestion: 'Read the file again and rebuild the edit or patch',
      recovery_actions: editRecoveryActions(file, actual, 'file_version_changed')
    }
  );
}

function sha256(value: string | Buffer): string {
  return createHash('sha256').update(value).digest('hex');
}

function normalizeNewlines(value: string): string {
  return value.replaceAll('\r\n', '\n');
}

function newlineAt(value: string, newlineIndex: number): '\r\n' | '\n' {
  return newlineIndex > 0 && value[newlineIndex - 1] === '\r' ? '\r\n' : '\n';
}

export function preferredNewlineNearRange(original: string, start = 0, end = start): '\r\n' | '\n' {
  const boundedStart = Math.max(0, Math.min(start, original.length));
  const boundedEnd = Math.max(boundedStart, Math.min(end, original.length));

  const inside = original.indexOf('\n', boundedStart);
  if (inside >= 0 && inside < boundedEnd) return newlineAt(original, inside);

  const after = original.indexOf('\n', boundedEnd);
  if (after >= 0) return newlineAt(original, after);

  const before = original.lastIndexOf('\n', Math.max(0, boundedStart - 1));
  if (before >= 0) return newlineAt(original, before);

  return '\n';
}

export function adaptNewlinesToRange(value: string, original: string, start = 0, end = start): string {
  const normalized = normalizeNewlines(value);
  return preferredNewlineNearRange(original, start, end) === '\r\n'
    ? normalized.replaceAll('\n', '\r\n')
    : normalized;
}

export function adaptNewlinesToOriginal(value: string, original: string): string {
  return adaptNewlinesToRange(value, original, 0, 0);
}

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function whitespaceFlexiblePattern(target: string): string {
  let pattern = '';
  let literal = '';
  let inWhitespace = false;
  for (const character of target) {
    if (/\s/u.test(character)) {
      if (literal) {
        pattern += escapeRegex(literal);
        literal = '';
      }
      if (!inWhitespace) pattern += '\\s+';
      inWhitespace = true;
    } else {
      literal += character;
      inWhitespace = false;
    }
  }
  if (literal) pattern += escapeRegex(literal);
  return pattern;
}

function lineRange(content: string, startLine: number, endLine: number): [number, number] {
  const starts = [0];
  for (let index = 0; index < content.length; index += 1) {
    if (content[index] === '\n' && index + 1 < content.length) starts.push(index + 1);
  }
  const totalLines = starts.length;
  if (!Number.isInteger(startLine) || !Number.isInteger(endLine) || startLine < 1 || startLine > endLine || endLine > totalLines) {
    throw new EditContractError(
      'EDIT_LINE_RANGE_INVALID',
      `Edit line range ${startLine}-${endLine} is outside 1-${totalLines}`,
      'validation',
      false,
      { start_line: startLine, end_line: endLine, total_lines: totalLines }
    );
  }
  return [starts[startLine - 1], endLine < totalLines ? starts[endLine] : content.length];
}

function byteToLine(content: string, offset: number): number {
  return (content.slice(0, Math.max(0, offset)).match(/\n/g)?.length ?? 0) + 1;
}

function whitespaceCandidates(original: string, target: string, range: [number, number]): Array<[number, number]> {
  const pattern = whitespaceFlexiblePattern(target);
  if (!pattern) return [];
  const matcher = new RegExp(pattern, 'gu');
  const source = original.slice(range[0], range[1]);
  const candidates: Array<[number, number]> = [];
  for (const match of source.matchAll(matcher)) {
    const offset = match.index ?? 0;
    candidates.push([range[0] + offset, range[0] + offset + match[0].length]);
  }
  return candidates;
}

function proposalDiff(file: string, expected: string, actual: string): string {
  const expectedLines = normalizeNewlines(expected).split('\n');
  const actualLines = normalizeNewlines(actual).split('\n');
  return [
    `--- a/${file}`,
    `+++ b/${file}`,
    '@@',
    ...expectedLines.map(line => `-${line}`),
    ...actualLines.map(line => `+${line}`),
    ''
  ].join('\n');
}

const EDIT_RESULT_DIFF_MAX_BYTES = 32 * 1024;
const EDIT_RESULT_DIFF_CONTEXT_LINES = 3;

export interface EditResultDiff {
  content: string;
  bytes: number;
  truncated: boolean;
}

function truncateDiffUtf8(value: string, maxBytes: number): EditResultDiff {
  const totalBytes = Buffer.byteLength(value);
  if (totalBytes <= maxBytes) return { content: value, bytes: totalBytes, truncated: false };
  let content = '';
  let bytes = 0;
  for (const character of value) {
    const size = Buffer.byteLength(character);
    if (bytes + size > maxBytes) break;
    content += character;
    bytes += size;
  }
  return { content, bytes, truncated: true };
}

function wholeFileDiff(file: string, original: string, updated: string): EditResultDiff {
  const before = normalizeNewlines(original);
  const after = normalizeNewlines(updated);
  const beforeLines = before === '' ? [] : before.replace(/\n$/, '').split('\n');
  const afterLines = after === '' ? [] : after.replace(/\n$/, '').split('\n');
  let commonPrefix = 0;
  while (
    commonPrefix < beforeLines.length
    && commonPrefix < afterLines.length
    && beforeLines[commonPrefix] === afterLines[commonPrefix]
  ) commonPrefix += 1;

  let commonSuffix = 0;
  while (
    commonSuffix < beforeLines.length - commonPrefix
    && commonSuffix < afterLines.length - commonPrefix
    && beforeLines[beforeLines.length - 1 - commonSuffix] === afterLines[afterLines.length - 1 - commonSuffix]
  ) commonSuffix += 1;

  const beforeStart = Math.max(0, commonPrefix - EDIT_RESULT_DIFF_CONTEXT_LINES);
  const afterStart = Math.max(0, commonPrefix - EDIT_RESULT_DIFF_CONTEXT_LINES);
  const beforeEnd = Math.min(beforeLines.length, beforeLines.length - commonSuffix + EDIT_RESULT_DIFF_CONTEXT_LINES);
  const afterEnd = Math.min(afterLines.length, afterLines.length - commonSuffix + EDIT_RESULT_DIFF_CONTEXT_LINES);
  const trailingBeforeStart = Math.max(commonPrefix, beforeLines.length - commonSuffix);
  const trailingAfterStart = Math.max(commonPrefix, afterLines.length - commonSuffix);
  const beforeChanged = beforeLines.slice(commonPrefix, trailingBeforeStart);
  const afterChanged = afterLines.slice(commonPrefix, trailingAfterStart);
  const leadingContext = beforeLines.slice(beforeStart, commonPrefix);
  const trailingContext = beforeLines.slice(trailingBeforeStart, beforeEnd);
  const beforeCount = beforeEnd - beforeStart;
  const afterCount = afterEnd - afterStart;
  const diff = [
    `--- a/${file}`,
    `+++ b/${file}`,
    `@@ -${beforeStart + 1},${beforeCount} +${afterStart + 1},${afterCount} @@`,
    ...leadingContext.map(line => ` ${line}`),
    ...beforeChanged.map(line => `-${line}`),
    ...afterChanged.map(line => `+${line}`),
    ...trailingContext.map(line => ` ${line}`),
    ''
  ].join('\n');
  return truncateDiffUtf8(diff, EDIT_RESULT_DIFF_MAX_BYTES);
}

function pruneEditProposals(ctx: ToolContext, key: string): void {
  const proposals = currentFolderRuntime(ctx, key).editProposals;
  const now = Date.now();
  for (const [id, proposal] of proposals) {
    if (now - proposal.createdAt > EDIT_PROPOSAL_TTL_MS || now < proposal.createdAt) proposals.delete(id);
  }
  while (proposals.size >= MAX_EDIT_PROPOSALS) {
    let oldest: [string, EditProposalRecord] | undefined;
    for (const entry of proposals) {
      if (!oldest || entry[1].createdAt < oldest[1].createdAt) oldest = entry;
    }
    if (!oldest) break;
    proposals.delete(oldest[0]);
  }
}

export function buildEditProposal(
  ctx: ToolContext,
  key: string,
  file: string,
  fileSha256: string,
  original: string,
  edits: JsonObject[]
): JsonObject | undefined {
  if (edits.length !== 1) return undefined;
  const edit = edits[0];
  if (String(edit.type ?? '') !== 'replace' || String(edit.match_mode ?? 'exact') !== 'exact' || Math.max(1, Number(edit.expected_occurrences ?? 1)) !== 1) {
    return undefined;
  }
  const oldText = typeof edit.old_text === 'string' ? edit.old_text : typeof edit.expected_text === 'string' ? edit.expected_text : '';
  if (!oldText) return undefined;
  const hasStart = edit.start_line !== undefined;
  const hasEnd = edit.end_line !== undefined;
  if (hasStart !== hasEnd) return undefined;
  const range: [number, number] = hasStart
    ? lineRange(original, Number(edit.start_line), Number(edit.end_line))
    : [0, original.length];
  const candidates = whitespaceCandidates(original, oldText, range);
  if (candidates.length !== 1) return undefined;

  const requestedReplacement = String(edit.new_text ?? '');
  const [start, end] = candidates[0];
  const replacement = adaptNewlinesToRange(requestedReplacement, original, start, end);
  const actualText = original.slice(start, end);
  const proposalId = randomUUID().replaceAll('-', '');
  const proposal: EditProposalRecord = {
    path: file,
    fileSha256,
    start,
    end,
    actualText,
    replacement,
    createdAt: Date.now()
  };
  const proposedContent = `${original.slice(0, start)}${replacement}${original.slice(end)}`;
  const proposedContentBytes = Buffer.byteLength(proposedContent);
  const proposedContentIncluded = proposedContentBytes <= MAX_PROPOSAL_PREVIEW_BYTES;
  const replacementBytes = Buffer.byteLength(replacement);
  const preferredFormat = replacementBytes <= SMALL_PROPOSAL_REPLACEMENT_BYTES ? 'replacement' : 'patch';
  pruneEditProposals(ctx, key);
  currentFolderRuntime(ctx, key).editProposals.set(proposalId, proposal);

  return {
    status: 'proposal_required',
    applied: false,
    proposal_id: proposalId,
    proposal_ttl_seconds: EDIT_PROPOSAL_TTL_MS / 1000,
    path: file,
    file_sha256: fileSha256,
    candidate_start_line: byteToLine(original, start),
    candidate_end_line: byteToLine(original, Math.max(start, end - 1)),
    actual_text: actualText,
    requested_old_text: oldText,
    requested_new_text: requestedReplacement,
    candidate_diff: proposalDiff(file, oldText, actualText),
    proposed_content: proposedContentIncluded ? proposedContent : null,
    proposed_content_bytes: proposedContentBytes,
    proposed_content_included: proposedContentIncluded,
    proposed_content_sha256: sha256(proposedContent),
    accepted_formats: ['accept', 'replacement', 'patch'],
    preferred_format: preferredFormat,
    preferred_format_reason: preferredFormat === 'replacement'
      ? 'small_replacement_is_cheaper'
      : 'large_replacement_may_benefit_from_patch',
    replacement_bytes: replacementBytes,
    replacement_max_bytes: MAX_PROPOSAL_REPLACEMENT_BYTES,
    small_replacement_threshold_bytes: SMALL_PROPOSAL_REPLACEMENT_BYTES,
    patch_efficiency_percent: PATCH_EFFICIENCY_PERCENT,
    proposal_patch_format: 'unified_diff_single_file_single_hunk',
    proposal_patch_max_bytes: MAX_PROPOSAL_PATCH_BYTES,
    next_action: 'apply_proposal',
    warnings: []
  };
}

export interface AppliedProposal {
  updated: string;
  proposalId: string;
  applyFormat: 'accept' | 'replacement' | 'patch';
}

export function applyEditProposal(
  ctx: ToolContext,
  key: string,
  file: string,
  fileSha256: string,
  original: string,
  rawApply: unknown
): AppliedProposal {
  if (!rawApply || typeof rawApply !== 'object' || Array.isArray(rawApply)) {
    throw new EditContractError('INVALID_ARGUMENT', 'apply_proposal must be an object', 'validation');
  }
  const apply = rawApply as JsonObject;
  const proposalId = typeof apply.proposal_id === 'string' ? apply.proposal_id : '';
  if (!proposalId) throw new EditContractError('INVALID_ARGUMENT', 'apply_proposal.proposal_id is required', 'validation');
  const proposalPatch = typeof apply.patch === 'string' ? apply.patch : undefined;
  const proposalReplacement = typeof apply.replacement === 'string' ? apply.replacement : undefined;
  if (proposalPatch !== undefined && proposalReplacement !== undefined) {
    throw new EditContractError('INVALID_ARGUMENT', 'apply_proposal.patch and apply_proposal.replacement are mutually exclusive', 'validation');
  }
  if (proposalPatch !== undefined && Buffer.byteLength(proposalPatch) > MAX_PROPOSAL_PATCH_BYTES) {
    throw new EditContractError('INVALID_ARGUMENT', `apply_proposal.patch exceeds ${MAX_PROPOSAL_PATCH_BYTES} bytes`, 'validation');
  }
  if (proposalReplacement !== undefined && Buffer.byteLength(proposalReplacement) > MAX_PROPOSAL_REPLACEMENT_BYTES) {
    throw new EditContractError('INVALID_ARGUMENT', `apply_proposal.replacement exceeds ${MAX_PROPOSAL_REPLACEMENT_BYTES} bytes`, 'validation');
  }

  pruneEditProposals(ctx, key);
  const proposal = currentFolderRuntime(ctx, key).editProposals.get(proposalId);
  if (!proposal) {
    throw new EditContractError(
      'EDIT_PROPOSAL_NOT_FOUND',
      'Edit proposal was not found or has expired.',
      'conflict',
      true,
      { proposal_id: proposalId, reason: 'missing_or_expired' }
    );
  }
  if (proposal.path !== file || proposal.fileSha256 !== fileSha256) {
    throw new EditContractError(
      'EDIT_PROPOSAL_STALE',
      'Edit proposal no longer matches the current file.',
      'conflict',
      true,
      { proposal_id: proposalId, reason: 'file_changed' }
    );
  }
  if (original.slice(proposal.start, proposal.end) !== proposal.actualText) {
    throw new EditContractError(
      'EDIT_PROPOSAL_STALE',
      'Edit proposal candidate no longer matches the current file.',
      'conflict',
      true,
      { proposal_id: proposalId, reason: 'candidate_changed' }
    );
  }

  let replacement: string;
  let applyFormat: AppliedProposal['applyFormat'];
  if (proposalPatch !== undefined) {
    replacement = applyRestrictedProposalPatch(proposal.replacement, proposalPatch);
    const patchBytes = Buffer.byteLength(proposalPatch);
    const replacementBytes = Buffer.byteLength(replacement);
    if (patchBytes * 100 >= replacementBytes * PATCH_EFFICIENCY_PERCENT) {
      throw new EditContractError(
        'EDIT_PROPOSAL_PATCH_INEFFICIENT',
        'Proposal patch costs as much as or more than sending the full replacement.',
        'validation',
        true,
        {
          reason: 'replacement_is_cheaper',
          patch_bytes: patchBytes,
          replacement_bytes: replacementBytes,
          patch_efficiency_percent: PATCH_EFFICIENCY_PERCENT,
          recommended_format: 'replacement',
          recommended_replacement: replacement
        }
      );
    }
    applyFormat = 'patch';
  } else if (proposalReplacement !== undefined) {
    replacement = proposalReplacement;
    applyFormat = 'replacement';
  } else {
    replacement = proposal.replacement;
    applyFormat = 'accept';
  }
  replacement = adaptNewlinesToRange(replacement, original, proposal.start, proposal.end);
  return {
    updated: `${original.slice(0, proposal.start)}${replacement}${original.slice(proposal.end)}`,
    proposalId,
    applyFormat
  };
}

export function removeEditProposal(ctx: ToolContext, key: string, proposalId: string): void {
  currentFolderRuntime(ctx, key).editProposals.delete(proposalId);
}

type HunkLine = { kind: 'context' | 'add' | 'remove'; value: string };
interface Hunk { oldStart?: number; lines: HunkLine[] }
interface FilePatch { path: string; hunks: Hunk[]; isNewFile: boolean; isDeleted: boolean }

function parseDiffPath(raw: string): string {
  const trimmed = raw.trim().split('\t')[0];
  const value = trimmed.startsWith('a/') || trimmed.startsWith('b/') ? trimmed.slice(2) : trimmed;
  return value === '/dev/null' ? '' : value.replaceAll('\\', '/');
}

function parseHunkOldStart(header: string): number | undefined {
  const matched = header.match(/^@@\s+-(\d+)/);
  return matched ? Math.max(1, Number(matched[1])) : undefined;
}

function finishPatch(files: FilePatch[], current: FilePatch | undefined, hunk: Hunk | undefined): [FilePatch | undefined, Hunk | undefined] {
  if (current && hunk) current.hunks.push(hunk);
  if (current) files.push(current);
  return [undefined, undefined];
}

function parseUnifiedDiff(patch: string): FilePatch[] {
  const patchLines = patch.split(/\r?\n/);
  if (patchLines.at(-1) === '') patchLines.pop();
  const codex = patchLines.some(line => line === '*** Begin Patch');
  const files: FilePatch[] = [];
  let current: FilePatch | undefined;
  let hunk: Hunk | undefined;
  for (const rawLine of patchLines) {
    const line = rawLine.replace(/\r$/, '');
    if (codex) {
      if (line === '*** Begin Patch') continue;
      if (line === '*** End Patch') {
        [current, hunk] = finishPatch(files, current, hunk);
        continue;
      }
      const header = line.startsWith('*** Add File: ')
        ? { path: line.slice(14), isNewFile: true, isDeleted: false }
        : line.startsWith('*** Update File: ')
          ? { path: line.slice(17), isNewFile: false, isDeleted: false }
          : line.startsWith('*** Delete File: ')
            ? { path: line.slice(17), isNewFile: false, isDeleted: true }
            : undefined;
      if (header) {
        [current, hunk] = finishPatch(files, current, hunk);
        current = { path: parseDiffPath(header.path), hunks: [], isNewFile: header.isNewFile, isDeleted: header.isDeleted };
        if (header.isNewFile) hunk = { oldStart: 1, lines: [] };
        continue;
      }
    } else if (line.startsWith('--- ')) {
      [current, hunk] = finishPatch(files, current, hunk);
      current = {
        path: parseDiffPath(line.slice(4)),
        hunks: [],
        isNewFile: line.includes('/dev/null'),
        isDeleted: false
      };
      continue;
    } else if (line.startsWith('+++ ')) {
      if (current) {
        const newPath = parseDiffPath(line.slice(4));
        if (newPath) current.path = newPath;
        if (line.includes('/dev/null')) current.isDeleted = true;
      }
      continue;
    }
    if (line.startsWith('@@')) {
      if (current && hunk) current.hunks.push(hunk);
      hunk = { oldStart: parseHunkOldStart(line), lines: [] };
      continue;
    }
    if (!current || current.isDeleted) continue;
    hunk ??= { lines: [] };
    if (line.startsWith('+')) hunk.lines.push({ kind: 'add', value: line.slice(1) });
    else if (line.startsWith('-')) hunk.lines.push({ kind: 'remove', value: line.slice(1) });
    else if (line.startsWith(' ')) hunk.lines.push({ kind: 'context', value: line.slice(1) });
    else if (line === '') hunk.lines.push({ kind: 'context', value: '' });
  }
  finishPatch(files, current, hunk);
  return files;
}

function splitPatchLines(original: string): { lines: string[]; lineEnding: string; hadTrailingNewline: boolean } {
  const lineEnding = original.includes('\r\n') ? '\r\n' : '\n';
  const hadTrailingNewline = original.endsWith('\n');
  const withoutTrailing = hadTrailingNewline ? original.slice(0, -lineEnding.length) : original;
  return {
    lines: withoutTrailing ? withoutTrailing.split(/\r?\n/) : [],
    lineEnding,
    hadTrailingNewline
  };
}

function hunkMatchesAt(lines: string[], pattern: string[], position: number): boolean {
  if (position < 0 || position + pattern.length > lines.length) return false;
  return pattern.every((value, index) => lines[position + index] === value);
}

function nearbyContexts(lines: string[], positions: number[], radius = 3): JsonObject[] {
  return positions.slice(0, 8).map(position => {
    const start = Math.max(0, position - radius);
    const end = Math.min(lines.length, position + radius + 1);
    return {
      line: position + 1,
      start_line: start + 1,
      end_line: end,
      preview: lines.slice(start, end)
    };
  });
}

function findHunkPosition(
  lines: string[],
  pattern: string[],
  replacement: string[],
  preferred: number | undefined,
  hunkIndex: number,
  file: string
): number {
  if (!pattern.length) return Math.min(lines.length, preferred ?? lines.length);
  if (preferred !== undefined && hunkMatchesAt(lines, pattern, preferred)) return preferred;
  const candidates: number[] = [];
  for (let position = 0; position + pattern.length <= lines.length; position += 1) {
    if (hunkMatchesAt(lines, pattern, position)) candidates.push(position);
  }
  if (candidates.length === 1) return candidates[0];
  if (!candidates.length) {
    throw new EditContractError(
      'PATCH_CONTEXT_NOT_FOUND',
      `Hunk ${hunkIndex} context did not match file content.`,
      'validation',
      false,
      {
        hunk_index: hunkIndex,
        preferred_line: preferred === undefined ? null : preferred + 1,
        pattern_preview: pattern.slice(0, 8),
        nearby_contexts: preferred === undefined ? [] : nearbyContexts(lines, [preferred]),
        recommended_tool: 'edit',
        suggestion: 'Read the exact target range and use edit for a single precise replacement, or include more unique patch context.',
        recovery_actions: [
          {
            action: 'read_target_range',
            tool: 'read_file',
            required_arguments: ['path'],
            arguments: {
              path: file,
              start_line: preferred === undefined ? null : Math.max(1, preferred - 2),
              end_line: preferred === undefined ? null : preferred + 4
            },
            reason: 'patch_context_not_found'
          },
          {
            action: 'switch_to_precise_edit',
            tool: 'edit',
            required_arguments: ['files'],
            arguments: { files: [{ path: file }] },
            reason: 'patch_context_not_found'
          }
        ]
      }
    );
  }
  throw new EditContractError(
    'PATCH_CONTEXT_AMBIGUOUS',
    `Hunk ${hunkIndex} context matched multiple locations; add more context or line numbers.`,
    'validation',
    false,
    {
      hunk_index: hunkIndex,
      candidate_lines: candidates.map(position => position + 1),
      nearby_contexts: nearbyContexts(lines, candidates),
      recommended_tool: 'edit',
      suggestion: 'Use edit with exact old_text and expected_sha256, or add unique surrounding lines to this hunk.',
      recovery_actions: candidates.slice(0, 8).map((position, candidateIndex) => ({
        action: 'apply_candidate_range',
        action_id: `patch-candidate-${hunkIndex}-${candidateIndex + 1}`,
        tool: 'edit',
        required_arguments: [],
        candidate_line: position + 1,
        arguments: {
          files: [{
            path: file,
            edits: [{
              type: 'replace_lines',
              start_line: position + 1,
              end_line: position + pattern.length,
              expected_text: pattern.join('\n'),
              new_text: replacement.join('\n')
            }]
          }]
        },
        reason: 'patch_context_ambiguous'
      }))
    }
  );
}

function applyHunks(original: string, hunks: Hunk[], file: string): string {
  const split = splitPatchLines(original);
  const lines = [...split.lines];
  let offset = 0;
  const issues: EditContractError[] = [];
  for (let hunkIndex = 0; hunkIndex < hunks.length; hunkIndex += 1) {
    const hunk = hunks[hunkIndex];
    const oldPattern = hunk.lines.filter(line => line.kind !== 'add').map(line => line.value);
    const replacement = hunk.lines.filter(line => line.kind !== 'remove').map(line => line.value);
    const preferred = hunk.oldStart === undefined
      ? undefined
      : Math.min(lines.length, Math.max(0, hunk.oldStart - 1 + offset));
    let position: number;
    try {
      position = findHunkPosition(lines, oldPattern, replacement, preferred, hunkIndex, file);
    } catch (error) {
      if (error instanceof EditContractError) {
        issues.push(error);
        continue;
      }
      throw error;
    }
    let index = position;
    let added = 0;
    let removed = 0;
    for (const line of hunk.lines) {
      if (line.kind === 'context') index += 1;
      else if (line.kind === 'remove') {
        if (index < lines.length) lines.splice(index, 1);
        removed += 1;
      } else {
        lines.splice(index, 0, line.value);
        index += 1;
        added += 1;
      }
    }
    offset += added - removed;
  }
  if (issues.length === 1) throw issues[0];
  if (issues.length > 1) {
    throw new EditContractError(
      'PATCH_PREFLIGHT_FAILED',
      `${issues.length} patch hunks failed preflight.`,
      'validation',
      false,
      {
        issue_count: issues.length,
        issues: issues.map(errorValue),
        recommended_tool: 'edit',
        suggestion: 'Resolve all listed hunk issues before retrying. Prefer edit for precise replacements.',
        recovery_actions: [{
          action: 'switch_to_precise_edits',
          tool: 'edit',
          required_arguments: ['files'],
          arguments: { files: [{ path: file }] },
          reason: 'multiple_patch_hunks_failed_preflight'
        }]
      }
    );
  }
  let output = lines.join(split.lineEnding);
  if (output && (split.hadTrailingNewline || original === '')) output += split.lineEnding;
  return output;
}

function applyRestrictedProposalPatch(proposedText: string, patch: string): string {
  let files: FilePatch[];
  try {
    files = parseUnifiedDiff(patch);
  } catch (error) {
    throw new EditContractError(
      'EDIT_PROPOSAL_PATCH_INVALID',
      'Proposal patch is not a valid unified diff.',
      'validation',
      true,
      { reason: 'invalid_unified_diff', source_error: error instanceof Error ? error.message : String(error) }
    );
  }
  const first = files[0];
  if (files.length !== 1 || !first || first.hunks.length !== 1 || first.isNewFile || first.isDeleted) {
    throw new EditContractError(
      'EDIT_PROPOSAL_PATCH_INVALID',
      'Proposal patch must contain exactly one file and one update hunk.',
      'validation',
      true,
      {
        reason: 'single_file_single_hunk_required',
        file_count: files.length,
        hunk_count: first?.hunks.length ?? 0
      }
    );
  }
  let updated: string;
  try {
    updated = applyHunks(proposedText, first.hunks, 'proposal');
  } catch (error) {
    throw new EditContractError(
      'EDIT_PROPOSAL_PATCH_MISMATCH',
      'Proposal patch did not apply exactly to the proposed replacement.',
      'conflict',
      true,
      {
        reason: 'proposal_text_mismatch',
        source_error: error instanceof EditContractError ? errorValue(error) : String(error)
      }
    );
  }
  if (updated === proposedText) {
    throw new EditContractError(
      'EDIT_PROPOSAL_PATCH_NO_CHANGES',
      'Proposal patch produced no changes.',
      'validation',
      true,
      { reason: 'no_changes' }
    );
  }
  return updated;
}

async function workspacePatchPath(
  root: string,
  cwd: string,
  patchPath: string,
  isNewFile: boolean
): Promise<{ file: string; display: string }> {
  validateWorkspaceUserPath(patchPath, { allowDot: false });
  const cwdRelative = relativeInside(root, cwd).replaceAll('\\', '/');
  const normalizedPatch = patchPath.replaceAll('\\', '/');
  const workspacePath = cwdRelative === '.'
    ? normalizedPatch
    : path.posix.join(cwdRelative, normalizedPatch);
  const resolved = isNewFile
    ? await resolveWritePath(root, workspacePath)
    : await resolveExistingWritePath(root, workspacePath);
  if (isNewFile) await rejectDirectWriteSymlink(resolved);
  return { file: resolved.full, display: resolved.display };
}

export async function preflightPatch(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject | undefined> {
  const patch = String(args.patch ?? '');
  if (!patch) return undefined;
  const files = parseUnifiedDiff(patch);
  if (!files.length) return undefined;
  const { root, cwd } = rootAndCwd(ctx, key);
  const expectedHashes = args.expected_sha256 && typeof args.expected_sha256 === 'object' && !Array.isArray(args.expected_sha256)
    ? args.expected_sha256 as JsonObject
    : {};
  try {
    for (const filePatch of files) {
      if (!filePatch.path) continue;
      const resolved = await workspacePatchPath(root, cwd, filePatch.path, filePatch.isNewFile);
      let original = '';
      let actualHash = 'missing';
      if (!filePatch.isNewFile) {
        const bytes = await readFile(resolved.file);
        actualHash = sha256(bytes);
        try {
          const decoded = decodeTextBuffer(bytes);
          if (decoded.encoding !== 'utf-8') {
            throw new EditContractError(
              'UNSUPPORTED_ENCODING',
              `Patch tools require UTF-8 files: ${resolved.display}`,
              'validation',
              false,
              { path: resolved.display, encoding: decoded.encoding }
            );
          }
          original = decoded.bom ? `\uFEFF${decoded.text}` : decoded.text;
        } catch (error) {
          if (error instanceof TextDecodingError) {
            throw new EditContractError(error.code, error.message, error.category, error.retryable, {
              ...error.details,
              path: resolved.display
            });
          }
          throw error;
        }
      }
      const expected = typeof expectedHashes[filePatch.path] === 'string'
        ? String(expectedHashes[filePatch.path])
        : typeof expectedHashes[resolved.display] === 'string'
          ? String(expectedHashes[resolved.display])
          : undefined;
      if (expected && expected.toLowerCase() !== actualHash.toLowerCase()) throw fileVersionMismatch(resolved.display, expected, actualHash);
      applyHunks(original, filePatch.hunks, resolved.display);
    }
    return undefined;
  } catch (error) {
    if (error instanceof EditContractError) return editFailure(error, 'PATCH_CHECK_FAILED');
    if (error instanceof WorkspacePathError) {
      return editFailure(new EditContractError(
        error.code,
        error.message,
        error.category,
        error.retryable,
        error.details as JsonObject
      ), 'PATCH_CHECK_FAILED');
    }
    return undefined;
  }
}

export function editResultDiff(file: string, original: string, updated: string): EditResultDiff {
  return wholeFileDiff(file, original, updated);
}
