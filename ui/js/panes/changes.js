// Changes pane: files that differ from the baseline (git HEAD, or what the
// agent changed outside git), a line-numbered diff viewer, and revert.
import { t } from '../i18n.js';
import { icon } from '../icons.js';
import { call, on } from '../api.js';
import { h, esc, errorToast, confirmDialog, debounce, showMenu, promptDialog, toast } from '../util.js';
import { registerPane, refresh, context, updateTabs } from '../workspace.js';

const state = new Map(); // convId -> { data, selected, diff, collapsed }
const lineHooks = [];    // (rowEl, info, ctx) => void — e.g. inline comments
const headerHooks = [];  // (headEl, ctx) => void

export function onDiffLine(fn) { lineHooks.push(fn); }
export function onChangesHeader(fn) { headerHooks.push(fn); }

/** Parse a unified diff into rows with old/new line numbers. */
export function parseUnified(text) {
  const rows = [];
  let o = 0, n = 0;
  for (const l of String(text || '').split('\n')) {
    if (l.startsWith('diff --git') || l.startsWith('index ') || l.startsWith('+++') || l.startsWith('---') || l.startsWith('new file') || l.startsWith('deleted file') || l.startsWith('similarity') || l.startsWith('rename ') || l.startsWith('old mode') || l.startsWith('new mode')) continue;
    const m = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/.exec(l);
    if (m) { o = +m[1]; n = +m[2]; rows.push({ type: 'hunk', text: l }); continue; }
    if (l === '' || l.startsWith('\\')) continue;
    if (l.startsWith('+')) rows.push({ type: 'add', newNo: n++, text: l.slice(1) });
    else if (l.startsWith('-')) rows.push({ type: 'del', oldNo: o++, text: l.slice(1) });
    else rows.push({ type: 'ctx', oldNo: o++, newNo: n++, text: l.slice(1) });
  }
  return rows;
}

function stateFor(convId) {
  if (!state.has(convId)) state.set(convId, { data: null, selected: null, diff: null });
  return state.get(convId);
}

const STATUS_LABEL = { M: 'M', A: 'A', D: 'D', R: 'R', '?': 'U' };

async function load(el, ctx) {
  const s = stateFor(ctx.convId);
  try { s.data = await call('changes_list', { conversationId: ctx.convId }); } catch (e) { s.data = { error: e.message || String(e), files: [] }; }
  if (s.selected && !s.data.files.some((f) => f.path === s.selected)) { s.selected = null; s.diff = null; }
  updateTabs();
  draw(el, ctx);
  if (s.selected) loadDiff(el, ctx, s.selected, true);
}

async function loadDiff(el, ctx, path, quiet) {
  const s = stateFor(ctx.convId);
  s.selected = path;
  if (!quiet) { s.diff = null; draw(el, ctx); }
  try { s.diff = await call('change_diff', { conversationId: ctx.convId, path }); } catch (e) { s.diff = `error: ${e.message || e}`; }
  if (s.selected === path) draw(el, ctx);
}

