import { createHash, randomUUID } from 'node:crypto';
import { access, copyFile, lstat, mkdir, mkdtemp, readFile, readdir, rename, rm, rmdir, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import type { JsonObject, ToolContext } from './types.js';
import { runBuffered } from './processes.js';
import { runGitBuffered } from './gitProcess.js';
import { parseWslUncPath } from './wsl.js';
import {
  exists, globRegex, relativeInside, resolveExistingPath, resolveInside,
  rootAndCwd, walk, WorkspacePathError
} from './workspace.js';

interface FormatterSpec {
  id: string;
  extensions: string[];
  configNames: string[];
  mutationRisk: 'targeted' | 'project';
}

interface CustomAdapter {
  id: string;
  program: string;
  extensions: string[];
  args: string[];
  configPath?: string;
}

interface CommandTemplate {
  program: string;
  args: string[];
}

interface SelectedAdapter {
  id: string;
  extensions: string[];
  configPath: string | null;
  selectionSource: string;
  mutationRisk: string;
  custom: boolean;
  commandTemplate?: CommandTemplate;
}

interface PlannedFormatFile {
  path: string;
  adapter_id: string;
  config_path: string | null;
  selection_source: string;
  rust_edition?: string;
}

interface FormatGroup {
  adapter_id: string;
  config_path: string | null;
  files: string[];
  mutation_risk: string;
  custom: boolean;
  rust_edition?: string;
  command_template?: CommandTemplate;
}

interface FormatPlan {
  scope: string;
  filesRequested: number;
  files: PlannedFormatFile[];
  groups: FormatGroup[];
  skipped: Array<{ path: string; reason: string }>;
  truncated: boolean;
}

interface OriginalFile {
  bytes: Buffer;
  sha256: string;
}

class FormatterError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly category: string,
    readonly retryable: boolean,
    readonly details: JsonObject = {}
  ) {
    super(message);
  }
}

const FORMATTER_SPECS: FormatterSpec[] = [
  { id: 'rustfmt', extensions: ['rs'], configNames: ['rustfmt.toml', '.rustfmt.toml', 'Cargo.toml'], mutationRisk: 'targeted' },
  { id: 'biome', extensions: ['js', 'jsx', 'mjs', 'cjs', 'ts', 'tsx', 'json', 'jsonc', 'css'], configNames: ['biome.json', 'biome.jsonc', 'package.json'], mutationRisk: 'targeted' },
  { id: 'dprint', extensions: ['js', 'jsx', 'mjs', 'cjs', 'ts', 'tsx', 'json', 'jsonc', 'md', 'toml'], configNames: ['dprint.json', '.dprint.json'], mutationRisk: 'targeted' },
  { id: 'prettier', extensions: ['js', 'jsx', 'mjs', 'cjs', 'ts', 'tsx', 'json', 'jsonc', 'yaml', 'yml', 'md', 'markdown', 'css', 'scss', 'less', 'html', 'vue', 'svelte'], configNames: ['.prettierrc', '.prettierrc.json', '.prettierrc.yaml', '.prettierrc.yml', '.prettierrc.js', '.prettierrc.cjs', 'prettier.config.js', 'prettier.config.cjs', 'package.json'], mutationRisk: 'targeted' },
  { id: 'ruff', extensions: ['py', 'pyi'], configNames: ['ruff.toml', '.ruff.toml', 'pyproject.toml'], mutationRisk: 'targeted' },
  { id: 'black', extensions: ['py', 'pyi'], configNames: ['pyproject.toml'], mutationRisk: 'targeted' },
  { id: 'gofmt', extensions: ['go'], configNames: ['go.mod'], mutationRisk: 'targeted' },
  { id: 'clang-format', extensions: ['c', 'h', 'cc', 'cpp', 'cxx', 'hpp', 'java', 'proto'], configNames: ['.clang-format', '_clang-format'], mutationRisk: 'targeted' },
  { id: 'csharpier', extensions: ['cs'], configNames: ['.csharpierrc', '.csharpierrc.json'], mutationRisk: 'targeted' },
  { id: 'ktfmt', extensions: ['kt', 'kts'], configNames: [], mutationRisk: 'targeted' },
  { id: 'ktlint', extensions: ['kt', 'kts'], configNames: ['.editorconfig'], mutationRisk: 'targeted' },
  { id: 'shfmt', extensions: ['sh', 'bash', 'zsh'], configNames: ['.editorconfig'], mutationRisk: 'targeted' },
  { id: 'terraform-fmt', extensions: ['tf', 'tfvars', 'hcl'], configNames: [], mutationRisk: 'targeted' },
  { id: 'taplo', extensions: ['toml'], configNames: ['taplo.toml', '.taplo.toml'], mutationRisk: 'targeted' },
  { id: 'builtin-json', extensions: ['json'], configNames: [], mutationRisk: 'targeted' }
];

const BUILTIN_IDS = new Set(FORMATTER_SPECS.map(spec => spec.id));
const GENERATED_FORMAT_FILES = new Set([
  'Cargo.lock', 'package-lock.json', 'pnpm-lock.yaml', 'yarn.lock', 'poetry.lock',
  'Pipfile.lock', 'composer.lock', 'go.sum'
]);
const SUPPORT_NAMES = ['package.json', 'pyproject.toml', 'Cargo.toml', 'go.mod', '.editorconfig'];
const MIRROR_IGNORED_PARTS = new Set(['node_modules', 'target', '.git', '.cache', '.ruff_cache', '__pycache__']);

function ok(value: JsonObject): JsonObject { return { ok: true, ...value }; }
function fail(error: FormatterError): JsonObject {
  return { ok: false, error: { code: error.code, message: error.message, category: error.category, retryable: error.retryable, details: error.details } };
}

