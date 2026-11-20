// Settings → Import: bring data over from the official Claude apps.
import { icon } from './icons.js';
import { t } from './i18n.js';
import { call, on as onBackend } from './api.js';
import { emit } from './store.js';
import { h, esc, button, errorToast, relTime } from './util.js';
import { refreshProjects } from './projects.js';

function section(title, desc) {
  const s = h(`<div class="imp-section"><div class="imp-head"><div class="t">${esc(title)}</div>${desc ? `<div class="d">${esc(desc)}</div>` : ''}</div><div class="imp-body"></div></div>`);
  return [s, s.querySelector('.imp-body')];
}

function errorsBox(errors, label) {
  if (!errors || !errors.length) return null;
  const items = errors.slice(0, 20).map((e) => `<div class="mono small">${esc(e.title || e.path || e.id || '')}: ${esc(e.error)}</div>`).join('');
  return h(`<div class="notice error" style="margin-top:10px">${icon('alert', 15)}<div class="grow"><div>${esc(t('imp_errors', { n: errors.length }))}</div>${items}</div></div>`);
}

/** Run an import while showing "Importing… n/total" on `label`. */
async function withProgress(label, fn) {
  const unlisten = await onBackend('import-progress', (p) => {
    if (p && p.total) label.textContent = t('imp_progress', { done: p.done, total: p.total });
  });
  try { return await fn(); } finally { unlisten(); }
}

export async function renderImport(body) {
  body.appendChild(h(`<h3>${esc(t('imp_title'))}</h3>`));
  body.appendChild(h(`<p class="help" style="margin:-8px 0 16px">${esc(t('imp_desc'))}</p>`));

  // --- claude.ai export --------------------------------------------------
  const [chats, cb] = section(t('imp_chats'), t('imp_chats_d'));
  const status = h('<div class="small muted imp-status"></div>');
  const actions = h('<div class="imp-actions"></div>');
  const choose = button(t('imp_choose'), 'sm accent');
  let exportUrl = 'https://claude.ai/settings/data-privacy-controls';
  const req = button(t('imp_request'), 'sm ghost', () => call('open_external', { url: exportUrl }).catch(errorToast));
  req.insertAdjacentHTML('beforeend', ' ' + icon('external', 12));
  actions.append(choose, req);
  cb.append(actions, status);
  const runExport = async (path) => {
    if (choose.disabled) return;
    choose.disabled = true;
    status.textContent = t('imp_progress', { done: 0, total: '…' });
    cb.querySelectorAll('.notice').forEach((n) => n.remove());
    try {
      const r = await withProgress(status, () => call('import_claude_export', { path }));
      status.textContent = t('imp_result_proj', { n: r.imported, p: r.projects, s: r.skipped });
      const eb = errorsBox(r.errors);
      if (eb) cb.appendChild(eb);
      emit('conversations-changed');
      refreshProjects();
    } catch (e) {
      status.textContent = '';
      errorToast(e);
    } finally { choose.disabled = false; }
  };
  choose.onclick = async () => {
    let path;
    try { path = await call('pick_import_file'); } catch (e) { errorToast(e); return; }
    if (path) runExport(path);
  };
  // Dropping the export file anywhere on this tab imports it too.
  const unlistenDrop = onBackend('tauri://drag-drop', (p) => {
    if (!chats.isConnected) { unlistenDrop.then((u) => u()); return; }
    const path = p && p.paths && p.paths.find((x) => /\.(zip|json)$/i.test(x));
    if (path) runExport(path);
  });
  body.appendChild(chats);

  // --- local data (scan) -------------------------------------------------
  const scanning = h(`<div class="small muted" style="padding:12px 0">${esc(t('imp_scanning'))}</div>`);
  body.appendChild(scanning);
  let scan;
  try { scan = await call('import_scan'); } catch (e) { scanning.textContent = e.message; return; }
  scanning.remove();
  if (scan.export_url) exportUrl = scan.export_url;

  body.appendChild(ccSessions(scan.claude_code));
  body.appendChild(desktopConnectors(scan.desktop));
  body.appendChild(serverImport(t('imp_cc_mcp'), t('imp_cc_mcp_d'), 'claude-code', scan.claude_code.mcp_servers, scan.claude_code.found));
}

