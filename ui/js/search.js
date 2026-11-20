// Command palette (Ctrl/Cmd+K): search chats and projects.
import { icon } from './icons.js';
import { t } from './i18n.js';
import { call } from './api.js';
import { store, emit } from './store.js';
import { $, esc, h, relTime } from './util.js';

let open = null;

export function openSearch() {
  if (open) { open.querySelector('input').focus(); return; }
  const scrim = h(`<div class="modal-scrim" style="align-items:flex-start"><div class="modal palette">
    <div class="pinput">${icon('search', 18)}<input placeholder="${t('search_placeholder')}"><span class="small muted">esc</span></div>
    <div class="results"></div></div></div>`);
  document.body.appendChild(scrim);
  open = scrim;
  const input = $('input', scrim);
  const res = $('.results', scrim);
  let items = [];
  let idx = 0;
  const close = () => { scrim.remove(); open = null; };
  const choose = (it) => { close(); if (it) emit('navigate', it.route); };
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
    const q = input.value;
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
    items = [...items.slice(0, 30), ...projs];
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
