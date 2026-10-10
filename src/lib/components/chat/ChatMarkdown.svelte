<script lang="ts">
  import RobotMention from './RobotMention.svelte';
  import {robotMentionParts} from '$lib/chat/robot-mentions';
  import AttachmentMention from './AttachmentMention.svelte';
  import { mentionParts } from '$lib/chat/mentions';
  import type { ChatFile } from '$lib/api/chat';
  import CodePreview from './CodePreview.svelte';
  import { chatMarkdownBlocks, markdownCode, markdownText, markdownHref } from '$lib/chat/markdown';
  import type { Token, MarkedToken } from 'marked';
  import { localPathTarget } from '$lib/chat/local-links';
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
{#snippet renderTokens(tokens:Token[], inLink=false)}
  {#each tokens as entry}
    {@const token=entry as MarkedToken}
    {#if token.type === 'space' || token.type === 'def'}<!-- layout whitespace / reference definition -->
    {:else if token.type === 'code'}<CodePreview language={token.lang??''} code={markdownCode(token)}/>
    {:else if token.type === 'heading'}<svelte:element this={`h${token.depth}`}>{@render renderTokens(token.tokens)}</svelte:element>
    {:else if token.type === 'paragraph'}<p>{@render renderTokens(token.tokens)}</p>
    {:else if token.type === 'blockquote'}<blockquote>{@render renderTokens(token.tokens)}</blockquote>
    {:else if token.type === 'list'}
      <svelte:element this={token.ordered?'ol':'ul'} start={token.ordered?Number(token.start):undefined}>
        {#each token.items as item}<li class:task-item={item.task}>{#if item.task}<input type="checkbox" checked={item.checked} disabled aria-label={item.text}/>{/if}{@render renderTokens(item.tokens)}</li>{/each}
      </svelte:element>
    {:else if token.type === 'checkbox'}<!-- rendered beside the task item -->
    {:else if token.type === 'hr'}<hr/>
    {:else if token.type === 'br'}<br/>
    {:else if token.type === 'strong'}<strong>{@render renderTokens(token.tokens,inLink)}</strong>
    {:else if token.type === 'em'}<em>{@render renderTokens(token.tokens,inLink)}</em>
    {:else if token.type === 'del'}<del>{@render renderTokens(token.tokens,inLink)}</del>
    {:else if token.type === 'codespan'}<code class="inline">{#if inLink}{token.text}{:else}{@render inline(token.text,true)}{/if}</code>
    {:else if token.type === 'link' || token.type === 'image'}
      {@const href=markdownHref(token.href)}
      {@const path=localPathTarget(markdownText(token.href))}
      {#if href && !inLink}
        {#if token.type==='image'}<a {href} target="_blank" rel="noopener noreferrer" onclick={event=>openLink(event,href)}><img src={href} alt={markdownText(token.text)} title={token.title??undefined} loading="lazy" referrerpolicy="no-referrer"/></a>
        {:else}<a {href} title={token.title??href} target="_blank" rel="noopener noreferrer" onclick={event=>openLink(event,href)} onauxclick={event=>openLink(event,href)}>{@render renderTokens(token.tokens,true)}</a>{/if}
      {:else if path && workspaceId && folderId && chatId && !inLink}
        {#if token.type==='image'}<ArtifactLink {workspaceId} {folderId} {chatId} {path} label={markdownText(token.text)}/>{:else}<LocalPathLink {workspaceId} {folderId} {chatId} {path} label={markdownText(token.text)}/>{/if}
      {:else}{markdownText(token.text)}{/if}
    {:else if token.type === 'table'}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex (Wide tables need keyboard scrolling.) -->
      <div class="table-scroll" role="region" aria-label={$t('chat.markdownTable')} tabindex="0">
        <table><thead><tr>{#each token.header as cell, column}<th scope="col" style:text-align={token.align[column]??'left'}>{@render renderTokens(cell.tokens)}</th>{/each}</tr></thead>
          <tbody>{#each token.rows as row}<tr>{#each row as cell, column}<td style:text-align={token.align[column]??'left'}>{@render renderTokens(cell.tokens)}</td>{/each}</tr>{/each}</tbody>
        </table>
      </div>
    {:else if token.type === 'html'}<span class="raw-html">{token.raw}</span>
    {:else if 'tokens' in token && token.tokens}{@render renderTokens(token.tokens,inLink)}
    {:else if token.type === 'escape'}{token.text}
    {:else if inLink}{markdownText(token.text)}
    {:else}{@render inline(markdownText(token.text))}{/if}
  {/each}
{/snippet}
<div class="markdown">{@render renderTokens(blocks)}</div>
<style>
.markdown{min-width:0;max-width:100%;font-size:14px;line-height:1.8;overflow-wrap:anywhere}
.markdown a{color:var(--chat-link);text-decoration:underline;text-underline-offset:3px}
.markdown :global(p){margin:8px 0}.markdown :global(h1),.markdown :global(h2),.markdown :global(h3),.markdown :global(h4),.markdown :global(h5),.markdown :global(h6){font-weight:650;line-height:1.4;margin:20px 0 8px}
.markdown :global(h1){font-size:1.65em}.markdown :global(h2){font-size:1.4em}.markdown :global(h3){font-size:1.2em}.markdown :global(h4),.markdown :global(h5),.markdown :global(h6){font-size:1em}
.markdown :global(ul),.markdown :global(ol){padding-left:1.8em;margin:8px 0;list-style:revert}.markdown :global(li){padding-left:.2em;margin:3px 0}.markdown :global(li>p){margin:4px 0}.markdown .task-item{list-style:none;position:relative}.task-item>input{margin-left:-1.5em;margin-right:.5em;accent-color:var(--primary)}
blockquote{margin:12px 0;padding:2px 14px;border-left:3px solid var(--color-border);color:var(--color-text-muted)}hr{border:0;border-top:1px solid var(--color-border);margin:20px 0}
.inline{background:var(--bg-main);padding:2px 5px;border-radius:4px;font-size:.9em;white-space:break-spaces}.raw-html{white-space:pre-wrap}.markdown img{display:block;max-width:100%;max-height:65vh;object-fit:contain;border-radius:8px}
.table-scroll{max-width:100%;overflow-x:auto;margin:12px 0;border:1px solid var(--color-border);border-radius:8px}.table-scroll:focus-visible{outline:2px solid var(--primary);outline-offset:2px}table{width:100%;border-collapse:collapse;font-size:13px}th,td{min-width:6rem;padding:8px 12px;vertical-align:top;border-bottom:1px solid var(--color-border)}th{font-weight:600;background:var(--card-bg)}tbody tr:last-child td{border-bottom:0}
</style>
