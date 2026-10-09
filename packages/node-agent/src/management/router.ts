import { handleChatRoute } from '../chat/management.js';
import type { IncomingMessage, ServerResponse } from 'node:http';
import { handleManagementUiRequest, isManagementUiPath } from '../managementUi.js';
import { sendJson } from '../oauth.js';
import { managementClientAllowed, sameOrigin, validAdminToken } from './http.js';
import { statusPayload } from './runtime.js';
import { handleManagementConfigurationRoute } from './routes/configuration.js';
import { handleManagementObservabilityRoute } from './routes/observability.js';
import type { ManagementOptions } from './types.js';

export async function handleManagementRequest(
  req: IncomingMessage,
  res: ServerResponse,
  pathname: string,
  options: ManagementOptions
): Promise<boolean> {
  const uiPath = isManagementUiPath(pathname);
  const managementPath = uiPath || pathname.startsWith('/admin/api/');
  if (!managementPath || !options.context.config.management.enabled) return false;
  if (!managementClientAllowed(req)) {
    sendJson(res, 403, {
      error: {
        code: 'LOCAL_MANAGEMENT_ONLY',
        message: 'Management UI is available only through a loopback address.'
      }
    });
    return true;
  }
  if (uiPath && await handleManagementUiRequest(req, res, pathname, options.adminToken)) return true;
  if (!pathname.startsWith('/admin/api/')) return false;
  if (!validAdminToken(req, options.adminToken) || !sameOrigin(req)) {
    sendJson(res, 403, {
      error: {
        code: 'MANAGEMENT_REQUEST_REJECTED',
        message: 'Management API requires a same-origin UI request.'
      }
    });
    return true;
  }
  if (pathname === '/admin/api/status' && req.method === 'GET') {
    sendJson(res, 200, statusPayload(options));
    return true;
  }
  if (await handleChatRoute(req, res, pathname, options)) return true;
  if (await handleManagementObservabilityRoute(req, res, pathname, options)) return true;
  if (await handleManagementConfigurationRoute(req, res, pathname, options)) return true;
  sendJson(res, 405, {
    error: { code: 'METHOD_NOT_ALLOWED', message: 'Method not allowed.' }
  });
  return true;
}
