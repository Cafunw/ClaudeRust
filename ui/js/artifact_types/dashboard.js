// Dashboards (application/vnd.ant.dashboard): a JSON data block drawn as KPI
// tiles, charts (line, area, bar, pie, donut: SVG drawn here, no library) and
// tables. Runs in the artifact's frame; the gallery draws its thumbnail with
// the same functions, so they are pure and escape every text they write.

const MAX_ITEMS = 24;
const MAX_SERIES = 12;
const MAX_POINTS = 2000;
const MAX_ROWS = 1000;

const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const str = (v, max = 200) => (v === undefined || v === null ? '' : String(v).slice(0, max));
const numOr = (v) => { const n = typeof v === 'number' ? v : typeof v === 'string' && v.trim() !== '' ? Number(v.replace(/[,\s]/g, '')) : NaN; return Number.isFinite(n) ? n : null; };
const FORMATS = ['number', 'currency', 'percent'];
const fmtOf = (v) => (FORMATS.includes(v) ? v : 'number');

/** The dashboard in `text`, its shapes checked and cut to size: `{title, subtitle,
 *  kpis, charts, tables}`, or `{error}` when it isn't a JSON object. */
export function parseDashboard(text) {
  let d;
  try { d = JSON.parse(text); } catch (e) { return { error: `The dashboard isn't valid JSON: ${e.message}` }; }
  if (!d || typeof d !== 'object' || Array.isArray(d)) return { error: 'The dashboard must be a JSON object with kpis, charts or tables.' };
  const list = (v) => (Array.isArray(v) ? v.slice(0, MAX_ITEMS) : []);
  const kpis = list(d.kpis).filter((k) => k && typeof k === 'object').map((k) => ({
    label: str(k.label, 80), value: numOr(k.value) ?? str(k.value, 40), format: fmtOf(k.format), currency: /^[A-Z]{3}$/.test(k.currency) ? k.currency : 'USD',
    delta: numOr(k.delta), deltaLabel: str(k.deltaLabel, 60),
  }));
  const charts = list(d.charts).filter((c) => c && typeof c === 'object').map((c) => {
    const type = ['line', 'area', 'bar', 'pie', 'donut'].includes(c.type) ? c.type : 'bar';
    const x = (Array.isArray(c.x) ? c.x : Array.isArray(c.labels) ? c.labels : []).slice(0, MAX_POINTS).map((v) => str(v, 40));
    const series = (Array.isArray(c.series) ? c.series : []).slice(0, MAX_SERIES).filter((s) => s && typeof s === 'object').map((s, i) => ({
      name: str(s.name, 60) || `#${i + 1}`, data: (Array.isArray(s.data) ? s.data : []).slice(0, MAX_POINTS).map(numOr),
    }));
    return { type, title: str(c.title, 120), x, series, format: fmtOf(c.format), currency: /^[A-Z]{3}$/.test(c.currency) ? c.currency : 'USD', stacked: !!c.stacked };
  });
  const tables = list(d.tables).filter((t) => t && typeof t === 'object').map((t) => ({
    title: str(t.title, 120),
    columns: (Array.isArray(t.columns) ? t.columns : []).slice(0, 40).map((v) => str(v, 60)),
    rows: (Array.isArray(t.rows) ? t.rows : []).slice(0, MAX_ROWS).map((r) => (Array.isArray(r) ? r.slice(0, 40) : [])),
  }));
  return { title: str(d.title, 160), subtitle: str(d.subtitle, 240), kpis, charts, tables };
}

/** `v` as text in `format` (number, currency, percent: 0.12 is 12%); `compact` for big figures. */
export function formatValue(v, format = 'number', { lang = 'en', currency = 'USD', compact = false } = {}) {
  if (typeof v !== 'number') return String(v ?? '');
  const loc = lang === 'tr' ? 'tr-TR' : 'en-US';
  const big = compact && Math.abs(v) >= 10000;
  const opts = format === 'percent' ? { style: 'percent', maximumFractionDigits: 1 }
    : format === 'currency' ? { style: 'currency', currency, maximumFractionDigits: big ? 1 : 2 }
    : { maximumFractionDigits: big ? 1 : 2 };
  if (big) opts.notation = 'compact';
  try { return new Intl.NumberFormat(loc, opts).format(v); } catch { return String(v); }
}

