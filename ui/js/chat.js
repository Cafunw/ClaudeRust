// Conversation view: turns, streaming updates, branching, permissions.
import { openSaveCommand } from './save_command.js';
import { watchBadge } from './watch_ui.js';
import { contextMeter, compactDivider, refreshMeters } from './context_ui.js';
import { openRewind } from './rewind.js';
import * as workspace from './workspace.js';
import { icon, spark } from './icons.js';
import { t, tOr, currentLanguage } from './i18n.js';
import { call, on } from './api.js';
import { store, emit, on as onUi, modelInfo, displayName, saveLocal } from './store.js';
import { $, esc, h, showMenu, toast, errorToast, initials, attachmentSub, lightbox, confirmDialog, promptDialog, fmtTokens, fmtUsd, snapshot, sameSnapshot, scrollBehavior, convTitle } from './util.js';
import { createComposer, researchOffered } from './composer.js';
import { renderParts, turnPlainText, copyText, unifiedDiffHtml, errorMessage, resumable } from './parts.js';
import { revealAll } from './stream_reveal.js';
import { editedFiles } from './diffstat.js';
import { questionCard } from './ask_question.js';
import * as artifacts from './artifacts.js';
import { PAGE_TURNS, pageStart, parked, drawWithin, FRAME_BUDGET, OPEN_BUDGET } from './turnwindow.js';
import { retryLabel } from './retry_ui.js';
import { panelStep } from './branch_panel.js';
import { announce, streamAnnouncer } from './a11y.js';
import { speechText, readAloud, readingKey, stopReading, readAloudAvailable, onVoicesReady } from './readaloud.js';
import { limitBanner } from './usage_limits.js';
import { researchLine, researchQuery } from './research_ui.js';
import { orgSupported, setPinned, setUnread, setSection, setArchived, opened } from './sidebar_org.js';
import { newSection } from './sidebar.js';
import { permissionInfo } from './mcp_tool_info.js';

let cur = null;

export function current() { return cur; }

function viewEl() { return $('#view'); }

function composerInitial(conv) {
  const o = conv.options || {};
  return {
    model: conv.model,
    thinking: o.thinking ?? (conv.kind === 'code' ? true : !!store.settings.thinking_default),
    effort: o.effort || null,
    ultracode: o.ultracode ?? null,
    web_search: o.web_search ?? false,
    fast: o.fast ?? false,
    research: o.research ?? false,
    style: o.style || 'normal',
    disabled_servers: o.disabled_servers || [],
    disabled_tools: o.disabled_tools || [],
    tool_access: o.tool_access || null,
    permission_mode: o.permission_mode || store.settings.code_permission_mode || 'default',
  };
}

/** An incognito chat: not listed, searched or remembered, and gone once closed.
 *  (The backend's flag, not the option asked for: a build without incognito keeps the chat.) */
export const isIncognito = (c) => !!c && c.incognito === true;

export function incognitoBanner() {
  return h(`<div class="incognito-banner" role="note">${icon('eyeOff', 15)}<span>${esc(t('incog_banner'))}</span></div>`);
}

/** Gone for good (incognito.rs: as a delete, which also stops its turn and removes its CLI transcript). */
function discardIncognito(id) {
  workspace.disposeConversation(id);
  store.drafts.delete(id);
  // (what its artifacts stored with window.storage goes with it: db.rs artifact_storage)
  call('close_incognito', { id }).catch(() => {});
}

// the latest open request wins: a slow load of a big conversation must not
// replace the one opened after it
let openSeq = 0;

// a turn started: the chat list changed (its order, the chat running), not the sidebar's sections (sidebar.js)
const CHATS_ONLY = { sections: false };

// get_conversation (the turns loaded so far and any after them, rendered)
// for callers that come within a few milliseconds of each other: the end of
// a turn brings generation-state and conversation-updated back to back, and
// a rename the command's reply and its conversation-updated, and each asked
// for all of it. A caller only ever joins a request that hasn't been sent
// yet, so it never gets data older than its call.
const JOIN_MS = 10;
const fetching = new Map(); // id -> promise of a request about to be sent
export function fetchConversation(id) {
  let p = fetching.get(id);
  if (!p) {
    p = new Promise((resolve, reject) => setTimeout(() => {
      fetching.delete(id);
      const since = cur && cur.id === id ? cur.order.find((x) => !String(x).startsWith('pending-')) : null;
      call('get_conversation', { id, limit: PAGE_TURNS, since }).then(resolve, reject);
    }, JOIN_MS));
    fetching.set(id, p);
  }
  return p;
}

/** Whether a get_conversation of `id` is about to be sent: what it brings is
 *  at least as new as anything the backend said before now (main.js need not
 *  ask for the conversation's record on a conversation-updated then). */
export function refreshing(id) { return fetching.has(id); }

export async function openConversation(id, { focus = true } = {}) {
  const seq = ++openSeq;
  // another chat that takes a moment: say so rather than leave the last one on screen
  const slow = cur && cur.id === id ? null : setTimeout(() => {
    if (seq === openSeq) viewEl().innerHTML = `<div class="view-loading" role="status"><span class="spinner"></span>${esc(t('chat_loading'))}</div>`;
  }, 150);
  let data;
  try { data = await call('get_conversation', { id, limit: PAGE_TURNS }); } finally { clearTimeout(slow); }
  if (seq !== openSeq) return;
  const conv = data.conversation;
  store.codeRoot = conv.kind === 'code' ? conv.cwd : null;
  emit('conversation-opened', { id, kind: conv.kind });
  opened(id);
  const v = viewEl();
  v.innerHTML = '';
  const top = h(`<div class="topbar"></div>`);
  const scroll = h(`<div class="chat-scroll"><div class="chat-col"></div></div>`);
  const bottom = h(`<div class="chat-bottom"><div class="inner"><div class="research-slot"></div><div class="perm-area"></div><div class="composer-slot"></div><div class="composer-hint"></div></div></div>`);
  v.append(top, scroll, bottom);
  const incognito = isIncognito(conv);
  if (incognito) {
    $('.inner', bottom).prepend(incognitoBanner());
    saveLocal({ lastRoute: null }); // not reopened at the next start: it's gone by then
  }
  $('.inner', bottom).prepend(limitBanner());

  clearRetry();
  if (cur && cur.ro) cur.ro.disconnect();
  cur = {
    id,
    conv,
    turns: new Map(),
    order: [],
    els: new Map(),
    scrollEl: scroll,
    colEl: $('.chat-col', scroll),
    permEl: $('.perm-area', bottom),
    bottomEl: bottom,
    topEl: top,
    running: data.running,
    artifacts: new Map(),
    autoOpened: new Set(),
    // order[shown..] are drawn; earlier turns are loaded but not drawn, and
    // with `more` the backend has turns before order[0]
    shown: 0,
    more: !!data.more,
    earlier: data.earlier_artifacts || [],
  };
  const composer = createComposer({
    mode: conv.kind === 'code' ? 'code' : 'chat',
    placeholder: conv.kind === 'code' ? t('code_ask_ph') : t('reply_placeholder'),
    initial: composerInitial(conv),
    // an incognito chat's draft isn't kept either
    draftKey: incognito ? null : id,
    onSend: (text, atts) => sendInConversation(text, atts),
    onStop: () => stopTurn(id),
    onOptions: (patch) => persistOptions(patch),
    onEditLast: () => editLastUserMessage(),
    onQueue: (text, atts) => {
      if (!queues.has(id)) queues.set(id, []);
      queues.get(id).push({ text, atts });
      showQueue(id);
    },
    context: () => ({ conversationId: id, cwd: cur && cur.conv.cwd, engine: (cur && cur.conv.options && cur.conv.options.engine) || 'builtin', conv: cur && cur.conv }),
  });
  cur.composer = composer;
  $('.composer-slot', bottom).appendChild(composer.el);
  $('.composer-hint', bottom).textContent = '';
  composer.setRunning(!!data.running, !!data.waiting);

  renderTopbar();
  cur.order = data.turns.map((x) => x.id);
  for (const x of data.turns) cur.turns.set(x.id, x);
  // the last page: what fits in a frame now, from the end where the view
  // opens, and the rest in the frames after (a scroll up waits for it)
  cur.shown = data.turns.length;
  const from = pageStart(data.turns, cur.shown);
  drawBefore(cur, from, OPEN_BUDGET);
  if (cur.shown > from) {
    const c = cur;
    c.earlierP = drawBack(c, from).catch(errorToast).finally(() => {
      c.earlierP = null;
      if (cur === c) drawEarlierButton();
    });
  }
  drawEarlierButton();
  markLast();
  // first, so the panel rebuildArtifacts sees open is this conversation's
  artifacts.onConversationOpened(id, cur.artifacts);
  rebuildArtifacts();
  for (const p of data.pending_permissions || []) showPermission(p);
  scroll.addEventListener('scroll', onScroll);
  scrollToBottom(true);
  // math, highlighted code and images lay out a moment later: keep the end
  // in view while you haven't scrolled away
  const state = cur;
  state.ro = new ResizeObserver(() => { if (cur === state && cur.stick !== false) scrollToBottom(false); });
  state.ro.observe(state.colEl);
  if (focus) composer.focus();
  showQueue(id);
  workspace.setConversation(conv);
  document.title = conv.title || 'ClaudeRust';
}

