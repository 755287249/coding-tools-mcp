import { createHash, randomUUID } from 'node:crypto';
import { readFile, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import type { JsonObject, ToolContext } from './types.js';
import { runGitBuffered } from './gitProcess.js';
import { scanExcludedWorktreeContainerPrefixes } from './gitWorktreeBoundaries.js';
import {
  exists, globRegex, readText, resolveExistingPath, resolveExistingWritePath,
  resolveInside, rootAndCwd, sha256File, walk, WorkspacePathError
} from './workspace.js';
import {
  decodeTextBuffer, encodeText, readDecodedTextFile, TextDecodingError,
  textDecodingErrorValue, type TextEncoding
} from './textCodec.js';
import {
  adaptNewlinesToRange, applyEditProposal, buildEditProposal, EditContractError,
  editFailure, editRecoveryActions, editResultDiff, fileVersionMismatch,
  preflightPatch, removeEditProposal
} from './editRecovery.js';
import {
  decodeRaster, identifyImage, ImageContractError, outputTooLarge,
  resizeDecodedImage, shouldResize, type ImageInfo
} from './imageCodec.js';
const ok = (value: JsonObject): JsonObject => ({ ok: true, ...value });
const fail = (code: string, message: string, details: JsonObject = {}): JsonObject => ({ ok: false, error: { code, message, category: 'tool', retryable: false, details } });

function canonicalJson(value: unknown): string {
  if (value === null || typeof value === 'boolean' || typeof value === 'number' || typeof value === 'string') {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  const object = value as Record<string, unknown>;
  return `{${Object.keys(object).sort().map(key => `${JSON.stringify(key)}:${canonicalJson(object[key])}`).join(',')}}`;
}

function replayableEditPlan(
  tool: 'edit' | 'edit_file' | 'edit_many',
  argumentsValue: JsonObject,
  files: JsonObject[],
  statefulDependencies: JsonObject[] = []
): JsonObject {
  const plan: JsonObject = {
    schema_version: 1,
    tool,
    arguments: argumentsValue,
    expected_result: { files },
    stateful_dependencies: statefulDependencies
  };
  return {
    ...plan,
    plan_sha256: createHash('sha256').update(canonicalJson(plan)).digest('hex')
  };
}

function splitInclusive(value: string): string[] {
  return value.match(/[^\n]*\n|[^\n]+$/g) ?? [];
}

function truncatePrefixUtf8(value: string, maxBytes: number): { content: string; truncated: boolean } {
  if (Buffer.byteLength(value) <= maxBytes) return { content: value, truncated: false };
  let content = '';
  let bytes = 0;
  for (const character of value) {
    const size = Buffer.byteLength(character);
    if (bytes + size > maxBytes) break;
    content += character;
    bytes += size;
  }
  return { content, truncated: true };
}

function newlineStyle(value: string): string {
  const hasCrLf = value.includes('\r\n');
  const hasLf = /(^|[^\r])\n/.test(value);
  if (hasCrLf && hasLf) return 'mixed';
  if (hasCrLf) return 'crlf';
  if (hasLf) return 'lf';
  return 'none';
}

interface LineEndingToken {
  content: string;
  eol: '' | '\r\n' | '\n';
}

function lineEndingTokens(value: string): LineEndingToken[] {
  const tokens: LineEndingToken[] = [];
  let start = 0;
  while (start < value.length) {
    const newline = value.indexOf('\n', start);
    if (newline < 0) {
      tokens.push({ content: value.slice(start), eol: '' });
      break;
    }
    const crlf = newline > start && value[newline - 1] === '\r';
    tokens.push({
      content: value.slice(start, crlf ? newline - 1 : newline),
      eol: crlf ? '\r\n' : '\n'
    });
    start = newline + 1;
  }
  if (value.endsWith('\n')) tokens.push({ content: '', eol: '' });
  return tokens;
}

function lineContentCounts(tokens: LineEndingToken[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const token of tokens) counts.set(token.content, (counts.get(token.content) ?? 0) + 1);
  return counts;
}

export interface UnchangedLineEndingChange {
  before_line: number;
  after_line: number;
  before_eol: '' | '\r\n' | '\n';
  after_eol: '' | '\r\n' | '\n';
}

export function unchangedLineEndingChanges(original: string, updated: string): UnchangedLineEndingChange[] {
  const before = lineEndingTokens(original);
  const after = lineEndingTokens(updated);
  const beforeCounts = lineContentCounts(before);
  const afterCounts = lineContentCounts(after);
  const unambiguous = (content: string) => beforeCounts.get(content) === 1 && afterCounts.get(content) === 1;
  const limit = Math.min(before.length, after.length);
  let prefix = 0;
  while (prefix < limit && before[prefix].content === after[prefix].content) prefix += 1;

  let suffix = 0;
  while (
    suffix < limit - prefix
    && before[before.length - 1 - suffix].content === after[after.length - 1 - suffix].content
  ) suffix += 1;

  const changes: UnchangedLineEndingChange[] = [];
  for (let index = 0; index < prefix; index += 1) {
    if (before[index].eol === after[index].eol || !unambiguous(before[index].content)) continue;
    const allowedFinalDeletionBoundary = index === prefix - 1
      && index === after.length - 1
      && after[index].eol === ''
      && before.length > after.length;
    if (!allowedFinalDeletionBoundary) {
      changes.push({
        before_line: index + 1,
        after_line: index + 1,
        before_eol: before[index].eol,
        after_eol: after[index].eol
      });
    }
  }
  for (let offset = 0; offset < suffix; offset += 1) {
    const beforeIndex = before.length - 1 - offset;
    const afterIndex = after.length - 1 - offset;
    if (before[beforeIndex].eol === after[afterIndex].eol || !unambiguous(before[beforeIndex].content)) continue;
    changes.push({
      before_line: beforeIndex + 1,
      after_line: afterIndex + 1,
      before_eol: before[beforeIndex].eol,
      after_eol: after[afterIndex].eol
    });
  }
  return changes.sort((left, right) => left.before_line - right.before_line || left.after_line - right.after_line);
}

function assertNoUnexpectedNewlineChurn(file: string, original: string, updated: string): void {
  const changes = unchangedLineEndingChanges(original, updated);
  if (!changes.length) return;
  throw new EditContractError(
    'EDIT_NEWLINE_CHURN',
    `Edit would change line endings on unchanged content in ${file}`,
    'validation',
    false,
    {
      path: file,
      newline_before: newlineStyle(original),
      newline_after: newlineStyle(updated),
      unchanged_line_ending_change_count: changes.length,
      unchanged_line_ending_changes: changes.slice(0, 20),
      changes_truncated: changes.length > 20,
      suggestion: 'Use a precise target range; line-ending normalization must not rewrite untouched content.'
    }
  );
}

export interface EditBlastRadius {
  before_line_count: number;
  after_line_count: number;
  changed_before_lines: number;
  changed_after_lines: number;
  changed_line_count: number;
  expected_change_line_budget: number;
  allowed_changed_line_count: number;
  change_ratio: number;
  change_measure: 'prefix_suffix' | 'bounded_line_diff';
  span_changed_line_count: number;
  excessive: boolean;
}

function semanticLines(value: string): string[] {
  return value.replaceAll('\r\n', '\n').split('\n');
}

function editTextLineUnits(value: unknown): number {
  if (typeof value !== 'string' || value.length === 0) return 0;
  return semanticLines(value).length;
}

function expectedEditLineBudget(edits: readonly JsonObject[]): number {
  let budget = 0;
  for (const edit of edits) {
    const occurrences = Math.max(1, Number.isSafeInteger(edit.expected_occurrences) ? Number(edit.expected_occurrences) : 1);
    switch (String(edit.type ?? '')) {
      case 'replace':
        budget += occurrences * (editTextLineUnits(edit.old_text) + editTextLineUnits(edit.new_text));
        break;
      case 'insert_before':
      case 'insert_after':
        budget += occurrences * editTextLineUnits(edit.text);
        break;
      case 'replace_lines': {
        const start = Number(edit.start_line);
        const end = Number(edit.end_line);
        const removed = Number.isInteger(start) && Number.isInteger(end) && end >= start ? end - start + 1 : 1;
        budget += removed + editTextLineUnits(edit.new_text);
        break;
      }
      case 'delete_lines': {
        const start = Number(edit.start_line);
        const end = Number(edit.end_line);
        budget += Number.isInteger(start) && Number.isInteger(end) && end >= start ? end - start + 1 : 1;
        break;
      }
      default:
        budget += 1;
    }
  }
  return Math.max(1, budget);
}

function boundedLineEditDistance(before: readonly string[], after: readonly string[], maxDistance: number): number | undefined {
  if (Math.abs(before.length - after.length) > maxDistance) return undefined;
  const limit = Math.min(maxDistance, before.length + after.length);
  let frontier = new Map<number, number>([[1, 0]]);
  for (let distance = 0; distance <= limit; distance += 1) {
    const next = new Map<number, number>();
    for (let diagonal = -distance; diagonal <= distance; diagonal += 2) {
      const down = frontier.get(diagonal + 1);
      const right = frontier.get(diagonal - 1);
      let x = diagonal === -distance || (diagonal !== distance && (right ?? -1) < (down ?? -1))
        ? down ?? 0
        : (right ?? 0) + 1;
      let y = x - diagonal;
      while (x < before.length && y < after.length && before[x] === after[y]) {
        x += 1;
        y += 1;
      }
      if (x >= before.length && y >= after.length) return distance;
      next.set(diagonal, x);
    }
    frontier = next;
  }
  return undefined;
}

export function editBlastRadius(original: string, updated: string, edits: readonly JsonObject[] = []): EditBlastRadius {
  const before = semanticLines(original);
  const after = semanticLines(updated);
  const limit = Math.min(before.length, after.length);
  let prefix = 0;
  while (prefix < limit && before[prefix] === after[prefix]) prefix += 1;
  let suffix = 0;
  while (suffix < limit - prefix && before[before.length - 1 - suffix] === after[after.length - 1 - suffix]) suffix += 1;
  const changedBefore = Math.max(0, before.length - prefix - suffix);
  const changedAfter = Math.max(0, after.length - prefix - suffix);
  const spanChangedLineCount = changedBefore + changedAfter;
  const expectedBudget = edits.length ? expectedEditLineBudget(edits) : Math.max(1, spanChangedLineCount);
  const allowedChangedLineCount = Math.max(40, expectedBudget * 12);
  const boundedDistance = edits.length && spanChangedLineCount > allowedChangedLineCount
    ? boundedLineEditDistance(before, after, allowedChangedLineCount)
    : undefined;
  const changedLineCount = boundedDistance ?? spanChangedLineCount;
  const changeMeasure = boundedDistance === undefined ? 'prefix_suffix' : 'bounded_line_diff';
  const denominator = Math.max(1, before.length + after.length);
  const changeRatio = changedLineCount / denominator;
  return {
    before_line_count: before.length,
    after_line_count: after.length,
    changed_before_lines: changedBefore,
    changed_after_lines: changedAfter,
    changed_line_count: changedLineCount,
    expected_change_line_budget: expectedBudget,
    allowed_changed_line_count: allowedChangedLineCount,
    change_ratio: Number(changeRatio.toFixed(4)),
    change_measure: changeMeasure,
    span_changed_line_count: spanChangedLineCount,
    excessive: Math.max(before.length, after.length) >= 80
      && changedLineCount > allowedChangedLineCount
      && changeRatio >= 0.6
  };
}

function assertNoUnexpectedEditBlastRadius(file: string, original: string, updated: string, edits: readonly JsonObject[]): EditBlastRadius {
  const radius = editBlastRadius(original, updated, edits);
  if (!radius.excessive) return radius;
  throw new EditContractError(
    'EDIT_BLAST_RADIUS',
    `Edit would change far more content than its guarded edit contract in ${file}`,
    'validation',
    false,
    {
      path: file,
      ...radius,
      suggestion: 'Read the target again and use a narrower precise edit. Large intentional rewrites should describe the full replacement in the edit contract.'
    }
  );
}

function numberedContent(content: string, startLine: number): string {
  return splitInclusive(content).map((line, index) => `${String(startLine + index).padStart(6)} | ${line}`).join('');
}

function regexFor(query: string, regex: boolean, caseSensitive: boolean, global = false): RegExp {
  const source = regex ? query : query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  try { return new RegExp(source, `${global ? 'g' : ''}${caseSensitive ? '' : 'i'}u`); }
  catch (error) { throw new Error(`invalid regex ${query}: ${error instanceof Error ? error.message : String(error)}`); }
}

function stableMatchId(file: string, line: number, queryIndex: number, query: string): string {
  return createHash('sha256').update(`${file}\0${line}\0${queryIndex}\0${query}`).digest('hex').slice(0, 24);
}

function previewAround(line: string, start: number, end: number, maxBytes: number): string {
  if (Buffer.byteLength(line) <= maxBytes) return line;
  const center = Math.floor((start + end) / 2);
  const radius = Math.max(16, Math.floor(maxBytes / 2));
  const sliced = line.slice(Math.max(0, center - radius), Math.min(line.length, center + radius));
  return truncatePrefixUtf8(sliced, maxBytes).content;
}

export async function readFileTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const { root } = rootAndCwd(ctx, key);
  const requestedPath = String(args.path ?? '');
  if (!requestedPath) throw new Error('path is required');
  const resolved = await resolveExistingPath(root, requestedPath);
  const file = resolved.full;
  const info = await stat(file);
  if (info.isDirectory()) throw new Error('IS_DIRECTORY');
  const data = await readFile(file);
  const decoded = decodeTextBuffer(data);
  const { text, encoding, bom } = decoded;
  const lines = splitInclusive(text);
  const totalLines = lines.length;
  const start = Math.max(1, Math.floor(Number(args.start_line ?? 1)));
  const requestedEnd = args.end_line === undefined ? undefined : Math.max(1, Math.floor(Number(args.end_line)));
  const end = Math.min(totalLines, requestedEnd ?? totalLines);
  const selected = end < start || start > totalLines ? '' : lines.slice(start - 1, end).join('');
  const maxBytes = Math.max(1, Math.min(1_048_576, Number(args.max_bytes ?? 131_072)));
  const truncatedValue = truncatePrefixUtf8(selected, maxBytes);
  const selectedLines = splitInclusive(truncatedValue.content).length;
  const actualEnd = truncatedValue.truncated && truncatedValue.content
    ? start + Math.max(0, selectedLines - 1)
    : end;
  const display = resolved.display;
  return ok({
    path: display,
    content: truncatedValue.content,
    encoding,
    bom,
    sha256: createHash('sha256').update(data).digest('hex'),
    newline: newlineStyle(text),
    start_line: start,
    end_line: actualEnd,
    requested_end_line: requestedEnd ?? null,
    total_lines: totalLines,
    total_bytes: data.length,
    bytes_read: Buffer.byteLength(truncatedValue.content),
    truncated: truncatedValue.truncated,
    truncated_by: truncatedValue.truncated ? 'bytes' : null,
    warnings: truncatedValue.truncated ? ['content truncated'] : []
  });
}

export async function readManyTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  type Request = { index: number; source_indexes: number[]; path: string; start_line: number; end_line?: number; max_bytes?: number };
  const requests: Request[] = [];
  const items = Array.isArray(args.items) ? args.items as JsonObject[] : [];
  const matches = Array.isArray(args.matches) ? args.matches as JsonObject[] : [];
  const contextLines = Math.max(0, Math.min(500, Number(args.context_lines ?? 20)));
  const append = (item: JsonObject, index: number, context: number) => {
    const file = String(item.path ?? '');
    if (!file) throw new Error('item.path is required');
    const line = item.line === undefined ? undefined : Math.max(1, Number(item.line));
    const start = item.start_line === undefined
      ? line === undefined ? 1 : Math.max(1, line - context)
      : Math.max(1, Number(item.start_line));
    const end = item.end_line === undefined
      ? line === undefined ? undefined : line + context
      : Math.max(1, Number(item.end_line));
    if (end !== undefined && end < start) throw new Error('item.end_line must be >= item.start_line');
    requests.push({ index, source_indexes: [index], path: file, start_line: start, ...(end === undefined ? {} : { end_line: end }), ...(item.max_bytes === undefined ? {} : { max_bytes: Number(item.max_bytes) }) });
  };
  items.slice(0, 100).forEach((item, index) => append(item, index, 0));
  matches.slice(0, 500).forEach((item, offset) => append(item, items.length + offset, contextLines));
  if (!requests.length) throw new Error('items or matches must contain at least one read request');

  let normalized = requests;
  if (args.merge_overlaps !== false) {
    const grouped = new Map<string, Request[]>();
    for (const request of requests) (grouped.get(request.path) ?? grouped.set(request.path, []).get(request.path)!).push(request);
    normalized = [];
    for (const group of grouped.values()) {
      group.sort((left, right) => left.start_line - right.start_line);
      for (const request of group) {
        const last = normalized.at(-1);
        const lastEnd = last?.end_line ?? Number.MAX_SAFE_INTEGER;
        if (last && last.path === request.path && request.start_line <= lastEnd + 1 && last.max_bytes === request.max_bytes) {
          last.end_line = last.end_line === undefined || request.end_line === undefined ? undefined : Math.max(last.end_line, request.end_line);
          last.source_indexes.push(...request.source_indexes);
        } else normalized.push({ ...request, source_indexes: [...request.source_indexes] });
      }
    }
    normalized.sort((left, right) => left.index - right.index);
  }

  const maxTotal = Math.max(1, Math.min(4_194_304, Number(args.max_total_bytes ?? 262_144)));
  const defaultMax = Math.max(1, Math.min(1_048_576, Number(args.max_bytes_per_file ?? 131_072)));
  const lineNumbers = args.line_numbers === true;
  const requestedContentMode = String(args.content_mode ?? (lineNumbers ? 'numbered' : 'plain'));
  if (!['plain', 'numbered', 'both'].includes(requestedContentMode)) {
    throw new Error('content_mode must be one of: plain, numbered, both');
  }
  const includePlainContent = requestedContentMode !== 'numbered';
  const includeNumberedContent = requestedContentMode !== 'plain';
  const results: JsonObject[] = [];
  let remaining = maxTotal;
  let failed = 0;
  let truncated = false;
  for (const request of normalized) {
    if (remaining <= 0) {
      failed += 1;
      truncated = true;
      results.push({ index: request.index, source_indexes: request.source_indexes, path: request.path, ok: false, error: { code: 'BATCH_LIMIT_REACHED', message: 'max_total_bytes reached before this item was read', category: 'limit', retryable: true, details: { max_total_bytes: maxTotal } } });
      continue;
    }
    try {
      const result = await readFileTool(ctx, key, {
        path: request.path,
        start_line: request.start_line,
        ...(request.end_line === undefined ? {} : { end_line: request.end_line }),
        max_bytes: Math.min(request.max_bytes ?? defaultMax, remaining)
      });
      remaining -= Number(result.bytes_read ?? 0);
      truncated ||= result.truncated === true;
      const plainContent = String(result.content ?? '');
      const numbered = includeNumberedContent
        ? numberedContent(plainContent, Number(result.start_line ?? 1))
        : undefined;
      if (!includePlainContent) delete result.content;
      results.push({
        ...result,
        index: request.index,
        source_indexes: request.source_indexes,
        content_mode: requestedContentMode,
        ...(!includePlainContent ? { content_omitted: true } : {}),
        ...(numbered === undefined ? {} : { numbered_content: numbered })
      });
    } catch (error) {
      failed += 1;
      results.push({
        index: request.index,
        source_indexes: request.source_indexes,
        path: request.path,
        ok: false,
        error: error instanceof TextDecodingError
          ? textDecodingErrorValue(error)
          : error instanceof WorkspacePathError
            ? { code: error.code, message: error.message, category: error.category, retryable: error.retryable, details: error.details }
            : { code: 'READ_FAILED', message: error instanceof Error ? error.message : String(error), category: 'tool', retryable: false, details: {} }
      });
    }
  }
  return ok({
    results,
    requested_count: normalized.reduce((sum, item) => sum + item.source_indexes.length, 0),
    result_count: normalized.length,
    merged_count: normalized.reduce((sum, item) => sum + Math.max(0, item.source_indexes.length - 1), 0),
    failed_count: failed,
    content_mode: requestedContentMode,
    bytes_read: maxTotal - remaining,
    max_total_bytes: maxTotal,
    truncated,
    warnings: truncated ? ['one or more reads were truncated'] : []
  });
}

