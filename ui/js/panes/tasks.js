// Tasks pane: background commands (and sub-agents) of the current code
// session, with live output and stop buttons.
import { t } from '../i18n.js';
import { icon } from '../icons.js';
import { call, on } from '../api.js';
import { h, esc, errorToast } from '../util.js';
import { registerPane, refresh, context, updateTabs } from '../workspace.js';

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
  // everything is fetched before the pane is cleared, so overlapping redraws
  // (the 2 s timer, events) can't both append
  const scripts = await scriptsFor(ctx);
  counts.set(ctx.convId, rows.filter((r) => r.status === 'running').length);
  updateTabs();
  el.innerHTML = '';
  const head = h(`<div class="pane-head"><span class="muted">${t('ws_tasks_d')}</span><div class="grow"></div><button class="btn sm ghost" data-a="clear">${t('clear_finished')}</button></div>`);
  head.querySelector('[data-a=clear]').onclick = async () => { await call('shells_clear', { conversationId: ctx.convId }).catch(errorToast); await call('subagents_clear', { conversationId: ctx.convId }).catch(() => {}); await call('workflows_clear', { conversationId: ctx.convId }).catch(() => {}); draw(el, ctx); };
  el.appendChild(head);
  el.appendChild(scriptsSection(el, ctx, scripts));
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

// ---- the project's scripts (package.json, Makefile, justfile, Cargo, …)

const scriptCache = new Map(); // convId -> { at, list }
const scriptsOpen = new Map(); // convId -> bool

async function scriptsFor(ctx, force) {
  const c = scriptCache.get(ctx.convId);
  if (c && !force && Date.now() - c.at < 30_000) return c.list;
  let list = [];
  try { list = await call('project_scripts', { conversationId: ctx.convId }); } catch { list = []; }
  scriptCache.set(ctx.convId, { at: Date.now(), list });
  return list;
}

function scriptsSection(el, ctx, list) {
  const box = h('<div class="scripts"></div>');
  if (!list.length) return box;
  const open = scriptsOpen.has(ctx.convId) ? scriptsOpen.get(ctx.convId) : list.length <= 12;
  const head = h(`<button class="scripts-head">${icon(open ? 'chevDown' : 'chevRight', 13)}<span>${esc(t('scripts_title'))}</span><span class="muted">${list.length}</span></button>`);
  head.onclick = () => { scriptsOpen.set(ctx.convId, !open); draw(el, ctx); };
  box.appendChild(head);
  if (!open) return box;
  for (const sc of list) {
    const r = h(`<div class="script-row" title="${esc(sc.detail ? `${sc.command}\n${sc.detail}` : sc.command)}">
      <span class="sc-src">${esc(sc.source)}</span><span class="sc-name ellipsis">${esc(sc.name)}</span><span class="sc-cmd ellipsis">${esc(sc.detail || sc.command)}</span>
      <button class="icon-btn sm" data-a="run" title="${esc(t('scripts_run'))}">${icon('play', 13)}</button>
      <button class="icon-btn sm" data-a="fix" title="${esc(t('scripts_fix'))}">${icon('sparkle', 13)}</button></div>`);
    r.querySelector('[data-a=run]').onclick = async () => {
      const term = await import('./terminal.js');
      if (!(await term.runInTerminal(sc.command))) errorToast(new Error(t('code_run_failed')));
    };
    r.querySelector('[data-a=fix]').onclick = async () => {
      const chat = await import('../chat.js');
      const c = chat.current();
      if (!c || c.id !== ctx.convId) return;
      c.composer.setText(t('scripts_fix_p', { cmd: sc.command }));
      c.composer.send();
    };
    box.appendChild(r);
  }
  return box;
}

let timer = null;
let shownEl = null; // the pane element the timer redraws
registerPane({
  id: 'tasks',
  order: 40,
  icon: 'list',
  label: () => t('ws_tasks'),
  badge: (ctx) => counts.get(ctx.convId) || '',
  // show() comes right after and draws it
  mount: (el) => { el.classList.add('tasks-pane'); },
  show: (el, ctx) => {
    draw(el, ctx);
    clearInterval(timer);
    shownEl = el;
    // the elapsed times and output tick while it's shown; not while the
    // window is hidden (it's drawn again the moment it's back)
    timer = setInterval(() => { const c = context(); if (c && el.offsetParent) { if (!document.hidden) draw(el, c); } else clearInterval(timer); }, 2000);
  },
  hide: () => { clearInterval(timer); timer = null; },
});

