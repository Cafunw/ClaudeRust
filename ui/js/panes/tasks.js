// Tasks pane: background commands (and sub-agents) of the current code
// session, with live output and stop buttons.
import { t, currentLanguage } from '../i18n.js';
import { icon } from '../icons.js';
import { call, on } from '../api.js';
import { h, esc, errorToast, snapshot, sameSnapshot } from '../util.js';
import { registerPane, refresh, context, updateTabs } from '../workspace.js';
import { errorText } from '../neterr.js';

const expanded = new Set();
const counts = new Map(); // convId -> running count
const extra = [];         // providers of more task rows: (ctx) => Promise<rows>

/** Let other modules (sub-agents) contribute rows: { id, title, sub, status, onStop, output }. */
export function addTaskProvider(fn) { extra.push(fn); }

// "5 sec" / "5 sn.": the units in the UI's language
const unit = (n, u) => new Intl.NumberFormat(currentLanguage(), { style: 'unit', unit: u, unitDisplay: 'short' }).format(n);

/** How long from `from` to `to` (or now), in ms: "45 sec", "3 min 4 sec", "2 hr 5 min". */
export function elapsed(from, to) {
  const s = Math.max(0, Math.round(((to || Date.now()) - from) / 1000));
  if (s < 60) return unit(s, 'second');
  if (s < 3600) return `${unit(Math.floor(s / 60), 'minute')} ${unit(s % 60, 'second')}`;
  return `${unit(Math.floor(s / 3600), 'hour')} ${unit(Math.floor((s % 3600) / 60), 'minute')}`;
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
  // the 2 s tick with nothing new but the clock: the times and open outputs
  // are updated in place, so focus, hover and scroll positions stay
  const sig = snapshot([rows.map((r) => [r.id, r.title, r.sub, r.status, r.exit, expanded.has(r.id)]), scripts, scriptsOpen.get(ctx.convId) ?? null, currentLanguage()]);
  if (sameSnapshot(el.__sig, sig)) {
    for (const r of rows) {
      const row = [...el.querySelectorAll('.task-row')].find((x) => x.dataset.id === String(r.id));
      if (!row) continue;
      row.querySelector('.task-time').textContent = timeText(r);
      if (expanded.has(r.id)) fillOutput(row.querySelector('.task-out'), r);
    }
    return;
  }
  el.__sig = sig;
  // a button pressed with the keyboard is drawn again: focus goes to its new copy
  const f = el.contains(document.activeElement) ? document.activeElement : null;
  const keep = f && f.dataset.a ? `${f.closest('[data-id]') ? `[data-id="${CSS.escape(f.closest('[data-id]').dataset.id)}"] ` : ''}[data-a="${f.dataset.a}"]` : null;
  const restore = () => { const b = keep && document.activeElement === document.body && el.querySelector(keep); if (b) b.focus(); };
  el.innerHTML = '';
  const head = h(`<div class="pane-head"><span class="muted">${t('ws_tasks_d')}</span><div class="grow"></div><button class="btn sm ghost" data-a="clear">${t('clear_finished')}</button></div>`);
  head.querySelector('[data-a=clear]').onclick = async () => { await call('shells_clear', { conversationId: ctx.convId }).catch(errorToast); await call('subagents_clear', { conversationId: ctx.convId }).catch(() => {}); await call('workflows_clear', { conversationId: ctx.convId }).catch(() => {}); draw(el, ctx); };
  el.appendChild(head);
  el.appendChild(scriptsSection(el, ctx, scripts));
  if (!rows.length) { el.appendChild(h(`<div class="pane-empty">${icon('list', 22)}<div>${t('ws_tasks_empty')}</div></div>`)); restore(); return; }
  for (const r of rows) {
    const open = expanded.has(r.id);
    const row = h(`<div class="task-row ${esc(r.status)}" data-id="${esc(String(r.id))}">
      <div class="task-line"><span class="dot"></span>
        <div class="task-main"><div class="task-title ellipsis" title="${esc(r.title)}">${esc(r.title)}</div><div class="task-sub ellipsis" title="${esc(r.sub || '')}">${esc(r.sub || '')}</div></div>
        <span class="task-time">${esc(timeText(r))}</span>
        <button class="icon-btn" data-a="out" title="${t('output')}" aria-label="${esc(t('output'))}" aria-expanded="${open}">${icon(open ? 'chevDown' : 'chevRight', 14)}</button>
        ${r.status === 'running' ? `<button class="icon-btn" data-a="stop" title="${t('stop')}" aria-label="${esc(t('stop'))}">${icon('stop', 14)}</button>` : ''}
      </div>
      ${open ? '<pre class="task-out" tabindex="0">…</pre>' : ''}</div>`);
    row.querySelector('[data-a=out]').onclick = () => { if (open) expanded.delete(r.id); else expanded.add(r.id); draw(el, ctx); };
    const stop = row.querySelector('[data-a=stop]');
    if (stop) stop.onclick = async () => {
      try { if (r.onStop) await r.onStop(); else await call('shell_kill', { id: r.id }); } catch (e) { errorToast(e); }
      draw(el, ctx);
    };
    if (open) fillOutput(row.querySelector('.task-out'), r);
    el.appendChild(row);
  }
  restore();
}