async function workspaceScanExcludedPrefixes(root: string, base: string): Promise<string[]> {
  if (path.relative(root, base) !== '') return [];
  return scanExcludedWorktreeContainerPrefixes(root);
}

export async function projectMapTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const selected = await resolveExistingPath(rootAndCwd(ctx, key).root, String(args.path ?? '.'));
  const root = selected.root;
  const base = selected.full;
  if (!(await stat(base)).isDirectory()) throw new Error('NOT_A_DIRECTORY');
  const maxFiles = Math.max(1, Math.min(50_000, Number(args.max_files ?? 10_000)));
  const maxEntries = Math.max(1, Math.min(10_000, Number(args.max_entries ?? 1_000)));
  const maxDepth = Math.max(1, Math.min(20, Number(args.max_depth ?? 4)));
  const includeIgnored = args.include_ignored === true;
  const excludedPrefixes = await workspaceScanExcludedPrefixes(root, base);
  const entries = await walk(root, base, {
    maxDepth,
    maxResults: Math.min(60_000, maxFiles + maxEntries + 1),
    includeDirectories: true,
    includeHidden: args.include_hidden === true,
    includeIgnored,
    includeGenerated: args.include_generated === true,
    excludedPrefixes
  });
  const tree = entries.slice(0, maxEntries).map(entry => ({
    path: entry.path,
    type: entry.type === 'directory' ? 'directory' : 'file',
    depth: path.relative(base, resolveInside(root, entry.path)).split(path.sep).filter(Boolean).length
  }));
  const files = entries.filter(entry => entry.type === 'file');
  const scanned = files.slice(0, maxFiles);
  const manifestKinds: Record<string, { kind: string; language: string; commands: string[] }> = {
    'package.json': { kind: 'npm', language: 'javascript', commands: ['npm test', 'npm run build'] },
    'Cargo.toml': { kind: 'cargo', language: 'rust', commands: ['cargo test', 'cargo check'] },
    'pyproject.toml': { kind: 'python', language: 'python', commands: ['python -m pytest'] },
    'requirements.txt': { kind: 'python', language: 'python', commands: ['python -m pytest'] },
    'go.mod': { kind: 'go', language: 'go', commands: ['go test ./...'] },
    'pom.xml': { kind: 'maven', language: 'java', commands: ['mvn test'] },
    'build.gradle': { kind: 'gradle', language: 'java', commands: ['./gradlew test'] },
    'CMakeLists.txt': { kind: 'cmake', language: 'cpp', commands: ['cmake --build build'] }
  };
  const manifests: JsonObject[] = [];
  const packageScripts: Record<string, string> = {};
  const languageNames: Record<string, string> = {
    '.ts': 'typescript', '.tsx': 'typescript', '.js': 'javascript', '.jsx': 'javascript', '.mjs': 'javascript', '.cjs': 'javascript',
    '.rs': 'rust', '.py': 'python', '.go': 'go', '.java': 'java', '.kt': 'kotlin', '.c': 'c', '.h': 'c', '.cc': 'cpp', '.cpp': 'cpp',
    '.cs': 'csharp', '.rb': 'ruby', '.php': 'php', '.swift': 'swift', '.sh': 'shell', '.ps1': 'powershell', '.html': 'html', '.css': 'css', '.vue': 'vue', '.svelte': 'svelte'
  };
  const languageCounts = new Map<string, number>();
  const entrypoints = new Set<string>();
  const testRoots = new Set<string>();
  for (const entry of scanned) {
    const full = resolveInside(root, entry.path);
    const name = path.basename(entry.path);
    const manifest = manifestKinds[name];
    if (manifest) {
      manifests.push({ path: entry.path, kind: manifest.kind, language: manifest.language, suggested_commands: manifest.commands });
      if (name === 'package.json') {
        try {
          const parsed = JSON.parse(await readText(full, 1_048_576)) as { scripts?: Record<string, unknown> };
          for (const [script, command] of Object.entries(parsed.scripts ?? {})) if (typeof command === 'string') packageScripts[script] = command;
        } catch { /* invalid manifest is still reported */ }
      }
    }
    const language = languageNames[path.extname(name).toLowerCase()];
    if (language) languageCounts.set(language, (languageCounts.get(language) ?? 0) + 1);
    if (/^(main|index|app|server|cli)\.[^.]+$/i.test(name) || ['Cargo.toml', 'package.json'].includes(name)) entrypoints.add(entry.path);
    if (/(^|\/)(test|tests|spec|__tests__)(\/|$)|\.(test|spec)\.[^.]+$/i.test(entry.path)) testRoots.add(path.dirname(entry.path).replaceAll('\\', '/'));
  }
  const languages = [...languageCounts.entries()]
    .sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]))
    .map(([language, count]) => ({ language, files: count }));
  const seenCommands = new Set<string>();
  const suggestedCommands: JsonObject[] = [];
  for (const manifest of manifests) {
    for (const command of manifest.suggested_commands as string[]) {
      if (!seenCommands.has(command)) { seenCommands.add(command); suggestedCommands.push({ command, source: manifest.path }); }
    }
  }
  for (const script of ['test', 'lint', 'format', 'fmt', 'build', 'check']) {
    if (packageScripts[script]) {
      const command = `npm run ${script}`;
      if (!seenCommands.has(command)) { seenCommands.add(command); suggestedCommands.push({ command, source: 'package.json#scripts' }); }
    }
  }
  return ok({
    path: selected.display,
    scanned_files: scanned.length,
    scan_excluded_worktree_container_count: excludedPrefixes.length,
    languages,
    manifests: manifests.sort((left, right) => String(left.path).localeCompare(String(right.path))),
    entrypoints: [...entrypoints].sort(),
    test_roots: [...testRoots].sort(),
    package_scripts: packageScripts,
    suggested_commands: suggestedCommands,
    tree: tree.sort((left, right) => left.path.localeCompare(right.path)),
    truncated: files.length > maxFiles,
    tree_truncated: entries.length > maxEntries,
    warnings: files.length > maxFiles ? ['file scan limit reached'] : []
  });
}

