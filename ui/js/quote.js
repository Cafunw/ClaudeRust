// Selecting text in one of Claude's answers shows a small bar: quote it in
// your reply, or ask Claude to explain it.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { h, esc } from './util.js';

let bar = null;

function hide() {
  if (bar) bar.remove();
  bar = null;
}

/** The selection, when it lies inside the text of a single answer. */
function selectedAnswerText() {
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || !sel.rangeCount) return null;
  const text = String(sel).trim();
  if (!text || text.length > 4000) return null;
  const range = sel.getRangeAt(0);
  const node = (n) => (n && n.nodeType === 1 ? n : n && n.parentElement);
  const a = node(range.startContainer);
  const b = node(range.endContainer);
  const turnA = a && a.closest('.chat-scroll .turn.assistant');
  if (!turnA || turnA !== (b && b.closest('.chat-scroll .turn.assistant'))) return null;
  if (!a.closest('.part-text') && !b.closest('.part-text')) return null;
  return { text, rect: range.getBoundingClientRect() };
}

/** `text` as a Markdown quote. */
export function asQuote(text) {
  return String(text).trim().split('\n').map((l) => `> ${l}`.trimEnd()).join('\n');
}

async function composer() {
  const chat = await import('./chat.js');
  const c = chat.current();
  return c && c.composer;
}

function show(sel) {
  hide();
  bar = h(`<div class="quote-bar"><button class="btn sm ghost" data-a="quote">${icon('chat', 13)} ${esc(t('quote_reply'))}</button><button class="btn sm ghost" data-a="explain">${icon('help', 13)} ${esc(t('quote_explain'))}</button></div>`);
  document.body.appendChild(bar);
  const r = sel.rect;
  const w = bar.offsetWidth;
  const left = Math.max(8, Math.min(window.innerWidth - w - 8, r.left + r.width / 2 - w / 2));
  const top = r.top > 50 ? r.top - bar.offsetHeight - 6 : r.bottom + 6;
  bar.style.left = `${left}px`;
  bar.style.top = `${top}px`;
  const text = sel.text;
  bar.querySelector('[data-a=quote]').onmousedown = async (e) => {
    e.preventDefault();
    hide();
    const c = await composer();
    if (!c) return;
    const cur = c.getText();
    c.setText(`${asQuote(text)}\n\n${cur}`);
    c.focus();
  };
  bar.querySelector('[data-a=explain]').onmousedown = async (e) => {
    e.preventDefault();
    hide();
    const c = await composer();
    if (!c) return;
    const draft = c.getText().trim();
    c.setText(`${asQuote(text)}\n\n${draft || t('quote_explain_p')}`);
    c.send();
  };
}

document.addEventListener('mouseup', (e) => {
  if (bar && bar.contains(e.target)) return;
  // let the selection settle first
  setTimeout(() => {
    const sel = selectedAnswerText();
    if (sel) show(sel); else hide();
  }, 10);
});
document.addEventListener('mousedown', (e) => { if (bar && !bar.contains(e.target)) hide(); });
document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && bar) { e.preventDefault(); hide(); } });
document.addEventListener('scroll', hide, true);
