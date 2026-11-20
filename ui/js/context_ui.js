// Context window meter (topbar), /compact, and the "earlier conversation
// summarized" divider.
import { t } from './i18n.js';
import { call, on } from './api.js';
import { h, esc, toast, errorToast, fmtTokens } from './util.js';
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