export async function listFilesTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const selected = await resolveExistingPath(rootAndCwd(ctx, key).root, String(args.path ?? '.'));
  const root = selected.root;
  const base = selected.full;
  if (!(await stat(base)).isDirectory()) throw new Error('NOT_A_DIRECTORY');
  const maxResults = Math.max(1, Math.min(50_000, Number(args.max_results ?? 1_000)));
  const recursive = args.recursive !== false;
  const maxDepth = recursive ? Math.max(1, Math.min(20, Number(args.max_depth ?? 20))) : 1;
  const excludedPrefixes = recursive ? await workspaceScanExcludedPrefixes(root, base) : [];
  let entries = await walk(root, base, {
    maxDepth: recursive ? maxDepth : 0,
    maxResults: 50_000,
    includeDirectories: true,
    includeHidden: args.include_hidden === true,
    includeIgnored: args.include_ignored === true,
    includeGenerated: args.include_generated === true,
    excludedPrefixes
  });
  const rawPatterns = [...(Array.isArray(args.patterns) ? args.patterns.map(String) : []), ...(args.glob ? [String(args.glob)] : [])];
  const includePatterns = (rawPatterns.length ? rawPatterns : ['**']).map(globRegex);
  const excludePatterns = (Array.isArray(args.exclude_patterns) ? args.exclude_patterns : []).map(String).map(globRegex);
  entries = entries.filter(entry => includePatterns.some(pattern => pattern.test(entry.path)) && !excludePatterns.some(pattern => pattern.test(entry.path)));
  const entryTypes = new Set(Array.isArray(args.entry_types) && args.entry_types.length ? args.entry_types.map(String) : ['file', 'symlink']);
  entries = entries.filter(entry => entryTypes.has(entry.type)).sort((left, right) => left.path.localeCompare(right.path));
  const truncated = entries.length > maxResults;
  const output = entries.slice(0, maxResults).map(entry => ({ path: entry.path, type: entry.type, size_bytes: entry.size ?? 0, modified: entry.modified ?? null }));
  return ok({
    path: selected.display,
    entries: output,
    returned_count: output.length,
    scan_excluded_worktree_container_count: excludedPrefixes.length,
    entry_types: [...entryTypes],
    recursive,
    max_depth: maxDepth,
    truncated,
    warnings: truncated ? ['result limit reached'] : []
  });
}

export interface SearchTextExecutionOptions {
  exactTotalFastTail?: boolean;
}

