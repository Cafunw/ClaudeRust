// Left sidebar: Chat / Code modes, navigation, starred and recent
// conversations of the mode, account menu.
import { icon } from './icons.js';
import { t, LANGUAGES, currentLanguage } from './i18n.js';
import { call, isMac } from './api.js';
import { store, emit, on, displayName, saveLocal } from './store.js';
import { $, esc, h, showMenu, initials, errorToast } from './util.js';
import { conversationMenu } from './chat.js';

const RECENT_LIMIT = 30;

// Chat and Code are separate modes, as in Claude Desktop: each has its own
// navigation and its own list of conversations.
const CHAT_ROUTES = new Set(['new', 'chats', 'projects', 'project', 'artifacts']);

function savedMode() {
  try { return JSON.parse(localStorage.getItem('clauderust') || '{}').appMode === 'code' ? 'code' : 'chat'; } catch { return 'chat'; }
}

/** The mode a route belongs to (null when it can't be told yet). */
function modeForRoute(r) {
  if (!r) return null;
  if (r.name === 'code') return 'code';
  if (CHAT_ROUTES.has(r.name)) return 'chat';
  if (r.name === 'chat') {
    const c = (store.conversations || []).find((x) => x.id === (r.params && r.params.id));
    return c ? (c.kind === 'code' ? 'code' : 'chat') : null;
  }
  return null;
}

/** Switch the sidebar to `mode` (and remember it). */
export function setMode(mode) {
  if (mode !== 'chat' && mode !== 'code') return;
  if (store.appMode === mode) return;
  store.appMode = mode;
  saveLocal({ appMode: mode });
  renderNav();
  renderLists();
}

/** Go to a mode: back to where you were in it, or its start page. */
function enterMode(mode) {
  const last = (JSON.parse(localStorage.getItem('clauderust') || '{}').lastByMode || {})[mode];
  setMode(mode);
  if (last && last.name === 'chat' && (store.conversations || []).some((c) => c.id === last.id)) emit('navigate', { name: 'chat', params: { id: last.id } });
  else if (last && last.name && last.name !== 'chat') emit('navigate', { name: last.name });
  else emit('navigate', { name: mode === 'code' ? 'code' : 'new' });
}

function rememberRoute() {
  const r = store.route;
  const mode = modeForRoute(r);
  if (!mode) return;
  setMode(mode);
  try {
    const all = JSON.parse(localStorage.getItem('clauderust') || '{}');
    const lastByMode = { ...(all.lastByMode || {}) };
    lastByMode[mode] = r.name === 'chat' ? { name: 'chat', id: r.params && r.params.id } : (r.name === 'project' ? { name: 'projects' } : { name: r.name });
    saveLocal({ lastByMode });
  } catch { /* storage unavailable */ }
}

export function mountSidebar() {
  const el = $('#sidebar');
  store.appMode = store.appMode || savedMode();
  el.innerHTML = `
    <div class="sidebar-top">
      <button class="icon-btn" data-act="toggle" title="${t('collapse')}" aria-label="${t('collapse')}">${icon('sidebar', 18)}</button>
      <span class="brand">ClaudeRust</span>
    </div>
    <div class="mode-switch" role="tablist">
      <button role="tab" data-mode="chat" title="${t('mode_chat')}">${icon('chat', 15)}<span class="label">${t('mode_chat')}</span></button>
      <button role="tab" data-mode="code" title="${t('mode_code')}">${icon('code', 15)}<span class="label">${t('mode_code')}</span></button>
    </div>
    <nav class="nav"></nav>
    <div class="sidebar-scroll"></div>
    <div class="sidebar-bottom"><button class="user-card"></button></div>`;
  el.classList.toggle('collapsed', store.sidebarCollapsed);
  el.addEventListener('click', (e) => {
    const m = e.target.closest('[data-mode]');
    if (m) { if (m.dataset.mode !== store.appMode) enterMode(m.dataset.mode); return; }
    const n = e.target.closest('[data-nav]');
    if (n) {
      if (n.dataset.nav === 'routines') { import('./routines.js').then((r) => r.openRoutines()); return; }
      emit('navigate', { name: n.dataset.nav });
      return;
    }
    if (e.target.closest('[data-act=toggle]')) toggleSidebar();
  });
  $('.user-card', el).addEventListener('click', (e) => accountMenu(e.currentTarget));
  on('conversations-changed', refreshSidebar);
  on('running-changed', renderLists);
  on('route-changed', () => { rememberRoute(); highlight(); });
  // a conversation's kind is only known once it's loaded
  on('conversation-opened', ({ kind }) => { setMode(kind === 'code' ? 'code' : 'chat'); rememberRoute(); highlight(); });
  on('settings-changed', renderUserCard);
  on('collapse-sidebar', () => { if (!store.sidebarCollapsed) toggleSidebar(); });
  renderNav();
  renderUserCard();
  refreshSidebar();
}

function renderNav() {
  const nav = $('#sidebar .nav');
  if (!nav) return;
  const mod = isMac ? '⌘' : 'Ctrl+';
  document.querySelectorAll('#sidebar [data-mode]').forEach((b) => b.classList.toggle('on', b.dataset.mode === store.appMode));
  nav.innerHTML = store.appMode === 'code'
    ? `<button class="nav-item new-chat" data-nav="code"><span class="plus-circle">${icon('plus', 15)}</span><span class="label">${t('new_session')}</span><span class="kbd">${mod}⇧O</span></button>
       <button class="nav-item" data-nav="routines">${icon('clock', 18)}<span class="label">${t('rt_title')}</span></button>`
    : `<button class="nav-item new-chat" data-nav="new"><span class="plus-circle">${icon('plus', 15)}</span><span class="label">${t('new_chat')}</span><span class="kbd">${mod}⇧O</span></button>
       <button class="nav-item" data-nav="chats">${icon('chat', 18)}<span class="label">${t('chats')}</span></button>
       <button class="nav-item" data-nav="projects">${icon('folder', 18)}<span class="label">${t('projects')}</span></button>
       <button class="nav-item" data-nav="artifacts">${icon('layers', 18)}<span class="label">${t('artifacts')}</span></button>`;
  highlight();
}

