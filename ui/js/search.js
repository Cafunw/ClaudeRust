// Command palette (Ctrl/Cmd+K): actions (">"), slash commands, files of the
// current code session ("@"), chats and projects.
import { icon } from './icons.js';
import { t } from './i18n.js';
import { call } from './api.js';
import { store, emit } from './store.js';
import { $, esc, h, relTime } from './util.js';
import * as workspace from './workspace.js';
import { allCommands, handle as runSlash } from './slash.js';

let open = null;

export function openSearch() {
  if (open) { open.querySelector('input').focus(); return; }
  const scrim = h(`<div class="modal-scrim" style="align-items:flex-start"><div class="modal palette">
    <div class="pinput">${icon('search', 18)}<input placeholder="${t('search_placeholder')}"><span class="small muted">esc</span></div>
    <div class="results"></div><div class="pal-hint small muted">${esc(t('pal_hint'))}</div></div></div>`);
  document.body.appendChild(scrim);
  open = scrim;
  const input = $('input', scrim);
  const res = $('.results', scrim);
  let items = [];
  let idx = 0;
  const close = () => { scrim.remove(); open = null; };
  const choose = (it) => {
    close();
    if (!it) return;
    if (it.run) it.run(); else emit('navigate', it.route);
  };
  const draw = () => {
    res.innerHTML = '';
    if (!items.length) { res.innerHTML = `<div class="muted" style="padding:16px;text-align:center">${t('no_results')}</div>`; return; }
    items.forEach((it, i) => {
      const b = h(`<button class="res ${i === idx ? 'kb' : ''}">${icon(it.icon, 16)}<div class="grow" style="min-width:0"><div class="t ellipsis">${esc(it.title)}</div>${it.snippet ? `<div class="s">${it.snippet}</div>` : ''}</div><span class="small muted">${esc(it.meta || '')}</span></button>`);
      b.onclick = () => choose(it);
      b.onmouseenter = () => { idx = i; res.querySelectorAll('.res').forEach((x, j) => x.classList.toggle('kb', j === i)); };
      res.appendChild(b);
    });
  };
  const run = async () => {
    const raw = input.value;
    const ctx = await chatContext();
    // "@file": files of the current code session, inserted as a mention
    if (raw.startsWith('@') && ctx && ctx.cwd) {
      const r = await call('file_search', { conversationId: ctx.conversationId, cwd: ctx.cwd, query: raw.slice(1) }).catch(() => []);
      items = r.map((f) => ({ title: f.path, icon: f.dir ? 'folder' : 'file', meta: t('pal_mention'), run: () => { if (ctx.composer) ctx.composer.insertText(/\s/.test(f.path) ? `@"${f.path}" ` : `@${f.path} `); } }));
      idx = 0;
      draw();
      return;
    }
    const actionsOnly = raw.startsWith('>');
    const q = actionsOnly ? raw.slice(1) : raw;
    const acts = (await actionItems(ctx)).map((a) => ({ a, s: score(a.title, q) })).filter((x) => x.s > 0).sort((x, y) => y.s - x.s).map((x) => x.a);
    void match;
    if (actionsOnly) {
      items = acts.slice(0, 40);
      idx = 0;
      draw();
      return;
    }
    const hits = await call('search', { query: q }).catch(() => []);
    items = hits.map((x) => ({
      title: x.title || t('untitled'),
      snippet: x.snippet ? esc(x.snippet).replace(/\u0001/g, '<mark>').replace(/\u0002/g, '</mark>') : '',
      meta: relTime(x.updated_at),
      icon: x.kind === 'code' ? 'terminal' : 'chat',
      route: { name: 'chat', params: { id: x.conversation_id } },
    }));
    const ql = q.trim().toLowerCase();
    const projs = store.projects.filter((p) => !ql || p.name.toLowerCase().includes(ql)).slice(0, ql ? 5 : 3)
      .map((p) => ({ title: p.name, snippet: esc(p.description || ''), meta: t('projects'), icon: 'folder', route: { name: 'project', params: { id: p.id } } }));
    items = q.trim() ? [...acts.slice(0, 6), ...items.slice(0, 30), ...projs] : [...items.slice(0, 30), ...projs];
    idx = 0;
    draw();
  };
  let tm;
  input.addEventListener('input', () => { clearTimeout(tm); tm = setTimeout(run, 120); });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.preventDefault(); close(); }
    else if (e.key === 'ArrowDown') { e.preventDefault(); idx = Math.min(items.length - 1, idx + 1); draw(); res.querySelector('.kb')?.scrollIntoView({ block: 'nearest' }); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); idx = Math.max(0, idx - 1); draw(); res.querySelector('.kb')?.scrollIntoView({ block: 'nearest' }); }
    else if (e.key === 'Enter') { e.preventDefault(); choose(items[idx]); }
  });
  scrim.addEventListener('mousedown', (e) => { if (e.target === scrim) close(); });
  input.focus();
  run();
}

