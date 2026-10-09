import { getBackend } from '$lib/backend';
export interface ChatMessage { id: string; role: 'user' | 'assistant'; text: string; created_at: number; reply_to?: string; final?: boolean }
export interface ChatSession { id: string; title: string; created_at: number; updated_at: number; closed: boolean; status: 'waiting' | 'connected' | 'offline' | 'closed'; archive_path: string; messages?: ChatMessage[] }
export interface ChatResult { sessions?: ChatSession[]; session?: ChatSession }
export type ChatAction = { action: 'list' | 'create' | 'read' | 'send' | 'close'; chat_id?: string; message_id?: string; text?: string; title?: string };
export function localChat(workspaceId: string, folderId: string, args: ChatAction): Promise<ChatResult> { return getBackend().chat.request(workspaceId, folderId, args); }