export async function searchTextTool(
  ctx: ToolContext,
  key: string,
  args: JsonObject,
  options: SearchTextExecutionOptions = {}
): Promise<JsonObject> {
  const selected = await resolveExistingPath(rootAndCwd(ctx, key).root, String(args.path ?? '.'));
  const root = selected.root;
  const base = selected.full;
  const defaultRegex = args.regex === true;
  const defaultCase = args.case_sensitive === true;
  const rawQueries = [...(args.query ? [args.query] : []), ...(Array.isArray(args.queries) ? args.queries : [])];
  const queries = rawQueries.map(value => typeof value === 'string'
    ? { query: value, regex: defaultRegex, caseSensitive: defaultCase }
    : { query: String((value as JsonObject).query ?? ''), regex: (value as JsonObject).regex === undefined ? defaultRegex : (value as JsonObject).regex === true, caseSensitive: (value as JsonObject).case_sensitive === undefined ? defaultCase : (value as JsonObject).case_sensitive === true });
  if (queries.some(query => !query.query)) throw new Error('queries[].query is required');
  const filenameQuery = args.filename_query === undefined ? undefined : String(args.filename_query);
  if (!queries.length && !filenameQuery) throw new Error('query, queries, or filename_query is required');
  const requestedMax = Number(args.max_results ?? 200);
  const maxResults = Math.max(1, Math.min(10_000, requestedMax));
  const cursor = Math.max(0, Number(args.cursor ?? 0));
  const requestedPreview = Number(args.max_preview_bytes ?? 512);
  const maxPreview = Math.max(64, Math.min(4_096, requestedPreview));
  const requestedContext = Number(args.context_lines ?? 0);
  const contextLines = Math.max(0, Math.min(20, requestedContext));
  const maxFileBytes = Math.max(1_024, Math.min(134_217_728, Number(args.max_file_bytes ?? 8 * 1024 * 1024)));
  const maxMatchesPerFile = args.max_matches_per_file === undefined ? Number.MAX_SAFE_INTEGER : Math.max(1, Number(args.max_matches_per_file));
  const filesOnly = args.files_only === true;
  const countOnly = args.count_only === true;
  const calculateTotal = countOnly || args.calculate_total === true;
  const exactTotalFastTail = options.exactTotalFastTail === true && calculateTotal && !countOnly;
  const includeRaw = [...(Array.isArray(args.include_globs) ? args.include_globs.map(String) : []), ...(args.glob ? [String(args.glob)] : [])];
  const include = includeRaw.map(globRegex);
  const exclude = (Array.isArray(args.exclude_globs) ? args.exclude_globs : []).map(String).map(globRegex);
  const filenameMatcher = filenameQuery ? regexFor(filenameQuery, args.filename_regex === true, args.filename_case_sensitive === true) : undefined;
  const queryMatchers = queries.map(query => regexFor(query.query, query.regex, query.caseSensitive, true));
  const excludedPrefixes = await workspaceScanExcludedPrefixes(root, base);
  const entries = (await walk(root, base, {
    maxDepth: 20,
    maxResults: 50_000,
    includeHidden: args.include_hidden === true,
    includeIgnored: args.include_ignored === true,
    includeGenerated: args.include_generated === true,
    excludedPrefixes
  })).filter(entry => entry.type === 'file').sort((left, right) => left.path.localeCompare(right.path));
  const matches: JsonObject[] = [];
  const files: JsonObject[] = [];
  const responseBytesLimit = 128 * 1024;
  const contextLineBytesLimit = Math.min(maxPreview, 2_048);
  let responseBytesEstimate = 0;
  let responseLimitReached = false;
  const queryCounts = queries.map(() => 0);
  let filesConsidered = 0;
  let scannedFiles = 0;
  let matchedFiles = 0;
  let skippedLargeFiles = 0;
  let skipped = 0;
  let truncated = false;
  let stoppedEarly = false;
  let stop = false;
  let fastTailActive = false;
  let fastTailSkippedMatchDetails = 0;

  for (const entry of entries) {
    if (stop) break;
    if (include.length && !include.some(pattern => pattern.test(entry.path))) continue;
    if (exclude.some(pattern => pattern.test(entry.path))) continue;
    if (filenameMatcher && !filenameMatcher.test(entry.path)) continue;
    filesConsidered += 1;
    if (filesOnly && !queries.length) {
      if (skipped++ < cursor) continue;
      if (files.length >= maxResults) {
        truncated = true;
        if (calculateTotal) continue;
        stoppedEarly = true;
        break;
      }
      files.push({ path: entry.path, match_id: stableMatchId(entry.path, 0, 0, 'filename'), matched_by: 'filename' });
      continue;
    }
    if ((entry.size ?? 0) > maxFileBytes) { skippedLargeFiles += 1; continue; }
    let text: string;
    try { text = await readText(resolveInside(root, entry.path), maxFileBytes); } catch { continue; }
    scannedFiles += 1;
    const lines = text.split(/\r?\n/);
    let fileMatchCount = 0;
    let fileRecorded = false;
    for (let queryIndex = 0; queryIndex < queries.length && !stop; queryIndex += 1) {
      const query = queries[queryIndex];
      for (let lineIndex = 0; lineIndex < lines.length && !stop; lineIndex += 1) {
        const matcher = queryMatchers[queryIndex];
        matcher.lastIndex = 0;
        for (const found of lines[lineIndex].matchAll(matcher)) {
          queryCounts[queryIndex] += 1;
          if (!fileRecorded) { matchedFiles += 1; fileRecorded = true; }
          if (fastTailActive) {
            fastTailSkippedMatchDetails += 1;
            continue;
          }
          fileMatchCount += 1;
          if (fileMatchCount > maxMatchesPerFile) continue;
          if (skipped < cursor) { skipped += 1; continue; }
          if (filesOnly) {
            if (!files.some(item => item.path === entry.path)) {
              if (files.length >= maxResults) {
                truncated = true;
                if (calculateTotal) {
                  if (exactTotalFastTail) {
                    fastTailActive = true;
                    fastTailSkippedMatchDetails += 1;
                  }
                  continue;
                }
                stoppedEarly = true;
                stop = true;
                break;
              }
              files.push({ path: entry.path, match_id: stableMatchId(entry.path, lineIndex + 1, queryIndex, query.query), matched_by: 'content', query_index: queryIndex, query: query.query });
            }
            continue;
          }
          if (countOnly) continue;
          if (responseLimitReached) continue;
          if (matches.length >= maxResults) {
            truncated = true;
            if (calculateTotal) {
              if (exactTotalFastTail) {
                fastTailActive = true;
                fastTailSkippedMatchDetails += 1;
              }
              continue;
            }
            stoppedEarly = true;
            stop = true;
            break;
          }
          const start = found.index ?? 0;
          const value = found[0] ?? '';
          const end = start + value.length;
          const boundedMatch = truncatePrefixUtf8(value, maxPreview);
          const boundedQuery = truncatePrefixUtf8(query.query, 1_024);
          const item: JsonObject = {
            match_id: stableMatchId(entry.path, lineIndex + 1, queryIndex, query.query),
            path: entry.path,
            line: lineIndex + 1,
            column: [...lines[lineIndex].slice(0, start)].length + 1,
            end_column: [...lines[lineIndex].slice(0, end)].length + 1,
            query_index: queryIndex,
            query: boundedQuery.content,
            match: boundedMatch.content,
            preview: previewAround(lines[lineIndex], start, end, maxPreview)
          };
          if (boundedQuery.truncated) item.query_truncated = true;
          if (boundedMatch.truncated) item.match_truncated = true;
          if (contextLines > 0) {
            let contextTruncated = false;
            const boundContextLine = (line: string) => {
              const bounded = truncatePrefixUtf8(line, contextLineBytesLimit);
              contextTruncated ||= bounded.truncated;
              return bounded.content;
            };
            item.before = lines.slice(Math.max(0, lineIndex - contextLines), lineIndex).map(boundContextLine);
            item.after = lines.slice(lineIndex + 1, Math.min(lines.length, lineIndex + 1 + contextLines)).map(boundContextLine);
            if (contextTruncated) item.context_truncated = true;
          }
          let itemBytes = Buffer.byteLength(JSON.stringify(item));
          if (matches.length === 0 && itemBytes > responseBytesLimit && (item.before || item.after)) {
            delete item.before;
            delete item.after;
            item.context_omitted_due_to_response_limit = true;
            itemBytes = Buffer.byteLength(JSON.stringify(item));
          }
          if (responseBytesEstimate + itemBytes > responseBytesLimit) {
            truncated = true;
            responseLimitReached = true;
            if (calculateTotal) {
              if (exactTotalFastTail) {
                fastTailActive = true;
                fastTailSkippedMatchDetails += 1;
              }
              continue;
            }
            stoppedEarly = true;
            stop = true;
            break;
          }
          responseBytesEstimate += itemBytes;
          matches.push(item);
        }
      }
    }
  }
  const returned = filesOnly ? files.length : matches.length;
  const normalized = requestedMax !== maxResults || requestedPreview !== maxPreview || requestedContext !== contextLines;
  const exactTotalForcedFullScan = calculateTotal && !countOnly && truncated && !stoppedEarly;
  const truncationReason = responseLimitReached ? 'response_limit' : truncated ? 'result_limit' : null;
  const earlyStopReason = stoppedEarly ? truncationReason : null;
  const searchRecommendation = exactTotalForcedFullScan
    ? `calculate_total=true forced a full scan after the ${responseLimitReached ? 'response payload' : 'result'} limit was reached. Omit calculate_total for discovery, narrow path/include_globs/filename_query, or use count_only=true when only the exact count is needed.`
    : responseLimitReached
      ? 'Search stopped at the response payload limit. Continue with next_cursor, reduce context_lines/max_preview_bytes, or narrow path/include_globs.'
      : stoppedEarly && String(args.path ?? '.') === '.' && !includeRaw.length && !filenameQuery
        ? 'Search stopped at max_results. Narrow path/include_globs/filename_query, or set calculate_total=true only when an exact total is required.'
        : null;
  const warnings = [
    ...(truncated ? [responseLimitReached ? 'response payload limit reached' : 'result limit reached'] : []),
    ...(exactTotalForcedFullScan ? [`calculate_total forced a full scan after the ${responseLimitReached ? 'response payload' : 'result'} limit was reached`] : [])
  ];
  return ok({
    query: args.query ?? null,
    queries: queries.map((query, index) => ({ index, query: query.query, regex: query.regex, case_sensitive: query.caseSensitive, matches: queryCounts[index] })),
    filename_query: filenameQuery ?? null,
    matches: filesOnly || countOnly ? [] : matches,
    files,
    total_matches: queryCounts.reduce((sum, count) => sum + count, 0),
    total_matches_exact: !stoppedEarly,
    calculate_total: calculateTotal,
    returned_count: returned,
    matched_files: matchedFiles,
    files_considered: filesConsidered,
    scan_excluded_worktree_container_count: excludedPrefixes.length,
    scanned_files: scannedFiles,
    skipped_large_files: skippedLargeFiles,
    cursor,
    next_cursor: truncated ? cursor + returned : null,
    scan_completed: !stoppedEarly,
    early_stop_reason: earlyStopReason,
    search_recommendation: searchRecommendation,
    exact_total_forced_full_scan: exactTotalForcedFullScan,
    exact_total_fast_tail: fastTailActive,
    exact_total_fast_tail_skipped_match_details: fastTailSkippedMatchDetails,
    response_bytes_limit: responseBytesLimit,
    response_bytes_estimate: responseBytesEstimate,
    context_line_bytes_limit: contextLineBytesLimit,
    response_limit_reached: responseLimitReached,
    arguments_normalized: normalized,
    normalized_arguments: normalized ? { max_results: maxResults, max_preview_bytes: maxPreview, context_lines: contextLines } : null,
    truncated,
    warnings
  });
}

