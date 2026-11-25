// Find in conversation (Ctrl/Cmd+F): highlights matches in the open chat
// and steps through them.
import { t } from './i18n.js';
import { icon } from './icons.js';
import * as keymap from './keymap.js';
import { h, esc } from './util.js';
import { drawAll, releaseAll } from './chat.js';

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

let seq = 0;
async function search(q) {
  const mine = ++seq;
  clearMarks();
  const col = document.querySelector('.chat-scroll .chat-col');
  if (!col || !q) return update();
  // a long conversation draws its older turns only when asked, and parks
  // those far off screen: all of them are drawn while the bar is open
  await drawAll();
  if (mine !== seq || !col.isConnected) return;
  clearMarks();
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
  // match on the original text (case-insensitively), so offsets stay right
  // even where lower-casing changes a string's length (e.g. "İ")
  const re = new RegExp(q.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'giu');
  for (const node of nodes) {
    if (hits.length >= 2000) break;
    const text = node.nodeValue;
    const frag = document.createDocumentFragment();
    let last = 0;
    for (const m of text.matchAll(re)) {
      if (!m[0]) continue;
      if (m.index > last) frag.appendChild(document.createTextNode(text.slice(last, m.index)));
      const mk = document.createElement('mark');
      mk.className = 'find-hit';
      mk.textContent = m[0];
      frag.appendChild(mk);
      hits.push(mk);
      last = m.index + m[0].length;
      if (hits.length >= 2000) break;
    }
    if (last === 0) continue;
    if (last < text.length) frag.appendChild(document.createTextNode(text.slice(last)));
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
  releaseAll();
}

export function openFind() {
  const view = document.querySelector('.chat-scroll');
  if (!view) return;
  if (bar && !bar.isConnected) { bar = null; hits = []; idx = -1; }
  if (!bar) {
    const btn = (a, ic, label) => `<button class="icon-btn sm" data-a="${a}" title="${esc(label)}" aria-label="${esc(label)}">${icon(ic, 14)}</button>`;
    bar = h(`<div class="find-bar" role="search">${icon('search', 14)}<input placeholder="${esc(t('find_in_chat'))}" aria-label="${esc(t('find_in_chat'))}"><span class="fb-count muted small" role="status"></span>
      ${btn('prev', 'chevUp', t('find_prev'))}${btn('next', 'chevDown', t('find_next'))}${btn('close', 'x', t('close'))}</div>`);
    view.parentNode.insertBefore(bar, view);
    const input = bar.querySelector('input');
    let tm;
    input.addEventListener('input', () => { clearTimeout(tm); tm = setTimeout(() => search(input.value.trim()), 150); });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); step(e.shiftKey ? -1 : 1); }
    });
    // from its field or one of its buttons
    bar.addEventListener('keydown', (e) => { if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); closeFind(); } });
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
  if (keymap.matches(e, 'find') && document.querySelector('.chat-scroll')) { e.preventDefault(); openFind(); }
});