/** Round axis steps covering `lo`..`hi` in about `n` ticks. */
export function niceTicks(lo, hi, n = 5) {
  if (!(hi > lo)) { const d = Math.abs(hi) || 1; lo = Math.min(lo, 0); hi = hi + d; }
  const raw = (hi - lo) / n;
  const p = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * p).find((s) => s >= raw) || 10 * p;
  const start = Math.floor(lo / step) * step;
  const out = [];
  for (let v = start; v <= hi + step * 1e-9 && out.length < 50; v += step) out.push(Math.round(v / step) * step);
  if (out[out.length - 1] < hi) out.push(out[out.length - 1] + step);
  return out;
}

export const PALETTE = { light: ['#2f6fdb', '#e07b39', '#3c9d6a', '#b5498f', '#7f6bd6', '#c9a227', '#4aa3b5', '#8a8a85'], dark: ['#6f9ff0', '#f0995e', '#5cc28a', '#de74b6', '#a596f0', '#e0bf4c', '#6fc3d3', '#a8a8a2'] };

/** Chart `c` (parseDashboard's) as SVG markup `w` x `h`. */
export function chartSvg(c, { width: w = 480, height: h = 260, dark = false, lang = 'en' } = {}) {
  const colors = PALETTE[dark ? 'dark' : 'light'];
  const ink = dark ? '#b7b5ad' : '#5e5d59', rule = dark ? '#474641' : '#e3e1da';
  const font = 'font-family="system-ui,Segoe UI,sans-serif" font-size="11"';
  const fmt = (v) => formatValue(v, c.format, { lang, currency: c.currency, compact: true });
  const open = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${w} ${h}" role="img" aria-label="${esc(c.title || c.type)}">`;
  if (!c.series.length) return `${open}<text x="${w / 2}" y="${h / 2}" text-anchor="middle" fill="${ink}" ${font}>—</text></svg>`;
  const legend = c.series.length > 1 || c.type === 'pie' || c.type === 'donut';
  if (c.type === 'pie' || c.type === 'donut') {
    const s = c.series[0];
    const vals = s.data.map((v) => Math.max(0, v || 0));
    const total = vals.reduce((a, b) => a + b, 0) || 1;
    const r = Math.min(h - 24, w * 0.55) / 2, cx = r + 12, cy = h / 2;
    let a0 = -Math.PI / 2, out = open;
    vals.forEach((v, i) => {
      const a1 = a0 + (v / total) * Math.PI * 2;
      const large = a1 - a0 > Math.PI ? 1 : 0;
      const p = (a) => `${(cx + r * Math.cos(a)).toFixed(2)},${(cy + r * Math.sin(a)).toFixed(2)}`;
      const d = v >= total ? `M${cx - r},${cy}a${r},${r} 0 1,0 ${2 * r},0a${r},${r} 0 1,0 ${-2 * r},0` : `M${cx},${cy}L${p(a0)}A${r},${r} 0 ${large},1 ${p(a1)}Z`;
      out += `<path d="${d}" fill="${colors[i % colors.length]}" stroke="${dark ? '#262624' : '#fff'}" stroke-width="1.5"><title>${esc(c.x[i] || '')}: ${esc(fmt(v))} (${Math.round((v / total) * 100)}%)</title></path>`;
      a0 = a1;
    });
    if (c.type === 'donut') out += `<circle cx="${cx}" cy="${cy}" r="${(r * 0.58).toFixed(2)}" fill="${dark ? '#262624' : '#fff'}"/>`;
    const lx = cx + r + 24;
    vals.slice(0, 14).forEach((v, i) => {
      const y = 18 + i * 18;
      out += `<rect x="${lx}" y="${y - 9}" width="10" height="10" rx="2" fill="${colors[i % colors.length]}"/><text x="${lx + 16}" y="${y}" fill="${ink}" ${font}>${esc((c.x[i] || '').slice(0, 22))} · ${esc(fmt(v))}</text>`;
    });
    return out + '</svg>';
  }
  const L = 52, R = 12, T = legend ? 26 : 10, B = 28;
  const n = Math.max(c.x.length, ...c.series.map((s) => s.data.length));
  const stackedTotals = c.stacked ? Array.from({ length: n }, (_, i) => c.series.reduce((a, s) => a + Math.max(0, s.data[i] || 0), 0)) : [];
  const all = c.stacked ? stackedTotals : c.series.flatMap((s) => s.data.filter((v) => v !== null));
  const ticks = niceTicks(Math.min(0, ...all), Math.max(0, ...all));
  const lo = ticks[0], hi = ticks[ticks.length - 1];
  const y = (v) => T + (h - T - B) * (1 - (v - lo) / (hi - lo || 1));
  const band = (w - L - R) / Math.max(1, n);
  const xc = (i) => L + band * (i + 0.5);
  let out = open;
  for (const t of ticks) out += `<line x1="${L}" x2="${w - R}" y1="${y(t).toFixed(1)}" y2="${y(t).toFixed(1)}" stroke="${rule}"/><text x="${L - 6}" y="${(y(t) + 3.5).toFixed(1)}" text-anchor="end" fill="${ink}" ${font}>${esc(fmt(t))}</text>`;
  const every = Math.max(1, Math.ceil(n / Math.max(1, Math.floor((w - L - R) / 56))));
  for (let i = 0; i < n; i += every) out += `<text x="${xc(i).toFixed(1)}" y="${h - 9}" text-anchor="middle" fill="${ink}" ${font}>${esc((c.x[i] || '').slice(0, 12))}</text>`;
  if (c.type === 'bar') {
    const k = c.stacked ? 1 : c.series.length;
    const bw = Math.max(1, (band * 0.72) / k);
    const base = new Array(n).fill(0);
    c.series.forEach((s, si) => {
      s.data.forEach((v, i) => {
        if (v === null) return;
        const from = c.stacked ? base[i] : 0, to = from + (c.stacked ? Math.max(0, v) : v);
        if (c.stacked) base[i] = to;
        const x0 = c.stacked ? xc(i) - bw / 2 : xc(i) - (band * 0.72) / 2 + si * bw;
        const top = Math.min(y(from), y(to)), ht = Math.abs(y(to) - y(from));
        out += `<rect x="${x0.toFixed(1)}" y="${top.toFixed(1)}" width="${(bw - (k > 1 ? 1 : 0)).toFixed(1)}" height="${Math.max(0.5, ht).toFixed(1)}" rx="2" fill="${colors[si % colors.length]}"><title>${esc(s.name)} · ${esc(c.x[i] || '')}: ${esc(fmt(v))}</title></rect>`;
      });
    });
  } else {
    c.series.forEach((s, si) => {
      const pts = s.data.map((v, i) => (v === null ? null : [xc(i), y(v)]));
      const runs = [];
      let cur = [];
      for (const p of pts) { if (p) cur.push(p); else if (cur.length) { runs.push(cur); cur = []; } }
      if (cur.length) runs.push(cur);
      const col = colors[si % colors.length];
      for (const r of runs) {
        const d = r.map((p, i) => `${i ? 'L' : 'M'}${p[0].toFixed(1)},${p[1].toFixed(1)}`).join('');
        if (c.type === 'area') out += `<path d="${d}L${r[r.length - 1][0].toFixed(1)},${y(Math.max(lo, 0)).toFixed(1)}L${r[0][0].toFixed(1)},${y(Math.max(lo, 0)).toFixed(1)}Z" fill="${col}" fill-opacity=".18"/>`;
        out += `<path d="${d}" fill="none" stroke="${col}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>`;
      }
      if (n <= 60) s.data.forEach((v, i) => { if (v !== null) out += `<circle cx="${xc(i).toFixed(1)}" cy="${y(v).toFixed(1)}" r="3" fill="${col}"><title>${esc(s.name)} · ${esc(c.x[i] || '')}: ${esc(fmt(v))}</title></circle>`; });
    });
  }
  if (legend) {
    let x = L;
    c.series.forEach((s, i) => { out += `<rect x="${x}" y="6" width="10" height="10" rx="2" fill="${colors[i % colors.length]}"/><text x="${x + 14}" y="15" fill="${ink}" ${font}>${esc(s.name.slice(0, 24))}</text>`; x += 26 + Math.min(24, s.name.length) * 6.4; });
  }
  return out + '</svg>';
}

