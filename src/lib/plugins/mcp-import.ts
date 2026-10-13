// Recognises the JSON shapes people paste or pick in the "add plugin" dialog.
// Pure (no Svelte / backend imports) so it can be unit tested with node --test.

export type McpServerConfig = Record<string, unknown>;
export type ImportKind =
  | { kind: 'plugin'; value: Record<string, unknown> }
  | { kind: 'mcp'; servers: Record<string, McpServerConfig> }
  | { kind: 'mcp-single'; server: McpServerConfig }
  | { kind: 'unknown' };

const NAME = /^[A-Za-z0-9][A-Za-z0-9._ -]{0,63}$/;

function object(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

export function isServer(value: unknown): boolean {
  return object(value) && (typeof value.command === 'string' && !!value.command.trim() || typeof value.url === 'string' && !!value.url.trim());
}

export function validServerName(name: string): boolean {
  return NAME.test(name.trim());
}

/**
 * Detects what a JSON value is:
 * - `{format:1, manifest, code}` → app plugin package
 * - `{mcpServers:{...}}` (Claude/Cursor), `{servers:{...}}` (VS Code), `{mcp:{servers:{...}}}` → MCP servers
 * - `{name:{command|url}}` → MCP servers without a wrapper
 * - `{command|url}` → one MCP server that still needs a name
 */
export function detectImport(value: unknown): ImportKind {
  if (!object(value)) return { kind: 'unknown' };
  if (value.format === 1 && 'manifest' in value && 'code' in value) return { kind: 'plugin', value };
  if (isServer(value)) return { kind: 'mcp-single', server: value as McpServerConfig };
  const wrapped = object(value.mcpServers) ? value.mcpServers
    : object(value.servers) ? value.servers
    : object(value.mcp) && object(value.mcp.servers) ? value.mcp.servers
    : value;
  const entries = Object.entries(wrapped);
  if (entries.length && entries.every(([, server]) => object(server))) {
    const servers = Object.fromEntries(entries) as Record<string, McpServerConfig>;
    // A bare object is only MCP if at least one entry looks like a server; avoids
    // treating arbitrary JSON as configuration.
    if (wrapped !== value || entries.some(([, server]) => isServer(server))) return { kind: 'mcp', servers };
  }
  return { kind: 'unknown' };
}

/** Returns a list of problems (server names without command/url, bad names). Empty = OK. */
export function serverProblems(servers: Record<string, McpServerConfig>): { invalidNames: string[]; missingTarget: string[] } {
  const invalidNames: string[] = [], missingTarget: string[] = [];
  for (const [name, server] of Object.entries(servers)) {
    if (!validServerName(name)) invalidNames.push(name);
    else if (!isServer(server)) missingTarget.push(name);
  }
  return { invalidNames, missingTarget };
}