function renderTopbar() {
  if (!cur) return;
  const c = cur.conv;
  const proj = c.project_id ? store.projects.find((p) => p.id === c.project_id) : null;
  cur.topEl.innerHTML = '';
  if (proj) {
    const crumb = h(`<button class="crumb">${icon('folder', 15)}<span class="ellipsis" style="max-width:180px">${esc(proj.name)}</span></button>`);
    crumb.onclick = () => emit('navigate', { name: 'project', params: { id: proj.id } });
    cur.topEl.append(crumb, h(`<span class="muted">/</span>`));
  }
  if (c.kind === 'code' && c.cwd) {
    const chip = h(`<button class="crumb" title="${esc(c.cwd)}">${icon('terminal', 15)}<span class="code-topinfo ellipsis" style="max-width:260px">${esc(c.cwd.split(/[\\/]/).filter(Boolean).pop() || c.cwd)}</span></button>`);
    chip.onclick = () => showMenu(chip, [
      { header: c.cwd },
      { label: t('folder_editor'), icon: 'external', onClick: () => call('open_in_editor', { path: c.cwd, line: null }).catch(errorToast) },
      { label: t('reveal'), icon: 'folder', onClick: () => call('reveal_path', { path: c.cwd }).catch(errorToast) },
      { label: t('folder_terminal'), icon: 'terminal', onClick: () => call('open_system_terminal', { path: c.cwd }).catch(errorToast) },
      { label: t('fv_copy_abs'), icon: 'copy', onClick: () => copyText(c.cwd).then(() => toast(t('copied'))) },
    ], { placement: 'bottom-start', width: 280 });
    cur.topEl.append(chip, h(`<span class="muted">/</span>`));
    if (c.options && c.options.engine === 'claude-code') cur.topEl.append(h(`<span class="active-tool eng" title="${esc(t('engine_cc'))}">${icon('terminal', 12)}<span class="lbl"> Claude Code</span></span>`));
    if (c.options && c.options.worktree && c.options.worktree.branch) cur.topEl.append(h(`<span class="active-tool wt" title="${esc(t('worktree_topbar', { path: c.options.worktree.path }))}">${icon('code', 12)} ${esc(c.options.worktree.branch)}</span>`));
  }
  const tb = h(`<button class="title-btn" aria-haspopup="menu" title="${esc(convTitle(c))}">${c.starred ? `<span style="color:var(--accent)">${icon('starFill', 14)}</span>` : ''}<span class="t">${esc(convTitle(c))}</span>${icon('chevDown', 14)}</button>`);
  tb.onclick = () => conversationMenu(tb, c, { onChange: () => refreshConversationMeta() });
  // the whole title on hover (it may be cut short, and a new title can arrive after this was drawn)
  tb.addEventListener('mouseenter', () => { tb.title = tb.querySelector('.t').textContent; });
  cur.topEl.append(tb, h('<div class="spacer"></div>'));
  if (c.kind === 'code') cur.topEl.append(watchBadge(c));
  const outline = h(`<button class="icon-btn" title="${esc(t('outline_title'))}">${icon('list', 17)}</button>`);
  outline.onclick = () => showOutline(outline);
  cur.topEl.append(outline);
  // one meter per open chat: it refreshes itself when a turn ends or the chat is compacted (context_ui.js), and the
  // topbar is drawn on every reload; switching a branch or rewinding refresh it (refreshMeters)
  cur.meter ||= contextMeter(cur.id, () => { cur.composer.setText('/cost'); cur.composer.send(); });
  cur.topEl.append(cur.meter);
  const usage = h(`<button class="icon-btn" title="${t('usage')}">${icon('info', 17)}</button>`);
  usage.onclick = () => showUsage(usage);
  cur.topEl.append(usage);
  if (c.kind === 'code') cur.topEl.append(workspace.toggleButton());
  if (cur.artifacts.size) {
    const ab = h(`<button class="icon-btn ap-toggle ${artifacts.isOpen() ? 'on' : ''}" title="${t('artifacts')}" aria-label="${t('artifacts')}" aria-pressed="${artifacts.isOpen()}">${icon('layers', 17)}</button>`);
    ab.onclick = () => artifacts.togglePanel(cur.id, cur.artifacts);
    cur.topEl.append(ab);
  }
}

async function showUsage(anchor) {
  try {
    const u = await call('usage_stats', { conversationId: cur.id });
    showMenu(anchor, [
      { header: `${t('usage')} · ${modelInfo(cur.conv.model).name}` },
      { label: t('usage_in_out', { i: fmtTokens(u.input_tokens), o: fmtTokens(u.output_tokens) }), icon: 'zap', disabled: true },
      { label: t('usage_cache_rw', { r: fmtTokens(u.cache_read_input_tokens), w: fmtTokens(u.cache_creation_input_tokens) }), icon: 'history', disabled: true },
      { label: `${t('cost')}: ${fmtUsd(u.cost_usd)}`, icon: 'info', disabled: true },
    ], { placement: 'bottom-end', width: 260 });
  } catch (e) { errorToast(e); }
}

export async function conversationMenu(anchor, c, { onChange, onDelete } = {}) {
  const items = [
    { label: c.starred ? t('unstar') : t('star'), icon: 'star', onClick: async () => { await call('update_conversation', { req: { id: c.id, starred: !c.starred } }); c.starred = !c.starred; onChange && onChange(); emit('conversations-changed'); } },
    { label: t('rename'), icon: 'edit', onClick: async () => {
      const v = await promptDialog(t('rename'), c.title || '');
      if (v === null) return;
      await call('update_conversation', { req: { id: c.id, title: v } });
      c.title = v; onChange && onChange(); emit('conversations-changed');
    } },
  ];
  if (orgSupported() && !isIncognito(c)) items.push(...orgItems(anchor, c, onChange));
  if (c.kind !== 'code') {
    items.push({ label: c.project_id ? t('remove_from_project') : t('add_to_project'), icon: 'folder', onClick: async () => {
      if (c.project_id) {
        await call('update_conversation', { req: { id: c.id, project_id: null } });
        c.project_id = null; onChange && onChange(); emit('conversations-changed');
        return;
      }
      const projects = await call('list_projects');
      if (!projects.length) { toast(t('no_projects')); return; }
      showMenu(anchor, projects.map((p) => ({ label: p.name, icon: 'folder', onClick: async () => {
        await call('update_conversation', { req: { id: c.id, project_id: p.id } });
        c.project_id = p.id; onChange && onChange(); emit('conversations-changed');
      } })), { width: 240 });
    } });
  }
  // `c` is either a loaded conversation (with `options`) or a list entry,
  // which carries just the engine and the instructions
  const opts = c.options || {};
  const ins = String(opts.instructions ?? c.instructions ?? '').trim();
  items.push({ label: t('conv_instructions'), sub: ins ? (ins.length > 40 ? `${ins.slice(0, 40)}…` : ins) : t('conv_instructions_none'), icon: 'edit', onClick: async () => {
    const v = await promptDialog(t('conv_instructions').replace(/…$/, ''), ins, { multiline: true, placeholder: t('conv_instructions_ph') });
    if (v === null) return;
    try {
      await call('update_conversation', { req: { id: c.id, options: { instructions: v.trim() } } });
    } catch (e) { errorToast(e); return; }
    if (c.options) c.options = { ...c.options, instructions: v.trim() };
    c.instructions = v.trim();
    if (cur && cur.id === c.id && cur !== c) cur.options = { ...(cur.options || {}), instructions: v.trim() };
    toast(v.trim() ? t('conv_instructions_set') : t('conv_instructions_cleared'));
    onChange && onChange();
  } });
  if (c.kind === 'code' && (opts.engine ?? c.engine) === 'claude-code') {
    items.push({ label: t('cc_open_terminal'), icon: 'terminal', onClick: () => call('cc_open_in_terminal', { conversationId: c.id }).catch(errorToast) });
  }
  // an incognito chat is kept nowhere: no export, no print (the backend refuses them too)
  if (!isIncognito(c)) items.push(
    { sep: true },
    { label: t('export_md'), icon: 'download', onClick: () => exportConv(c, 'md') },
    { label: t('export_json'), icon: 'download', onClick: () => exportConv(c, 'json') },
    { label: t('export_html'), icon: 'download', onClick: () => exportConv(c, 'html') },
    { label: t('print_chat'), icon: 'printer', onClick: () => printConv(c) },
  );
  items.push(
    { sep: true },
    { label: t('delete'), icon: 'trash', danger: true, onClick: async () => {
      if (!(await confirmDialog(t('confirm_delete_chat')))) return;
      try { await call('delete_conversation', { id: c.id }); } catch (e) { errorToast(e); return; }
      workspace.disposeConversation(c.id);
      store.drafts.delete(c.id);
      emit('conversations-changed');
      if (onDelete) onDelete(); else if (cur && cur.id === c.id) emit('navigate', { name: c.kind === 'code' ? 'code' : 'new' });
    } },
  );
  showMenu(anchor, items, { width: 220 });
}

