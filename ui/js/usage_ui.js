// Usage dashboard: tokens and cost across all conversations.
import { t } from './i18n.js';
import { call } from './api.js';
import { emit } from './store.js';
import { h, esc, modal, fmtTokens, fmtUsd as money, errorToast } from './util.js';
import { registerCommand } from './slash.js';

function chart(byDay, days) {
  const map = new Map(byDay.map((d) => [d.date, d]));
  const today = new Date();
  const pts = [];
  for (let i = days - 1; i >= 0; i--) {
    const d = new Date(today.getFullYear(), today.getMonth(), today.getDate() - i);
    const key = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
    pts.push({ key, label: d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' }), v: (map.get(key) || {}).cost_usd || 0 });
  }
  const max = Math.max(0.01, ...pts.map((p) => p.v));
  const W = 640, H = 150, bw = W / pts.length;
  const bars = pts.map((p, i) => {
    const bh = Math.max(p.v > 0 ? 2 : 0, (p.v / max) * (H - 20));
    return `<rect x="${(i * bw + 1).toFixed(1)}" y="${(H - bh).toFixed(1)}" width="${Math.max(1, bw - 2).toFixed(1)}" height="${bh.toFixed(1)}" rx="2"><title>${esc(p.label)}: ${money(p.v)}</title></rect>`;
  }).join('');
  return `<svg class="usage-chart" viewBox="0 0 ${W} ${H + 18}" preserveAspectRatio="none">${bars}
    <text x="0" y="${H + 14}">${esc(pts[0].label)}</text><text x="${W}" y="${H + 14}" text-anchor="end">${esc(pts[pts.length - 1].label)}</text>
    <text x="${W}" y="10" text-anchor="end">${money(max)}</text></svg>`;
}

export async function openUsage(days = 30) {
  const m = modal({ title: t('usage_title'), body: '<div class="pane-empty"><span class="spinner"></span></div>', wide: true });
  const draw = async (d) => {
    let u;
    try { u = await call('usage_overview', { days: d }); } catch (e) { errorToast(e); m.close(); return; }
    const tot = u.totals;
    m.body.innerHTML = '';
    const body = h(`<div class="usage-dash">
      <div class="seg-row"><div class="segmented">${[7, 30, 90].map((x) => `<button data-d="${x}" class="${x === d ? 'on' : ''}">${t('usage_days', { n: x })}</button>`).join('')}</div></div>
      <div class="usage-kpis">
        <div><b>${money(tot.cost_usd)}</b><span>${t('cost')}</span></div>
        <div><b>${fmtTokens(tot.input_tokens)}</b><span>${t('input_tokens')}</span></div>
        <div><b>${fmtTokens(tot.output_tokens)}</b><span>${t('output_tokens')}</span></div>
        <div><b>${tot.responses}</b><span>${t('usage_responses')}</span></div>
      </div>
      ${chart(u.by_day, d)}
      <div class="usage-cols">
        <div><div class="section-title">${t('usage_by_model')}</div><table class="kv">${u.by_model.map((x) => `<tr><td>${esc(x.name || x.model)}<div class="muted small">${fmtTokens(x.input_tokens)} in · ${fmtTokens(x.output_tokens)} out · ${x.responses}×</div></td><td>${money(x.cost_usd)}</td></tr>`).join('') || `<tr><td class="muted">—</td><td></td></tr>`}</table></div>
        <div><div class="section-title">${t('usage_top_convs')}</div><table class="kv">${u.top_conversations.map((x) => `<tr class="link" data-id="${esc(x.id)}"><td>${esc(x.title || t('untitled'))}<div class="muted small">${esc(x.kind)}</div></td><td>${money(x.cost_usd)}</td></tr>`).join('') || `<tr><td class="muted">—</td><td></td></tr>`}</table></div>
      </div>
      <p class="muted small">${esc(t('usage_note'))}</p></div>`);
    body.querySelectorAll('[data-d]').forEach((b) => (b.onclick = () => draw(+b.dataset.d)));
    body.querySelectorAll('tr.link').forEach((r) => (r.onclick = () => { m.close(); emit('navigate', { name: 'chat', params: { id: r.dataset.id } }); }));
    m.body.appendChild(body);
  };
  draw(days);
}

registerCommand({ name: 'usage', description: () => t('cmd_usage'), run: () => openUsage() });
