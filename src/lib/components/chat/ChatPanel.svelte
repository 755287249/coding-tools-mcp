<script lang="ts">
  import { t, locale } from '$lib/i18n';
  import { tick } from 'svelte';
  import Plus from '@lucide/svelte/icons/plus';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import MessageSquare from '@lucide/svelte/icons/message-square';
  import Copy from '@lucide/svelte/icons/copy';
  import Download from '@lucide/svelte/icons/download';
  import Square from '@lucide/svelte/icons/square';
  import Check from '@lucide/svelte/icons/check';
  import ChatMarkdown from './ChatMarkdown.svelte';
  import ChatAttachment from './ChatAttachment.svelte';
  import { localChat, type ChatSession, type ChatFile } from '$lib/api/chat';
  import { getBackend, loadMcpAuthSecrets } from '$lib/backend';
  import { buildConnectionPrompt } from '$lib/connect/prompt';
  import { buildChatPrompt } from '$lib/connect/chat-prompt';
  import type { AuthConfig, WorkspaceFolder } from '$lib/types';
  let { workspaceId, folders, activeFolderId = '', endpoint = '', auth }: { workspaceId: string; folders: WorkspaceFolder[]; activeFolderId?: string; endpoint?: string; auth: AuthConfig } = $props();
  let folderId = $state('');
  let sessions = $state<ChatSession[]>([]);
  let selected = $state('');
  let detail = $state<ChatSession | null>(null);
  let draft = $state('');
  let error = $state('');
  let busy = $state(false);
  let copied = $state(false);
  let guide = $state(false);
  let feed: HTMLDivElement;
  let fileInput: HTMLInputElement;
  let attachments = $state<ChatFile[]>([]);
  let generation = 0;
  let retry: { text: string; id: string; attachmentKey: string } | null = null;
  const selectedFolder = $derived(folders.find(f => f.id === folderId));
  const messages = $derived(detail?.messages ?? []);
  const statusLabel = $derived(detail?.status === 'waiting' ? $t('chat.19') : detail?.status === 'connected' ? $t('chat.20') : detail?.status === 'closed' ? $t('chat.21') : $t('chat.22'));
  const bootstrap = $derived(buildChatPrompt(selected, folderId, endpoint ? `MCP URL: ${endpoint}` : ''));
  async function refresh(ws: string, folder: string, gen: number) {
    const list = await localChat(ws, folder, { action: 'list' });
    if (gen !== generation) return;
    sessions = list.sessions ?? [];
    if (!selected && sessions.length) selected = sessions[0].id;
    const target = selected;
    if (target) {
      const result = await localChat(ws, folder, { action: 'read', chat_id: target });
      if (gen !== generation || selected !== target) return;
      const nearBottom = !feed || feed.scrollHeight - feed.scrollTop - feed.clientHeight < 120;
      const changed = detail?.updated_at !== result.session?.updated_at || detail?.id !== target;
      detail = result.session ?? null;
      if (error.includes('Chat storage is busy')) error = '';
      if (changed && nearBottom) { await tick(); if (feed) feed.scrollTop = feed.scrollHeight; }
    }
  }
  $effect(() => {
    if (!folders.some(f => f.id === folderId)) folderId = folders.find(f => f.id === activeFolderId)?.id ?? folders[0]?.id ?? '';
  });
  $effect(() => {
    const ws = workspaceId, folder = folderId; const gen = ++generation;
    sessions = []; selected = ''; detail = null; draft = ''; error = ''; retry = null; attachments = [];
    let stopped = false; let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      if (!folder || stopped) return;
      try { await refresh(ws, folder, gen); } catch (e) { if (gen === generation) error = String(e); }
      if (!stopped) timer = setTimeout(poll, 1500);
    }
    void poll();
    return () => { stopped = true; clearTimeout(timer); generation++; };
  });
  async function choose(s: ChatSession) {
    selected = s.id; detail = null; draft = ''; retry = null; attachments = []; guide = false; error = '';
    const gen = generation;
    try { await refresh(workspaceId, folderId, gen); } catch(e) { if (gen === generation) error = String(e); }
  }
  async function create() {
    if (busy || !folderId) return; busy = true; error = ''; const gen = generation;
    try {
      const result = await localChat(workspaceId, folderId, { action: 'create', title: '新对话' });
      if (gen !== generation) return;
      detail = result.session ?? null; selected = detail?.id ?? ''; draft = ''; retry = null; attachments = []; guide = true;
      await refresh(workspaceId, folderId, gen);
    } catch(e) { if (gen === generation) error = String(e); } finally { busy = false; }
  }
  async function send() {
    if (busy || (!draft.trim() && !attachments.length) || !selected || !detail || detail.closed) return;
    busy = true; error = ''; const gen = generation; const target = selected; const content = draft.trim();
    const attachmentKey = attachments.map(f => f.id).join(',');
    if (!retry || retry.text !== content || retry.attachmentKey !== attachmentKey) retry = { text: content, id: crypto.randomUUID(), attachmentKey };
    try {
      const result = await localChat(workspaceId, folderId, { action: 'send', chat_id: target, message_id: retry.id, text: content, attachment_ids: attachments.map(f => f.id) });
      if (gen !== generation || selected !== target) return;
      detail = result.session ?? null; draft = ''; retry = null; attachments = []; guide = false;
      await tick(); if (feed) feed.scrollTop = feed.scrollHeight;
    } catch(e) { if (gen === generation) error = String(e); } finally { busy = false; }
  }
  async function uploadFiles(files: FileList | null) {
    if (!files?.length || busy || !selected || !detail || detail.closed) return;
    if (attachments.length + files.length > 5 || [...files].some(f => !f.size || f.size > 2 * 1024 * 1024)) { error = $t('chat.51'); return; }
    busy = true; error = ''; const gen = generation, target = selected, folder = folderId;
    try {
      for (const file of [...files]) {
        const data = await new Promise<string>((resolve, reject) => {
          const reader = new FileReader(); reader.onerror = () => reject(new Error($t('chat.57')));
          reader.onload = () => resolve(String(reader.result).split(',')[1]); reader.readAsDataURL(file);
        });
        const result = await localChat(workspaceId, folder, {action:'upload', chat_id:target, upload_id:crypto.randomUUID(), name:file.name, data_base64:data});
        if (gen !== generation || target !== selected) return;
        if (result.attachment) attachments = [...attachments, result.attachment];
      }
    } catch(e) { if (gen === generation) error = String(e); } finally { busy = false; if (fileInput) fileInput.value = ''; }
  }
  async function close() {
    if (busy || !selected) return; busy = true; error = ''; const gen = generation, target = selected;
    try { const result = await localChat(workspaceId, folderId, { action: 'close', chat_id: target }); if (gen === generation && target === selected) detail = result.session ?? null; }
    catch(e) { if (gen === generation) error = String(e); } finally { busy = false; }
  }
  async function copyPrompt() {
    if (!selected || !detail || detail.closed) return;
    try {
      const target = selected, folder = folderId;
      const loaded = endpoint ? await loadMcpAuthSecrets(getBackend(), workspaceId, auth) : null;
      if (target !== selected || folder !== folderId) return;
      const connection = loaded ? buildConnectionPrompt({ workspaceName: selectedFolder?.name ?? '', endpoint, authType: auth.type, clientId: loaded.oauth_client_id || auth.oauth_client_id, password: loaded.oauth_password ?? '', bearerToken: loaded.bearer_token ?? '', folders: folders.map(f => f.path) }, $locale) : '';
      await navigator.clipboard.writeText(buildChatPrompt(target, folder, connection)); copied = true;
    } catch { guide = true; error = $t('chat.42'); }
  }
  function download() {
    if (!detail) return;
    const text = `# ${detail.title}\n\nSession: ${detail.id}\n\n` + messages.map(m => `## ${m.role === 'user' ? $t('chat.26') : 'AI'} · ${new Date(m.created_at).toLocaleString()}\n\n${m.tool_event ? `Tool (AI reported): ${m.tool_event.name} · ${m.tool_event.status}\n\n` : ''}${m.text}\n${(m.attachments ?? []).map(f => `\nAttachment: ${f.name} (${f.size} bytes)\nPath: ${f.path}\n`).join('')}`).join('\n');
    const url = URL.createObjectURL(new Blob([text], {type:'text/markdown;charset=utf-8'}));
    const a = document.createElement('a'); a.href=url; a.download=`chat-${detail.id}.md`; a.click(); setTimeout(() => URL.revokeObjectURL(url),1000);
  }
