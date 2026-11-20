// Left sidebar: navigation, starred and recent chats, account menu.
import { icon } from './icons.js';
import { t, LANGUAGES, currentLanguage } from './i18n.js';
import { call, isMac } from './api.js';
import { store, emit, on, displayName, saveLocal } from './store.js';
import { $, esc, h, showMenu, initials, errorToast } from './util.js';
import { conversationMenu } from './chat.js';

const RECENT_LIMIT = 30;

export function mountSidebar() {
  const el = $('#sidebar');
  const mod = isMac ? '⌘' : 'Ctrl+';
  el.innerHTML = `
    <div class="sidebar-top">
      <button class="icon-btn" data-act="toggle" title="${t('collapse')}" aria-label="${t('collapse')}">${icon('sidebar', 18)}</button>
      <span class="brand">ClaudeRust</span>
    </div>
    <nav class="nav">
      <button class="nav-item new-chat" data-nav="new"><span class="plus-circle">${icon('plus', 15)}</span><span class="label">${t('new_chat')}</span><span class="kbd">${mod}⇧O</span></button>
      <button class="nav-item" data-nav="chats">${icon('chat', 18)}<span class="label">${t('chats')}</span></button>
      <button class="nav-item" data-nav="projects">${icon('folder', 18)}<span class="label">${t('projects')}</span></button>
      <button class="nav-item" data-nav="artifacts">${icon('layers', 18)}<span class="label">${t('artifacts')}</span></button>
      <button class="nav-item" data-nav="code">${icon('code', 18)}<span class="label">${t('code')}</span></button>
    </nav>
    <div class="sidebar-scroll"></div>
    <div class="sidebar-bottom"><button class="user-card"></button></div>`;
  el.classList.toggle('collapsed', store.sidebarCollapsed);
  el.addEventListener('click', (e) => {
    const n = e.target.closest('[data-nav]');
    if (n) { emit('navigate', { name: n.dataset.nav }); return; }
    if (e.target.closest('[data-act=toggle]')) toggleSidebar();
  });
  $('.user-card', el).addEventListener('click', (e) => accountMenu(e.currentTarget));
  on('conversations-changed', refreshSidebar);
  on('running-changed', renderLists);
  on('route-changed', highlight);
  on('settings-changed', renderUserCard);
  on('collapse-sidebar', () => { if (!store.sidebarCollapsed) toggleSidebar(); });
  renderUserCard();
  refreshSidebar();
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
    { label: t('help'), icon: 'help', onClick: () => call('open_external', { url: 'https://docs.anthropic.com' }).catch(errorToast) },
    { sep: true },
    { label: t('remove_key'), icon: 'logout', onClick: async () => {
      try { await call('set_api_key', { key: null }); emit('reload-settings'); } catch (e) { errorToast(e); }
    } },
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
  const a = h(`<button class="chat-link" data-id="${esc(c.id)}">
    ${c.kind === 'code' ? `<span class="code-badge">${icon('terminal', 14)}</span>` : ''}
    <span class="title">${esc(c.title || t('new_chat'))}</span>
    ${running || waiting ? '<span class="dot"></span>' : ''}
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
  const convs = store.conversations || [];
  const starred = convs.filter((c) => c.starred);
  if (starred.length) {
    box.appendChild(h(`<div class="section-title">${t('starred')}</div>`));
    starred.forEach((c) => box.appendChild(chatLink(c)));
  }
  const recents = convs.filter((c) => !c.starred).slice(0, RECENT_LIMIT);
  if (recents.length) {
    box.appendChild(h(`<div class="section-title">${t('recents')}</div>`));
    recents.forEach((c) => box.appendChild(chatLink(c)));
    if (convs.length - starred.length > RECENT_LIMIT) {
      const all = h(`<button class="view-all">${t('view_all')} ${icon('chevRight', 12)}</button>`);
      all.onclick = () => emit('navigate', { name: 'chats' });
      box.appendChild(all);
    }
  }
  highlight();
}

function highlight() {
  const r = store.route;
  document.querySelectorAll('#sidebar [data-nav]').forEach((b) => {
    const n = b.dataset.nav;
    b.classList.toggle('active', n !== 'new' && (r.name === n || (n === 'projects' && r.name === 'project')));
  });
  document.querySelectorAll('#sidebar .chat-link').forEach((b) => b.classList.toggle('active', r.name === 'chat' && r.params && r.params.id === b.dataset.id));
  void currentLanguage;
}
