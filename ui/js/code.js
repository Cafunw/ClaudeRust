// Code page: start coding sessions in a local folder and list past ones.
import { openRoutines } from './routines.js';
import * as workspace from './workspace.js';
import { icon, spark } from './icons.js';
import { t } from './i18n.js';
import { call } from './api.js';
import { store, emit, on, saveLocal, loadLocal, newChatModel, newChatEffort } from './store.js';
import { $, esc, h, relTime, errorToast, toast, confirmDialog, searchToggle, showMenu, promptDialog, convTitle, snippetHtml, hasWords } from './util.js';
import { createComposer } from './composer.js';
import { startConversation } from './home.js';
import { conversationMenu } from './chat.js';

// a newer render (a double press, or leaving the page) supersedes this one
let renderSeq = 0;

export async function renderCode(view, params = {}) {
  const seq = ++renderSeq;
  const stale = () => seq !== renderSeq || store.route.name !== 'code';
  let folder = loadLocal().codeFolder || '';
  const sessions = await call('list_conversations', { kind: 'code' }).catch(() => []);
  if (stale()) return;
  const recentFolders = [...new Set(sessions.map((s) => s.cwd).filter(Boolean))].slice(0, 6);
  if (!folder && recentFolders.length) folder = recentFolders[0];
  const base = (p) => p.split(/[\\/]/).filter(Boolean).pop() || p;

  // As in Claude Desktop: a quiet stage (where to work), the composer docked at the bottom and
  // the environment (engine, folder, worktree) in the row under it.
  view.innerHTML = `<div class="topbar"><span class="top-title">${esc(t('new_session'))}</span><span class="spacer"></span>
      <button class="icon-btn" data-act="routines" title="${esc(t('rt_title'))}" aria-label="${esc(t('rt_title'))}">${icon('clock', 16)}</button>
      <button class="icon-btn" data-act="import" title="${esc(t('imp_cc_btn'))}" aria-label="${esc(t('imp_cc_btn'))}">${icon('download', 16)}</button>
      <button class="icon-btn" data-act="sessions" title="${esc(t('sessions'))}" aria-label="${esc(t('sessions'))}">${icon('list', 16)}</button></div>
    <div class="code-stage"><span class="env-mark">${spark(24)}</span>
      <div class="env-card"><div class="env-list"></div>
        <button class="env-choose" data-act="folder">${icon('folder', 14)}<span class="grow">${esc(t('choose_folder'))}</span>${icon('chevDown', 12)}</button></div>
      <button class="env-card env-wt" data-act="wt"></button>
    </div>
    <div class="code-dock"><div class="inner"><div class="composer-slot"></div></div></div>`;

  // Engine: the official Claude Code CLI (signs in with your Claude account) or the built-in agent (API key).
  const cc = await call('claude_code_info', {}).catch(() => ({ found: false }));
  if (stale()) return;
  const defEngine = store.settings.code_engine === 'builtin' ? 'builtin'
    : store.settings.code_engine === 'claude-code' ? 'claude-code' : (cc.found ? 'claude-code' : 'builtin');
  let engine = loadLocal().codeEngine || defEngine;
  const engineIssue = () => (engine === 'claude-code' ? (cc.found ? '' : t('cc_missing')) : (store.settings.has_api_key ? '' : t('no_key')));
  // Worktree isolation: auto (setting) → on → off
  const wtState = () => loadLocal().codeWorktree || 'auto';

  const envRow = h('<div class="env-row"></div>');
  const drawEnv = () => {
    const rows = [...new Set([folder, ...recentFolders].filter(Boolean))].slice(0, 3);
    const list = $('.env-list', view);
    list.innerHTML = rows.length ? '' : `<div class="env-empty">${esc(t('folder_missing'))}</div>`;
    for (const f of rows) {
      const b = h(`<button class="env-item ${f === folder ? 'on' : ''}" title="${esc(f)}">${icon('folder', 14)}<span class="ellipsis grow">${esc(base(f))}</span>${f === folder ? icon('check', 14) : ''}</button>`);
      b.onclick = () => setFolder(f);
      list.appendChild(b);
    }
    const w = wtState();
    $('.env-wt', view).innerHTML = `<b>${esc(t(`code_wt_${w}_t`))}</b><span>${esc(t(`code_wt_${w}_d`))}</span>`;
    const issue = engineIssue();
    envRow.innerHTML = `<button class="env-btn" data-act="engine" title="${esc(issue)}">${icon(engine === 'claude-code' ? 'terminal' : 'key', 14)}<span>${esc(engine === 'claude-code' ? t('engine_cc') : t('engine_builtin'))}</span>${issue ? '<i class="warn-dot"></i>' : ''}${icon('chevDown', 11)}</button>
      <button class="env-btn" data-act="folder" title="${esc(folder)}">${icon('folder', 14)}<span class="ellipsis">${esc(folder ? base(folder) : t('folder_missing'))}</span>${icon('chevDown', 11)}</button>
      <button class="env-btn" data-act="wt" title="${esc(t('worktree_help'))}" aria-label="${esc(t(`code_wt_${w}_t`))}"><span class="chk ${w}" aria-hidden="true">${w === 'on' ? icon('check', 11) : w === 'auto' ? '–' : ''}</span><span>Worktree</span></button>`;
  };
  // the CLI's defaults (effort, ultracode) can differ per folder: the model button asks again
  const setFolder = (f) => { folder = f; saveLocal({ codeFolder: f }); drawEnv(); composer.refresh(); composer.focus(); };
  const setEngine = (e) => { engine = e; saveLocal({ codeEngine: e }); drawEnv(); composer.refresh(); };

  const folderMenu = (anchor, placement) => showMenu(anchor, [
    ...[...new Set([folder, ...recentFolders].filter(Boolean))].map((f) => ({ label: base(f), sub: f, checked: f === folder, onClick: () => setFolder(f) })),
    folder || recentFolders.length ? { sep: true } : null,
    { label: t('choose_folder'), icon: 'folder', onClick: async () => { const d = await call('pick_directory').catch(errorToast); if (d) setFolder(d); } },
    { label: t('code_type_path'), icon: 'edit', onClick: async () => { const v = await promptDialog(t('code_type_path'), folder, { placeholder: '/path/to/project' }); if (v && v.trim()) setFolder(v.trim()); } },
  ], { placement, width: 280 });
  const engineMenu = (anchor) => showMenu(anchor, [
    { label: t('engine_cc'), icon: 'terminal', sub: cc.found ? t('cc_found', { v: cc.version || 'claude' }) : t('cc_missing'), checked: engine === 'claude-code', onClick: () => setEngine('claude-code') },
    { label: t('engine_builtin'), icon: 'key', sub: store.settings.has_api_key ? t('engine_builtin_d') : t('no_key'), checked: engine === 'builtin', onClick: () => setEngine('builtin') },
    { sep: true },
    engine === 'claude-code'
      ? (cc.found
        ? { label: t('cc_login'), icon: 'user', onClick: () => call('claude_code_login').catch(errorToast) }
        : { label: t('cc_install'), icon: 'external', onClick: () => call('open_external', { url: 'https://docs.claude.com/en/docs/claude-code/setup' }).catch(errorToast) })
      : (store.settings.has_api_key ? null : { label: t('open_settings'), icon: 'settings', onClick: () => emit('open-settings', 'account') }),
  ], { placement: 'top-start', width: 300 });

  view.addEventListener('click', (e) => {
    const b = e.target.closest('[data-act]');
    if (!b || b.closest('.composer .cbox')) return;
    const act = b.dataset.act;
    if (act === 'routines') openRoutines();
    else if (act === 'import') emit('open-settings', 'import');
    else if (act === 'sessions') emit('navigate', { name: 'sessions' });
    else if (act === 'engine') engineMenu(b);
    else if (act === 'folder') folderMenu(b, b.closest('.env-row') ? 'top-start' : 'bottom-start');
    else if (act === 'wt') {
      const order = ['auto', 'on', 'off'];
      const inRow = !!b.closest('.env-row');
      saveLocal({ codeWorktree: order[(order.indexOf(wtState()) + 1) % 3] });
      drawEnv();
      // the row's button was drawn again: the keyboard stays on it
      if (inRow) $('[data-act=wt]', envRow).focus();
    }
  });

  const composer = createComposer({
    mode: 'code',
    placeholder: t('code_ask_ph'),
    draftKey: 'code-new',
    initial: { model: newChatModel('code'), effort: newChatEffort('code'), thinking: true, permission_mode: store.settings.code_permission_mode || 'default' },
    context: () => ({ cwd: folder, engine }),
    onSend: async (text, atts, st) => {
      if (!folder) { toast(t('folder_missing'), 'error'); return false; }
      const w = loadLocal().codeWorktree || 'auto';
      return startConversation({ text, atts, st, kind: 'code', cwd: folder, engine, worktree: w === 'on' ? true : w === 'off' ? false : null });
    },
  });
  composer.el.classList.add('dock');
  // after the "+" the box shows (CSS places it there), so Tab meets it before the row under the box
  $('[data-act=attach]', composer.el).after(envRow);
  $('.composer-slot', view).appendChild(composer.el);
  drawEnv();
  if (params.text) composer.setText(params.text);
  composer.focus();
}