function ccSessions(cc) {
  const [sec, b] = section(t('imp_cc'), t('imp_cc_d'));
  const sessions = cc.sessions || [];
  if (!sessions.length) { b.appendChild(h(`<div class="small muted">${esc(t('imp_none'))}</div>`)); return sec; }
  const list = h('<div class="imp-list"></div>');
  const picked = new Set();
  const go = button('', 'sm accent');
  const all = button(t('imp_select_new'), 'sm ghost');
  const status = h('<div class="small muted imp-status"></div>');
  const sync = () => {
    go.textContent = t('imp_import_n', { n: picked.size });
    go.disabled = picked.size === 0;
  };
  for (const s of sessions) {
    const folder = (s.cwd || '').split(/[\\/]/).filter(Boolean).pop() || s.cwd || '';
    const row = h(`<label class="imp-row ${s.imported ? 'done' : ''}">
      <input type="checkbox" ${s.imported ? 'disabled' : ''}>
      <span class="grow"><span class="n ellipsis">${esc(s.title || s.session_id)}</span>
        <span class="m small muted ellipsis">${icon('folder', 12)} <span title="${esc(s.cwd || '')}">${esc(folder)}</span> · ${esc(relTime(s.updated_at))} · ${esc(t(s.prompts === 1 ? 'imp_prompt_one' : 'imp_prompts', { n: s.prompts }))}</span></span>
      ${s.imported ? `<span class="badge">${esc(t('imp_imported'))}</span>` : ''}</label>`);
    const cbx = row.querySelector('input');
    cbx.addEventListener('change', () => { if (cbx.checked) picked.add(s.path); else picked.delete(s.path); sync(); });
    list.appendChild(row);
  }
  all.onclick = () => {
    list.querySelectorAll('.imp-row:not(.done) input').forEach((x) => { x.checked = true; });
    sessions.filter((s) => !s.imported).forEach((s) => picked.add(s.path));
    sync();
  };
  go.onclick = async () => {
    if (!picked.size) return;
    go.disabled = true; all.disabled = true;
    b.querySelectorAll('.notice').forEach((n) => n.remove());
    try {
      const paths = [...picked];
      const r = await withProgress(status, () => call('import_claude_code_sessions', { paths }));
      status.textContent = t('imp_result', { n: r.imported, s: r.skipped });
      const eb = errorsBox(r.errors);
      if (eb) b.appendChild(eb);
      const failed = new Set((r.errors || []).map((e) => e.path));
      list.querySelectorAll('.imp-row').forEach((row, i) => {
        const s = sessions[i];
        if (picked.has(s.path) && !failed.has(s.path)) {
          s.imported = true;
          row.classList.add('done');
          const x = row.querySelector('input'); x.checked = false; x.disabled = true;
          if (!row.querySelector('.badge')) row.insertAdjacentHTML('beforeend', `<span class="badge">${esc(t('imp_imported'))}</span>`);
        }
      });
      picked.clear();
      emit('conversations-changed');
    } catch (e) { errorToast(e); } finally { all.disabled = false; sync(); }
  };
  sync();
  b.appendChild(list);
  if (cc.total_sessions > sessions.length) b.appendChild(h(`<div class="small muted" style="margin-top:6px">${esc(t('imp_more', { n: sessions.length, total: cc.total_sessions }))}</div>`));
  const actions = h('<div class="imp-actions"></div>');
  actions.append(go, all);
  b.append(actions, status);
  return sec;
}

function serverRows(servers) {
  const list = h('<div class="imp-list compact"></div>');
  for (const s of servers) {
    list.appendChild(h(`<div class="imp-row ${s.exists ? 'done' : ''}">${icon('plug', 14)}<span class="grow"><span class="n">${esc(s.name)}</span></span>
      <span class="small muted">${esc(s.transport)}</span>${s.exists ? `<span class="badge">${esc(t('imp_exists'))}</span>` : ''}</div>`));
  }
  return list;
}

