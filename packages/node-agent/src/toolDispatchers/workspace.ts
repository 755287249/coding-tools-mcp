import {
  applyPatchTool,
  editTool,
  listFilesTool,
  patchCheckTool,
  projectMapTool,
  readFileTool,
  readManyTool,
  searchTextTool,
  viewImageTool
} from '../fileTools.js';
import { fileOpsTool } from '../fileOpsTools.js';
import { formatFilesTool } from '../formatterTools.js';
import type { ToolHandlerMap } from '../toolDispatch/contract.js';

export const workspaceToolHandlers = {
  read_file: ({ ctx, key, args }) => readFileTool(ctx, key, args),
  read_many: ({ ctx, key, args }) => readManyTool(ctx, key, args),
  project_map: ({ ctx, key, args }) => projectMapTool(ctx, key, args),
  list_files: ({ ctx, key, args }) => listFilesTool(ctx, key, args),
  search_text: ({ ctx, key, args, knowledgeCanary }) => searchTextTool(ctx, key, args, {
    exactTotalFastTail: knowledgeCanary?.applied === true
      && knowledgeCanary.implementation === 'search_exact_total_fast_tail'
  }),
  apply_patch: ({ ctx, key, args }) => applyPatchTool(ctx, key, args),
  edit: ({ ctx, key, args }) => editTool(ctx, key, args),
  file_ops: ({ ctx, key, args }) => fileOpsTool(ctx, key, args),
  format_files: ({ ctx, key, args }) => formatFilesTool(ctx, key, args),
  patch_check: ({ ctx, key, args }) => patchCheckTool(ctx, key, args),
  view_image: ({ ctx, key, args }) => viewImageTool(ctx, key, args)
} satisfies ToolHandlerMap;
