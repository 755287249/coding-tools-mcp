<script lang="ts">
  import { t, locale } from '$lib/i18n';
  import { tick, untrack, onDestroy } from 'svelte';
  import Plus from '@lucide/svelte/icons/plus';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import MessageSquare from '@lucide/svelte/icons/message-square';
  import Copy from '@lucide/svelte/icons/copy';
  import Download from '@lucide/svelte/icons/download';
  import Square from '@lucide/svelte/icons/square';
  import Check from '@lucide/svelte/icons/check';
  import Pencil from '@lucide/svelte/icons/pencil';
  import X from '@lucide/svelte/icons/x';
  import ChatToolActivity from './ChatToolActivity.svelte';
  import { chatDrafts, chatDraftKey, conversationAccent } from '$lib/chat/drafts';
  import { groupChatMessages } from '$lib/chat/tool-activity';
  import ChatMarkdown from './ChatMarkdown.svelte';
  import ChatReplyState from './ChatReplyState.svelte';
  import ChatUserState from './ChatUserState.svelte';
  import ChatStatusIcon from './ChatStatusIcon.svelte';
  import ChatAttachment from './ChatAttachment.svelte';
  import { localChat, type ChatSession, type ChatFile } from '$lib/api/chat';
  import { getBackend, loadMcpAuthSecrets } from '$lib/backend';
  import { buildConnectionPrompt } from '$lib/connect/prompt';
  import { buildChatPrompt } from '$lib/connect/chat-prompt';
  import { pendingChatState, chatReplyState, chatUserState, USER_MESSAGE_STATUS_KEYS } from '$lib/chat/status';
  import type { AuthConfig, WorkspaceFolder } from '$lib/types';
  let { workspaceId, folders, activeFolderId = '', endpoint = '', auth }: { workspaceId: string; folders: WorkspaceFolder[]; activeFolderId?: string; endpoint?: string; auth: AuthConfig } = $props();
  let folderId = $state('');
  let sessions = $state<ChatSession[]>([]);
  let selected = $state('');
  let detail = $state<ChatSession | null>(null);
  let draft = $state('');
  let draftStorageError = $state(false);
  let activeDraftKey = '';
  let error = $state('');
  let busy = $state(false);
  let copied = $state(false);
  let renameId = $state('');
  let renameTitle = $state('');
  let renameInput = $state<HTMLInputElement>();
  let guide = $state(false);
  let seen = $state<Record<string, number>>({});
  let feed: HTMLDivElement;
  let fileInput: HTMLInputElement;
  let attachments = $state<ChatFile[]>([]);
  let generation = 0;
  let retry: { text: string; id: string; attachmentKey: string } | null = null;
  const sessionStatusKeys = { waiting: 'chat.19', connected: 'chat.20', offline: 'chat.22', closed: 'chat.21' } as const;
  const selectedFolder = $derived(folders.find(f => f.id === folderId));
  const messages = $derived(detail?.messages ?? []);
  const feedItems = $derived(groupChatMessages(messages));
  const pendingState = $derived(pendingChatState(detail));
  const statusLabel = $derived(pendingState === 'awaiting_user' ? $t('chat.73') : pendingState === 'processing' ? $t('chat.70') : pendingState === 'interrupted' ? $t('chat.71') : detail?.status === 'waiting' ? $t('chat.19') : detail?.status === 'connected' ? $t('chat.20') : detail?.status === 'closed' ? $t('chat.21') : $t('chat.22'));
  const bootstrap = $derived(buildChatPrompt(selected, folderId, endpoint ? `MCP URL: ${endpoint}` : ''));
  const unreadTotal = $derived(sessions.reduce((total, session) => total + unread(session), 0));
  function replyCount(session: ChatSession): number {
    return session.assistant_message_count ?? session.messages?.filter(m => m.role === 'assistant').length ?? 0;
  }
  function unread(session: ChatSession): number { return Math.max(0, replyCount(session) - (seen[session.id] ?? 0)); }
  function seenKey(ws: string, folder: string): string { return `ctmcp-chat-seen:${ws}:${folder}`; }
  function restoreSeen(ws: string, folder: string): Record<string, number> {
    try {
      const saved = JSON.parse(localStorage.getItem(seenKey(ws, folder)) ?? '{}');
      if (!saved || typeof saved !== 'object' || Array.isArray(saved)) return {};
      return Object.fromEntries(Object.entries(saved).filter(([, value]) => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0)) as Record<string, number>;
    } catch { return {}; }
  }
  function markRead() {
    if (!detail || !feed || document.visibilityState !== 'visible' || feed.scrollHeight - feed.scrollTop - feed.clientHeight >= 120) return;
    const count = replyCount(detail);
    if ((seen[detail.id] ?? 0) >= count) return;
    seen = { ...seen, [detail.id]: count };
    try { localStorage.setItem(seenKey(workspaceId, folderId), JSON.stringify(seen)); } catch { /* Browsing remains usable without local storage. */ }
  }
  async function showUnread() {
    const target = sessions.find(session => session.id === selected && unread(session)) ?? sessions.find(session => unread(session));
    if (!target) return;
    if (target.id !== selected) await choose(target);
    await tick();
    if (feed) { feed.scrollTo({ top: feed.scrollHeight, behavior: 'instant' }); markRead(); }
  }
  function persistDraft() {
    if (activeDraftKey) draftStorageError = !chatDrafts.save(activeDraftKey, { text: draft, attachments, retry });
  }
  function activateDraft(ws: string, folder: string, chat: string) {
    const key = chatDraftKey(ws, folder, chat);
    if (activeDraftKey === key) return;
    persistDraft();
    activeDraftKey = key;
    const saved = chatDrafts.load(key);
    draft = saved.draft.text; attachments = saved.draft.attachments; retry = saved.draft.retry;
    draftStorageError = !saved.persisted;
  }
  onDestroy(persistDraft);
  async function refresh(ws: string, folder: string, gen: number) {
    const list = await localChat(ws, folder, { action: 'list' });
    if (gen !== generation) return;
    sessions = list.sessions ?? [];
    if (!selected && sessions.length) selected = sessions[0].id;
    const target = selected;
    if (target) {
      activateDraft(ws, folder, target);
      const result = await localChat(ws, folder, { action: 'read', chat_id: target });
      if (gen !== generation || selected !== target) return;
      const nearBottom = !feed || feed.scrollHeight - feed.scrollTop - feed.clientHeight < 120;
      const switched = detail?.id !== target;
      const changed = detail?.updated_at !== result.session?.updated_at || detail?.id !== target;
      detail = result.session ?? null;
      if (error.includes('Chat storage is busy')) error = '';
      if (changed && (nearBottom || switched)) { await tick(); if (gen === generation && selected === target && feed) feed.scrollTop = feed.scrollHeight; }
      if (gen === generation && selected === target) markRead();
    }
  }
  $effect(() => {
    if (!folders.some(f => f.id === folderId)) folderId = folders.find(f => f.id === activeFolderId)?.id ?? folders[0]?.id ?? '';
  });
  $effect(() => {
    const ws = workspaceId, folder = folderId; const gen = ++generation;
    untrack(persistDraft); activeDraftKey = ''; draftStorageError = false;
    seen = restoreSeen(ws, folder);
    sessions = []; selected = ''; detail = null; draft = ''; error = ''; retry = null; attachments = []; renameId = ''; renameTitle = '';
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
    selected = s.id; detail = null; activateDraft(workspaceId, folderId, s.id); guide = false; error = '';
    const gen = generation;
    try { await refresh(workspaceId, folderId, gen); } catch(e) { if (gen === generation) error = String(e); }
  }
  async function startRename(session: ChatSession) {
    if (busy) return;
    renameId = session.id; renameTitle = session.title; error = '';
    await tick();
    if (renameId === session.id) { renameInput?.focus(); renameInput?.select(); }
  }
  function cancelRename() {
    if (busy) return;
    renameId = ''; renameTitle = '';
  }
  async function saveRename() {
    if (busy || !renameId) return;
    const title = renameTitle.trim();
    if (!title || new TextEncoder().encode(title).length > 240) { error = $t('chat.87'); return; }
    const target = renameId, gen = generation;
    busy = true; error = '';
    try {
      const result = await localChat(workspaceId, folderId, { action: 'rename', chat_id: target, title });
      if (gen !== generation) return;
      if (result.session) {
        sessions = sessions.map(session => session.id === target ? result.session! : session);
        if (selected === target) detail = result.session;
      }
      renameId = ''; renameTitle = '';
      await refresh(workspaceId, folderId, gen);
    } catch (e) { if (gen === generation) error = String(e); }
    finally { busy = false; }
  }
  async function create() {
    if (busy || !folderId) return; busy = true; error = ''; const gen = generation;
    try {
      const result = await localChat(workspaceId, folderId, { action: 'create', title: '新对话' });
      if (gen !== generation) return;
      detail = result.session ?? null; selected = detail?.id ?? '';
      if (selected) activateDraft(workspaceId, folderId, selected);
      guide = true;
      await refresh(workspaceId, folderId, gen);
    } catch(e) { if (gen === generation) error = String(e); } finally { busy = false; }
  }
  async function send() {
    if (busy || (!draft.trim() && !attachments.length) || !selected || !detail || detail.closed) return;
    busy = true; error = ''; const gen = generation; const target = selected; const content = draft.trim(); const cacheKey = activeDraftKey;
    const attachmentKey = attachments.map(f => f.id).join(',');
    if (!retry || retry.text !== content || retry.attachmentKey !== attachmentKey) retry = { text: content, id: crypto.randomUUID(), attachmentKey };
    const sent = retry; persistDraft();
    try {
      const result = await localChat(workspaceId, folderId, { action: 'send', chat_id: target, message_id: sent.id, text: content, attachment_ids: attachments.map(f => f.id) });
      const cleared = chatDrafts.clearSent(cacheKey, sent);
      if (gen !== generation || selected !== target) return;
      detail = result.session ?? null;
      if (cleared.cleared) { draft = ''; retry = null; attachments = []; draftStorageError = !cleared.persisted; }
      guide = false;
      await tick(); if (feed) feed.scrollTop = feed.scrollHeight;
    } catch(e) { if (gen === generation) error = String(e); } finally { busy = false; }
  }
  function pasteImages(event: ClipboardEvent) {
    const files = Array.from(event.clipboardData?.files ?? []).filter(file => file.type.startsWith('image/'));
    if (!files.length || busy || !selected || !detail || detail.closed) return;
    event.preventDefault();
    void uploadFiles(files);
  }
  async function uploadFiles(files: FileList | File[] | null) {
    if (!files?.length || busy || !selected || !detail || detail.closed) return;
    if (attachments.length + files.length > 5 || [...files].some(f => !f.size || f.size > 2 * 1024 * 1024)) { error = $t('chat.51'); return; }
    busy = true; error = ''; const gen = generation, target = selected, folder = folderId, ws = workspaceId, cacheKey = activeDraftKey;
    persistDraft();
    try {
      for (const file of [...files]) {
        const data = await new Promise<string>((resolve, reject) => {
          const reader = new FileReader(); reader.onerror = () => reject(new Error($t('chat.57')));
          reader.onload = () => resolve(String(reader.result).split(',')[1]); reader.readAsDataURL(file);
        });
        const result = await localChat(ws, folder, {action:'upload', chat_id:target, upload_id:crypto.randomUUID(), name:file.name, data_base64:data});
        if (result.attachment) {
          const cached = chatDrafts.load(cacheKey).draft;
          if (!cached.attachments.some(file => file.id === result.attachment!.id)) cached.attachments.push(result.attachment);
          chatDrafts.save(cacheKey, cached);
        }
        if (gen !== generation || target !== selected) return;
        if (result.attachment) { attachments = [...attachments, result.attachment]; persistDraft(); }
      }
    } catch(e) { if (gen === generation) error = String(e); } finally { busy = false; if (fileInput) fileInput.value = ''; }
  }
  async function close() {
    if (busy || !selected) return; busy = true; error = ''; const gen = generation, target = selected;
    try { const result = await localChat(workspaceId, folderId, { action: 'close', chat_id: target }); if (gen === generation && target === selected) detail = result.session ?? null; }
    catch(e) { if (gen === generation) error = String(e); } finally { busy = false; }
  }
  async function detach() {
    if (busy || !selected || !detail || detail.closed) return;
    busy = true; error = ''; const gen = generation, target = selected;
    try {
      const result = await localChat(workspaceId, folderId, { action: 'detach', chat_id: target });
      if (gen === generation && target === selected) { detail = result.session ?? null; guide = true; }
    } catch (e) { if (gen === generation) error = String(e); } finally { busy = false; }
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
    const text = `# ${detail.title}\n\nSession: ${detail.id}\n\n` + messages.map(m => { const receipt = chatUserState(detail, m.id); return `## ${m.role === 'user' ? $t('chat.26') : 'AI'} · ${new Date(m.created_at).toLocaleString()}\n\n${m.tool_event ? `Tool (AI reported): ${m.tool_event.name} · ${m.tool_event.status}\n\n` : ''}${m.text}\n${receipt ? `\n${$t('chat.84')}：${$t(receipt.read ? 'chat.79' : 'chat.80')} · ${$t(USER_MESSAGE_STATUS_KEYS[receipt.status])}\n` : ''}${m.role === 'assistant' ? `\nReply state: ${m.awaiting_user ? 'awaiting_user' : m.final === false ? 'supplementing' : 'complete'}\n` : ''}${m.tool_event?.input ? `\nInput:\n${m.tool_event.input}\n` : ''}${m.tool_event?.output ? `\nOutput:\n${m.tool_event.output}\n` : ''}${m.tool_event?.output_truncated ? '\nOutput truncated\n' : ''}${(m.attachments ?? []).map(f => `\nAttachment: ${f.name} (${f.size} bytes)\nPath: ${f.path}\n`).join('')}`; }).join('\n');
    const url = URL.createObjectURL(new Blob([text], {type:'text/markdown;charset=utf-8'}));
    const a = document.createElement('a'); a.href=url; a.download=`chat-${detail.id}.md`; a.click(); setTimeout(() => URL.revokeObjectURL(url),1000);
  }
</script>

<svelte:document onvisibilitychange={markRead}/>
<svelte:window onpagehide={persistDraft}/>
<section class="chat-shell" aria-label={$t("chat.35")}>
  <aside class="session-sidebar">
    <div class="sidebar-heading"><span>{$t("chat.0")}</span><span class="beta">{$t("chat.48")}</span></div>
    <button class="new-chat" onclick={create} disabled={busy || !folderId}><Plus size={16}/>{$t("chat.1")}</button>
    <label class="folder-label" for="chat-folder">{$t("chat.2")}</label>
    <select id="chat-folder" aria-label={$t("chat.2")} bind:value={folderId} disabled={busy}>{#each folders as folder}<option value={folder.id}>{folder.name}</option>{/each}</select>
    <div class="section-label">{$t("chat.3")}</div>
    <nav aria-label="聊天会话">{#each sessions as s (s.id)}
      <div class="session-row" data-status={s.status}>
        {#if unread(s)}<span class="unread-badge" title={`${$t("chat.90")}: ${unread(s)}`} aria-label={`${$t("chat.90")}: ${unread(s)}`}>{unread(s) > 99 ? '99+' : unread(s)}</span>{/if}
        {#if renameId === s.id}
          <form class="session-rename" onsubmit={(event) => { event.preventDefault(); void saveRename(); }}>
            <input bind:this={renameInput} bind:value={renameTitle} aria-label={$t('chat.86')} maxlength="240" disabled={busy} onkeydown={(event) => { if (event.key === 'Escape') { event.preventDefault(); cancelRename(); } if (event.key === 'Enter' && event.isComposing) event.preventDefault(); }}/>
            <button type="submit" class="rename-action" disabled={busy || !renameTitle.trim()} title={$t('Save')} aria-label={$t('Save')}><Check size={14}/></button>
            <button type="button" class="rename-action" disabled={busy} onclick={cancelRename} title={$t('Cancel')} aria-label={$t('Cancel')}><X size={14}/></button>
          </form>
        {:else}
          <button class="session-choice" class:active={selected === s.id} onclick={() => choose(s)} disabled={busy} title={`${s.title} · ${$t(sessionStatusKeys[s.status])}`} aria-label={`${s.title} · ${$t(sessionStatusKeys[s.status])}`}><MessageSquare size={15}/><span>{s.title}</span><i class="session-presence" title={$t(sessionStatusKeys[s.status])} aria-hidden="true"></i>{#if s.closed}<span class="ended">{$t("chat.4")}</span>{/if}</button>
          <button type="button" class="rename-action" disabled={busy} onclick={() => void startRename(s)} title={$t('chat.85')} aria-label={`${$t('chat.85')}: ${s.title}`}><Pencil size={13}/></button>
        {/if}
      </div>
    {/each}</nav>
    <div class="local-note"><span class="dot"></span>{$t("chat.5")}</div>
  </aside>
  <div class="conversation" style:--conversation-accent={conversationAccent(selected)}>
    <header class="chat-header"><div><strong>{selectedFolder?.name ?? $t('chat.23')}</strong><span class="status"><i class:online={detail?.status === 'waiting'}></i>{statusLabel}</span></div><div class="header-actions">{#if unreadTotal}<button class="unread-total" onclick={showUnread} title={$t("chat.91")} aria-label={`${$t("chat.90")}: ${unreadTotal}`}>{$t("chat.90")}<span>{unreadTotal > 99 ? '99+' : unreadTotal}</span></button>{/if}{#if selected && (detail?.status === 'connected' || detail?.status === 'waiting')}<button class="detach-ai" disabled={busy} onclick={detach} title={$t("chat.89")}>{$t("chat.88")}</button>{/if}{#if selected}<button title={$t("chat.36")} aria-label={$t("chat.36")} onclick={download}><Download size={16}/></button><button title={$t("chat.37")} onclick={() => guide = !guide}>{$t("chat.6")}</button>{/if}</div></header>
    {#if error}<div class="chat-error" role="alert">{error}<button onclick={() => error = ''} aria-label={$t("chat.38")}>×</button></div>{/if}
    {#if guide && selected}<div class="guide"><div><strong>{$t("chat.7")}</strong><button onclick={copyPrompt} disabled={!detail || detail.closed}>{#if copied}<Check size={14}/>{:else}<Copy size={14}/>{/if}{copied ? $t('chat.25') : $t('chat.24')}</button></div><p>{$t("chat.8")}</p><details><summary>{$t("chat.9")}</summary><pre>{bootstrap}</pre></details></div>{/if}
    <div class="message-feed" bind:this={feed} onscroll={markRead}>
      {#if !messages.length}<div class="empty-state"><div class="empty-icon"><MessageSquare size={25} strokeWidth={1.4}/></div><p class="overline">{$t("chat.47")}</p><h2>{$t("chat.10")}</h2><p>{$t("chat.11")}<br/>{$t("chat.12")}</p>{#if !selected}<button class="start-button" onclick={create} disabled={busy || !folderId}><Plus size={15}/>{$t("chat.13")}</button>{:else}<button class="start-button" onclick={copyPrompt} disabled={!detail || detail.closed}><Copy size={15}/>{$t("chat.14")}</button>{/if}<div class="suggestions"><button onclick={() => { draft = '先阅读项目结构，告诉我你发现了什么。'; persistDraft(); }}>{$t("chat.15")}<span>↗</span></button><button onclick={() => { draft = '帮我制定一个实现计划，先不要修改文件。'; persistDraft(); }}>{$t("chat.16")}<span>↗</span></button></div></div>
      {:else}<div class="message-column">{#each feedItems as item (item.id)}{#if item.kind === 'tools'}<article class="assistant grouped-tools"><div class="message-meta">{$t("chat.54")}</div><div class="message-body"><ChatToolActivity messages={item.messages} {workspaceId} {folderId} chatId={selected}/></div></article>{:else}{@const m = item.message}<article class:user={m.role === 'user'} class:assistant={m.role === 'assistant'}><div class="message-meta">{m.role === 'user' ? $t('chat.26') : m.final === false ? $t('chat.27') : 'AI'}<time>{new Date(m.created_at).toLocaleTimeString([], {hour:'2-digit',minute:'2-digit'})}</time></div><div class="message-body"><ChatMarkdown text={m.text}/>{#each m.attachments ?? [] as file (workspaceId + ":" + folderId + ":" + selected + ":" + file.id)}<ChatAttachment {workspaceId} {folderId} chatId={selected} {file}/>{/each}<ChatReplyState state={chatReplyState(detail, m.id)}/><ChatUserState state={chatUserState(detail, m.id)}/></div></article>{/if}{/each}{#if pendingState}<div class="pending" class:processing={pendingState === 'processing'} role="status" aria-live="polite"><ChatStatusIcon state={pendingState} label={pendingState === 'awaiting_user' ? $t('chat.73') : pendingState === 'processing' ? $t('chat.70') : pendingState === 'interrupted' ? $t('chat.71') : $t('chat.29')}/>{pendingState === 'awaiting_user' ? $t('chat.73') : pendingState === 'processing' ? $t('chat.70') : pendingState === 'interrupted' ? $t('chat.71') : $t('chat.29')}</div>{/if}</div>{/if}
    </div>
    <footer class="composer-area"><form onsubmit={(e) => { e.preventDefault(); void send(); }}><textarea aria-label={$t("chat.39")} bind:value={draft} oninput={(event) => { draft = event.currentTarget.value; persistDraft(); }} onpaste={pasteImages} placeholder={detail?.closed ? $t('chat.30') : selected ? $t('chat.31') : $t('chat.32')} disabled={!selected || !detail || detail.closed || busy} onkeydown={(e) => { if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); void send(); } }} rows="3" maxlength="32000"></textarea><input class="file-input" type="file" multiple bind:this={fileInput} onchange={() => uploadFiles(fileInput.files)} aria-label={$t('chat.50')}/>{#if attachments.length}<div class="draft-files">{#each attachments as file (file.id)}<span>{file.name}<button type="button" disabled={busy} aria-label={$t('chat.53')} onclick={() => { attachments = attachments.filter(f => f.id !== file.id); persistDraft(); }}>×</button></span>{/each}</div>{/if}<div class="composer-toolbar"><span><span class="dot"></span>{selectedFolder?.name ?? $t('chat.33')}</span><div><button type="button" disabled={busy || !selected || detail?.closed} title={$t('chat.51')} onclick={() => fileInput.click()}>{$t('chat.50')}</button>{#if selected && !detail?.closed}<button type="button" class="end-button" onclick={close} disabled={busy} title={$t("chat.40")}><Square size={12}/>{$t("chat.17")}</button>{/if}<button class="send-button" aria-label={$t("chat.41")} disabled={busy || (!draft.trim() && !attachments.length) || !selected || detail?.closed}><ArrowUp size={18}/></button></div></div></form>{#if draftStorageError}<p class="draft-warning" role="status">{$t("chat.94")}</p>{/if}<div class="composer-hint">{$t("chat.18")} <span>{detail?.archive_path ?? $t('chat.34')}</span></div></footer>
  </div>
</section>
<style>


.file-input{display:none}.draft-files{display:flex;flex-wrap:wrap;gap:8px;font-size:11px}.draft-files span{border:1px solid var(--color-border);padding:5px 8px;border-radius:6px;overflow-wrap:anywhere}.draft-files button{margin-left:8px}

.chat-shell{height:max(640px,calc(100dvh - 150px));max-height:1100px;display:grid;grid-template-columns:218px minmax(0,1fr);border:1px solid var(--color-border);border-radius:16px;overflow:hidden;background:var(--card-bg);color:var(--color-text)}button{cursor:pointer}button:disabled{cursor:default;opacity:.45}button:focus-visible,select:focus-visible,textarea:focus-visible{outline:2px solid var(--primary);outline-offset:3px}.session-sidebar{padding:22px 14px 14px;background:var(--color-bg);display:flex;flex-direction:column;border-right:1px solid var(--color-border);min-height:0}.sidebar-heading{display:flex;align-items:center;justify-content:space-between;font-size:13px;font-weight:600;padding:0 9px 20px}.beta{font:9px monospace;letter-spacing:1px;color:var(--color-text-muted);border:1px solid var(--color-border);padding:3px 5px;border-radius:4px}.new-chat{display:flex;gap:9px;align-items:center;border:1px solid var(--color-border);border-radius:9px;padding:10px 12px;background:var(--card-bg);font-size:12px}.folder-label,.section-label{font-size:10px;color:var(--color-text-muted);margin:22px 8px 8px;display:block}.section-label{margin-top:28px}select{font-size:11px;width:100%;padding:9px;border:1px solid var(--color-border);border-radius:7px;background:var(--card-bg)}nav{overflow:auto;flex:1}nav button{display:flex;align-items:center;gap:8px;width:100%;text-align:left;padding:11px 9px;margin:3px 0;border-radius:7px;font-size:12px;color:var(--color-text-muted)}nav button.active{background:var(--card-bg);color:var(--color-text);box-shadow:0 1px 4px #00000008}nav button span:first-of-type{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.ended{font-size:9px;margin-left:auto;white-space:nowrap}.local-note{font-size:10px;color:var(--color-text-muted);padding:20px 6px 3px;display:flex;gap:7px;align-items:center}.dot{display:inline-block;width:5px;height:5px;border-radius:50%;background:#83a18e}.conversation{display:flex;flex-direction:column;min-width:0;min-height:0}.chat-header{padding:19px 26px;display:flex;justify-content:space-between;align-items:center;border-bottom:1px solid var(--color-border);gap:12px}.chat-header>div:first-child{display:flex;align-items:center;gap:17px;flex-wrap:wrap}.chat-header strong{font-size:13px;font-weight:550}.status{font-size:10px;color:var(--color-text-muted);display:flex;align-items:center;gap:6px}.status i{width:6px;height:6px;border-radius:50%;background:#989b98}.status i.online{background:#49a476;box-shadow:0 0 0 3px #49a47618}.header-actions{display:flex;gap:14px;align-items:center;font-size:11px;white-space:nowrap;color:var(--color-text-muted)}.message-feed{flex:1;overflow-y:auto;min-height:0;scroll-behavior:smooth}.empty-state{max-width:600px;margin:0 auto;padding:65px 25px 40px;text-align:center}.empty-icon{display:grid;place-items:center;width:54px;height:54px;border:1px solid var(--color-border);border-radius:17px;margin:0 auto 23px}.overline{font:9px monospace!important;letter-spacing:2.5px;color:var(--color-text-muted)}.empty-state h2{font-weight:500;font-size:30px;letter-spacing:-1px;margin:15px 0}.empty-state p{font-size:12px;line-height:1.9;color:var(--color-text-muted)}.start-button{display:inline-flex;align-items:center;gap:8px;margin:23px 0;padding:9px 15px;border:1px solid var(--color-border);border-radius:8px;font-size:11px}.suggestions{display:flex;gap:10px;margin:15px auto 0;max-width:430px}.suggestions button{border:1px solid var(--color-border);padding:15px 13px;border-radius:10px;text-align:left;flex:1;font-size:11px}.suggestions span{float:right;color:var(--color-text-muted)}.message-column{max-width:800px;margin:auto;padding:25px 40px 30px}article{margin-bottom:26px}.message-meta{display:flex;align-items:center;gap:10px;font-size:11px;font-weight:600;margin-bottom:9px}.message-meta time{font-size:9px;color:var(--color-text-muted);font-weight:400}.user{margin-left:14%}.user .message-meta{justify-content:flex-end}.user .message-body{background:color-mix(in srgb,#6e92ba 9%,var(--card-bg));border:1px solid var(--color-border);padding:12px 17px;border-radius:14px 14px 3px 14px}.assistant .message-body{padding:12px 17px;background:color-mix(in srgb,#829887 6%,var(--card-bg));border:1px solid var(--color-border);border-radius:14px 14px 14px 3px}.pending{font-size:11px;color:var(--color-text-muted);display:flex;align-items:center;gap:8px}.composer-area{padding:12px 30px 17px}.composer-area form{max-width:760px;margin:0 auto;border:1px solid var(--color-border);border-radius:17px;padding:14px 16px 10px;box-shadow:0 3px 15px #00000004}.composer-area textarea{display:block;resize:none;width:100%;background:transparent;border:0;font:13px/1.7 inherit;color:var(--color-text);outline:none;min-height:62px}.composer-toolbar{display:flex;justify-content:space-between;align-items:center;margin-top:7px;font-size:10px;color:var(--color-text-muted)}.composer-toolbar>span{display:flex;align-items:center;gap:7px}.composer-toolbar>div{display:flex;gap:14px;align-items:center}.send-button{display:grid;place-items:center;width:31px;height:31px;border-radius:50%;background:var(--color-text);color:var(--card-bg)}.end-button{display:flex;gap:5px;align-items:center;font-size:10px}.composer-hint{max-width:760px;margin:10px auto 0;display:flex;justify-content:space-between;gap:10px;font-size:9px;color:var(--color-text-muted)}.composer-hint span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:50%}.guide{margin:15px 25px 0;padding:14px 16px;border:1px solid var(--color-border);border-radius:10px;font-size:11px;background:var(--color-bg)}.guide>div{display:flex;justify-content:space-between;gap:10px;align-items:center}.guide button{display:flex;gap:6px;align-items:center;white-space:nowrap}.guide p{color:var(--color-text-muted);margin:8px 0;line-height:1.7}.guide pre{white-space:pre-wrap;overflow-wrap:anywhere;font-size:10px;line-height:1.8;max-height:130px;overflow:auto;margin-top:8px}.guide summary{cursor:pointer;font-size:10px}.chat-error{display:flex;justify-content:space-between;gap:10px;margin:12px 25px 0;padding:10px;border:1px solid var(--danger);color:var(--danger);border-radius:8px;font-size:11px;overflow-wrap:anywhere}@media(max-width:800px){.chat-shell{grid-template-columns:160px minmax(0,1fr)}.message-column{padding:20px}.chat-header{padding:15px}.composer-area{padding:10px 15px}.empty-state{padding-top:45px}.suggestions{flex-direction:column}}@media(max-width:560px){:global(.sx-layout:has(.chat-shell) > .sx-side){display:none}:global(.sx-layout:has(.chat-shell)){grid-template-columns:minmax(0,1fr)}.chat-shell{grid-template-columns:1fr;height:calc(100dvh - 140px);min-height:600px}.session-sidebar{padding:10px;display:grid;grid-template-columns:1fr 1fr;gap:8px;border-right:0;border-bottom:1px solid var(--color-border)}.sidebar-heading,.folder-label,.section-label,.local-note{display:none}nav{grid-column:1/-1;display:flex;max-height:40px;gap:6px}nav button{width:auto;min-width:100px}.new-chat{padding:7px 10px}.chat-header{padding:12px}.empty-state{padding:25px 20px}.empty-state h2{font-size:24px}.guide{margin:10px}.composer-hint span{display:none}.message-column{padding:15px}}
.session-row{display:flex;align-items:center;gap:2px;min-width:0}.session-choice{flex:1;min-width:0}.session-choice :global(svg){flex-shrink:0}nav .rename-action{width:25px;flex:0 0 25px;justify-content:center;padding:6px 3px;margin:0}.session-rename{display:flex;gap:2px;align-items:center;min-width:0;width:100%;padding:5px 0}.session-rename input{min-width:0;width:100%;flex:1;border:1px solid var(--color-border);border-radius:5px;background:var(--card-bg);padding:7px 5px;font-size:12px;color:var(--color-text)}.session-rename input:focus-visible{outline:2px solid var(--primary);outline-offset:1px}@media(max-width:560px){.session-row{min-width:145px;max-width:300px;flex-shrink:0}.session-row:has(.session-rename){width:280px}nav:has(.session-rename){max-height:60px}nav .rename-action{min-width:25px}}
.session-row{border:1px solid transparent;border-radius:8px;margin:3px 0}.session-row[data-status="waiting"]{background:color-mix(in srgb,#31a56c 12%,var(--card-bg));border-color:color-mix(in srgb,#31a56c 45%,var(--color-border))}.session-row[data-status="connected"]{background:color-mix(in srgb,#bc8d36 8%,var(--card-bg));border-color:color-mix(in srgb,#bc8d36 25%,var(--color-border))}.session-row[data-status="closed"]{opacity:.65}.session-row[data-status="waiting"] .session-choice.active,.session-row[data-status="connected"] .session-choice.active{background:transparent;box-shadow:inset 3px 0 var(--color-text-muted)}.session-presence{width:6px;height:6px;border-radius:50%;flex:0 0 6px;background:#8b8d92;margin-left:auto}.session-row[data-status="waiting"] .session-presence{background:#31a56c;box-shadow:0 0 0 3px #31a56c20}.session-row[data-status="connected"] .session-presence{background:#bc8d36}
.session-row{position:relative}.unread-badge{position:absolute;top:-3px;right:0;min-width:15px;height:15px;padding:0 4px;border-radius:9px;background:#55749a;color:white;font:9px/15px system-ui;text-align:center;pointer-events:none;z-index:1}.unread-total{display:flex;align-items:center;gap:5px;color:var(--color-text)}.unread-total span{background:#55749a;color:white;border-radius:9px;padding:1px 5px;font-size:9px}.header-actions{flex-wrap:wrap;justify-content:flex-end;row-gap:6px}
.grouped-tools{margin-bottom:16px}.grouped-tools .message-body{padding:9px 12px;background:transparent}
.conversation{background:color-mix(in srgb,var(--conversation-accent) 8%,var(--card-bg))}.draft-warning{max-width:760px;margin:8px auto 0;font-size:10px;color:var(--color-text-muted)}
</style>
