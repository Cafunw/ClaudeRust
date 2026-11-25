// "Add from GitHub": a repository's text files, picked from its tree, added
// to the composer or to a project's knowledge (github_source.rs).
import { icon } from './icons.js';
import { t, tOr } from './i18n.js';
import { call } from './api.js';
import { $, esc, h, modal, button, humanSize, toast } from './util.js';
import { attachmentError } from './composer.js';
import { errorText } from './neterr.js';

/** A failure of a source in the UI's language: a refused attachment, a refusal
 *  the backend names by kind, else the error's plain text; `name` the file it was about. */
export function sourceError(e, name) {
  const att = attachmentError(e, name);
  if (att) return att;
  const said = tOr(`errk_${e && e.kind}`, null) || errorText(e);
  return name ? `${name}: ${said}` : said;
}

/** An error box for a source dialog. */
export function sourceNotice(text) {
  return h(`<div class="notice error">${icon('alert', 15)}<div class="grow small">${esc(text)}</div></div>`);
}

/** `files` (repository paths, each kept with its index `i`) as folders under
 *  `root` (a folder, a file, or "" for the whole repository). */
export function fileTree(files, root) {
  const top = { name: '', dirs: new Map(), files: [] };
  files.forEach((f, i) => {
    const rel = f.path === root ? f.path.split('/').pop() : root ? f.path.slice(root.length + 1) : f.path;
    const parts = rel.split('/');
    let node = top;
    for (const d of parts.slice(0, -1)) {
      if (!node.dirs.has(d)) node.dirs.set(d, { name: d, dirs: new Map(), files: [] });
      node = node.dirs.get(d);
    }
    node.files.push({ ...f, i, name: parts[parts.length - 1] });
  });
  return top;
}

/** What the picked `files` add up to, and the cap of `limits` they're over ('files', 'bytes' or null). */
export function pickSummary(files, limits) {
  const bytes = files.reduce((n, f) => n + (f.size || 0), 0);
  return { count: files.length, bytes, over: files.length > limits.files ? 'files' : bytes > limits.bytes ? 'bytes' : null };
}

const show = (el, on) => { el.style.display = on ? '' : 'none'; };
export const filesN = (n) => t(n === 1 ? 'ghsrc_files_one' : 'ghsrc_files_n', { n });

/** "owner/repo", and the branch and folder after it, of an address like https://github.com/owner/repo/tree/main/src. */
export function repoLabel(url) {
  const [owner, repo, , ...rest] = new URL(url).pathname.split('/').filter(Boolean).map(decodeURIComponent);
  return rest.length ? `${owner}/${repo} · ${rest.join('/')}` : `${owner}/${repo}`;
}

/** A project's `files` in order, the ones added from one GitHub address (their
 *  `github.url`) together under a `{ group, repo, files }` heading where the first was. */
export function knowledgeRows(files) {
  const groups = new Map();
  for (const f of files) if (f.github) groups.set(f.github.url, [...(groups.get(f.github.url) || []), f]);
  const rows = [];
  for (const f of files) {
    const g = f.github && groups.get(f.github.url);
    if (!g) rows.push(f);
    else if (g[0] === f) rows.push({ group: f.github.url, repo: repoLabel(f.github.url), files: g }, ...g);
  }
  return rows;
}

/** What a "Sync from GitHub" did (github_sync's answer), as toasts: `[text, kind]`. Nothing that was kept goes unsaid. */
export function syncSummary(r) {
  const names = (list) => list.slice(0, 3).join(', ') + (list.length > 3 ? ', …' : '');
  const out = [];
  if (r.up_to_date) out.push([t('ghsync_current', { repo: r.repo }), '']);
  else if (r.updated.length) out.push([t(r.updated.length === 1 ? 'ghsync_updated_one' : 'ghsync_updated', { repo: r.repo, n: r.updated.length, names: names(r.updated) }), '']);
  else if (r.unchanged) out.push([t('ghsync_unchanged', { repo: r.repo }), '']);
  if (r.missing.length) out.push([t(r.missing.length === 1 ? 'ghsync_missing_one' : 'ghsync_missing', { n: r.missing.length, names: names(r.missing) }), '']);
  if (r.errors.length) out.push([t('ghsync_failed_some', { n: r.errors.length, first: sourceError(r.errors[0], r.errors[0].name) }), 'error']);
  return out;
}

