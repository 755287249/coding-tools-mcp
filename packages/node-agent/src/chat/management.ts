import type { IncomingMessage, ServerResponse } from 'node:http';
import type { ManagementOptions } from '../management/types.js';
import { runtimeRecord } from '../management/runtime.js';
import { readManagementBody } from '../management/http.js';
import { sendJson } from '../oauth.js';
import { chatUi } from './store.js';
import { revealChatPath } from './reveal.js';
export async function handleChatRoute(req: IncomingMessage, res: ServerResponse, pathname: string, options: ManagementOptions): Promise<boolean> {
  const route = pathname.match(/^\/admin\/api\/workspaces\/([A-Za-z0-9._-]{1,128})\/chat$/);
  if (!route) return false;
  if (req.method !== 'POST') { sendJson(res, 405, { error: { message: 'POST required' } }); return true; }
  try {
    const runtime = runtimeRecord(options, route[1]);
    if (!runtime) throw new Error('Workspace not found');
    const body = await readManagementBody(req, 3 * 1024 * 1024) as Record<string, unknown>;
    const folder = runtime.context.config.folders.find(f => f.id === body?.folder_id);
    if (!folder) throw new Error('Select a configured folder');
    const result = chatUi(folder.path, body);
    if (body.action === 'reveal_path') await revealChatPath(String(result.path), result.is_directory === true);
    sendJson(res, 200, result);
  } catch (error) { sendJson(res, 400, { error: { code: 'CHAT_ERROR', message: error instanceof Error ? error.message : String(error) } }); }
  return true;
}
