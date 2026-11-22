// Files pane: the session folder's file tree (.gitignore-aware) and a
// read-only, highlighted viewer. Files and lines can be @mentioned.
import { t, currentLanguage } from '../i18n.js';
import { icon } from '../icons.js';
import { call, on } from '../api.js';
import { h, esc, errorToast, humanSize, debounce, showMenu, toast, relTime, snapshot, sameSnapshot } from '../util.js';
import { copyText } from '../parts.js';
import { registerPane, context, refresh, show } from '../workspace.js';
import { keyRows } from './rows.js';
import { errorText } from '../neterr.js';

const state = new Map(); // convId -> { open: Set<dir>, file: path|null, filter: '' }

function st(convId) {
  if (!state.has(convId)) state.set(convId, { open: new Set(), file: null, line: null, filter: '', grep: '', mode: 'names', regex: false, case: false, scroll: 0, seq: 0 });
  return state.get(convId);
}

async function composer() {
  const chat = await import('../chat.js');
  const c = chat.current();
  return c && c.composer;
}

async function mention(path, line) {
  const c = await composer();
  if (!c) return;
  // the line goes inside the quotes: `@"a b.rs#L12"` (how mentions are parsed)
  const target = line ? `${path}#L${line}` : path;
  c.insertText(`@${/\s/.test(target) ? `"${target}"` : target} `);
}

const base = (p) => p.split('/').pop();

async function emitNav(id) {
  const { emit } = await import('../store.js');
  emit('navigate', { name: 'chat', params: { id } });
}

async function drawTree(el, ctx) {
  const s = st(ctx.convId);
  el.innerHTML = '';
  const grep = s.mode === 'contents';
  const head = h(`<div class="pane-head fb-head">
    <div class="fb-mode segmented"><button data-m="names" class="${grep ? '' : 'on'}" aria-pressed="${!grep}" title="${esc(t('files_mode_names'))}">${icon('file', 13)}</button><button data-m="contents" class="${grep ? 'on' : ''}" aria-pressed="${grep}" title="${esc(t('files_mode_contents'))}">${icon('search', 13)}</button></div>
    <input class="input sm fb-filter" placeholder="${esc(grep ? t('files_grep_ph') : t('files_filter'))}" aria-label="${esc(grep ? t('files_grep_ph') : t('files_filter'))}" spellcheck="false">
    ${grep ? `<button class="icon-btn sm fb-opt ${s.case ? 'on' : ''}" data-o="case" aria-pressed="${!!s.case}" title="${esc(t('files_case'))}" aria-label="${esc(t('files_case'))}">Aa</button><button class="icon-btn sm fb-opt ${s.regex ? 'on' : ''}" data-o="regex" aria-pressed="${!!s.regex}" title="${esc(t('files_regex'))}" aria-label="${esc(t('files_regex'))}">.*</button>` : ''}
    <button class="icon-btn" data-a="refresh" title="${t('refresh')}">${icon('refreshCw', 14)}</button></div>`);
  const list = h('<div class="fb-tree"></div>');
  el.append(head, list);
  const input = head.querySelector('input');
  input.value = grep ? (s.grep || '') : s.filter;
  head.querySelector('[data-a=refresh]').onclick = () => drawTree(el, ctx);
  head.querySelectorAll('.fb-mode button').forEach((b) => { b.onclick = () => { if (s.mode !== b.dataset.m) { s.mode = b.dataset.m; s.scroll = 0; drawTree(el, ctx).then(() => el.querySelector('.fb-filter').focus()); } }; });
  head.querySelectorAll('.fb-opt').forEach((b) => { b.onclick = () => { s[b.dataset.o] = !s[b.dataset.o]; b.classList.toggle('on', !!s[b.dataset.o]); b.setAttribute('aria-pressed', String(!!s[b.dataset.o])); fill(list, ctx); }; });
  const run = debounce(() => { if (grep) s.grep = input.value; else s.filter = input.value.trim(); fill(list, ctx); }, grep ? 300 : 150);
  input.addEventListener('input', run);
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { const first = list.querySelector('.fb-row.file, .gr-hit'); if (first) first.click(); }
    if (e.key === 'ArrowDown') { const first = list.querySelector(ROWS); if (first) { e.preventDefault(); first.focus(); } }
  });
  await fill(list, ctx);
  list.scrollTop = s.scroll;
  list.addEventListener('scroll', () => { s.scroll = list.scrollTop; });
}

