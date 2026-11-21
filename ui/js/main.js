// App bootstrap, routing, global events and shortcuts.
import { t, setLanguage, currentLanguage } from './i18n.js';
import { call, on as onBackend, isMac } from './api.js';
import { onTurn } from './turnstream.js';
import { store, on, emit, loadLocal, saveLocal } from './store.js';
import { $, closeMenu, errorToast, toast } from './util.js';
import { mountSidebar, refreshSidebarSoon, toggleSidebar } from './sidebar.js';
import { renderNewChat } from './home.js';
import * as chat from './chat.js';
import { setToolProgress } from './parts.js';
import * as workspace from './workspace.js';
import * as keymap from './keymap.js';
import './commands_builtin.js';
import './rewind.js';
import './memory_ui.js';
import './context_ui.js';
import './hooks_ui.js';
import './commands_agent.js';
import './routines.js';
import './watch_ui.js';
import './usage_ui.js';
import './shortcuts.js';
import './findbar.js';
import './updates.js';
import './quote.js';
import './todos_ui.js';
import './panes/changes.js';
import './panes/worktree_actions.js';
import './panes/review.js';
import './panes/commit.js';
import './panes/plan.js';
import './panes/terminal.js';
import './panes/tasks.js';
import './panes/preview.js';
import './panes/map.js';
import './panes/history.js';
import './panes/files.js';
import * as artifacts from './artifacts.js';
import { renderProjects, renderProject, refreshProjects } from './projects.js';
import { renderCode, renderChats, renderSessions } from './code.js';
import { openSettings } from './settings.js';
import { openSearch } from './search.js';
import { renderOnboarding } from './onboarding.js';
import { linkTarget } from './deeplink.js';

const mq = window.matchMedia('(prefers-color-scheme: dark)');

function applyAppearance() {
  const s = store.settings;
  const theme = s.theme === 'light' || s.theme === 'dark' ? s.theme : (mq.matches ? 'dark' : 'light');
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.font = s.chat_font || 'default';
  document.documentElement.dataset.width = s.chat_width || 'normal';
}
mq.addEventListener('change', applyAppearance);

function layout() {
  document.body.innerHTML = `<div class="app" id="app">
    <aside class="sidebar" id="sidebar"></aside>
    <div class="sidebar-scrim" id="sidebar-scrim"></div>
    <main class="main"><div class="view" id="view"></div></main>
    <aside class="workspace-panel hidden" id="workspace-panel"></aside>
    <aside class="artifact-panel hidden" id="artifact-panel"></aside>
  </div>`;
}

let navToken = 0;
async function navigate({ name, params = {} }) {
  closeMenu();
  const token = ++navToken;
  const prev = store.route;
  const sameConv = prev.name === 'chat' && name === 'chat' && prev.params.id === params.id;
  if (!sameConv) {
    chat.closeCurrent();
    artifacts.close();
    if (name !== 'chat') workspace.setConversation(null);
  }
  store.route = { name, params };
  saveLocal({ lastRoute: name === 'chat' || name === 'new' || name === 'code' ? { name, params: { id: params.id } } : null });
  emit('route-changed');
  const view = $('#view');
  try {
    switch (name) {
      case 'chat':
        if (!sameConv || params.reload !== false) await chat.openConversation(params.id);
        if (token !== navToken) return;
        if (params.artifact) {
          const c = chat.current();
          if (c) artifacts.openLatestByIdentifier(c.id, c.artifacts, params.artifact);
        }
        break;
      case 'chats': await renderChats(view); break;
      case 'projects': await renderProjects(view); break;
      case 'project': await renderProject(view, params.id); break;
      case 'artifacts': await artifacts.renderGallery(view); break;
      case 'code': await renderCode(view, params); break;
      case 'sessions': await renderSessions(view); break;
      default: renderNewChat(view, params); store.route = { name: 'new', params };
    }
  } catch (e) {
    errorToast(e);
    if (name !== 'new' && token === navToken) navigate({ name: 'new' });
  }
}

async function saveSettings(patch) {
  try {
    const prevLang = store.settings.language;
    store.settings = await call('save_settings', { patch });
    applyAppearance();
    if ('language' in patch && patch.language !== prevLang) {
      setLanguage(store.settings.language);
      rerenderShell();
    }
    emit('settings-changed');
  } catch (e) { errorToast(e); }
}

