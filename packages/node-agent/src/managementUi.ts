import { readFile, stat } from "node:fs/promises";
import type { IncomingMessage, ServerResponse } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const uiRoot = fileURLToPath(new URL("./ui/", import.meta.url));

function securityHeaders(contentSecurityPolicy = false): Record<string, string> {
  const headers: Record<string, string> = {
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
    "x-frame-options": "DENY",
    "referrer-policy": "no-referrer",
    "permissions-policy": "camera=(), microphone=(), geolocation=()",
  };
  if (contentSecurityPolicy) {
    headers["content-security-policy"] = [
      "default-src 'none'",
      "style-src 'self'",
      "script-src 'self'",
      "connect-src 'self'",
      "img-src 'self' blob: data:",
      "manifest-src 'self'",
      "worker-src 'self'",
      "frame-src 'self'",
      "base-uri 'none'",
      "form-action 'none'",
      "frame-ancestors 'none'",
    ].join("; ");
  }
  return headers;
}

function contentTypeFor(filePath: string): string {
  const ext = path.extname(filePath).toLowerCase();
  switch (ext) {
    case ".html":
      return "text/html; charset=utf-8";
    case ".js":
      return "text/javascript; charset=utf-8";
    case ".css":
      return "text/css; charset=utf-8";
    case ".json":
      return "application/json; charset=utf-8";
    case ".webmanifest":
      return "application/manifest+json; charset=utf-8";
    case ".svg":
      return "image/svg+xml; charset=utf-8";
    case ".png":
      return "image/png";
    case ".woff2":
      return "font/woff2";
    default:
      return "application/octet-stream";
  }
}

function injectAdminToken(html: string, adminToken: string): string {
  if (!/^[A-Za-z0-9_-]+$/.test(adminToken)) throw new Error("Invalid management UI token.");
  if (html.includes('name="ctmcp-admin-token"')) {
    return html.replace(
      /<meta\s+name="ctmcp-admin-token"\s+content="[^"]*"\s*\/?>/i,
      `<meta name="ctmcp-admin-token" content="${adminToken}">`,
    );
  }
  return html.replace(
    /<head([^>]*)>/i,
    `<head$1>\n<meta name="ctmcp-admin-token" content="${adminToken}">`,
  );
}

function resolveUiFile(pathname: string, assetRoot: string): { filePath: string; spaFallback: boolean } | null {
  const relative = decodeURIComponent(pathname.replace(/^\/ui\/?/, ""));
  const target = path.resolve(assetRoot, relative);
  const root = path.resolve(assetRoot);
  if (target !== root && !target.startsWith(root + path.sep)) return null;
  return { filePath: target, spaFallback: !path.extname(relative) };
}

async function sendFile(res: ServerResponse, filePath: string, html = false): Promise<void> {
  const content = await readFile(filePath);
  const headers: Record<string, string> = {
    "content-type": html ? "text/html; charset=utf-8" : contentTypeFor(filePath),
    ...securityHeaders(html),
  };
  if (path.basename(filePath) === "sw.js") headers["service-worker-allowed"] = "/ui/";
  res.writeHead(200, headers).end(content);
}