/** Pin, unread, section and archive items of the conversation menu (sidebar_cmds.rs). */
function orgItems(anchor, c, onChange) {
  const done = (r) => { if (r) Object.assign(c, { pinned_at: r.pinned_at, archived_at: r.archived_at, unread: r.unread, section_id: r.section_id }); onChange && onChange(); emit('conversations-changed'); };
  const kind = c.kind === 'code' ? 'code' : 'chat';
  const moveTo = async (sectionId) => done(await setSection(c.id, sectionId));
  return [
    { label: c.pinned_at ? t('org_unpin') : t('org_pin'), icon: 'pin', onClick: async () => done(await setPinned(c.id, !c.pinned_at)) },
    { label: c.unread ? t('org_mark_read') : t('org_mark_unread'), icon: 'eye', onClick: async () => done(await setUnread(c.id, !c.unread)) },
    { label: t('org_move'), icon: 'layers', keepOpen: true, onClick: () => {
      const sections = ((store.sections || {})[kind]) || [];
      showMenu(anchor, [
        ...sections.map((s) => ({ label: s.name, checked: s.id === c.section_id, onClick: () => moveTo(s.id) })),
        ...(c.section_id ? [{ label: t('org_move_out'), icon: 'x', onClick: () => moveTo(null) }] : []),
        { sep: true },
        { label: t('org_new_section'), icon: 'plus', onClick: async () => { const s = await newSection(kind); if (s) await moveTo(s.id); } },
      ], { width: 240 });
    } },
    { label: c.archived_at ? t('org_unarchive') : t('org_archive'), icon: 'archive', onClick: async () => {
      done(await setArchived(c.id, !c.archived_at));
      if (!c.archived_at) return;
      toast(t('org_archived_toast'));
      if (cur && cur.id === c.id) emit('navigate', { name: c.kind === 'code' ? 'code' : 'new' });
    } },
  ];
}

/** Print the conversation (or save it as a PDF): its HTML export, in a
 *  hidden frame that runs no scripts. (The app's own print key stays off:
 *  it would print the app.) */
async function printConv(c) {
  let html;
  try { html = await call('export_conversation', { id: c.id, format: 'html' }); } catch (e) { errorToast(e); return; }
  document.querySelectorAll('iframe.print-frame').forEach((f) => f.remove());
  const f = document.createElement('iframe');
  f.className = 'print-frame';
  f.setAttribute('aria-hidden', 'true');
  f.tabIndex = -1;
  // same origin, so the app can call print(); no allow-scripts, so the page runs nothing
  f.setAttribute('sandbox', 'allow-same-origin allow-modals');
  f.onload = () => {
    const w = f.contentWindow;
    w.addEventListener('afterprint', () => f.remove());
    w.focus();
    w.print();
  };
  f.srcdoc = html;
  document.body.appendChild(f);
}

