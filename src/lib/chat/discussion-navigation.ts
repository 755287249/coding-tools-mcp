import type { ChatSession, Discussion } from '../api/chat';
export type NavigationChat = ChatSession & { discussionId?: string };
export function discussionLocation(workspace: string, folder: string, id?: string): string {
 const query = new URLSearchParams({folder, panel:'discussions'});
 if(id) query.set('discussion', id); else query.set('createGroup', '1');
 return `/workspace/${encodeURIComponent(workspace)}?${query}`;
}
export function discussionNavigation(d: Discussion, sources: ChatSession[]): NavigationChat {
 const live = sources.filter(s=>d.members.includes(s.id) && !s.closed && !s.archived);
 const online = !d.paused && live.some(s=>s.status==='connected'||s.status==='waiting');
 return {id:`discussion:${d.id}`,discussionId:d.id,title:d.name,mode:'group',pinned:d.pinned,
  archived:d.archived,closed:false,created_at:d.updated_at,updated_at:d.updated_at,
  archive_path:d.archive_path??'',status:online?'connected':'offline',
  work_state:!d.paused&&live.some(s=>s.work_state==='processing')?'processing':null};
}
