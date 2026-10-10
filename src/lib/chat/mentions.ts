import type { ChatFile } from '../api/chat';
/** Use the server's unique display label, even when clipboard filenames repeat. */
export function appendAttachmentMention(text: string, file: ChatFile): string {
  if (!file.label || referencedAttachments(text, [file]).length) return text;
  return text + (text && !/\s$/.test(text) ? ' ' : '') + '@' + file.label + ' ';
}
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

/** Draft attachments win by identity; history is opt-in in the composer. */
export function groupedMentionChoices(draft: ChatFile[], history: ChatFile[], query: string) {
  const current = attachmentCandidates(draft);
  const ids = new Set(current.map(file => file.id));
  return {
    current: mentionChoices(current, query),
    history: mentionChoices(history.filter(file => !ids.has(file.id)), query),
  };
}