async function exportConv(c, fmt) {
  try {
    const content = await call('export_conversation', { id: c.id, format: fmt === 'json' || fmt === 'html' ? fmt : 'md' });
    const base = (c.title || 'conversation').replace(/[\\/:*?"<>|]+/g, '-').slice(0, 80);
    await call('save_text_file', { name: `${base}.${fmt}`, content });
  } catch (e) { errorToast(e); }
}

async function refreshConversationMeta() {
  if (!cur) return;
  try {
    const id = cur.id;
    // the record only (title, options), not the rendered conversation
    const conv = await call('conversation_meta', { id });
    if (!cur || cur.id !== id) return;
    cur.conv = conv;
    renderTopbar();
    document.title = cur.conv.title || 'ClaudeRust';
  } catch { /* ignore */ }
}

function persistOptions(patch) {
  if (!cur) return;
  const req = { id: cur.id };
  const opts = { ...patch };
  if ('model' in opts) { req.model = opts.model; delete opts.model; cur.conv.model = req.model; }
  if (Object.keys(opts).length) req.options = opts;
  call('update_conversation', { req }).then((c) => { if (cur && cur.id === c.id) cur.conv = c; }).catch(errorToast);
}

async function sendInConversation(text, atts) {
  if (!cur) return false;
  const id = cur.id;
  // research is asked per message (research.rs reads it off the question)
  const research = !!cur.composer.state.research && researchOffered();
  appendOptimisticUser(text, atts);
  try {
    await call('send_message', { req: { conversation_id: id, text, attachment_ids: atts.map((a) => a.id).filter(Boolean), research } });
  } catch (e) {
    await reload();
    throw e;
  }
  await reload();
  emit('conversations-changed', CHATS_ONLY);
  return true;
}

function appendOptimisticUser(text, atts) {
  rewindow();
  const turn = { id: 'pending-' + Date.now(), role: 'user', parts: [], siblings: { count: 1 } };
  if (atts.length) turn.parts.push({ kind: 'attachments', items: atts.map((a) => ({ name: a.name, kind: a.kind, size: a.size, src: a.preview })) });
  if (text) turn.parts.push({ kind: 'user_text', text });
  upsertTurn(turn, false);
  markLast();
  scrollToBottom(true);
}

export async function reload() {
  if (cur) cur.colEl.querySelectorAll('.compact-divider.pending').forEach((x) => x.remove());
  if (!cur) return;
  const id = cur.id;
  const data = await fetchConversation(id);
  if (!cur || cur.id !== id) return;
  cur.conv = data.conversation;
  cur.more = !!data.more;
  cur.earlier = data.earlier_artifacts || [];
  // the same turns stay drawn: from the first one drawn, while it's still there
  const at = data.turns.findIndex((x) => x.id === cur.order[cur.shown]);
  cur.order = data.turns.map((x) => x.id);
  cur.turns = new Map(data.turns.map((x) => [x.id, x]));
  cur.shown = at >= 0 ? at : pageStart(data.turns, data.turns.length);
  dropUndrawn();
  for (const turn of data.turns.slice(cur.shown)) upsertTurn(turn, false);
  placeTurns();
  drawEarlierButton();
  parkLater();
  markLast();
  rebuildArtifacts();
  renderTopbar();
  document.title = cur.conv.title || 'ClaudeRust';
  cur.running = data.running;
  cur.composer.setRunning(!!data.running, !!data.waiting);
  cur.colEl.querySelectorAll('[data-busy]').forEach((b) => showBusy(b, !!data.running));
  scrollToBottom(false);
}

// ----------------------------------------------------------------- turns

function nearBottom() {
  const s = cur.scrollEl;
  return s.scrollHeight - s.scrollTop - s.clientHeight < 120;
}

function scrollToBottom(force) {
  if (!cur) return;
  if (force || cur.stick !== false) cur.scrollEl.scrollTop = cur.scrollEl.scrollHeight;
  updateScrollButton();
}

function onScroll() {
  if (!cur) return;
  // stop following the end only when you scroll up: scroll events arrive a
  // frame late, after content may have grown below a programmatic jump
  const top = cur.scrollEl.scrollTop;
  if (nearBottom()) cur.stick = true;
  else if (top < (cur.lastTop ?? 0) - 2) cur.stick = false;
  cur.lastTop = top;
  updateScrollButton();
  parkLater();
  if (top < 600 && cur.stick === false) showEarlier();
}

// ----------------------------------------------------- earlier turns

function hasEarlier() { return !!cur && (cur.shown > 0 || cur.more); }

/** The "Show earlier messages" button at the top while older turns aren't drawn. */
function drawEarlierButton() {
  let b = cur.colEl.querySelector(':scope > .show-earlier');
  if (!hasEarlier()) { if (b) b.remove(); return; }
  if (b) return;
  b = h(`<button class="btn sm ghost show-earlier">${icon('history', 14)} ${esc(t('show_earlier'))}</button>`);
  b.onclick = () => showEarlier();
  cur.colEl.prepend(b);
}

/** Draw the page of turns before the first one drawn, loading it first when
 *  the backend has it, and keep what's on screen where it was. One at a
 *  time: callers meanwhile wait for the one under way. */
function showEarlier() {
  const c = cur;
  if (!c || !hasEarlier()) return Promise.resolve();
  c.earlierP ||= drawEarlier(c).catch(errorToast).finally(() => { c.earlierP = null; });
  return c.earlierP;
}

async function drawEarlier(c) {
  if (c.shown === 0) {
    const first = c.order[0];
    const data = await call('get_conversation', { id: c.id, limit: PAGE_TURNS, before: first });
    // (a reload in the meantime that changed the branch: the next scroll asks again)
    if (cur !== c || c.order[0] !== first) return;
    const older = data.turns.filter((x) => !c.turns.has(x.id));
    c.more = !!data.more && older.length > 0;
    for (const x of older) c.turns.set(x.id, x);
    c.order = [...older.map((x) => x.id), ...c.order];
    c.shown = older.length;
  }
  const start = pageStart(c.order.map((id) => c.turns.get(id)), c.shown);
  c.stick = false;
  await drawBack(c, start);
  if (cur !== c) return;
  drawEarlierButton();
  rebuildArtifacts();
  updateScrollButton();
}

/** Draw the turns before the first one drawn, newest first, down to index
 *  `start` while `budget` ms last (each laid out as it comes, so the budget
 *  counts its layout too), keeping what's on screen where it was. */
function drawBefore(c, start, budget) {
  const anchor = c.els.get(c.order[c.shown]) || null;
  const y0 = anchor ? anchor.getBoundingClientRect().top : 0;
  drawWithin(c.shown - start, () => {
    c.shown--;
    upsertTurn(c.turns.get(c.order[c.shown]), false, c.els.get(c.order[c.shown + 1]) || null);
    void c.colEl.offsetHeight;
  }, budget);
  if (anchor) c.scrollEl.scrollTop += anchor.getBoundingClientRect().top - y0;
}

/** The same down to `start`, a frame's worth in each frame (never in a
 *  scroll event's, which has its own work); a send meanwhile moves `shown`
 *  back to the last page, which ends it. */
async function drawBack(c, start) {
  while (cur === c && c.shown > start) {
    await new Promise((r) => requestAnimationFrame(r));
    if (cur !== c || c.shown <= start) return;
    drawBefore(c, start, FRAME_BUDGET);
  }
}

/** Draw every turn (the find bar and the outline look through all of them). */
async function showAllEarlier() {
  const c = cur;
  while (c && cur === c && hasEarlier()) {
    const was = [c.shown, c.order.length];
    await showEarlier();
    if (c.shown === was[0] && c.order.length === was[1]) return;
  }
}

/** Remove the elements of turns that aren't drawn (before `shown`, or no longer on the branch). */
function dropUndrawn() {
  const keep = new Set(cur.order.slice(cur.shown));
  for (const [tid, el] of cur.els) if (!keep.has(tid)) { el.remove(); cur.els.delete(tid); }
}

/** Back to the last page (you sent, or a new turn started): what scrolling
 *  up drew goes, and "Show earlier messages" with it. */
function rewindow() {
  if (!cur || cur.hold) return;
  const start = pageStart(cur.order.map((id) => cur.turns.get(id)), cur.order.length);
  if (start <= cur.shown) return;
  const anchor = cur.els.get(cur.order[start]);
  const y0 = anchor ? anchor.getBoundingClientRect().top : 0;
  cur.shown = start;
  dropUndrawn();
  drawEarlierButton();
  if (anchor) cur.scrollEl.scrollTop += anchor.getBoundingClientRect().top - y0;
}

// ----------------------------------------------------- parked turns (turnwindow.js)

function park(el, height) {
  el.style.height = `${height}px`;
  el.replaceChildren();
  el.__usig = null;
  el.classList.add('parked');
}

/** Draw parked turns again from their data (in order, while `budget` ms
 *  last), keeping what's on screen where it is: one above the view can come
 *  out taller or shorter than its placeholder (the window was resized
 *  meanwhile). How many were drawn. */
function unpark(c, list, budget = Infinity) {
  const top = c.scrollEl.getBoundingClientRect().top;
  const anchor = c.order.slice(c.shown).map((id) => c.els.get(id)).find((el) => el && el.getBoundingClientRect().bottom > top);
  const y0 = anchor ? anchor.getBoundingClientRect().top : 0;
  const n = drawWithin(list.length, (i) => {
    const [el, turn] = list[i];
    el.classList.remove('parked');
    el.style.height = '';
    if (turn.role === 'user') renderUserTurn(el, turn); else renderAssistantTurn(el, turn);
    if (budget !== Infinity) void el.offsetHeight; // the budget counts its layout too
  }, budget);
  const dy = anchor ? anchor.getBoundingClientRect().top - y0 : 0;
  if (Math.abs(dy) >= 1) c.scrollEl.scrollTop += dy;
  return n;
}

let parkFrame = 0;
function parkLater() {
  if (!parkFrame) parkFrame = requestAnimationFrame(parkFar);
}

function parkFar() {
  parkFrame = 0;
  const c = cur;
  if (!c || c.hold) return;
  const view = c.scrollEl.clientHeight;
  if (!view) return; // not laid out (hidden): nothing to measure against
  const top = c.scrollEl.getBoundingClientRect().top;
  const go = [];
  const back = [];
  for (const tid of c.order.slice(c.shown)) {
    const el = c.els.get(tid);
    const turn = c.turns.get(tid);
    if (!el || !turn) continue;
    const r = el.getBoundingClientRect();
    const was = el.classList.contains('parked');
    // the answer coming in, and a turn being edited, stay drawn
    const dist = Math.max(top - r.bottom, r.top - top - view, 0);
    const want = parked(dist, view, was) && !turn.streaming && !el.contains(document.activeElement);
    if (want && !was) go.push([el, r.height]);
    else if (!want && was) back.push([el, turn, dist]);
  }
  // (measured first: parking keeps every height, so nothing moves)
  for (const [el, height] of go) park(el, height);
  // the nearest first, a frame's worth; the rest on the next frame
  back.sort((a, b) => a[2] - b[2]);
  if (back.length && unpark(c, back, FRAME_BUDGET) < back.length) parkLater();
}

/** Draw every turn and keep them all drawn until `releaseAll` (the find bar searches the page). */
export async function drawAll() {
  const c = cur;
  if (!c) return;
  c.hold = true;
  await showAllEarlier();
  if (cur !== c) return;
  const list = c.order.slice(c.shown).map((id) => [c.els.get(id), c.turns.get(id)]).filter(([el]) => el && el.classList.contains('parked'));
  if (list.length) unpark(c, list);
}

export function releaseAll() {
  if (!cur || !cur.hold) return;
  cur.hold = false;
  parkLater();
}

function updateScrollButton() {
  if (!cur) return;
  // (kept, not looked up: a search from the view goes through the whole
  // conversation first, on every scroll event and streamed change)
  const bottom = cur.bottomEl;
  let btn = bottom.querySelector('.scroll-down');
  const show = !nearBottom();
  if (show && !btn) {
    btn = h(`<button class="scroll-down" aria-label="${esc(t('a11y_scroll_down'))}" title="${esc(t('a11y_scroll_down'))}">${icon('arrowDown', 16)}</button>`);
    btn.onclick = () => { cur.stick = true; cur.scrollEl.scrollTo({ top: cur.scrollEl.scrollHeight, behavior: scrollBehavior() }); };
    bottom.appendChild(btn);
  } else if (!show && btn) btn.remove();
}

/** Put the turn elements in conversation order after anything else in the
 *  column, as appending each of them would, but move nothing when they're
 *  in order already: a moved turn is laid out again and loses the scroll
 *  positions inside it, and a reload (after every answer) would move all. */
function placeTurns() {
  const want = cur.order.slice(cur.shown).map((tid) => cur.els.get(tid));
  const kids = cur.colEl.children;
  const start = kids.length - want.length;
  if (start >= 0 && want.every((el, j) => kids[start + j] === el)) return;
  for (const el of want) cur.colEl.appendChild(el);
}

/** A turn's disclaimer: it's in the action row, the turn's last child, so
 *  there's no need to search the whole turn (this runs for every turn on
 *  every streamed change). */
function disclaimerOf(el) {
  const last = el.lastElementChild;
  if (last && last.classList.contains('turn-actions')) return last.querySelector('.disclaimer');
  return el.querySelector('.disclaimer');
}

function markLast() {
  if (!cur) return;
  const lastId = cur.order[cur.order.length - 1];
  const lastEl = lastId ? cur.els.get(lastId) : null;
  cur.els.forEach((el) => {
    el.classList.toggle('last', !!lastEl && el === lastEl);
    // disclaimer only on the last assistant turn
    const d = disclaimerOf(el);
    if (d) d.classList.toggle('hidden', el !== lastEl);
  });
}

// A new turn is laid out once, so its real size is known and remembered
// (contain-intrinsic-size: auto); after that it's skipped while off screen.
// Skipped from the start, it would take a guessed size and the view would
// jump as you scroll onto it.
const unsettled = new Set();
function settleLater(el) {
  if (!unsettled.size) requestAnimationFrame(() => requestAnimationFrame(() => {
    for (const e of unsettled) e.classList.add('settled');
    unsettled.clear();
  }));
  unsettled.add(el);
}

/** Draw a turn: anew, or what changed in it. A new one goes at the end, or before `before`. */
function upsertTurn(turn, live = true, before = null) {
  if (!cur) return;
  // remove optimistic placeholders once real user turns arrive
  if (turn.role === 'user' && !String(turn.id).startsWith('pending-')) {
    for (const [tid, el] of cur.els) if (String(tid).startsWith('pending-')) { el.remove(); cur.els.delete(tid); }
  }
  const stick = live ? nearBottom() : true;
  let el = cur.els.get(turn.id);
  if (!el) {
    el = h(`<div class="turn ${turn.role}" data-id="${esc(turn.id)}"></div>`);
    cur.els.set(turn.id, el);
    cur.colEl.insertBefore(el, before);
    settleLater(el);
  }
  cur.turns.set(turn.id, turn);
  if (!cur.order.includes(turn.id)) cur.order.push(turn.id);
  if (el.classList.contains('parked')) {
    if (!live) return; // drawn from its data when it comes near
    el.classList.remove('parked');
    el.style.height = '';
  }
  if (turn.role === 'user') renderUserTurn(el, turn); else renderAssistantTurn(el, turn, live);
  if (live && stick) scrollToBottom(true);
}

function attachmentsHtml(items) {
  return `<div class="user-atts">${items.map((a) => a.src && a.kind === 'image'
    ? `<img src="${esc(a.src)}" alt="${esc(a.name)}" title="${esc(a.name)}" data-zoom role="button" tabindex="0">`
    : `<div class="att"${a.missing ? '' : ' role="button" tabindex="0" aria-haspopup="dialog"'}><div class="ficon">${a.kind === 'pdf' ? 'PDF' : esc((a.name || '').split('.').pop().slice(0, 4) || 'TXT')}</div><div class="info"><div class="n">${esc(a.name)}</div><div class="s">${a.missing ? esc(t('att_missing')) : attachmentSub(a)}</div></div></div>`).join('')}</div>`;
}

/** Whether `what` (a message action: busy_* in i18n.js) has to wait for the
 *  response under way here, which the backend refuses it during: then a toast
 *  says so, with Stop. */
function waitForIdle(what) {
  if (!cur || !cur.running) return false;
  const id = cur.id;
  toast(t(`busy_${what}`), '', { action: t('stop'), onAction: () => stopTurn(id) });
  return true;
}

/** Button `b` of message action `what` (see `waitForIdle`): while Claude
 *  responds, it shows it can't be used now, and its tooltip says why. */
function idleOnly(b, what) {
  b.dataset.busy = what;
  b.dataset.tip = b.title;
  showBusy(b, !!(cur && cur.running));
  return b;
}

function showBusy(b, busy) {
  if (busy) b.setAttribute('aria-disabled', 'true'); else b.removeAttribute('aria-disabled');
  b.title = busy ? t(`busy_${b.dataset.busy}`) : b.dataset.tip;
}

function branchNav(turn) {
  const s = turn.siblings;
  if (!s || s.count <= 1) return null;
  const nav = h(`<span class="branch-nav" role="group" aria-label="${esc(t('a11y_version_of', { n: s.index + 1, total: s.count }))}"><button class="icon-btn sm" data-d="-1" aria-label="${esc(t('a11y_prev_version'))}" title="${esc(t('a11y_prev_version'))}" ${s.index === 0 ? 'disabled' : ''}>${icon('chevLeft', 14)}</button><span class="count" aria-hidden="true">${s.index + 1} / ${s.count}</span><button class="icon-btn sm" data-d="1" aria-label="${esc(t('a11y_next_version'))}" title="${esc(t('a11y_next_version'))}" ${s.index >= s.count - 1 ? 'disabled' : ''}>${icon('chevRight', 14)}</button></span>`);
  nav.querySelectorAll('button').forEach((b) => idleOnly(b, 'versions').addEventListener('click', async () => {
    const target = s.ids[s.index + Number(b.dataset.d)];
    if (!target || waitForIdle('versions')) return;
    try {
      await call('switch_branch', { conversationId: cur.id, messageId: target });
      await reload();
      refreshMeters(cur.id);
    } catch (e) { errorToast(e); }
  }));
  return nav;
}

// ---- outline: jump between your messages

const userText = (turn) => (turn && turn.role === 'user' && (turn.parts.find((p) => p.kind === 'user_text') || {}).text) || '';

/** Your messages drawn, in order, as [element, text] (from the data: a parked one is empty). */
function userTurns() {
  if (!cur) return [];
  return cur.order.slice(cur.shown).map((id) => [cur.els.get(id), userText(cur.turns.get(id))]).filter(([el, text]) => el && text);
}

function jumpTo(el) {
  el.scrollIntoView({ block: 'start', behavior: scrollBehavior() });
  el.classList.add('flash');
  setTimeout(() => el.classList.remove('flash'), 1200);
}

function showOutline(anchor) {
  const mine = userTurns();
  if (!mine.length) { toast(t('outline_empty')); return; }
  showMenu(anchor, [
    { header: t('outline_title') },
    ...(hasEarlier() ? [{ label: t('show_earlier'), icon: 'history', onClick: () => showAllEarlier().then(() => showOutline(anchor)) }] : []),
    ...mine.map(([e, raw], i) => {
      const text = raw.trim().replace(/\s+/g, ' ');
      return { label: text.length > 90 ? `${text.slice(0, 90)}…` : text, right: String(i + 1), onClick: () => jumpTo(e) };
    }),
  ], { placement: 'bottom-end', width: 380 });
}

// Alt+↑ / Alt+↓: previous / next of your messages
window.addEventListener('keydown', (e) => {
  if (!e.altKey || e.ctrlKey || e.metaKey || e.shiftKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') || !cur || e.defaultPrevented) return;
  // keep the key for fields (Alt+↓ opens a <select>) and for dialogs and menus
  if (e.target && e.target.closest && e.target.closest('textarea, input, select, [contenteditable], .xterm')) return;
  if (document.querySelector('.modal-scrim, .popover')) return;
  const els = userTurns().map(([el]) => el);
  if (!els.length) return;
  e.preventDefault();
  // positions relative to the top of the scroll area; the message a jump
  // landed on sits at ~0, so it's neither "above" nor "below"
  const top = cur.scrollEl.getBoundingClientRect().top;
  const pos = els.map((x) => x.getBoundingClientRect().top - top);
  const target = e.key === 'ArrowUp'
    ? [...els].reverse().find((x, i) => pos[els.length - 1 - i] < -10)
    : els.find((x, i) => pos[i] > 10);
  if (target) jumpTo(target);
});

function renderUserTurn(el, turn) {
  const text = (turn.parts.find((p) => p.kind === 'user_text') || {}).text || '';
  const atts = turn.parts.find((p) => p.kind === 'attachments');
  const name = displayName();
  // a reload (after every answer) passes every turn again: one that hasn't
  // changed keeps its DOM instead of being rebuilt and laid out anew
  const sig = snapshot([turn, initials(name), cur ? cur.conv.kind : null, currentLanguage()]);
  const same = sameSnapshot(el.__usig, sig);
  el.__usig = sig;
  if (same) return;
  el.innerHTML = `${turn.meta && turn.meta.compact_summary ? '<div class="cd-slot"></div>' : ''}${atts ? attachmentsHtml(atts.items) : ''}
    ${text ? `<div class="user-bubble"><span class="avatar">${esc(initials(name))}</span><div class="user-text">${esc(text)}</div></div>` : ''}
    <div class="turn-actions"></div>`;
  el.querySelectorAll('[data-zoom]').forEach((img) => {
    img.addEventListener('click', () => lightbox(img.src, img.alt));
    img.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); lightbox(img.src, img.alt); } });
  });
  const cds = el.querySelector('.cd-slot');
  if (cds) cds.replaceWith(compactDivider(turn.meta.compact_summary, turn.meta.compact_auto));
  const ut = el.querySelector('.user-text');
  if (ut && text.length > 1500) {
    ut.classList.add('clamped');
    // a click or Enter shows the rest
    ut.tabIndex = 0;
    ut.setAttribute('aria-expanded', 'false');
    const unclamp = () => { ut.classList.remove('clamped'); ut.removeAttribute('tabindex'); ut.removeAttribute('aria-expanded'); };
    ut.addEventListener('click', unclamp);
    ut.addEventListener('keydown', (e) => { if (e.key === 'Enter' && ut.classList.contains('clamped')) { e.preventDefault(); unclamp(); } });
  }
  const actions = el.querySelector('.turn-actions');
  if (String(turn.id).startsWith('pending-')) return;
  const nav = branchNav(turn);
  if (nav) actions.appendChild(nav);
  if (cur && cur.conv.kind === 'code') {
    const rb = idleOnly(h(`<button class="icon-btn sm" title="${t('rewind_here')}">${icon('history', 15)}</button>`), 'rewind');
    rb.onclick = () => rewindTo(turn, text);
    actions.appendChild(rb);
  }
  if (text) {
    const eb = idleOnly(h(`<button class="icon-btn sm" title="${t('edit')}">${icon('edit', 15)}</button>`), 'edit');
    eb.onclick = () => startEdit(el, turn, text);
    const cb = h(`<button class="icon-btn sm" title="${t('copy')}">${icon('copy', 15)}</button>`);
    cb.onclick = async () => { await copyText(text); cb.innerHTML = icon('check', 15); setTimeout(() => (cb.innerHTML = icon('copy', 15)), 1400); };
    actions.append(eb, cb);
    if (cur && cur.conv.kind === 'code') {
      const sb = h(`<button class="icon-btn sm" title="${t('sc_action')}">${icon('zap', 15)}</button>`);
      sb.onclick = () => openSaveCommand({ text, conversationId: cur.id, cwd: cur.conv.cwd });
      actions.append(sb);
    }
  }
}