export async function patchCheckTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const patch = String(args.patch ?? '');
  if (!patch) throw new Error('patch is required');
  const preflightFailure = await preflightPatch(ctx, key, args);
  if (preflightFailure) return preflightFailure;
  const { cwd } = rootAndCwd(ctx, key);
  const result = await runGitBuffered(cwd, ['apply', '--check', '-'], patch, 30_000);
  return result.code === 0
    ? ok({ valid: true, preflight: true, stdout: result.stdout, stderr: result.stderr })
    : fail('PATCH_CHECK_FAILED', result.stderr || result.stdout, {
      recommended_tool: 'edit',
      suggestion: 'Read the current target content and retry with precise guarded edits.',
      recovery_actions: [{
        action: 'switch_to_precise_edits',
        tool: 'edit',
        required_arguments: ['files'],
        reason: 'git_patch_check_failed'
      }]
    });
}

export async function applyPatchTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const checked = await patchCheckTool(ctx, key, args);
  if (checked.ok !== true || args.dry_run === true) return { ...checked, dry_run: args.dry_run === true, applied: false };
  const { cwd } = rootAndCwd(ctx, key);
  const result = await runGitBuffered(cwd, ['apply', '-'], String(args.patch), 30_000);
  return result.code === 0
    ? ok({ applied: true, preflight: true, stdout: result.stdout, stderr: result.stderr })
    : fail('PATCH_APPLY_FAILED', result.stderr || result.stdout, {
      suggestion: 'The workspace changed after preflight. Read the target files and rebuild the patch.',
      recovery_actions: [{
        action: 'read_current_files',
        tool: 'read_many',
        required_arguments: ['items'],
        reason: 'workspace_changed_after_patch_preflight'
      }]
    });
}

interface TextRange {
  start: number;
  end: number;
}

function escapeRegexCharacter(value: string): string {
  return '\\^$.*+?()[]{}|'.includes(value) ? `\\${value}` : value;
}

function textPattern(target: string, matchMode: string): string {
  let pattern = '';
  for (let index = 0; index < target.length;) {
    const character = target[index];
    if (matchMode === 'whitespace' && /\s/.test(character)) {
      while (index < target.length && /\s/.test(target[index])) index += 1;
      pattern += '\\s+';
      continue;
    }
    if (character === '\r' && target[index + 1] === '\n') {
      pattern += '\\r?\\n';
      index += 2;
      continue;
    }
    if (character === '\n') {
      pattern += '\\r?\\n';
      index += 1;
      continue;
    }
    pattern += escapeRegexCharacter(character);
    index += 1;
  }
  return pattern;
}

function lineNumberAt(value: string, offset: number): number {
  let line = 1;
  for (let index = 0; index < Math.min(offset, value.length); index += 1) {
    if (value[index] === '\n') line += 1;
  }
  return line;
}

function lineRangeOffsets(value: string, startLine: number, endLine: number, editIndex: number): TextRange {
  const starts = [0];
  for (let index = 0; index < value.length; index += 1) {
    if (value[index] === '\n') starts.push(index + 1);
  }
  if (startLine < 1 || endLine < startLine || endLine > starts.length) {
    throw new EditContractError(
      'EDIT_LINE_RANGE_INVALID',
      `edits[${editIndex}] line range ${startLine}-${endLine} is invalid`,
      'validation',
      false,
      { edit_index: editIndex, start_line: startLine, end_line: endLine, total_lines: starts.length }
    );
  }
  return {
    start: starts[startLine - 1],
    end: endLine < starts.length ? starts[endLine] : value.length
  };
}

interface LineEditRange extends TextRange {
  contentEnd: number;
  trailingNewline: '' | '\r\n' | '\n';
  precedingNewline: '' | '\r\n' | '\n';
}

function newlineEndingAt(value: string, endOffset: number): '' | '\r\n' | '\n' {
  if (endOffset <= 0 || value[endOffset - 1] !== '\n') return '';
  return endOffset > 1 && value[endOffset - 2] === '\r' ? '\r\n' : '\n';
}

function lineEditRange(value: string, startLine: number, endLine: number, editIndex: number): LineEditRange {
  const range = lineRangeOffsets(value, startLine, endLine, editIndex);
  const trailingNewline = range.end > range.start ? newlineEndingAt(value, range.end) : '';
  const precedingNewline = newlineEndingAt(value, range.start);
  return {
    ...range,
    contentEnd: range.end - trailingNewline.length,
    trailingNewline,
    precedingNewline
  };
}

function contextMatches(value: string, range: TextRange, edit: JsonObject, matchMode: string): boolean {
  const before = typeof edit.before_context === 'string' ? edit.before_context : undefined;
  const after = typeof edit.after_context === 'string' ? edit.after_context : undefined;
  if (before !== undefined) {
    const pattern = new RegExp(`${textPattern(before, matchMode)}$`);
    if (!pattern.test(value.slice(0, range.start))) return false;
  }
  if (after !== undefined) {
    const pattern = new RegExp(`^${textPattern(after, matchMode)}`);
    if (!pattern.test(value.slice(range.end))) return false;
  }
  return true;
}

function textRanges(value: string, target: string, edit: JsonObject, editIndex: number): TextRange[] {
  const matchMode = String(edit.match_mode ?? 'exact');
  const hasStart = edit.start_line !== undefined;
  const hasEnd = edit.end_line !== undefined;
  if (hasStart !== hasEnd) {
    throw new EditContractError('INVALID_ARGUMENT', `edits[${editIndex}].start_line and end_line must be provided together`);
  }
  const search = hasStart
    ? lineRangeOffsets(value, Number(edit.start_line), Number(edit.end_line), editIndex)
    : { start: 0, end: value.length };
  const expression = new RegExp(textPattern(target, matchMode), 'g');
  const ranges: TextRange[] = [];
  const source = value.slice(search.start, search.end);
  for (const match of source.matchAll(expression)) {
    const start = search.start + (match.index ?? 0);
    const range = { start, end: start + match[0].length };
    if (contextMatches(value, range, edit, matchMode)) ranges.push(range);
    if (match[0].length === 0) expression.lastIndex += 1;
  }
  return ranges;
}

function candidateDetails(value: string, ranges: TextRange[]): JsonObject {
  const lines = value.split(/\r?\n/);
  const contexts = ranges.slice(0, 8).map(range => {
    const startLine = lineNumberAt(value, range.start);
    const endLine = lineNumberAt(value, Math.max(range.start, range.end - 1));
    const contextStart = Math.max(1, startLine - 3);
    const contextEnd = Math.min(lines.length, endLine + 3);
    return {
      start_line: startLine,
      end_line: endLine,
      context_start_line: contextStart,
      context_end_line: contextEnd,
      preview: lines.slice(contextStart - 1, contextEnd)
    };
  });
  return {
    candidate_lines: ranges.map(range => lineNumberAt(value, range.start)),
    candidate_ranges: ranges.map(range => ({
      start_line: lineNumberAt(value, range.start),
      end_line: lineNumberAt(value, Math.max(range.start, range.end - 1))
    })),
    candidate_contexts: contexts,
    candidate_context_limit: 8,
    candidate_contexts_truncated: ranges.length > 8
  };
}

function resolveTextRanges(value: string, target: string, edit: JsonObject, editIndex: number): TextRange[] {
  const ranges = textRanges(value, target, edit, editIndex);
  const expected = Math.max(1, Number(edit.expected_occurrences ?? 1));
  if (ranges.length !== expected) {
    throw new EditContractError(
      'EDIT_MATCH_COUNT_MISMATCH',
      `edits[${editIndex}] expected ${expected} guarded matches but found ${ranges.length}`,
      'validation',
      false,
      {
        edit_index: editIndex,
        expected_occurrences: expected,
        actual_occurrences: ranges.length,
        ...candidateDetails(value, ranges),
        recovery_reason: ranges.length === 0 ? 'target_text_not_found' : 'target_text_not_unique'
      }
    );
  }
  return ranges;
}

function replaceTextRanges(value: string, ranges: TextRange[], replacement: (matched: string, range: TextRange) => string): string {
  let updated = value;
  for (const range of [...ranges].reverse()) {
    updated = updated.slice(0, range.start) + replacement(value.slice(range.start, range.end), range) + updated.slice(range.end);
  }
  return updated;
}

