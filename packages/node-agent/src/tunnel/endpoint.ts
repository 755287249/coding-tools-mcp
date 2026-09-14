export const TUNNEL_PROTOCOL_VERSION = 4;
export const TUNNEL_WS_PATH = '/_tunnel/v1';
export const TUNNEL_ENROLL_PATH = '/_tunnel/enroll';
export const TUNNEL_SUBPROTOCOL = 'coding-tools-tunnel-v4';

export interface TunnelEndpoint {
  publicUrl: string;
  baseUrl: string;
  clientId: string;
  websocketUrl: string;
}

export function parseBuiltinPublicUrl(value: string): TunnelEndpoint {
  const url = new URL(value.trim());
  if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash) {
    throw new Error('built-in tunnel public URL must be a clean HTTPS URL');
  }
  const match = /^\/builtin\/clients\/([A-Za-z0-9_-]{1,64})\/mcp\/?$/.exec(url.pathname);
  if (!match) throw new Error('built-in tunnel public URL must use /builtin/clients/<client-id>/mcp');
  url.pathname = url.pathname.replace(/\/$/, '');
  const publicUrl = url.toString().replace(/\/$/, '');
  const base = new URL(publicUrl);
  base.pathname = base.pathname.replace(/\/mcp$/, '');
  const websocket = new URL(publicUrl);
  websocket.protocol = 'wss:';
  websocket.pathname = TUNNEL_WS_PATH;
  websocket.search = '';
  websocket.hash = '';
  return { publicUrl, baseUrl: base.toString().replace(/\/$/, ''), clientId: match[1], websocketUrl: websocket.toString() };
}

export function builtinEndpointForClient(value: string, clientId: string): TunnelEndpoint {
  if (!/^[A-Za-z0-9_-]{1,64}$/.test(clientId)) {
    throw new Error('built-in tunnel client ID must contain only letters, numbers, underscores, or hyphens');
  }
  const configured = parseBuiltinPublicUrl(value);
  const url = new URL(configured.publicUrl);
  url.pathname = `/builtin/clients/${clientId}/mcp`;
  return parseBuiltinPublicUrl(url.toString());
}

export function authSigningPayload(nonce: string, deviceId: string, clientId: string, workerId: string): Buffer {
  return Buffer.from(JSON.stringify({
    protocol_version: TUNNEL_PROTOCOL_VERSION,
    nonce,
    device_id: deviceId,
    client_id: clientId,
    service: 'mcp',
    worker_id: workerId
  }));
}

export function serverChallengeSigningPayload(
  nonce: string,
  expiresAtUnixMs: number,
  serverId: string,
  deviceId: string,
  clientId: string,
  workerId: string
): Buffer {
  return Buffer.from(JSON.stringify({
    protocol_version: TUNNEL_PROTOCOL_VERSION,
    nonce,
    expires_at_unix_ms: expiresAtUnixMs,
    server_id: serverId,
    device_id: deviceId,
    client_id: clientId,
    service: 'mcp',
    worker_id: workerId
  }));
}

export function serverAckSigningPayload(
  nonce: string,
  serverId: string,
  deviceId: string,
  clientId: string,
  workerId: string,
  workerPolicy: unknown
): Buffer {
  return Buffer.from(JSON.stringify({
    protocol_version: TUNNEL_PROTOCOL_VERSION,
    nonce,
    server_id: serverId,
    device_id: deviceId,
    client_id: clientId,
    service: 'mcp',
    worker_id: workerId,
    worker_policy: workerPolicy
  }));
}