/** After /compact: show where the summary will start. */
export function showPendingCompact() {
  if (!cur) return;
  cur.colEl.querySelectorAll('.compact-divider.pending').forEach((x) => x.remove());
  const d = compactDivider(t('compact_pending_d'), false);
  d.classList.add('pending');
  cur.colEl.appendChild(d);
  scrollToBottom(true);
  // its tooltip says a compaction is pending
  refreshMeters(cur.id);
}

function rewindTo(turn, text) {
  if (waitForIdle('rewind')) return;
  const c = cur;
  openRewind({
    conversationId: c.id,
    messageId: turn.id,
    preview: text,
    onConversationRewound: async (msgText) => {
      if (cur !== c) return;
      await reload();
      refreshMeters(c.id);
      c.composer.setText(msgText || text);
      c.composer.focus();
    },
  });
}

/** /rewind: pick one of the user's messages. */
export function openRewindPicker() {
  if (!cur) return;
  const users = cur.order.map((id) => cur.turns.get(id)).filter((tn) => tn && tn.role === 'user');
  if (!users.length || waitForIdle('rewind')) return;
  const items = users.slice(-15).reverse().map((tn) => {
    const text = (tn.parts.find((p) => p.kind === 'user_text') || {}).text || '';
    return { label: text.slice(0, 80) || '…', icon: 'history', onClick: () => rewindTo(tn, text) };
  });
  const anchor = cur.composer.el;
  showMenu(anchor, items, { placement: 'top-start', width: 420 });
}

