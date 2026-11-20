// Terminal pane: an xterm.js view of a real PTY shell in the project folder.
// xterm is loaded on first use, so it costs nothing until opened.
import { t } from '../i18n.js';
import { call, on } from '../api.js';
import { h, errorToast } from '../util.js';
import { registerPane, context } from '../workspace.js';

let lib = null;
const sessions = new Map(); // convId -> { term, fit, ptyId, el, exited }
const byPty = new Map();    // ptyId -> session

async function loadLib() {
  if (lib) return lib;
  if (!document.querySelector('link[data-xterm]')) {
    const l = document.createElement('link');
    l.rel = 'stylesheet'; l.href = 'vendor/xterm/xterm.css'; l.dataset.xterm = '1';
    document.head.appendChild(l);
  }
  const [x, f] = await Promise.all([import('../../vendor/xterm/xterm.mjs'), import('../../vendor/xterm/addon-fit.mjs')]);
  lib = { Terminal: x.Terminal, FitAddon: f.FitAddon };
  return lib;
}

function themeColors() {
  const cs = getComputedStyle(document.documentElement);
  const v = (n, d) => (cs.getPropertyValue(n).trim() || d);
  const dark = document.documentElement.dataset.theme === 'dark';
  return {
    background: v('--bg-1', dark ? '#1f1e1d' : '#faf9f5'),
    foreground: v('--text-0', dark ? '#eee' : '#222'),
    cursor: v('--accent', '#c96442'),
    selectionBackground: dark ? '#4a4a6a' : '#cfd8ff',
  };
}

async function start(s, ctx) {
  const { cols, rows } = s.term;
  try {
    const r = await call('term_open', { conversationId: ctx.convId, cwd: ctx.cwd, cols, rows });
    s.ptyId = r.id;
    s.exited = false;
    byPty.set(r.id, s);
  } catch (e) {
    s.term.writeln(`\r\n\x1b[31m${(e && e.message) || e}\x1b[0m`);
  }
}

async function mount(el, ctx) {
  el.classList.add('term-pane');
  const host = h('<div class="term-host"></div>');
  el.appendChild(host);
  let L;
  try { L = await loadLib(); } catch (e) { errorToast(e); return; }
  const term = new L.Terminal({
    fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--font-mono') || 'monospace',
    fontSize: 12.5,
    cursorBlink: true,
    scrollback: 5000,
    allowProposedApi: false,
    theme: themeColors(),
  });
  const fit = new L.FitAddon();
  term.loadAddon(fit);
  term.open(host);
  const s = { term, fit, ptyId: null, el, exited: false, convId: ctx.convId };
  sessions.set(ctx.convId, s);
  requestAnimationFrame(() => { try { fit.fit(); } catch { /* hidden */ } start(s, ctx); });
  term.onData((d) => {
    if (s.exited) {
      if (d === '\r') { term.clear(); start(s, context() || ctx); }
      return;
    }
    if (s.ptyId) call('term_write', { id: s.ptyId, data: d }).catch(() => {});
  });
  term.onResize(({ cols, rows }) => { if (s.ptyId && !s.exited) call('term_resize', { id: s.ptyId, cols, rows }).catch(() => {}); });
}

function refit() {
  const ctx = context();
  const s = ctx && sessions.get(ctx.convId);
  if (s && s.el.offsetParent) { try { s.fit.fit(); } catch { /* ignore */ } }
}

on('term-output', ({ id, data }) => { const s = byPty.get(id); if (s) s.term.write(data); });
on('term-exit', ({ id }) => {
  const s = byPty.get(id);
  if (!s) return;
  byPty.delete(id);
  s.exited = true;
  s.ptyId = null;
  s.term.writeln(`\r\n\x1b[2m${t('term_exited')}\x1b[0m`);
});
window.addEventListener('resize', refit);
window.addEventListener('workspace-resize', refit);
new MutationObserver(() => { for (const s of sessions.values()) s.term.options.theme = themeColors(); })
  .observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });

registerPane({
  id: 'terminal',
  order: 20,
  icon: 'terminal',
  label: () => t('ws_terminal'),
  mount,
  show: () => { requestAnimationFrame(() => { refit(); const ctx = context(); const s = ctx && sessions.get(ctx.convId); if (s) s.term.focus(); }); },
  dispose: (convId) => {
    const s = sessions.get(convId);
    if (!s) return;
    if (s.ptyId) call('term_close', { id: s.ptyId }).catch(() => {});
    s.term.dispose();
    sessions.delete(convId);
  },
});
