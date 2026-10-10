<script lang="ts">
  import RobotMention from './RobotMention.svelte';
  import {robotMentionParts} from '$lib/chat/robot-mentions';
  import AttachmentMention from './AttachmentMention.svelte';
  import { mentionParts } from '$lib/chat/mentions';
  import type { ChatFile } from '$lib/api/chat';
  import CodePreview from './CodePreview.svelte';
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
  const blocks = $derived(text.split(/(```[^\n]*\n[\s\S]*?```)/g).filter(Boolean));
</script>
{#snippet inline(text:string,literal=false)}
  {#each chatLinkParts(text,literal) as token}{#if token.href}<a href={token.href} target="_blank" rel="noopener noreferrer" title={token.href} onclick={event=>openLink(event,token.href!)} onauxclick={event=>openLink(event,token.href!)}>{token.text}</a>{:else if token.path&&workspaceId&&folderId&&chatId}{#if token.image}<ArtifactLink {workspaceId} {folderId} {chatId} path={token.path} label={token.text}/>{:else}<LocalPathLink {workspaceId} {folderId} {chatId} path={token.path} label={token.text}/>{/if}{:else}{#each mentionParts(token.text,attachments) as part}{#if part.file}<AttachmentMention file={part.file} {workspaceId} {folderId} {chatId}/>{:else}{#each robotMentionParts(part.text,literal?{}:robotAliases) as robot}{#if robot.memberId}<RobotMention text={robot.text} memberId={robot.memberId}/>{:else}{robot.text}{/if}{/each}{/if}{/each}{/if}{/each}
{/snippet}
{#snippet prose(text:string)}
  {#each text.split(/(`[^`]+`|\*\*[^*]+\*\*)/g) as part}{#if part.startsWith('`') && part.endsWith('`')}<code class="inline">{@render inline(part.slice(1,-1),true)}</code>{:else if part.startsWith('**') && part.endsWith('**')}<strong>{@render inline(part.slice(2,-2))}</strong>{:else}{@render inline(part)}{/if}{/each}
{/snippet}
<div class="markdown">
  {#each blocks as block}
    {#if block.startsWith('```')}<CodePreview language={block.slice(3,block.indexOf('\n'))} code={block.slice(block.indexOf('\n')+1,-3)}/>
    {:else}{#each block.split('\n') as line}
      {#if /^#{1,3} /.test(line)}<h3>{@render prose(line.replace(/^#+ /,''))}</h3>
      {:else if /^[-*] /.test(line)}<div class="bullet"><span>•</span><span>{@render prose(line.slice(2))}</span></div>
      {:else}<div class="prose-line">{@render prose(line)}</div>{/if}
    {/each}{/if}
  {/each}
</div>
<style>.markdown a{color:var(--chat-link);text-decoration:underline;text-underline-offset:3px}.markdown{font-size:14px;line-height:1.85;overflow-wrap:anywhere}.prose-line{white-space:pre-wrap;min-height:.55em;margin:0}.markdown h3{font-size:16px;font-weight:600;margin:18px 0 8px}.bullet{display:flex;gap:10px;margin:4px 0}.inline{background:var(--bg-main);padding:2px 5px;border-radius:4px;font-size:.9em}</style>