function sha256(bytes: Buffer): string { return createHash('sha256').update(bytes).digest('hex'); }
const utf8Decoder = new TextDecoder('utf-8', { fatal: true });
function decodeUtf8(bytes: Buffer, relative: string): string {
  try { return utf8Decoder.decode(bytes); }
  catch { throw new FormatterError('FORMATTER_OUTPUT_ENCODING', `File is not valid UTF-8: ${relative}`, 'validation', false, { path: relative }); }
}

async function rejectSymlinkTraversal(root: string, full: string): Promise<void> {
  let current = root;
  for (const part of relativeInside(root, full).replaceAll('\\', '/').split('/').filter(Boolean)) {
    current = path.join(current, part);
    try {
      const info = await lstat(current);
      if (info.isSymbolicLink()) {
        throw new FormatterError('SYMLINK_WRITE_BLOCKED', `Formatter path may not traverse a symbolic link: ${relativeInside(root, current).replaceAll('\\', '/')}`, 'security', false);
      }
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') break;
      throw error;
    }
  }
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

function simpleUnifiedDiff(file: string, before: string, after: string): string {
  if (before === after) return '';
  const oldLines = before.split(/\r?\n/);
  const newLines = after.split(/\r?\n/);
  return [
    `--- a/${file}`,
    `+++ b/${file}`,
    `@@ -1,${oldLines.length} +1,${newLines.length} @@`,
    ...oldLines.map(line => `-${line}`),
    ...newLines.map(line => `+${line}`),
    ''
  ].join('\n');
}

function defaultFormatter(extension: string): string | undefined {
  if (extension === 'rs') return 'rustfmt';
  if (['js', 'jsx', 'mjs', 'cjs', 'ts', 'tsx', 'yaml', 'yml', 'md', 'markdown', 'css', 'scss', 'less', 'html', 'vue', 'svelte'].includes(extension)) return 'prettier';
  if (extension === 'json') return 'builtin-json';
  if (extension === 'jsonc') return 'prettier';
  if (['py', 'pyi'].includes(extension)) return 'ruff';
  if (extension === 'go') return 'gofmt';
  if (['c', 'h', 'cc', 'cpp', 'cxx', 'hpp', 'java', 'proto'].includes(extension)) return 'clang-format';
  if (extension === 'cs') return 'csharpier';
  if (['kt', 'kts'].includes(extension)) return 'ktfmt';
  if (['sh', 'bash', 'zsh'].includes(extension)) return 'shfmt';
  if (['tf', 'tfvars', 'hcl'].includes(extension)) return 'terraform-fmt';
  if (extension === 'toml') return 'taplo';
  return undefined;
}

function formatterConfigError(message: string, details: JsonObject): FormatterError {
  return new FormatterError('FORMATTER_CONFIG_INVALID', message, 'validation', false, details);
}

function validateRelativeConfigPath(adapterId: string, field: string, value: string): void {
  if (!value || path.isAbsolute(value) || value.split(/[\\/]/).includes('..')) {
    throw formatterConfigError('Custom formatter paths must stay inside the workspace', { adapter_id: adapterId, field, value });
  }
}

function validAdapterId(value: string): boolean {
  return value.length > 0 && value.length <= 128 && /^[A-Za-z0-9._-]+$/.test(value);
}

async function loadCustomAdapters(root: string): Promise<Map<string, CustomAdapter>> {
  const relativeConfig = '.coding-tools/formatters.json';
  const configPath = path.join(root, '.coding-tools', 'formatters.json');
  if (!(await exists(configPath))) return new Map();
  let parsed: unknown;
  try { parsed = JSON.parse(await readFile(configPath, 'utf8')); }
  catch (error) {
    throw formatterConfigError('Custom formatter configuration is not valid JSON', { path: relativeConfig, error: error instanceof Error ? error.message : String(error) });
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw formatterConfigError('Custom formatter configuration requires a formatters object', { path: relativeConfig });
  }
  const formatters = (parsed as Record<string, unknown>).formatters;
  if (!formatters || typeof formatters !== 'object' || Array.isArray(formatters)) {
    throw formatterConfigError('Custom formatter configuration requires a formatters object', { path: relativeConfig });
  }
  const entries = Object.entries(formatters as Record<string, unknown>);
  if (entries.length > 50) throw formatterConfigError('Custom formatter configuration exceeds 50 adapters', { adapter_count: entries.length });
  const adapters = new Map<string, CustomAdapter>();
  for (const [id, raw] of entries) {
    if (!validAdapterId(id)) throw formatterConfigError('Custom formatter ID contains unsupported characters', { adapter_id: id });
    if (BUILTIN_IDS.has(id)) throw formatterConfigError('Custom formatter ID conflicts with a built-in adapter', { adapter_id: id });
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw formatterConfigError('Custom formatter entry must be an object', { adapter_id: id });
    const value = raw as Record<string, unknown>;
    if (typeof value.program !== 'string' || !value.program) throw formatterConfigError('Custom formatter requires a program', { adapter_id: id });
    validateRelativeConfigPath(id, 'program', value.program);
    if (!Array.isArray(value.extensions) || value.extensions.length < 1 || value.extensions.length > 50) {
      throw formatterConfigError('Custom formatter extensions must contain 1 to 50 entries', { adapter_id: id });
    }
    const extensions: string[] = [];
    for (const item of value.extensions) {
      const extension = typeof item === 'string' ? item.replace(/^\./, '').toLowerCase() : '';
      if (!extension || !/^[A-Za-z0-9_+-]+$/.test(extension)) throw formatterConfigError('Custom formatter extension is invalid', { adapter_id: id });
      if (!extensions.includes(extension)) extensions.push(extension);
    }
    if (!Array.isArray(value.args) || value.args.length > 100 || value.args.some(item => typeof item !== 'string')) {
      throw formatterConfigError('Custom formatter requires an args array of strings with at most 100 entries', { adapter_id: id });
    }
    const args = value.args as string[];
    let hasFiles = false;
    for (const argument of args) {
      if (argument === '{files}' || argument === '{file}') hasFiles = true;
      else if (!['{config}', '{workspace}'].includes(argument) && /[{}]/.test(argument)) {
        throw formatterConfigError('Custom formatter contains an unsupported placeholder', { adapter_id: id, argument });
      }
    }
    if (!hasFiles) throw formatterConfigError('Custom formatter args require {files} or {file}', { adapter_id: id });
    const configPathValue = value.config === undefined ? undefined : String(value.config);
    if (configPathValue !== undefined) validateRelativeConfigPath(id, 'config', configPathValue);
    adapters.set(id, { id, program: value.program, extensions, args, ...(configPathValue === undefined ? {} : { configPath: configPathValue }) });
  }
  return adapters;
}

async function nearestFormatterConfig(root: string, file: string, names: string[]): Promise<string | null> {
  if (!names.length) return null;
  let directory = path.dirname(file);
  while (directory === root || directory.startsWith(`${root}${path.sep}`)) {
    for (const name of names) {
      const candidate = path.join(directory, name);
      if (await exists(candidate)) return relativeInside(root, candidate).replaceAll('\\', '/');
    }
    if (directory === root) break;
    directory = path.dirname(directory);
  }
  return null;
}

async function customSelection(root: string, adapter: CustomAdapter): Promise<SelectedAdapter> {
  const configPath = adapter.configPath ?? null;
  if (configPath) await rejectSymlinkTraversal(root, resolveInside(root, configPath));
  await rejectSymlinkTraversal(root, resolveInside(root, adapter.program));
  return {
    id: adapter.id,
    extensions: adapter.extensions,
    configPath,
    selectionSource: 'workspace_config',
    mutationRisk: 'targeted',
    custom: true,
    commandTemplate: { program: adapter.program, args: adapter.args }
  };
}

async function selectAdapter(root: string, file: string, extension: string, explicit: string, strict: boolean, custom: Map<string, CustomAdapter>): Promise<SelectedAdapter | undefined> {
  if (explicit !== 'auto') {
    const customAdapter = custom.get(explicit);
    if (customAdapter) {
      if (!customAdapter.extensions.includes(extension)) {
        if (strict) throw new FormatterError('INVALID_ARGUMENT', `Formatter ${explicit} does not support ${relativeInside(root, file)}`, 'validation', false);
        return undefined;
      }
      return await customSelection(root, customAdapter);
    }
    const spec = FORMATTER_SPECS.find(item => item.id === explicit);
    if (!spec) throw new FormatterError('INVALID_ARGUMENT', `Unknown formatter adapter: ${explicit}`, 'validation', false);
    if (!spec.extensions.includes(extension)) {
      if (strict) throw new FormatterError('INVALID_ARGUMENT', `Formatter ${explicit} does not support ${relativeInside(root, file)}`, 'validation', false);
      return undefined;
    }
    const configPath = await nearestFormatterConfig(root, file, spec.configNames);
    return { id: spec.id, extensions: spec.extensions, configPath, selectionSource: 'explicit', mutationRisk: spec.mutationRisk, custom: false };
  }

  const customMatches = [...custom.values()].filter(adapter => adapter.extensions.includes(extension));
  if (customMatches.length > 1) {
    throw new FormatterError('FORMATTER_AMBIGUOUS', `Multiple custom formatters support .${extension}`, 'validation', false, {
      extension,
      adapter_ids: customMatches.map(adapter => adapter.id).sort(),
      suggestion: 'Specify formatter explicitly'
    });
  }
  if (customMatches[0]) return await customSelection(root, customMatches[0]);

  const configured: Array<{ spec: FormatterSpec; config: string; depth: number }> = [];
  for (const spec of FORMATTER_SPECS) {
    if (!spec.extensions.includes(extension) || !spec.configNames.length) continue;
    const config = await nearestFormatterConfig(root, file, spec.configNames);
    if (config) configured.push({ spec, config, depth: config.split('/').length });
  }
  configured.sort((left, right) => right.depth - left.depth);
  if (configured[0]) {
    const { spec, config } = configured[0];
    const base = path.basename(config);
    return { id: spec.id, extensions: spec.extensions, configPath: config, selectionSource: ['package.json', 'pyproject.toml', 'Cargo.toml', 'go.mod'].includes(base) ? 'manifest' : 'nearest_config', mutationRisk: spec.mutationRisk, custom: false };
  }
  const id = defaultFormatter(extension);
  const spec = FORMATTER_SPECS.find(item => item.id === id);
  return spec ? { id: spec.id, extensions: spec.extensions, configPath: null, selectionSource: 'language_default', mutationRisk: spec.mutationRisk, custom: false } : undefined;
}

async function collectFormatCandidates(root: string, args: JsonObject): Promise<{ scope: string; paths: string[]; requested: number; truncated: boolean }> {
  const scope = String(args.scope ?? 'files');
  const maxFiles = Math.max(1, Math.min(10_000, Number(args.max_files ?? 500)));
  const explicit = Array.isArray(args.paths) ? args.paths.map(String) : [];
  if (scope === 'files' && !explicit.length) throw new FormatterError('INVALID_ARGUMENT', 'format_files with scope=files requires at least one path', 'validation', false);
  const candidates = new Set<string>();
  const addFile = async (value: string) => {
    const resolved = await resolveExistingPath(root, value);
    const info = await stat(resolved.full);
    if (info.isDirectory()) {
      const entries = await walk(root, resolved.full, { maxDepth: 20, maxResults: 50_000, includeHidden: false });
      for (const entry of entries) if (entry.type === 'file') candidates.add(entry.path);
    } else if (info.isFile()) candidates.add(resolved.display);
  };
  if (scope === 'files') {
    for (const value of explicit) await addFile(value);
  } else if (scope === 'project') {
    const entries = await walk(root, root, { maxDepth: 20, maxResults: 50_000, includeHidden: false });
    for (const entry of entries) if (entry.type === 'file') candidates.add(entry.path);
  } else if (scope === 'changed' || scope === 'staged') {
    const commands = scope === 'staged'
      ? [['diff', '--cached', '--name-only', '-z']]
      : [['diff', '--name-only', '-z'], ['diff', '--cached', '--name-only', '-z'], ['ls-files', '--others', '--exclude-standard', '-z']];
    for (const command of commands) {
      const result = await runGitBuffered(root, command, undefined, 30_000);
      if (result.code !== 0) throw new FormatterError('FORMAT_SCOPE_GIT_FAILED', result.stderr || `git ${command.join(' ')} failed`, 'runtime', true);
      for (const value of result.stdout.split('\0').filter(Boolean)) {
        try {
          const resolved = await resolveExistingPath(root, value);
          if ((await stat(resolved.full)).isFile()) candidates.add(resolved.display);
        } catch (error) {
          if (error instanceof WorkspacePathError && error.code === 'NOT_FOUND') continue;
          throw error;
        }
      }
    }
  } else throw new FormatterError('INVALID_ARGUMENT', `Unsupported format scope: ${scope}`, 'validation', false);

  const include = (Array.isArray(args.include_patterns) ? args.include_patterns : []).map(String).map(globRegex);
  const exclude = (Array.isArray(args.exclude_patterns) ? args.exclude_patterns : []).map(String).map(globRegex);
  let paths = [...candidates].sort();
  if (include.length) paths = paths.filter(value => include.some(pattern => pattern.test(value)));
  if (exclude.length) paths = paths.filter(value => !exclude.some(pattern => pattern.test(value)));
  const requested = paths.length;
  const truncated = paths.length > maxFiles;
  paths = paths.slice(0, maxFiles);
  return { scope, paths, requested, truncated };
}

interface CargoRustEditionMetadata {
  package_present: boolean;
  package_edition?: string;
  package_inherits_workspace: boolean;
  workspace_edition?: string;
}

export function cargoRustEditionMetadata(content: string): CargoRustEditionMetadata {
  let section = '';
  let packagePresent = false;
  let packageEdition: string | undefined;
  let packageInheritsWorkspace = false;
  let workspaceEdition: string | undefined;
  for (const rawLine of content.split(/\r?\n/)) {
    const sectionMatch = rawLine.match(/^\s*\[\s*([^\]]+)\s*\]\s*(?:#.*)?$/);
    if (sectionMatch) {
      section = sectionMatch[1].trim();
      if (section === 'package') packagePresent = true;
      continue;
    }
    if (section === 'package') {
      const direct = rawLine.match(/^\s*edition\s*=\s*["']([^"']+)["']/);
      if (direct) packageEdition = direct[1];
      if (/^\s*edition\.workspace\s*=\s*true\b/i.test(rawLine)) packageInheritsWorkspace = true;
    } else if (section === 'workspace.package') {
      const workspace = rawLine.match(/^\s*edition\s*=\s*["']([^"']+)["']/);
      if (workspace) workspaceEdition = workspace[1];
    }
  }
  return {
    package_present: packagePresent,
    ...(packageEdition ? { package_edition: packageEdition } : {}),
    package_inherits_workspace: packageInheritsWorkspace,
    ...(workspaceEdition ? { workspace_edition: workspaceEdition } : {})
  };
}

async function rustEditionForFile(root: string, file: string): Promise<string> {
  let directory = path.dirname(file);
  let inheritsWorkspace = false;
  while (directory === root || directory.startsWith(`${root}${path.sep}`)) {
    const manifest = path.join(directory, 'Cargo.toml');
    if (await exists(manifest)) {
      const metadata = cargoRustEditionMetadata(await readFile(manifest, 'utf8'));
      if (inheritsWorkspace && metadata.workspace_edition) return metadata.workspace_edition;
      if (!inheritsWorkspace && metadata.package_present) {
        if (metadata.package_edition) return metadata.package_edition;
        if (metadata.package_inherits_workspace) {
          inheritsWorkspace = true;
          if (metadata.workspace_edition) return metadata.workspace_edition;
        } else {
          return '2015';
        }
      }
    }
    if (directory === root) break;
    directory = path.dirname(directory);
  }
  return '2021';
}

async function planFormatFiles(root: string, args: JsonObject): Promise<FormatPlan> {
  const collected = await collectFormatCandidates(root, args);
  const strict = args.strict === true;
  const explicit = String(args.formatter ?? 'auto');
  const custom = await loadCustomAdapters(root);
  const files: PlannedFormatFile[] = [];
  const skipped: Array<{ path: string; reason: string }> = [];
  const groups = new Map<string, FormatGroup>();
  for (const relative of collected.paths) {
    const resolved = await resolveExistingPath(root, relative);
    const full = resolved.full;
    if (GENERATED_FORMAT_FILES.has(path.basename(relative))) { skipped.push({ path: relative, reason: 'generated_manifest' }); continue; }
    const head = (await readFile(full)).subarray(0, 8192);
    if (head.includes(0)) { skipped.push({ path: relative, reason: 'binary_file' }); continue; }
    const extension = path.extname(relative).slice(1).toLowerCase();
    const selected = await selectAdapter(root, full, extension, explicit, strict, custom);
    if (!selected) { skipped.push({ path: relative, reason: explicit === 'auto' ? 'unsupported_extension' : 'unsupported_formatter' }); continue; }
    const rustEdition = selected.id === 'rustfmt' ? await rustEditionForFile(root, full) : undefined;
    const planned: PlannedFormatFile = {
      path: relative,
      adapter_id: selected.id,
      config_path: selected.configPath,
      selection_source: selected.selectionSource,
      ...(rustEdition ? { rust_edition: rustEdition } : {})
    };
    files.push(planned);
    const groupKey = `${selected.id}\0${selected.configPath ?? ''}\0${rustEdition ?? ''}`;
    const group = groups.get(groupKey) ?? {
      adapter_id: selected.id,
      config_path: selected.configPath,
      files: [],
      mutation_risk: selected.mutationRisk,
      custom: selected.custom,
      ...(rustEdition ? { rust_edition: rustEdition } : {}),
      ...(selected.commandTemplate ? { command_template: selected.commandTemplate } : {})
    };
    group.files.push(relative);
    groups.set(groupKey, group);
  }
  return {
    scope: collected.scope,
    filesRequested: collected.requested,
    files,
    groups: [...groups.values()].sort((left, right) => left.adapter_id.localeCompare(right.adapter_id)),
    skipped: skipped.sort((left, right) => left.path.localeCompare(right.path)),
    truncated: collected.truncated
  };
}

async function copyWorkspaceFile(root: string, mirrorRoot: string, relative: string): Promise<void> {
  const resolved = await resolveExistingPath(root, relative);
  const source = resolved.full;
  const info = await stat(source);
  if (!info.isFile()) return;
  const destination = resolveInside(mirrorRoot, relative);
  await mkdir(path.dirname(destination), { recursive: true });
  await copyFile(source, destination);
}

async function collectNearestSupportFiles(root: string, relative: string, output: Set<string>): Promise<void> {
  let directory = path.dirname(resolveInside(root, relative));
  while (directory === root || directory.startsWith(`${root}${path.sep}`)) {
    for (const name of SUPPORT_NAMES) {
      const candidate = path.join(directory, name);
      if (await exists(candidate)) output.add(relativeInside(root, candidate).replaceAll('\\', '/'));
    }
    if (directory === root) break;
    directory = path.dirname(directory);
  }
}

async function prepareMirror(root: string, mirrorRoot: string, plan: FormatPlan): Promise<void> {
  const support = new Set<string>();
  for (const file of plan.files) {
    await copyWorkspaceFile(root, mirrorRoot, file.path);
    if (file.config_path) support.add(file.config_path);
    await collectNearestSupportFiles(root, file.path, support);
  }
  for (const relative of support) if (await exists(resolveInside(root, relative))) await copyWorkspaceFile(root, mirrorRoot, relative);
}

type MirrorLocation = 'workspace' | 'system_temp';

interface MirrorRoot {
  parent: string;
  mirrorRoot: string;
  createdParent: boolean;
  location: MirrorLocation;
}

function mirrorWriteUnavailable(error: unknown): boolean {
  const code = (error as NodeJS.ErrnoException | undefined)?.code;
  return code === 'EACCES' || code === 'EPERM' || code === 'EROFS';
}

async function createTemporaryMirrorRoot(): Promise<MirrorRoot> {
  const parent = await mkdtemp(path.join(tmpdir(), 'ctmcp-format-'));
  const mirrorRoot = path.join(parent, 'mirror');
  await mkdir(mirrorRoot);
  return { parent, mirrorRoot, createdParent: true, location: 'system_temp' };
}

async function createMirrorRoot(root: string): Promise<MirrorRoot> {
  const parent = path.join(root, '.coding-tools-format');
  let createdParent = false;
  let info;
  try { info = await lstat(parent); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    try {
      await mkdir(parent);
      createdParent = true;
      info = await lstat(parent);
    } catch (mkdirError) {
      if (mirrorWriteUnavailable(mkdirError)) return createTemporaryMirrorRoot();
      if ((mkdirError as NodeJS.ErrnoException).code !== 'EEXIST') throw mkdirError;
      info = await lstat(parent);
    }
  }
  if (info.isSymbolicLink() || !info.isDirectory()) {
    throw new FormatterError('FORMATTER_MIRROR_UNSAFE', 'Formatter mirror root must be a real workspace directory', 'security', false, {
      path: '.coding-tools-format'
    });
  }
  const mirrorRoot = path.join(parent, randomUUID());
  try {
    await mkdir(mirrorRoot);
  } catch (error) {
    if (!mirrorWriteUnavailable(error)) throw error;
    if (createdParent) await rmdir(parent).catch(() => undefined);
    return createTemporaryMirrorRoot();
  }
  return { parent, mirrorRoot, createdParent, location: 'workspace' };
}

function ignoredMirrorArtifact(relative: string): boolean {
  return relative.split('/').some(part => MIRROR_IGNORED_PARTS.has(part));
}

async function snapshotTree(root: string): Promise<Map<string, string>> {
  const snapshot = new Map<string, string>();
  async function visit(directory: string): Promise<void> {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      const full = path.join(directory, entry.name);
      const relative = relativeInside(root, full).replaceAll('\\', '/');
      if (ignoredMirrorArtifact(relative)) continue;
      if (entry.isDirectory()) await visit(full);
      else if (entry.isFile()) snapshot.set(relative, sha256(await readFile(full)));
    }
  }
  await visit(root);
  return snapshot;
}

function changedPaths(before: Map<string, string>, after: Map<string, string>): string[] {
  return [...new Set([...before.keys(), ...after.keys()])].filter(file => before.get(file) !== after.get(file)).sort();
}

export function rustfmtCommandArgs(files: string[], edition = '2021'): string[] {
  return ['--edition', edition, ...files];
}

function builtInCommand(adapter: string, files: string[], rustEdition?: string): { candidates: string[]; args: string[] } {
  switch (adapter) {
    case 'rustfmt': return { candidates: ['rustfmt'], args: rustfmtCommandArgs(files, rustEdition) };
    case 'prettier': return { candidates: ['prettier'], args: ['--write', ...files] };
    case 'biome': return { candidates: ['biome'], args: ['format', '--write', ...files] };
    case 'dprint': return { candidates: ['dprint'], args: ['fmt', ...files] };
    case 'ruff': return { candidates: ['ruff'], args: ['format', ...files] };
    case 'black': return { candidates: ['black'], args: files };
    case 'gofmt': return { candidates: ['gofmt'], args: ['-w', ...files] };
    case 'clang-format': return { candidates: ['clang-format'], args: ['-i', ...files] };
    case 'csharpier': return { candidates: ['csharpier', 'dotnet-csharpier'], args: ['format', ...files] };
    case 'ktfmt': return { candidates: ['ktfmt'], args: files };
    case 'ktlint': return { candidates: ['ktlint'], args: ['-F', ...files] };
    case 'shfmt': return { candidates: ['shfmt'], args: ['-w', ...files] };
    case 'terraform-fmt': return { candidates: ['terraform'], args: ['fmt', ...files] };
    case 'taplo': return { candidates: ['taplo'], args: ['format', ...files] };
    default: throw new FormatterError('INVALID_ARGUMENT', `No command adapter is registered for ${adapter}`, 'validation', false);
  }
}

function renderCustomCommand(template: CommandTemplate, files: string[], configPath: string | null): { candidates: string[]; args: string[]; workspaceRelative: true } {
  const args: string[] = [];
  for (const argument of template.args) {
    if (argument === '{files}' || argument === '{file}') args.push(...files);
    else if (argument === '{workspace}') args.push('.');
    else if (argument === '{config}') {
      if (!configPath) throw formatterConfigError('Custom formatter uses {config} without a configured path', { program: template.program });
      args.push(configPath);
    } else args.push(argument);
  }
  return { candidates: [template.program], args, workspaceRelative: true };
}

export function formatterExecutableCandidates(root: string, names: string[]): string[] {
  const wslWorkspace = Boolean(parseWslUncPath(root));
  const windowsWorkspace = process.platform === 'win32' && !wslWorkspace;
  const directories = windowsWorkspace
    ? ['node_modules/.bin', '.venv/Scripts', 'venv/Scripts', 'bin', 'tools']
    : ['node_modules/.bin', '.venv/bin', 'venv/bin', 'bin', 'tools'];
  const extensions = windowsWorkspace ? ['', '.cmd', '.exe', '.bat'] : [''];
  const output: string[] = [];
  for (const name of names) {
    for (const directory of directories) for (const extension of extensions) {
      const relative = `${directory}/${name}${extension}`;
      output.push(wslWorkspace ? resolveInside(root, relative) : path.join(root, directory, `${name}${extension}`));
    }
    output.push(name);
  }
  return [...new Set(output)];
}

async function resolveExecutable(root: string, candidates: string[], workspaceRelative: boolean): Promise<string | undefined> {
  if (workspaceRelative) {
    const resolved = resolveInside(root, candidates[0]);
    try { if ((await stat(resolved)).isFile()) return resolved; } catch { return undefined; }
    return undefined;
  }
  for (const candidate of formatterExecutableCandidates(root, candidates)) {
    if (!path.isAbsolute(candidate) && candidate === path.basename(candidate)) return candidate;
    try { await access(candidate); return candidate; } catch { /* next */ }
  }
  return undefined;
}

function formatterEnvironment(): NodeJS.ProcessEnv {
  const allowed = ['PATH', 'PATHEXT', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'TEMP', 'TMP', 'HOME', 'USERPROFILE', 'LANG', 'LC_ALL', 'NO_COLOR', 'CI'];
  const environment: NodeJS.ProcessEnv = {};
  for (const name of allowed) if (process.env[name] !== undefined) environment[name] = process.env[name];
  environment.NO_COLOR = '1';
  return environment;
}

export function formatterLaunchSpec(
  workspaceRoot: string,
  executable: string,
  args: string[]
): { program: string; args: string[] } {
  const wslWorkspace = Boolean(parseWslUncPath(workspaceRoot));
  if (/\.(?:mjs|cjs|js)$/i.test(executable)) {
    return { program: wslWorkspace ? 'node' : process.execPath, args: [executable, ...args] };
  }
  if (!wslWorkspace && process.platform === 'win32' && /\.(?:cmd|bat)$/i.test(executable)) {
    return { program: process.env.COMSPEC || 'cmd.exe', args: ['/d', '/s', '/c', executable, ...args] };
  }
  return { program: executable, args };
}

async function runFormatter(root: string, mirrorRoot: string, group: FormatGroup, timeoutMs: number): Promise<{ unavailable: boolean; stdout: string; stderr: string }> {
  const command = group.command_template
    ? renderCustomCommand(group.command_template, group.files, group.config_path)
    : { ...builtInCommand(group.adapter_id, group.files, group.rust_edition), workspaceRelative: false as const };
  const executable = await resolveExecutable(root, command.candidates, command.workspaceRelative);
  if (!executable) return { unavailable: true, stdout: '', stderr: '' };
  const launch = formatterLaunchSpec(root, executable, command.args);
  const wslWorkspace = Boolean(parseWslUncPath(root));
  let result;
  try {
    result = await runBuffered(
      launch.program, launch.args, mirrorRoot, undefined, timeoutMs, formatterEnvironment(),
      { routeWsl: true, cleanEnvironment: wslWorkspace }
    );
  }
  catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (/ENOENT|not found|cannot find/i.test(message)) return { unavailable: true, stdout: '', stderr: message };
    throw new FormatterError('FORMATTER_START_FAILED', `Could not start formatter ${group.adapter_id}`, 'runtime', true, { adapter_id: group.adapter_id, error: message });
  }
  if (result.code !== 0) {
    throw new FormatterError('FORMATTER_FAILED', `Formatter ${group.adapter_id} failed`, 'runtime', true, {
      adapter_id: group.adapter_id,
      exit_code: result.code,
      stdout: truncatePrefixUtf8(result.stdout, 16_384).content,
      stderr: truncatePrefixUtf8(result.stderr, 16_384).content
    });
  }
  return { unavailable: false, stdout: result.stdout, stderr: result.stderr };
}

