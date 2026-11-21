// Conversation view: turns, streaming updates, branching, permissions.
import { openSaveCommand } from './save_command.js';
import { watchBadge } from './watch_ui.js';
import { contextMeter, compactDivider } from './context_ui.js';
import { openRewind } from './rewind.js';
import * as workspace from './workspace.js';
import { icon, spark } from './icons.js';
import { t } from './i18n.js';
import { call, on } from './api.js';
import { store, emit, modelInfo, displayName } from './store.js';
import { $, esc, h, showMenu, toast, errorToast, initials, attachmentSub, lightbox, confirmDialog, promptDialog, fmtTokens, fmtUsd } from './util.js';
import { createComposer } from './composer.js';
import { renderParts, turnPlainText, copyText, unifiedDiffHtml } from './parts.js';
import * as artifacts from './artifacts.js';

let cur = null;

export function current() { return cur; }

function viewEl() { return $('#view'); }

function composerInitial(conv) {
  const o = conv.options || {};
  return {
    model: conv.model,
    thinking: o.thinking ?? (conv.kind === 'code' ? true : !!store.settings.thinking_default),
    effort: o.effort || null,
    web_search: o.web_search ?? false,
    style: o.style || 'normal',
    disabled_servers: o.disabled_servers || [],
    permission_mode: o.permission_mode || store.settings.code_permission_mode || 'default',
  };
}

// the latest open request wins: a slow load of a big conversation must not
// replace the one opened after it
let openSeq = 0;

export async function openConversation(id, { focus = true } = {}) {
  const seq = ++openSeq;
  const data = await call('get_conversation', { id });
  if (seq !== openSeq) return;
  const conv = data.conversation;
  store.codeRoot = conv.kind === 'code' ? conv.cwd : null;
  emit('conversation-opened', { id, kind: conv.kind });
  const v = viewEl();
  v.innerHTML = '';
  const top = h(`<div class="topbar"></div>`);
  const scroll = h(`<div class="chat-scroll"><div class="chat-col"></div></div>`);
  const bottom = h(`<div class="chat-bottom"><div class="inner"><div class="perm-area"></div><div class="composer-slot"></div><div class="composer-hint"></div></div></div>`);
  v.append(top, scroll, bottom);

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
    topEl: top,
    running: data.running,
    artifacts: new Map(),
    autoOpened: new Set(),
  };
  const composer = createComposer({
    mode: conv.kind === 'code' ? 'code' : 'chat',
    placeholder: conv.kind === 'code' ? t('code_placeholder') : t('reply_placeholder'),
    initial: composerInitial(conv),
    draftKey: id,
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
  composer.setRunning(!!data.running);

  renderTopbar();
  for (const turn of data.turns) upsertTurn(turn, false);
  markLast();
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
  artifacts.onConversationOpened(id, cur.artifacts);
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
    if (c.options && c.options.engine === 'claude-code') cur.topEl.append(h(`<span class="active-tool" title="${esc(t('engine_cc'))}">${icon('terminal', 12)} Claude Code</span>`));
    if (c.options && c.options.worktree && c.options.worktree.branch) cur.topEl.append(h(`<span class="active-tool wt" title="${esc(t('worktree_topbar', { path: c.options.worktree.path }))}">${icon('code', 12)} ${esc(c.options.worktree.branch)}</span>`));
  }
  const tb = h(`<button class="title-btn">${c.starred ? `<span style="color:var(--accent)">${icon('starFill', 14)}</span>` : ''}<span class="t">${esc(c.title || t('new_chat'))}</span>${icon('chevDown', 14)}</button>`);
  tb.onclick = () => conversationMenu(tb, c, { onChange: () => refreshConversationMeta() });
  cur.topEl.append(tb, h('<div class="spacer"></div>'));
  if (c.kind === 'code') cur.topEl.append(watchBadge(c));
  const outline = h(`<button class="icon-btn" title="${esc(t('outline_title'))}">${icon('list', 17)}</button>`);
  outline.onclick = () => showOutline(outline);
  cur.topEl.append(outline);
  cur.topEl.append(contextMeter(cur.id, () => { cur.composer.setText('/cost'); cur.composer.send(); }));
  const usage = h(`<button class="icon-btn" title="${t('usage')}">${icon('info', 17)}</button>`);
  usage.onclick = () => showUsage(usage);
  cur.topEl.append(usage);
  if (c.kind === 'code') cur.topEl.append(workspace.toggleButton());
  if (cur.artifacts.size) {
    const ab = h(`<button class="icon-btn" title="${t('artifacts')}">${icon('layers', 17)}</button>`);
    ab.onclick = () => artifacts.togglePanel(cur.id, cur.artifacts);
    cur.topEl.append(ab);
  }
}