function validatePreciseEditContract(edits: JsonObject[]): void {
  const issues: JsonObject[] = [];
  for (let editIndex = 0; editIndex < edits.length; editIndex += 1) {
    const edit = edits[editIndex];
    if (!edit || typeof edit !== 'object' || Array.isArray(edit)) {
      issues.push({ edit_index: editIndex, field: null, reason: 'edit_must_be_object' });
      continue;
    }
    const editType = typeof edit.type === 'string' ? edit.type : '';
    if (!editType) {
      issues.push({ edit_index: editIndex, field: 'type', reason: 'type_required' });
      continue;
    }

    let allowed: readonly string[];
    let required: readonly string[];
    let nonEmptyStrings: readonly string[];
    switch (editType) {
      case 'replace':
        allowed = ['type', 'old_text', 'new_text', 'match_mode', 'before_context', 'after_context', 'expected_occurrences', 'start_line', 'end_line'];
        required = ['type', 'old_text', 'new_text'];
        nonEmptyStrings = ['old_text'];
        break;
      case 'insert_before':
      case 'insert_after':
        allowed = ['type', 'anchor', 'text', 'match_mode', 'before_context', 'after_context', 'expected_occurrences', 'start_line', 'end_line'];
        required = ['type', 'anchor', 'text'];
        nonEmptyStrings = ['anchor', 'text'];
        break;
      case 'replace_lines':
        allowed = ['type', 'start_line', 'end_line', 'new_text', 'expected_text'];
        required = ['type', 'start_line', 'end_line', 'new_text'];
        nonEmptyStrings = [];
        break;
      case 'delete_lines':
        allowed = ['type', 'start_line', 'end_line', 'expected_text'];
        required = ['type', 'start_line', 'end_line'];
        nonEmptyStrings = [];
        break;
      default:
        issues.push({
          edit_index: editIndex,
          field: 'type',
          edit_type: editType,
          reason: 'unsupported_type',
          allowed_values: ['replace', 'insert_before', 'insert_after', 'replace_lines', 'delete_lines']
        });
        continue;
    }

    for (const field of Object.keys(edit)) {
      if (!allowed.includes(field)) {
        issues.push({
          edit_index: editIndex,
          edit_type: editType,
          field,
          reason: 'unexpected_field',
          allowed_fields: [...allowed]
        });
      }
    }
    for (const field of required) {
      if (!Object.prototype.hasOwnProperty.call(edit, field)) {
        issues.push({ edit_index: editIndex, edit_type: editType, field, reason: 'missing_required_field' });
      }
    }

    for (const field of ['old_text', 'new_text', 'anchor', 'text', 'expected_text', 'before_context', 'after_context']) {
      if (!Object.prototype.hasOwnProperty.call(edit, field)) continue;
      const value = edit[field];
      if (typeof value !== 'string') {
        issues.push({ edit_index: editIndex, edit_type: editType, field, reason: 'field_must_be_string' });
      } else if (nonEmptyStrings.includes(field) && value.length === 0) {
        issues.push({ edit_index: editIndex, edit_type: editType, field, reason: 'field_must_be_non_empty' });
      }
    }
    if (Object.prototype.hasOwnProperty.call(edit, 'match_mode') && edit.match_mode !== 'exact' && edit.match_mode !== 'whitespace') {
      issues.push({
        edit_index: editIndex,
        edit_type: editType,
        field: 'match_mode',
        reason: 'invalid_enum_value',
        allowed_values: ['exact', 'whitespace']
      });
    }
    if (Object.prototype.hasOwnProperty.call(edit, 'expected_occurrences')) {
      const count = Number(edit.expected_occurrences);
      if (!Number.isInteger(count) || count < 1) {
        issues.push({
          edit_index: editIndex,
          edit_type: editType,
          field: 'expected_occurrences',
          reason: 'field_must_be_positive_integer'
        });
      }
    }

    const start = Number(edit.start_line);
    const end = Number(edit.end_line);
    const hasStart = Object.prototype.hasOwnProperty.call(edit, 'start_line');
    const hasEnd = Object.prototype.hasOwnProperty.call(edit, 'end_line');
    if (hasStart && (!Number.isInteger(start) || start < 1)) {
      issues.push({ edit_index: editIndex, edit_type: editType, field: 'start_line', reason: 'field_must_be_positive_integer' });
    }
    if (hasEnd && (!Number.isInteger(end) || end < 1)) {
      issues.push({ edit_index: editIndex, edit_type: editType, field: 'end_line', reason: 'field_must_be_positive_integer' });
    }
    if (['replace', 'insert_before', 'insert_after'].includes(editType) && hasStart !== hasEnd) {
      issues.push({ edit_index: editIndex, edit_type: editType, field: 'start_line,end_line', reason: 'line_range_pair_required' });
    }
    if (hasStart && hasEnd && Number.isInteger(start) && Number.isInteger(end) && end < start) {
      issues.push({
        edit_index: editIndex,
        edit_type: editType,
        field: 'end_line',
        reason: 'line_range_order_invalid',
        start_line: start,
        end_line: end
      });
    }
  }

  if (issues.length) {
    throw new EditContractError(
      'EDIT_CONTRACT_INVALID',
      'Precise edit contract validation failed',
      'validation',
      false,
      {
        issue_count: issues.length,
        issues,
        suggestion: 'Rebuild each edit using only the fields required by its type'
      }
    );
  }
}

function tryApplyOriginalSnapshotTextEdits(original: string, edits: JsonObject[]): string | null {
  if (edits.length < 2 || !edits.slice(1).some(edit => edit.start_line !== undefined && edit.end_line !== undefined)) return null;
  const operations: Array<TextRange & { replacement: string }> = [];
  try {
    for (let editIndex = 0; editIndex < edits.length; editIndex += 1) {
      const edit = edits[editIndex];
      const type = String(edit.type);
      if (!['replace', 'insert_before', 'insert_after'].includes(type)) return null;
      const target = type === 'replace' ? String(edit.old_text ?? '') : String(edit.anchor ?? '');
      if (!target) return null;
      const ranges = resolveTextRanges(original, target, edit, editIndex);
      for (const range of ranges) {
        const matched = original.slice(range.start, range.end);
        const inserted = adaptNewlinesToRange(
          String(type === 'replace' ? edit.new_text ?? '' : edit.text ?? ''),
          original,
          range.start,
          range.end
        );
        const replacement = type === 'replace'
          ? inserted
          : type === 'insert_before'
            ? `${inserted}${matched}`
            : `${matched}${inserted}`;
        operations.push({ ...range, replacement });
      }
    }
  } catch {
    return null;
  }
  const ordered = [...operations].sort((left, right) => left.start - right.start || left.end - right.end);
  for (let index = 1; index < ordered.length; index += 1) {
    if (ordered[index]!.start < ordered[index - 1]!.end) return null;
  }
  let text = original;
  for (const operation of [...ordered].reverse()) {
    text = text.slice(0, operation.start) + operation.replacement + text.slice(operation.end);
  }
  return text;
}

function applyEdits(original: string, edits: JsonObject[]): string {
  const snapshotApplied = tryApplyOriginalSnapshotTextEdits(original, edits);
  if (snapshotApplied !== null) return snapshotApplied;
  let text = original;
  for (let editIndex = 0; editIndex < edits.length; editIndex += 1) {
    const edit = edits[editIndex];
    switch (String(edit.type)) {
      case 'replace': {
        const oldText = String(edit.old_text ?? '');
        if (!oldText) throw new EditContractError('INVALID_ARGUMENT', `edits[${editIndex}].old_text is required`);
        const ranges = resolveTextRanges(text, oldText, edit, editIndex);
        text = replaceTextRanges(text, ranges, (_matched, range) =>
          adaptNewlinesToRange(String(edit.new_text ?? ''), text, range.start, range.end));
        break;
      }
      case 'replace_lines': {
        const start = Number(edit.start_line);
        const end = Number(edit.end_line);
        const range = lineEditRange(text, start, end, editIndex);
        const actual = text.slice(range.start, range.contentEnd);
        if (typeof edit.expected_text === 'string'
          && actual.replaceAll('\r\n', '\n') !== edit.expected_text.replaceAll('\r\n', '\n')) {
          throw new EditContractError(
            'EDIT_EXPECTED_TEXT_MISMATCH',
            `edits[${editIndex}] line range content did not match expected_text`,
            'conflict',
            true,
            { edit_index: editIndex, start_line: start, end_line: end, actual_text: actual }
          );
        }
        const replacement = adaptNewlinesToRange(String(edit.new_text ?? ''), text, range.start, range.end);
        text = text.slice(0, range.start) + replacement + text.slice(range.contentEnd);
        break;
      }
      case 'insert_before':
      case 'insert_after': {
        const anchor = String(edit.anchor ?? '');
        if (!anchor) throw new EditContractError('INVALID_ARGUMENT', `edits[${editIndex}].anchor is required`);
        const ranges = resolveTextRanges(text, anchor, edit, editIndex);
        text = replaceTextRanges(text, ranges, (matched, range) => {
          const insertion = adaptNewlinesToRange(String(edit.text ?? ''), text, range.start, range.end);
          return edit.type === 'insert_before'
            ? `${insertion}${matched}`
            : `${matched}${insertion}`;
        });
        break;
      }
      case 'delete_lines': {
        const start = Number(edit.start_line);
        const end = Number(edit.end_line);
        const range = lineEditRange(text, start, end, editIndex);
        const actual = text.slice(range.start, range.contentEnd);
        if (typeof edit.expected_text === 'string'
          && actual.replaceAll('\r\n', '\n') !== edit.expected_text.replaceAll('\r\n', '\n')) {
          throw new EditContractError(
            'EDIT_EXPECTED_TEXT_MISMATCH',
            `edits[${editIndex}] line range content did not match expected_text`,
            'conflict',
            true,
            { edit_index: editIndex, start_line: start, end_line: end, actual_text: actual }
          );
        }
        const deleteStart = range.trailingNewline || !range.precedingNewline
          ? range.start
          : range.start - range.precedingNewline.length;
        text = text.slice(0, deleteStart) + text.slice(range.end);
        break;
      }
      default:
        throw new EditContractError('INVALID_ARGUMENT', `Unsupported edits[${editIndex}].type: ${String(edit.type)}`);
    }
  }
  return text;
}

const MAX_DIRECT_EDIT_RECOVERY_BYTES = 32 * 1024;

function boundedDirectRecoveryAction(action: JsonObject): JsonObject | undefined {
  return Buffer.byteLength(JSON.stringify(action)) <= MAX_DIRECT_EDIT_RECOVERY_BYTES ? action : undefined;
}

function editTarget(edit: JsonObject): string | undefined {
  const type = String(edit.type ?? '');
  if (type === 'replace') return typeof edit.old_text === 'string' && edit.old_text ? edit.old_text : undefined;
  if (type === 'insert_before' || type === 'insert_after') {
    return typeof edit.anchor === 'string' && edit.anchor ? edit.anchor : undefined;
  }
  return undefined;
}

