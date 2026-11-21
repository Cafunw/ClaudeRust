// History pane: recent commits, each commit's diff per file, and actions
// (ask Claude to explain it, revert it).
import { t } from '../i18n.js';
import { icon } from '../icons.js';
import { call, on } from '../api.js';
import { h, esc, errorToast, confirmDialog, toast, relTime } from '../util.js';
import { registerPane, context, refresh, show } from '../workspace.js';
import { parseUnified } from './changes.js';

const open = new Map(); // convId -> sha

function diffTable(text) {
  // the first line is the "a/x b/x" header of the file section
  const rows = parseUnified(String(text || '').split('\n').slice(1).join('\n'));
  return `<table class="dv"><tbody>${rows.map((r) => r.type === 'hunk'
    ? `<tr class="dv-hunk"><td colspan="3">${esc(r.text)}</td></tr>`
    : `<tr class="dv-${r.type}"><td class="no">${r.oldNo ?? ''}</td><td class="no">${r.newNo ?? ''}</td><td class="code"><span class="sign">${r.type === 'add' ? '+' : r.type === 'del' ? '-' : ' '}</span>${esc(r.text)}</td></tr>`).join('')}</tbody></table>`;
}

async function ask(ctx, prompt) {
  const chat = await import('../chat.js');
  const c = chat.current();
  if (!c || c.id !== ctx.convId) return;
  c.composer.setText(prompt);
  c.composer.send();
}

const searches = new Map(); // convId -> { q, mode, seq }

function searchFor(convId) {
  if (!searches.has(convId)) searches.set(convId, { q: '', mode: 'message', seq: 0 });
  return searches.get(convId);
}

/** The head (branch, count, search) is built once, so typing isn't interrupted. */
function ensureHead(el, ctx) {
  if (el.querySelector('.hist-head')) return;
  const s = searchFor(ctx.convId);
  el.innerHTML = '';
  const head = h(`<div class="hist-head"><div class="pane-head"><span class="branch">${icon('code', 13)} <span class="b"></span></span><span class="muted cnt"></span><div class="grow"></div><button class="icon-btn" data-a="refresh" title="${t('refresh')}">${icon('refreshCw', 14)}</button></div>
    <div class="hist-search"><input class="input sm" placeholder="${esc(t('hist_search_ph'))}" spellcheck="false"><select class="select sm">
      <option value="message">${esc(t('hist_mode_message'))}</option><option value="author">${esc(t('hist_mode_author'))}</option><option value="code">${esc(t('hist_mode_code'))}</option></select></div></div>`);
  el.append(head, h('<div class="hist-list"></div>'));
  const input = head.querySelector('input');
  const sel = head.querySelector('select');
  input.value = s.q;
  sel.value = s.mode;
  let tm;
  input.addEventListener('input', () => { clearTimeout(tm); tm = setTimeout(() => { s.q = input.value; draw(el, ctx); }, 350); });
  sel.addEventListener('change', () => { s.mode = sel.value; if (s.q.trim()) draw(el, ctx); });
  head.querySelector('[data-a=refresh]').onclick = () => draw(el, ctx);
}

async function draw(el, ctx) {
  ensureHead(el, ctx);
  const s = searchFor(ctx.convId);
  const seq = ++s.seq;
  const list = el.querySelector('.hist-list');
  let data;
  try { data = await call('git_log', { conversationId: ctx.convId, query: s.q.trim() || null, mode: s.mode }); } catch (e) { list.innerHTML = `<div class="pane-empty">${esc(e.message || String(e))}</div>`; return; }
  if (seq !== s.seq) return; // a newer search is on its way
  el.querySelector('.hist-head .b').textContent = data.branch;
  el.querySelector('.hist-head .cnt').textContent = data.commits.length === 1 ? t('hist_commit_one') : `${data.commits.length} ${t('hist_commits')}`;
  list.innerHTML = '';
  if (!data.commits.length) { list.appendChild(h(`<div class="pane-empty">${esc(s.q.trim() ? t('no_results') : t('hist_empty'))}</div>`)); return; }
  for (const c of data.commits) {
    const isOpen = open.get(ctx.convId) === c.sha;
    const row = h(`<div class="hist-row ${isOpen ? 'on' : ''}">
      <div class="hist-line"><code class="sha">${esc(c.short)}</code><div class="hist-main"><div class="hist-subj ellipsis">${esc(c.subject)}</div><div class="muted small">${esc(c.author)} · ${esc(relTime(c.time))}${c.refs ? ` · <span class="refs">${esc(c.refs)}</span>` : ''}</div></div>
      <button class="icon-btn sm" data-a="explain" title="${t('hist_explain')}">${icon('sparkle', 13)}</button>
      <button class="icon-btn sm" data-a="revert" title="${t('hist_revert')}">${icon('retry', 13)}</button></div>
      <div class="hist-body"></div></div>`);
    row.querySelector('.hist-line').onclick = (e) => {
      if (e.target.closest('button')) return;
      open.set(ctx.convId, isOpen ? null : c.sha);
      draw(el, ctx);
    };
    row.querySelector('[data-a=explain]').onclick = () => ask(ctx, t('hist_explain_p', { sha: c.short, subject: c.subject }));
    row.querySelector('[data-a=revert]').onclick = async () => {
      if (!(await confirmDialog(t('hist_revert_confirm', { sha: c.short, subject: c.subject }), { okLabel: t('hist_revert') }))) return;
      try { toast(await call('git_revert', { conversationId: ctx.convId, sha: c.sha })); draw(el, ctx); } catch (e) { errorToast(e); }
    };
    if (isOpen) {
      const body = row.querySelector('.hist-body');
      body.innerHTML = '<div class="pane-empty"><span class="spinner"></span></div>';
      call('git_show', { conversationId: ctx.convId, sha: c.sha }).then((d) => {
        body.innerHTML = `${d.message && d.message !== c.subject ? `<pre class="hist-msg">${esc(d.message)}</pre>` : ''}${d.files.map((f, i) => `<details class="hist-file" ${i < 3 ? 'open' : ''}><summary><code>${esc(f.path)}</code> <span class="diff-stat"><span class="a">+${f.added}</span> <span class="d">-${f.removed}</span></span></summary><div class="diff-view">${diffTable(f.diff)}</div></details>`).join('')}`;
      }).catch((e) => { body.textContent = e.message || String(e); });
    }
    list.appendChild(row);
  }
}

registerPane({
  id: 'history',
  order: 15,
  icon: 'history',
  label: () => t('ws_history'),
  mount: (el, ctx) => { el.classList.add('history-pane'); draw(el, ctx); },
  show: (el, ctx) => draw(el, ctx),
});

on('files-changed', ({ conversation_id }) => { const c = context(); if (c && c.convId === conversation_id) refresh('history'); });

/** Open the History pane on a commit (e.g. from the Files viewer's blame). */
export function showCommit(sha) {
  const c = context();
  if (!c) return;
  open.set(c.convId, sha);
  show('history');
  refresh('history');
}