function draw(el, ctx) {
  const s = stateFor(ctx.convId);
  const d = s.data;
  el.innerHTML = '';
  if (!d) { el.appendChild(h(`<div class="pane-empty"><span class="spinner"></span></div>`)); return; }
  const files = d.files || [];
  const add = files.reduce((a, f) => a + (f.added || 0), 0);
  const del = files.reduce((a, f) => a + (f.removed || 0), 0);
  const head = h(`<div class="pane-head">
    ${d.repo ? `<button class="branch branch-btn" title="${esc(t('branch_menu'))}">${icon('code', 13)} ${esc(d.branch || '')} ${icon('chevDown', 12)}</button>` : `<span class="muted">${t('changes_session')}</span>`}
    <span class="diff-stat"><span class="a">+${add}</span> <span class="d">-${del}</span></span>
    <div class="grow"></div>
    <span class="hooks"></span>
    <button class="icon-btn" data-a="refresh" title="${t('refresh')}">${icon('refreshCw', 14)}</button>
    ${files.length ? `<button class="icon-btn" data-a="more" title="${esc(t('changes_more'))}">${icon('more', 14)}</button>` : ''}
  </div>`);
  head.querySelector('[data-a=refresh]').onclick = () => load(el, ctx);
  const bb = head.querySelector('.branch-btn');
  if (bb) bb.onclick = (ev) => branchMenu(ev.currentTarget, el, ctx);
  const revertAll = async () => {
    if (!(await confirmDialog(t('revert_all_confirm', { n: files.length }), { okLabel: t('revert_all') }))) return;
    try { await call('change_revert', { conversationId: ctx.convId, paths: files.map((f) => f.path) }); } catch (e) { errorToast(e); }
    load(el, ctx);
  };
  const more = head.querySelector('[data-a=more]');
  if (more) more.onclick = (ev) => showMenu(ev.currentTarget, [
    ...(d.repo ? [
      { label: t('changes_copy_patch'), icon: 'copy', onClick: async () => {
        try { const { copyText } = await import('../parts.js'); await copyText(await call('changes_patch', { conversationId: ctx.convId })); toast(t('copied')); } catch (e) { errorToast(e); }
      } },
      { label: t('changes_save_patch'), icon: 'download', onClick: async () => {
        try { await call('save_text_file', { name: `${(d.branch || 'changes').replace(/[\\/:*?"<>|]+/g, '-')}.patch`, content: await call('changes_patch', { conversationId: ctx.convId }) }); } catch (e) { errorToast(e); }
      } },
      { sep: true },
    ] : []),
    { label: t('revert_all'), icon: 'retry', danger: true, onClick: revertAll },
  ], { placement: 'bottom-end', width: 260 });
  for (const fn of headerHooks) fn(head.querySelector('.hooks'), ctx, d);
  el.appendChild(head);
  if (d.error) el.appendChild(h(`<div class="pane-empty">${esc(d.error)}</div>`));
  if (!files.length) {
    if (!d.error) el.appendChild(h(`<div class="pane-empty">${icon('check', 22)}<div>${d.repo ? t('changes_none_repo') : t('changes_none')}</div></div>`));
    return;
  }
  const list = h('<div class="change-list"></div>');
  for (const f of files) {
    const name = f.path.split('/').pop();
    const dir = f.path.slice(0, f.path.length - name.length);
    const row = h(`<div class="change-row ${s.selected === f.path ? 'on' : ''}" title="${esc(f.path)}">
      <span class="cst s-${STATUS_LABEL[f.status] || 'M'}">${STATUS_LABEL[f.status] || f.status}</span>
      <span class="cname ellipsis"><b>${esc(name)}</b> <span class="muted">${esc(dir)}</span></span>
      <span class="diff-stat"><span class="a">+${f.added || 0}</span> <span class="d">-${f.removed || 0}</span></span>
      <button class="icon-btn sm" data-a="ask" title="${t('ask_claude')}">${icon('sparkle', 13)}</button>
      <button class="icon-btn sm" data-a="open" title="${t('open_file')}">${icon('external', 13)}</button>
      <button class="icon-btn sm" data-a="revert" title="${t('revert')}">${icon('retry', 13)}</button>
    </div>`);
    row.onclick = (e) => { if (e.target.closest('button')) return; if (s.selected === f.path) { s.selected = null; s.diff = null; draw(el, ctx); } else loadDiff(el, ctx, f.path); };
    row.querySelector('[data-a=open]').onclick = () => call('open_in_editor', { path: `${d.root}/${f.path}`, line: null }).catch(errorToast);
    row.querySelector('[data-a=ask]').onclick = (ev) => {
      const mention = /\s/.test(f.path) ? `@"${f.path}"` : `@${f.path}`;
      const ask = async (prompt) => {
        const chat = await import('../chat.js');
        const c = chat.current();
        if (!c || c.id !== ctx.convId) return;
        c.composer.setText(prompt);
        c.composer.send();
      };
      showMenu(ev.currentTarget, [
        { label: t('ask_review'), icon: 'eye', onClick: () => ask(t('ask_review_p', { file: mention })) },
        { label: t('ask_explain'), icon: 'help', onClick: () => ask(t('ask_explain_p', { file: mention })) },
        { label: t('ask_tests'), icon: 'check', onClick: () => ask(t('ask_tests_p', { file: mention })) },
        { label: t('ask_custom'), icon: 'edit', onClick: async () => { const chat = await import('../chat.js'); const c = chat.current(); if (c) { c.composer.setText(`${mention} `); c.composer.focus(); } } },
      ], { placement: 'bottom-end', width: 260 });
    };
    row.querySelector('[data-a=revert]').onclick = async () => {
      if (!(await confirmDialog(t('revert_confirm', { file: f.path }), { okLabel: t('revert') }))) return;
      try { await call('change_revert', { conversationId: ctx.convId, paths: [f.path] }); } catch (e) { errorToast(e); }
      load(el, ctx);
    };
    list.appendChild(row);
    if (s.selected === f.path) list.appendChild(diffView(s.diff, f.path, ctx, d.repo && f.status !== '?' && f.status !== 'A' ? () => load(el, ctx) : null));
  }
  el.appendChild(list);
}