const FALLBACK_PAGE = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="ctmcp-admin-token" content="">
<meta name="description" content="Coding Tools MCP Headless Agent management interface">
<title>Coding Tools MCP</title>
</head>
<body data-ui-framework="svelte"></body>
</html>`;

async function readIndexHtml(assetRoot: string): Promise<string> {
  try {
    return await readFile(path.join(assetRoot, "index.html"), "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return FALLBACK_PAGE;
    throw error;
  }
}

async function sendPage(res: ServerResponse, adminToken: string, assetRoot: string): Promise<void> {
  const html = injectAdminToken(await readIndexHtml(assetRoot), adminToken);
  res.writeHead(200, {
    "content-type": "text/html; charset=utf-8",
    ...securityHeaders(true),
  }).end(html);
}

function sendUnavailable(res: ServerResponse, code: string | undefined): void {
  res.writeHead(code === "ENOENT" ? 503 : 500, {
    "content-type": "application/json; charset=utf-8",
    ...securityHeaders(),
  }).end(JSON.stringify({
    error: {
      code: code === "ENOENT" ? "MANAGEMENT_UI_NOT_BUILT" : "MANAGEMENT_UI_UNAVAILABLE",
      message: code === "ENOENT"
        ? "Management UI assets are unavailable. Run the package build before starting the Agent."
        : "Management UI assets could not be loaded.",
    },
  }));
}

const UI_CLIENT_ROUTE = /^\/(?:workspace|settings|quick-setup)(?:\/|$)/;

export function isManagementUiPath(pathname: string): boolean {
  return pathname === "/" || pathname === "/ui" || pathname.startsWith("/ui/") || UI_CLIENT_ROUTE.test(pathname);
}

function redirectToUiBase(req: IncomingMessage, res: ServerResponse, pathname: string): void {
  const raw = req.url ?? pathname;
  const searchIndex = raw.indexOf("?");
  const search = searchIndex >= 0 ? raw.slice(searchIndex) : "";
  res.writeHead(302, { location: `/ui${pathname}${search}`, ...securityHeaders() }).end();
}

export async function handleManagementUiRequest(
  req: IncomingMessage,
  res: ServerResponse,
  pathname: string,
  adminToken: string,
  assetRoot = uiRoot,
): Promise<boolean> {
  if (!isManagementUiPath(pathname)) return false;
  if (req.method !== "GET") {
    res.writeHead(405, {
      allow: "GET",
      "content-type": "application/json; charset=utf-8",
      ...securityHeaders(),
    }).end(JSON.stringify({ error: { code: "METHOD_NOT_ALLOWED", message: "Method not allowed." } }));
    return true;
  }
  if (pathname === "/ui/chat-preview.html") {
    try {
      const runner = await readFile(path.join(assetRoot, "chat-preview.html"));
      res.writeHead(200, {
        "content-type": "text/html; charset=utf-8",
        "cache-control": "no-store",
        "x-content-type-options": "nosniff",
        "referrer-policy": "no-referrer",
        "permissions-policy": "camera=(), microphone=(), geolocation=()",
        "content-security-policy": "sandbox allow-scripts; frame-ancestors 'self'",
      }).end(runner);
    } catch { res.writeHead(404).end(); }
    return true;
  }
  if (pathname === "/" ) {
    res.writeHead(302, { location: "/ui/", ...securityHeaders() }).end();
    return true;
  }
  if (UI_CLIENT_ROUTE.test(pathname)) {
    redirectToUiBase(req, res, pathname);
    return true;
  }
  if (pathname === "/ui" || pathname === "/ui/") {
    try {
      await sendPage(res, adminToken, assetRoot);
    } catch (error) {
      sendUnavailable(res, (error as NodeJS.ErrnoException).code);
    }
    return true;
  }

  const resolved = resolveUiFile(pathname, assetRoot);
  if (!resolved) return false;
  try {
    const info = await stat(resolved.filePath);
    if (info.isFile()) {
      await sendFile(res, resolved.filePath, path.extname(resolved.filePath) === ".html");
      return true;
    }
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code !== "ENOENT") {
      sendUnavailable(res, code);
      return true;
    }
  }

  if (resolved.spaFallback) {
    try {
      await sendPage(res, adminToken, assetRoot);
      return true;
    } catch (error) {
      sendUnavailable(res, (error as NodeJS.ErrnoException).code);
      return true;
    }
  }

  res.writeHead(404, {
    "content-type": "application/json; charset=utf-8",
    ...securityHeaders(),
  }).end(JSON.stringify({ error: { code: "NOT_FOUND", message: "Not found." } }));
  return true;
}
