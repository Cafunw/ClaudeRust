// Map pane: the project's files and the imports between them, laid out
// with a small force simulation. Hover to see a file's links, click to
// focus it, double-click to mention it in the prompt.
import { t } from '../i18n.js';
import { icon } from '../icons.js';
import { call } from '../api.js';
import { h, esc } from '../util.js';
import { registerPane } from '../workspace.js';
import { errorText } from '../neterr.js';

const NS = 'http://www.w3.org/2000/svg';
const PALETTE = ['#c96442', '#4a7bd0', '#3a9a5b', '#b07a10', '#8e5cc2', '#c0453a', '#2f8a8a', '#7a6a4f', '#d0699a', '#5a6b8c'];

function layout(nodes, edges, w, hgt) {
  const idx = new Map(nodes.map((n, i) => [n.id, i]));
  const groups = [...new Set(nodes.map((n) => n.group))];
  // start grouped by top-level folder on a circle
  nodes.forEach((n, i) => {
    const g = groups.indexOf(n.group);
    const a = (g / Math.max(1, groups.length)) * Math.PI * 2;
    n.x = w / 2 + Math.cos(a) * w * 0.28 + ((i * 37) % 60) - 30;
    n.y = hgt / 2 + Math.sin(a) * hgt * 0.28 + ((i * 53) % 60) - 30;
    n.vx = 0; n.vy = 0;
  });
  const links = edges.map((e) => [idx.get(e.from), idx.get(e.to)]).filter(([a, b]) => a !== undefined && b !== undefined);
  const k = Math.sqrt((w * hgt) / Math.max(1, nodes.length)) * 0.95;
  const iters = nodes.length > 300 ? 120 : 260;
  for (let it = 0; it < iters; it++) {
    const temp = 1 - it / iters;
    for (let i = 0; i < nodes.length; i++) {
      const a = nodes[i];
      for (let j = i + 1; j < nodes.length; j++) {
        const b = nodes[j];
        let dx = a.x - b.x, dy = a.y - b.y;
        let d2 = dx * dx + dy * dy || 0.01;
        if (d2 > 90000) continue;
        const f = (k * k) / d2;
        a.vx += dx * f * 0.02; a.vy += dy * f * 0.02;
        b.vx -= dx * f * 0.02; b.vy -= dy * f * 0.02;
      }
    }
    for (const [i, j] of links) {
      const a = nodes[i], b = nodes[j];
      const dx = b.x - a.x, dy = b.y - a.y;
      const d = Math.sqrt(dx * dx + dy * dy) || 1;
      const f = ((d - k) / d) * 0.06;
      a.vx += dx * f; a.vy += dy * f; b.vx -= dx * f; b.vy -= dy * f;
    }
    for (const n of nodes) {
      n.vx += (w / 2 - n.x) * 0.002; n.vy += (hgt / 2 - n.y) * 0.002;
      n.x += Math.max(-20, Math.min(20, n.vx)) * temp;
      n.y += Math.max(-20, Math.min(20, n.vy)) * temp;
      n.vx *= 0.6; n.vy *= 0.6;
    }
  }
  return links;
}

function el(tag, attrs) {
  const e = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs || {})) e.setAttribute(k, v);
  return e;
}