function startEdit(el, turn, text) {
  if (waitForIdle('edit')) return;
  const box = h(`<div class="edit-box"><textarea></textarea><div class="row"><button class="btn sm" data-a="cancel">${t('cancel')}</button><button class="btn sm primary" data-a="save">${t('save')}</button></div></div>`);
  const ta = box.querySelector('textarea');
  ta.value = text;
  const bubble = el.querySelector('.user-bubble');
  bubble.replaceWith(box);
  el.__usig = null; // drawn again on cancel (or reload), whatever the data
  el.querySelector('.turn-actions').classList.add('hidden');
  ta.style.height = Math.min(ta.scrollHeight + 4, window.innerHeight * 0.5) + 'px';
  ta.focus();
  const cancel = () => renderUserTurn(el, turn);
  let saving = false;
  const saveBtn = box.querySelector('[data-a=save]');
  const save = async () => {
    const v = ta.value.trim();
    // once: a double click or a held Enter would send the edit twice
    if (!v || saving) return;
    saving = true;
    saveBtn.disabled = true;
    try {
      await call('send_message', { req: { conversation_id: cur.id, text: v, edit: true, parent_id: turn.parent_id || '' } });
      await reload();
      emit('conversations-changed', CHATS_ONLY);
    } catch (e) {
      errorToast(e);
      saving = false;
      saveBtn.disabled = false;
    }
  };
  box.querySelector('[data-a=cancel]').onclick = cancel;
  box.querySelector('[data-a=save]').onclick = save;
  // Esc from the field or its Cancel and Save
  box.addEventListener('keydown', (e) => { if (e.key === 'Escape') { e.preventDefault(); cancel(); } });
  ta.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && !e.repeat) { e.preventDefault(); save(); }
  });
}

function editLastUserMessage() {
  if (!cur) return;
  for (let i = cur.order.length - 1; i >= 0; i--) {
    const turn = cur.turns.get(cur.order[i]);
    if (turn && turn.role === 'user') {
      const text = (turn.parts.find((p) => p.kind === 'user_text') || {}).text;
      if (text) startEdit(cur.els.get(turn.id), turn, text);
      return;
    }
  }
}

/** "Edited N files" summary under a finished code-session turn. */
function renderTurnFiles(el, turn, streaming) {
  let box = el.querySelector('.turn-files');
  // nothing to show and nothing shown (e.g. while streaming): done
  if (!box && (streaming || !cur || cur.conv.kind !== 'code')) return;
  const files = !streaming && cur && cur.conv.kind === 'code' ? editedFiles(turn.parts, cur.conv.cwd) : [];
  const sig = JSON.stringify(files);
  if (box && box.__sig === sig) return;
  if (!files.length) { if (box) box.remove(); return; }
  if (!box) { box = h('<div class="turn-files"></div>'); el.querySelector('.assistant-body').appendChild(box); }
  box.__sig = sig;
  const add = files.reduce((a, f) => a + f.added, 0);
  const del = files.reduce((a, f) => a + f.removed, 0);
  box.innerHTML = `<div class="tf-head">${icon('edit', 13)}<span>${esc(files.length === 1 ? t('turn_files_one') : t('turn_files', { n: files.length }))}</span><span class="diff-stat"><span class="a">+${add}</span> <span class="d">-${del}</span></span><span class="grow"></span><button class="btn sm ghost" data-a="review">${esc(t('turn_files_review'))}</button></div>
    <div class="tf-list">${files.map((f) => `<button class="tf-file" data-path="${esc(f.path)}" title="${esc(f.path)}"><span class="ellipsis">${esc(f.path)}</span>${f.created ? `<span class="tf-new">${esc(t('turn_files_new'))}</span>` : ''}<span class="diff-stat"><span class="a">+${f.added}</span> <span class="d">-${f.removed}</span></span></button>`).join('')}</div>`;
  box.querySelector('[data-a=review]').onclick = async () => { const ws = await import('./workspace.js'); ws.show('changes'); };
  box.querySelectorAll('.tf-file').forEach((b) => {
    b.onclick = async () => { const files = await import('./panes/files.js'); files.showFile(b.dataset.path); };
  });
}

/** Read aloud / Stop reading for answer `key` (Markdown `md`). */
function readAloudButton(key, md) {
  const b = h('<button class="icon-btn sm read-aloud"></button>');
  const draw = () => {
    const on = readingKey() === key;
    b.innerHTML = icon(on ? 'stop' : 'speaker', 15);
    b.title = t(on ? 'ra_stop' : 'ra_read');
    b.setAttribute('aria-label', b.title);
    b.setAttribute('aria-pressed', String(on));
  };
  b.onclick = () => { if (readingKey() === key) stopReading(); else readAloud(key, md, draw); draw(); };
  draw();
  return b;
}

// voices that come a moment after start: the answers drawn by then get their button
onVoicesReady(() => {
  if (!cur) return;
  for (const [id, el] of cur.els) {
    const actions = el.classList.contains('assistant') && el.querySelector('.turn-actions');
    if (actions && cur.turns.get(id)) { actions.__sig = null; renderAssistantTurn(el, cur.turns.get(id)); }
  }
});

/** "· 12s · $0.04" after the model name, with the token counts on hover. */
function turnStats(meta) {
  const u = meta.usage || {};
  const secs = (meta.duration_ms || 0) / 1000;
  const bits = [];
  if (secs >= 1) bits.push(secs >= 60 ? `${Math.floor(secs / 60)}m ${Math.round(secs % 60)}s` : `${secs.toFixed(secs < 10 ? 1 : 0)}s`);
  if (meta.cost_usd > 0) bits.push(fmtUsd(meta.cost_usd));
  if (!bits.length) return null;
  const tip = `${t('input_tokens')}: ${fmtTokens(u.input_tokens || 0)} · ${t('output_tokens')}: ${fmtTokens(u.output_tokens || 0)}${u.cache_read_input_tokens ? ` · ${t('cache_tokens')}: ${fmtTokens(u.cache_read_input_tokens)} / ${fmtTokens(u.cache_creation_input_tokens || 0)}` : ''}`;
  return h(`<span class="small muted turn-stats" title="${esc(tip)}">· ${esc(bits.join(' · '))}</span>`);
}

