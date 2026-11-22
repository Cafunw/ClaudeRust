// Inline review comments on the Changes pane's diff: comment on any line,
// then send all comments to Claude as one revision request.
import { t } from '../i18n.js';
import { icon } from '../icons.js';
import { h, esc } from '../util.js';
import { loadLocal, saveLocal } from '../store.js';
import { onDiffLine, onChangesHeader, onChangesInput } from './changes.js';
import { refresh } from '../workspace.js';

// The comments are read for every line of a diff that's drawn: kept here
// (this module is the only one that writes them) instead of parsing local
// storage thousands of times per draw.
let cache = null; // { all, byKey: Map<convId, Map<key, comment>> }
function comments() {
  if (!cache) cache = { all: loadLocal().reviewComments || {}, byKey: new Map() };
  return cache;
}
function load(convId) { return comments().all[convId] || []; }
/** The comment on a line, if any. */
function commentAt(convId, k) {
  const c = comments();
  if (!c.byKey.has(convId)) {
    const m = new Map();
    for (const x of load(convId)) if (!m.has(key(x))) m.set(key(x), x); // the first, as a search would find
    c.byKey.set(convId, m);
  }
  return c.byKey.get(convId).get(k);
}
function save(convId, list) {
  const all = loadLocal().reviewComments || {};
  if (list.length) all[convId] = list; else delete all[convId];
  saveLocal({ reviewComments: all });
  cache = null;
}

const key = (c) => `${c.path}|${c.side}|${c.line}`;

// a comment added or removed redraws the Changes pane
onChangesInput((ctx) => load(ctx.convId));

/** The prompt sent for a set of comments. */
export function reviewPrompt(list) {
  const lines = [t('review_prompt_intro'), ''];
  list.forEach((c, i) => {
    lines.push(`${i + 1}. \`${c.path}:${c.line}\`${c.side === 'old' ? ` ${t('review_removed_line')}` : ''} — ${c.body.trim()}`);
    if (c.code && c.code.trim()) lines.push(`   > ${c.code.trim().slice(0, 200)}`);
  });
  lines.push('', t('review_prompt_outro'));
  return lines.join('\n');
}

function editorRow(initial, onSave, onCancel) {
  const tr = h(`<tr class="dv-comment-edit"><td colspan="3"><div class="rc-edit">
    <textarea rows="2" placeholder="${esc(t('review_placeholder'))}" aria-label="${esc(t('review_placeholder'))}">${esc(initial || '')}</textarea>
    <div class="rc-actions"><button class="btn sm ghost" data-a="cancel">${t('cancel')}</button><button class="btn sm primary" data-a="save">${t('review_add')}</button></div>
  </div></td></tr>`);
  const ta = tr.querySelector('textarea');
  const save = () => { const v = ta.value.trim(); if (v) onSave(v); else onCancel(); };
  tr.querySelector('[data-a=save]').onclick = save;
  tr.querySelector('[data-a=cancel]').onclick = onCancel;
  ta.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); save(); }
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); onCancel(); }
  });
  setTimeout(() => ta.focus(), 0);
  return tr;
}

function commentRow(c, convId) {
  const tr = h(`<tr class="dv-comment"><td colspan="3"><div class="rc">
    <span class="rc-icon">${icon('chat', 13)}</span><div class="rc-body">${esc(c.body)}</div>
    <button class="icon-btn sm" data-a="edit" title="${t('edit')}">${icon('edit', 13)}</button>
    <button class="icon-btn sm" data-a="del" title="${t('delete')}">${icon('trash', 13)}</button>
  </div></td></tr>`);
  tr.querySelector('[data-a=del]').onclick = () => { save(convId, load(convId).filter((x) => key(x) !== key(c))); refresh('changes'); };
  tr.querySelector('[data-a=edit]').onclick = () => {
    const ed = editorRow(c.body, (v) => { save(convId, load(convId).map((x) => (key(x) === key(c) ? { ...x, body: v } : x))); refresh('changes'); }, () => ed.replaceWith(tr));
    tr.replaceWith(ed);
  };
  return tr;
}

onDiffLine((tr, info, ctx) => {
  if (info.type === 'hunk') return;
  const side = info.type === 'del' ? 'old' : 'new';
  const line = side === 'old' ? info.oldNo : info.newNo;
  const c0 = { path: info.path, side, line, code: info.text };
  const btn = h(`<button class="rc-add" title="${esc(t('review_add_title'))}" aria-label="${esc(t('review_add_title'))}: ${esc(`${info.path}:${line}`)}">${icon('plus', 11)}</button>`);
  tr.querySelector('td.code').prepend(btn);
  btn.onclick = (e) => {
    e.stopPropagation();
    if (tr.nextSibling && tr.nextSibling.classList && tr.nextSibling.classList.contains('dv-comment-edit')) return;
    const ed = editorRow('', (body) => {
      const list = load(ctx.convId).filter((x) => key(x) !== key(c0));
      list.push({ ...c0, body });
      save(ctx.convId, list);
      refresh('changes');
    }, () => { ed.remove(); tr.focus(); });
    tr.after(ed);
  };
  // existing comment on this line: show it after the row once it's attached
  const existing = commentAt(ctx.convId, key(c0));
  if (existing) queueMicrotask(() => { if (tr.parentNode) tr.after(commentRow(existing, ctx.convId)); });
});

onChangesHeader((slot, ctx) => {
  const list = load(ctx.convId);
  if (!list.length) return;
  const send = h(`<button class="btn sm primary" title="${esc(t('review_send_d'))}">${icon('arrowUp', 13)} ${esc(t('review_send', { n: list.length }))}</button>`);
  send.onclick = async () => {
    if (send.disabled) return;
    const chat = await import('../chat.js');
    const c = chat.current();
    if (!c || c.id !== ctx.convId) return;
    const prompt = reviewPrompt(load(ctx.convId));
    send.disabled = true;
    let sent = false;
    try {
      c.composer.setText(prompt);
      sent = await c.composer.send();
    } finally { send.disabled = false; }
    // not sent (an error, or busy): keep the comments
    if (!sent) return;
    save(ctx.convId, []);
    refresh('changes');
  };
  const clear = h(`<button class="icon-btn sm" title="${esc(t('review_clear'))}">${icon('x', 13)}</button>`);
  clear.onclick = () => { save(ctx.convId, []); refresh('changes'); };
  slot.append(send, clear);
});
