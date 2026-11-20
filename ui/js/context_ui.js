// Context window meter (topbar), /compact, and the "earlier conversation
// summarized" divider.
import { t } from './i18n.js';
import { call, on } from './api.js';
import { h, esc, toast, errorToast, fmtTokens, modal } from './util.js';
import { registerCommand } from './slash.js';

/** A topbar element showing context use and cost; refreshes itself. */
export function contextMeter(conversationId, onClick) {
  const el = h(`<button class="ctx-meter" title=""><svg viewBox="0 0 20 20" width="16" height="16"><circle cx="10" cy="10" r="8" class="trk"/><circle cx="10" cy="10" r="8" class="val" transform="rotate(-90 10 10)"/></svg><span class="pct"></span><span class="cost"></span></button>`);
  el.onclick = onClick;
  el.dataset.conv = conversationId;
  updateMeter(el);
  return el;
}

export async function updateMeter(el) {
  let u;
  try { u = await call('usage_stats', { conversationId: el.dataset.conv }); } catch { return; }
  const used = u.context_tokens || 0;
  const win = u.context_window || 0;
  const pct = win ? Math.min(100, Math.round((used / win) * 100)) : 0;
  const circ = 2 * Math.PI * 8;
  const val = el.querySelector('.val');
  val.setAttribute('stroke-dasharray', `${(circ * pct) / 100} ${circ}`);
  el.classList.toggle('warn', pct >= 70);
  el.classList.toggle('full', u.auto_compact_pct && pct >= u.auto_compact_pct);
  el.classList.toggle('empty', !used);
  el.querySelector('.pct').textContent = used ? `${pct}%` : '';
  el.querySelector('.cost').textContent = u.cost_usd ? `$${u.cost_usd < 0.01 ? u.cost_usd.toFixed(3) : u.cost_usd.toFixed(2)}` : '';
  el.title = `${t('context_used')}: ${fmtTokens(used)} / ${fmtTokens(win)}${u.pending_compact ? ` · ${t('compact_pending')}` : ''}\n${t('cost')}: $${(u.cost_usd || 0).toFixed(4)}`;
}

export function refreshMeters() {
  document.querySelectorAll('.ctx-meter').forEach((el) => updateMeter(el));
}

/** Divider shown before a message whose earlier context was summarized. */
export function compactDivider(summary, auto) {
  const el = h(`<div class="compact-divider"><button class="cd-head"><span class="line"></span><span class="lab">${esc(auto ? t('compact_divider_auto') : t('compact_divider'))}</span><span class="line"></span></button><div class="cd-body hidden"></div></div>`);
  el.querySelector('.cd-head').onclick = async () => {
    const b = el.querySelector('.cd-body');
    if (b.classList.contains('hidden') && !b.dataset.done) {
      b.dataset.done = '1';
      try { b.innerHTML = await call('render_markdown', { text: summary || '' }); } catch { b.textContent = summary || ''; }
      b.classList.add('md');
    }
    b.classList.toggle('hidden');
  };
  return el;
}

registerCommand({
  name: 'compact',
  description: () => t('cmd_compact'),
  hint: '[instructions]',
  run: async (args, ctx) => {
    if (ctx.engine === 'claude-code') return `/compact ${args}`.trim();
    if (!ctx.conversationId) return undefined;
    try {
      await call('compact_conversation', { conversationId: ctx.conversationId, instructions: args || null });
      const chat = await import('./chat.js');
      chat.showPendingCompact();
    } catch (e) { errorToast(e); }
    return undefined;
  },
});

let compactToast = null;
on('compacting', ({ auto }) => { compactToast = toast(auto ? t('compacting_auto') : t('compacting')); });
on('compacted', ({ error, auto }) => {
  void compactToast;
  if (error) toast(`${t('compact_failed')}: ${error}`, 'error');
  else toast(auto ? t('compacted_auto') : t('compacted'));
  refreshMeters();
});
on('generation-state', ({ running }) => { if (!running) refreshMeters(); });

const CTX_COLORS = { system: '#8e5cc2', memory: '#2f8a8a', tools: '#4a7bd0', mcp_tools: '#5a6b8c', messages: '#c96442', tool_calls: '#b07a10', summary: '#3a9a5b' };

async function openContext(ctx) {
  if (!ctx.conversationId) return;
  let d;
  try { d = await call('context_breakdown', { conversationId: ctx.conversationId }); } catch (e) { errorToast(e); return; }
  const win = d.window || 1;
  const pct = (n) => Math.max(0, (n / win) * 100);
  const free = Math.max(0, win - d.used);
  const bar = d.categories.map((c) => `<span style="width:${pct(c.tokens).toFixed(2)}%;background:${CTX_COLORS[c.name] || '#999'}" title="${esc(t('ctx_' + c.name))}: ${fmtTokens(c.tokens)}"></span>`).join('');
  const body = h(`<div class="ctx-view">
    <div class="ctx-total"><b>${fmtTokens(d.used)}</b> / ${fmtTokens(win)} ${esc(t('ctx_tokens'))} · ${pct(d.used).toFixed(1)}%${d.measured ? '' : ` · <span class="muted">${esc(t('ctx_estimated'))}</span>`}</div>
    <div class="ctx-bar">${bar}</div>
    <table class="kv">${d.categories.map((c) => `<tr><td><i class="ctx-dot" style="background:${CTX_COLORS[c.name] || '#999'}"></i>${esc(t('ctx_' + c.name))}</td><td>${fmtTokens(c.tokens)} · ${pct(c.tokens).toFixed(1)}%</td></tr>`).join('')}
      <tr><td><i class="ctx-dot" style="background:var(--border)"></i>${esc(t('ctx_free'))}</td><td>${fmtTokens(free)} · ${pct(free).toFixed(1)}%</td></tr></table>
    <p class="muted small">${esc(d.auto_compact_pct ? t('ctx_autocompact', { n: d.auto_compact_pct }) : t('ctx_no_autocompact'))}</p></div>`);
  modal({ title: t('ctx_title'), body });
}

registerCommand({ name: 'context', description: () => t('cmd_context'), engines: ['builtin'], run: (_a, ctx) => openContext(ctx) });
