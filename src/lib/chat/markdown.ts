import { Lexer, type Token, type Tokens } from 'marked';
import { decodeHTML } from 'entities';

/** CommonMark + GitHub Flavored Markdown. Rendering uses Svelte text nodes only. */
export function chatMarkdownBlocks(text: string): Token[] {
  return Lexer.lex(text.replace(/\r\n?/g, '\n'), { gfm: true, breaks: true });
}
export const markdownText = (text: string) => decodeHTML(text);

/** Marked normalizes code text; keep the source trailing newline for fenced copy. */
export function markdownCode(token: Tokens.Code): string {
  const match = /^ {0,3}(`{3,}|~{3,})[^\n]*\n/.exec(token.raw);
  if (!match) return token.text;
  const body = token.raw.slice(match[0].length);
  const closing = new RegExp(`^ {0,3}${match[1]![0]}{${match[1]!.length},}[^\S\n]*(?:\n|$)`, 'm');
  const end = closing.exec(body);
  // Indented fences are normalized by the CommonMark tokenizer.
  const content = end ? body.slice(0, end.index) : body;
  const indent = /^ */.exec(token.raw)![0].length;
  return indent ? content.replace(new RegExp(`^ {0,${indent}}`, 'gm'), '') : content;
}
/** Resolve entities before scheme validation; raw HTML is never executable. */
export function markdownHref(value: string): string | null {
  const href = decodeHTML(value).trim();
  if (!/^https?:\/\//i.test(href) || /[\x00-\x20\x7f]/.test(href)) return null;
  try { return new URL(href).href; } catch { return null; }
}