async function readOriginals(root: string, plan: FormatPlan, args: JsonObject): Promise<Map<string, OriginalFile>> {
  const expected = (args.expected_sha256 as Record<string, unknown> | undefined) ?? {};
  const originals = new Map<string, OriginalFile>();
  for (const file of plan.files) {
    const resolved = await resolveExistingPath(root, file.path);
    const bytes = await readFile(resolved.full);
    const actual = sha256(bytes);
    const expectedHash = expected[file.path];
    if (expectedHash !== undefined && String(expectedHash).toLowerCase() !== actual) {
      throw new FormatterError('FILE_VERSION_MISMATCH', `File changed since it was read: ${file.path}`, 'conflict', true, {
        path: file.path,
        expected_sha256: String(expectedHash),
        actual_sha256: actual,
        suggestion: 'Read the file again and rebuild the formatting request'
      });
    }
    originals.set(file.path, { bytes, sha256: actual });
  }
  return originals;
}

async function writeAtomic(file: string, bytes: Buffer): Promise<void> {
  const temporary = `${file}.${process.pid}.${randomUUID()}.tmp`;
  await writeFile(temporary, bytes, { flag: 'wx' });
  try { await rename(temporary, file); }
  catch (error) { await rm(temporary, { force: true }).catch(() => undefined); throw error; }
}