// ---------------------------------------------------------------- the frame

const STRINGS = { en: { bad: 'This dashboard couldn’t be drawn.' }, tr: { bad: 'Bu gösterge paneli çizilemedi.' } };

function boot(content) {
  const lang = /(^|[#&])lang=tr\b/.test(location.hash) ? 'tr' : 'en';
  const dark = document.body.classList.contains('dark');
  const d = parseDashboard(content);
  if (d.error) {
    document.body.insertAdjacentHTML('beforeend', '<div class="err-box"></div>');
    document.body.querySelector('.err-box').textContent = `${STRINGS[lang].bad} ${d.error}`;
    throw new Error(d.error);
  }
  const root = document.createElement('main');
  root.className = 'dash';
  let html = d.title ? `<h1>${esc(d.title)}</h1>` : '';
  if (d.subtitle) html += `<div class="sub">${esc(d.subtitle)}</div>`;
  if (d.kpis.length) {
    html += '<section class="kpis">' + d.kpis.map((k) => {
      const delta = k.delta === null ? '' : `<div class="d ${k.delta >= 0 ? 'up' : 'down'}">${k.delta >= 0 ? '▲' : '▼'} ${esc(formatValue(Math.abs(k.delta), 'percent', { lang }))}${k.deltaLabel ? ' ' + esc(k.deltaLabel) : ''}</div>`;
      return `<div class="kpi"><div class="l">${esc(k.label)}</div><div class="v">${esc(formatValue(k.value, k.format, { lang, currency: k.currency, compact: true }))}</div>${delta}</div>`;
    }).join('') + '</section>';
  }
  if (d.charts.length) html += '<section class="charts">' + d.charts.map((c, i) => `<figure class="chart" data-i="${i}" style="margin:0">${c.title ? `<h2>${esc(c.title)}</h2>` : ''}<div class="plot"></div></figure>`).join('') + '</section>';
  for (const t of d.tables) {
    const cell = (v) => { const n = numOr(v); return n === null ? `<td>${esc(str(v, 300))}</td>` : `<td class="num">${esc(typeof v === 'number' ? formatValue(v, 'number', { lang }) : str(v, 300))}</td>`; };
    html += `<section class="dtable">${t.title ? `<h2>${esc(t.title)}</h2>` : ''}<table><thead><tr>${t.columns.map((c) => `<th scope="col">${esc(c)}</th>`).join('')}</tr></thead><tbody>${t.rows.map((r) => `<tr>${r.map(cell).join('')}</tr>`).join('')}</tbody></table></section>`;
  }
  root.innerHTML = html;
  document.body.appendChild(root);
  const plots = () => root.querySelectorAll('.chart').forEach((f) => {
    const w = Math.max(240, Math.round(f.querySelector('.plot').clientWidth));
    f.querySelector('.plot').innerHTML = chartSvg(d.charts[Number(f.dataset.i)], { width: w, height: Math.round(Math.min(320, Math.max(200, w * 0.55))), dark, lang });
  });
  plots();
  let t = null;
  new ResizeObserver(() => { clearTimeout(t); t = setTimeout(plots, 80); }).observe(root);
}

// only in an artifact's own page (the gallery and the tests import the functions above)
const DATA = typeof document !== 'undefined' && typeof document.getElementById === 'function' && document.getElementById('art-data');
if (DATA && DATA.dataset.type === 'dashboard') boot(JSON.parse(DATA.textContent));
