import type { ChatFile, ChatMessage } from '../api/chat';
import { MAX_BROWSER_PREVIEW_BYTES } from './attachment-transfer';

export const IMAGE_GALLERY = Symbol('image-gallery');
export type PreviewImage = {
  workspaceId: string; folderId: string; chatId: string; path: string; name: string;
  file?: ChatFile;
};
export type ImageGallery = () => PreviewImage[];
export const imageKey = (image: PreviewImage) => JSON.stringify([image.workspaceId,image.folderId,image.chatId,image.path]);
export function attachmentImages(files: ChatFile[], workspaceId: string, folderId: string, chatId: string): PreviewImage[] {
  return files.filter(file=>file.mime.startsWith('image/') && file.size<=MAX_BROWSER_PREVIEW_BYTES)
    .map(file=>({workspaceId,folderId,chatId,path:file.path,name:file.label??file.name,file}));
}
/** Preserve display order and collapse repeated references to the same image. */
export function uniqueImages(images: PreviewImage[]): PreviewImage[] {
  const seen=new Set<string>();
  return images.filter(image=>{const key=imageKey(image);if(seen.has(key))return false;seen.add(key);return true;});
}
export function conversationImages(messages: ChatMessage[], workspaceId: string, folderId: string, chatId: string): PreviewImage[] {
  return uniqueImages(messages.flatMap(message=>{
    const images=attachmentImages(message.attachments??[],workspaceId,folderId,chatId);
    for(const match of message.text.matchAll(/mcp-assistant\/artifacts\/[^\s<>"`\])]+/g)) {
      const path=match[0].replace(/[.,;，。；]+$/,'');
      if(/\.(png|jpe?g|gif|webp)$/i.test(path))images.push({workspaceId,folderId,chatId,path,name:path.split('/').at(-1)??path});
    }
    return images;
  }));
}