/** All coding sessions: grouped by folder when there are many, with a filter and live status. */
export async function renderSessions(view) {
  const route = store.route;
  const sessions = await call('list_conversations', { kind: 'code' }).catch(() => []);
  if (store.route !== route || route.name !== 'sessions') return;
  view.innerHTML = `<div class="page"><div class="page-inner">
    <div class="page-head wrap"><h1>${t('sessions')}</h1><div class="grow"></div>
      <button class="btn sess-sec" data-act="routines" title="${esc(t('rt_title'))}" aria-label="${esc(t('rt_title'))}">${icon('clock', 14)}<span class="lbl"> ${esc(t('rt_title'))}</span></button>
      <button class="btn sess-sec" data-act="import" title="${esc(t('imp_cc_btn'))}" aria-label="${esc(t('imp_cc_btn'))}">${icon('download', 14)}<span class="lbl"> ${esc(t('imp_cc_btn'))}</span></button>
      <button class="btn primary" data-act="new">${icon('plus', 16)} ${t('new_session')}</button></div>
    <div class="sessions"></div></div></div>`;
  $('[data-act=import]', view).onclick = () => emit('open-settings', 'import');
  $('[data-act=routines]', view).onclick = () => openRoutines();
  $('[data-act=new]', view).onclick = () => emit('navigate', { name: 'code' });
  const list = $('.sessions', view);
  const statusOf = (id) => ([...store.permissions.values()].some((p) => p.conversation_id === id) ? 'waiting' : store.running.has(id) ? 'running' : '');
  if (!sessions.length) list.innerHTML = `<div class="muted small" style="padding:10px 2px">${t('no_sessions')}</div>`;
  // with many sessions: a filter, and sessions grouped by folder
  const filter = h(`<div class="search-box sess-filter ${sessions.length > 6 ? '' : 'hidden'}">${icon('search', 15)}<input class="input" placeholder="${esc(t('sess_filter'))}" aria-label="${esc(t('sess_filter'))}"></div>`);
  list.before(filter);
  const drawSessions = (q) => {
    list.innerHTML = '';
    const shown = sessions.filter((s) => hasWords(`${s.title || ''} ${s.cwd || ''}`, q));
    if (sessions.length && !shown.length) { list.innerHTML = `<div class="muted small" style="padding:10px 2px">${esc(t('no_results'))}</div>`; return; }
    const groups = new Map();
    for (const s of shown) {
      const k = s.cwd || '';
      if (!groups.has(k)) groups.set(k, []);
      groups.get(k).push(s);
    }
    const grouped = groups.size > 1 && shown.length > 6;
    for (const [dir, items] of groups) {
      if (grouped) list.appendChild(h(`<div class="sess-group" title="${esc(dir)}">${icon('folder', 13)} ${esc(dir.split(/[\\/]/).filter(Boolean).pop() || dir || '—')} <span class="muted">${items.length}</span></div>`));
      for (const s of items) {
        const status = statusOf(s.id);
        // the folder's name (the whole path in the tooltip): a long path no longer runs over the row
        const row = h(`<button class="list-item" data-id="${esc(s.id)}" title="${esc(s.cwd || '')}">${icon('terminal', 16)}<div class="grow" style="min-width:0"><div class="t ellipsis">${esc(s.title || t('untitled'))}</div><div class="d mono ellipsis">${grouped || !s.cwd ? '' : `${esc(s.cwd.split(/[\\/]/).filter(Boolean).pop() || s.cwd)} · `}${relTime(s.updated_at)}${s.engine === 'claude-code' ? ' · Claude Code' : ''}</div></div><span class="sess-status ${status}">${status ? esc(t('sess_' + status)) : ''}</span><span class="icon-btn sm more">${icon('more', 16)}</span></button>`);
        const menu = () => conversationMenu(row.querySelector('.more'), s, { onChange: () => renderSessions(view), onDelete: () => renderSessions(view) });
        row.addEventListener('click', (e) => {
          if (e.target.closest('.more')) { e.stopPropagation(); menu(); return; }
          emit('navigate', { name: 'chat', params: { id: s.id } });
        });
        // right-click, the Menu key or Shift+F10: the row's menu (its ⋯ is no Tab stop of its own)
        row.addEventListener('contextmenu', (e) => { e.preventDefault(); menu(); });
        list.appendChild(row);
      }
    }
  };
  if (sessions.length) drawSessions('');
  filter.querySelector('input').addEventListener('input', (e) => drawSessions(e.target.value));
  // keep the status chips current while this page is shown
  const refreshStatus = () => {
    if (!list.isConnected) return;
    list.querySelectorAll('.list-item[data-id]').forEach((r) => {
      const st = statusOf(r.dataset.id);
      const chip = r.querySelector('.sess-status');
      // written only when it changes: this runs every 1.5 s, and a write
      // (even of the same text) has the list styled and laid out again
      const cls = `sess-status ${st}`;
      const text = st ? t('sess_' + st) : '';
      if (chip.className !== cls) chip.className = cls;
      if (chip.textContent !== text) chip.textContent = text;
    });
  };
  const offs = [on('running-changed', refreshStatus)];
  // (nothing to do while the window is hidden: it's caught up within 1.5 s of coming back)
  const watch = setInterval(() => { if (!list.isConnected) { clearInterval(watch); offs.forEach((f) => f && f()); } else if (!document.hidden) refreshStatus(); }, 1500);
}

