// Inline review comments on the Changes pane's diff: comment on any line,
// then send all comments to Claude as one revision request.
import { t } from '../i18n.js';
import { icon } from '../icons.js';
import { h, esc } from '../util.js';
import { loadLocal, saveLocal } from '../store.js';
import { onDiffLine, onChangesHeader } from './changes.js';
import { refresh } from '../workspace.js';

function load(convId) { return (loadLocal().reviewComments || {})[convId] || []; }
function save(convId, list) {
  const all = loadLocal().reviewComments || {};
  if (list.length) all[convId] = list; else delete all[convId];
  saveLocal({ reviewComments: all });
}

const key = (c) => `${c.path}|${c.side}|${c.line}`;

/** The prompt sent for a set of comments. */
export function reviewPrompt(list) {
  const lines = [t('review_prompt_intro'), ''];
  list.forEach((c, i) => {
    lines.push(`${i + 1}. \`${c.path}:${c.line}\`${c.side === 'old' ? ' (removed line)' : ''} — ${c.body.trim()}`);
    if (c.code && c.code.trim()) lines.push(`   > ${c.code.trim().slice(0, 200)}`);
  });
  lines.push('', t('review_prompt_outro'));
  return lines.join('\n');
}

function editorRow(initial, onSave, onCancel) {
  const tr = h(`<tr class="dv-comment-edit"><td colspan="3"><div class="rc-edit">
    <textarea rows="2" placeholder="${esc(t('review_placeholder'))}">${esc(initial || '')}</textarea>
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
  const btn = h(`<button class="rc-add" title="${esc(t('review_add_title'))}">${icon('plus', 11)}</button>`);
  tr.querySelector('td.code').prepend(btn);
  btn.onclick = (e) => {
    e.stopPropagation();
    if (tr.nextSibling && tr.nextSibling.classList && tr.nextSibling.classList.contains('dv-comment-edit')) return;
    const ed = editorRow('', (body) => {
      const list = load(ctx.convId).filter((x) => key(x) !== key(c0));
      list.push({ ...c0, body });
      save(ctx.convId, list);
      refresh('changes');
    }, () => ed.remove());
    tr.after(ed);
  };
  // existing comment on this line: show it after the row once it's attached
  const existing = load(ctx.convId).find((x) => key(x) === key(c0));
  if (existing) queueMicrotask(() => { if (tr.parentNode) tr.after(commentRow(existing, ctx.convId)); });
});

onChangesHeader((slot, ctx) => {
  const list = load(ctx.convId);
  if (!list.length) return;
  const send = h(`<button class="btn sm primary" title="${esc(t('review_send_d'))}">${icon('arrowUp', 13)} ${esc(t('review_send', { n: list.length }))}</button>`);
  send.onclick = async () => {
    const chat = await import('../chat.js');
    const c = chat.current();
    if (!c || c.id !== ctx.convId) return;
    c.composer.setText(reviewPrompt(load(ctx.convId)));
    await c.composer.send();
    save(ctx.convId, []);
    refresh('changes');
  };
  const clear = h(`<button class="icon-btn sm" title="${esc(t('review_clear'))}">${icon('x', 13)}</button>`);
  clear.onclick = () => { save(ctx.convId, []); refresh('changes'); };
  slot.append(send, clear);
});
