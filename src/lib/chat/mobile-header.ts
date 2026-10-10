import type { Writable } from 'svelte/store';
/** Context keeps mobile header ownership local to each mounted application shell. */
export const MOBILE_CHAT_HEADER = Symbol('mobile-chat-header');
export interface MobileChatHeader {
  owner: symbol;
  title: string;
  status: string;
  online: boolean;
  presence?:ReturnType<typeof import('./navigation').sessionPresence>;
  openMenu: () => void;
}
export interface MobileHeaderContext {
  header: Writable<MobileChatHeader | null>;
  newChat: () => unknown;
}
