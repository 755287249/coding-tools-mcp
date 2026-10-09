import type { ChatFile, ChatAction, ChatResult } from '../api/chat';
export const TRANSFER_CHUNK_BYTES = 512 * 1024;
export const MAX_BROWSER_PREVIEW_BYTES = 32 * 1024 * 1024;
type Request = (args: ChatAction) => Promise<ChatResult>;
const base64 = (bytes: Uint8Array) => { let binary = ''; for (let offset=0;offset<bytes.length;offset+=8192) binary += String.fromCharCode(...bytes.subarray(offset,offset+8192)); return btoa(binary); };

/** Bounded transport chunks; the attachment itself has no 2 MiB upload cap. */
export async function uploadLocalFile(request: Request, chatId: string, file: File, uploadId: string): Promise<ChatFile> {
  if (!file.size) throw new Error('File is empty');
  let attachment: ChatFile | undefined;
  for (let offset=0;offset<file.size;offset+=TRANSFER_CHUNK_BYTES) {
    const bytes = new Uint8Array(await file.slice(offset,offset+TRANSFER_CHUNK_BYTES).arrayBuffer());
    const args: ChatAction = {action:'upload_chunk',chat_id:chatId,upload_id:uploadId,name:file.name,offset,total_size:file.size,data_base64:base64(bytes)};
    let result: ChatResult;
    try { result = await request(args); } catch { result = await request(args); }
    if (result.next_offset !== offset+bytes.length) throw new Error('Upload offset mismatch');
    attachment = result.attachment ?? attachment;
  }
  if (!attachment || attachment.size !== file.size) throw new Error('Upload was not finalized');
  return attachment;
}

/** Validate the complete content before displaying bytes returned by local range reads. */
export async function readChatFile(request: Request, chat: string, file: ChatFile): Promise<Uint8Array> {
  if (file.size > MAX_BROWSER_PREVIEW_BYTES) throw new Error('Open this file locally to preview it');
  const bytes = new Uint8Array(file.size);
  for (let offset=0;offset<file.size;) {
    const result = await request({action:'read_attachment_chunk',chat_id:chat,upload_id:file.id,offset});
    if (!result.data_base64 || result.attachment?.sha256 !== file.sha256) throw new Error('Attachment content changed');
    const chunk = Uint8Array.from(atob(result.data_base64),c=>c.charCodeAt(0));
    if (!chunk.length || offset+chunk.length>file.size || result.next_offset !== offset+chunk.length) throw new Error('Invalid attachment range');
    bytes.set(chunk,offset);offset+=chunk.length;
  }
  const hash = [...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(b=>b.toString(16).padStart(2,'0')).join('');
  if (hash !== file.sha256) throw new Error('Attachment content changed');
  return bytes;
}
