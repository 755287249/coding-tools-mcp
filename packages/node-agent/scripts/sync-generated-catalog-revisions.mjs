import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = fileURLToPath(new URL('..', import.meta.url));
const defaultGeneratedFile = path.join(packageRoot, 'src', 'rustCatalog.generated.ts');
const profileNames = ['advanced', 'read-only', 'compat-readonly-all', 'guarded-core', 'trusted-core'];

function argumentValue(name) {
  const index = process.argv.indexOf(name);
  if (index < 0) return undefined;
  const value = process.argv[index + 1];
  if (!value || value.startsWith('--')) throw new Error(`${name} requires a value`);
  return value;
}

function exportRange(source, exportName, nextExportName) {
  const marker = `export const ${exportName}`;
  const start = source.indexOf(marker);
  if (start < 0) throw new Error(`Generated catalog is missing ${exportName}`);
  const assignment = source.indexOf(' = ', start);
  if (assignment < 0) throw new Error(`Generated catalog has an invalid ${exportName} declaration`);
  const nextMarker = `\nexport const ${nextExportName}`;
  const end = source.indexOf(nextMarker, assignment);
  if (end < 0) throw new Error(`Generated catalog is missing ${nextExportName} after ${exportName}`);
  return { start, assignment, end };
}

function parseJsonExport(source, exportName, nextExportName) {
  const range = exportRange(source, exportName, nextExportName);
  const expression = source.slice(range.assignment + 3, range.end).trim();
  if (!expression.endsWith(';')) throw new Error(`Generated catalog ${exportName} does not end with a semicolon`);
  try {
    return JSON.parse(expression.slice(0, -1));
  } catch (error) {
    throw new Error(`Generated catalog ${exportName} is not JSON: ${error.message}`);
  }
}

function computeRevisions(source) {
  const catalog = parseJsonExport(source, 'rustCatalog', 'rustToolNamesByProfile');
  const namesByProfile = parseJsonExport(source, 'rustToolNamesByProfile', 'rustToolAnnotationOverridesByProfile');
  const overridesByProfile = parseJsonExport(source, 'rustToolAnnotationOverridesByProfile', 'rustToolsetRevisionByProfile');
  if (!Array.isArray(catalog) || catalog.length === 0) throw new Error('Generated Rust catalog is missing or empty');

  const byName = new Map();
  for (const tool of catalog) {
    if (!tool || typeof tool.name !== 'string' || byName.has(tool.name)) {
      throw new Error('Generated Rust catalog contains an invalid or duplicate tool');
    }
    byName.set(tool.name, tool);
  }

  const revisions = {};
  for (const profile of profileNames) {
    const names = namesByProfile?.[profile];
    const overrides = overridesByProfile?.[profile] ?? {};
    if (!Array.isArray(names) || names.length === 0) throw new Error(`Generated ${profile} tool names are missing or empty`);
    if (!overrides || typeof overrides !== 'object' || Array.isArray(overrides)) throw new Error(`Generated ${profile} annotation overrides are invalid`);
    const tools = names.map(name => {
      const base = byName.get(name);
      if (!base) throw new Error(`Generated ${profile} profile references an unknown tool: ${name}`);
      const annotations = overrides[name];
      return annotations === undefined ? base : { ...base, annotations };
    });
    revisions[profile] = createHash('sha256').update(JSON.stringify(tools)).digest('hex').slice(0, 16);
  }
  return revisions;
}

function replaceRevisions(source, revisions) {
  const range = exportRange(source, 'rustToolsetRevisionByProfile', 'rustBehavioralParityFixtures');
  const declaration = `export const rustToolsetRevisionByProfile: Readonly<Record<ToolProfile, string>> = ${JSON.stringify(revisions, null, 2)};`;
  return `${source.slice(0, range.start)}${declaration}${source.slice(range.end)}`;
}

const checkOnly = process.argv.includes('--check');
const requestedFile = argumentValue('--file');
const generatedFile = requestedFile ? path.resolve(process.cwd(), requestedFile) : defaultGeneratedFile;
const source = await readFile(generatedFile, 'utf8');
const revisions = computeRevisions(source);
const updated = replaceRevisions(source, revisions);

if (checkOnly) {
  if (updated !== source) {
    const current = parseJsonExport(source, 'rustToolsetRevisionByProfile', 'rustBehavioralParityFixtures');
    const drift = profileNames
      .filter(profile => current?.[profile] !== revisions[profile])
      .map(profile => `${profile}: ${current?.[profile] ?? '<missing>'} -> ${revisions[profile]}`)
      .join(', ');
    throw new Error(`Generated toolset revisions are stale (${drift}); run pnpm run sync:rust-revisions`);
  }
  console.log(`Generated toolset revisions are current: ${profileNames.map(profile => `${profile}=${revisions[profile]}`).join(' ')}`);
} else {
  if (updated !== source) await writeFile(generatedFile, updated);
  console.log(`${updated === source ? 'verified' : 'updated'} ${path.relative(process.cwd(), generatedFile)} toolset revisions`);
}