function guardedCandidateRecoveryActions(
  original: string,
  file: string,
  actualSha256: string,
  edits: JsonObject[],
  error: EditContractError
): JsonObject[] {
  if (error.code !== 'EDIT_MATCH_COUNT_MISMATCH' || edits.length !== 1 || Number(error.details.edit_index ?? -1) !== 0) return [];
  const edit = edits[0]!;
  const target = editTarget(edit);
  if (!target) return [];
  let ranges: TextRange[];
  try {
    ranges = textRanges(original, target, edit, 0);
  } catch {
    return [];
  }
  if (ranges.length <= 1) return [];
  const actions: JsonObject[] = [];
  for (let candidateIndex = 0; candidateIndex < Math.min(8, ranges.length); candidateIndex += 1) {
    const range = ranges[candidateIndex]!;
    const startLine = lineNumberAt(original, range.start);
    const endLine = lineNumberAt(original, Math.max(range.start, range.end - 1));
    const guardedEdit: JsonObject = {
      ...structuredClone(edit),
      expected_occurrences: 1,
      start_line: startLine,
      end_line: endLine
    };
    try {
      const guardedRanges = textRanges(original, target, guardedEdit, 0);
      if (guardedRanges.length !== 1 || guardedRanges[0]!.start !== range.start || guardedRanges[0]!.end !== range.end) continue;
      validatePreciseEditContract([guardedEdit]);
      const updated = applyEdits(original, [guardedEdit]);
      if (updated === original) continue;
      assertNoUnexpectedNewlineChurn(file, original, updated);
      assertNoUnexpectedEditBlastRadius(file, original, updated, [guardedEdit]);
    } catch {
      continue;
    }
    const action = boundedDirectRecoveryAction({
      action: 'retry_match_candidate',
      action_id: `edit-candidate-${candidateIndex + 1}`,
      tool: 'edit',
      required_arguments: [],
      candidate_line: startLine,
      arguments: {
        files: [{
          path: file,
          expected_sha256: actualSha256,
          edits: [guardedEdit]
        }]
      },
      reason: 'edit_match_candidate_revalidated'
    });
    if (action) actions.push(action);
  }
  return actions;
}

function directRetryGuardedEditAction(
  original: string,
  file: string,
  actualSha256: string,
  edits: JsonObject[]
): JsonObject | undefined {
  if (edits.length === 0) return undefined;
  try {
    validatePreciseEditContract(edits);
    const updated = applyEdits(original, edits);
    if (updated === original) return undefined;
    assertNoUnexpectedNewlineChurn(file, original, updated);
    assertNoUnexpectedEditBlastRadius(file, original, updated, edits);
  } catch {
    return undefined;
  }
  return boundedDirectRecoveryAction({
    action: 'retry_guarded_edit',
    action_id: 'edit-current-version',
    tool: 'edit',
    required_arguments: [],
    arguments: {
      files: [{
        path: file,
        expected_sha256: actualSha256,
        edits: structuredClone(edits)
      }]
    },
    reason: 'edit_revalidated_on_current_content'
  });
}

function staleSingleFileEditError(
  file: string,
  expectedSha256: string,
  actualSha256: string,
  original: string,
  edits: JsonObject[]
): EditContractError {
  const base = fileVersionMismatch(file, expectedSha256, actualSha256);
  const direct = directRetryGuardedEditAction(original, file, actualSha256, edits);
  if (!direct) return base;
  return new EditContractError(base.code, base.message, base.category, base.retryable, {
    ...base.details,
    direct_recovery_action_count: 1,
    recovery_actions: [direct, ...(Array.isArray(base.details.recovery_actions) ? base.details.recovery_actions as JsonObject[] : [])]
  });
}

function enrichSingleFileEditError(
  error: unknown,
  file: string,
  actualSha256: string,
  original: string,
  edits: JsonObject[]
): unknown {
  const enriched = enrichEditError(error, file, actualSha256);
  if (!(enriched instanceof EditContractError)) return enriched;
  const direct = guardedCandidateRecoveryActions(original, file, actualSha256, edits, enriched);
  if (!direct.length) return enriched;
  return new EditContractError(enriched.code, enriched.message, enriched.category, enriched.retryable, {
    ...enriched.details,
    direct_recovery_action_count: direct.length,
    recovery_actions: [
      ...direct,
      ...(Array.isArray(enriched.details.recovery_actions) ? enriched.details.recovery_actions as JsonObject[] : [])
    ]
  });
}

interface PreparedEdit {
  file: string;
  path: string;
  original: string;
  originalBytes: Buffer;
  updated: string;
  updatedBytes: Buffer;
  encoding: TextEncoding;
  bom: boolean;
  beforeHash: string;
  blastRadius: EditBlastRadius;
}

function enrichEditError(error: unknown, file: string, actualSha256: string): unknown {
  if (!(error instanceof EditContractError)) return error;
  return new EditContractError(error.code, error.message, error.category, error.retryable, {
    ...error.details,
    path: error.details.path ?? file,
    actual_sha256: error.details.actual_sha256 ?? actualSha256,
    recovery_actions: error.details.recovery_actions ?? editRecoveryActions(file, actualSha256, error.code)
  });
}

async function prepareEdit(ctx: ToolContext, key: string, args: JsonObject): Promise<PreparedEdit> {
  const { root } = rootAndCwd(ctx, key);
  const requested = String(args.path ?? '');
  if (!requested) throw new EditContractError('INVALID_ARGUMENT', 'path is required');
  const resolved = await resolveExistingWritePath(root, requested);
  const file = resolved.full;
  const display = resolved.display;
  const decoded = await readDecodedTextFile(file);
  const original = decoded.text;
  const originalBytes = decoded.bytes;
  const beforeHash = createHash('sha256').update(originalBytes).digest('hex');
  if (args.expected_sha256 && String(args.expected_sha256).toLowerCase() !== beforeHash.toLowerCase()) {
    throw fileVersionMismatch(display, String(args.expected_sha256), beforeHash);
  }
  const edits = Array.isArray(args.edits) ? args.edits as JsonObject[] : [];
  if (!edits.length) throw new EditContractError('INVALID_ARGUMENT', 'edits are required');
  try {
    validatePreciseEditContract(edits);
    const updated = applyEdits(original, edits);
    assertNoUnexpectedNewlineChurn(display, original, updated);
    const blastRadius = assertNoUnexpectedEditBlastRadius(display, original, updated, edits);
    return {
      file,
      path: display,
      original,
      originalBytes,
      updated,
      updatedBytes: encodeText(updated, decoded.encoding, decoded.bom),
      encoding: decoded.encoding,
      bom: decoded.bom,
      beforeHash,
      blastRadius
    };
  } catch (error) {
    throw enrichEditError(error, display, beforeHash);
  }
}

export async function editFileTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  try {
    const started = performance.now();
    const { root } = rootAndCwd(ctx, key);
    const requested = String(args.path ?? '');
    if (!requested) throw new EditContractError('INVALID_ARGUMENT', 'path is required');
    if (args.edits !== undefined && args.apply_proposal !== undefined) {
      throw new EditContractError(
        'EDIT_CONTRACT_INVALID',
        'edit_file accepts either edits or apply_proposal, not both',
        'validation',
        false,
        {
          path: requested,
          issue_count: 1,
          issues: [{ field: 'edits,apply_proposal', reason: 'mutually_exclusive_fields' }],
          suggestion: 'Send precise edits or apply one stored proposal',
          recovery_actions: [{
            action: 'choose_edit_mode',
            tool: 'edit',
            arguments: { files: [{ path: requested }] },
            required_arguments: ['files[0].edits_or_apply_proposal'],
            reason: 'mutually_exclusive_fields'
          }]
        }
      );
    }
    const resolved = await resolveExistingWritePath(root, requested);
    const file = resolved.full;
    const display = resolved.display;
    const decoded = await readDecodedTextFile(file);
    const original = decoded.text;
    const beforeHash = createHash('sha256').update(decoded.bytes).digest('hex');
    const directEdits = Array.isArray(args.edits) ? args.edits as JsonObject[] : [];
    if (args.expected_sha256 && String(args.expected_sha256).toLowerCase() !== beforeHash.toLowerCase()) {
      throw staleSingleFileEditError(display, String(args.expected_sha256), beforeHash, original, directEdits);
    }

    let updated: string;
    let proposalId: string | undefined;
    let proposalApplyFormat: 'direct' | 'accept' | 'replacement' | 'patch' = 'direct';
    if (args.apply_proposal !== undefined) {
      const applied = applyEditProposal(ctx, key, display, beforeHash, original, args.apply_proposal);
      updated = applied.updated;
      proposalId = applied.proposalId;
      proposalApplyFormat = applied.applyFormat;
    } else {
      const edits = directEdits;
      if (!edits.length) throw new EditContractError('INVALID_ARGUMENT', 'edits or apply_proposal is required');
      try {
        validatePreciseEditContract(edits);
      } catch (error) {
        throw enrichEditError(error, display, beforeHash);
      }
      try {
        updated = applyEdits(original, edits);
      } catch (error) {
        const proposal = buildEditProposal(ctx, key, display, beforeHash, original, edits);
        if (proposal) return ok(proposal);
        throw enrichSingleFileEditError(error, display, beforeHash, original, edits);
      }
    }

    if (updated === original) throw new EditContractError('PATCH_FAILED', 'Edits produced no changes.');
    assertNoUnexpectedNewlineChurn(display, original, updated);
    const blastRadius = assertNoUnexpectedEditBlastRadius(display, original, updated, directEdits);
    const updatedBytes = encodeText(updated, decoded.encoding, decoded.bom);
    const afterHash = createHash('sha256').update(updatedBytes).digest('hex');
    const dryRun = args.dry_run === true;
    const preflightFinished = performance.now();
    const editPlan = dryRun ? (() => {
      const replayFile: JsonObject = {
        path: display,
        expected_sha256: beforeHash
      };
      if (args.edits !== undefined) replayFile.edits = args.edits;
      if (args.apply_proposal !== undefined) replayFile.apply_proposal = args.apply_proposal;
      const replayArguments: JsonObject = { files: [replayFile], dry_run: false };
      if (typeof args.reason === 'string' && args.reason.length > 0) replayArguments.reason = args.reason;
      const statefulDependencies = proposalId ? [{
        type: 'edit_proposal',
        proposal_id: proposalId,
        ttl_seconds: 300
      }] : [];
      return replayableEditPlan('edit', replayArguments, [{
        path: display,
        before_sha256: beforeHash,
        after_sha256: afterHash
      }], statefulDependencies);
    })() : null;
    const planFinished = performance.now();
    if (!dryRun) {
      const currentHash = await sha256File(file);
      if (currentHash.toLowerCase() !== beforeHash.toLowerCase()) throw fileVersionMismatch(display, beforeHash, currentHash);
      await writeFile(file, updatedBytes);
      if (proposalId) removeEditProposal(ctx, key, proposalId);
    }
    const completed = performance.now();
    const phaseDurationsMs = {
      preflight_ms: Math.max(0, Math.round(preflightFinished - started)),
      plan_ms: Math.max(0, Math.round(planFinished - preflightFinished)),
      commit_ms: Math.max(0, Math.round(completed - planFinished)),
      total_ms: Math.max(0, Math.round(completed - started))
    };
    const boundedDiff = editResultDiff(display, original, updated);
    return ok({
      status: proposalId ? 'proposal_applied' : 'edited',
      proposal_id: proposalId ?? null,
      proposal_apply_format: proposalApplyFormat,
      dry_run: dryRun,
      preflight: true,
      applied: !dryRun,
      clean: true,
      change_id: dryRun ? null : randomUUID().replaceAll('-', ''),
      path: display,
      operation: 'update',
      before_sha256: beforeHash,
      after_sha256: afterHash,
      edit_plan: editPlan,
      encoding: decoded.encoding,
      newline_before: newlineStyle(original),
      newline_after: newlineStyle(updated),
      newline_guard: 'passed',
      blast_radius_guard: 'passed',
      blast_radius: blastRadius,
      phase_durations_ms: phaseDurationsMs,
      bom: decoded.bom,
      diff: boundedDiff.content,
      diff_bytes: boundedDiff.bytes,
      diff_truncated: boundedDiff.truncated,
      affected_files: [{ path: display, operation: 'update' }],
      files_created: [],
      files_modified: [display],
      files_deleted: [],
      recovery: dryRun ? null : 'git',
      warnings: []
    });
  } catch (error) {
    if (error instanceof EditContractError) return editFailure(error);
    throw error;
  }
}

