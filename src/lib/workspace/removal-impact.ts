import { localChat } from '$lib/api/chat';
import { seedRequest } from '$lib/api/seeds';
import type { WorkspaceFolder } from '$lib/types';

export interface RemovalImpact { folders: number; chats: number; seeds: number; unavailable: number }
export async function removalImpact(workspaceId: string, folders: WorkspaceFolder[]): Promise<RemovalImpact> {
  const result: RemovalImpact = {folders:folders.length,chats:0,seeds:0,unavailable:0};
  // Counts are advisory snapshots; the backend rechecks ownership during removal.
  await Promise.all(folders.map(async folder => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const [chats,seeds] = await Promise.race([
        Promise.all([localChat(workspaceId,folder.id,{action:'list'}),seedRequest(workspaceId,folder.id,{action:'seed_list',limit:1,include_archived:true})]),
        new Promise<never>((_,reject)=>{timer=setTimeout(()=>reject(new Error('Impact lookup timed out')),8000);})
      ]);
      result.chats += chats.sessions?.length ?? 0;
      result.seeds += Math.max(0,(seeds.total??0)-seeds.retired_count);
    } catch { result.unavailable++; }
    finally { clearTimeout(timer); }
  }));
  return result;
}