const ROWS = '.fb-row, .gr-file, .gr-hit';

/** `flat`: a search result, shown as its name and then its folder (no tree to open). */
function row(entry, depth, ctx, el, flat = false) {
  const s = st(ctx.convId);
  const open = entry.dir && s.open.has(entry.path);
  const name = base(entry.path);
  const dir = entry.path.slice(0, entry.path.length - name.length);
  const r = h(`<div class="fb-row ${entry.dir ? 'dir' : 'file'}" style="padding-left:${8 + depth * 14}px" title="${esc(entry.path)}" data-k="${esc(entry.path)}"${entry.dir && !flat ? ` aria-expanded="${open}"` : ''}>
    <span class="tw">${entry.dir && !flat ? icon(open ? 'chevDown' : 'chevRight', 12) : ''}</span>${icon(entry.dir ? 'folder' : 'file', 14)}<span class="nm ellipsis">${flat ? `${esc(name)} <span class="muted">${esc(dir)}</span>` : esc(name)}</span>
    ${entry.dir ? '' : `<button class="icon-btn sm fb-at" title="${esc(t('files_mention'))}" aria-label="${esc(t('files_mention'))}: ${esc(name)}">@</button>`}</div>`);
  if (entry.dir) {
    r.onclick = () => {
      if (open) s.open.delete(entry.path); else s.open.add(entry.path);
      fill(el.querySelector('.fb-tree'), ctx);
    };
  } else {
    r.onclick = (e) => {
      if (e.target.closest('.fb-at')) { mention(entry.path); return; }
      s.file = entry.path;
      s.line = null;
      draw(el, ctx);
    };
  }
  return r;
}

/** quiet: a refresh in the background (files changed): no spinner, and the scroll position is kept. */
async function fill(list, ctx, opts) {
  await fillList(list, ctx, opts);
  keyRows(list, ROWS);
}

