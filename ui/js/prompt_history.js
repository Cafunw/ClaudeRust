// Ctrl/Cmd+R in the message box: search the messages you've sent before (in
// any conversation) and put one back in the box.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { call } from './api.js';
import { h, esc, relTime, debounce } from './util.js';

let pop = null;

function close() {
  if (pop) { pop.remove(); pop = null; }
}

/** Open the history search above `anchor` (the composer element). */
export function openPromptHistory(composer, anchor) {
  if (pop) { close(); return; }
  pop = h(`<div class="ph-pop" role="dialog"><div class="ph-search">${icon('history', 14)}<input placeholder="${esc(t('ph_placeholder'))}" spellcheck="false"><kbd>Esc</kbd></div><div class="ph-list"></div></div>`);
  document.body.appendChild(pop);
  const r = anchor.getBoundingClientRect();
  pop.style.left = `${r.left}px`;
  pop.style.width = `${r.width}px`;
  pop.style.bottom = `${window.innerHeight - r.top + 8}px`;
  const input = pop.querySelector('input');
  const list = pop.querySelector('.ph-list');
  let items = [];
  let sel = 0;
  let seq = 0;
  const pick = (i) => {
    const it = items[i];
    if (!it) return;
    close();
    composer.setText(it.text);
    composer.focus();
  };
  const draw = () => {
    if (!items.length) { list.innerHTML = `<div class="ph-empty">${esc(input.value.trim() ? t('no_results') : t('ph_empty'))}</div>`; return; }
    list.innerHTML = items.map((it, i) => `<button class="ph-item ${i === sel ? 'on' : ''}" data-i="${i}"><div class="ph-text">${esc(it.text.length > 400 ? `${it.text.slice(0, 400)}…` : it.text)}</div><div class="ph-meta">${esc(it.title || '')} · ${esc(relTime(it.at))}</div></button>`).join('');
    list.querySelectorAll('.ph-item').forEach((b) => { b.onmousedown = (e) => { e.preventDefault(); pick(Number(b.dataset.i)); }; });
    const on = list.querySelector('.ph-item.on');
    if (on) on.scrollIntoView({ block: 'nearest' });
  };
  const load = async () => {
    const my = ++seq;
    let r = [];
    try { r = await call('prompt_history', { query: input.value.trim() }); } catch { r = []; }
    if (my !== seq || !pop) return;
    items = r;
    sel = 0;
    draw();
  };
  const run = debounce(load, 120);
  input.addEventListener('input', run);
  input.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); if (items.length) { sel = (sel + 1) % items.length; draw(); } }
    else if (e.key === 'ArrowUp') { e.preventDefault(); if (items.length) { sel = (sel - 1 + items.length) % items.length; draw(); } }
    else if (e.key === 'Enter') { e.preventDefault(); pick(sel); }
    else if (e.key === 'Escape' || ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'r')) { e.preventDefault(); e.stopPropagation(); close(); composer.focus(); }
  });
  input.addEventListener('blur', () => setTimeout(() => { if (pop && !pop.contains(document.activeElement)) close(); }, 150));
  // start from what's typed in the box
  input.value = (composer.getText() || '').trim().slice(0, 80);
  input.focus();
  input.select();
  load();
}