</script>

<section class="chat-shell" aria-label={$t("chat.35")}>
  <aside class="session-sidebar">
    <div class="sidebar-heading"><span>{$t("chat.0")}</span><span class="beta">{$t("chat.48")}</span></div>
    <button class="new-chat" onclick={create} disabled={busy || !folderId}><Plus size={16}/>{$t("chat.1")}</button>
    <label class="folder-label" for="chat-folder">{$t("chat.2")}</label>
    <select id="chat-folder" aria-label={$t("chat.2")} bind:value={folderId} disabled={busy}>{#each folders as folder}<option value={folder.id}>{folder.name}</option>{/each}</select>
    <div class="section-label">{$t("chat.3")}</div>
    <nav aria-label="聊天会话">{#each sessions as s}<button class:active={selected === s.id} onclick={() => choose(s)} disabled={busy}><MessageSquare size={15}/><span>{s.messages?.[0]?.text || s.title}</span>{#if s.closed}<span class="ended">{$t("chat.4")}</span>{/if}</button>{/each}</nav>
    <div class="local-note"><span class="dot"></span>{$t("chat.5")}</div>
  </aside>
  <div class="conversation">
    <header class="chat-header"><div><strong>{selectedFolder?.name ?? $t('chat.23')}</strong><span class="status"><i class:online={detail?.status === 'waiting'}></i>{statusLabel}</span></div><div class="header-actions">{#if selected}<button title={$t("chat.36")} aria-label={$t("chat.36")} onclick={download}><Download size={16}/></button><button title={$t("chat.37")} onclick={() => guide = !guide}>{$t("chat.6")}</button>{/if}</div></header>
    {#if error}<div class="chat-error" role="alert">{error}<button onclick={() => error = ''} aria-label={$t("chat.38")}>×</button></div>{/if}
    {#if guide && selected}<div class="guide"><div><strong>{$t("chat.7")}</strong><button onclick={copyPrompt} disabled={!detail || detail.closed}>{#if copied}<Check size={14}/>{:else}<Copy size={14}/>{/if}{copied ? $t('chat.25') : $t('chat.24')}</button></div><p>{$t("chat.8")}</p><details><summary>{$t("chat.9")}</summary><pre>{bootstrap}</pre></details></div>{/if}
    <div class="message-feed" bind:this={feed}>
      {#if !messages.length}<div class="empty-state"><div class="empty-icon"><MessageSquare size={25} strokeWidth={1.4}/></div><p class="overline">{$t("chat.47")}</p><h2>{$t("chat.10")}</h2><p>{$t("chat.11")}<br/>{$t("chat.12")}</p>{#if !selected}<button class="start-button" onclick={create} disabled={busy || !folderId}><Plus size={15}/>{$t("chat.13")}</button>{:else}<button class="start-button" onclick={copyPrompt} disabled={!detail || detail.closed}><Copy size={15}/>{$t("chat.14")}</button>{/if}<div class="suggestions"><button onclick={() => draft = '先阅读项目结构，告诉我你发现了什么。'}>{$t("chat.15")}<span>↗</span></button><button onclick={() => draft = '帮我制定一个实现计划，先不要修改文件。'}>{$t("chat.16")}<span>↗</span></button></div></div>
      {:else}<div class="message-column">{#each messages as m (m.id)}<article class:user={m.role === 'user'} class:assistant={m.role === 'assistant'}><div class="message-meta">{m.role === 'user' ? $t('chat.26') : m.final === false ? $t('chat.27') : 'AI'}<time>{new Date(m.created_at).toLocaleTimeString([], {hour:'2-digit',minute:'2-digit'})}</time></div><div class="message-body">{#if m.tool_event}<details class="tool-event"><summary>{$t('chat.54')} · {m.tool_event.name} · {m.tool_event.status === 'running' ? $t('chat.55') : m.tool_event.status === 'completed' ? $t('chat.56') : $t('chat.58')}</summary><ChatMarkdown text={m.text}/></details>{:else}<ChatMarkdown text={m.text}/>{/if}{#each m.attachments ?? [] as file (file.id)}<ChatAttachment {workspaceId} {folderId} chatId={selected} {file}/>{/each}</div></article>{/each}{#if messages.at(-1)?.role === 'user'}<div class="pending"><span></span>{detail?.status === 'offline' ? $t('chat.28') : $t('chat.29')}</div>{/if}</div>{/if}
    </div>
    <footer class="composer-area"><form onsubmit={(e) => { e.preventDefault(); void send(); }}><textarea aria-label={$t("chat.39")} bind:value={draft} placeholder={detail?.closed ? $t('chat.30') : selected ? $t('chat.31') : $t('chat.32')} disabled={!selected || !detail || detail.closed || busy} onkeydown={(e) => { if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); void send(); } }} rows="3" maxlength="32000"></textarea><input class="file-input" type="file" multiple bind:this={fileInput} onchange={() => uploadFiles(fileInput.files)} aria-label={$t('chat.50')}/>{#if attachments.length}<div class="draft-files">{#each attachments as file (file.id)}<span>{file.name}<button type="button" disabled={busy} aria-label={$t('chat.53')} onclick={() => attachments = attachments.filter(f => f.id !== file.id)}>×</button></span>{/each}</div>{/if}<div class="composer-toolbar"><span><span class="dot"></span>{selectedFolder?.name ?? $t('chat.33')}</span><div><button type="button" disabled={busy || !selected || detail?.closed} title={$t('chat.51')} onclick={() => fileInput.click()}>{$t('chat.50')}</button>{#if selected && !detail?.closed}<button type="button" class="end-button" onclick={close} disabled={busy} title={$t("chat.40")}><Square size={12}/>{$t("chat.17")}</button>{/if}<button class="send-button" aria-label={$t("chat.41")} disabled={busy || (!draft.trim() && !attachments.length) || !selected || detail?.closed}><ArrowUp size={18}/></button></div></div></form><div class="composer-hint">{$t("chat.18")} <span>{detail?.archive_path ?? $t('chat.34')}</span></div></footer>
  </div>
</section>
<style>
.file-input{display:none}.draft-files{display:flex;flex-wrap:wrap;gap:8px;font-size:11px}.draft-files span{border:1px solid var(--color-border);padding:5px 8px;border-radius:6px;overflow-wrap:anywhere}.draft-files button{margin-left:8px}.tool-event{border:1px solid var(--color-border);border-radius:9px;padding:10px;font-size:12px}.tool-event summary{cursor:pointer;color:var(--color-text-muted)}

.chat-shell{height:max(640px,calc(100dvh - 150px));max-height:1100px;display:grid;grid-template-columns:218px minmax(0,1fr);border:1px solid var(--color-border);border-radius:16px;overflow:hidden;background:var(--card-bg);color:var(--color-text)}button{cursor:pointer}button:disabled{cursor:default;opacity:.45}button:focus-visible,select:focus-visible,textarea:focus-visible{outline:2px solid var(--primary);outline-offset:3px}.session-sidebar{padding:22px 14px 14px;background:var(--color-bg);display:flex;flex-direction:column;border-right:1px solid var(--color-border);min-height:0}.sidebar-heading{display:flex;align-items:center;justify-content:space-between;font-size:13px;font-weight:600;padding:0 9px 20px}.beta{font:9px monospace;letter-spacing:1px;color:var(--color-text-muted);border:1px solid var(--color-border);padding:3px 5px;border-radius:4px}.new-chat{display:flex;gap:9px;align-items:center;border:1px solid var(--color-border);border-radius:9px;padding:10px 12px;background:var(--card-bg);font-size:12px}.folder-label,.section-label{font-size:10px;color:var(--color-text-muted);margin:22px 8px 8px;display:block}.section-label{margin-top:28px}select{font-size:11px;width:100%;padding:9px;border:1px solid var(--color-border);border-radius:7px;background:var(--card-bg)}nav{overflow:auto;flex:1}nav button{display:flex;align-items:center;gap:8px;width:100%;text-align:left;padding:11px 9px;margin:3px 0;border-radius:7px;font-size:12px;color:var(--color-text-muted)}nav button.active{background:var(--card-bg);color:var(--color-text);box-shadow:0 1px 4px #00000008}nav button span:first-of-type{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.ended{font-size:9px;margin-left:auto;white-space:nowrap}.local-note{font-size:10px;color:var(--color-text-muted);padding:20px 6px 3px;display:flex;gap:7px;align-items:center}.dot{display:inline-block;width:5px;height:5px;border-radius:50%;background:#83a18e}.conversation{display:flex;flex-direction:column;min-width:0;min-height:0}.chat-header{padding:19px 26px;display:flex;justify-content:space-between;align-items:center;border-bottom:1px solid var(--color-border);gap:12px}.chat-header>div:first-child{display:flex;align-items:center;gap:17px;flex-wrap:wrap}.chat-header strong{font-size:13px;font-weight:550}.status{font-size:10px;color:var(--color-text-muted);display:flex;align-items:center;gap:6px}.status i{width:6px;height:6px;border-radius:50%;background:#989b98}.status i.online{background:#49a476;box-shadow:0 0 0 3px #49a47618}.header-actions{display:flex;gap:14px;align-items:center;font-size:11px;white-space:nowrap;color:var(--color-text-muted)}.message-feed{flex:1;overflow-y:auto;min-height:0;scroll-behavior:smooth}.empty-state{max-width:600px;margin:0 auto;padding:65px 25px 40px;text-align:center}.empty-icon{display:grid;place-items:center;width:54px;height:54px;border:1px solid var(--color-border);border-radius:17px;margin:0 auto 23px}.overline{font:9px monospace!important;letter-spacing:2.5px;color:var(--color-text-muted)}.empty-state h2{font-weight:500;font-size:30px;letter-spacing:-1px;margin:15px 0}.empty-state p{font-size:12px;line-height:1.9;color:var(--color-text-muted)}.start-button{display:inline-flex;align-items:center;gap:8px;margin:23px 0;padding:9px 15px;border:1px solid var(--color-border);border-radius:8px;font-size:11px}.suggestions{display:flex;gap:10px;margin:15px auto 0;max-width:430px}.suggestions button{border:1px solid var(--color-border);padding:15px 13px;border-radius:10px;text-align:left;flex:1;font-size:11px}.suggestions span{float:right;color:var(--color-text-muted)}.message-column{max-width:800px;margin:auto;padding:25px 40px 30px}article{margin-bottom:26px}.message-meta{display:flex;align-items:center;gap:10px;font-size:11px;font-weight:600;margin-bottom:9px}.message-meta time{font-size:9px;color:var(--color-text-muted);font-weight:400}.user{margin-left:14%}.user .message-meta{justify-content:flex-end}.user .message-body{background:var(--color-bg);border:1px solid var(--color-border);padding:12px 17px;border-radius:14px 14px 3px 14px}.assistant .message-body{padding:3px 0}.pending{font-size:11px;color:var(--color-text-muted);display:flex;align-items:center;gap:8px}.pending span{height:6px;width:6px;border-radius:50%;background:var(--color-text-muted)}.composer-area{padding:12px 30px 17px}.composer-area form{max-width:760px;margin:0 auto;border:1px solid var(--color-border);border-radius:17px;padding:14px 16px 10px;box-shadow:0 3px 15px #00000004}.composer-area textarea{display:block;resize:none;width:100%;background:transparent;border:0;font:13px/1.7 inherit;color:var(--color-text);outline:none;min-height:62px}.composer-toolbar{display:flex;justify-content:space-between;align-items:center;margin-top:7px;font-size:10px;color:var(--color-text-muted)}.composer-toolbar>span{display:flex;align-items:center;gap:7px}.composer-toolbar>div{display:flex;gap:14px;align-items:center}.send-button{display:grid;place-items:center;width:31px;height:31px;border-radius:50%;background:var(--color-text);color:var(--card-bg)}.end-button{display:flex;gap:5px;align-items:center;font-size:10px}.composer-hint{max-width:760px;margin:10px auto 0;display:flex;justify-content:space-between;gap:10px;font-size:9px;color:var(--color-text-muted)}.composer-hint span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:50%}.guide{margin:15px 25px 0;padding:14px 16px;border:1px solid var(--color-border);border-radius:10px;font-size:11px;background:var(--color-bg)}.guide>div{display:flex;justify-content:space-between;gap:10px;align-items:center}.guide button{display:flex;gap:6px;align-items:center;white-space:nowrap}.guide p{color:var(--color-text-muted);margin:8px 0;line-height:1.7}.guide pre{white-space:pre-wrap;overflow-wrap:anywhere;font-size:10px;line-height:1.8;max-height:130px;overflow:auto;margin-top:8px}.guide summary{cursor:pointer;font-size:10px}.chat-error{display:flex;justify-content:space-between;gap:10px;margin:12px 25px 0;padding:10px;border:1px solid var(--danger);color:var(--danger);border-radius:8px;font-size:11px;overflow-wrap:anywhere}@media(max-width:800px){.chat-shell{grid-template-columns:160px minmax(0,1fr)}.message-column{padding:20px}.chat-header{padding:15px}.composer-area{padding:10px 15px}.empty-state{padding-top:45px}.suggestions{flex-direction:column}}@media(max-width:560px){:global(.sx-layout:has(.chat-shell) > .sx-side){display:none}:global(.sx-layout:has(.chat-shell)){grid-template-columns:minmax(0,1fr)}.chat-shell{grid-template-columns:1fr;height:calc(100dvh - 140px);min-height:600px}.session-sidebar{padding:10px;display:grid;grid-template-columns:1fr 1fr;gap:8px;border-right:0;border-bottom:1px solid var(--color-border)}.sidebar-heading,.folder-label,.section-label,.local-note{display:none}nav{grid-column:1/-1;display:flex;max-height:40px;gap:6px}nav button{width:auto;min-width:100px}.new-chat{padding:7px 10px}.chat-header{padding:12px}.empty-state{padding:25px 20px}.empty-state h2{font-size:24px}.guide{margin:10px}.composer-hint span{display:none}.message-column{padding:15px}}
</style>
