// Code page: start coding sessions in a local folder and list past ones.
import * as workspace from './workspace.js';
import { icon } from './icons.js';
import { t } from './i18n.js';
import { call } from './api.js';
import { store, emit, saveLocal, loadLocal } from './store.js';
import { $, esc, h, relTime, errorToast, toast, confirmDialog } from './util.js';
import { createComposer } from './composer.js';
import { startConversation } from './home.js';
import { conversationMenu } from './chat.js';

export async function renderCode(view) {
  let folder = loadLocal().codeFolder || '';
  const sessions = await call('list_conversations', { kind: 'code' }).catch(() => []);
  const recentFolders = [...new Set(sessions.map((s) => s.cwd).filter(Boolean))].slice(0, 6);
  if (!folder && recentFolders.length) folder = recentFolders[0];

  view.innerHTML = `<div class="page"><div class="page-inner">
    <div class="page-head"><h1>${t('code_title')}</h1></div>
    <p class="muted" style="margin:-10px 0 22px">${t('code_desc')}</p>
    <div class="code-hero">
      <div class="engine-pick"></div>
      <div class="folder-pick"></div>
      <div class="composer-slot"></div>
    </div>
    <div class="section-title sessions-head" style="padding-left:2px"><span>${t('sessions')}</span><button class="btn sm ghost" data-act="import">${icon('download', 14)} ${t('imp_cc_btn')}</button></div>
    <div class="sessions"></div></div></div>`;
  $('[data-act=import]', view).onclick = () => emit('open-settings', 'import');

  const pick = $('.folder-pick', view);
  const drawFolder = () => {
    pick.innerHTML = '';
    const chip = h(`<span class="folder-chip ${folder ? '' : 'empty'}" title="${esc(folder)}" role="button" tabindex="0">${icon('folder', 14)} <span class="ellipsis">${esc(folder || t('folder_missing'))}</span></span>`);
    // Click the chip to type or paste a path directly.
    const editPath = () => {
      const input = h(`<input class="input mono" style="max-width:520px;height:32px" placeholder="/path/to/project">`);
      input.value = folder;
      chip.replaceWith(input);
      input.focus();
      input.select();
      const commit = () => {
        const v = input.value.trim();
        if (v) { folder = v; saveLocal({ codeFolder: v }); }
        drawFolder();
        composer.focus();
      };
      input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); commit(); } if (e.key === 'Escape') drawFolder(); });
      input.addEventListener('blur', commit);
    };
    chip.addEventListener('click', editPath);
    chip.addEventListener('keydown', (e) => { if (e.key === 'Enter') editPath(); });
    const b = h(`<button class="btn sm">${t('choose_folder')}</button>`);
    b.onclick = async () => {
      const d = await call('pick_directory').catch(errorToast);
      if (d) { folder = d; saveLocal({ codeFolder: d }); drawFolder(); composer.focus(); }
    };
    pick.append(chip, b);
    for (const f of recentFolders.filter((x) => x !== folder).slice(0, 4)) {
      const r = h(`<button class="chip" title="${esc(f)}">${icon('history', 13)} ${esc(f.split(/[\\/]/).filter(Boolean).pop() || f)}</button>`);
      r.onclick = () => { folder = f; saveLocal({ codeFolder: f }); drawFolder(); };
      pick.appendChild(r);
    }
  };
  // Engine: the official Claude Code CLI (signs in with your Claude account) or the built-in agent (API key).
  const cc = await call('claude_code_info', {}).catch(() => ({ found: false }));
  const defEngine = store.settings.code_engine === 'builtin' ? 'builtin'
    : store.settings.code_engine === 'claude-code' ? 'claude-code' : (cc.found ? 'claude-code' : 'builtin');
  let engine = loadLocal().codeEngine || defEngine;
  const ep = $('.engine-pick', view);
  const drawEngine = () => {
    ep.innerHTML = `<div class="segmented"><button data-e="claude-code" class="${engine === 'claude-code' ? 'on' : ''}">${icon('terminal', 13)} ${t('engine_cc')}</button><button data-e="builtin" class="${engine === 'builtin' ? 'on' : ''}">${icon('key', 13)} ${t('engine_builtin')}</button></div><span class="engine-status small"></span>`;
    ep.querySelectorAll('[data-e]').forEach((b) => b.onclick = () => { engine = b.dataset.e; saveLocal({ codeEngine: engine }); drawEngine(); });
    const stEl = $('.engine-status', ep);
    if (engine === 'claude-code') {
      if (cc.found) {
        stEl.innerHTML = `<span class="muted">${esc(t('cc_found', { v: cc.version || 'claude' }))}</span> <button class="btn sm ghost" data-a="login">${t('cc_login')}</button>`;
        stEl.querySelector('[data-a=login]').onclick = () => call('claude_code_login').catch(errorToast);
      } else {
        stEl.innerHTML = `<span style="color:var(--danger)">${esc(t('cc_missing'))}</span> <button class="btn sm" data-a="install">${t('cc_install')} ${icon('external', 12)}</button>`;
        stEl.querySelector('[data-a=install]').onclick = () => call('open_external', { url: 'https://docs.claude.com/en/docs/claude-code/setup' }).catch(errorToast);
      }
    } else if (!store.settings.has_api_key) {
      stEl.innerHTML = `<span style="color:var(--danger)">${esc(t('no_key'))}</span> <button class="btn sm ghost" data-a="key">${t('open_settings')}</button>`;
      stEl.querySelector('[data-a=key]').onclick = () => emit('open-settings', 'account');
    } else {
      stEl.innerHTML = `<span class="muted">${esc(t('engine_builtin_d'))}</span>`;
    }
  };
  drawEngine();
  const composer = createComposer({
    mode: 'code',
    placeholder: t('code_placeholder'),
    draftKey: 'code-new',
    initial: { model: store.settings.code_model || store.settings.default_model, thinking: true, permission_mode: store.settings.code_permission_mode || 'default' },
    context: () => ({ cwd: folder, engine }),
    onSend: async (text, atts, st) => {
      if (!folder) { toast(t('folder_missing'), 'error'); return false; }
      return startConversation({ text, atts, st, kind: 'code', cwd: folder, engine });
    },
  });
  drawFolder();
  $('.composer-slot', view).appendChild(composer.el);

  const list = $('.sessions', view);
  if (!sessions.length) list.innerHTML = `<div class="muted small" style="padding:10px 2px">${t('no_sessions')}</div>`;
  for (const s of sessions) {
    const row = h(`<button class="list-item">${icon('terminal', 16)}<div class="grow"><div class="t">${esc(s.title || t('untitled'))}</div><div class="d mono">${esc(s.cwd || '')} · ${relTime(s.updated_at)}${s.engine === 'claude-code' ? ' · Claude Code' : ''}</div></div><span class="icon-btn sm more">${icon('more', 16)}</span></button>`);
    row.addEventListener('click', (e) => {
      if (e.target.closest('.more')) { e.stopPropagation(); conversationMenu(e.target.closest('.more'), s, { onChange: () => renderCode(view), onDelete: () => renderCode(view) }); return; }
      emit('navigate', { name: 'chat', params: { id: s.id } });
    });
    list.appendChild(row);
  }
  composer.focus();
}