/** `live`: drawn for a streamed change (its new words are revealed in steps). */
function renderAssistantTurn(el, turn, live = false) {
  if (!el.querySelector('.assistant-body')) {
    el.innerHTML = `<div class="assistant-body"><div class="parts"></div><div class="streaming-indicator hidden">${spark(18, 'spark-anim')}</div></div><div class="turn-actions"></div>`;
  }
  const partsEl = el.querySelector('.parts');
  const streaming = !!turn.streaming;
  const ctx = {
    streaming,
    live,
    conversationId: cur && cur.id,
    openSettings: (tab) => emit('open-settings', tab),
    onRetry: () => retry(turn),
    onContinue: () => cur && cur.composer && sendInConversation(t('continue'), []).catch(errorToast),
    // only a backend that knows continue_reply (bootstrap's features) gets the button
    onResume: store.features && store.features.continue_reply && resumable(turn.parts || []) ? resumeReply : null,
  };
  const drawn = renderParts(partsEl, turn.parts || [], ctx);
  // artifact card clicks (cards only appear in parts just drawn)
  if (drawn) partsEl.querySelectorAll('.artifact-card:not([data-bound])').forEach((card) => {
    card.dataset.bound = '1';
    const open = () => artifacts.open(cur.id, cur.artifacts, card.dataset.identifier, Number(card.dataset.version));
    card.addEventListener('click', open);
    // a role=button opens with Space too
    card.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(); } });
  });
  // finished, stopped or failed: the whole text at once
  if (!streaming) revealAll(partsEl);
  el.querySelector('.streaming-indicator').classList.toggle('hidden', !streaming);
  renderTurnFiles(el, turn, streaming);
  const actions = el.querySelector('.turn-actions');
  if (streaming) { actions.innerHTML = ''; actions.__sig = ''; return; }
  const sig = JSON.stringify([turn.siblings, turn.meta && turn.meta.feedback, turn.parts.length, turn.meta && turn.meta.cost_usd, turn.meta && turn.meta.duration_ms]);
  if (actions.__sig === sig) return;
  actions.__sig = sig;
  actions.innerHTML = '';
  const nav = branchNav(turn);
  if (nav) actions.appendChild(nav);
  const text = turnPlainText(turn);
  // a reading of this answer as it was before (a retry, an edit) stops
  const said = readingKey();
  if (said && said.startsWith(`${turn.id}\n`) && said !== `${turn.id}\n${text}`) stopReading();
  if (text) {
    const cb = h(`<button class="icon-btn sm" title="${t('copy')}">${icon('copy', 15)}</button>`);
    cb.onclick = async () => { await copyText(text); cb.innerHTML = icon('check', 15); setTimeout(() => (cb.innerHTML = icon('copy', 15)), 1400); };
    actions.appendChild(cb);
    if (readAloudAvailable()) actions.appendChild(readAloudButton(`${turn.id}\n${text}`, text));
    // fork: continue from here in a new conversation
    const lastId = (turn.msg_ids || []).at(-1);
    if (lastId) {
      const forkBtn = h(`<button class="icon-btn sm" title="${t('fork_here')}">${icon('branch', 15)}</button>`);
      forkBtn.onclick = async () => {
        try {
          const r = await call('fork_conversation', { conversationId: cur.id, messageId: lastId });
          emit('conversations-changed');
          emit('navigate', { name: 'chat', params: { id: r.conversation_id } });
          toast(t('forked'));
        } catch (e) { errorToast(e); }
      };
      actions.appendChild(forkBtn);
    }
  }
  // a turn that failed has no answer to rate
  const failed = turn.parts.some((p) => p.kind === 'notice' && p.code === 'error');
  const fb = turn.meta && turn.meta.feedback;
  const up = h(`<button class="icon-btn sm ${fb === 'up' ? 'active' : ''}" title="${t('good')}">${icon('thumbUp', 15)}</button>`);
  const down = h(`<button class="icon-btn sm ${fb === 'down' ? 'active' : ''}" title="${t('bad')}">${icon('thumbDown', 15)}</button>`);
  const setFb = async (v) => {
    const nv = fb === v ? null : v;
    await call('message_feedback', { messageId: turn.id, value: nv }).catch(errorToast);
    turn.meta = { ...(turn.meta || {}), feedback: nv };
    renderAssistantTurn(el, turn);
  };
  up.onclick = () => setFb('up');
  down.onclick = () => setFb('down');
  const rb = idleOnly(h(`<button class="icon-btn sm" title="${t('retry')}">${icon('retry', 15)}</button>`), 'retry');
  rb.onclick = () => retry(turn);
  const rm = idleOnly(h(`<button class="icon-btn sm" title="${t('retry_with')}" style="width:18px;margin-left:-4px">${icon('chevDown', 12)}</button>`), 'retry');
  rm.onclick = () => !waitForIdle('retry') && showMenu(rm, store.models.filter((m) => m.available !== false && (m.featured || m.id === cur.conv.model)).map((m) => ({ label: m.name, sub: tOr(`model_d_${m.id}`, m.description), checked: m.id === cur.conv.model, onClick: () => retry(turn, m.id) })), { width: 260 });
  if (!failed) actions.append(up, down);
  actions.append(rb, rm);
  const model = turn.meta && turn.meta.model;
  // ("<synthetic>": a reply Claude Code made up itself, such as its sign-in error, not a model's)
  if (model && model !== '<synthetic>') actions.appendChild(h(`<span class="small muted model-label">${esc(modelInfo(model).name)}</span>`));
  const stats = turnStats(turn.meta || {});
  if (stats) actions.appendChild(stats);
  actions.appendChild(h(`<span class="disclaimer">${t('disclaimer')}</span>`));
  markLast();
}

/** Go on with the last answer from where it was cut short, as the same answer. */
async function resumeReply() {
  if (!cur || waitForIdle('resume')) return;
  try {
    await call('send_message', { req: { conversation_id: cur.id, text: '', continue_reply: true } });
    emit('conversations-changed', CHATS_ONLY);
  } catch (e) { errorToast(e); }
  await reload();
}

async function retry(turn, model) {
  if (!cur || !turn.parent_id || waitForIdle('retry')) return;
  try {
    await call('regenerate', { conversationId: cur.id, userMessageId: turn.parent_id, model: model || null });
    if (model) { cur.conv.model = model; cur.composer.setState({ model }); }
    await reload();
  } catch (e) { errorToast(e); }
}

function rebuildArtifacts() {
  if (!cur) return;
  const map = new Map();
  // those of turns not loaded yet first (the backend lists them), then the loaded turns'
  const all = [cur.earlier, ...cur.order.map((id) => (cur.turns.get(id) || {}).artifacts || [])];
  for (const a of all.flat()) {
    if (!map.has(a.identifier)) map.set(a.identifier, []);
    const list = map.get(a.identifier);
    list[a.version - 1] = a;
  }
  for (const [k, v] of map) map.set(k, v.filter(Boolean));
  const hadNone = cur.artifacts.size === 0;
  cur.artifacts = map;
  if (hadNone && map.size) renderTopbar();
  artifacts.update(cur.id, map);
  const step = panelStep(map.size > 0, artifacts.isOpen(), !!cur.panelParked);
  if (step === 'close') artifacts.close();
  else if (step === 'reopen') artifacts.togglePanel(cur.id, map);
  if (step) cur.panelParked = step === 'close';
}

// ------------------------------------------------------------- live events

// a streaming answer told to screen readers a sentence or so at a time
const answerVoice = streamAnnouncer((s) => announce(s), { prepare: speechText });

export function onTurnEvent({ conversation_id, turn }) {
  if (!cur || cur.id !== conversation_id) return;
  if (cur.retry && Date.now() >= cur.retry.until) clearRetry();
  if (!cur.els.has(turn.id)) rewindow();
  if (turn.role === 'assistant') answerVoice.update(turn.id, turnPlainText(turn), !turn.streaming);
  upsertTurn(turn, true);
  markLast();
  rebuildArtifacts();
  // auto-open an artifact that's being generated
  for (const a of turn.artifacts || []) {
    const key = `${a.identifier}@${a.version}`;
    if (turn.streaming && !a.complete && !a.interrupted && !cur.autoOpened.has(key) && store.settings.artifacts_enabled !== false) {
      cur.autoOpened.add(key);
      artifacts.open(cur.id, cur.artifacts, a.identifier, a.version, { streaming: true });
    }
  }
}

// Messages queued while Claude was responding, per conversation.
const queues = new Map();

/** Stop the turn in `id`. Queued messages come back into the box (text
 *  and attachments) instead of being sent once the turn ends. */
export function stopTurn(id) {
  const q = queues.get(id) || [];
  if (q.length) {
    queues.set(id, []);
    showQueue(id);
    if (cur && cur.id === id) {
      const draft = cur.composer.getText().trim();
      const text = q.map((x) => x.text).join('\n\n');
      cur.composer.setText(draft ? `${draft}\n\n${text}` : text);
      for (const x of q) for (const a of x.atts || []) cur.composer.addStaged(a);
    }
  }
  if (cur && cur.id === id) clearRetry();
  call('stop_generation', { conversationId: id }).catch(errorToast);
}

// ------------------------------------------------------- automatic retries

/** "Rate limited, retrying in 12 s (2/5)" under the composer, counting down,
 *  while the backend waits to try the request again. */
function showRetry(ev) {
  clearRetry();
  const hint = $('.composer-hint', cur.bottomEl);
  const until = Date.now() + Math.max(0, Number(ev.wait_ms) || 0);
  const draw = () => { hint.textContent = retryLabel(ev, Math.ceil((until - Date.now()) / 1000)); };
  cur.retry = { until, timer: setInterval(draw, 1000) };
  hint.classList.add('retrying');
  draw();
}

function clearRetry() {
  if (!cur || !cur.retry) return;
  clearInterval(cur.retry.timer);
  cur.retry = null;
  const hint = $('.composer-hint', cur.bottomEl);
  hint.textContent = '';
  hint.classList.remove('retrying');
}

on('generation-retry', (ev) => { if (ev && cur && cur.id === ev.conversation_id && cur.running !== false) showRetry(ev); });

// ------------------------------------------------------------- research

/** Research mode's progress over the composer, with Stop, until its turn ends. */
function showResearch(p) {
  const slot = $('.research-slot', cur.bottomEl);
  let card = slot.querySelector('.research-card');
  if (!card) {
    card = h(`<div class="research-card" role="status"><span class="spinner"></span><div class="grow"><div class="rs-line"></div><div class="rs-msg small muted ellipsis"></div></div><button class="btn sm">${icon('stop', 13)} ${esc(t('rs_stop'))}</button></div>`);
    const id = cur.id;
    card.querySelector('button').onclick = () => stopTurn(id);
    slot.appendChild(card);
  }
  card.querySelector('.rs-line').textContent = researchLine(p);
  card.querySelector('.rs-msg').textContent = researchQuery(p);
}

function clearResearch() {
  const slot = cur && $('.research-slot', cur.bottomEl);
  if (slot) slot.innerHTML = '';
}

on('research-progress', (p) => { if (p && cur && cur.id === p.conversation_id) showResearch(p); });

function showQueue(convId) {
  if (!cur || cur.id !== convId) return;
  const q = queues.get(convId) || [];
  cur.composer.showQueue(q, (i) => { q.splice(i, 1); showQueue(convId); });
}

async function sendQueued(convId) {
  const q = queues.get(convId);
  if (!q || !q.length) return;
  const next = q.shift();
  showQueue(convId);
  try {
    if (cur && cur.id === convId) await sendInConversation(next.text, next.atts);
    else { await call('send_message', { req: { conversation_id: convId, text: next.text, attachment_ids: next.atts.map((a) => a.id).filter(Boolean) } }); emit('conversations-changed', CHATS_ONLY); }
  } catch (e) { errorToast(e); }
}

