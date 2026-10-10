/** Match the same complete, case-insensitive #aliases as collaboration routing. */
export function robotMentionParts(text: string, aliases: Record<string, string> = {}) {
  const ids = new Map(Object.entries(aliases).map(([id, alias]) => [alias.toLowerCase(), id]));
  const parts: {text: string; memberId?: string}[] = [];
  let offset = 0;
  for (const match of text.matchAll(/#[\p{L}\p{N}_-]+/gu)) {
    const memberId = ids.get(match[0].slice(1).toLowerCase());
    if (!memberId) continue;
    if (match.index > offset) parts.push({text: text.slice(offset, match.index)});
    parts.push({text: match[0], memberId});
    offset = match.index + match[0].length;
  }
  if (offset < text.length) parts.push({text: text.slice(offset)});
  return parts;
}
