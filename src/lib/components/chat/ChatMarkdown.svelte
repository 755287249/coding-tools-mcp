<script lang="ts">
  import CodePreview from "./CodePreview.svelte";
  let { text }: { text: string } = $props();
  const blocks = $derived(text.split(/(```[^\n]*\n[\s\S]*?```)/g).filter(Boolean));
</script>
<div class="markdown">
  {#each blocks as block}
    {#if block.startsWith('```')}
      <CodePreview language={block.slice(3, block.indexOf('\n'))} code={block.slice(block.indexOf('\n') + 1, -3)}/>
    {:else}
      {#each block.split('\n') as line}
        {#if /^#{1,3} /.test(line)}<h3>{line.replace(/^#+ /, '')}</h3>
        {:else if /^[-*] /.test(line)}<div class="bullet"><span>•</span><span>{line.slice(2)}</span></div>
        {:else}<p>{#each line.split(/(`[^`]+`|\*\*[^*]+\*\*)/g) as part}{#if part.startsWith('`') && part.endsWith('`')}<code class="inline">{part.slice(1,-1)}</code>{:else if part.startsWith('**') && part.endsWith('**')}<strong>{part.slice(2,-2)}</strong>{:else}{part}{/if}{/each}</p>{/if}
      {/each}
    {/if}
  {/each}
</div>
<style>
.markdown{font-size:14px;line-height:1.85;overflow-wrap:anywhere}.markdown p{white-space:pre-wrap;min-height:.55em;margin:0}.markdown h3{font-size:16px;font-weight:600;margin:18px 0 8px}.bullet{display:flex;gap:10px;margin:4px 0}.inline{background:var(--bg-main);padding:2px 5px;border-radius:4px;font-size:.9em}
</style>