async function showUsage(anchor) {
  try {
    const u = await call('usage_stats', { conversationId: cur.id });
    showMenu(anchor, [
      { header: `${t('usage')} · ${modelInfo(cur.conv.model).name}` },
      { label: `${fmtTokens(u.input_tokens)} in · ${fmtTokens(u.output_tokens)} out`, icon: 'zap', disabled: true },
      { label: `cache: ${fmtTokens(u.cache_read_input_tokens)} read · ${fmtTokens(u.cache_creation_input_tokens)} write`, icon: 'history', disabled: true },
      { label: `${t('cost')}: $${u.cost_usd.toFixed(4)}`, icon: 'info', disabled: true },
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
  items.push(
    { sep: true },
    { label: t('export_md'), icon: 'download', onClick: () => exportConv(c, 'md') },
    { label: t('export_json'), icon: 'download', onClick: () => exportConv(c, 'json') },
    { label: t('export_html'), icon: 'download', onClick: () => exportConv(c, 'html') },
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
    const data = await call('get_conversation', { id: cur.id });
    cur.conv = data.conversation;
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
  appendOptimisticUser(text, atts);
  try {
    await call('send_message', { req: { conversation_id: id, text, attachment_ids: atts.map((a) => a.id).filter(Boolean) } });
  } catch (e) {
    await reload();
    throw e;
  }
  await reload();
  emit('conversations-changed');
  return true;
}

function appendOptimisticUser(text, atts) {
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
  const data = await call('get_conversation', { id });
  if (!cur || cur.id !== id) return;
  cur.conv = data.conversation;
  const keep = new Set(data.turns.map((x) => x.id));
  for (const [tid, el] of cur.els) if (!keep.has(tid)) { el.remove(); cur.els.delete(tid); cur.turns.delete(tid); }
  cur.order = [];
  for (const turn of data.turns) upsertTurn(turn, false);
  // ensure DOM order
  for (const tid of cur.order) cur.colEl.appendChild(cur.els.get(tid));
  markLast();
  rebuildArtifacts();
  renderTopbar();
  cur.running = data.running;
  cur.composer.setRunning(!!data.running);
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
}

function updateScrollButton() {
  if (!cur) return;
  const bottom = cur.scrollEl.parentElement.querySelector('.chat-bottom');
  let btn = bottom.querySelector('.scroll-down');
  const show = !nearBottom();
  if (show && !btn) {
    btn = h(`<button class="scroll-down" aria-label="scroll">${icon('arrowDown', 16)}</button>`);
    btn.onclick = () => { cur.stick = true; cur.scrollEl.scrollTo({ top: cur.scrollEl.scrollHeight, behavior: 'smooth' }); };
    bottom.appendChild(btn);
  } else if (!show && btn) btn.remove();
}

function markLast() {
  if (!cur) return;
  cur.els.forEach((el) => el.classList.remove('last'));
  const lastId = cur.order[cur.order.length - 1];
  if (lastId && cur.els.get(lastId)) cur.els.get(lastId).classList.add('last');
  // disclaimer only on the last assistant turn
  cur.els.forEach((el, id) => {
    const d = el.querySelector('.disclaimer');
    if (d) d.classList.toggle('hidden', id !== lastId);
  });
}

function upsertTurn(turn, live = true) {
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
    cur.colEl.appendChild(el);
  }
  cur.turns.set(turn.id, turn);
  if (!cur.order.includes(turn.id)) cur.order.push(turn.id);
  if (turn.role === 'user') renderUserTurn(el, turn); else renderAssistantTurn(el, turn);
  if (live && stick) scrollToBottom(true);
}

function attachmentsHtml(items) {
  return `<div class="user-atts">${items.map((a) => a.src && a.kind === 'image'
    ? `<img src="${esc(a.src)}" alt="${esc(a.name)}" data-zoom>`
    : `<div class="att"><div class="ficon">${a.kind === 'pdf' ? 'PDF' : esc((a.name || '').split('.').pop().slice(0, 4) || 'TXT')}</div><div class="info"><div class="n">${esc(a.name)}</div><div class="s">${a.missing ? esc(t('att_missing')) : attachmentSub(a)}</div></div></div>`).join('')}</div>`;
}

function branchNav(turn) {
  const s = turn.siblings;
  if (!s || s.count <= 1) return null;
  const nav = h(`<span class="branch-nav"><button class="icon-btn sm" data-d="-1" ${s.index === 0 ? 'disabled' : ''}>${icon('chevLeft', 14)}</button><span class="count">${s.index + 1} / ${s.count}</span><button class="icon-btn sm" data-d="1" ${s.index >= s.count - 1 ? 'disabled' : ''}>${icon('chevRight', 14)}</button></span>`);
  nav.querySelectorAll('button').forEach((b) => b.addEventListener('click', async () => {
    const target = s.ids[s.index + Number(b.dataset.d)];
    if (!target) return;
    try {
      await call('switch_branch', { conversationId: cur.id, messageId: target });
      await reload();
    } catch (e) { errorToast(e); }
  }));
  return nav;
}

// ---- outline: jump between your messages

/** The user-message elements currently shown, in order. */
function userTurnEls() {
  return cur ? [...cur.colEl.querySelectorAll('.turn.user')].filter((e) => e.querySelector('.user-text')) : [];
}

function jumpTo(el) {
  el.scrollIntoView({ block: 'start', behavior: 'smooth' });
  el.classList.add('flash');
  setTimeout(() => el.classList.remove('flash'), 1200);
}

function showOutline(anchor) {
  const els = userTurnEls();
  if (!els.length) { toast(t('outline_empty')); return; }
  showMenu(anchor, [
    { header: t('outline_title') },
    ...els.map((e, i) => {
      const text = e.querySelector('.user-text').textContent.trim().replace(/\s+/g, ' ');
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
  const els = userTurnEls();
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
  el.innerHTML = `${turn.meta && turn.meta.compact_summary ? '<div class="cd-slot"></div>' : ''}${atts ? attachmentsHtml(atts.items) : ''}
    ${text ? `<div class="user-bubble"><span class="avatar">${esc(initials(name))}</span><div class="user-text">${esc(text)}</div></div>` : ''}
    <div class="turn-actions"></div>`;
  el.querySelectorAll('[data-zoom]').forEach((img) => img.addEventListener('click', () => lightbox(img.src)));
  const cds = el.querySelector('.cd-slot');
  if (cds) cds.replaceWith(compactDivider(turn.meta.compact_summary, turn.meta.compact_auto));
  const ut = el.querySelector('.user-text');
  if (ut && text.length > 1500) {
    ut.classList.add('clamped');
    ut.addEventListener('click', () => ut.classList.remove('clamped'));
  }
  const actions = el.querySelector('.turn-actions');
  if (String(turn.id).startsWith('pending-')) return;
  const nav = branchNav(turn);
  if (nav) actions.appendChild(nav);
  if (cur && cur.conv.kind === 'code') {
    const rb = h(`<button class="icon-btn sm" title="${t('rewind_here')}">${icon('history', 15)}</button>`);
    rb.onclick = () => rewindTo(turn, text);
    actions.appendChild(rb);
  }
  if (text) {
    const eb = h(`<button class="icon-btn sm" title="${t('edit')}">${icon('edit', 15)}</button>`);
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
}

function rewindTo(turn, text) {
  const c = cur;
  openRewind({
    conversationId: c.id,
    messageId: turn.id,
    preview: text,
    onConversationRewound: async (msgText) => {
      if (cur !== c) return;
      await reload();
      c.composer.setText(msgText || text);
      c.composer.focus();
    },
  });
}

/** /rewind: pick one of the user's messages. */
export function openRewindPicker() {
  if (!cur) return;
  const users = cur.order.map((id) => cur.turns.get(id)).filter((tn) => tn && tn.role === 'user');
  if (!users.length) return;
  const items = users.slice(-15).reverse().map((tn) => {
    const text = (tn.parts.find((p) => p.kind === 'user_text') || {}).text || '';
    return { label: text.slice(0, 80) || '…', icon: 'history', onClick: () => rewindTo(tn, text) };
  });
  const anchor = cur.composer.el;
  showMenu(anchor, items, { placement: 'top-start', width: 420 });
}

function startEdit(el, turn, text) {
  if (cur.running) { toast(t('stop')); return; }
  const box = h(`<div class="edit-box"><textarea></textarea><div class="row"><button class="btn sm" data-a="cancel">${t('cancel')}</button><button class="btn sm primary" data-a="save">${t('save')}</button></div></div>`);
  const ta = box.querySelector('textarea');
  ta.value = text;
  const bubble = el.querySelector('.user-bubble');
  bubble.replaceWith(box);
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
      emit('conversations-changed');
    } catch (e) {
      errorToast(e);
      saving = false;
      saveBtn.disabled = false;
    }
  };
  box.querySelector('[data-a=cancel]').onclick = cancel;
  box.querySelector('[data-a=save]').onclick = save;
  ta.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.preventDefault(); cancel(); }
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

const EDIT_TOOLS = new Set(['Write', 'Edit', 'MultiEdit', 'NotebookEdit']);
const lineCount = (s) => (s ? String(s).replace(/\n$/, '').split('\n').length : 0);

/** Files a turn wrote or edited, with rough +/- line counts from the tool inputs. */
export function editedFiles(parts, cwd) {
  const files = new Map();
  for (const p of parts || []) {
    if (p.kind !== 'tool' || !EDIT_TOOLS.has(p.name) || p.status === 'error' || p.status === 'cancelled' || p.status === 'denied') continue;
    if (p.result && p.result.is_error) continue;
    const inp = p.input || {};
    const abs = inp.file_path || inp.notebook_path;
    if (!abs) continue;
    let rel = String(abs);
    if (cwd && rel.startsWith(cwd.replace(/\/$/, '') + '/')) rel = rel.slice(cwd.replace(/\/$/, '').length + 1);
    const f = files.get(rel) || { path: rel, added: 0, removed: 0, created: false };
    if (/File created successfully/.test((p.result && p.result.text) || '')) f.created = true;
    if (p.name === 'Write') f.added += lineCount(inp.content);
    else if (p.name === 'Edit') { f.added += lineCount(inp.new_string); f.removed += lineCount(inp.old_string); }
    else if (p.name === 'MultiEdit') for (const e of inp.edits || []) { f.added += lineCount(e.new_string); f.removed += lineCount(e.old_string); }
    files.set(rel, f);
  }
  return [...files.values()];
}

/** "Edited N files" summary under a finished code-session turn. */
function renderTurnFiles(el, turn, streaming) {
  let box = el.querySelector('.turn-files');
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

function renderAssistantTurn(el, turn) {
  if (!el.querySelector('.assistant-body')) {
    el.innerHTML = `<div class="assistant-body"><div class="parts"></div><div class="streaming-indicator hidden">${spark(18, 'spark-anim')}</div></div><div class="turn-actions"></div>`;
  }
  const partsEl = el.querySelector('.parts');
  const ctx = {
    openSettings: (tab) => emit('open-settings', tab),
    onRetry: () => retry(turn),
    onContinue: () => cur && cur.composer && sendInConversation(t('continue'), []).catch(errorToast),
  };
  renderParts(partsEl, turn.parts || [], ctx);
  // artifact card clicks
  partsEl.querySelectorAll('.artifact-card:not([data-bound])').forEach((card) => {
    card.dataset.bound = '1';
    const open = () => artifacts.open(cur.id, cur.artifacts, card.dataset.identifier, Number(card.dataset.version));
    card.addEventListener('click', open);
    card.addEventListener('keydown', (e) => { if (e.key === 'Enter') open(); });
  });
  const streaming = !!turn.streaming;
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
  if (text) {
    const cb = h(`<button class="icon-btn sm" title="${t('copy')}">${icon('copy', 15)}</button>`);
    cb.onclick = async () => { await copyText(text); cb.innerHTML = icon('check', 15); setTimeout(() => (cb.innerHTML = icon('copy', 15)), 1400); };
    actions.appendChild(cb);
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
  const rb = h(`<button class="icon-btn sm" title="${t('retry')}">${icon('retry', 15)}</button>`);
  rb.onclick = () => retry(turn);
  const rm = h(`<button class="icon-btn sm" title="${t('retry_with')}" style="width:18px;margin-left:-4px">${icon('chevDown', 12)}</button>`);
  rm.onclick = () => showMenu(rm, store.models.filter((m) => m.available !== false && (m.featured || m.id === cur.conv.model)).map((m) => ({ label: m.name, sub: m.description, checked: m.id === cur.conv.model, onClick: () => retry(turn, m.id) })), { width: 260 });
  actions.append(up, down, rb, rm);
  const model = turn.meta && turn.meta.model;
  if (model) actions.appendChild(h(`<span class="small muted model-label">${esc(modelInfo(model).name)}</span>`));
  const stats = turnStats(turn.meta || {});
  if (stats) actions.appendChild(stats);
  actions.appendChild(h(`<span class="disclaimer">${t('disclaimer')}</span>`));
  markLast();
}

async function retry(turn, model) {
  if (!cur) return;
  if (!turn.parent_id) return;
  try {
    await call('regenerate', { conversationId: cur.id, userMessageId: turn.parent_id, model: model || null });
    if (model) { cur.conv.model = model; cur.composer.setState({ model }); }
    await reload();
  } catch (e) { errorToast(e); }
}

function rebuildArtifacts() {
  if (!cur) return;
  const map = new Map();
  for (const id of cur.order) {
    const turn = cur.turns.get(id);
    for (const a of (turn && turn.artifacts) || []) {
      if (!map.has(a.identifier)) map.set(a.identifier, []);
      const list = map.get(a.identifier);
      list[a.version - 1] = a;
    }
  }
  for (const [k, v] of map) map.set(k, v.filter(Boolean));
  const hadNone = cur.artifacts.size === 0;
  cur.artifacts = map;
  if (hadNone && map.size) renderTopbar();
  artifacts.update(cur.id, map);
}

// ------------------------------------------------------------- live events

export function onTurnEvent({ conversation_id, turn }) {
  if (!cur || cur.id !== conversation_id) return;
  upsertTurn(turn, true);
  markLast();
  rebuildArtifacts();
  // auto-open an artifact that's being generated
  for (const a of turn.artifacts || []) {
    const key = `${a.identifier}@${a.version}`;
    if (turn.streaming && !a.complete && !cur.autoOpened.has(key) && store.settings.artifacts_enabled !== false) {
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
  call('stop_generation', { conversationId: id }).catch(errorToast);
}

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
    else { await call('send_message', { req: { conversation_id: convId, text: next.text, attachment_ids: next.atts.map((a) => a.id).filter(Boolean) } }); emit('conversations-changed'); }
  } catch (e) { errorToast(e); }
}

export function onStateEvent({ conversation_id, running, error }) {
  if (running) store.running.add(conversation_id); else store.running.delete(conversation_id);
  emit('running-changed');
  if (!running && !error && (queues.get(conversation_id) || []).length) setTimeout(() => sendQueued(conversation_id), 300);
  if (!cur || cur.id !== conversation_id) return;
  cur.running = running;
  cur.composer.setRunning(running);
  if (!running) {
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
  const el = document.querySelector(`.perm-card[data-req="${CSS.escape(request_id)}"]`);
  if (el) el.remove();
}

function respond(p, decision, message) {
  call('respond_permission', { requestId: p.request_id, decision, message: message || null }).catch(errorToast);
  const el = document.querySelector(`.perm-card[data-req="${CSS.escape(p.request_id)}"]`);
  if (el) el.remove();
}

function showPermission(p) {
  if (!cur || document.querySelector(`.perm-card[data-req="${CSS.escape(p.request_id)}"]`)) return;
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
    title = `${icon('plug', 17)} ${t('wants_to_use')} <span class="mono" style="font-size:13px">${esc(p.tool_name || p.tool)}</span>`;
    desc = `${t('from_server')} ${p.server}`;
    const j = JSON.stringify(input, null, 2);
    detail = j && j !== '{}' ? `<pre>${esc(j)}</pre>` : '';
    buttons = [['once', t('allow_once'), 'primary'], ['chat', t('allow_chat'), ''], ['always', t('allow_always'), ''], ['deny', t('deny'), 'ghost']];
  }
  const card = h(`<div class="perm-card" data-req="${esc(p.request_id)}">
    <div class="ptitle">${title}</div>
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
  cur.permEl.appendChild(card);
  if (p.kind === 'plan') {
    call('render_markdown', { text: input.plan || '' }).then((html) => { const m = card.querySelector('.plan-md'); if (m) m.innerHTML = html; }).catch(() => {});
  }
  scrollToBottom(true);
}

export function closeCurrent() {
  openSeq++; // a load still under way is for a view that's gone
  if (cur && cur.ro) cur.ro.disconnect();
  cur = null;
}

on('compacted', ({ conversation_id, auto, error }) => { if (auto && !error && cur && cur.id === conversation_id) reload(); });