export function toggleSidebar() {
  store.sidebarCollapsed = !store.sidebarCollapsed;
  $('#sidebar').classList.toggle('collapsed', store.sidebarCollapsed);
  saveLocal({ sidebarCollapsed: store.sidebarCollapsed });
}

function renderUserCard() {
  const card = $('#sidebar .user-card');
  if (!card) return;
  const name = displayName() || 'You';
  card.innerHTML = `<span class="avatar">${esc(initials(name))}</span><span class="meta"><div class="name ellipsis">${esc(store.settings.full_name || name)}</div><div class="plan">${store.settings.has_api_key ? t('api_plan') : store.settings.code_engine === 'claude-code' ? 'Claude Code' : t('no_key')}</div></span><span class="chev muted">${icon('chevUp', 14)}</span>`;
}

function accountMenu(anchor) {
  showMenu(anchor, [
    { header: store.settings.full_name || displayName() || 'ClaudeRust' },
    { label: t('settings'), icon: 'settings', right: isMac ? '⌘,' : 'Ctrl+,', onClick: () => emit('open-settings', 'general') },
    { label: t('language'), icon: 'lang', keepOpen: true, onClick: () => showMenu(anchor, LANGUAGES.map((l) => ({
      label: l.name, checked: (store.settings.language || 'auto') === l.id,
      onClick: () => emit('save-settings', { language: l.id }),
    })), { placement: 'top-start', width: 220 }) },
    { label: t('imp_menu'), icon: 'download', onClick: () => emit('open-settings', 'import') },
    { label: t('help'), icon: 'help', onClick: () => call('open_external', { url: 'https://docs.anthropic.com' }).catch(errorToast) },
    ...(store.settings.has_api_key ? [
      { sep: true },
      { label: t('remove_key'), icon: 'logout', onClick: async () => {
        try { await call('set_api_key', { key: null }); emit('reload-settings'); } catch (e) { errorToast(e); }
      } },
    ] : []),
  ], { placement: 'top-start', width: 260 });
}

export async function refreshSidebar() {
  try {
    store.conversations = await call('list_conversations', {});
  } catch (e) { console.error(e); }
  renderLists();
}

function chatLink(c) {
  const running = store.running.has(c.id) || c.running && store.running.has(c.id);
  const waiting = [...store.permissions.values()].some((p) => p.conversation_id === c.id);
  const folder = c.kind === 'code' && c.cwd ? c.cwd.split(/[\\/]/).filter(Boolean).pop() || c.cwd : '';
  const a = h(`<button class="chat-link ${folder ? 'two' : ''}" data-id="${esc(c.id)}" ${folder ? `title="${esc(c.cwd)}"` : ''}>
    <span class="txt"><span class="title">${esc(c.title || (c.kind === 'code' ? t('untitled') : t('new_chat')))}</span>${folder ? `<span class="sub">${esc(folder)}</span>` : ''}</span>
    ${waiting ? `<span class="dot wait" title="${esc(t('sess_waiting'))}"></span>` : running ? `<span class="dot" title="${esc(t('sess_running'))}"></span>` : ''}
    <span class="icon-btn sm more" role="button" aria-label="more">${icon('more', 16)}</span></button>`);
  a.addEventListener('click', (e) => {
    if (e.target.closest('.more')) {
      e.stopPropagation();
      conversationMenu(e.target.closest('.more'), c, { onChange: () => refreshSidebar() });
      return;
    }
    emit('navigate', { name: 'chat', params: { id: c.id } });
  });
  return a;
}

function renderLists() {
  const box = $('#sidebar .sidebar-scroll');
  if (!box) return;
  box.innerHTML = '';
  const code = store.appMode === 'code';
  // only this mode's conversations
  const convs = (store.conversations || []).filter((c) => (c.kind === 'code') === code);
  const starred = convs.filter((c) => c.starred);
  if (starred.length) {
    box.appendChild(h(`<div class="section-title">${t('starred')}</div>`));
    starred.forEach((c) => box.appendChild(chatLink(c)));
  }
  const recents = convs.filter((c) => !c.starred).slice(0, RECENT_LIMIT);
  if (recents.length) {
    box.appendChild(h(`<div class="section-title">${code ? t('sessions') : t('recents')}</div>`));
    recents.forEach((c) => box.appendChild(chatLink(c)));
    if (convs.length - starred.length > RECENT_LIMIT) {
      const all = h(`<button class="view-all">${t('view_all')} ${icon('chevRight', 12)}</button>`);
      all.onclick = () => emit('navigate', { name: code ? 'code' : 'chats' });
      box.appendChild(all);
    }
  } else if (!starred.length) {
    box.appendChild(h(`<div class="sidebar-empty muted small">${esc(code ? t('no_sessions') : t('no_chats'))}</div>`));
  }
  highlight();
}

function highlight() {
  const r = store.route;
  document.querySelectorAll('#sidebar [data-nav]').forEach((b) => {
    const n = b.dataset.nav;
    b.classList.toggle('active', n !== 'new' && n !== 'code' && (r.name === n || (n === 'projects' && r.name === 'project')));
  });
  document.querySelectorAll('#sidebar .chat-link').forEach((b) => b.classList.toggle('active', r.name === 'chat' && r.params && r.params.id === b.dataset.id));
  void currentLanguage;
}