async function fillList(list, ctx, { quiet = false } = {}) {
  const s = st(ctx.convId);
  const el = list.closest('.ws-pane');
  const seq = (s.seq = (s.seq || 0) + 1);
  if (s.mode === 'contents' || s.filter) list.__tree = null; // what's drawn below is no tree
  if (s.mode === 'contents') {
    const q = s.grep || '';
    if (!q.trim()) { list.innerHTML = `<div class="pane-empty">${esc(t('files_grep_hint'))}</div>`; return; }
    if (!quiet) list.innerHTML = '<div class="pane-empty"><span class="spinner"></span></div>';
    let r;
    try { r = await call('search_in_files', { conversationId: ctx.convId, query: q, regex: !!s.regex, caseSensitive: !!s.case }); } catch (e) { if (seq === s.seq) list.innerHTML = `<div class="pane-empty">${esc(errorText(e))}</div>`; return; }
    if (seq !== s.seq) return;
    list.innerHTML = '';
    if (!r.hits.length) { list.appendChild(h(`<div class="pane-empty">${esc(t('no_results'))}</div>`)); return; }
    list.appendChild(h(`<div class="gr-sum muted small">${esc(t('files_grep_sum', { n: r.hits.length, files: r.files }))}${r.truncated ? ` · ${esc(t('files_grep_more'))}` : ''}</div>`));
    let last = null;
    for (const hit of r.hits) {
      if (hit.path !== last) {
        last = hit.path;
        const fr = h(`<div class="gr-file" title="${esc(hit.path)}" data-k="${esc(hit.path)}">${icon('file', 13)}<span class="ellipsis">${esc(hit.path)}</span></div>`);
        fr.onclick = () => { s.file = hit.path; s.line = null; draw(el, ctx); };
        list.appendChild(fr);
      }
      const enc = new TextEncoder();
      const dec = new TextDecoder();
      const bytes = enc.encode(hit.text);
      const pre = dec.decode(bytes.slice(0, hit.start));
      const mid = dec.decode(bytes.slice(hit.start, hit.end));
      const post = dec.decode(bytes.slice(hit.end));
      const hr = h(`<div class="gr-hit" data-k="${esc(`${hit.path}:${hit.line}`)}"><span class="no">${hit.line}</span><span class="tx">${esc(pre.trimStart())}<mark>${esc(mid)}</mark>${esc(post)}</span><button class="icon-btn sm fb-at" title="${esc(t('files_mention'))}" aria-label="${esc(t('files_mention'))}: ${esc(`${base(hit.path)}:${hit.line}`)}">@</button></div>`);
      hr.onclick = (e) => {
        if (e.target.closest('.fb-at')) { mention(hit.path, hit.line); return; }
        s.file = hit.path;
        s.line = hit.line;
        draw(el, ctx);
      };
      list.appendChild(hr);
    }
    return;
  }
  if (s.filter) {
    let hits = [];
    try { hits = await call('file_search', { conversationId: ctx.convId, query: s.filter }); } catch { hits = []; }
    if (seq !== s.seq) return; // a newer query has been typed
    list.innerHTML = '';
    if (!hits.length) { list.appendChild(h(`<div class="pane-empty">${esc(t('no_results'))}</div>`)); return; }
    for (const f of hits) {
      const r = row({ path: f.path, dir: !!f.dir }, 0, ctx, el, true);
      if (f.dir) r.onclick = () => { s.filter = ''; let p = ''; for (const part of f.path.split('/')) { p = p ? `${p}/${part}` : part; s.open.add(p); } drawTree(el, ctx); };
      list.appendChild(r);
    }
    return;
  }
  const frag = document.createDocumentFragment();
  const seen = []; // what the rows are drawn from
  const walk = async (dir, depth) => {
    let items;
    try { items = await call('project_dir', { conversationId: ctx.convId, dir }); } catch (e) { seen.push(['error', errorText(e)]); frag.appendChild(h(`<div class="pane-empty">${esc(errorText(e))}</div>`)); return; }
    for (const it of items) {
      seen.push([depth, it, !!(it.dir && s.open.has(it.path))]);
      frag.appendChild(row(it, depth, ctx, el));
      if (it.dir && s.open.has(it.path)) await walk(it.path, depth + 1);
    }
  };
  await walk('', 0);
  if (seq !== s.seq) return;
  // the tree that's shown already (a refill after files changed, mostly):
  // keep its rows, and with them hover and focus
  const sig = snapshot([seen, currentLanguage()]);
  if (sameSnapshot(list.__tree, sig)) { list.__tree = sig; return; }
  list.__tree = sig;
  list.innerHTML = '';
  if (!frag.childNodes.length) list.appendChild(h(`<div class="pane-empty">${esc(t('files_empty'))}</div>`));
  list.appendChild(frag);
}

