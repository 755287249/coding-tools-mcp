import type { ChatFile } from '../api/chat';
export function attachmentCandidates(files: ChatFile[]): ChatFile[] {
  return [...new Map(files.filter(file=>file.label).map(file=>[file.id,file])).values()];
}
export function mentionQuery(text: string, caret: number): {start:number; end:number; query:string} | null {
  const match = /@([^\s@]*)$/.exec(text.slice(0,caret));
  return match ? {start:caret-match[0].length,end:caret,query:match[1]} : null;
}
export function mentionChoices(files: ChatFile[], query: string): ChatFile[] {
  const normalized=query.toLocaleLowerCase();
  return attachmentCandidates(files).filter(file=>`${file.label} ${file.name}`.toLocaleLowerCase().includes(normalized));
}
export function mentionParts(text: string, files: ChatFile[]) {
  const byLabel = new Map(attachmentCandidates(files).map(file=>[file.label,file]));
  return text.split(/(@(?:图片|文件)[1-9]\d*)/g).filter(Boolean).map(text=>({text,file:byLabel.get(text.slice(1))}));
}
export function referencedAttachments(text: string, files: ChatFile[]): ChatFile[] {
  return attachmentCandidates(mentionParts(text,files).flatMap(part=>part.file?[part.file]:[]));
}
