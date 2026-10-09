import { getBackend } from '$lib/backend';
export interface ChatFile { id: string; name: string; path: string; mime: string; size: number; sha256: string }
export interface ChatMessage { attachments?: ChatFile[]; tool_event?: {name: string; status: "running" | "completed" | "failed"}; id: string; role: 'user' | 'assistant'; text: string; created_at: number; reply_to?: string; final?: boolean }
export interface ChatSession { id: string; title: string; created_at: number; updated_at: number; closed: boolean; status: 'waiting' | 'connected' | 'offline' | 'closed'; archive_path: string; messages?: ChatMessage[] }
export interface ChatResult { attachment?: ChatFile; data_base64?: string; sessions?: ChatSession[]; session?: ChatSession }
export type ChatAction = { action: 'list' | 'create' | 'read' | 'send' | 'close' | 'upload' | 'read_attachment'; chat_id?: string; message_id?: string; text?: string; title?: string; upload_id?: string; name?: string; data_base64?: string; attachment_ids?: string[] };
export function localChat(workspaceId: string, folderId: string, args: ChatAction): Promise<ChatResult> { return getBackend().chat.request(workspaceId, folderId, args); }
