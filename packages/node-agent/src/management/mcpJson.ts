import { promises as fs } from 'node:fs';
import path from 'node:path';

const NAME = /^[A-Za-z0-9][A-Za-z0-9._ -]{0,63}$/;

function isObject(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

function hasTarget(server: Record<string, unknown>): boolean {
  return ['command', 'url'].some(key => typeof server[key] === 'string' && (server[key] as string).trim().length > 0);
}

/**
 * Merges servers into `mcpServers` of `<folder>/.mcp.json` (same behaviour as the
 * Tauri `add_workspace_mcp_servers` command). Keeps every other entry and refuses to
 * overwrite a file that is not valid JSON.
 */
export async function mergeMcpJson(folder: string, servers: unknown): Promise<string[]> {
  if (!isObject(servers)) throw new Error('servers must be an object');
  const entries = Object.entries(servers);
  if (!entries.length || entries.length > 64) throw new Error('servers must contain 1 to 64 entries');
  for (const [name, server] of entries) {
    if (!NAME.test(name)) throw new Error(`invalid MCP server name: ${name}`);
    if (!isObject(server) || !hasTarget(server)) throw new Error(`MCP server ${name} needs a command or url`);
  }
  const stat = await fs.stat(folder).catch(() => null);
  if (!stat?.isDirectory()) throw new Error(`project folder does not exist: ${folder}`);
  const file = path.join(folder, '.mcp.json');
  let document: Record<string, unknown> = {};
  const text = await fs.readFile(file, 'utf8').catch((error: NodeJS.ErrnoException) => {
    if (error.code === 'ENOENT') return '';
    throw error;
  });
  if (text.trim()) {
    let parsed: unknown;
    try { parsed = JSON.parse(text); }
    catch (error) { throw new Error(`${file} is not valid JSON: ${error instanceof Error ? error.message : String(error)}`); }
    if (!isObject(parsed)) throw new Error(`${file} must contain a JSON object`);
    document = parsed;
  }
  const existing = document.mcpServers ?? {};
  if (!isObject(existing)) throw new Error(`mcpServers in ${file} must be an object`);
  document.mcpServers = { ...existing, ...servers };
  const temp = `${file}.${process.pid}.tmp`;
  await fs.writeFile(temp, `${JSON.stringify(document, null, 2)}\n`, 'utf8');
  await fs.rename(temp, file);
  return entries.map(([name]) => name);
}