async function drawFile(el, ctx) {
  const s = st(ctx.convId);
  const was = el.contains(document.activeElement) ? document.activeElement.dataset.a : null;
  el.innerHTML = `<div class="pane-empty"><span class="spinner"></span></div>`;
  let f;
  try { f = await call('project_file', { conversationId: ctx.convId, path: s.file }); } catch (e) { s.file = null; errorToast(e); drawTree(el, ctx); return; }
  el.innerHTML = '';
  const crumbs = f.path.split('/').map((p) => esc(p)).join('<span class="sep">/</span>');
  const head = h(`<div class="pane-head fv-head"><button class="icon-btn" data-a="back" title="${t('back')}">${icon('arrowLeft', 14)}</button>
    <div class="fv-path ellipsis" title="${esc(f.path)}">${crumbs}</div>
    <span class="muted small">${f.lines != null ? `${f.lines} ${esc(t('files_lines'))} · ` : ''}${esc(humanSize(f.size))}</span>
    <button class="icon-btn ${s.blame ? 'on' : ''}" data-a="blame" title="${esc(t('fv_blame'))}">${icon('history', 14)}</button>
    <button class="icon-btn" data-a="ask" title="${esc(t('fv_ask'))}">${icon('sparkle', 14)}</button>
    <button class="icon-btn" data-a="at" title="${esc(t('files_mention'))}" aria-label="${esc(t('files_mention'))}">@</button>
    <button class="icon-btn" data-a="more" title="${esc(t('fv_more'))}">${icon('more', 14)}</button>
    <button class="icon-btn" data-a="edit" title="${esc(t('open_in_editor'))}">${icon('external', 14)}</button></div>`);
  const asMention = () => `@${/\s/.test(f.path) ? `"${f.path}"` : f.path}`;
  const ask = async (prompt) => {
    const c = await composer();
    if (!c) return;
    const draft = c.getText().trim();
    c.setText(draft ? `${draft}\n\n${prompt}` : prompt);
    c.send();
  };
  head.querySelector('[data-a=ask]').onclick = (ev) => showMenu(ev.currentTarget, [
    { label: t('fv_explain'), icon: 'help', onClick: () => ask(t('fv_explain_p', { file: asMention() })) },
    { label: t('fv_review'), icon: 'eye', onClick: () => ask(t('fv_review_p', { file: asMention() })) },
    { label: t('fv_tests'), icon: 'check', onClick: () => ask(t('fv_tests_p', { file: asMention() })) },
    { label: t('ask_custom'), icon: 'edit', onClick: () => mention(f.path) },
  ], { placement: 'bottom-end', width: 240 });
  head.querySelector('[data-a=more]').onclick = (ev) => showMenu(ev.currentTarget, [
    { label: t('fv_copy_path'), icon: 'copy', onClick: () => copyText(f.path).then(() => toast(t('copied'))) },
    { label: t('fv_copy_abs'), icon: 'copy', onClick: () => copyText(String(f.abs)).then(() => toast(t('copied'))) },
    { label: t('reveal'), icon: 'folder', onClick: () => call('reveal_path', { path: String(f.abs) }).catch(errorToast) },
    { sep: true },
    { label: t('fv_sessions'), icon: 'chat', onClick: async () => {
      let list = [];
      try { list = await call('file_sessions', { conversationId: ctx.convId, path: f.path }); } catch (e) { errorToast(e); return; }
      const anchor = head.querySelector('[data-a=more]');
      showMenu(anchor, list.length
        ? [{ header: t('fv_sessions') }, ...list.map((c) => ({ label: c.title || t('untitled'), sub: relTime(c.at), icon: 'chat', checked: c.id === ctx.convId, onClick: () => emitNav(c.id) }))]
        : [{ label: t('fv_sessions_none'), disabled: true }], { placement: 'bottom-end', width: 300 });
    } },
  ], { placement: 'bottom-end', width: 260 });
  head.querySelector('[data-a=back]').onclick = () => { s.file = null; s.line = null; drawTree(el, ctx); };
  head.querySelector('[data-a=blame]').onclick = () => { s.blame = !s.blame; drawFile(el, ctx); };
  head.querySelector('[data-a=at]').onclick = () => mention(f.path);
  head.querySelector('[data-a=edit]').onclick = () => call('open_in_editor', { path: f.abs, line: s.line || null }).catch(errorToast);
  el.appendChild(head);
  // opened from the keyboard (its row is gone now), or the blame button redrew it: focus stays in the pane
  if (document.activeElement === document.body) (head.querySelector(`[data-a="${was}"]`) || head.querySelector('[data-a=back]')).focus({ preventScroll: true });
  if (f.binary || f.too_large) {
    el.appendChild(h(`<div class="pane-empty">${icon('file', 22)}<div>${esc(f.binary ? t('files_binary') : t('files_too_large'))}</div></div>`));
    return;
  }
  const n = f.shown || 0;
  let gutter = '';
  for (let i = 1; i <= n; i++) gutter += `<span>${i}</span>\n`;
  const body = h(`<div class="fv-body"><pre class="fv-gutter" title="${esc(t('files_line_tip'))}">${gutter}</pre><pre class="fv-code"><code class="hl-code">${f.html}</code></pre></div>`);
  body.querySelector('.fv-gutter').onclick = (e) => {
    const sp = e.target.closest('span');
    if (sp) mention(f.path, Number(sp.textContent));
  };
  el.appendChild(body);
  if (s.blame) addBlame(body, ctx, f.path, n);
  // a line to show (from /todos): scroll to it and mark it
  const target = s.line && body.querySelectorAll('.fv-gutter span')[s.line - 1];
  if (target) {
    target.classList.add('on');
    const band = h('<div class="fv-hl"></div>');
    band.style.top = `${target.offsetTop}px`;
    band.style.height = `${target.offsetHeight}px`;
    body.appendChild(band);
    requestAnimationFrame(() => target.scrollIntoView({ block: 'center' }));
  }
  if (f.lines > n) el.appendChild(h(`<div class="muted small fv-more">${esc(t('files_truncated', { n, total: f.lines }))}</div>`));
}

