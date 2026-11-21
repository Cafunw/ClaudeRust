// Extra keyboard shortcuts (switch between recent conversations, open
// side-panel panes) and the shortcut sheet (Ctrl+/ or /shortcuts).
import { t } from './i18n.js';
import { isMac } from './api.js';
import { store, emit, on } from './store.js';
import { esc, modal } from './util.js';
import { registerCommand } from './slash.js';
import * as workspace from './workspace.js';
import * as keymap from './keymap.js';

const MOD = isMac ? '⌘' : 'Ctrl';
const PANES = ['changes', 'history', 'plan', 'terminal', 'tasks', 'preview', 'map', 'files'];

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
  // Ctrl+Tab / Ctrl+Shift+Tab: previous conversations, like an editor
  if (e.ctrlKey && e.key === 'Tab') {
    e.preventDefault();
    if (mru.length < 2) return;
    if (!cycle) cycle = { index: 0 };
    cycle.index = (cycle.index + (e.shiftKey ? mru.length - 1 : 1)) % mru.length;
    emit('navigate', { name: 'chat', params: { id: mru[cycle.index] } });
    return;
  }
  // Alt+1..8: side-panel panes of a code session
  if (e.altKey && !e.ctrlKey && !e.metaKey && /^[1-8]$/.test(e.key) && workspace.context()) {
    e.preventDefault();
    workspace.toggle(PANES[+e.key - 1]);
    return;
  }
  if (keymap.matches(e, 'shortcuts')) { e.preventDefault(); openShortcuts(); }
}, true);
window.addEventListener('keyup', (e) => { if (e.key === 'Control') cycle = null; });

export function openShortcuts() {
  const rows = [
    [t('new_chat'), keymap.shown('new_chat')],
    [t('sc_palette'), keymap.shown('palette')],
    [t('sc_palette_cmds'), keymap.shown('palette') ? `${keymap.shown('palette')}, >` : ''],
    [t('sc_switch_conv'), 'Ctrl+Tab / Ctrl+Shift+Tab'],
    [t('sc_sidebar'), keymap.shown('sidebar')],
    [t('settings'), keymap.shown('settings')],
    [t('ws_toggle'), keymap.shown('workspace')],
    [t('sc_panes'), 'Alt+1 … Alt+8'],
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
    [t('sc_find'), keymap.shown('find')],
    [t('sc_history'), keymap.shown('history')],
    [t('sc_sheet'), keymap.shown('shortcuts')],
  ].filter(([, k]) => k);
  modal({
    title: t('sc_sheet_title'),
    body: `<table class="kv shortcuts">${rows.map(([a, k]) => `<tr><td>${esc(a)}</td><td><kbd>${esc(k)}</kbd></td></tr>`).join('')}</table>
      <p class="muted small" style="margin-top:10px">${esc(t('sc_panes_list'))}</p><p class="muted small">${esc(t('sc_customize'))}</p>`,
  });
}

registerCommand({ name: 'shortcuts', description: () => t('cmd_shortcuts'), run: () => openShortcuts() });