document.addEventListener('visibilitychange', () => {
  if (document.hidden || !timer || !shownEl) return;
  const c = context();
  if (c && shownEl.offsetParent) draw(shownEl, c);
});

// sub-agents of the built-in agent
addTaskProvider(async (ctx) => {
  const list = await call('subagents_list', { conversationId: ctx.convId });
  return list.map((a) => ({
    id: a.id, kind: 'agent', title: `${t('subagent')} · ${a.description}`, sub: `${a.steps} ${t('subagent_steps')}`, status: a.status === 'stopped' ? 'killed' : a.status,
    started: a.started_at, finished: a.finished_at, exit: null,
    onStop: () => call('subagent_stop', { id: a.id }),
    output: () => call('subagent_output', { id: a.id }),
  }));
});
on('subagents-changed', ({ conversation_id }) => { const c = context(); if (c && c.convId === conversation_id) refresh('tasks'); });

// Claude Code's workflows (ultracode): the phase it is in, every agent and what it cost so far
const kTokens = (n) => (n >= 1000 ? `${Math.round(n / 100) / 10}k` : String(n || 0));
function workflowText(w) {
  const lines = [];
  for (const p of w.phases || []) {
    lines.push(p.title);
    for (const a of (w.agents || []).filter((x) => x.phase === p.title)) {
      const ended = a.state === 'done' || a.state === 'error';
      const mark = a.state === 'done' ? '✓' : a.state === 'error' ? '✗' : '…';
      const bits = [a.model && String(a.model).replace(/^claude-/, ''), a.tokens ? `${kTokens(a.tokens)} tok` : '', a.duration_ms ? `${Math.round(a.duration_ms / 1000)}s` : '', !ended && a.last_tool ? a.last_tool : ''].filter(Boolean);
      lines.push(`  ${mark} ${a.label}${bits.length ? ' · ' + bits.join(' · ') : ''}`);
      // what it came back with, or why it failed
      const out = (a.state === 'error' ? a.error : a.result) || '';
      if (ended && out) lines.push(`      ${out.replace(/\s+/g, ' ').slice(0, 200)}`);
    }
  }
  return lines.join('\n') || w.description || '';
}
addTaskProvider(async (ctx) => {
  const list = await call('workflows_list', { conversationId: ctx.convId });
  return list.slice().reverse().map((w) => {
    const agents = w.agents || [];
    const live = agents.filter((a) => a.state !== 'done' && a.state !== 'error');
    const phase = (live[0] || agents[agents.length - 1] || {}).phase;
    return {
      id: 'wf-' + w.id, kind: 'workflow', title: `${t('workflow')} · ${w.name || w.description || ''}`,
      sub: [phase, agents.length ? `${agents.length - live.length}/${agents.length} ${t('workflow_agents')}` : '', w.tokens ? `${kTokens(w.tokens)} tok` : ''].filter(Boolean).join(' · '),
      status: w.status, started: w.started_at, finished: w.finished_at, exit: null,
      onStop: () => call('workflow_stop', { conversationId: ctx.convId, taskId: w.id }),
      output: async () => workflowText(w),
    };
  });
});
on('workflows-changed', ({ conversation_id }) => {
  const c = context();
  if (!c || c.convId !== conversation_id) return;
  // the tab's badge counts them too, while the pane isn't drawn
  Promise.all([call('workflows_list', { conversationId: c.convId }), call('shells_list', { conversationId: c.convId })])
    .then(([w, s]) => { counts.set(c.convId, [...w, ...s].filter((x) => x.status === 'running').length); refresh(); })
    .catch(() => {});
});

on('shells-changed', ({ conversation_id }) => {
  const c = context();
  if (c && c.convId === conversation_id) {
    call('shells_list', { conversationId: c.convId }).then((l) => { counts.set(c.convId, l.filter((s) => s.status === 'running').length); refresh(); }).catch(() => {});
  }
});
