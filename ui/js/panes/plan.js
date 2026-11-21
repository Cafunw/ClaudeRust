// Plan pane: the session's latest plan (from plan mode) and its todo list,
// kept up to date while Claude works.
import { t } from '../i18n.js';
import { icon } from '../icons.js';
import { call, on } from '../api.js';
import { onTurn } from '../turnstream.js';
import { h, esc, debounce } from '../util.js';
import { registerPane, context, updateTabs, refresh } from '../workspace.js';

const cache = new Map(); // convId -> plan state

async function load(el, ctx) {
  let s;
  try { s = await call('plan_state', { conversationId: ctx.convId }); } catch { s = { todos: null, plan: null }; }
  cache.set(ctx.convId, s);
  updateTabs();
  await draw(el, ctx, s);
}

async function draw(el, ctx, s) {
  const todos = Array.isArray(s.todos) ? s.todos : [];
  const done = todos.filter((x) => x.status === 'completed').length;
  el.innerHTML = '';
  if (!todos.length && !s.plan) {
    el.appendChild(h(`<div class="pane-empty">${icon('compass', 22)}<div>${t('plan_empty')}</div>${s.mode !== 'plan' ? `<button class="btn sm" data-a="plan">${t('plan_start')}</button>` : ''}</div>`));
    const b = el.querySelector('[data-a=plan]');
    if (b) b.onclick = async () => { const chat = await import('../chat.js'); const c = chat.current(); if (c) { c.composer.setState({ permission_mode: 'plan' }); c.composer.refresh(); call('update_conversation', { req: { id: ctx.convId, options: { permission_mode: 'plan' } } }).catch(() => {}); c.composer.focus(); } };
    return;
  }
  if (todos.length) {
    const pct = Math.round((done / todos.length) * 100);
    const sec = h(`<div class="plan-sec"><div class="plan-h">${icon('list', 14)} ${t('todos')} <span class="muted">${done}/${todos.length}</span></div>
      <div class="progress"><div style="width:${pct}%"></div></div>
      <ul class="todos big">${todos.map((td) => `<li class="${esc(td.status)}"><span class="box">${td.status === 'completed' ? icon('check', 11) : td.status === 'in_progress' ? '<span class="spinner tiny"></span>' : ''}</span><span>${esc(td.status === 'in_progress' && td.activeForm ? td.activeForm : td.content)}</span></li>`).join('')}</ul></div>`);
    el.appendChild(sec);
  }
  if (s.plan) {
    const status = s.plan_status || 'pending';
    const sec = h(`<div class="plan-sec"><div class="plan-h">${icon('compass', 14)} ${t('plan_title')} <span class="plan-badge ${status}">${esc(t('plan_' + status))}</span></div><div class="md plan-md"></div></div>`);
    el.appendChild(sec);
    try { sec.querySelector('.plan-md').innerHTML = await call('render_markdown', { text: s.plan }); } catch { sec.querySelector('.plan-md').textContent = s.plan; }
  }
}

const reloadSoon = debounce(() => refresh('plan'), 500);

registerPane({
  id: 'plan',
  order: 30,
  icon: 'compass',
  label: () => t('ws_plan'),
  badge: (ctx) => {
    const s = cache.get(ctx.convId);
    if (!s || !Array.isArray(s.todos) || !s.todos.length) return '';
    const done = s.todos.filter((x) => x.status === 'completed').length;
    return done < s.todos.length ? `${done}/${s.todos.length}` : '';
  },
  // show() comes right after and loads it
  mount: (el) => { el.classList.add('plan-pane'); },
  show: (el, ctx) => load(el, ctx),
});

onTurn(({ conversation_id, turn }) => {
  const c = context();
  if (!c || c.convId !== conversation_id) return;
  const parts = (turn && turn.parts) || [];
  if (parts.some((p) => p.tool === 'TodoWrite' || p.tool === 'ExitPlanMode')) reloadSoon();
});
on('generation-state', ({ conversation_id, running }) => { const c = context(); if (c && c.convId === conversation_id && !running) reloadSoon(); });