/** A column with the commit that last changed each line. */
async function addBlame(body, ctx, path, n) {
  const col = h(`<pre class="fv-blame"><span class="muted">…</span></pre>`);
  body.insertBefore(col, body.firstChild);
  let b;
  try { b = await call('file_blame', { conversationId: ctx.convId, path }); } catch (e) { col.innerHTML = `<span class="fv-blame-err" title="${esc(errorText(e))}">${esc(errorText(e))}</span>`; return; }
  const zero = /^0+$/;
  let out = '';
  for (let i = 0; i < Math.min(n, b.lines.length); i++) {
    const ci = b.lines[i];
    if (i > 0 && b.lines[i - 1] === ci) { out += '<span class="same"></span>'; continue; }
    const c = b.commits[ci] || {};
    const label = zero.test(c.sha || '') ? t('blame_uncommitted') : `${String(c.sha).slice(0, 7)} ${c.author} · ${relTime(c.time)}`;
    out += `<span class="${zero.test(c.sha || '') ? 'unc' : 'c'}" data-sha="${esc(c.sha || '')}" title="${esc(`${c.summary || ''}${c.author ? `\n${c.author}` : ''}`)}">${esc(label)}</span>`;
  }
  col.innerHTML = out;
  // match the code's actual line pitch (fonts can make it differ from the CSS value)
  const g = body.querySelectorAll('.fv-gutter span');
  if (g.length > 1) {
    const pitch = (g[g.length - 1].getBoundingClientRect().top - g[0].getBoundingClientRect().top) / (g.length - 1);
    if (pitch > 0) col.style.setProperty('--row-h', `${pitch}px`);
  }
  col.onclick = async (e) => {
    const sp = e.target.closest('span.c');
    if (!sp) return;
    const hist = await import('./history.js');
    hist.showCommit(sp.dataset.sha);
  };
}

function draw(el, ctx) {
  return st(ctx.convId).file ? drawFile(el, ctx) : drawTree(el, ctx);
}

/** Show a project file in the Files pane, optionally at a line. */
export function showFile(path, line) {
  const ctx = context();
  if (!ctx) return;
  st(ctx.convId).file = path;
  st(ctx.convId).line = line || null;
  show('files');
  refresh('files');
}

/**
 * Refill the tree (or results) in place after files changed: the header,
 * with the filter box someone may be typing in, stays as it is, so it keeps
 * focus, selection and caret.
 */
async function refill(el, ctx) {
  const list = el.querySelector('.fb-tree');
  if (!list) return draw(el, ctx);
  const s = st(ctx.convId);
  const top = list.scrollTop;
  await fill(list, ctx, { quiet: true });
  if (list.isConnected) { list.scrollTop = top; s.scroll = top; }
  return undefined;
}

registerPane({
  id: 'files',
  order: 65,
  icon: 'folder',
  label: () => t('ws_files'),
  // show() comes right after and draws it
  mount: (el) => { el.classList.add('files-pane'); },
  show: (el, ctx) => {
    const s = st(ctx.convId);
    if (s.soft) { s.soft = false; if (!s.file) return refill(el, ctx); }
    return draw(el, ctx);
  },
});

// An agent edits and runs commands in bursts, each one a files-changed: the
// tree is read again once the burst settles (each refill reads every open
// folder), not once per event.
const changed = new Set(); // conversations with files changed since the last refill
const refillSoon = debounce(() => {
  const c = context();
  const ids = [...changed];
  changed.clear();
  if (!c || !ids.includes(c.convId)) return;
  const s = st(c.convId);
  if (s.file) return;
  s.soft = true;
  refresh('files');
  // refresh() only reaches the pane while it's the one shown
  s.soft = false;
}, 200);

on('files-changed', ({ conversation_id }) => {
  const c = context();
  if (c && c.convId === conversation_id && !st(conversation_id).file) {
    changed.add(conversation_id);
    refillSoon();
  }
});