function rerenderShell() {
  mountSidebar();
  navigate(store.route.name === 'chat' ? store.route : { name: store.route.name, params: store.route.params });
}

async function reloadSettings() {
  const boot = await call('bootstrap');
  store.settings = boot.settings;
  emit('settings-changed');
  // same rule as at start: a Claude Code user may have no API key at all
  if (!store.settings.has_api_key && !store.settings.onboarded) startOnboarding();
  else refreshModels();
}

let oaiLoaded = false;
async function refreshModels() {
  // models of the OpenAI-compatible endpoint, once per start
  if (!oaiLoaded && store.settings.oai_base_url) { oaiLoaded = true; await call('oai_refresh').catch(() => {}); }
  try { store.models = await call('get_models', { refresh: true }); emit('models-changed'); } catch { /* offline */ }
}

async function refreshMcp() {
  try { const st = await call('mcp_status'); store.mcpServers = st.servers; } catch { store.mcpServers = []; }
}

function startOnboarding() {
  linksReady = false;
  $('#sidebar').classList.add('hidden');
  renderOnboarding($('#view'), (target) => {
    $('#sidebar').classList.remove('hidden');
    call('bootstrap').then((b) => {
      store.settings = b.settings;
      store.models = b.models;
      mountSidebar();
      refreshModels();
      navigate({ name: target === 'code' ? 'code' : 'new' });
      linksReady = true;
      takeDeepLink();
    });
  });
}

// a clauderust:// link waits in the backend until onboarding is over
let linksReady = false;
async function takeDeepLink() {
  if (!linksReady) return;
  const to = linkTarget(await call('deep_link_take').catch(() => null));
  if (to && to.settings) openSettings('general');
  else if (to) navigate(to);
}

/** A field that takes typed text: its Esc and its right-click menu are its own. */
const isTextField = (el) => !!el && (el.isContentEditable || el.tagName === 'TEXTAREA' || (el.tagName === 'INPUT' && el.type !== 'checkbox'));

function bindShortcuts() {
  // WebView2's own page menu (Back, Refresh, Save as, Print): text fields and selected text keep theirs (copy, paste)
  document.addEventListener('contextmenu', (e) => {
    if (!isTextField(e.target) && window.getSelection().isCollapsed) e.preventDefault();
  });
  window.addEventListener('keydown', (e) => {
    const mod = isMac ? e.metaKey : e.ctrlKey;
    const k = e.key.toLowerCase();
    // WebView2's browser keys: a reload loses queued messages and the open terminals, print prints the app
    if (e.key === 'F5' || (mod && (k === 'r' || k === 'p'))) e.preventDefault();
    // in Code mode the same keys start a new session
    if (keymap.matches(e, 'new_chat')) { e.preventDefault(); navigate({ name: store.appMode === 'code' ? 'code' : 'new' }); }
    else if (keymap.matches(e, 'palette')) { e.preventDefault(); openSearch(); }
    else if (keymap.matches(e, 'sidebar')) { e.preventDefault(); toggleSidebar(); }
    else if (keymap.matches(e, 'settings')) { e.preventDefault(); openSettings('general'); }
    else if (mod && (k === '=' || k === '+')) { e.preventDefault(); setZoom((store.settings.zoom || 1) + 0.1); }
    else if (mod && k === '-') { e.preventDefault(); setZoom((store.settings.zoom || 1) - 0.1); }
    else if (mod && k === '0') { e.preventDefault(); setZoom(1); }
    // Esc already taken (the composer's own stop, the quote bar, a pane's field) or typed into a field
    // or a side panel leaves the turn alone
    else if (e.key === 'Escape' && !e.defaultPrevented && !isTextField(e.target) && (e.target === document.body || e.target.closest('#view'))
      && !document.querySelector('.modal-scrim, .popover')) {
      const c = chat.current();
      // the composer's own stop: queued messages come back instead of being sent
      if (c && c.running && !c.composer.waiting()) { e.preventDefault(); chat.stopTurn(c.id); }
    }
  });
}

async function setZoom(z) {
  z = Math.round(Math.max(0.5, Math.min(2.5, z)) * 100) / 100;
  try { await call('set_zoom', { zoom: z }); store.settings.zoom = z; } catch (e) { errorToast(e); }
}