function enrichEditManyError(error: unknown, fileIndex: number): unknown {
  if (!(error instanceof EditContractError)) return error;
  return new EditContractError(error.code, error.message, error.category, error.retryable, {
    ...error.details,
    file_index: error.details.file_index ?? fileIndex
  });
}

export async function editTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const files = Array.isArray(args.files) ? args.files as JsonObject[] : [];
  if (!files.length) return editFailure(new EditContractError('INVALID_ARGUMENT', 'files are required'));
  if (files.length > 100) return editFailure(new EditContractError('INVALID_ARGUMENT', 'edit supports at most 100 files'));
  if (files.length === 1) {
    const file = files[0];
    if (!file || typeof file !== 'object' || Array.isArray(file)) {
      return editFailure(new EditContractError('INVALID_ARGUMENT', 'files[0] must be an object'));
    }
    const single: JsonObject = {};
    for (const field of ['path', 'expected_sha256', 'edits', 'apply_proposal']) {
      if (file[field] !== undefined) single[field] = file[field];
    }
    if (args.dry_run !== undefined) single.dry_run = args.dry_run;
    if (args.reason !== undefined) single.reason = args.reason;
    const result = await editFileTool(ctx, key, single);
    return result.ok === true ? { ...result, atomic: true } : result;
  }
  if (files.some(file => file && typeof file === 'object' && !Array.isArray(file) && file.apply_proposal !== undefined)) {
    return editFailure(new EditContractError(
      'EDIT_CONTRACT_INVALID',
      'apply_proposal is supported only when edit contains one file',
      'validation',
      false,
      {
        file_count: files.length,
        suggestion: 'Apply a proposal in a single-file edit call',
        recovery_actions: [{
          action: 'split_proposal_edit',
          tool: 'edit',
          required_arguments: ['files'],
          reason: 'proposal_requires_single_file'
        }]
      }
    ));
  }
  return editManyTool(ctx, key, args);
}

export async function editManyTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  try {
    const started = performance.now();
    const files = Array.isArray(args.files) ? args.files as JsonObject[] : [];
    if (!files.length) throw new EditContractError('INVALID_ARGUMENT', 'files are required');
    const prepared: PreparedEdit[] = [];
    for (let fileIndex = 0; fileIndex < files.length; fileIndex += 1) {
      try {
        prepared.push(await prepareEdit(ctx, key, files[fileIndex]));
      } catch (error) {
        throw enrichEditManyError(error, fileIndex);
      }
    }
    const preflightFinished = performance.now();
    const dryRun = args.dry_run === true;
    if (!dryRun) {
      const written: Array<{ file: string; originalBytes: Buffer }> = [];
      try {
        for (let fileIndex = 0; fileIndex < prepared.length; fileIndex += 1) {
          const item = prepared[fileIndex];
          if (item.updated === item.original) continue;
          const currentHash = await sha256File(item.file);
          if (currentHash.toLowerCase() !== item.beforeHash.toLowerCase()) {
            throw enrichEditManyError(fileVersionMismatch(item.path, item.beforeHash, currentHash), fileIndex);
          }
          await writeFile(item.file, item.updatedBytes);
          written.push({ file: item.file, originalBytes: item.originalBytes });
        }
      } catch (error) {
        await Promise.allSettled(written.map(item => writeFile(item.file, item.originalBytes)));
        throw error;
      }
    }
    const commitFinished = performance.now();
    const results = prepared.map(item => ({
      path: item.path,
      changed: item.updated !== item.original,
      before_sha256: item.beforeHash,
      after_sha256: createHash('sha256').update(item.updatedBytes).digest('hex'),
      encoding: item.encoding,
      bom: item.bom,
      newline_before: newlineStyle(item.original),
      newline_after: newlineStyle(item.updated),
      newline_guard: 'passed',
      blast_radius_guard: 'passed',
      blast_radius: item.blastRadius
    }));
    const editPlan = dryRun ? (() => {
      const replayFiles = files.map((file, fileIndex) => ({
        path: prepared[fileIndex].path,
        edits: file.edits,
        expected_sha256: prepared[fileIndex].beforeHash
      }));
      const replayArguments: JsonObject = { files: replayFiles, dry_run: false };
      if (typeof args.reason === 'string' && args.reason.length > 0) replayArguments.reason = args.reason;
      return replayableEditPlan(
        'edit',
        replayArguments,
        results.map(result => ({
          path: result.path,
          before_sha256: result.before_sha256,
          after_sha256: result.after_sha256
        }))
      );
    })() : null;
    const completed = performance.now();
    const phaseDurationsMs = {
      preflight_ms: Math.max(0, Math.round(preflightFinished - started)),
      commit_ms: Math.max(0, Math.round(commitFinished - preflightFinished)),
      plan_ms: Math.max(0, Math.round(completed - commitFinished)),
      total_ms: Math.max(0, Math.round(completed - started))
    };
    return ok({
      atomic: true,
      dry_run: dryRun,
      results,
      edit_plan: editPlan,
      phase_durations_ms: phaseDurationsMs
    });
  } catch (error) {
    if (error instanceof EditContractError) return editFailure(error);
    throw error;
  }
}



function boundedImageInteger(value: unknown, fallback: number, minimum: number, maximum: number): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.max(minimum, Math.min(maximum, Math.trunc(parsed))) : fallback;
}

function imageFailure(error: ImageContractError): JsonObject {
  return {
    ok: false,
    error: {
      code: error.code,
      message: error.message,
      category: 'validation',
      retryable: false,
      details: error.details
    }
  };
}

export async function viewImageTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const { root } = rootAndCwd(ctx, key);
  const requestedPath = String(args.path ?? '').trim();
  if (!requestedPath) throw new Error('path is required');
  const resolved = await resolveExistingPath(root, requestedPath);
  const file = resolved.full;
  const fileInfo = await stat(file);
  if (fileInfo.isDirectory()) throw new Error('IS_DIRECTORY');
  const maxBytes = boundedImageInteger(args.max_bytes, 5 * 1024 * 1024, 1024, 10 * 1024 * 1024);
  const maxWidth = boundedImageInteger(args.max_width, 2_000, 1, 10_000);
  const maxHeight = boundedImageInteger(args.max_height, 2_000, 1, 10_000);
  const autoResize = args.auto_resize !== false;
  let data: Buffer = await readFile(file);
  let image: ImageInfo;
  let raster;
  try {
    image = identifyImage(data);
    raster = decodeRaster(data, image);
    if (raster) image = { ...image, width: raster.width, height: raster.height };
  } catch (error) {
    if (error instanceof ImageContractError) return imageFailure(error);
    throw error;
  }
  const original = {
    bytes: data.length,
    width: image.width,
    height: image.height,
    mime_type: image.mimeType
  };
  let resized = false;
  const warnings: string[] = [];
  if (autoResize && shouldResize(data.length, image, maxBytes, maxWidth, maxHeight)) {
    if (raster) {
      try {
        const output = resizeDecodedImage(raster, image, maxWidth, maxHeight, maxBytes);
        if (output) {
          data = output.data;
          image = { mimeType: output.mimeType, width: output.width, height: output.height };
          resized = true;
        } else {
          warnings.push('auto_resize requested but image resize failed or format unsupported');
        }
      } catch (error) {
        warnings.push(`auto_resize failed: ${error instanceof Error ? error.message : String(error)}`);
      }
    } else {
      warnings.push(`auto_resize requested but ${image.mimeType} resize is unsupported by the pure-JavaScript Node codec`);
    }
  }
  if (data.length > maxBytes) return imageFailure(outputTooLarge(maxBytes, data.length, {
    mime_type: image.mimeType,
    width: image.width,
    height: image.height,
    auto_resize: autoResize
  }));
  const base64 = data.toString('base64');
  const dataUrl = `data:${image.mimeType};base64,${base64}`;
  const display = resolved.display;
  const result: JsonObject = {
    path: display,
    mime_type: image.mimeType,
    bytes: data.length,
    width: image.width,
    height: image.height,
    resized,
    original,
    base64,
    data_url: dataUrl,
    warnings
  };
  if (args.output !== 'data_url') result.content = [{ type: 'image', mimeType: image.mimeType, data: base64 }];
  return ok(result);
}