export async function renderChats(view) {
  view.innerHTML = `<div class="page"><div class="page-inner">
    <div class="page-head"><h1>${t('all_chats')}</h1><div class="grow"></div><button class="btn" data-act="select">${t('select')}</button><button class="btn danger hidden" data-act="delsel">${icon('trash', 15)} ${t('delete_selected')}</button><button class="btn primary" data-act="new">${icon('plus', 16)} ${t('new_chat')}</button></div>
    <div class="search-box">${icon('search', 16)}<input class="input" placeholder="${t('search_chats')}"></div>
    <div class="list"></div></div></div>`;
  $('[data-act=new]', view).onclick = () => emit('navigate', { name: 'new' });
  const list = $('.list', view);
  const all = await call('list_conversations', {}).catch(() => []);
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
    for (const id of selected) { await call('delete_conversation', { id }).catch(errorToast); workspace.disposeConversation(id); }
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
      const snip = c.snippet ? `<div class="d">${esc(c.snippet).replace(/\u0001/g, '<mark>').replace(/\u0002/g, '</mark>')}</div>` : `<div class="d">${t('last_message')} ${relTime(c.updated_at)}</div>`;
      const row = h(`<button class="list-item"><input type="checkbox" class="${selecting ? '' : 'hidden'}" ${selected.has(c.id) ? 'checked' : ''}>${c.kind === 'code' ? icon('terminal', 16) : icon('chat', 16)}<div class="grow"><div class="t">${esc(c.title || t('untitled'))}</div>${snip}</div><span class="icon-btn sm more">${icon('more', 16)}</span></button>`);
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
      list.appendChild(row);
    }
  };
  let tm;
  $('.search-box input', view).addEventListener('input', (e) => { clearTimeout(tm); tm = setTimeout(() => draw(e.target.value), 160); });
  draw('');
  $('.search-box input', view).focus();
}
