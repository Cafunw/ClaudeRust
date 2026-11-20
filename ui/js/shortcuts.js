// Extra keyboard shortcuts (switch between recent conversations, open
// side-panel panes) and the shortcut sheet (Ctrl+/ or /shortcuts).
import { t } from './i18n.js';
import { isMac } from './api.js';
import { store, emit, on } from './store.js';
import { esc, modal } from './util.js';
import { registerCommand } from './slash.js';
import * as workspace from './workspace.js';

const MOD = isMac ? '⌘' : 'Ctrl';
const PANES = ['changes', 'history', 'plan', 'terminal', 'tasks', 'preview', 'map'];

// most recently used conversations, newest first
const mru = [];
on('route-changed', () => {
  const r = store.route;
  if (r && r.name === 'chat' && r.params && r.params.id) {
    const i = mru.indexOf(r.params.id);
    if (i >= 0) mru.splice(i, 1);
    mru.unshift(r.params.id);
    if (mru.length > 20) mru.pop();
  }
});

let cycle = null; // { index } while Ctrl is held
window.addEventListener('keydown', (e) => {
  const mod = isMac ? e.metaKey : e.ctrlKey;
  // Ctrl+Tab / Ctrl+Shift+Tab: previous conversations, like an editor
  if (e.ctrlKey && e.key === 'Tab') {
    e.preventDefault();
    if (mru.length < 2) return;
    if (!cycle) cycle = { index: 0 };
    cycle.index = (cycle.index + (e.shiftKey ? mru.length - 1 : 1)) % mru.length;
    emit('navigate', { name: 'chat', params: { id: mru[cycle.index] } });
    return;
  }
  // Alt+1..7: side-panel panes of a code session
  if (e.altKey && !e.ctrlKey && !e.metaKey && /^[1-7]$/.test(e.key) && workspace.context()) {
    e.preventDefault();
    workspace.toggle(PANES[+e.key - 1]);
    return;
  }
  if (mod && e.key === '/') { e.preventDefault(); openShortcuts(); }
}, true);
window.addEventListener('keyup', (e) => { if (e.key === 'Control') cycle = null; });

export function openShortcuts() {
  const rows = [
    [t('new_chat'), `${MOD}+N`],
    [t('sc_palette'), `${MOD}+K`],
    [t('sc_palette_cmds'), `${MOD}+K, >`],
    [t('sc_switch_conv'), 'Ctrl+Tab / Ctrl+Shift+Tab'],
    [t('sc_sidebar'), `${MOD}+B`],
    [t('settings'), `${MOD}+,`],
    [t('ws_toggle'), `${MOD}+\``],
    [t('sc_panes'), 'Alt+1 … Alt+7'],
    [t('sc_send'), 'Enter'],
    [t('sc_newline'), 'Shift+Enter'],
    [t('sc_queue'), t('sc_queue_k')],
    [t('sc_stop'), 'Esc'],
    [t('sc_edit_last'), '↑'],
    [t('sc_mention'), '@'],
    [t('sc_commands'), '/'],
    [t('sc_memory'), '# …'],
    [t('sc_zoom'), `${MOD} + / − / 0`],
    [t('sc_quick'), isMac ? '⌥Space' : 'Ctrl+Alt+Space'],
    [t('sc_sheet'), `${MOD}+/`],
  ];
  modal({
    title: t('sc_sheet_title'),
    body: `<table class="kv shortcuts">${rows.map(([a, k]) => `<tr><td>${esc(a)}</td><td><kbd>${esc(k)}</kbd></td></tr>`).join('')}</table>
      <p class="muted small" style="margin-top:10px">${esc(t('sc_panes_list'))}</p>`,
  });
}

registerCommand({ name: 'shortcuts', description: () => t('cmd_shortcuts'), run: () => openShortcuts() });