function bindBackend() {
  onTurn((p) => chat.onTurnEvent(p));
  onBackend('generation-state', (p) => { chat.onStateEvent(p); refreshSidebarSoon(); });
  onBackend('conversation-updated', (p) => {
    refreshSidebarSoon();
    const c = chat.current();
    if (c && c.id === p.conversation_id) {
      // the record only (title, options), not the rendered conversation
      call('conversation_meta', { id: c.id }).then((conv) => {
        const cc = chat.current();
        if (!cc || cc.id !== c.id) return;
        cc.conv = conv;
        document.title = conv.title || 'ClaudeRust';
        const tt = document.querySelector('.topbar .title-btn .t');
        if (tt) tt.textContent = conv.title || t('new_chat');
        if (cc.composer && conv.options && conv.options.permission_mode) cc.composer.setState({ permission_mode: conv.options.permission_mode });
      }).catch(() => {});
    }
  });
  onBackend('tool-progress', (p) => setToolProgress(p.tool_use_id, p.text));
  onBackend('permission-request', (p) => chat.onPermissionRequest(p));
  onBackend('permission-resolved', (p) => chat.onPermissionResolved(p));
  onBackend('mcp-changed', () => refreshMcp());
  onBackend('menu-new-chat', () => navigate({ name: 'new' }));
  onBackend('deep-link', takeDeepLink);
  // things the backend finished in the background (e.g. where a deleted
  // session's work was kept)
  onBackend('notice', (n) => toast(n.message, n.error ? 'error' : '', { duration: 12000 }));
  // the backend changed a setting itself (e.g. "always allow" on a prompt)
  onBackend('settings-updated', async () => {
    try { store.settings = await call('get_settings'); emit('settings-changed'); } catch { /* keep the current copy */ }
  });
  onBackend('quick-entry', async ({ text }) => {
    await navigate({ name: 'new', params: { text } });
    if (store.activeComposer && text.trim()) store.activeComposer.send();
  });
  // native file drag & drop
  onBackend('tauri://drag-enter', () => store.activeComposer && !document.querySelector('.modal-scrim') && store.activeComposer.setDrag(true));
  onBackend('tauri://drag-leave', () => store.activeComposer && store.activeComposer.setDrag(false));
  onBackend('tauri://drag-drop', (p) => {
    const c = store.activeComposer;
    if (!c) return;
    c.setDrag(false);
    if (document.querySelector('.modal-scrim')) return; // e.g. Settings → Import handles it
    if (p && p.paths && p.paths.length) c.addPaths(p.paths);
  });
}

async function init() {
  let boot;
  try {
    boot = await call('bootstrap');
  } catch (e) {
    document.body.textContent = 'Failed to start: ' + e.message;
    return;
  }
  store.settings = boot.settings;
  store.models = boot.models;
  if (store.settings.oai_base_url) refreshModels();
  store.platform = boot.platform;
  store.version = boot.version;
  store.render = boot.render || {};
  store.dataDir = boot.data_dir;
  store.mcpConfigPath = boot.mcp_config_path;
  const style = document.createElement('style');
  style.id = 'hl-css';
  style.textContent = boot.highlight_css;
  document.head.appendChild(style);
  setLanguage(store.settings.language);
  applyAppearance();
  const local = loadLocal();
  store.sidebarCollapsed = !!local.sidebarCollapsed || window.innerWidth < 760;
  layout();
  bindShortcuts();
  bindBackend();

  on('navigate', navigate);
  on('open-settings', (tab) => openSettings(tab));
  on('refresh-models', () => refreshModels());
  on('save-settings', saveSettings);
  on('reload-settings', reloadSettings);
  on('set-zoom', setZoom);
  on('models-refresh', refreshModels);

  if (!store.settings.has_api_key && !store.settings.onboarded) {
    startOnboarding();
    return;
  }
  mountSidebar();
  refreshProjects();
  refreshMcp();
  refreshModels();
  const last = local.lastRoute;
  if (last && last.name === 'chat' && last.params && last.params.id) navigate(last);
  else navigate({ name: last && last.name === 'code' ? 'code' : (store.appMode === 'code' ? 'code' : 'new') });
  linksReady = true;
  takeDeepLink();
  void currentLanguage;
}

// endless CSS animations pause while the window is hidden (see app.css)
document.addEventListener('visibilitychange', () => document.documentElement.toggleAttribute('data-hidden', document.hidden));

init();
