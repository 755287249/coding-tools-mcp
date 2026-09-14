import type { IncomingMessage, Server, ServerResponse } from 'node:http';
import type { AgentConfig, JsonObject, ToolContext } from '../../types.js';
import type { OAuthRuntime } from '../../oauth.js';
import { protectedResourceMetadataUrl, sendJson } from '../../oauth.js';
import {
  MCP_STREAM_HEARTBEAT_INTERVAL_MS,
  LATEST_LEGACY_MCP_PROTOCOL_VERSION,
  MODERN_MCP_PROTOCOL_VERSION,
  StreamingJsonResponse,
  decorateModernMcpResult,
  isModernMcpRequest,
  mcpProtocolVersion,
  sendMcpAccepted,
  sendMcpMethodNotAllowed,
  sendMcpTransportError,
  startMcpSubscriptionStream,
  validateJsonRpcMessage,
  validateMcpConnection,
  validateModernMcpRequest,
  validateModernMcpToolHeaders,
  type McpTransportIssue
} from '../../mcpTransport.js';
import type { ToolCatalogSnapshot } from '../catalog.js';
import { currentListenerPort, readRequestBody } from '../http.js';
import { dispatchMcpMethod, rpcErrorResponse } from '../mcp/dispatcher.js';
import { McpToolCallLifecycle } from '../mcp/lifecycle.js';
import { toolRuntimeFor } from '../../toolRuntime.js';
import { AGENT_VERSION } from '../../version.js';

function objectValue(value: unknown): JsonObject | undefined {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as JsonObject : undefined;
}

function clientSupportsFormElicitation(request: JsonObject): boolean {
  const params = objectValue(request.params);
  const meta = objectValue(params?._meta);
  const capabilities = objectValue(meta?.['io.modelcontextprotocol/clientCapabilities']);
  return objectValue(capabilities?.elicitation) !== undefined;
}

const modernRequestMethods = new Set([
  'server/discover',
  'tools/list',
  'tools/call',
  'prompts/list',
  'prompts/get',
  'resources/list',
  'resources/read',
  'tasks/get',
  'tasks/update',
  'tasks/cancel',
  'subscriptions/listen'
]);

function missingElicitationPreflight(config: AgentConfig, request: JsonObject, method: string): McpTransportIssue | undefined {
  if (method !== 'tools/call' || !isModernMcpRequest({}, request) || clientSupportsFormElicitation(request)) return undefined;
  const params = objectValue(request.params) ?? {};
  const name = String(params.name ?? '');
  const args = objectValue(params.arguments) ?? {};
  if (typeof params.requestState === 'string' && objectValue(params.inputResponses)) return undefined;
  if (config.securityPolicyCustomized || config.permissionMode !== 'guarded' || args.confirm === true) return undefined;
  const runtime = toolRuntimeFor(name);
  if (!runtime.guardedPermission) return undefined;
  return {
    status: 400,
    code: -32021,
    message: `Client capability elicitation is required to approve ${name}`,
    data: { requiredCapabilities: { elicitation: {} } }
  };
}

function requestedSubscriptionNotifications(request: JsonObject): JsonObject | undefined {
  const notifications = objectValue(objectValue(request.params)?.notifications);
  if (!notifications) return undefined;
  for (const key of ['toolsListChanged', 'promptsListChanged', 'resourcesListChanged']) {
    const value = notifications[key];
    if (value !== undefined && typeof value !== 'boolean') return undefined;
  }
  const resources = notifications.resourceSubscriptions;
  if (resources !== undefined && (!Array.isArray(resources) || resources.some(uri => typeof uri !== 'string'))) {
    return undefined;
  }
  return notifications;
}

interface McpRouteOptions {
  base: string;
  catalog: ToolCatalogSnapshot;
  config: AgentConfig;
  context: ToolContext;
  heartbeatIntervalMs?: number;
  oauth: OAuthRuntime;
  pathname: string;
  server: Server;
  startedAt: number;
}

