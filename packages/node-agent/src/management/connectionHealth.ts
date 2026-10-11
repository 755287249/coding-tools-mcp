import type { ManagementOptions } from './types.js';
export async function probeConnectionInfo(endpoint: string): Promise<string | undefined> {
  try {
    const url = new URL(endpoint); url.pathname = `${url.pathname.replace(/\/mcp\/?$/, '').replace(/\/$/, '')}/mcp/info`;
    url.search = ''; url.hash = '';
    const response = await fetch(url, {redirect:'error',signal:AbortSignal.timeout(4000)});
    if (!response.ok) return `HTTP ${response.status}`;
    const info = await response.json() as {name?:string;protocolVersion?:string};
    return ['coding-tools-mcp-node','coding-tools-mcp'].includes(info.name ?? '') && typeof info.protocolVersion === 'string' ? undefined : 'Endpoint is not an MCP service';
  } catch (error) { return error instanceof Error ? error.message : String(error); }
}
export async function connectionHealth(options: ManagementOptions) {
  const config = options.configStore.current;
  const local = `http://${config.host === '0.0.0.0' ? '127.0.0.1' : config.host === '::' ? '[::1]' : config.host.includes(':') ? `[${config.host}]` : config.host}:${config.port}`;
  const endpoint = config.tunnel?.enabled ? config.tunnel.publicUrl : config.publicBaseUrl || `${local}/mcp`;
  const [serviceError,entryError] = await Promise.all([probeConnectionInfo(`${local}/mcp`),probeConnectionInfo(endpoint)]);
  const tunnel = options.context.tunnelStatus;
  const connecting = tunnel?.enabled && ['connecting','starting','reconnecting'].includes(tunnel.state);
  return {state:serviceError?'offline':connecting?'connecting':entryError?'error':'online',observedAt:Date.now(),service:serviceError?'error':'online',entry:entryError?'error':'online',endpoint,detail:serviceError || entryError || 'MCP service and entry are reachable'};
}
