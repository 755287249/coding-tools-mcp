import { get } from 'svelte/store';
import { confirm } from '$lib/api/native';
import { t } from '$lib/i18n';
import type { WorkspaceFolder } from '$lib/types';
import { removalImpact } from './removal-impact';

export async function confirmFolderRemoval(workspaceId:string,folder:WorkspaceFolder):Promise<boolean> {
  const impact=await removalImpact(workspaceId,[folder]),tr=get(t);
  return confirm([
    tr('Remove folder “{name}” from the workspace? Files on disk will not be deleted.',{name:folder.name}),
    folder.path,
    tr('workspace.impact',{folders:impact.folders,chats:impact.chats,seeds:impact.seeds}),
    impact.unavailable?tr('workspace.impactUnknown'): '',
    tr('workspace.folderImpactHint')
  ].filter(Boolean).join('\n\n'),{title:tr('Remove folder'),kind:'warning',okLabel:tr('Remove'),cancelLabel:tr('Cancel')});
}