export async function handleMcpRoute(
  req: IncomingMessage,
  res: ServerResponse,
  options: McpRouteOptions
): Promise<boolean> {
  const { base, catalog, config, context, oauth, pathname, server, startedAt } = options;
  if (pathname !== '/mcp') return false;

  const sendTransportIssue = (issue: McpTransportIssue, method?: string): void => {
    const fields: JsonObject = {
      rpc_error_code: String(issue.code),
      http_status: issue.status,
      transport_mode: 'streamable-http'
    };
    const protocolVersion = mcpProtocolVersion(req.headers);
    if (protocolVersion !== undefined) fields.protocol_version = protocolVersion;
    if (method) fields.method = method;
    context.usageStore.recordDiagnosticEvent({
      eventType: 'transport_event',
      event: 'mcp_transport_error',
      severity: 'error',
      failureDomain: 'transport',
      fields
    });
    sendMcpTransportError(res, issue);
  };

  let requestId: unknown = null;
  let lifecycle: McpToolCallLifecycle | undefined;
  let stream: StreamingJsonResponse | undefined;
  try {
    const connectionIssue = validateMcpConnection(
      req.headers,
      config,
      currentListenerPort(server, config.port)
    );
    if (connectionIssue) {
      sendTransportIssue(connectionIssue);
      return true;
    }
    if (!oauth.verifyBearer(req.headers, base)) {
      res.writeHead(401, {
        'www-authenticate': `Bearer resource_metadata="${protectedResourceMetadataUrl(base)}"`,
        'cache-control': 'no-store'
      }).end('Unauthorized');
      return true;
    }
    if (req.method !== 'POST') {
      sendMcpMethodNotAllowed(res);
      return true;
    }

    let parsed: unknown;
    try {
      parsed = JSON.parse((await readRequestBody(req)).toString());
    } catch (error) {
      const tooLarge = error instanceof Error && error.message === 'request body too large';
      const issue: McpTransportIssue = tooLarge
        ? { status: 400, code: -32600, message: 'request body too large' }
        : { status: 400, code: -32700, message: 'Parse error' };
      sendTransportIssue(issue);
      return true;
    }
    const validated = validateJsonRpcMessage(parsed);
    if ('status' in validated) {
      sendTransportIssue(validated);
      return true;
    }
    const request = validated.body;
    requestId = validated.id;
    const method = validated.method ?? '';
    if (validated.kind === 'response') {
      if (mcpProtocolVersion(req.headers) === MODERN_MCP_PROTOCOL_VERSION) {
        sendTransportIssue({
          status: 400,
          code: -32600,
          id: requestId,
          message: 'Streamable HTTP accepts only JSON-RPC requests or notifications from clients'
        });
      } else {
        sendMcpAccepted(res);
      }
      return true;
    }

    const modernIssue = validateModernMcpRequest(req.headers, request);
    if (modernIssue) {
      sendTransportIssue(modernIssue, method);
      return true;
    }
    const requestedTool = method === 'tools/call'
      ? catalog.tools.find(tool => tool.name === String((request.params as JsonObject | undefined)?.name ?? ''))
      : undefined;
    const mirroredHeaderIssue = validateModernMcpToolHeaders(req.headers, request, requestedTool);
    if (mirroredHeaderIssue) {
      sendTransportIssue(mirroredHeaderIssue, method);
      return true;
    }
    const missingElicitationIssue = requestedTool
      ? missingElicitationPreflight(config, request, method)
      : undefined;
    if (missingElicitationIssue) {
      missingElicitationIssue.id = requestId;
      sendTransportIssue(missingElicitationIssue, method);
      return true;
    }
    const protocolVersion = isModernMcpRequest(req.headers, request)
      ? MODERN_MCP_PROTOCOL_VERSION
      : mcpProtocolVersion(req.headers) ?? LATEST_LEGACY_MCP_PROTOCOL_VERSION;

    if (protocolVersion === MODERN_MCP_PROTOCOL_VERSION
        && validated.kind === 'request'
        && !modernRequestMethods.has(method)) {
      sendJson(res, 404, {
        jsonrpc: '2.0',
        id: requestId,
        error: { code: -32601, message: `Method not found: ${method}` }
      });
      return true;
    }

    if (method === 'subscriptions/listen' && protocolVersion === MODERN_MCP_PROTOCOL_VERSION) {
      if (validated.kind !== 'request') {
        sendTransportIssue({
          status: 400,
          code: -32600,
          id: requestId,
          message: 'subscriptions/listen requires a JSON-RPC request id'
        }, method);
        return true;
      }
      const requestedNotifications = requestedSubscriptionNotifications(request);
      if (!requestedNotifications) {
        sendJson(res, 200, {
          jsonrpc: '2.0',
          id: requestId,
          error: { code: -32602, message: 'Invalid params: notifications is required and must be a valid subscription filter' }
        });
        return true;
      }
      // This server currently advertises tools.listChanged=false and no prompt/resource
      // notification capabilities, so no requested change stream can be honored yet.
      startMcpSubscriptionStream(
        res,
        requestId,
        {},
        options.heartbeatIntervalMs ?? MCP_STREAM_HEARTBEAT_INTERVAL_MS
      );
      return true;
    }

    const fastPath = method === 'initialize'
      || method === 'server/discover'
      || method === 'ping'
      || method === 'tools/list'
      || method === 'prompts/list'
      || method === 'resources/list'
      || method.startsWith('notifications/');
    if (validated.kind === 'request' && !fastPath) {
      stream = new StreamingJsonResponse(
        res,
        options.heartbeatIntervalMs ?? MCP_STREAM_HEARTBEAT_INTERVAL_MS
      );
    }

    let rpcResponse: JsonObject;
    try {
      if (method === 'tools/call') lifecycle = new McpToolCallLifecycle(context, req, res);
      const result = await dispatchMcpMethod({
        catalog,
        context,
        method,
        processLifecycle: lifecycle?.process,
        protocolVersion,
        req,
        request,
        startedAt
      });
      const wireResult = protocolVersion === MODERN_MCP_PROTOCOL_VERSION
        ? decorateModernMcpResult(result, method, {
            name: 'coding-tools-mcp-node',
            title: 'Coding Tools MCP Node Agent',
            version: AGENT_VERSION
          })
        : result;
      rpcResponse = { jsonrpc: '2.0', id: requestId, result: wireResult };
      lifecycle?.complete();
    } catch (error) {
      lifecycle?.abort();
      rpcResponse = rpcErrorResponse(requestId, error);
    } finally {
      lifecycle?.dispose();
    }

    if (validated.kind === 'notification') {
      sendMcpAccepted(res);
      return true;
    }
    if (stream) {
      stream.finish(rpcResponse);
      return true;
    }
    sendJson(res, 200, rpcResponse);
    return true;
  } catch (error) {
    lifecycle?.abort();
    lifecycle?.dispose();
    const response = rpcErrorResponse(requestId, error);
    if (stream) {
      stream.finish(response);
      return true;
    }
    if (!res.headersSent) {
      sendJson(res, 200, response);
      return true;
    }
    res.destroy();
    return true;
  }
}