async function branchMenu(anchor, el, ctx) {
  let list = [];
  try { list = await call('git_branches', { conversationId: ctx.convId }); } catch (e) { errorToast(e); return; }
  const stashes = await call('git_stash_count', { conversationId: ctx.convId }).catch(() => 0);
  const dirty = !!(stateFor(ctx.convId).data && stateFor(ctx.convId).data.files && stateFor(ctx.convId).data.files.length);
  const stash = async (pop) => {
    try { toast(await call('git_stash', { conversationId: ctx.convId, pop })); } catch (e) { errorToast(e); }
    load(el, ctx);
  };
  const go = async (name, create) => {
    try { toast(await call('git_switch', { conversationId: ctx.convId, name, create })); } catch (e) { errorToast(e); }
    load(el, ctx);
  };
  showMenu(anchor, [
    { header: t('branch_switch') },
    ...list.slice(0, 15).map((b) => ({ label: b.name, sub: b.subject, icon: 'code', checked: b.current, onClick: () => { if (!b.current) go(b.name, false); } })),
    { sep: true },
    { label: t('branch_new'), icon: 'plus', onClick: async () => {
      const name = await promptDialog(t('branch_new'), 'clauderust/');
      if (name && name.trim()) go(name.trim(), true);
    } },
    { sep: true },
    { label: t('stash_push'), icon: 'download', disabled: !dirty, onClick: () => stash(false) },
    { label: t('stash_pop'), sub: stashes ? t('stash_count', { n: stashes }) : '', icon: 'retry', disabled: !stashes, onClick: () => stash(true) },
  ], { placement: 'bottom-start', width: 300 });
}

/** `onHunkReverted`: per-hunk revert is offered (tracked files in git). */
function diffView(text, path, ctx, onHunkReverted) {
  const box = h('<div class="diff-view"></div>');
  if (text == null) { box.innerHTML = '<div class="pane-empty"><span class="spinner"></span></div>'; return box; }
  const rows = parseUnified(text);
  if (!rows.length) { box.innerHTML = `<div class="muted" style="padding:10px">${t('no_textual_changes')}</div>`; return box; }
  const tbl = h('<table class="dv"><tbody></tbody></table>');
  const tb = tbl.querySelector('tbody');
  let hunk = -1;
  for (const r of rows) {
    if (r.type === 'hunk') {
      hunk += 1;
      const tr = h(`<tr class="dv-hunk"><td colspan="3"><span class="dv-hunk-text">${esc(r.text)}</span>${onHunkReverted ? `<button class="btn sm ghost dv-hunk-revert" title="${esc(t('hunk_revert_tip'))}">${icon('retry', 12)} ${esc(t('revert'))}</button>` : ''}</td></tr>`);
      const b = tr.querySelector('.dv-hunk-revert');
      if (b) {
        const index = hunk;
        b.onclick = async (e) => {
          e.stopPropagation();
          try { await call('change_revert_hunk', { conversationId: ctx.convId, path, index, header: r.text }); onHunkReverted(); } catch (err) { errorToast(err); }
        };
      }
      tb.appendChild(tr);
      continue;
    }
    const tr = h(`<tr class="dv-${r.type}"><td class="no">${r.oldNo ?? ''}</td><td class="no">${r.newNo ?? ''}</td><td class="code"><span class="sign">${r.type === 'add' ? '+' : r.type === 'del' ? '-' : ' '}</span>${esc(r.text)}</td></tr>`);
    for (const fn of lineHooks) fn(tr, { path, ...r }, ctx);
    tb.appendChild(tr);
  }
  box.appendChild(tbl);
  return box;
}

const reloadSoon = debounce(() => {
  const ctx = context();
  if (!ctx) return;
  refresh('changes');
}, 600);

registerPane({
  id: 'changes',
  order: 10,
  icon: 'edit',
  label: () => t('ws_changes'),
  badge: (ctx) => { const s = state.get(ctx.convId); return s && s.data && s.data.files ? s.data.files.length || '' : ''; },
  mount: (el, ctx) => { el.classList.add('changes-pane'); load(el, ctx); },
  show: (el, ctx) => load(el, ctx),
});

on('files-changed', ({ conversation_id }) => { const c = context(); if (c && c.convId === conversation_id) reloadSoon(); });
on('generation-state', ({ conversation_id, running }) => { const c = context(); if (c && c.convId === conversation_id && !running) reloadSoon(); });