async function draw(pane, ctx) {
  pane.innerHTML = `<div class="pv-bar"><input class="pv-url" placeholder="${esc(t('map_filter'))}" aria-label="${esc(t('map_filter'))}"><button class="icon-btn sm" data-a="reload" title="${t('refresh')}">${icon('refreshCw', 14)}</button></div><div class="map-body"><div class="pane-empty"><span class="spinner"></span></div></div><div class="map-info muted small"></div>`;
  const body = pane.querySelector('.map-body');
  const info = pane.querySelector('.map-info');
  pane.querySelector('[data-a=reload]').onclick = () => draw(pane, ctx);
  let g;
  try { g = await call('arch_map', { conversationId: ctx.convId }); } catch (e) { body.innerHTML = `<div class="pane-empty">${esc(errorText(e))}</div>`; return; }
  if (!g.nodes.length) { body.innerHTML = `<div class="pane-empty">${icon('layers', 22)}<div>${esc(t('map_empty'))}</div></div>`; return; }
  const W = 1000, H = 800;
  const nodes = g.nodes.map((n) => ({ ...n }));
  const links = layout(nodes, g.edges, W, H);
  const groups = [...new Set(nodes.map((n) => n.group))];
  const color = (n) => PALETTE[groups.indexOf(n.group) % PALETTE.length];
  // fit the view to the laid-out graph (ignoring far outliers)
  const linked = new Set(links.flat());
  const fitNodes = linked.size >= 3 ? nodes.filter((_, i) => linked.has(i)) : nodes;
  const xs = fitNodes.map((n) => n.x).sort((a, b) => a - b), ys = fitNodes.map((n) => n.y).sort((a, b) => a - b);
  const q = (arr, f) => arr[Math.min(arr.length - 1, Math.max(0, Math.floor(arr.length * f)))];
  const vb = { x: q(xs, 0.02) - 60, y: q(ys, 0.02) - 40, w: Math.max(200, q(xs, 0.98) - q(xs, 0.02) + 160), h: Math.max(160, q(ys, 0.98) - q(ys, 0.02) + 80) };
  const svg = el('svg', { viewBox: `${vb.x} ${vb.y} ${vb.w} ${vb.h}`, class: 'map-svg', preserveAspectRatio: 'xMidYMid meet', role: 'img', 'aria-label': t('ws_map') });
  const view = el('g');
  svg.appendChild(view);
  const lineEls = links.map(([i, j]) => {
    const l = el('line', { x1: nodes[i].x, y1: nodes[i].y, x2: nodes[j].x, y2: nodes[j].y, class: 'map-edge' });
    view.appendChild(l);
    return l;
  });
  const nbrs = nodes.map(() => new Set());
  links.forEach(([i, j], li) => { nbrs[i].add(li); nbrs[j].add(li); });
  const nodeEls = nodes.map((n, i) => {
    const r = Math.max(3.5, Math.min(14, Math.sqrt(n.loc || 1) / 2.2));
    const gEl = el('g', { class: 'map-node', transform: `translate(${n.x},${n.y})` });
    const c = el('circle', { r, fill: color(n) });
    const label = el('text', { x: r + 3, y: 4, class: 'map-label' });
    label.textContent = n.id.split('/').pop();
    gEl.append(c, label);
    gEl.addEventListener('mouseenter', () => focus(i, false));
    gEl.addEventListener('mouseleave', () => focus(sel, false));
    gEl.addEventListener('click', (e) => { e.stopPropagation(); sel = sel === i ? -1 : i; focus(sel, true); });
    gEl.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      call('open_in_editor', { path: `${g.root}/${n.id}`, line: null }).catch(() => {});
    });
    gEl.addEventListener('dblclick', async () => {
      const chat = await import('../chat.js');
      const c2 = chat.current();
      if (c2) c2.composer.insertText(`@${n.id} `);
    });
    view.appendChild(gEl);
    return gEl;
  });
  let sel = -1;
  function focus(i, pin) {
    svg.classList.toggle('focusing', i >= 0);
    nodeEls.forEach((e) => e.classList.remove('hot', 'near'));
    lineEls.forEach((e) => e.classList.remove('hot'));
    if (i < 0) { info.textContent = t('map_help', { n: nodes.length, e: links.length }); return; }
    nodeEls[i].classList.add('hot');
    let inn = 0, out = 0;
    for (const li of nbrs[i]) {
      lineEls[li].classList.add('hot');
      const [a, b] = links[li];
      nodeEls[a === i ? b : a].classList.add('near');
      if (a === i) out++; else inn++;
    }
    info.textContent = `${nodes[i].id} · ${nodes[i].loc} ${t('map_lines')} · ${t('map_imports', { n: out })} · ${t('map_imported_by', { n: inn })}${pin ? '' : ''}`;
  }
  svg.addEventListener('click', () => { sel = -1; focus(-1); });
  // pan & zoom
  let tx = 0, ty = 0, sc = 1, drag = null;
  const apply = () => view.setAttribute('transform', `translate(${tx},${ty}) scale(${sc})`);
  svg.addEventListener('wheel', (e) => {
    e.preventDefault();
    const pt = svg.getBoundingClientRect();
    const mx = vb.x + ((e.clientX - pt.left) / pt.width) * vb.w, my = vb.y + ((e.clientY - pt.top) / pt.height) * vb.h;
    const f = e.deltaY < 0 ? 1.15 : 1 / 1.15;
    tx = mx - (mx - tx) * f; ty = my - (my - ty) * f; sc *= f;
    apply();
  }, { passive: false });
  // window listeners only while dragging: added on every draw and never
  // removed, they kept each old map (and its SVG) alive
  const onMove = (e) => {
    if (!drag) return;
    const pt = svg.getBoundingClientRect();
    tx = drag.tx + ((e.clientX - drag.x) / pt.width) * vb.w;
    ty = drag.ty + ((e.clientY - drag.y) / pt.height) * vb.h;
    apply();
  };
  const onUp = () => {
    drag = null;
    window.removeEventListener('mousemove', onMove);
    window.removeEventListener('mouseup', onUp);
  };
  svg.addEventListener('mousedown', (e) => {
    drag = { x: e.clientX, y: e.clientY, tx, ty };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  });
  // filter
  pane.querySelector('.pv-url').addEventListener('input', (e) => {
    const q = e.target.value.trim().toLowerCase();
    nodeEls.forEach((ne, i) => ne.classList.toggle('dim', !!q && !nodes[i].id.toLowerCase().includes(q)));
  });
  body.innerHTML = '';
  body.appendChild(svg);
  const legend = h(`<div class="map-legend">${groups.slice(0, 10).map((gr) => `<span><i style="background:${PALETTE[groups.indexOf(gr) % PALETTE.length]}"></i>${esc(gr)}</span>`).join('')}</div>`);
  body.appendChild(legend);
  focus(-1);
  if (g.truncated) info.textContent += ` · ${t('map_truncated')}`;
}

registerPane({
  id: 'map',
  order: 60,
  icon: 'layers',
  label: () => t('ws_map'),
  mount: (el, ctx) => { el.classList.add('map-pane'); draw(el, ctx); },
});
