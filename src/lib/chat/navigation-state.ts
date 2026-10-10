import { onChatCacheReset } from './cache-lifecycle.js';
import { get, writable } from 'svelte/store';
import { workspaces } from '../stores/app';
import { workspaceFolders } from '../types';
import { localChat, localDiscussion } from '../api/chat';
import { discussionNavigation, type NavigationChat } from './discussion-navigation';
import { conversationOrder } from './navigation';
import { reconcileSnapshot, startVisiblePolling } from './polling';

export const navigationSessions = writable<Record<string, NavigationChat[]>>({});
export const navigationErrors = writable<Record<string, string>>({});
export const navigationScope = (workspace:string, folder:string) => JSON.stringify([workspace,folder]);
let identity=0;
onChatCacheReset(()=>{identity++;navigationSessions.set({});navigationErrors.set({})});
let subscribers = 0;
let release: (()=>void) | undefined;
/** The shell owns polling, so opening a drawer neither empties data nor starts requests. */
export function acquireNavigation() {
  if (++subscribers === 1) {
    let signature = '', stop: (()=>void) | undefined;
    const unsubscribe = workspaces.subscribe(list => {
      const targets = list.flatMap(workspace => workspaceFolders(workspace).map(folder => ({workspace:workspace.id,folder:folder.id,key:navigationScope(workspace.id,folder.id)})));
      const next = JSON.stringify(targets);
      if (signature === next) return;
      signature = next; stop?.();
      const keys = new Set(targets.map(t=>t.key));
      navigationSessions.update(current=>Object.fromEntries(Object.entries(current).filter(([key])=>keys.has(key))));
      navigationErrors.set({});
      let stopped = false;
      const cancel = startVisiblePolling(async () => {
        for(let offset=0;offset<targets.length;offset+=4) {
          if (stopped || document.hidden) return;
          await Promise.all(targets.slice(offset,offset+4).map(async target => {
            const epoch=identity;
            try {
              const [chats, groups] = await Promise.all([localChat(target.workspace,target.folder,{action:'list'}), localDiscussion(target.workspace,target.folder,{action:'discussion_list'})]);
              if (stopped || epoch!==identity) return;
              const next = [...(chats.sessions??[]),...(groups.discussions??[]).map(d=>discussionNavigation(d,chats.sessions??[]))].sort(conversationOrder);
              navigationSessions.update(current=>{const items=reconcileSnapshot(current[target.key],next);return items===current[target.key]?current:{...current,[target.key]:items}});
              if(get(navigationErrors)[target.key]) navigationErrors.update(current=>({...current,[target.key]:''}));
            } catch(error) { if(!stopped && epoch===identity) navigationErrors.update(current=>({...current,[target.key]:String(error)})); }
          }));
        }
      },()=>3000);
      stop=()=>{stopped=true;cancel()};
    });
    release=()=>{unsubscribe();stop?.();navigationSessions.set({});navigationErrors.set({})};
  }
  return ()=>{if(--subscribers===0){release?.();release=undefined}};
}
