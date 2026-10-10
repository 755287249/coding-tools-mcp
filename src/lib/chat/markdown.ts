export type TableAlignment = 'left' | 'center' | 'right';
export type ChatMarkdownBlock =
  | { type: 'line'; text: string }
  | { type: 'code'; language: string; code: string }
  | { type: 'table'; header: string[]; align: TableAlignment[]; rows: string[][] };

/** Keep escaped pipes and matched inline-code spans inside their cells. */
function tableRow(line: string): { cells: string[]; hasPipe: boolean } {
  const text = line.trim();
  const cells: string[] = [];
  let cell = '', firstPipe = -1, lastPipe = -1;
  for (let i = 0; i < text.length; i++) {
    const char = text[i]!;
    if (char === '\\' && i + 1 < text.length) {
      const next = text[++i]!;
      cell += next === '|' ? '|' : '\\' + next;
    } else if (char === '`') {
      let end = i + 1;
      while (text[end] === '`') end++;
      const fence = text.slice(i, end);
      let close = text.indexOf(fence, end);
      while (close >= 0 && (text[close - 1] === '`' || text[close + fence.length] === '`')) {
        close = text.indexOf(fence, close + fence.length);
      }
      if (close >= 0) {
        const span = text.slice(i, close + fence.length);
        cell += span.replace(/\\\|/g, '|');
        i = close + fence.length - 1;
      } else {
        cell += fence;
        i = end - 1;
      }
    } else if (char === '|') {
      if (firstPipe < 0) firstPipe = i;
      lastPipe = i;
      cells.push(cell.trim());
      cell = '';
    } else cell += char;
  }
  cells.push(cell.trim());
  if (firstPipe === 0) cells.shift();
  if (lastPipe === text.length - 1 && lastPipe >= 0) cells.pop();
  return { cells, hasPipe: firstPipe >= 0 };
}

/** Parse only supported blocks; cell/line content stays text for Svelte escaping. */
export function chatMarkdownBlocks(text: string): ChatMarkdownBlock[] {
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  const blocks: ChatMarkdownBlock[] = [];
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!;
    const fence = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (fence && !(fence[1]![0] === '`' && fence[2]!.includes('`'))) {
      const marker = fence[1]!;
      const closing = new RegExp(`^ {0,3}${marker[0]}{${marker.length},}\\s*$`);
      let end = i + 1;
      while (end < lines.length && !closing.test(lines[end]!)) end++;
      const code = lines.slice(i + 1, end).join('\n') + (end < lines.length && end > i + 1 ? '\n' : '');
      blocks.push({ type: 'code', language: fence[2]!.trim(), code });
      i = end;
      continue;
    }
    const header = tableRow(line);
    const delimiter = i + 1 < lines.length ? tableRow(lines[i + 1]!) : null;
    if (header.hasPipe && header.cells.length > 0 && delimiter
      && delimiter.cells.length === header.cells.length
      && delimiter.cells.every(cell => /^:?-+:?$/.test(cell))) {
      const align: TableAlignment[] = delimiter.cells.map(cell => cell.endsWith(':') ? cell.startsWith(':') ? 'center' : 'right' : 'left');
      const rows: string[][] = [];
      i += 2;
      for (; i < lines.length; i++) {
        // A fence always starts a code block, even if its info string has pipes.
        if (/^ {0,3}(`{3,}|~{3,})/.test(lines[i]!)) break;
        const row = tableRow(lines[i]!);
        if (!row.hasPipe) break;
        rows.push(header.cells.map((_, column) => row.cells[column] ?? ''));
      }
      i--;
      blocks.push({ type: 'table', header: header.cells, align, rows });
    } else blocks.push({ type: 'line', text: line });
  }
  return blocks;
}
