// The Artifacts gallery's thumbnails: what each artifact looks like, drawn so
// nothing of it runs. A web page or deck is its own page in a frame with no
// scripts at all (sandbox=""), shrunk to the card; an SVG an image; a sheet
// its first cells worked out, a dashboard its first figures and chart, a doc
// its rendered start; code its text. React and Mermaid need their libraries
// to draw anything, which a thumbnail doesn't load: their type, title and
// first heading.
import { call, artifactUrl } from './api.js';
import { esc } from './util.js';
import { icon } from './icons.js';
import { parseCsv, evaluate, display, hasHeader, isErr } from './artifact_types/sheet.js';
import { parseDashboard, chartSvg, formatValue } from './artifact_types/dashboard.js';

const SLIDES = 'application/vnd.ant.slides', DOC = 'application/vnd.ant.document', SHEET = 'application/vnd.ant.sheet', DASHBOARD = 'application/vnd.ant.dashboard';

const svgUrl = (svg) => 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svg);

/** What a React component or diagram shows first: its first heading, else its first line. */
export function firstHeading(kind, content) {
  const s = String(content);
  if (kind === 'application/vnd.ant.react' || kind === 'text/html') {
    const m = s.match(/<h[1-3][^>]*>\s*([^<{]{2,80}?)\s*</i) || s.match(/<title>\s*([^<]{2,80}?)\s*<\/title>/i);
    return m ? m[1].trim() : '';
  }
  const line = s.split('\n').map((l) => l.trim()).find((l) => l && !l.startsWith('%%'));
  return (line || '').slice(0, 80);
}

/** A sheet's top left corner as `{head, rows}` of shown text (at most `r` rows, `c` columns, numbers in `lang`). */
export function sheetCorner(content, { r = 6, c = 5, lang = 'en' } = {}) {
  const rows = parseCsv(content).slice(0, 200);
  const values = evaluate(rows);
  const shown = rows.slice(0, r).map((row, i) => row.slice(0, c).map((raw, j) => ({ text: display(raw, values[i][j], lang), num: typeof values[i][j] === 'number', err: isErr(values[i][j]) })));
  return { head: hasHeader(rows), rows: shown };
}

async function frame(el, a, dark, w, h, contain) {
  const token = await call('serve_artifact', { kind: a.kind, content: a.content, dark });
  const f = document.createElement('iframe');
  f.setAttribute('sandbox', '');
  f.setAttribute('tabindex', '-1');
  f.setAttribute('aria-hidden', 'true');
  f.setAttribute('referrerpolicy', 'no-referrer');
  const fit = () => {
    const k = contain ? Math.min(el.clientWidth / w, el.clientHeight / h) : el.clientWidth / w;
    f.style.cssText = `position:absolute;left:${contain ? (el.clientWidth - w * k) / 2 : 0}px;top:${contain ? (el.clientHeight - h * k) / 2 : 0}px;width:${w}px;height:${h}px;border:0;transform-origin:0 0;transform:scale(${k});pointer-events:none;background:${a.kind === 'text/html' ? '#fff' : 'transparent'}`;
  };
  fit();
  f.src = artifactUrl(token);
  el.appendChild(f);
  new ResizeObserver(fit).observe(el);
}

function tile(el, a, iconName) {
  const head = firstHeading(a.kind, a.content);
  el.innerHTML = `<div style="height:100%;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:6px;padding:8px 14px;box-sizing:border-box;text-align:center;color:var(--text-2)">${icon(iconName, 22)}<div class="ellipsis" style="max-width:100%;font-size:12.5px;color:var(--text-1)">${esc(head || a.title || '')}</div></div>`;
}

/** Fill `el`, a card's preview box, with a picture of gallery item `a` ({kind, content, title}). */
export async function renderPreview(el, a, { dark, lang, iconName }) {
  el.innerHTML = '';
  switch (a.kind) {
    case 'image/svg+xml':
      el.innerHTML = `<img alt="" src="${esc(svgUrl(a.content))}" style="width:100%;height:100%;object-fit:contain;padding:8px;box-sizing:border-box">`;
      return;
    case 'text/html': return frame(el, a, dark, 1024, 680, false);
    case SLIDES: return frame(el, a, dark, 1920, 1080, true);
    case 'text/markdown':
    case DOC: {
      const html = await call('render_markdown', { text: a.content.slice(0, 2500) });
      el.innerHTML = `<div class="md" aria-hidden="true" style="width:200%;transform:scale(.5);transform-origin:0 0;padding:14px 22px;box-sizing:border-box;pointer-events:none">${html}</div>`;
      return;
    }
    case SHEET: {
      const { head, rows } = sheetCorner(a.content, { lang });
      const cell = (c, i) => `<td style="padding:2px 6px;border:1px solid var(--border);white-space:nowrap;overflow:hidden;max-width:90px;text-overflow:ellipsis;${c.num ? 'text-align:right;' : ''}${head && i === 0 ? 'font-weight:600;' : ''}${c.err ? 'color:var(--danger, #d0453a);' : ''}">${esc(c.text)}</td>`;
      el.innerHTML = `<table aria-hidden="true" style="border-collapse:collapse;font-size:11px;margin:8px;font-variant-numeric:tabular-nums">${rows.map((r, i) => `<tr>${r.map((c) => cell(c, i)).join('')}</tr>`).join('')}</table>`;
      return;
    }
    case DASHBOARD: {
      const d = parseDashboard(a.content);
      if (d.error) return tile(el, a, iconName);
      const kpis = d.kpis.slice(0, 3).map((k) => `<div style="flex:1;min-width:0;border:1px solid var(--border);border-radius:6px;padding:4px 6px"><div class="ellipsis" style="font-size:10px;color:var(--text-2)">${esc(k.label)}</div><div class="ellipsis" style="font-size:13px;font-weight:600">${esc(formatValue(k.value, k.format, { lang, currency: k.currency, compact: true }))}</div></div>`).join('');
      const chart = d.charts[0] ? `<img alt="" style="width:100%;flex:1;min-height:0;object-fit:contain" src="${esc(svgUrl(chartSvg(d.charts[0], { width: 360, height: 150, dark, lang })))}">` : '';
      el.innerHTML = `<div aria-hidden="true" style="height:100%;display:flex;flex-direction:column;gap:4px;padding:6px 8px;box-sizing:border-box">${kpis ? `<div style="display:flex;gap:4px">${kpis}</div>` : ''}${chart}</div>`;
      return;
    }
    case 'application/vnd.ant.code':
      el.innerHTML = `<pre>${esc(a.content.slice(0, 600))}</pre>`;
      return;
    default: tile(el, a, iconName);
  }
}
