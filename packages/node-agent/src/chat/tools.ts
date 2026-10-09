import { chatTool, chatWait } from './store.js';
import { selectedFolder } from '../workspace.js';
import type { ToolHandlerMap } from '../toolDispatch/contract.js';
export const chatToolHandlers: ToolHandlerMap = {
  chat_open: ({ ctx, key, args }) => chatTool(selectedFolder(ctx, key).path, 'chat_open', args),
  chat_wait: ({ ctx, key, args, processLifecycle }) => chatWait(selectedFolder(ctx, key).path, args, processLifecycle?.signal),
  chat_reply: ({ ctx, key, args }) => chatTool(selectedFolder(ctx, key).path, 'chat_reply', args),
  chat_close: ({ ctx, key, args }) => chatTool(selectedFolder(ctx, key).path, 'chat_close', args),
};