export async function renderChats(view) {
  view.innerHTML = `<div class="page"><div class="page-inner">
    <div class="page-head wrap"><h1>${t('all_chats')}</h1><div class="grow"></div><button class="btn" data-act="select">${t('select')}</button><button class="btn danger hidden" data-act="delsel">${icon('trash', 15)} ${t('delete_selected')}</button><button class="btn primary" data-act="new">${icon('plus', 16)} ${t('new_chat')}</button></div>
    <div class="search-box">${icon('search', 16)}<input class="input" placeholder="${t('search_chats')}" aria-label="${esc(t('search_chats'))}"></div>
    <div class="list"></div></div></div>`;
  $('[data-act=new]', view).onclick = () => emit('navigate', { name: 'new' });
  searchToggle(view);
  const list = $('.list', view);
  // Chat mode's history: code sessions live in Code mode
  const all = (await call('list_conversations', {}).catch(() => [])).filter((c) => c.kind !== 'code');
  let selecting = false;
  const selected = new Set();
  const delBtn = $('[data-act=delsel]', view);
  const updateDel = () => { delBtn.classList.toggle('hidden', !selecting || !selected.size); delBtn.lastChild.textContent = ` ${t('delete_selected')} (${selected.size})`; };
  $('[data-act=select]', view).onclick = (e) => {
    selecting = !selecting;
    selected.clear();
    e.currentTarget.textContent = selecting ? t('cancel') : t('select');
    list.classList.toggle('selecting', selecting);
    list.querySelectorAll('input[type=checkbox]').forEach((c) => { c.checked = false; c.classList.toggle('hidden', !selecting); });
    updateDel();
  };
  delBtn.onclick = async () => {
    if (!selected.size || !(await confirmDialog(t('confirm_delete_chat')))) return;
    for (const id of selected) {
      try { await call('delete_conversation', { id }); } catch (e) { errorToast(e); continue; }
      workspace.disposeConversation(id);
      store.drafts.delete(id);
    }
    emit('conversations-changed');
    renderChats(view);
  };
  const draw = async (q) => {
    let items = all;
    if (q.trim()) {
      const hits = await call('search', { query: q }).catch(() => []);
      const ids = hits.map((x) => x.conversation_id);
      const snippets = new Map(hits.map((x) => [x.conversation_id, x.snippet]));
      items = ids.map((id) => all.find((c) => c.id === id)).filter(Boolean).map((c) => ({ ...c, snippet: snippets.get(c.id) }));
    }
    list.innerHTML = items.length ? '' : `<div class="empty-state"><div class="big">${q ? t('no_results') : t('no_chats')}</div></div>`;
    for (const c of items) {
      const snip = c.snippet ? `<div class="d">${snippetHtml(c.snippet)}</div>` : '';
      const row = h(`<button class="list-item"><input type="checkbox" class="${selecting ? '' : 'hidden'}" ${selected.has(c.id) ? 'checked' : ''}><div class="grow"><div class="t ellipsis">${esc(convTitle(c))}</div>${snip}</div><span class="when">${relTime(c.updated_at)}</span><span class="icon-btn sm more">${icon('more', 16)}</span></button>`);
      row.addEventListener('click', (e) => {
        if (selecting) {
          e.preventDefault();
          const cb = row.querySelector('input');
          if (e.target !== cb) cb.checked = !cb.checked;
          if (cb.checked) selected.add(c.id); else selected.delete(c.id);
          updateDel();
          return;
        }
        if (e.target.closest('.more')) { e.stopPropagation(); conversationMenu(e.target.closest('.more'), c, { onChange: () => renderChats(view), onDelete: () => renderChats(view) }); return; }
        emit('navigate', { name: 'chat', params: { id: c.id } });
      });
      // right-click, the Menu key or Shift+F10: the row's menu
      row.addEventListener('contextmenu', (e) => { if (selecting) return; e.preventDefault(); conversationMenu(row.querySelector('.more'), c, { onChange: () => renderChats(view), onDelete: () => renderChats(view) }); });
      list.appendChild(row);
    }
  };
  let tm;
  $('.search-box input', view).addEventListener('input', (e) => { clearTimeout(tm); tm = setTimeout(() => draw(e.target.value), 160); });
  draw('');
  $('.search-box input', view).focus();
}
