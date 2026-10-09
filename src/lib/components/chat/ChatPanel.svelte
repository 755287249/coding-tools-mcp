<script lang="ts">
  import { randomId, copyText } from "$lib/browser-tools";
  import { autoGrow, messageBytes } from '$lib/chat/autogrow';
  import ChatMembers from './ChatMembers.svelte';
  import ChatTaskPanel from './ChatTaskPanel.svelte';
  import { taskPlanMarkdown } from '$lib/chat/task-state';
  import DraftAttachment from './DraftAttachment.svelte';
  import { uploadLocalFile } from '$lib/chat/attachment-transfer';
  import { appendAttachmentMention, attachmentCandidates, mentionQuery, mentionChoices, mentionParts, referencedAttachments } from '$lib/chat/mentions';
  import type { Snippet } from 'svelte';
  import { connectionResult, visibleChatMessages, isPristineConversation } from '$lib/chat/connection';
  import { t, locale } from '$lib/i18n';
  import { tick, untrack, onDestroy } from 'svelte';
  import Plus from '@lucide/svelte/icons/plus';
  import Folder from '@lucide/svelte/icons/folder';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ArrowDown from '@lucide/svelte/icons/arrow-down';
  import ChatOutline from './ChatOutline.svelte';
  import { sessionPresence } from '$lib/chat/navigation';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import MessageSquare from '@lucide/svelte/icons/message-square';
  import Copy from '@lucide/svelte/icons/copy';
  import Download from '@lucide/svelte/icons/download';
  import Check from '@lucide/svelte/icons/check';
  import Pencil from '@lucide/svelte/icons/pencil';
  import X from '@lucide/svelte/icons/x';
  import Settings from '@lucide/svelte/icons/settings';
  import ChatToolActivity from './ChatToolActivity.svelte';
  import { chatDrafts, chatDraftKey, conversationAccent } from '$lib/chat/drafts';
  import { createSessionCache, sessionCacheKey } from '$lib/chat/session-cache';
  import { groupChatMessages } from '$lib/chat/tool-activity';
  import ChatMarkdown from './ChatMarkdown.svelte';
  import ChatReplyState from './ChatReplyState.svelte';
  import ChatUserState from './ChatUserState.svelte';
  import ChatStatusIcon from './ChatStatusIcon.svelte';
  import ChatAttachment from './ChatAttachment.svelte';
  import { localChat, type ChatSession, type ChatFile, type ChatAction } from '$lib/api/chat';
  import { getBackend, loadMcpAuthSecrets } from '$lib/backend';
  import { buildConnectionPrompt } from '$lib/connect/prompt';
  import { buildChatPrompt } from '$lib/connect/chat-prompt';
  import { pendingChatState, chatReplyState, chatUserState, USER_MESSAGE_STATUS_KEYS } from '$lib/chat/status';
  import type { AuthConfig, WorkspaceFolder } from '$lib/types';
  let { onPlugins, tasksOpen=false, tasksExpanded=false, onCloseTasks=()=>{}, onToggleTasksExpanded=()=>{}, workspaceId, folders, activeFolderId = '', endpoint = '', auth, externalNavigation = false, requestedChatId = '', requestedFolderId = '', startNew = false, connectRequested = false, headerActions, onNavigate }: { onPlugins?:(folderId:string)=>void;tasksOpen?:boolean;tasksExpanded?:boolean;onCloseTasks?:()=>void;onToggleTasksExpanded?:()=>void; workspaceId: string; folders: WorkspaceFolder[]; activeFolderId?: string; endpoint?: string; auth: AuthConfig; externalNavigation?: boolean; requestedChatId?: string; requestedFolderId?: string; startNew?: boolean; connectRequested?: boolean; headerActions?: Snippet; onNavigate?: (folderId: string, chatId: string) => void } = $props();
  let newMode=$state<'work'|'group'>('work');
  let memberBusy=$state(false);
  let projectOpen=$state(false);
  async function memberAction(args:ChatAction){
    if(!selected||memberBusy)return false;const scope=currentScope;memberBusy=true;error='';
    try{const result=await localChat(workspaceId,folderId,{...args,chat_id:selected});if(scope===currentScope&&result.session){readSequence++;detail=result.session;return true;}return false;}
    catch(e){if(scope===currentScope)error=String(e);return false;}finally{memberBusy=false}
  }
  async function changeMode(next:'work'|'group'){
    if(!selected){newMode=next;return;}await memberAction({action:'set_mode',mode:next});
  }
  function mentionAgent(name:string){const at=composer?.selectionStart??draft.length;draft=draft.slice(0,at)+(at&&!/\s/.test(draft[at-1])?' ':'')+'@'+name+' '+draft.slice(at);persistDraft();void tick().then(()=>composer?.focus());}
  function selectProject(id:string){persistDraft();projectOpen=false;onNavigate?.(id,'');folderId=id;}
  let tasksWidth=$state(350);
  let folderId = $state('');
  let sessions = $state<ChatSession[]>([]);
  let selected = $state('');
  let detail = $state<ChatSession | null>(null);
  const isNewConversation=$derived(!selected || isPristineConversation(detail));
  /** No AI attached yet: highlight the connect button with a red dot. */
  const needsAi=$derived(!!folderId && !detail?.closed && !(detail?.status==='connected'||detail?.status==='waiting'));
  const mode=$derived(detail?.mode??newMode);
  let loadingDetail = $state(false);
  const detailCache = createSessionCache();
  let displayedScope: { workspace: string; folder: string; chat: string } | null = null;
  let readSequence = 0;
  let readError = '';
  let draft = $state('');
  let draftStorageError = $state(false);
  let activeDraftKey = '';
  let error = $state('');
  let busy = $state(false);
  let uploading = $state(false);
  let referenceOpen = $state(false);
  let referenceInput = $state('');
  let referenceRetry: {key:string; id:string} | null = null;
  $effect(() => {const scope = [workspaceId, folderId, selected]; referenceOpen = false; referenceInput = ''; referenceRetry = null;});
  let attachments = $state<ChatFile[]>([]);
  let composer: HTMLTextAreaElement;
  let draftHighlight = $state<HTMLDivElement>();
  let caret = $state(0);
  let mentionFocused = $state(false);
  let mentionIndex = $state(0);
  const mentionFiles = $derived(attachmentCandidates([...(detail?.messages ?? []).flatMap(message=>message.attachments ?? []),...attachments]));
  const mention = $derived(mentionFocused ? mentionQuery(draft,caret) : null);
  const choices = $derived(mention ? mentionChoices(mentionFiles,mention.query) : []);
  const highlightedDraft = $derived(mentionParts(draft,mentionFiles));
  $effect(()=>{const query=mention?.query;mentionIndex=0;});
  function updateCaret() { caret=composer?.selectionStart ?? 0; }
  function insertMention(file: ChatFile) {
    if(!mention || !file.label)return;
    const before=draft.slice(0,mention.start),after=draft.slice(mention.end);
    draft=before+'@'+file.label+' '+after;caret=before.length+file.label.length+2;
    if(!attachments.some(f=>f.id===file.id))attachments=[...attachments,file];
    persistDraft();mentionFocused=false;
    void tick().then(()=>{composer?.focus();composer?.setSelectionRange(caret,caret)});
  }
  function composerKeys(event: KeyboardEvent) {
    if(event.isComposing)return;
    if(mention && choices.length){
      if(event.key==='ArrowDown'||event.key==='ArrowUp'){event.preventDefault();mentionIndex=(mentionIndex+(event.key==='ArrowDown'?1:-1)+choices.length)%choices.length;return;}
      if(event.key==='Enter'||event.key==='Tab'){event.preventDefault();insertMention(choices[mentionIndex] ?? choices[0]);return;}
      if(event.key==='Escape'){event.preventDefault();mentionFocused=false;return;}
    }
    if(event.key==='Enter'&&!event.shiftKey){event.preventDefault();void send();}
  }
  function removeAttachment(file:ChatFile) {
    attachments=attachments.filter(f=>f.id!==file.id);
    if(file.label)draft=draft.replace(new RegExp('@'+file.label+'(?![0-9])','g'),'');
    persistDraft();
  }
  let column = $state<HTMLDivElement>();
  let following = $state(true);
  let activeMessage = $state('');
  let lastScrollTop = 0;
  let copied = $state(false);
  let queueChanging = $state(false);
  async function toggleQueueMode() {
    if(queueChanging || !detail || !selected || detail.closed || loadingDetail)return;
    const scope=currentScope,ws=workspaceId,folder=folderId,chat=selected;
    const mode=detail.queue_mode==='split'?'merge':'split';queueChanging=true;
    try{const result=await localChat(ws,folder,{action:'set_queue_mode',chat_id:chat,mode});if(scope===currentScope&&result.session){readSequence++;detail=result.session;}}
    catch(e){if(scope===currentScope)error=String(e)}finally{queueChanging=false}
  }
  let renameId = $state('');
  let renameTitle = $state('');
  let renameInput = $state<HTMLInputElement>();
  let disconnectDialog = $state<HTMLDialogElement>();
  let disconnectAt = $state(0);
  let disconnectSeconds = $state(10);
  $effect(() => {
    const scope = [workspaceId, folderId, selected];
    disconnectDialog?.close(); disconnectAt = 0;
  });
  $effect(() => {
    if (!disconnectAt) return;
    const update = () => disconnectSeconds = Math.max(0, Math.ceil((disconnectAt - Date.now()) / 1000));
    update(); const timer = setInterval(update, 200);
    return () => clearInterval(timer);
  });
  function requestDisconnect() { disconnectAt = Date.now() + 10000; disconnectDialog?.showModal(); }
  async function confirmDisconnect() {
    if (!disconnectAt || Date.now() < disconnectAt) return;
    disconnectDialog?.close(); disconnectAt = 0; await detach();
  }
  let guide = $state(false);
  let connectDialog = $state<HTMLDialogElement>();
  let connectionPrompt = $state('');
  let connectionError = $state('');
  let connectionLoading = $state(false);
  let connectionScope = $state('');
  let connectionRequest = $state('');
  let connectionSending = $state(false);
  let connectionRetry = '';
  let connectionMembers=$state<string[]>([]);
  let promptSequence = 0;
  const currentScope = $derived(sessionCacheKey(workspaceId, folderId, selected));
  const pairing = $derived(connectionRequest ? connectionResult(detail, connectionRequest) : 'waiting');
  const joinedMember = $derived(detail?.members?.find(m=>!connectionMembers.includes(m.id)&&m.status!=='offline'));
  const connectionPeer = $derived(mode==='group' ? joinedMember :
    detail?.connection_id && (detail.status==='connected'||detail.status==='waiting')
      ? {id:detail.connection_id,name:detail.agent_name??'AI'} : undefined);
  const connectionComplete = $derived(mode==='group' ? !!joinedMember : pairing==='success');
  const connectionGreeting = $derived(detail?.messages?.find(m=>m.role==='assistant'&&m.reply_to===connectionRequest&&m.final===true)?.text??'');
  $effect(() => {
    if (guide && connectionScope === currentScope) connectDialog?.showModal();
    else untrack(() => { connectDialog?.close(); guide = false; connectionPrompt = ''; connectionError = ''; connectionRequest = ''; connectionRetry = ''; promptSequence++; });
  });
  $effect(() => {
    const requested = connectRequested, chat = requestedChatId, folder = requestedFolderId;
    if (requested && chat && selected === chat && folderId === folder) untrack(() => { void openConnection(); });
  });
  async function openConnection() {
    if (busy || !folderId) return;
    if (!selected) await create(true);
    if (!selected || detail?.closed) return;
    onNavigate?.(folderId, selected);
    connectionMembers=(detail?.members??[]).map(m=>m.id);connectionScope = currentScope; connectionError = ''; connectionSending = false; copied = false; guide = true;
    connectionRequest = detail?.messages?.find(m => m.kind === 'connection_request' && !detail?.messages?.some(r => r.role === 'assistant' && r.reply_to === m.id && r.final === true))?.id ?? '';
    connectionRetry=connectionRequest;
    const request = ++promptSequence, ws = workspaceId, folder = folderId, chat = selected, scope = currentScope;
    connectionLoading = true; connectionPrompt = '';
    try {
      const loaded = endpoint ? await loadMcpAuthSecrets(getBackend(), ws, auth) : null;
      if (request !== promptSequence || scope !== currentScope || !guide) return;
      const connection = loaded ? buildConnectionPrompt({ workspaceName: selectedFolder?.name ?? '', endpoint, authType: auth.type, clientId: loaded.oauth_client_id || auth.oauth_client_id, password: loaded.oauth_password ?? '', bearerToken: loaded.bearer_token ?? '', folders: folders.map(f => f.path) }, $locale, true) : '';
      connectionPrompt = buildChatPrompt(chat, folder, connection);
      await prepareConnection(request);
    } catch { if (request === promptSequence) connectionError = $t('chat.42'); }
    finally { if (request === promptSequence) connectionLoading = false; }
  }
  async function prepareConnection(request: number) {
    if (request !== promptSequence || connectionScope !== currentScope || !selected || !connectionPrompt) return;
    const scope = currentScope, ws = workspaceId, folder = folderId, chat = selected;
    if(mode==='group'){connectionRequest='group-join';return;}
    // Reopening a live connection displays its existing greeting and public ID.
    const previous = [...(detail?.messages??[])].reverse().find(m=>m.kind==='connection_request');
    if(connectionPeer && previous && connectionResult(detail,previous.id)==='success') {
      connectionRequest=previous.id;return;
    }
    connectionRetry ||= connectionRequest || randomId();
    connectionSending = true; connectionError = '';
    try {
      const result = await localChat(ws, folder, {action:'request_connection', chat_id:chat, message_id:connectionRetry});
      if (request !== promptSequence || scope !== currentScope || !guide) return;
      if (!result.connection_request_id || !result.session) throw new Error('Missing connection request');
      readSequence++; detail = result.session; connectionRequest = result.connection_request_id;
    } catch (e) {
      if (request === promptSequence && scope === currentScope && guide) {
        connectionError = String(e); connectionPrompt = '';
      }
    } finally { if(request===promptSequence)connectionSending = false; }
  }
  let seen = $state<Record<string, number>>({});
  let feed = $state<HTMLDivElement>();
  let fileInput: HTMLInputElement;
  let generation = 0;
  let retry: { text: string; id: string; attachmentKey: string } | null = null;
  const presenceKeys = { online: 'chat.98', offline: 'chat.22', working: 'chat.70', error: 'chat.71', closed: 'chat.21' } as const;
  const selectedFolder = $derived(folders.find(f => f.id === folderId));
  const messages = $derived(visibleChatMessages(detail));
  const emptyComposer = $derived(!messages.length && (!selected || !!detail) && !detail?.closed);
  let tongueRevealed = $state(false);
  let tonguePointer = $state(false);
  let tongueFocused = $state(false);
  const tongueVisible = $derived(emptyComposer || tongueRevealed || projectOpen || referenceOpen);
  $effect(() => {
    const scope = currentScope;
    tongueRevealed = false; tonguePointer = false; tongueFocused = false; projectOpen = false;
  });
  $effect(() => {
    if (emptyComposer || tonguePointer || tongueFocused || projectOpen || referenceOpen) {
      if (tonguePointer || tongueFocused) tongueRevealed = true;
      return;
    }
    const timer = setTimeout(() => tongueRevealed = false, 3000);
    return () => clearTimeout(timer);
  });
  const feedItems = $derived(groupChatMessages(messages, detail?.closed ?? false));
  const pendingState = $derived(pendingChatState(detail));
  const statusLabel = $derived(pendingState === 'awaiting_user' ? $t('chat.73') : pendingState === 'processing' ? $t('chat.70') : pendingState === 'interrupted' ? $t('chat.71') : detail?.status === 'waiting' ? $t('chat.19') : detail?.status === 'connected' ? $t('chat.20') : detail?.status === 'closed' ? $t('chat.21') : $t('chat.22'));
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
  function updateScroll() {
    if (!feed) return;
    const bottom = feed.scrollHeight - feed.scrollTop - feed.clientHeight < 32;
    if (bottom) following = true;
    else if (feed.scrollTop < lastScrollTop) following = false;
    lastScrollTop = feed.scrollTop;
    const top = feed.getBoundingClientRect().top + 40;
    const rows = [...feed.querySelectorAll<HTMLElement>('[data-user-message]')];
    activeMessage = rows.findLast(row => row.getBoundingClientRect().top <= top)?.dataset.userMessage ?? rows[0]?.dataset.userMessage ?? '';
    markRead();
  }
  function toBottom() {
    following = true;
    if (feed) { feed.scrollTop = feed.scrollHeight; lastScrollTop = feed.scrollTop; updateScroll(); }
  }
  function jumpToMessage(id: string) {
    if (!feed) return;
    const target = [...feed.querySelectorAll<HTMLElement>('[data-user-message]')].find(row => row.dataset.userMessage === id);
    if (!target) return;
    following = false;
    feed.scrollTop += target.getBoundingClientRect().top - feed.getBoundingClientRect().top - 20;
    lastScrollTop = feed.scrollTop; activeMessage = id;
    target.focus({ preventScroll: true });
  }
  $effect(() => {
    const container = feed, content = column;
    if (!container || !content) return;
    const observer = new ResizeObserver(() => { if (following) toBottom(); });
    observer.observe(content); observer.observe(container);
    return () => observer.disconnect();
  });
  async function showUnread() {
    const target = sessions.find(session => session.id === selected && unread(session)) ?? sessions.find(session => unread(session));
    if (!target) return;
    if (target.id !== selected) await choose(target);
    await tick(); toBottom();
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
  function rememberDetail() {
    if (detail && displayedScope?.chat === detail.id) {
      detailCache.put(sessionCacheKey(displayedScope.workspace, displayedScope.folder, detail.id), detail);
    }
  }
  function selectSession(ws: string, folder: string, target: string) {
    rememberDetail();
    readSequence++;
    selected = target;
    displayedScope = { workspace: ws, folder, chat: target };
    detail = detailCache.get(sessionCacheKey(ws, folder, target));
    loadingDetail = !detail;
    following = true; activeMessage = ''; error = ''; readError = '';
    activateDraft(ws, folder, target);
    const gen = generation;
    void tick().then(() => { if (gen === generation && selected === target) toBottom(); });
  }
  async function readSession(ws: string, folder: string, target: string, gen: number) {
    const request = ++readSequence;
    const current = () => gen === generation && selected === target && request === readSequence;
    try {
      const result = await localChat(ws, folder, { action: 'read', chat_id: target });
      if (!current()) return;
      if (!result.session || result.session.id !== target) throw new Error('Chat session not found');
      const switched = detail?.id !== target;
      detail = result.session;
      displayedScope = { workspace: ws, folder, chat: target };
      detailCache.put(sessionCacheKey(ws, folder, target), result.session);
      loadingDetail = false;
      if (error === readError) error = '';
      readError = '';
      await tick();
      if (current() && (switched || following)) toBottom();
      if (current()) markRead();
    } catch (e) {
      if (current()) { loadingDetail = false; readError = String(e); error = readError; }
    }
  }
  async function refresh(ws: string, folder: string, gen: number) {
    const list = await localChat(ws, folder, { action: 'list' });
    if (gen !== generation) return;
    sessions = list.sessions ?? [];
    if (!selected && requestedChatId) selectSession(ws, folder, requestedChatId);
    else if (!selected && sessions.length && !startNew) {
      selectSession(ws, folder, sessions[0].id); onNavigate?.(folder, selected);
    }
    const target = selected;
    if (target) await readSession(ws, folder, target, gen);
  }
  $effect(() => {
    if (requestedFolderId && folders.some(f => f.id === requestedFolderId)) folderId = requestedFolderId;
    else if (!folders.some(f => f.id === folderId)) folderId = folders.find(f => f.id === activeFolderId)?.id ?? folders[0]?.id ?? '';
  });
  $effect(() => {
    const ws = workspaceId, folder = folderId; const gen = ++generation;
    untrack(() => { persistDraft(); rememberDetail(); }); activeDraftKey = ''; draftStorageError = false;
    readSequence++; displayedScope = null; loadingDetail = false; readError = '';
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
  $effect(() => {
    const ws = workspaceId, target = requestedChatId, folder = folderId, fresh = startNew;
    untrack(() => {
      if (folder && target && selected !== target) { selectSession(ws, folder, target); void readSession(ws, folder, target, generation); }
      else if (!target && fresh) { rememberDetail(); readSequence++; selected = ''; detail = null; displayedScope = null; loadingDetail = false; activateDraft(ws, folder, 'new'); }
    });
  });
  async function choose(s: ChatSession) {
    selectSession(workspaceId, folderId, s.id); onNavigate?.(folderId, s.id); guide = false;
    await readSession(workspaceId, folderId, s.id, generation);
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
        if (selected === target) { readSequence++; detail = result.session; }
      }
      renameId = ''; renameTitle = '';
      await refresh(workspaceId, folderId, gen);
    } catch (e) { if (gen === generation) error = String(e); }
    finally { busy = false; }
  }
  async function create(keepDraft = false) {
    if (busy || !folderId) return; busy = true; error = ''; const gen = generation, previous = selected;
    try {
      const result = await localChat(workspaceId, folderId, { action: 'create', title: '新对话',mode:newMode });
      if (gen !== generation || selected !== previous) return;
      readSequence++;
      detail = result.session ?? null; selected = detail?.id ?? '';
      displayedScope = selected ? { workspace: workspaceId, folder: folderId, chat: selected } : null;
      if (selected) { const unsent = draft; activateDraft(workspaceId, folderId, selected); if (keepDraft) { draft = unsent; persistDraft(); chatDrafts.save(chatDraftKey(workspaceId, folderId, 'new'), {text:'',attachments:[],retry:null}); } }
      guide = false;
      await refresh(workspaceId, folderId, gen);
      if (selected) onNavigate?.(folderId, selected);
      if (!keepDraft && selected) { setTimeout(() => { if (gen === generation && selected === result.session?.id) void openConnection(); }, 0); }
    } catch(e) { if (gen === generation) error = String(e); } finally { busy = false; }
  }
  async function send() {
    if (messageBytes(draft) > 32000) { error = $t('chat.tooLong'); return; }
    if (!selected && !busy && draft.trim()) await create(true);
    if (busy || (!draft.trim() && !attachments.length) || !selected || !detail || detail.closed) return;
    busy = true; error = ''; const gen = generation; const target = selected; const content = draft.trim(); const cacheKey = activeDraftKey;
    attachments = attachmentCandidates([...attachments,...referencedAttachments(content,mentionFiles)]).concat(attachments.filter(file=>!file.label));
    const attachmentKey = attachments.map(f => f.id).join(',');
    if (!retry || retry.text !== content || retry.attachmentKey !== attachmentKey) retry = { text: content, id: randomId(), attachmentKey };
    const sent = retry; persistDraft();
    try {
      const result = await localChat(workspaceId, folderId, { action: 'send', chat_id: target, message_id: sent.id, text: content, attachment_ids: attachments.map(f => f.id) });
      const cleared = chatDrafts.clearSent(cacheKey, sent);
      if (gen !== generation || selected !== target) return;
      readSequence++;
      detail = result.session ?? null;
      if (cleared.cleared) { draft = ''; retry = null; attachments = []; draftStorageError = !cleared.persisted; }
      guide = false;
      await tick(); toBottom();
      onNavigate?.(folderId, target);
    } catch(e) { if (gen === generation) error = String(e); } finally { busy = false; }
  }
  async function attachReference() {
    const source = referenceInput.trim();
    if (!source || busy) return;
    if (!selected) await create(true);
    if (!selected || busy || !detail || detail.closed) return;
    const ws = workspaceId, folder = folderId, chat = selected, gen = generation;
    const key = JSON.stringify([ws, folder, chat, source]);
    if (referenceRetry?.key !== key) referenceRetry = {key, id:randomId()};
    busy = true; uploading = true; error = '';
    try {
      const result = await localChat(ws, folder, {action:'upload', chat_id:chat, upload_id:referenceRetry.id, name:source.split('/').at(-1), source_path:source});
      if (gen !== generation || selected !== chat) return;
      if (result.attachment && !attachments.some(file => file.id === result.attachment?.id)) attachments = [...attachments, result.attachment];
      persistDraft(); referenceOpen = false; referenceInput = ''; referenceRetry = null;
    } catch (e) { if (gen === generation) { error = String(e); referenceInput = source; referenceOpen = true; } }
    finally { busy = false; uploading = false; }
  }
  function pasteImages(event: ClipboardEvent) {
    const files = Array.from(event.clipboardData?.files ?? []).filter(file => file.type.startsWith('image/'));
    if (!files.length || busy || detail?.closed) return;
    event.preventDefault();
    void uploadFiles(files, true);
  }
  async function uploadFiles(files: FileList | File[] | null, appendMentions = false) {
    if (files?.length && !selected && !busy) { uploading = true; try { await create(true); } finally { uploading = false; } }
    if (!files?.length || busy || !selected || !detail || detail.closed) return;
    if ([...files].some(f => !f.size)) { error = $t('chat.emptyFile'); return; }
    busy = true; uploading = true; error = ''; const gen = generation, target = selected, folder = folderId, ws = workspaceId, cacheKey = activeDraftKey;
    persistDraft();
    try {
      for (const file of [...files]) {
        const attachment = await uploadLocalFile(args=>localChat(ws,folder,args),target,file,randomId());
        const result = {attachment};
        if (result.attachment) {
          const cached = chatDrafts.load(cacheKey).draft;
          if (!cached.attachments.some(file => file.id === result.attachment!.id)) cached.attachments.push(result.attachment);
          if (appendMentions) cached.text = appendAttachmentMention(cached.text, result.attachment);
          chatDrafts.save(cacheKey, cached);
        }
        if (gen !== generation || target !== selected) return;
        if (result.attachment) {
          attachments = [...attachments, result.attachment];
          if (appendMentions) draft = appendAttachmentMention(draft, result.attachment);
          persistDraft();
        }
      }
    } catch(e) { if (gen === generation) error = String(e); } finally {
      busy = false; uploading = false; if (fileInput) fileInput.value = '';
      await tick();
      if (gen === generation && target === selected && [document.body, fileInput, composer].includes(document.activeElement as HTMLElement)) composer?.focus({ preventScroll: true });
    }
  }
  async function detach() {
    if (busy || !selected || !detail || detail.closed) return;
    busy = true; error = ''; const gen = generation, target = selected;
    try {
      const result = await localChat(workspaceId, folderId, { action: 'detach', chat_id: target });
      if (gen === generation && target === selected) { readSequence++; detail = result.session ?? null; guide = false; }
    } catch (e) { if (gen === generation) error = String(e); } finally { busy = false; }
  }
  async function copyPrompt() {
    if (!connectionPrompt || connectionLoading) return;
    try { await copyText(connectionPrompt); copied = true; }
    catch { connectionError = $t('chat.42'); }
  }
  function download() {
    if (!detail) return;
    const text = `# ${detail.title}\n\nSession: ${detail.id}\n\n` + messages.map(m => { const receipt = chatUserState(detail, m.id); return `## ${m.role === 'user' ? $t('chat.26') : m.agent_name??detail?.agent_name??'AI'} · ${new Date(m.created_at).toLocaleString()}\n\n${m.tool_event ? `Tool (AI reported): ${m.tool_event.name} · ${m.tool_event.status}\n\n` : ''}${m.text}\n${taskPlanMarkdown(m.task_plan)}${(m.agent_plans??[]).map(p=>'\nAgent: '+(detail?.members?.find(a=>a.id===p.agent_id)?.name??p.agent_id)+'\n'+taskPlanMarkdown(p.plan)).join('')}${m.recipient_ids?.length?'\nRecipients: '+m.recipient_ids.map(id=>detail?.members?.find(a=>a.id===id)?.name??id).join(', ')+'\n':''}${receipt ? `\n${$t('chat.84')}：${$t(receipt.read ? 'chat.79' : 'chat.80')} · ${$t(USER_MESSAGE_STATUS_KEYS[receipt.status])}\n` : ''}${m.role === 'assistant' ? `\nReply state: ${m.awaiting_user ? 'awaiting_user' : m.final === false ? 'supplementing' : 'complete'}\n` : ''}${m.tool_event?.input ? `\nInput:\n${m.tool_event.input}\n` : ''}${m.tool_event?.output ? `\nOutput:\n${m.tool_event.output}\n` : ''}${m.tool_event?.output_truncated ? '\nOutput truncated\n' : ''}${(m.attachments ?? []).map(f => `\nAttachment: ${f.name} (${f.size} bytes)\nPath: ${f.path}\nReference: @${f.label ?? f.name}\n`).join('')}`; }).join('\n');
    const url = URL.createObjectURL(new Blob([text], {type:'text/markdown;charset=utf-8'}));
    const a = document.createElement('a'); a.href=url; a.download=`chat-${detail.id}.md`; a.click(); setTimeout(() => URL.revokeObjectURL(url),1000);
  }
</script>

<svelte:document onvisibilitychange={markRead}/>
<svelte:window onpagehide={persistDraft}/>
<section class="chat-shell" class:group-mode={mode==='group'} class:tasks-visible={tasksOpen} style:--chat-task-width={`${tasksWidth}px`} class:external-navigation={externalNavigation} class:landing={emptyComposer} aria-label={$t("chat.35")}>
  {#if !externalNavigation}<aside class="session-sidebar">
    <div class="sidebar-heading"><span>{$t("chat.0")}</span><span class="beta">{$t("chat.48")}</span></div>
    <button class="new-chat" onclick={() => create()} disabled={busy || !folderId}><Plus size={16}/>{$t("chat.1")}</button>
    <label class="folder-label" for="chat-folder">{$t("chat.2")}</label>
    <select id="chat-folder" aria-label={$t("chat.2")} bind:value={folderId} disabled={busy}>{#each folders as folder}<option value={folder.id}>{folder.name}</option>{/each}</select>
    <div class="section-label">{$t("chat.3")}</div>
    <nav aria-label={$t("chat.3")}>{#each sessions as s (s.id)}
      {@const presence = sessionPresence(selected === s.id && detail ? detail : s)}
      <div class="session-row" class:selected={selected === s.id} data-presence={presence}>
        {#if unread(s)}<span class="unread-badge" title={`${$t("chat.90")}: ${unread(s)}`} aria-label={`${$t("chat.90")}: ${unread(s)}`}>{unread(s) > 99 ? '99+' : unread(s)}</span>{/if}
        {#if renameId === s.id}
          <form class="session-rename" onsubmit={(event) => { event.preventDefault(); void saveRename(); }}>
            <input bind:this={renameInput} bind:value={renameTitle} aria-label={$t('chat.86')} maxlength="240" disabled={busy} onkeydown={(event) => { if (event.key === 'Escape') { event.preventDefault(); cancelRename(); } if (event.key === 'Enter' && event.isComposing) event.preventDefault(); }}/>
            <button type="submit" class="rename-action" disabled={busy || !renameTitle.trim()} title={$t('Save')} aria-label={$t('Save')}><Check size={14}/></button>
            <button type="button" class="rename-action" disabled={busy} onclick={cancelRename} title={$t('Cancel')} aria-label={$t('Cancel')}><X size={14}/></button>
          </form>
        {:else}
          <button class="session-choice" class:active={selected === s.id} onclick={() => choose(s)} disabled={busy} title={`${s.title} · ${$t(presenceKeys[presence])}`} aria-label={`${s.title} · ${$t(presenceKeys[presence])}`}><MessageSquare size={15}/><span>{s.title}</span><i class="session-presence" title={$t(presenceKeys[presence])} aria-hidden="true"></i>{#if s.closed}<span class="ended">{$t("chat.4")}</span>{/if}</button>
          <button type="button" class="rename-action" disabled={busy} onclick={() => void startRename(s)} title={$t('chat.85')} aria-label={`${$t('chat.85')}: ${s.title}`}><Pencil size={13}/></button>
        {/if}
      </div>
    {/each}</nav>
    <div class="local-note"><span class="dot"></span>{$t("chat.5")}</div>
  </aside>{/if}
  <div class="conversation" style:--conversation-accent={conversationAccent(selected)}>
    <header class="chat-header">
      <div class="chat-heading"><strong title={detail?.title}>{detail?.title ?? selectedFolder?.name ?? $t('chat.23')}</strong><span class="status"><i class:online={detail?.status === 'waiting' || detail?.status === 'connected'}></i>{statusLabel}</span></div>
      <div class="header-actions">
        {#if unreadTotal}<button class="unread-total" onclick={showUnread} aria-label={`${$t('chat.90')}: ${unreadTotal}`}>{unreadTotal}</button>{/if}
        <div class="connection-actions"><button disabled={busy || !selected || !(detail?.status === 'connected' || detail?.status === 'waiting')} onclick={requestDisconnect}>{$t('chat.disconnect')}</button><button class="connect-button" class:needs-ai={needsAi} title={needsAi?$t('chat.connectHint'):undefined} disabled={busy || !folderId || detail?.closed} onclick={openConnection}>{$t('chat.connect')}{#if needsAi}<i class="attention-dot" aria-hidden="true"></i>{/if}</button></div>
        {#if selected}<button title={$t('chat.36')} aria-label={$t('chat.36')} onclick={download}><Download size={16}/></button>{/if}
        {#if selected && detail && !detail.closed && !isNewConversation}<button type="button" title={$t('chat.mode')} aria-label={$t('chat.mode')} popovertarget="conversation-mode"><Settings size={16}/></button><div id="conversation-mode" class="mode-menu" popover="auto"><strong>{$t('chat.mode')}</strong><div class="mode-switch"><button type="button" aria-pressed={mode==='work'} disabled={busy||memberBusy} onclick={()=>changeMode('work')}>{$t('chat.work')}</button><button type="button" aria-pressed={mode==='group'} disabled={busy||memberBusy} onclick={()=>changeMode('group')}>{$t('chat.group')}</button></div><p>{$t('chat.modeReturnHint')}</p>{#if error}<p role="alert">{error}</p>{/if}<button type="button" popovertarget="conversation-mode" popovertargetaction="hide">{$t('Close')}</button></div>{/if}
        {#if headerActions}{@render headerActions()}{/if}
      </div>
    </header>
    {#if error}<div class="chat-error" role="alert">{error}<button onclick={() => error = ''} aria-label={$t("chat.38")}>×</button></div>{/if}
    {#if isNewConversation}<div class="mode-switch" aria-label={$t('chat.mode')}><button type="button" aria-pressed={mode==='group'} disabled={busy||memberBusy||detail?.closed} onclick={()=>changeMode('group')}>{$t('chat.group')}</button><button type="button" aria-pressed={mode==='work'} disabled={busy||memberBusy||detail?.closed} onclick={()=>changeMode('work')}>{$t('chat.work')}</button></div>{/if}
    <div class="conversation-body">
    <div class="feed-frame">
    <div class="message-feed" bind:this={feed} onscroll={updateScroll} aria-busy={loadingDetail}>
      {#if selected && !detail}<div class="chat-loading" role="status">{loadingDetail ? $t("Loading…") : error}</div>
      {:else if !messages.length}<div class="empty-state"><h2>{$t('chat.102')}</h2></div>
      {:else}<div class="message-column" bind:this={column}>{#each feedItems as item (item.id)}{#if item.kind === 'tools'}<article class="assistant grouped-tools"><div class="message-meta">{item.messages[0]?.agent_name ?? 'AI'} · {$t("chat.54")}</div><div class="message-body"><ChatToolActivity messages={item.messages} settled={item.settled} {workspaceId} {folderId} chatId={selected}/></div></article>{:else}{@const m = item.message}<article data-user-message={m.role === 'user' ? m.id : undefined} tabindex="-1" class:user={m.role === 'user'} class:assistant={m.role === 'assistant'}><div class="message-meta">{m.role === 'user' ? $t('chat.26') : m.agent_name ?? detail?.agent_name ?? (m.final === false ? $t('chat.27') : 'AI')}<time>{new Date(m.created_at).toLocaleTimeString([], {hour:'2-digit',minute:'2-digit'})}</time></div><div class="message-body"><ChatMarkdown text={m.text} attachments={m.attachments ?? []} {workspaceId} {folderId} chatId={selected}/>{#each m.attachments ?? [] as file (workspaceId + ":" + folderId + ":" + selected + ":" + file.id)}<ChatAttachment {workspaceId} {folderId} chatId={selected} {file}/>{/each}<ChatReplyState state={chatReplyState(detail, m.id)}/><ChatUserState state={chatUserState(detail, m.id)}/></div></article>{/if}{/each}{#if pendingState}<div class="pending" class:processing={pendingState === 'processing'} role="status" aria-live="polite"><ChatStatusIcon state={pendingState} label={pendingState === 'awaiting_user' ? $t('chat.73') : pendingState === 'processing' ? $t('chat.70') : pendingState === 'interrupted' ? $t('chat.71') : $t('chat.29')}/>{pendingState === 'awaiting_user' ? $t('chat.73') : pendingState === 'processing' ? $t('chat.70') : pendingState === 'interrupted' ? $t('chat.71') : $t('chat.29')}</div>{/if}</div>{/if}
    </div>
    <ChatOutline {messages} activeId={activeMessage} onSelect={jumpToMessage}/>
    </div>
    <footer class="composer-area">
      {#if detail?.queued_messages?.length}<div class="outbox" aria-label={$t('chat.outbox')}>
        <button type="button" class="queue-mode" disabled={queueChanging||loadingDetail||detail.closed} aria-pressed={detail.queue_mode!=='split'} aria-busy={queueChanging} onclick={toggleQueueMode} title={$t(detail.queue_mode==='split'?'chat.queueSplitHint':'chat.queueMergeHint')} aria-label={$t(detail.queue_mode==='split'?'chat.queueSplitHint':'chat.queueMergeHint')}>{$t(detail.queue_mode==='split'?'chat.queueSplit':'chat.queueMerge')}</button>
        <div class="queued-items" aria-live="polite">
          {#if detail.queue_mode !== 'split'}
            <details class="queued-card merged-queue"><summary><span>{$t('chat.queueItem')} · {$t('chat.queueMerge')} ({detail.queued_messages.length})</span><span>{detail.queued_messages.map(item=>item.text).join(' · ')}</span></summary>
              <div>{#each detail.queued_messages as item,i (item.id)}<section><strong>{$t('chat.queueItem')} {i+1}</strong><p>{item.text}</p>{#if item.attachments?.length}<small>{item.attachments.map(f=>f.label??f.name).join(' · ')}</small>{/if}</section>{/each}</div>
            </details>
          {:else}
            {#each detail.queued_messages as item,i (item.id)}<details class="queued-card"><summary><span>{$t('chat.queueItem')} {i+1}</span><span>{item.text}</span></summary><div><p>{item.text}</p>{#if item.attachments?.length}<small>{item.attachments.map(f=>f.label??f.name).join(' · ')}</small>{/if}</div></details>{/each}
          {/if}
        </div>
      </div>{/if}{#if referenceOpen}<div class="reference-input"><input aria-label={$t('chat.123')} placeholder="mcp-assistant/artifacts/file.png" bind:value={referenceInput} disabled={busy} onkeydown={event=>{if(event.key==='Enter'){event.preventDefault();void attachReference()}}}/><button type="button" onclick={attachReference} disabled={busy||!referenceInput.trim()}>{$t('chat.122')}</button></div>{/if}<div class="bottom-control"><button type="button" class="jump-bottom" onclick={toBottom} title={$t('chat.96')} aria-label={$t('chat.96')} aria-pressed={following}><ArrowDown size={16}/></button></div><form onsubmit={(e) => { e.preventDefault(); void send(); }}>{#if attachments.length}<div class="draft-files">{#each attachments as file (file.id)}<DraftAttachment {file} {workspaceId} {folderId} chatId={selected} disabled={busy} onRemove={()=>removeAttachment(file)}/>{/each}</div>{/if}<div class="composer-input"><div class="mention-input"><div class="draft-highlight" bind:this={draftHighlight} aria-hidden="true">{#each highlightedDraft as part}{#if part.file}<span class="attachment-mention">{part.text}</span>{:else}{part.text}{/if}{/each}{'\n'}</div><textarea use:autoGrow={[draft, mode]} bind:this={composer} aria-label={$t("chat.39")} bind:value={draft} oninput={(event) => { draft = event.currentTarget.value; updateCaret(); mentionFocused=true; persistDraft(); }} onfocus={()=>{mentionFocused=true;updateCaret()}} onblur={()=>mentionFocused=false} onclick={()=>{mentionFocused=true;updateCaret()}} onkeyup={updateCaret} onscroll={()=>{if(draftHighlight){draftHighlight.scrollTop=composer.scrollTop;draftHighlight.scrollLeft=composer.scrollLeft}}} onpaste={pasteImages} placeholder={detail?.closed ? $t('chat.30') : $t('chat.31')} disabled={!folderId || detail?.closed || (busy && !uploading)} onkeydown={composerKeys} rows={mode==='group'?1:3} aria-autocomplete="list" aria-controls="attachment-choices"></textarea></div>{#if mention && choices.length}<div class="mention-choices" id="attachment-choices" role="listbox" aria-label={$t('chat.attachmentChoices')}>{#each choices as file,i (file.id)}<button type="button" role="option" aria-selected={i===mentionIndex} class:active={i===mentionIndex} onpointerdown={event=>event.preventDefault()} onclick={()=>insertMention(file)}><strong>@{file.label}</strong><span>{file.name}</span></button>{/each}</div>{/if}</div><input class="file-input" type="file" multiple bind:this={fileInput} onchange={() => uploadFiles(fileInput.files)} aria-label={$t('chat.50')}/><div class="composer-toolbar"><span class="attach-trigger" role="group" onpointerenter={()=>tonguePointer=true} onpointerleave={()=>tonguePointer=false} onfocusin={()=>tongueFocused=true} onfocusout={()=>tongueFocused=false}><button class="attach-button" type="button" disabled={busy || !folderId || detail?.closed} title={$t('chat.51')} aria-label={$t('chat.50')} onclick={() => fileInput.click()}><Plus size={19}/></button></span><div><button class="send-button" aria-label={$t("chat.41")} disabled={messageBytes(draft)>32000 || busy || (!!selected && !detail) || (!draft.trim() && !attachments.length) || !folderId || detail?.closed}><ArrowUp size={18}/></button></div></div></form><div class="composer-tongue" class:revealed={tongueVisible} inert={!tongueVisible} role="group" aria-label={$t('chat.project')} onpointerenter={()=>tonguePointer=true} onpointerleave={()=>tonguePointer=false} onfocusin={()=>tongueFocused=true} onfocusout={()=>tongueFocused=false}>
      <div class="project-picker"><button type="button" disabled={busy||detail?.closed} aria-expanded={projectOpen} onclick={()=>projectOpen=!projectOpen}><Folder size={14}/><span>{selectedFolder?.name??$t('chat.project')}</span><ChevronDown size={12}/></button>{#if projectOpen}<div class="project-options">{#each folders as folder}<button type="button" onclick={()=>selectProject(folder.id)} class:chosen={folder.id===folderId}>{folder.name}</button>{/each}</div>{/if}</div>
      <button type="button" disabled={busy||!folderId||detail?.closed} onclick={()=>fileInput.click()}>{$t('chat.files')}</button>
      <button type="button" disabled={busy||!onPlugins||detail?.closed} onclick={()=>{persistDraft();onPlugins?.(folderId)}}>{$t('chat.plugins')}</button>
      <button type="button" onclick={()=>referenceOpen=!referenceOpen} disabled={busy||detail?.closed}>{$t("chat.122")}</button>
    </div>{#if mode==='group'}<ChatMembers members={detail?.members??[]} busy={busy||memberBusy||!!detail?.closed} onMention={mentionAgent} onConnect={openConnection} onChange={memberAction}/>{/if}{#if draftStorageError}<p class="draft-warning" role="status">{$t("chat.94")}</p>{/if}{#if messageBytes(draft)>32000}<p class="draft-warning" role="alert">{$t('chat.tooLong')} ({messageBytes(draft)} / 32000)</p>{/if}<div class="composer-hint">{$t("chat.18")} <span>{detail?.archive_path ?? $t('chat.34')}</span></div></footer>
    </div>
  </div>
  {#if tasksOpen}<ChatTaskPanel {workspaceId} bind:width={tasksWidth} session={detail} expanded={tasksExpanded} onClose={onCloseTasks} onToggleExpanded={onToggleTasksExpanded}/>{/if}
</section>
<dialog class="connect-dialog" bind:this={connectDialog} onclose={() => { guide = false; connectionPrompt = ''; }}>
  <header><h2>{$t('chat.connect')}</h2><button aria-label={$t('Close')} onclick={()=>guide=false}><X size={18}/></button></header>
  {#if connectionPeer || connectionComplete}
    <div class="connection-success" role="status"><Check size={18}/><strong>{$t('chat.connectedSuccess')}</strong></div>
    <p class="connection-id">{$t('chat.connectionId')}: <code>{connectionPeer?.id??detail?.connection_id??selected}</code>{#if connectionPeer?.name} · {connectionPeer.name}{/if}</p>
    {#if connectionComplete && connectionGreeting}<p class="connection-greeting">{connectionGreeting}</p>
    {:else if !connectionComplete}<p role="status">{$t('chat.waitGreeting')}</p>{/if}
  {/if}
  {#if !connectionComplete}
    <p>{$t('chat.sendInstruction')}</p>
    <textarea class="connection-prompt" readonly aria-label={$t('chat.9')} value={connectionLoading ? $t('Loading…') : connectionPrompt}></textarea>
    {#if !connectionLoading && connectionRequest && !connectionPeer}<p class="pairing-hint" role="status">{$t('chat.pairing')} {$t('chat.pairingHint')}</p>{/if}
    {#if pairing === 'failed'}<p role="alert">{$t('chat.pairingFailed')}</p>{/if}
  {/if}
  <div class="connection-buttons">
    {#if !connectionComplete}<button onclick={copyPrompt} disabled={connectionLoading || connectionSending || !connectionPrompt}>{#if copied}<Check size={14}/>{:else}<Copy size={14}/>{/if}{copied ? $t('chat.25') : $t('chat.24')}</button>{/if}
    {#if connectionError || pairing==='failed'}<button onclick={openConnection} disabled={connectionLoading || connectionSending}>{$t('shell.resume')}</button>{/if}
    <button onclick={()=>guide=false}>{$t(connectionComplete?'chat.showConversation':'chat.backConversation')}</button>
  </div>
  {#if connectionError}<p class="connection-error" role="alert">{connectionError}</p>{/if}
</dialog>
<dialog class="disconnect-dialog" bind:this={disconnectDialog} onclose={()=>disconnectAt=0}>
  <h2>{$t('chat.88')}</h2><p>{$t('shell.disconnectNotice')}</p>
  <div><button onclick={()=>disconnectDialog?.close()}>{$t('Cancel')}</button><button class="confirm-disconnect" disabled={disconnectSeconds>0 || busy} onclick={confirmDisconnect}>{$t('chat.88')}{disconnectSeconds>0 ? ` (${disconnectSeconds})` : ''}</button></div>
</dialog>
<style>
.mode-menu{margin:auto;max-width:340px;padding:20px;background:#252525;color:#ddd;border:1px solid #ffffff25;border-radius:12px}.mode-menu::backdrop{background:#0005}.mode-menu p{font-size:12px;line-height:1.6;color:#aaa}.mode-menu>button{padding:6px 12px;border:1px solid #ffffff25;border-radius:6px}
.chat-loading{padding:32px 40px;color:var(--color-text-muted);font-size:12px}
.reference-input{display:flex;gap:8px;max-width:760px;margin:0 auto 10px;font-size:11px}.reference-input input{min-width:0;flex:1;padding:8px;border:1px solid var(--color-border);border-radius:7px;background:var(--card-bg)}.reference-input button{padding:8px;border:1px solid var(--color-border);border-radius:7px}



.file-input{display:none}.draft-files{display:flex;flex-wrap:wrap;gap:8px;font-size:11px}

.chat-shell{container-type:inline-size;position:relative;height:100%;min-height:0;display:grid;grid-template-columns:218px minmax(0,1fr);border:1px solid var(--color-border);border-radius:16px;overflow:hidden;background:var(--card-bg);color:var(--color-text)}button{cursor:pointer}button:disabled{cursor:default;opacity:.45}button:focus-visible,select:focus-visible,textarea:focus-visible{outline:2px solid var(--primary);outline-offset:3px}.session-sidebar{padding:22px 14px 14px;background:var(--color-bg);display:flex;flex-direction:column;border-right:1px solid var(--color-border);min-height:0}.sidebar-heading{display:flex;align-items:center;justify-content:space-between;font-size:13px;font-weight:600;padding:0 9px 20px}.beta{font:9px monospace;letter-spacing:1px;color:var(--color-text-muted);border:1px solid var(--color-border);padding:3px 5px;border-radius:4px}.new-chat{display:flex;gap:9px;align-items:center;border:1px solid var(--color-border);border-radius:9px;padding:10px 12px;background:var(--card-bg);font-size:12px}.folder-label,.section-label{font-size:10px;color:var(--color-text-muted);margin:22px 8px 8px;display:block}.section-label{margin-top:28px}select{font-size:11px;width:100%;padding:9px;border:1px solid var(--color-border);border-radius:7px;background:var(--card-bg)}nav{overflow:auto;flex:1}nav button{display:flex;align-items:center;gap:8px;width:100%;text-align:left;padding:11px 9px;margin:3px 0;border-radius:7px;font-size:12px;color:var(--color-text-muted)}nav button.active{background:var(--card-bg);color:var(--color-text);box-shadow:0 1px 4px #00000008}nav button span:first-of-type{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.ended{font-size:9px;margin-left:auto;white-space:nowrap}.local-note{font-size:10px;color:var(--color-text-muted);padding:20px 6px 3px;display:flex;gap:7px;align-items:center}.dot{display:inline-block;width:5px;height:5px;border-radius:50%;background:#83a18e}.conversation{display:flex;flex-direction:column;min-width:0;min-height:0}.chat-header{padding:19px 26px;display:flex;justify-content:space-between;align-items:center;border-bottom:1px solid var(--color-border);gap:12px}.chat-header>div:first-child{display:flex;align-items:center;gap:17px;flex-wrap:wrap}.chat-header strong{font-size:13px;font-weight:550}.status{font-size:10px;color:var(--color-text-muted);display:flex;align-items:center;gap:6px}.status i{width:6px;height:6px;border-radius:50%;background:#989b98}.status i.online{background:#49a476;box-shadow:0 0 0 3px #49a47618}.header-actions{display:flex;gap:14px;align-items:center;font-size:11px;white-space:nowrap;color:var(--color-text-muted)}.message-feed{flex:1;overflow-y:auto;min-height:0;scroll-behavior:auto}.empty-state{max-width:600px;margin:0 auto;padding:65px 25px 40px;text-align:center}.empty-state h2{font-weight:500;font-size:30px;letter-spacing:-1px;margin:15px 0}.message-column{max-width:800px;margin:auto;padding:25px 40px 30px}article{margin-bottom:26px}.message-meta{display:flex;align-items:center;gap:10px;font-size:11px;font-weight:600;margin-bottom:9px}.message-meta time{font-size:9px;color:var(--color-text-muted);font-weight:400}.user{margin-left:14%}.user .message-meta{justify-content:flex-end}.user .message-body{background:color-mix(in srgb,#6e92ba 9%,var(--card-bg));border:1px solid var(--color-border);padding:12px 17px;border-radius:14px 14px 3px 14px}.assistant .message-body{padding:12px 17px;background:color-mix(in srgb,#829887 6%,var(--card-bg));border:1px solid var(--color-border);border-radius:14px 14px 14px 3px}.pending{font-size:11px;color:var(--color-text-muted);display:flex;align-items:center;gap:8px}.composer-area{padding:12px 30px 17px}.composer-area form{max-width:760px;margin:0 auto;border:1px solid var(--color-border);border-radius:17px;padding:14px 16px 10px;box-shadow:0 3px 15px #00000004}.composer-area textarea{display:block;resize:none;width:100%;background:transparent;border:0;font:13px/1.7 inherit;color:var(--color-text);outline:none;min-height:62px}.composer-toolbar{display:flex;justify-content:space-between;align-items:center;margin-top:7px;font-size:10px;color:var(--color-text-muted)}.composer-toolbar>span{display:flex;align-items:center;gap:7px}.composer-toolbar>div{display:flex;gap:14px;align-items:center}.send-button{display:grid;place-items:center;width:31px;height:31px;border-radius:50%;background:var(--color-text);color:var(--card-bg)}.composer-hint{max-width:760px;margin:10px auto 0;display:flex;justify-content:space-between;gap:10px;font-size:9px;color:var(--color-text-muted)}.composer-hint span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:50%}.chat-error{display:flex;justify-content:space-between;gap:10px;margin:12px 25px 0;padding:10px;border:1px solid var(--danger);color:var(--danger);border-radius:8px;font-size:11px;overflow-wrap:anywhere}@media(max-width:800px){.chat-shell{grid-template-columns:160px minmax(0,1fr)}.message-column{padding:20px}.chat-header{padding:15px}.composer-area{padding:10px 15px}.empty-state{padding-top:45px}}@media(max-width:560px){.chat-shell{grid-template-columns:1fr;height:100%;min-height:0}.session-sidebar{padding:10px;display:grid;grid-template-columns:1fr 1fr;gap:8px;border-right:0;border-bottom:1px solid var(--color-border)}.sidebar-heading,.folder-label,.section-label,.local-note{display:none}nav{grid-column:1/-1;display:flex;max-height:40px;gap:6px}nav button{width:auto;min-width:100px}.new-chat{padding:7px 10px}.chat-header{padding:12px}.empty-state{padding:25px 20px}.empty-state h2{font-size:24px}.composer-hint span{display:none}.message-column{padding:15px}}
.session-row{display:flex;align-items:center;gap:2px;min-width:0}.session-choice{flex:1;min-width:0}.session-choice :global(svg){flex-shrink:0}nav .rename-action{width:25px;flex:0 0 25px;justify-content:center;padding:6px 3px;margin:0}.session-rename{display:flex;gap:2px;align-items:center;min-width:0;width:100%;padding:5px 0}.session-rename input{min-width:0;width:100%;flex:1;border:1px solid var(--color-border);border-radius:5px;background:var(--card-bg);padding:7px 5px;font-size:12px;color:var(--color-text)}.session-rename input:focus-visible{outline:2px solid var(--primary);outline-offset:1px}@media(max-width:560px){.session-row{min-width:145px;max-width:300px;flex-shrink:0}.session-row:has(.session-rename){width:280px}nav:has(.session-rename){max-height:60px}nav .rename-action{min-width:25px}}
.session-row{border:1px solid transparent;border-radius:8px;margin:3px 0;background:transparent}.session-row.selected{background:var(--surface-hover,var(--card-bg));border-color:var(--border-strong,var(--color-border))}.session-row.selected .session-choice{color:var(--color-text)}.session-choice.active{background:transparent;box-shadow:none}.session-presence{width:6px;height:6px;border-radius:50%;flex:0 0 6px;background:#8b8d92;margin-left:auto}.session-row[data-presence="online"] .session-presence{background:#31a56c}.session-row[data-presence="working"] .session-presence{background:#6a9bd4}.session-row[data-presence="error"] .session-presence{background:#db7969}
.session-row{position:relative}.unread-badge{position:absolute;top:-3px;right:0;min-width:15px;height:15px;padding:0 4px;border-radius:9px;background:#55749a;color:white;font:9px/15px system-ui;text-align:center;pointer-events:none;z-index:1}.unread-total{display:flex;align-items:center;gap:5px;color:var(--color-text)}.header-actions{flex-wrap:wrap;justify-content:flex-end;row-gap:6px}
.grouped-tools{margin-bottom:16px}.grouped-tools .message-body{padding:9px 12px;background:transparent}
.conversation{background:color-mix(in srgb,var(--conversation-accent) 8%,var(--card-bg))}.draft-warning{max-width:760px;margin:8px auto 0;font-size:10px;color:var(--color-text-muted)}
.feed-frame{position:relative;display:flex;flex:1;min-height:0}.message-feed{padding-right:30px}.bottom-control{display:flex;justify-content:center;margin:-4px 0 8px}.jump-bottom{display:grid;place-items:center;width:28px;height:28px;border-radius:50%;border:1px solid var(--color-border);background:var(--card-bg);color:var(--color-text-muted)}.jump-bottom:hover{color:var(--color-text);border-color:var(--border-strong)}.composer-input{display:flex;align-items:flex-start;gap:10px}.attach-button{flex:none;display:grid;place-items:center;width:28px;height:32px;color:#fff}.composer-input textarea{min-width:0;flex:1}.composer-area{padding-top:6px}.message-feed{overflow-anchor:none}article:focus{outline:none}article:focus-visible{outline:1px solid var(--color-border);outline-offset:6px}
@media(max-width:560px){.message-column{padding:15px 8px}.composer-hint{display:none}.chat-header{gap:6px}.session-sidebar{padding:8px}.composer-area{padding:6px 10px 10px}.header-actions{gap:8px}.message-feed{padding-right:24px}}
.external-navigation{grid-template-columns:minmax(0,1fr);border:0;border-radius:0}.external-navigation .conversation{background:var(--card-bg)}.external-navigation .assistant .message-body{background:transparent;border-color:transparent;padding-left:0}.external-navigation .user .message-body{background:var(--surface-hover,var(--color-bg))}.external-navigation .message-column{max-width:840px}.external-navigation .chat-header{padding:15px 28px}.external-navigation.landing .empty-state h2{font-size:30px}.external-navigation .composer-area{padding-bottom:20px}.external-navigation .composer-area form{background:var(--surface-hover,var(--color-bg));border-radius:20px}.external-navigation .composer-hint{opacity:.6}
.conversation,.external-navigation .conversation{background:#181818;color:#eee}.message-feed{padding-right:0;padding-left:24px;background:var(--color-bg)}
.assistant .message-body,.external-navigation .assistant .message-body{background:transparent;border:0;border-radius:0;padding:0}
.user .message-body,.external-navigation .user .message-body{background:#173e76;color:#fff;border:0}
.send-button{background:#2c67c5;color:#fff}.attach-button{border-radius:50%}.attach-button:hover{background:var(--surface-hover)}
.disconnect-dialog{margin:auto;max-width:min(420px,calc(100vw - 32px));padding:24px;border:1px solid var(--color-border);border-radius:14px;background:var(--color-bg);color:var(--color-text)}
.disconnect-dialog::backdrop{background:#0008}.disconnect-dialog h2{font-size:16px}.disconnect-dialog p{font-size:13px;line-height:1.7;margin:16px 0}.disconnect-dialog>div{display:flex;gap:12px;justify-content:flex-end}.disconnect-dialog button{padding:8px 12px;border-radius:7px;border:1px solid var(--color-border)}.confirm-disconnect{background:#2563eb;color:white}
.message-feed{background:#181818;color:#eee}.composer-area form,.external-navigation .composer-area form{background:#363636;color:#eee}

.chat-header{flex:none;min-height:60px;gap:12px}.chat-header .chat-heading{min-width:0;flex:1;gap:12px;flex-wrap:nowrap}.chat-heading strong{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.status{white-space:nowrap}.header-actions{flex-wrap:nowrap;gap:14px}.connection-actions{display:flex;border:1px solid #ffffff24;border-radius:9px;overflow:hidden}.connection-actions button{padding:7px 13px}.connection-actions button+button{border-left:1px solid #ffffff24}.connection-actions button:hover:enabled{background:#ffffff10}
.landing .feed-frame{flex:0 0 auto;margin-top:auto}.landing .empty-state{padding:20px 16px 24px}.landing .empty-state h2{margin:0;font-size:30px;font-weight:500}.landing .composer-area{margin-bottom:auto;padding-top:0;padding-bottom:60px}.landing .bottom-control,.landing .composer-hint{display:none}.landing .message-feed{padding:0;overflow:visible}.composer-area{background:#181818}.composer-area textarea{color:#eee}
.connect-dialog{margin:auto;width:min(560px,calc(100vw - 32px));max-height:calc(100dvh - 32px);overflow:auto;padding:24px;border:1px solid var(--color-border);border-radius:18px;background:var(--color-bg);color:var(--color-text);box-shadow:0 20px 80px #0007}.connect-dialog::backdrop{background:#0007;backdrop-filter:blur(5px)}.connect-dialog header{display:flex;justify-content:space-between;align-items:center;margin-bottom:18px}.connect-dialog h2{font-size:17px;font-weight:600}.connect-dialog p{font-size:13px;line-height:1.7;margin:12px 0}.connection-prompt{width:100%;height:220px;resize:vertical;padding:12px;border:1px solid var(--color-border);border-radius:10px;background:var(--card-bg);font:11px/1.7 monospace;color:var(--color-text)}.connection-buttons{display:flex;justify-content:space-between;gap:12px;margin-top:18px}.connection-buttons button{display:flex;align-items:center;gap:7px;padding:9px 14px;border:1px solid var(--color-border);border-radius:8px}.connection-success{display:flex;align-items:center;gap:8px;color:#8fcaa0}.connection-id code{user-select:all;overflow-wrap:anywhere}.connection-greeting{padding:12px;border-radius:8px;background:#ffffff08}.pairing-hint{color:var(--color-text-muted)}.connection-error{color:var(--danger);overflow-wrap:anywhere}
@media(max-width:700px){.chat-header,.external-navigation .chat-header{padding:12px}.chat-header .chat-heading{flex-direction:column;align-items:flex-start;gap:3px}.chat-heading strong{max-width:100%}.header-actions{gap:8px}.connection-actions button{padding:6px 8px}.landing .composer-area{padding-bottom:35px}}

.composer-input{position:relative}.mention-input{position:relative;min-width:0;flex:1;display:block}.mention-input textarea,.draft-highlight{grid-area:1/1;width:100%;box-sizing:border-box;padding:0;border:0;font:13px/1.7 system-ui;letter-spacing:normal;white-space:pre-wrap;overflow-wrap:break-word;tab-size:8}.draft-highlight{position:absolute;inset:0;height:100%;pointer-events:none;overflow:hidden;color:#eee;max-height:100%;min-height:62px}.mention-input textarea{position:relative;z-index:1;color:transparent!important;caret-color:#eee;background:transparent;resize:none}.attachment-mention{color:#79b5ff;background:#397ddd22;border-radius:3px}.mention-choices{position:absolute;bottom:calc(100% + 10px);left:0;z-index:15;width:min(340px,100%);max-height:220px;overflow:auto;border:1px solid #ffffff25;border-radius:12px;background:#262626;box-shadow:0 12px 32px #0007;padding:5px}.mention-choices button{display:flex;align-items:center;gap:12px;width:100%;text-align:left;padding:9px;border-radius:7px;font-size:12px}.mention-choices button.active,.mention-choices button:hover{background:#ffffff12}.mention-choices strong{color:#79b5ff;white-space:nowrap}.mention-choices span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:#aaa}.draft-files{margin-bottom:10px;flex-wrap:nowrap;overflow-x:auto;padding:4px 2px;max-height:150px}

.outbox{display:flex;align-items:flex-start;gap:10px;max-width:760px;margin:0 auto 10px;color:#ddd}.queue-mode{display:grid;place-items:center;flex:none;width:29px;height:29px;border:1px solid #ffffff30;border-radius:50%;font-size:12px;background:#292929}.queue-mode:hover{background:#3b3b3b}.queue-mode:disabled{opacity:.5;cursor:wait}.queue-mode[aria-pressed=true]{border-color:#91b9f5;background:#24364c}.merged-queue section+section{border-top:1px solid #ffffff20;margin-top:10px;padding-top:10px}.queued-items{flex:1;min-width:0;max-height:160px;overflow:auto;display:flex;flex-direction:column;gap:5px}.queued-card{border:1px solid #ffffff20;border-radius:10px;background:#262626;font-size:12px}.queued-card summary{display:flex;gap:10px;padding:7px 10px;cursor:pointer;list-style:none}.queued-card summary::-webkit-details-marker{display:none}.queued-card summary span:first-child{flex:none;color:#91b9f5}.queued-card summary span:last-child{white-space:nowrap;overflow:hidden;text-overflow:ellipsis;color:#bbb}.queued-card>div{padding:2px 10px 10px;white-space:pre-wrap;overflow-wrap:anywhere}.queued-card small{color:#aaa}.landing:has(.outbox) .composer-area{padding-bottom:35px}
/* connect button attention dot */.connect-button{position:relative}.attention-dot{position:absolute;top:3px;right:3px;width:7px;height:7px;border-radius:50%;background:#ef4b4b;box-shadow:0 0 0 2px var(--card-bg);pointer-events:none}@media(prefers-reduced-motion:no-preference){.attention-dot{animation:attention-pulse 1.6s ease-in-out infinite}}@keyframes attention-pulse{50%{opacity:.45}}
@container (min-width:800px){.tasks-visible .conversation{margin-right:var(--chat-task-width)}}
.mode-switch{display:flex;justify-content:center;align-self:center;gap:3px;margin:20px 0 0;padding:4px;background:#252525;border:1px solid #ffffff0c;border-radius:22px;font-size:12px}.mode-switch button{padding:7px 22px;border-radius:18px;color:#969696}.mode-switch button[aria-pressed=true]{background:#414141;color:#eee;box-shadow:0 2px 4px #0003}.group-mode .composer-area form{position:relative;display:flex;flex-wrap:wrap;align-items:center;gap:8px;border-radius:28px;padding:10px 12px 10px 20px}.group-mode .composer-input{flex:1;min-width:100px;margin-left:26px}.group-mode .mention-input textarea,.group-mode .draft-highlight{min-height:28px;line-height:28px}.group-mode .composer-toolbar{margin:0;gap:8px}.group-mode .composer-toolbar>span{position:absolute;left:12px}.group-mode .composer-toolbar>span>button:not(.attach-button){display:none}.group-mode .draft-files{width:100%;margin:0}.project-picker{position:relative}.project-picker>button{padding:5px}.project-options{position:absolute;left:0;bottom:calc(100% + 12px);z-index:25;min-width:190px;max-width:300px;max-height:250px;overflow:auto;padding:6px;background:#282828;border:1px solid #ffffff25;border-radius:12px;box-shadow:0 12px 25px #0008}.project-options button{display:block;width:100%;padding:10px;text-align:left;border-radius:7px;overflow-wrap:anywhere}.project-options button:hover,.project-options button.chosen{background:#ffffff0e}.composer-toolbar>span>button{padding:4px 6px}.group-mode.landing .composer-area{padding-bottom:35px}
.mention-input textarea::placeholder{color:#aaa;opacity:1}
.composer-area form{position:relative;z-index:1}
.composer-tongue{display:flex;align-items:center;flex-wrap:wrap;gap:4px;max-width:736px;width:calc(100% - 24px);margin:-10px auto 0;padding:16px 10px 7px;border-radius:0 0 15px 15px;background:#272727;color:#ddd;font-size:11px;box-sizing:border-box}
.composer-tongue>button,.composer-tongue .project-picker>button{display:flex;align-items:center;gap:6px;padding:5px 8px;border-radius:6px;min-height:28px}
.composer-tongue button:hover:enabled{background:#ffffff12;color:#fff}.composer-tongue button:disabled{opacity:.45}
.composer-tongue .project-picker{min-width:0;max-width:45%}.composer-tongue .project-picker>button{max-width:100%}.composer-tongue .project-picker span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.composer-tongue .project-options{z-index:40}
@container (max-width:799px){.tasks-visible .conversation{margin-right:0;display:none}.tasks-visible :global(.chat-task-panel){width:100%!important}}
.conversation-body{display:flex;flex:1;flex-direction:column;min-height:0}
.landing .conversation-body{justify-content:center;overflow:auto}
.landing .feed-frame{flex:0 0 auto;margin-top:0}
.landing .composer-area,.group-mode.landing .composer-area{flex:none;margin-bottom:0;padding-bottom:24px}
.attach-trigger{padding:8px;margin:-8px}
.composer-tongue{max-height:0;min-height:0;padding-top:0;padding-bottom:0;margin-top:0;opacity:0;visibility:hidden;transform:translateY(-10px);pointer-events:none;overflow:hidden;transition:max-height .2s ease,padding .2s ease,margin .2s ease,opacity .2s ease,transform .2s ease,visibility .2s}
.composer-tongue.revealed{max-height:160px;padding-top:16px;padding-bottom:7px;margin-top:-10px;opacity:1;visibility:visible;transform:translateY(0);pointer-events:auto;overflow:visible}
@media(prefers-reduced-motion:reduce){.composer-tongue{transition:none}}
.mention-input{max-height:50dvh;overflow-y:auto;scrollbar-gutter:stable}.mention-input textarea{overflow:hidden;max-height:none}.draft-highlight{bottom:auto;height:auto;max-height:none;min-height:100%}.composer-area{flex-shrink:0}.composer-area form{max-height:70dvh;overflow-y:auto}
</style>
