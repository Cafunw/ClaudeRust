// Tasks pane: background commands (and sub-agents) of the current code
// session, with live output and stop buttons.
import { t } from '../i18n.js';
import { icon } from '../icons.js';
import { call, on } from '../api.js';
import { h, esc, errorToast } from '../util.js';
import { registerPane, refresh, context } from '../workspace.js';

const expanded = new Set();
const counts = new Map(); // convId -> running count
const extra = [];         // providers of more task rows: (ctx) => Promise<rows>

/** Let other modules (sub-agents) contribute rows: { id, title, sub, status, onStop, output }. */
export function addTaskProvider(fn) { extra.push(fn); }

function elapsed(from, to) {
  const s = Math.max(0, Math.round(((to || Date.now()) - from) / 1000));
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m ${s % 60}s` : `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

async function draw(el, ctx) {
  let shells = [];
  try { shells = await call('shells_list', { conversationId: ctx.convId }); } catch (e) { errorToast(e); }
  const rows = shells.map((s) => ({
    id: s.id, kind: 'shell', title: s.description || s.command, sub: s.command, status: s.status,
    started: s.started_at, finished: s.finished_at, exit: s.exit_code,
  }));
  for (const fn of extra) { try { rows.push(...(await fn(ctx))); } catch { /* ignore */ } }
  counts.set(ctx.convId, rows.filter((r) => r.status === 'running').length);
  el.innerHTML = '';
  const head = h(`<div class="pane-head"><span class="muted">${t('ws_tasks_d')}</span><div class="grow"></div><button class="btn sm ghost" data-a="clear">${t('clear_finished')}</button></div>`);
  head.querySelector('[data-a=clear]').onclick = async () => { await call('shells_clear', { conversationId: ctx.convId }).catch(errorToast); draw(el, ctx); };
  el.appendChild(head);
  if (!rows.length) { el.appendChild(h(`<div class="pane-empty">${icon('list', 22)}<div>${t('ws_tasks_empty')}</div></div>`)); return; }
  for (const r of rows) {
    const open = expanded.has(r.id);
    const row = h(`<div class="task-row ${r.status}">
      <div class="task-line"><span class="dot"></span>
        <div class="task-main"><div class="task-title ellipsis">${esc(r.title)}</div><div class="task-sub ellipsis">${esc(r.sub || '')}</div></div>
        <span class="task-time">${r.status === 'running' ? elapsed(r.started) : (r.exit != null && r.exit !== 0 ? `exit ${r.exit} · ` : '') + elapsed(r.started, r.finished)}</span>
        <button class="icon-btn" data-a="out" title="${t('output')}">${icon(open ? 'chevDown' : 'chevRight', 14)}</button>
        ${r.status === 'running' ? `<button class="icon-btn" data-a="stop" title="${t('stop')}">${icon('stop', 14)}</button>` : ''}
      </div>
      ${open ? '<pre class="task-out">…</pre>' : ''}</div>`);
    row.querySelector('[data-a=out]').onclick = () => { if (open) expanded.delete(r.id); else expanded.add(r.id); draw(el, ctx); };
    const stop = row.querySelector('[data-a=stop]');
    if (stop) stop.onclick = async () => {
      try { if (r.onStop) await r.onStop(); else await call('shell_kill', { id: r.id }); } catch (e) { errorToast(e); }
      draw(el, ctx);
    };
    if (open) {
      const pre = row.querySelector('.task-out');
      const fill = async () => {
        try {
          const text = r.output ? await r.output() : (await call('shell_output', { id: r.id })).output;
          pre.textContent = text || t('no_output');
          pre.scrollTop = pre.scrollHeight;
        } catch (e) { pre.textContent = (e && e.message) || String(e); }
      };
      fill();
    }
    el.appendChild(row);
  }
}

let timer = null;
registerPane({
  id: 'tasks',
  order: 40,
  icon: 'list',
  label: () => t('ws_tasks'),
  badge: (ctx) => counts.get(ctx.convId) || '',
  mount: (el, ctx) => { el.classList.add('tasks-pane'); draw(el, ctx); },
  show: (el, ctx) => {
    draw(el, ctx);
    clearInterval(timer);
    timer = setInterval(() => { const c = context(); if (c && el.offsetParent) draw(el, c); else clearInterval(timer); }, 2000);
  },
  hide: () => { clearInterval(timer); timer = null; },
});

on('shells-changed', ({ conversation_id }) => {
  const c = context();
  if (c && c.convId === conversation_id) {
    call('shells_list', { conversationId: c.convId }).then((l) => { counts.set(c.convId, l.filter((s) => s.status === 'running').length); refresh(); }).catch(() => {});
  }
});