async function applyGuarded(root: string, originals: Map<string, OriginalFile>, formatted: Map<string, Buffer>): Promise<void> {
  for (const relative of formatted.keys()) {
    const resolved = await resolveExistingPath(root, relative);
    const current = await readFile(resolved.full);
    const actual = sha256(current);
    const expected = originals.get(relative)!.sha256;
    if (actual !== expected) {
      throw new FormatterError('FILE_VERSION_MISMATCH', `File changed since it was read: ${relative}`, 'conflict', true, {
        path: relative,
        expected_sha256: expected,
        actual_sha256: actual,
        suggestion: 'Read the file again and rebuild the formatting request'
      });
    }
  }
  const written: string[] = [];
  try {
    for (const [relative, bytes] of formatted) {
      const resolved = await resolveExistingPath(root, relative);
      await writeAtomic(resolved.full, bytes);
      written.push(relative);
    }
  } catch (error) {
    const rollbackFailures: string[] = [];
    for (const relative of [...written].reverse()) {
      try {
        const resolved = await resolveExistingPath(root, relative);
        await writeAtomic(resolved.full, originals.get(relative)!.bytes);
      } catch {
        rollbackFailures.push(relative);
      }
    }
    throw new FormatterError('FORMAT_APPLY_FAILED', 'Could not apply formatted output transactionally', 'runtime', true, {
      error: error instanceof Error ? error.message : String(error),
      rolled_back: written.filter(file => !rollbackFailures.includes(file)),
      rollback_failures: rollbackFailures
    });
  }
}

