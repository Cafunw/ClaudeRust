// Conversation view: turns, streaming updates, branching, permissions.
import { icon, spark } from './icons.js';
import { t } from './i18n.js';
import { call } from './api.js';
import { store, emit, modelInfo, displayName } from './store.js';
import { $, esc, h, showMenu, toast, errorToast, initials, humanSize, lightbox, confirmDialog, promptDialog, fmtTokens } from './util.js';
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

export async function openConversation(id, { focus = true } = {}) {
  const data = await call('get_conversation', { id });
  const conv = data.conversation;
  store.codeRoot = conv.kind === 'code' ? conv.cwd : null;
  const v = viewEl();
  v.innerHTML = '';
  const top = h(`<div class="topbar"></div>`);
  const scroll = h(`<div class="chat-scroll"><div class="chat-col"></div></div>`);
  const bottom = h(`<div class="chat-bottom"><div class="inner"><div class="perm-area"></div><div class="composer-slot"></div><div class="composer-hint"></div></div></div>`);
  v.append(top, scroll, bottom);

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
    onStop: () => call('stop_generation', { conversationId: id }).catch(errorToast),
    onOptions: (patch) => persistOptions(patch),
    onEditLast: () => editLastUserMessage(),
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
  if (focus) composer.focus();
  artifacts.onConversationOpened(id, cur.artifacts);
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
    chip.onclick = () => call('reveal_path', { path: c.cwd }).catch(errorToast);
    cur.topEl.append(chip, h(`<span class="muted">/</span>`));
    if (c.options && c.options.engine === 'claude-code') cur.topEl.append(h(`<span class="active-tool" title="${esc(t('engine_cc'))}">${icon('terminal', 12)} Claude Code</span>`));
  }
  const tb = h(`<button class="title-btn">${c.starred ? `<span style="color:var(--accent)">${icon('starFill', 14)}</span>` : ''}<span class="t">${esc(c.title || t('new_chat'))}</span>${icon('chevDown', 14)}</button>`);
  tb.onclick = () => conversationMenu(tb, c, { onChange: () => refreshConversationMeta() });
  cur.topEl.append(tb, h('<div class="spacer"></div>'));
  const usage = h(`<button class="icon-btn" title="${t('usage')}">${icon('info', 17)}</button>`);
  usage.onclick = () => showUsage(usage);
  cur.topEl.append(usage);
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
  items.push(
    { sep: true },
    { label: t('export_md'), icon: 'download', onClick: () => exportConv(c, 'md') },
    { label: t('export_json'), icon: 'download', onClick: () => exportConv(c, 'json') },
    { sep: true },
    { label: t('delete'), icon: 'trash', danger: true, onClick: async () => {
      if (!(await confirmDialog(t('confirm_delete_chat')))) return;
      await call('delete_conversation', { id: c.id });
      emit('conversations-changed');
      if (onDelete) onDelete(); else if (cur && cur.id === c.id) emit('navigate', { name: c.kind === 'code' ? 'code' : 'new' });
    } },
  );
  showMenu(anchor, items, { width: 220 });
}

async function exportConv(c, fmt) {
  try {
    const content = await call('export_conversation', { id: c.id, format: fmt === 'json' ? 'json' : 'md' });
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
  cur.stick = nearBottom();
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
    : `<div class="att"><div class="ficon">${a.kind === 'pdf' ? 'PDF' : esc((a.name || '').split('.').pop().slice(0, 4) || 'TXT')}</div><div class="info"><div class="n">${esc(a.name)}</div><div class="s">${humanSize(a.size || 0)}</div></div></div>`).join('')}</div>`;
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

function renderUserTurn(el, turn) {
  const text = (turn.parts.find((p) => p.kind === 'user_text') || {}).text || '';
  const atts = turn.parts.find((p) => p.kind === 'attachments');
  const name = displayName();
  el.innerHTML = `${atts ? attachmentsHtml(atts.items) : ''}
    ${text ? `<div class="user-bubble"><span class="avatar">${esc(initials(name))}</span><div class="user-text">${esc(text)}</div></div>` : ''}
    <div class="turn-actions"></div>`;
  el.querySelectorAll('[data-zoom]').forEach((img) => img.addEventListener('click', () => lightbox(img.src)));
  const ut = el.querySelector('.user-text');
  if (ut && text.length > 1500) {
    ut.classList.add('clamped');
    ut.addEventListener('click', () => ut.classList.remove('clamped'));
  }
  const actions = el.querySelector('.turn-actions');
  if (String(turn.id).startsWith('pending-')) return;
  const nav = branchNav(turn);
  if (nav) actions.appendChild(nav);
  if (text) {
    const eb = h(`<button class="icon-btn sm" title="${t('edit')}">${icon('edit', 15)}</button>`);
    eb.onclick = () => startEdit(el, turn, text);
    const cb = h(`<button class="icon-btn sm" title="${t('copy')}">${icon('copy', 15)}</button>`);
    cb.onclick = async () => { await copyText(text); cb.innerHTML = icon('check', 15); setTimeout(() => (cb.innerHTML = icon('copy', 15)), 1400); };
    actions.append(eb, cb);
  }
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
  const save = async () => {
    const v = ta.value.trim();
    if (!v) return;
    try {
      await call('send_message', { req: { conversation_id: cur.id, text: v, edit: true, parent_id: turn.parent_id || '' } });
      await reload();
      emit('conversations-changed');
    } catch (e) { errorToast(e); }
  };
  box.querySelector('[data-a=cancel]').onclick = cancel;
  box.querySelector('[data-a=save]').onclick = save;
  ta.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.preventDefault(); cancel(); }
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); save(); }
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
  const actions = el.querySelector('.turn-actions');
  if (streaming) { actions.innerHTML = ''; actions.__sig = ''; return; }
  const sig = JSON.stringify([turn.siblings, turn.meta && turn.meta.feedback, turn.parts.length]);
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

export function onStateEvent({ conversation_id, running, error }) {
  if (running) store.running.add(conversation_id); else store.running.delete(conversation_id);
  emit('running-changed');
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

export function closeCurrent() { cur = null; }