function serverImport(title, desc, source, servers, found, sec, b) {
  if (!sec) [sec, b] = section(title, desc);
  servers = servers || [];
  if (!found || !servers.length) {
    b.appendChild(h(`<div class="small muted">${esc(t('imp_none'))}</div>`));
    return sec;
  }
  b.appendChild(serverRows(servers));
  const fresh = servers.filter((s) => !s.exists);
  const status = h('<div class="small muted imp-status"></div>');
  const go = button(fresh.length ? t('imp_import_n', { n: fresh.length }) : t('imp_all_added'), 'sm accent');
  go.disabled = !fresh.length;
  go.onclick = async () => {
    go.disabled = true;
    try {
      const r = await call('import_mcp_servers', { source, names: fresh.map((s) => s.name) });
      status.textContent = t('imp_servers_added', { n: r.imported });
      b.querySelectorAll('.imp-row').forEach((row) => {
        row.classList.add('done');
        if (!row.querySelector('.badge')) row.insertAdjacentHTML('beforeend', `<span class="badge">${esc(t('imp_exists'))}</span>`);
      });
      go.textContent = t('imp_all_added');
    } catch (e) { errorToast(e); go.disabled = false; }
  };
  const actions = h('<div class="imp-actions"></div>');
  actions.appendChild(go);
  b.append(actions, status);
  return sec;
}

function desktopConnectors(d) {
  const [sec, b] = section(t('imp_desktop'), t('imp_desktop_d'));
  if (!d.found) { b.appendChild(h(`<div class="small muted">${esc(t('imp_none'))}</div>`)); return sec; }
  b.appendChild(h(`<div class="imp-sub">${esc(t('imp_servers'))}</div>`));
  const sb = h('<div></div>');
  b.appendChild(sb);
  serverImport('', '', 'desktop', d.mcp_servers, true, sec, sb);

  b.appendChild(h(`<div class="imp-sub">${esc(t('imp_extensions'))}</div>`));
  const exts = d.extensions || [];
  if (!exts.length) { b.appendChild(h(`<div class="small muted">${esc(t('imp_none'))}</div>`)); return sec; }
  const list = h('<div class="imp-list compact"></div>');
  const picked = new Set();
  const go = button('', 'sm accent');
  const status = h('<div class="small muted imp-status"></div>');
  const sync = () => { go.textContent = picked.size ? t('imp_import_n', { n: picked.size }) : t('imp_all_added'); go.disabled = !picked.size; };
  for (const x of exts) {
    const row = h(`<label class="imp-row ${x.installed ? 'done' : ''}"><input type="checkbox" ${x.installed ? 'disabled' : 'checked'}>
      <span class="grow"><span class="n">${esc(x.name)} <span class="small muted" style="font-weight:400">${esc(x.version || '')}</span></span>
      ${x.description ? `<span class="m small muted ellipsis">${esc(x.description)}</span>` : ''}</span>
      ${x.installed ? `<span class="badge">${esc(t('imp_exists'))}</span>` : x.enabled ? '' : `<span class="badge muted">${esc(t('imp_off_there'))}</span>`}</label>`);
    if (!x.installed) picked.add(x.id);
    row.querySelector('input').addEventListener('change', (e) => { if (e.target.checked) picked.add(x.id); else picked.delete(x.id); sync(); });
    list.appendChild(row);
  }
  go.onclick = async () => {
    go.disabled = true;
    b.querySelectorAll('.notice').forEach((n) => n.remove());
    try {
      const r = await call('import_desktop_extensions', { ids: [...picked] });
      status.textContent = t('imp_ext_added', { n: (r.installed || []).length });
      const eb = errorsBox(r.errors);
      if (eb) b.appendChild(eb);
      const failed = new Set((r.errors || []).map((e) => e.id));
      list.querySelectorAll('.imp-row').forEach((row, i) => {
        const x = exts[i];
        if (picked.has(x.id) && !failed.has(x.id)) {
          row.classList.add('done');
          const c = row.querySelector('input'); c.checked = false; c.disabled = true;
          picked.delete(x.id);
        }
      });
    } catch (e) { errorToast(e); } finally { sync(); }
  };
  sync();
  const actions = h('<div class="imp-actions"></div>');
  actions.appendChild(go);
  b.append(list, actions, status);
  return sec;
}