/** Match score of a title for a query (0 = no match). */
function score(title, q) {
  const ql = q.trim().toLowerCase();
  if (!ql) return 1;
  const tl = title.toLowerCase();
  const at = tl.indexOf(ql);
  if (at >= 0) {
    const wordStart = at === 0 || /[\s/:—-]/.test(tl[at - 1]);
    return (wordStart ? 300 : 200) - Math.min(at, 100);
  }
  // all words present
  const words = ql.split(/\s+/).filter(Boolean);
  if (words.length > 1 && words.every((w) => tl.includes(w))) return 150;
  // characters in order, only for short queries
  if (ql.length > 4) return 0;
  let i = 0;
  for (const c of ql) {
    i = tl.indexOf(c, i);
    if (i < 0) return 0;
    i++;
  }
  return 20;
}
const match = (title, q) => score(title, q) > 0;

async function chatContext() {
  const chat = await import('./chat.js');
  const c = chat.current();
  if (!c) return null;
  return { kind: c.conv.kind, conversationId: c.id, cwd: c.conv.cwd, engine: (c.conv.options || {}).engine || 'builtin', conv: c.conv, composer: c.composer };
}

async function actionItems(ctx) {
  const a = [
    { title: t('new_chat'), icon: 'plus', meta: 'Ctrl+Shift+O', run: () => emit('navigate', { name: 'new' }) },
    { title: t('pal_new_code'), icon: 'code', run: () => emit('navigate', { name: 'code' }) },
    { title: t('chats'), icon: 'chat', run: () => emit('navigate', { name: 'chats' }) },
    { title: t('projects'), icon: 'folder', run: () => emit('navigate', { name: 'projects' }) },
    { title: t('artifacts'), icon: 'layers', run: () => emit('navigate', { name: 'artifacts' }) },
    { title: t('settings'), icon: 'settings', meta: 'Ctrl+,', run: () => emit('open-settings') },
    { title: `${t('settings')}: ${t('pal_appearance')}`, icon: 'sun', run: () => emit('open-settings', 'appearance') },
    { title: `${t('settings')}: Developer / MCP`, icon: 'plug', run: () => emit('open-settings', 'developer') },
    { title: t('pal_import'), icon: 'download', run: () => emit('open-settings', 'import') },
    { title: t('pal_routines'), icon: 'clock', run: async () => (await import('./routines.js')).openRoutines() },
    { title: t('pal_usage'), icon: 'zap', run: async () => (await import('./usage_ui.js')).openUsage() },
    { title: t('pal_toggle_theme'), icon: 'moon', run: () => emit('save-settings', { theme: document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark' }) },
  ];
  if (ctx && ctx.kind === 'code') {
    a.push({ title: t('ws_toggle'), icon: 'sidebar', meta: 'Ctrl+`', run: () => workspace.toggle() });
    for (const p of ['changes', 'terminal', 'plan', 'tasks', 'preview']) a.push({ title: t('cmd_open_pane', { pane: t('ws_' + p) }), icon: { changes: 'edit', terminal: 'terminal', plan: 'compass', tasks: 'list', preview: 'globe' }[p], run: () => workspace.show(p) });
  }
  if (ctx) {
    for (const c of await allCommands(ctx)) {
      a.push({ title: `/${c.name}${c.description ? ' — ' + c.description : ''}`, icon: c.kind === 'custom' ? 'fileText' : 'zap', meta: t('pal_command'), run: async () => {
        if (c.hint) { ctx.composer.setText(`/${c.name} `); ctx.composer.focus(); return; }
        const r = await runSlash(`/${c.name}`, ctx);
        if (r && r.prompt) { ctx.composer.setText(r.prompt); ctx.composer.send(); }
      } });
    }
  }
  return a;
}