export function onStateEvent({ conversation_id, running, error }) {
  if (running) store.running.add(conversation_id); else store.running.delete(conversation_id);
  emit('running-changed');
  if (!running && !error && (queues.get(conversation_id) || []).length) setTimeout(() => sendQueued(conversation_id), 300);
  if (!cur || cur.id !== conversation_id) return;
  cur.running = running;
  cur.composer.setRunning(running);
  cur.colEl.querySelectorAll('[data-busy]').forEach((b) => showBusy(b, running));
  if (!running) {
    answerVoice.finish();
    if (error) announce(errorMessage(error), true);
    clearRetry();
    clearResearch();
    cur.permEl.innerHTML = '';
    reload().catch(() => {});
    if (error && error.kind === 'no_api_key') emit('open-settings', 'account');
  }
}

// ------------------------------------------------------------ permissions

export function onPermissionRequest(p) {
  store.permissions.set(p.request_id, p);
  emit('running-changed');
  if (cur && cur.id === p.conversation_id) showPermission(p);
}

export function onPermissionResolved({ request_id }) {
  store.permissions.delete(request_id);
  emit('running-changed');
  dropCard(document.querySelector(`.perm-card[data-req="${CSS.escape(request_id)}"]`));
}

/** Remove a prompt's card; the keyboard goes on to the next prompt, or back to the box. */
function dropCard(el) {
  if (!el) return;
  // (a disabled button loses focus: respond notes where it was)
  const hadFocus = el.__hadFocus || el.contains(document.activeElement);
  el.remove();
  if (!hadFocus || !cur) return;
  const next = cur.permEl.querySelector('.perm-card .btn.primary');
  if (next) next.focus(); else cur.composer.focus();
}

/** Answer a prompt; its card goes once the answer is taken (it stays, to try again, if that fails). */
function respond(p, decision, message) {
  const el = document.querySelector(`.perm-card[data-req="${CSS.escape(p.request_id)}"]`);
  const controls = el ? [...el.querySelectorAll('button, input, textarea, select')] : [];
  if (el) el.__hadFocus = el.contains(document.activeElement);
  controls.forEach((x) => { x.disabled = true; });
  call('respond_permission', { requestId: p.request_id, decision, message: message || null })
    .then(() => dropCard(el), (e) => { controls.forEach((x) => { x.disabled = false; }); errorToast(e); });
}

function showPermission(p) {
  if (!cur || document.querySelector(`.perm-card[data-req="${CSS.escape(p.request_id)}"]`)) return;
  if (p.kind === 'question') { cur.permEl.appendChild(questionCard(p, (d, m) => respond(p, d, m))); scrollToBottom(true); return; }
  const input = p.input || {};
  let title = '', desc = '', detail = '';
  let buttons = [];
  if (p.kind === 'plan') {
    title = `${icon('compass', 17)} ${t('plan_ready')}`;
    detail = `<div class="md plan-md" style="max-height:300px;overflow:auto;margin-bottom:10px;font-size:14px"></div>`;
    buttons = [
      ['approve_edits', t('approve_plan_edits'), 'primary'],
      ['approve', t('approve_plan'), ''],
      ['deny', t('keep_planning'), 'ghost'],
    ];
  } else if (p.kind === 'code') {
    if (p.tool === 'Bash') {
      title = `${icon('terminal', 17)} ${t('run_command')}`;
      desc = input.description || '';
      detail = `<pre>${esc(input.command || '')}</pre>`;
    } else if (!['Write', 'Edit', 'MultiEdit', 'NotebookEdit'].includes(p.tool)) {
      title = `${icon('wrench', 17)} ${t('wants_to_use')} <span class="mono" style="font-size:13px">${esc(p.tool)}</span>`;
      desc = p.description || '';
      const j = JSON.stringify(input, null, 2);
      detail = j && j !== '{}' ? `<pre>${esc(j)}</pre>` : '';
    } else {
      title = `${icon(p.tool === 'Write' ? 'file' : 'edit', 17)} ${p.tool === 'Write' ? t('write_file') : t('make_edit')}`;
      desc = input.file_path || '';
      detail = p.diff ? unifiedDiffHtml(p.diff) : '';
    }
    buttons = [['once', t('allow_once'), 'primary'], ['session', t('allow_session'), ''], ['deny', t('deny'), 'ghost']];
  } else {
    const tool = permissionInfo(p);
    title = tool.title;
    desc = tool.desc;
    const j = JSON.stringify(input, null, 2);
    detail = `${tool.about}${j && j !== '{}' ? `<pre>${esc(j)}</pre>` : ''}`;
    buttons = [['once', t('allow_once'), 'primary'], ['chat', t('allow_chat'), ''], ['always', t('allow_always'), ''], ['deny', t('deny'), 'ghost']];
  }
  const card = h(`<div class="perm-card" data-req="${esc(p.request_id)}" role="group" tabindex="-1" aria-labelledby="pt-${esc(p.request_id)}">
    <div class="ptitle" id="pt-${esc(p.request_id)}">${title}</div>
    ${desc ? `<div class="pdesc mono">${esc(desc)}</div>` : '<div style="height:8px"></div>'}
    ${detail}
    <div class="pactions"></div>
    <div class="feedback"><input class="input" placeholder="${esc(t('tell_claude'))}"><button class="btn sm">${t('deny')}</button></div>
  </div>`);
  const actions = card.querySelector('.pactions');
  for (const [d, label, cls] of buttons) {
    const b = h(`<button class="btn sm ${cls}">${esc(label)}</button>`);
    b.onclick = () => {
      if (d === 'deny' && !card.classList.contains('with-feedback')) {
        card.classList.add('with-feedback');
        card.querySelector('.feedback input').focus();
        return;
      }
      respond(p, d);
    };
    actions.appendChild(b);
  }
  const fb = card.querySelector('.feedback');
  const sendDeny = () => respond(p, 'deny', fb.querySelector('input').value);
  fb.querySelector('button').onclick = sendDeny;
  fb.querySelector('input').addEventListener('keydown', (e) => { if (e.key === 'Enter') sendDeny(); });
  // the keyboard: Enter on the card takes the first choice (Allow once), Esc denies
  card.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); respond(p, 'deny'); }
    else if (e.key === 'Enter' && e.target === card) { e.preventDefault(); respond(p, buttons[0][0]); }
  });
  cur.permEl.appendChild(card);
  const said = card.querySelector('.ptitle').textContent.trim();
  announce(`${/[.?!:]$/.test(said) ? said : `${said}.`} ${t('perm_keys')}`, true);
  // not while you're typing a message
  const a = document.activeElement;
  if (!(a && (a.tagName === 'TEXTAREA' || a.tagName === 'INPUT') && a.value.trim())) actions.querySelector('button').focus({ preventScroll: true });
  if (p.kind === 'plan') {
    call('render_markdown', { text: input.plan || '' }).then((html) => { const m = card.querySelector('.plan-md'); if (m) m.innerHTML = html; }).catch(() => {});
  }
  scrollToBottom(true);
}

export function closeCurrent() {
  openSeq++; // a load still under way is for a view that's gone
  clearRetry();
  stopReading();
  answerVoice.reset();
  if (cur && cur.ro) cur.ro.disconnect();
  // an incognito chat goes when it's closed, without asking
  if (cur && isIncognito(cur.conv)) discardIncognito(cur.id);
  if (cur) opened(null);
  cur = null;
}

on('compacted', ({ conversation_id, auto, error }) => { if (auto && !error && cur && cur.id === conversation_id) reload(); });
// Claude Code idles on a workflow: what you type now goes straight in (the workflow goes on) instead of queueing
on('cc-waiting', ({ conversation_id, waiting }) => {
  if (cur && cur.id === conversation_id && cur.running) cur.composer.setRunning(true, waiting);
  // what was queued while the turn worked goes now, not after the workflow
  if (waiting && (queues.get(conversation_id) || []).length) sendQueued(conversation_id);
});
// a message written by a button elsewhere (an artifact's "Try fixing"): sent as if typed, or queued while Claude responds
onUi('send-message', ({ conversationId, text }) => {
  if (!cur || cur.id !== conversationId) return;
  if (cur.running && !cur.composer.waiting()) {
    if (!queues.has(cur.id)) queues.set(cur.id, []);
    queues.get(cur.id).push({ text, atts: [] });
    showQueue(cur.id);
    return;
  }
  sendInConversation(text, []).catch(errorToast);
});
// the CLI ignored ultracode (dynamic workflows off for the account): say so once per chat
const ucWarned = new Set();
on('ultracode-unavailable', ({ conversation_id }) => {
  if (!cur || cur.id !== conversation_id || ucWarned.has(conversation_id)) return;
  ucWarned.add(conversation_id);
  toast(t('ultracode_unavailable'), 'error');
});
// fast mode asked for, and the account's CLI runs the turn at standard speed (claudecode.rs): once per chat
const fastWarned = new Set();
on('fast-mode-unavailable', ({ conversation_id }) => {
  if (!cur || cur.id !== conversation_id || fastWarned.has(conversation_id)) return;
  fastWarned.add(conversation_id);
  toast(t('fast_unavailable'));
});
