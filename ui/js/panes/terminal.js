// Terminal pane: an xterm.js view of a real PTY shell in the project folder.
// xterm is loaded on first use, so it costs nothing until opened.
import { t } from '../i18n.js';
import { call, on } from '../api.js';
import { h, esc, errorToast, toast, showMenu } from '../util.js';
import { icon } from '../icons.js';
import { registerPane, context } from '../workspace.js';

let lib = null;
const sessions = new Map(); // convId -> { term, fit, ptyId, el, exited }
const byPty = new Map();    // ptyId -> session
const unclaimed = new Map(); // ptyId -> { at, data } output seen before its session

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
    // output that arrived before we knew the id (the shell's first prompt)
    const early = unclaimed.get(r.id);
    if (early) { unclaimed.delete(r.id); s.term.write(early.data); }
  } catch (e) {
    s.term.writeln(`\r\n\x1b[31m${(e && e.message) || e}\x1b[0m`);
  }
}

/** Text of the last `maxLines` screen lines up to the cursor, with wrapped
 *  lines joined and trailing blank lines dropped. */
export function recentOutput(term, maxLines = 1000) {
  const buf = term.buffer.active;
  const end = buf.baseY + buf.cursorY;
  const lines = [];
  for (let i = Math.max(0, end - maxLines); i <= end; i++) {
    const l = buf.getLine(i);
    if (!l) continue;
    const text = l.translateToString(true);
    if (l.isWrapped && lines.length) lines[lines.length - 1] += text;
    else lines.push(text);
  }
  while (lines.length && !lines[lines.length - 1].trim()) lines.pop();
  while (lines.length && !lines[0].trim()) lines.shift();
  return lines.join('\n');
}

const INLINE_MAX = 4000;

/** Put terminal text into the conversation: inline as a code block when
 *  short, as an attachment when long. `fix` also asks Claude to fix it. */
async function sendToChat(s, fix) {
  const sel = s.term.getSelection();
  const text = (sel && sel.trim()) ? sel.replace(/\s+$/, '') : recentOutput(s.term);
  if (!text.trim()) { toast(t('term_nothing')); return; }
  const chat = await import('../chat.js');
  const c = chat.current();
  if (!c || !c.composer) return;
  const ask = fix ? t('term_fix_prompt') : '';
  try {
    if (text.length > INLINE_MAX) {
      c.composer.addStaged(await call('stage_text', { name: 'Terminal output.txt', text }));
      if (ask) c.composer.setText(ask);
    } else {
      const block = `${sel && sel.trim() ? t('term_selection_intro') : t('term_output_intro')}\n\`\`\`\n${text}\n\`\`\`\n`;
      c.composer.setText(ask ? `${ask}\n\n${block}` : block);
    }
    if (fix) c.composer.send();
    else c.composer.focus();
  } catch (e) { errorToast(e); }
}

async function mount(el, ctx) {
  el.classList.add('term-pane');
  const host = h('<div class="term-host"></div>');
  const bar = h(`<div class="term-actions"><button class="btn sm" data-a="ask" title="${esc(t('term_ask_tip'))}">${icon('chat', 13)} ${esc(t('term_ask'))} ${icon('chevDown', 12)}</button></div>`);
  el.append(bar, host);
  bar.querySelector('[data-a=ask]').onclick = (ev) => {
    const s = sessions.get(ctx.convId);
    if (!s) return;
    const hasSel = !!(s.term.getSelection() || '').trim();
    showMenu(ev.currentTarget, [
      { label: hasSel ? t('term_add_selection') : t('term_add_output'), icon: 'plus', onClick: () => sendToChat(s, false) },
      { label: t('term_fix'), icon: 'wrench', onClick: () => sendToChat(s, true) },
    ], { placement: 'bottom-end' });
  };
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

on('term-output', ({ id, data }) => {
  const s = byPty.get(id);
  if (s) { s.term.write(data); return; }
  // A new shell can print before `term_open` has returned its id: keep that
  // output for a few seconds so `start` can show it.
  const now = Date.now();
  for (const [k, v] of unclaimed) if (now - v.at > 5000) unclaimed.delete(k);
  const u = unclaimed.get(id) || { at: now, data: '' };
  if (u.data.length < 65536) u.data += data;
  unclaimed.set(id, u);
});
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