function timeText(r) {
  if (r.status === 'running') return elapsed(r.started);
  return (r.exit != null && r.exit !== 0 ? `${t('task_exit', { n: r.exit })} · ` : '') + elapsed(r.started, r.finished);
}

/** A task's output into `pre`; it follows the end only while it's scrolled to the end. */
async function fillOutput(pre, r) {
  let text;
  try { text = (r.output ? await r.output() : (await call('shell_output', { id: r.id })).output) || t('no_output'); } catch (e) { text = errorText(e); }
  if (pre.textContent === text) return;
  const atEnd = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 4 || pre.textContent === '…';
  pre.textContent = text;
  if (atEnd) pre.scrollTop = pre.scrollHeight;
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
  const head = h(`<button class="scripts-head" data-a="scripts" aria-expanded="${open}">${icon(open ? 'chevDown' : 'chevRight', 13)}<span>${esc(t('scripts_title'))}</span><span class="muted">${list.length}</span></button>`);
  head.onclick = () => { scriptsOpen.set(ctx.convId, !open); draw(el, ctx); };
  box.appendChild(head);
  if (!open) return box;
  for (const sc of list) {
    const r = h(`<div class="script-row" data-id="${esc(`script:${sc.source}:${sc.name}`)}" title="${esc(sc.detail ? `${sc.command}\n${sc.detail}` : sc.command)}">
      <span class="sc-src">${esc(sc.source)}</span><span class="sc-name ellipsis">${esc(sc.name)}</span><span class="sc-cmd ellipsis">${esc(sc.detail || sc.command)}</span>
      <button class="icon-btn sm" data-a="run" title="${esc(t('scripts_run'))}" aria-label="${esc(t('scripts_run'))}: ${esc(sc.name)}">${icon('play', 13)}</button>
      <button class="icon-btn sm" data-a="fix" title="${esc(t('scripts_fix'))}" aria-label="${esc(t('scripts_fix'))}: ${esc(sc.name)}">${icon('sparkle', 13)}</button></div>`);
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
      const bits = [a.model && String(a.model).replace(/^claude-/, ''), a.tokens ? `${kTokens(a.tokens)} ${t('tokens')}` : '', a.duration_ms ? elapsed(0, a.duration_ms) : '', !ended && a.last_tool ? a.last_tool : ''].filter(Boolean);
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
      sub: [phase, agents.length ? `${agents.length - live.length}/${agents.length} ${t('workflow_agents')}` : '', w.tokens ? `${kTokens(w.tokens)} ${t('tokens')}` : ''].filter(Boolean).join(' · '),
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
