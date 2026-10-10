<script lang="ts">
  import RobotMention from './RobotMention.svelte';
  import {robotMentionParts} from '$lib/chat/robot-mentions';
  import AttachmentMention from './AttachmentMention.svelte';
  import { mentionParts } from '$lib/chat/mentions';
  import type { ChatFile } from '$lib/api/chat';
  import CodePreview from './CodePreview.svelte';
  import { chatMarkdownBlocks } from '$lib/chat/markdown';
  import { t } from '$lib/i18n';
  import ArtifactLink from './ArtifactLink.svelte';
  import LocalPathLink from './LocalPathLink.svelte';
  import {chatLinkParts} from '$lib/chat/local-links';
  let { text, attachments = [], workspaceId='', folderId='', chatId='', robotAliases={} }: { text:string;attachments?:ChatFile[];workspaceId?:string;folderId?:string;chatId?:string;robotAliases?:Record<string,string> } = $props();
  import { openExternal } from '$lib/api/native';
  /** The desktop webview cannot follow target=_blank; hand http(s) links (pages and downloads) to the system browser. */
  function openLink(event: MouseEvent, href: string) {
    if (event.type === 'auxclick' && event.button !== 1) return;
    event.preventDefault();
    openExternal(href).catch(error => {
      console.warn('open link failed', error);
      try { globalThis.open?.(href, '_blank', 'noopener,noreferrer'); } catch {}
    });
  }
  const blocks = $derived(chatMarkdownBlocks(text));
</script>
{#snippet inline(text:string,literal=false)}
  {#each chatLinkParts(text,literal) as token}{#if token.href}<a href={token.href} target="_blank" rel="noopener noreferrer" title={token.href} onclick={event=>openLink(event,token.href!)} onauxclick={event=>openLink(event,token.href!)}>{token.text}</a>{:else if token.path&&workspaceId&&folderId&&chatId}{#if token.image}<ArtifactLink {workspaceId} {folderId} {chatId} path={token.path} label={token.text}/>{:else}<LocalPathLink {workspaceId} {folderId} {chatId} path={token.path} label={token.text}/>{/if}{:else}{#each mentionParts(token.text,attachments) as part}{#if part.file}<AttachmentMention file={part.file} {workspaceId} {folderId} {chatId}/>{:else}{#each robotMentionParts(part.text,literal?{}:robotAliases) as robot}{#if robot.memberId}<RobotMention text={robot.text} memberId={robot.memberId}/>{:else}{robot.text}{/if}{/each}{/if}{/each}{/if}{/each}
{/snippet}
{#snippet prose(text:string)}
  {#each text.split(/(`[^`]+`|\*\*[^*]+\*\*)/g) as part}{#if part.startsWith('`') && part.endsWith('`')}<code class="inline">{@render inline(part.slice(1,-1),true)}</code>{:else if part.startsWith('**') && part.endsWith('**')}<strong>{@render inline(part.slice(2,-2))}</strong>{:else}{@render inline(part)}{/if}{/each}
{/snippet}
<div class="markdown">
  {#each blocks as block}
    {#if block.type === 'code'}<CodePreview language={block.language} code={block.code}/>
    {:else if block.type === 'table'}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users must be able to focus and scroll wide tables.) -->
      <div class="table-scroll" role="region" aria-label={$t('chat.markdownTable')} tabindex="0">
        <table>
          <thead><tr>{#each block.header as cell, column}<th scope="col" style:text-align={block.align[column]}>{@render prose(cell)}</th>{/each}</tr></thead>
          <tbody>{#each block.rows as row}<tr>{#each row as cell, column}<td style:text-align={block.align[column]}>{@render prose(cell)}</td>{/each}</tr>{/each}</tbody>
        </table>
      </div>
    {:else}
      {@const line = block.text}
      {#if /^#{1,3} /.test(line)}<h3>{@render prose(line.replace(/^#+ /,''))}</h3>
      {:else if /^[-*] /.test(line)}<div class="bullet"><span>•</span><span>{@render prose(line.slice(2))}</span></div>
      {:else}<div class="prose-line">{@render prose(line)}</div>{/if}
    {/if}
  {/each}
</div>
<style>.markdown a{color:var(--chat-link);text-decoration:underline;text-underline-offset:3px}.markdown{min-width:0;max-width:100%;font-size:14px;line-height:1.85;overflow-wrap:anywhere}.prose-line{white-space:pre-wrap;min-height:.55em;margin:0}.markdown h3{font-size:16px;font-weight:600;margin:18px 0 8px}.bullet{display:flex;gap:10px;margin:4px 0}.inline{background:var(--bg-main);padding:2px 5px;border-radius:4px;font-size:.9em}.table-scroll{max-width:100%;overflow-x:auto;margin:12px 0;border:1px solid var(--color-border);border-radius:8px}.table-scroll:focus-visible{outline:2px solid var(--primary);outline-offset:2px}table{width:100%;border-collapse:collapse;font-size:13px}th,td{min-width:6rem;padding:8px 12px;vertical-align:top;border-bottom:1px solid var(--color-border)}th{font-weight:600;background:var(--card-bg)}tbody tr:last-child td{border-bottom:0}</style>