/** The picker. Each file staged for the composer goes to `onAdd(attachment)`;
 *  with `projectId` the files go to that project's knowledge, then `onDone()`. */
export function openGithubSource({ onAdd, projectId, onDone } = {}) {
  const body = h(`<div>
    <div class="field"><label class="label" for="gh-url">${esc(t('ghsrc_url'))}</label>
      <div class="row"><input class="input grow" id="gh-url" spellcheck="false" autocomplete="off" placeholder="${esc(t('ghsrc_url_ph'))}"><button class="btn" data-act="load">${esc(t('ghsrc_load'))}</button></div>
      <div class="help" style="overflow-wrap:anywhere">${esc(t('ghsrc_url_help'))}</div></div>
    <details class="field" data-f="token"><summary class="small" style="cursor:pointer">${icon('key', 13)} ${esc(t('ghsrc_token'))}</summary>
      <div class="help">${esc(t('ghsrc_token_help'))}</div>
      <div class="row" style="margin-top:8px"><input class="input grow" type="password" autocomplete="off" spellcheck="false" aria-label="${esc(t('ghsrc_token'))}" placeholder="github_pat_…"><button class="btn" data-act="save-token">${esc(t('ghsrc_token_save'))}</button></div>
      <div class="row small muted" style="margin-top:6px"><span class="grow" data-f="token-state"></span><button class="btn sm" data-act="clear-token">${esc(t('ghsrc_token_remove'))}</button></div>
    </details>
    <div data-f="result" aria-live="polite"></div>
  </div>`);
  const urlIn = $('#gh-url', body);
  const loadBtn = $('[data-act=load]', body);
  const tokenBox = $('[data-f=token]', body);
  const tokenIn = $('input[type=password]', tokenBox);
  const clearBtn = $('[data-act=clear-token]', tokenBox);
  const result = $('[data-f=result]', body);
  const summary = h('<div class="grow small muted" aria-live="polite"></div>');
  const add = button(t('ghsrc_add'), 'primary', () => addPicked());
  const m = modal({ title: t('add_from_github'), body, footer: [summary, button(t('cancel'), '', () => m.close()), add] });

  // the listing on screen, with its URL and what is picked in it
  let listing = null;
  let picked = () => [];
  let adding = false;
  let seq = 0;

  function refresh() {
    const s = listing ? pickSummary(picked(), listing.limits) : { count: 0, over: null };
    summary.textContent = !listing ? ''
      : s.over === 'files' ? t('ghsrc_over_files', { n: listing.limits.files })
        : s.over === 'bytes' ? t('ghsrc_over_bytes', { size: humanSize(listing.limits.bytes) })
          : t(s.count === 1 ? 'ghsrc_summary_one' : 'ghsrc_summary', { n: s.count, size: humanSize(s.bytes) });
    summary.style.color = s.over ? 'var(--danger)' : '';
    add.disabled = adding || !s.count || !!s.over;
  }

  function showToken(s) {
    $('[data-f=token-state]', tokenBox).textContent = `${t(s.saved ? 'ghsrc_token_saved' : 'ghsrc_token_none')}${s.persists ? '' : ` ${t('ghsrc_token_mem')}`}`;
    show(clearBtn, s.saved);
  }
  async function saveToken(token) {
    try {
      showToken(await call('github_token_set', { token }));
      tokenIn.value = '';
      if (urlIn.value.trim()) load();
    } catch (e) { toast(sourceError(e), 'error'); }
  }
  // a store that can't be read says so, rather than an empty line
  call('github_token_status').then(showToken).catch((e) => { show(clearBtn, false); $('[data-f=token-state]', tokenBox).textContent = sourceError(e); });
  $('[data-act=save-token]', tokenBox).onclick = () => { if (tokenIn.value.trim()) saveToken(tokenIn.value); else tokenIn.focus(); };
  clearBtn.onclick = () => saveToken(null);

  async function load() {
    const url = urlIn.value.trim();
    if (!url) { urlIn.focus(); return; }
    const my = ++seq;
    listing = null;
    refresh();
    loadBtn.disabled = true;
    result.innerHTML = `<div class="row small muted" style="padding:14px 2px"><span class="spinner"></span>${esc(t('ghsrc_loading'))}</div>`;
    try {
      const l = await call('github_list', { url });
      if (my !== seq) return;
      listing = { ...l, url };
      renderTree();
    } catch (e) {
      if (my !== seq) return;
      result.innerHTML = '';
      result.appendChild(sourceNotice(sourceError(e)));
      // what a token can solve: open its box
      if (['github_not_found', 'github_rate_limited', 'github_bad_token', 'github_forbidden'].includes(e.kind)) tokenBox.open = true;
    } finally {
      if (my === seq) loadBtn.disabled = false;
      refresh();
    }
  }

  function renderTree() {
    const l = listing;
    const files = l.files;
    const pad = (depth) => `padding-left:${12 + depth * 18}px`;
    const rows = [];
    const dirFiles = [];
    const dirNames = [];
    const walk = (node, depth) => {
      const mine = [];
      for (const d of [...node.dirs.values()].sort((a, b) => a.name.localeCompare(b.name))) {
        const di = dirFiles.length;
        dirFiles.push([]);
        dirNames.push(d.name);
        const at = rows.push('') - 1;
        const inner = walk(d, depth + 1);
        dirFiles[di] = inner;
        const vendored = inner.every((i) => files[i].vendored);
        rows[at] = `<div class="imp-row" data-d="${di}" ${vendored ? 'data-v' : ''} style="${pad(depth)};cursor:pointer"><button class="icon-btn sm" data-fold="${di}" aria-expanded="true" aria-label="${esc(t('ghsrc_collapse', { x: d.name }))}">${icon('chevDown', 14)}</button><input type="checkbox" data-dc="${di}" aria-label="${esc(d.name)}"><span class="grow"><span class="n">${icon('folder', 14)} ${esc(d.name)}</span></span><span class="badge">${esc(filesN(inner.length))}</span></div><div data-kids="${di}">`;
        rows.push('</div>');
        mine.push(...inner);
      }
      for (const f of node.files) {
        const why = f.skip ? t(f.skip === 'binary' ? 'ghsrc_binary' : 'ghsrc_large') : humanSize(f.size);
        rows.push(`<label class="imp-row ${f.skip ? 'done' : ''}" ${f.vendored ? 'data-v' : ''} style="${pad(depth)}"><span style="width:24px;flex:none"></span><input type="checkbox" data-i="${f.i}" ${f.skip ? 'disabled' : ''}><span class="grow"><span class="n">${esc(f.name)}</span></span><span class="badge">${esc(why)}</span></label>`);
        mine.push(f.i);
      }
      return mine;
    };
    const tree = walk(fileTree(files, l.path), 1);
    const vendoredN = files.filter((f) => f.vendored).length;
    result.innerHTML = `<div class="row small" style="margin:2px 0 8px;flex-wrap:wrap">${icon('branch', 14)}<b>${esc(l.owner)}/${esc(l.repo)}</b><span class="muted">${esc(l.ref || l.sha.slice(0, 7))}${l.path ? ` · ${esc(l.path)}` : ''}</span></div>
      ${l.truncated ? `<div class="notice warn">${icon('info', 15)}<div class="grow small">${esc(t('ghsrc_truncated', { n: files.length }))}</div></div>` : ''}
      ${files.length ? `<div class="imp-list" style="max-height:min(42vh,380px)"><label class="imp-row" style="${pad(0)}"><input type="checkbox" data-all><span class="grow"><span class="n">${esc(t('ghsrc_select_all'))}</span></span><span class="badge" data-all-n></span></label>${rows.join('')}</div>
        ${vendoredN ? `<label class="row small" style="margin-top:8px;cursor:pointer"><input type="checkbox" data-show-v>${esc(t('ghsrc_vendored', { n: vendoredN }))}</label>` : ''}
        <div class="help">${esc(t('ghsrc_limits', { n: l.limits.files, size: humanSize(l.limits.bytes), file: humanSize(l.limits.file_bytes) }))}</div>`
    : `<div class="empty-state" style="padding:24px 8px">${esc(t('ghsrc_empty'))}</div>`}`;
    if (!files.length) return;
    const list = $('.imp-list', result);
    const boxOf = new Map([...list.querySelectorAll('input[data-i]')].map((b) => [Number(b.dataset.i), b]));
    const dirBoxes = [...list.querySelectorAll('input[data-dc]')];
    const allBox = $('input[data-all]', list);
    let showVendored = false;
    const pickable = (i) => !files[i].skip && (showVendored || !files[i].vendored);
    const setFiles = (idx, on) => { for (const i of idx) if (pickable(i)) boxOf.get(i).checked = on; };
    const mark = (box, idx) => {
      const can = idx.filter(pickable);
      const n = can.filter((i) => boxOf.get(i).checked).length;
      box.disabled = !can.length;
      box.checked = can.length > 0 && n === can.length;
      box.indeterminate = n > 0 && n < can.length;
    };
    const sync = () => {
      dirBoxes.forEach((b) => mark(b, dirFiles[b.dataset.dc]));
      mark(allBox, tree);
      $('[data-all-n]', list).textContent = filesN(tree.filter((i) => showVendored || !files[i].vendored).length);
      refresh();
    };
    picked = () => files.filter((f, i) => pickable(i) && boxOf.get(i).checked);
    list.addEventListener('change', (e) => {
      const x = e.target;
      if (x.dataset.dc !== undefined) setFiles(dirFiles[x.dataset.dc], x.checked);
      else if (x.dataset.all !== undefined) setFiles(tree, x.checked);
      sync();
    });
    // a folder's row (but its box) folds it
    list.addEventListener('click', (e) => {
      const row = e.target.closest('[data-d]');
      if (!row || e.target.matches('input')) return;
      const b = $('[data-fold]', row);
      const open = b.getAttribute('aria-expanded') !== 'true';
      b.setAttribute('aria-expanded', String(open));
      b.setAttribute('aria-label', t(open ? 'ghsrc_collapse' : 'ghsrc_expand', { x: dirNames[b.dataset.fold] }));
      b.innerHTML = icon(open ? 'chevDown' : 'chevRight', 14);
      show($(`[data-kids="${b.dataset.fold}"]`, list), open);
    });
    const vendoredRows = [...list.querySelectorAll('[data-v]')];
    vendoredRows.forEach((r) => show(r, false));
    const showV = $('input[data-show-v]', result);
    if (showV) showV.addEventListener('change', () => { showVendored = showV.checked; vendoredRows.forEach((r) => show(r, showVendored)); sync(); });
    // everything picked when it fits the caps
    if (!pickSummary(files.filter((f, i) => pickable(i)), l.limits).over) setFiles(tree, true);
    sync();
  }

  async function addPicked() {
    const l = listing;
    const paths = picked().map((f) => f.path);
    if (!l || !paths.length || adding) return;
    adding = true;
    add.textContent = t('ghsrc_adding');
    refresh();
    try {
      const r = await call('github_add', { url: l.url, sha: l.sha, paths, projectId: projectId || null });
      if (onAdd) r.added.forEach(onAdd);
      const repo = `${l.owner}/${l.repo}`;
      if (r.added.length) toast(t(r.added.length === 1 ? 'ghsrc_added_one' : 'ghsrc_added', { n: r.added.length, repo }));
      if (r.errors.length) toast(t('ghsrc_failed_some', { n: r.errors.length, first: sourceError(r.errors[0], r.errors[0].name) }), 'error');
      if (r.added.length) { m.close(); if (onDone) onDone(); }
    } catch (e) {
      toast(sourceError(e), 'error');
    } finally {
      adding = false;
      add.textContent = t('ghsrc_add');
      refresh();
    }
  }

  loadBtn.onclick = load;
  const onEnter = (el, fn) => el.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.isComposing) { e.preventDefault(); fn(); } });
  onEnter(urlIn, load);
  onEnter(tokenIn, () => $('[data-act=save-token]', tokenBox).click());
  refresh();
  setTimeout(() => urlIn.focus(), 30);
}