export async function formatFilesTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  try {
    const selectedRoot = await resolveExistingPath(rootAndCwd(ctx, key).root, '.');
    const root = selectedRoot.root;
    const mode = String(args.mode ?? 'plan');
    if (!['plan', 'check', 'apply'].includes(mode)) throw new FormatterError('INVALID_ARGUMENT', `Unsupported format mode: ${mode}`, 'validation', false);
    const plan = await planFormatFiles(root, args);
    if (ctx.config.securityPolicy.requireWriteConfirmation && mode === 'apply' && plan.scope === 'project' && args.confirm !== true) {
      throw new FormatterError('CONFIRMATION_REQUIRED', 'format_files scope=project mode=apply requires confirm=true', 'permission', false);
    }
    const customGroups = plan.groups.filter(group => group.custom);
    if (ctx.config.securityPolicy.requireWriteConfirmation && mode !== 'plan' && customGroups.length && args.confirm !== true) {
      throw new FormatterError('CUSTOM_FORMATTER_REQUIRES_CONFIRMATION', 'Custom formatter execution requires confirm=true', 'permission', false, {
        custom_adapters: customGroups.map(group => group.adapter_id),
        suggestion: 'Review mode=plan output, then retry with confirm=true'
      });
    }
    const base = {
      mode,
      scope: plan.scope,
      files_requested: plan.filesRequested,
      files_supported: plan.files.length,
      groups: plan.groups.map(({ command_template: _command, ...group }) => group),
      formatter_group_count: plan.groups.length,
      custom_formatter_group_count: customGroups.length,
      selection: plan.files,
      files_skipped: plan.skipped,
      files_skipped_count: plan.skipped.length
    };
    if (mode === 'plan') {
      return ok({
        ...base,
        status: 'planned',
        files_changed: [], files_changed_count: 0,
        files_unchanged: [], files_unchanged_count: 0,
        unavailable_adapters: [], unexpected_changes: [],
        diff: '', diff_bytes: 0, diff_truncated: false, applied: false,
        warnings: plan.truncated ? ['max_files limit reached'] : []
      });
    }

    const originals = await readOriginals(root, plan, args);
    const { parent, mirrorRoot, createdParent, location: mirrorLocation } = await createMirrorRoot(root);
    const unavailable = new Set<string>();
    const unavailableFiles = new Set<string>();
    const skipped = [...plan.skipped];
    const formatted = new Map<string, Buffer>();
    let diff = '';
    try {
      await prepareMirror(root, mirrorRoot, plan);
      let mirrorSnapshot = await snapshotTree(mirrorRoot);
      const timeoutMs = Math.max(1, Math.min(600_000, Number(args.timeout_ms ?? 120_000)));
      for (const group of plan.groups) {
        if (group.adapter_id === 'builtin-json') {
          for (const relative of group.files) {
            const file = resolveInside(mirrorRoot, relative);
            let parsed;
            try { parsed = JSON.parse(await readFile(file, 'utf8')); }
            catch (error) { throw new FormatterError('FORMATTER_FAILED', `Invalid JSON in ${relative}`, 'validation', false, { adapter_id: 'builtin-json', error: error instanceof Error ? error.message : String(error) }); }
            await writeFile(file, `${JSON.stringify(parsed, null, 2)}\n`);
          }
        } else {
          const outcome = await runFormatter(root, mirrorRoot, group, timeoutMs);
          if (outcome.unavailable) {
            if (args.strict === true || group.custom) throw new FormatterError('FORMATTER_UNAVAILABLE', `Formatter ${group.adapter_id} is not installed`, 'runtime', true, { adapter_id: group.adapter_id });
            unavailable.add(group.adapter_id);
            for (const relative of group.files) {
              unavailableFiles.add(relative);
              skipped.push({ path: relative, reason: 'formatter_unavailable' });
            }
            continue;
          }
        }
        const after = await snapshotTree(mirrorRoot);
        const allowed = new Set(group.files);
        const unexpected = changedPaths(mirrorSnapshot, after).filter(relative => !allowed.has(relative));
        if (unexpected.length) {
          throw new FormatterError('FORMAT_UNEXPECTED_CHANGES', `Formatter ${group.adapter_id} changed files outside the requested group`, 'conflict', false, {
            adapter_id: group.adapter_id,
            unexpected_changes: unexpected,
            allowed_files: group.files
          });
        }
        mirrorSnapshot = after;
      }

      const changed: string[] = [];
      const unchanged: string[] = [];
      for (const file of plan.files) {
        if (unavailableFiles.has(file.path)) continue;
        let bytes: Buffer;
        try { bytes = await readFile(resolveInside(mirrorRoot, file.path)); }
        catch { throw new FormatterError('FORMATTER_OUTPUT_MISSING', `Formatter did not preserve output for ${file.path}`, 'runtime', false, { path: file.path, adapter_id: file.adapter_id }); }
        const original = originals.get(file.path)!;
        if (bytes.equals(original.bytes)) { unchanged.push(file.path); continue; }
        const before = decodeUtf8(original.bytes, file.path);
        const after = decodeUtf8(bytes, file.path);
        if (before.includes('\0') || after.includes('\0')) throw new FormatterError('FORMATTER_OUTPUT_ENCODING', `Formatter produced non-text output: ${file.path}`, 'runtime', false, { path: file.path });
        diff += simpleUnifiedDiff(file.path, before, after);
        formatted.set(file.path, bytes);
        changed.push(file.path);
      }
      changed.sort();
      unchanged.sort();
      if (mode === 'apply' && formatted.size) await applyGuarded(root, originals, formatted);
      const maxDiffBytes = Math.max(1_024, Math.min(1_048_576, Number(args.max_diff_bytes ?? 262_144)));
      const bounded = truncatePrefixUtf8(diff, maxDiffBytes);
      const warnings = [
        ...(plan.truncated ? ['max_files limit reached'] : []),
        ...(unavailable.size ? [`Unavailable formatters were skipped: ${[...unavailable].sort().join(', ')}`] : []),
        ...(mirrorLocation === 'system_temp' ? ['Workspace formatter mirror was not writable; used an isolated system temporary directory.'] : [])
      ];
      return ok({
        ...base,
        status: mode === 'check' ? 'checked' : changed.length ? 'applied' : 'unchanged',
        files_changed: changed,
        files_changed_count: changed.length,
        files_unchanged: unchanged,
        files_unchanged_count: unchanged.length,
        files_skipped: skipped.sort((left, right) => left.path.localeCompare(right.path)),
        files_skipped_count: skipped.length,
        unavailable_adapters: [...unavailable].sort(),
        unexpected_changes: [],
        mirror_location: mirrorLocation,
        diff: bounded.content,
        diff_bytes: Buffer.byteLength(bounded.content),
        diff_truncated: bounded.truncated,
        applied: mode === 'apply' && changed.length > 0,
        warnings
      });
    } finally {
      await rm(mirrorRoot, { recursive: true, force: true }).catch(() => undefined);
      if (createdParent) await rmdir(parent).catch(() => undefined);
    }
  } catch (error) {
    if (error instanceof FormatterError) return fail(error);
    throw error;
  }
}
