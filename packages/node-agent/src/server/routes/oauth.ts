import type { IncomingMessage, ServerResponse } from 'node:http';
import {
  authorizationMetadata,
  type OAuthRuntime,
  resourceMetadata,
  sendJson
} from '../../oauth.js';
import { readRequestBody, sendText } from '../http.js';

interface OAuthRouteOptions {
  base: string;
  localPathname: string;
  oauth: OAuthRuntime;
  prefix: string;
  url: URL;
}

function isAuthorizationMetadata(pathname: string, prefix: string): boolean {
  return pathname === '/.well-known/oauth-authorization-server'
    || (prefix !== '' && pathname === `/.well-known/oauth-authorization-server${prefix}`);
}

function isResourceMetadata(pathname: string, prefix: string): boolean {
  return pathname === '/.well-known/oauth-protected-resource'
    || pathname === '/.well-known/oauth-protected-resource/mcp'
    || (prefix !== '' && pathname === `/.well-known/oauth-protected-resource${prefix}/mcp`);
}

export async function handleOAuthRoute(
  req: IncomingMessage,
  res: ServerResponse,
  options: OAuthRouteOptions
): Promise<boolean> {
  const { base, localPathname, oauth, prefix, url } = options;

  if (req.method === 'GET' && isAuthorizationMetadata(url.pathname, prefix)) {
    sendJson(res, 200, authorizationMetadata(base, oauth));
    return true;
  }
  if (req.method === 'GET' && isResourceMetadata(url.pathname, prefix)) {
    sendJson(res, 200, resourceMetadata(base));
    return true;
  }
  if (localPathname === '/oauth/authorize' && req.method === 'GET') {
    const scoped = new URL(url.toString());
    scoped.pathname = '/oauth/authorize';
    const output = oauth.authorizePage(scoped);
    sendText(res, output.status, output.body, 'text/html; charset=utf-8');
    return true;
  }
  if (localPathname === '/oauth/authorize' && req.method === 'POST') {
    const form = new URLSearchParams((await readRequestBody(req, 8192)).toString());
    const output = await oauth.authorizeSubmitOneTime(form, base);
    if (output.location) {
      res.writeHead(output.status, {
        location: output.location,
        'cache-control': 'no-store'
      }).end();
      return true;
    }
    sendText(res, output.status, output.body ?? 'Authorization failed', 'text/html; charset=utf-8');
    return true;
  }
  if (localPathname === '/oauth/token' && req.method === 'POST') {
    const form = new URLSearchParams((await readRequestBody(req, 8192)).toString());
    const output = oauth.exchangeToken(form, req.headers, base);
    sendJson(res, output.status, output.body);
    return true;
  }

  return false;
}
