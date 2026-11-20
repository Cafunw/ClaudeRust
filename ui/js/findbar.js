// Find in conversation (Ctrl/Cmd+F): highlights matches in the open chat
// and steps through them.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { isMac } from './api.js';
import { h, esc } from './util.js';

let bar = null;
let hits = [];
let idx = -1;

function clearMarks() {
  for (const m of document.querySelectorAll('mark.find-hit')) {
    const p = m.parentNode;
    p.replaceChild(document.createTextNode(m.textContent), m);
    p.normalize();
  }
  hits = [];
  idx = -1;
}

function search(q) {
  clearMarks();
  const col = document.querySelector('.chat-scroll .chat-col');
  if (!col || !q) return update();
  const ql = q.toLowerCase();
  const walker = document.createTreeWalker(col, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => {
      const p = n.parentElement;
      if (!p || p.closest('.turn-actions, button, .katex, script, style') || !n.nodeValue.toLowerCase().includes(ql)) return NodeFilter.FILTER_REJECT;
      // skip text inside collapsed blocks: a hit there could not be shown
      return p.getClientRects().length ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT;
    },
  });
  const nodes = [];
  while (walker.nextNode()) nodes.push(walker.currentNode);
  for (const node of nodes) {
    let text = node.nodeValue;
    const frag = document.createDocumentFragment();
    let i;
    while ((i = text.toLowerCase().indexOf(ql)) >= 0) {
      if (i > 0) frag.appendChild(document.createTextNode(text.slice(0, i)));
      const m = document.createElement('mark');
      m.className = 'find-hit';
      m.textContent = text.slice(i, i + q.length);
      frag.appendChild(m);
      hits.push(m);
      text = text.slice(i + q.length);
      if (hits.length > 2000) break;
    }
    if (text) frag.appendChild(document.createTextNode(text));
    node.parentNode.replaceChild(frag, node);
  }
  idx = hits.length ? 0 : -1;
  update();
}

function update() {
  hits.forEach((m, i) => m.classList.toggle('cur', i === idx));
  if (bar) bar.querySelector('.fb-count').textContent = hits.length ? `${idx + 1} / ${hits.length}` : (bar.querySelector('input').value ? t('no_results') : '');
  if (idx >= 0) hits[idx].scrollIntoView({ block: 'center' });
}

function step(d) {
  if (!hits.length) return;
  idx = (idx + d + hits.length) % hits.length;
  update();
}

export function closeFind() {
  clearMarks();
  if (bar) bar.remove();
  bar = null;
}

export function openFind() {
  const view = document.querySelector('.chat-scroll');
  if (!view) return;
  if (bar && !bar.isConnected) { bar = null; hits = []; idx = -1; }
  if (!bar) {
    bar = h(`<div class="find-bar">${icon('search', 14)}<input placeholder="${esc(t('find_in_chat'))}"><span class="fb-count muted small"></span>
      <button class="icon-btn sm" data-a="prev">${icon('chevUp', 14)}</button><button class="icon-btn sm" data-a="next">${icon('chevDown', 14)}</button><button class="icon-btn sm" data-a="close">${icon('x', 14)}</button></div>`);
    view.parentNode.insertBefore(bar, view);
    const input = bar.querySelector('input');
    let tm;
    input.addEventListener('input', () => { clearTimeout(tm); tm = setTimeout(() => search(input.value.trim()), 150); });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); step(e.shiftKey ? -1 : 1); }
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); closeFind(); }
    });
    bar.querySelector('[data-a=prev]').onclick = () => step(-1);
    bar.querySelector('[data-a=next]').onclick = () => step(1);
    bar.querySelector('[data-a=close]').onclick = () => closeFind();
  }
  const input = bar.querySelector('input');
  const sel = String(window.getSelection() || '').trim();
  if (sel && sel.length < 100) { input.value = sel; search(sel); }
  input.focus();
  input.select();
}

window.addEventListener('keydown', (e) => {
  const mod = isMac ? e.metaKey : e.ctrlKey;
  if (mod && !e.shiftKey && e.key.toLowerCase() === 'f' && document.querySelector('.chat-scroll')) { e.preventDefault(); openFind(); }
});
