// Desktop extensions (Settings → Developer): install a .mcpb/.dxt after a
// preview (who signed it, what it runs, its settings form), the Node.js
// runtime the node ones need, and on an installed one's card its version,
// Reconfigure (the same form filled in, secrets never shown) and Uninstall.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { call, on as onBackend } from './api.js';
import { esc, h, modal, button, toast, errorToast, confirmDialog, humanSize } from './util.js';
import { errorText } from './neterr.js';

/** Server `name` of `status` (mcp_status's reply) as an installed extension
 *  ({name, version, display_name}, from its config entry; `runtime` when its
 *  server couldn't start without one), or null. */
export function installedExtension(status, name) {
  const x = ((((status && status.config) || {}).mcpServers || {})[name] || {}).extension;
  if (!(x && typeof x === 'object' && x.path)) return null;
  const runtime = (((status && status.servers) || []).find((s) => s.name === name) || {}).runtime;
  return { name, version: x.version, display_name: x.display_name, ...(runtime ? { runtime } : {}) };
}

/** The value a settings field gives: `raw` is the field's text (or a checkbox's state).
 *  A secret left empty while one is saved is left out (undefined): the saved one stays. */
export function fieldValue(def, raw, savedSecret = false) {
  const type = def.type || 'string';
  if (type === 'boolean') return !!raw;
  const v = String(raw ?? '').trim();
  if (def.sensitive && savedSecret && v === '') return undefined;
  if (def.multiple) return v ? v.split('\n').map((x) => x.trim()).filter(Boolean) : null;
  if (type === 'number') return v === '' ? null : Number(v);
  return v === '' ? null : v;
}

/** The first required setting of `spec` that `values` (the form's) leaves empty
 *  with no default and no saved secret: its title, or null. */
export function missingRequired(spec, values, saved = []) {
  for (const [key, def] of Object.entries(spec || {})) {
    const v = values[key];
    const empty = v === undefined || v === null || v === '' || (Array.isArray(v) && !v.length);
    if (def.required && empty && def.default == null && !saved.includes(key)) return def.title || key;
  }
  return null;
}

/** The preview's line about a bundle's signature (extension_sign.rs's check):
 *  {cls, text, confirm}, `confirm` when installing it needs an explicit yes. */
export function signatureLine(sig) {
  const s = (sig && sig.status) || 'unsigned';
  const who = sig && sig.publisher;
  if (s === 'trusted') return { cls: 'ok', text: t('ext_sig_trusted', { who }), confirm: false };
  if (s === 'invalid') return { cls: 'error', text: t('ext_sig_invalid'), confirm: false };
  if (s === 'unverifiable') return { cls: 'warn', text: who ? t('ext_sig_unverifiable', { who }) : t('ext_sig_unverifiable_anon'), confirm: true };
  if (s === 'self_signed' || s === 'untrusted') return { cls: 'warn', text: t(`ext_sig_${s}`, { who }), confirm: true };
  return { cls: 'warn', text: t('ext_sig_unsigned'), confirm: true };
}

/** The form of a manifest's user_config `spec` into `body`, filled from `values`
 *  (else the defaults); `saved`: keys of secrets already stored. Returns a reader. */
function configForm(body, spec, values = null, saved = []) {
  const read = {};
  for (const [key, def] of Object.entries(spec || {})) {
    const type = def.type || 'string';
    const has = values && Object.prototype.hasOwnProperty.call(values, key) && values[key] != null;
    const cur = has ? values[key] : def.default;
    const secret = !!def.sensitive && saved.includes(key);
    let el;
    if (type === 'boolean') {
      el = h(`<label class="switch"><input type="checkbox" ${cur ? 'checked' : ''} aria-label="${esc(def.title || key)}"><span class="track"></span></label>`);
      read[key] = () => fieldValue(def, el.querySelector('input').checked);
    } else {
      el = def.multiple
        ? h(`<textarea class="textarea mono" rows="3" placeholder="${esc(t('ext_one_per_line'))}" aria-label="${esc(def.title || key)}"></textarea>`)
        : h(`<input class="input ${def.sensitive ? 'mono' : ''}" type="${def.sensitive ? 'password' : type === 'number' ? 'number' : 'text'}" autocomplete="off" aria-label="${esc(def.title || key)}">`);
      el.value = def.sensitive && secret ? '' : Array.isArray(cur) ? cur.join('\n') : (cur ?? '');
      if (secret) el.placeholder = t('ext_secret_saved');
      read[key] = () => fieldValue(def, el.value, secret);
    }
    const f = h(`<div class="field"><label class="label">${esc(def.title || key)}${def.required ? ' *' : ''}</label></div>`);
    f.appendChild(el);
    if (def.description) f.appendChild(h(`<div class="help">${esc(def.description)}</div>`));
    body.appendChild(f);
  }
  return () => {
    const out = {};
    for (const [k, get] of Object.entries(read)) { const v = get(); if (v !== undefined) out[k] = v; }
    return out;
  };
}

// ---------------------------------------------------------------- Node.js

/** "Download Node.js …?" with a Download button, its progress and Cancel;
 *  `done(status)` once it is downloaded (or, when it was there already, on
 *  "Restart servers"). Draws itself into the element it returns. */
export function nodeRuntimePrompt(done) {
  const el = h('<div class="notice warn ext-node" role="status"></div>');
  let rt = null;
  let progress = null;
  const draw = (phase, err = '') => {
    el.className = 'notice warn ext-node';
    if (phase === 'downloading') {
      const { downloaded = 0, total = null } = progress || {};
      const pct = total ? Math.min(100, Math.round((downloaded / total) * 100)) : null;
      el.innerHTML = `${icon('download', 15)}<div class="grow small"><div>${esc(t('node_downloading', { v: rt.version }))} <span class="muted amount">${esc(total ? `${humanSize(downloaded)} / ${humanSize(total)}` : humanSize(downloaded))}</span></div>
        <div class="upd-bar ${pct === null ? 'indeterminate' : ''}" role="progressbar" aria-valuemin="0" aria-valuemax="100" ${pct === null ? '' : `aria-valuenow="${pct}"`}><span style="width:${pct === null ? 30 : pct}%"></span></div>
        <div class="actions"><button class="btn sm" data-a="cancel">${esc(t('cancel'))}</button></div></div>`;
    } else if (phase === 'ready' || phase === 'there') {
      el.className = 'notice ext-node';
      el.innerHTML = `${icon('check', 15)}<div class="grow small">${esc(rt.installed ? t('node_ready', { v: rt.version }) : t('node_own'))}
        ${phase === 'there' && done ? `<div class="actions"><button class="btn sm" data-a="restart">${esc(t('reload'))}</button></div>` : ''}</div>`;
    } else if (!rt.supported) {
      el.innerHTML = `${icon('alert', 15)}<div class="grow small">${esc(t('node_unsupported_t'))}</div>`;
    } else {
      el.innerHTML = `${icon(err ? 'alert' : 'download', 15)}<div class="grow small"><div>${esc(t('node_prompt', { v: rt.version, mb: rt.size_mb }))}</div>
        <div class="muted" style="margin-top:2px">${esc(t('node_verify_note'))}</div>
        ${err ? `<div class="ext-err" role="alert" style="color:var(--danger);margin-top:6px">${esc(err)}</div>` : ''}
        <div class="actions"><button class="btn sm primary" data-a="get">${esc(err ? t('node_retry') : t('node_download'))}</button></div></div>`;
    }
    const get = el.querySelector('[data-a=get]');
    if (get) get.onclick = start;
    const cancel = el.querySelector('[data-a=cancel]');
    if (cancel) cancel.onclick = () => call('node_runtime_cancel').catch(errorToast);
    const restart = el.querySelector('[data-a=restart]');
    if (restart) restart.onclick = () => done(rt);
  };
  const start = async () => {
    progress = null;
    draw('downloading');
    // moved in place, so a focused Cancel stays focused
    const off = await onBackend('node-runtime-progress', (p) => {
      progress = p;
      const bar = el.querySelector('.upd-bar');
      if (!bar || !p.total) { draw('downloading'); return; }
      const pct = Math.min(100, Math.round((p.downloaded / p.total) * 100));
      bar.classList.remove('indeterminate');
      bar.setAttribute('aria-valuenow', String(pct));
      bar.querySelector('span').style.width = `${pct}%`;
      el.querySelector('.amount').textContent = `${humanSize(p.downloaded)} / ${humanSize(p.total)}`;
    });
    try {
      rt = await call('node_runtime_install');
      draw('ready');
      done && done(rt);
    } catch (e) {
      draw('offer', e && e.kind === 'cancelled' ? t('node_cancelled') : errorText(e));
    } finally { off(); }
  };
  call('node_runtime_status').then((r) => { rt = r; draw(r.installed || r.on_path ? 'there' : 'offer'); }).catch((e) => { el.textContent = errorText(e); });
  return el;
}

/** The "Node.js for extensions" row of Settings → Developer: which node the
 *  extensions use, Download when there's none, Remove for the downloaded one. */
export function nodeRuntimeRow() {
  const el = h(`<div class="setting-row ext-node-row"><div class="txt"><div class="t">${esc(t('node_title'))}</div><div class="d" aria-live="polite"></div></div><div class="ctl row" style="gap:6px"></div></div>`);
  const d = el.querySelector('.d');
  const ctl = el.querySelector('.ctl');
  const draw = (rt) => {
    ctl.replaceChildren();
    if (rt.installed) {
      d.textContent = t('node_installed', { v: rt.version });
      d.title = rt.installed;
      ctl.appendChild(button(t('node_remove'), 'sm ghost', async () => {
        if (!(await confirmDialog(t('node_remove_confirm', { v: rt.version }), { okLabel: t('node_remove') }))) return;
        try { draw(await call('node_runtime_remove')); toast(t('node_removed')); } catch (e) { toast(errorText(e), 'error'); }
      }));
    } else {
      d.textContent = rt.on_path ? t('node_own') : t('node_none_missing');
      if (!rt.on_path && rt.supported) {
        ctl.appendChild(button(t('node_download_mb', { mb: rt.size_mb }), 'sm', () => { modal({ title: t('node_title'), body: nodeRuntimePrompt(draw) }); }));
      }
    }
  };
  call('node_runtime_status').then(draw).catch((e) => { d.textContent = errorText(e); });
  return el;
}

// ------------------------------------------------------------- the preview

// the facts of the preview, two columns (labels, values that may wrap anywhere: hashes, paths)
const FACTS = 'display:grid;grid-template-columns:max-content minmax(0,1fr);gap:4px 12px;margin:0 0 12px';
const DT = 'color:var(--text-2)';
const DD = 'margin:0;min-width:0;overflow-wrap:anywhere';

/** What a missing runtime (inspect's `runtime_missing`, a server's `runtime`) asks of the user. */
function runtimeNotice(runtime, done) {
  if (runtime === 'node') return nodeRuntimePrompt(done);
  return h(`<div class="notice warn">${icon('alert', 15)}<div class="grow small">${esc(t(`ext_needs_${runtime}`))}</div></div>`);
}

/** The install preview of bundle `info` (extension_inspect's reply, plus
 *  `entry`/`source_url` for one from the directory); `install(values)` installs it.
 *  For an update, `saved` (extension_settings' reply) fills the form in. */
export function previewDialog(info, install, saved = null) {
  const entry = info.entry || null;
  const sig = signatureLine(info.signature);
  const body = h(`<div class="ext-preview">
    <p style="margin:0 0 4px;font-weight:600">${esc(info.display_name || info.name)} <span class="muted small">${esc(info.version || '')}</span></p>
    ${info.author || entry ? `<p class="small muted" style="margin:0 0 10px">${esc(entry ? t('ext_from_directory', { publisher: entry.publisher }) : info.author)}</p>` : ''}
    ${info.description ? `<p class="small" style="margin:0 0 12px">${esc(info.description)}</p>` : ''}
    <div class="notice ${sig.cls === 'ok' ? '' : sig.cls}">${icon(sig.cls === 'ok' ? 'check' : 'alert', 15)}<div class="grow small">${esc(sig.text)}</div></div>
    ${info.compatible === false ? `<div class="notice warn">${icon('alert', 15)}<div class="grow small">${esc(t('ext_incompatible'))}</div></div>` : ''}
    <dl class="ext-facts small" style="${FACTS}">
      <dt style="${DT}">${esc(t('ext_runs'))}</dt><dd style="${DD}"><code class="mono">${esc(info.run || '?')}</code><div class="muted">${esc(t('ext_runs_note'))}</div></dd>
      ${(info.env || []).length ? `<dt style="${DT}">${esc(t('ext_env'))}</dt><dd class="mono" style="${DD}">${esc(info.env.join(', '))}</dd>` : ''}
      ${Array.isArray(info.tools) && info.tools.length ? `<dt style="${DT}">${esc(t('ext_tools'))}</dt><dd style="${DD}">${esc(info.tools.map((x) => x && x.name).filter(Boolean).join(', '))}</dd>` : ''}
      ${info.source_url ? `<dt style="${DT}">${esc(t('ext_source'))}</dt><dd class="mono" style="${DD}">${esc(info.source_url)}</dd>` : ''}
      ${entry && entry.license ? `<dt style="${DT}">${esc(t('ext_license'))}</dt><dd style="${DD}">${esc(entry.license)}</dd>` : ''}
      <dt style="${DT}">SHA-256</dt><dd class="mono" style="${DD}">${esc(info.sha256 || '')}${info.size ? ` <span class="muted">(${esc(humanSize(info.size))})</span>` : ''}</dd>
      ${info.signature && info.signature.fingerprint ? `<dt style="${DT}">${esc(t('ext_cert'))}</dt><dd class="mono" style="${DD}">${esc(info.signature.fingerprint)}</dd>` : ''}
    </dl></div>`);
  if (info.runtime_missing) body.appendChild(runtimeNotice(info.runtime_missing));
  const secrets = (saved && saved.secrets_set) || [];
  const values = configForm(body, info.user_config, saved && saved.values, secrets);
  let trusted = !sig.confirm;
  if (sig.confirm) {
    const c = h(`<label class="ext-confirm small" style="display:flex;gap:8px;align-items:flex-start;margin-top:12px"><input type="checkbox"> <span>${esc(t('ext_trust_confirm'))}</span></label>`);
    c.querySelector('input').onchange = (e) => { trusted = e.target.checked; ok.disabled = !trusted; };
    body.appendChild(c);
  }
  const err = h('<div class="ext-err small" role="alert" style="color:var(--danger);margin-top:10px" hidden></div>');
  body.appendChild(err);
  const fail = (msg) => { err.textContent = msg; err.hidden = !msg; };
  const label = saved ? t('ed_update', { v: info.version || '?' }) : t('install');
  const ok = button(label, 'primary', async () => {
    const v = values();
    const missing = missingRequired(info.user_config, v, secrets);
    if (missing) { fail(t('ext_required_missing', { field: missing })); return; }
    fail('');
    ok.disabled = true;
    try { await install(v); m.close(); } catch (e) { fail(errorText(e)); ok.disabled = !trusted; }
  });
  ok.disabled = !trusted || (info.signature && info.signature.status === 'invalid');
  const m = modal({ title: saved ? label : t('install_extension'), body, wide: true, footer: [button(t('cancel'), '', () => m.close()), ok] });
  return m;
}

export async function installExtension(redraw) {
  let path;
  try { path = await call('pick_extension'); } catch (e) { errorToast(e); return; }
  if (!path) return;
  let info;
  try { info = await call('extension_inspect', { path }); } catch (e) { toast(errorText(e), 'error'); return; }
  previewDialog(info, async (values) => {
    const r = await call('extension_install', { path, values });
    toast(t('extension_installed', { name: r.installed }));
    redraw(r);
  });
}

async function reconfigure(ext, redraw) {
  let cfg;
  try { cfg = await call('extension_settings', { name: ext.name }); } catch (e) { errorToast(e); return; }
  const spec = cfg.user_config || {};
  const body = h(`<div><p style="margin:0 0 14px;font-weight:600">${esc(ext.display_name || ext.name)} <span class="muted small">${esc(ext.version || '')}</span></p></div>`);
  if (!Object.keys(spec).length) body.appendChild(h(`<p class="small muted">${esc(t('ext_no_settings'))}</p>`));
  const values = configForm(body, spec, cfg.values || {}, cfg.secrets_set || []);
  const err = h('<div class="ext-err small" role="alert" style="color:var(--danger);margin-top:10px" hidden></div>');
  body.appendChild(err);
  const saveB = button(t('save_restart'), 'primary', async () => {
    const v = values();
    const missing = missingRequired(spec, v, cfg.secrets_set || []);
    if (missing) { err.textContent = t('ext_required_missing', { field: missing }); err.hidden = false; return; }
    saveB.disabled = true;
    try { const r = await call('extension_reconfigure', { name: ext.name, values: v }); toast(t('config_saved')); m.close(); redraw(r); } catch (e) { err.textContent = errorText(e); err.hidden = false; saveB.disabled = false; }
  });
  const m = modal({ title: t('ext_reconfigure'), body, footer: [button(t('cancel'), '', () => m.close()), saveB] });
}

/** Version, Reconfigure and Uninstall for installed extension `ext` (see
 *  installedExtension), and what to do when its runtime is missing; with `update`
 *  ({version, run(button)}: the directory lists a newer version) its badge and button. */
export function extensionControls(ext, redraw, update = null) {
  const el = h(`<div class="ext-ctl" style="margin-top:8px"><div class="row" style="gap:6px;flex-wrap:wrap">
    <span class="cd-cat">${icon('download', 12)} ${esc(t('ext_version', { v: ext.version || '?' }))}</span>
    ${update ? `<span class="cd-cat ext-upd">${esc(t('ed_update_available'))}</span><button class="btn sm primary" data-a="upd">${esc(t('ed_update', { v: update.version }))}</button>` : ''}
    <button class="btn sm ghost" data-a="conf">${esc(t('ext_reconfigure'))}</button>
    <button class="btn sm ghost" data-a="rm">${esc(t('ext_uninstall'))}</button></div></div>`);
  if (ext.runtime) el.appendChild(runtimeNotice(ext.runtime, async () => { try { redraw(await call('mcp_reload')); } catch (e) { errorToast(e); } }));
  el.querySelector('[data-a=conf]').onclick = () => reconfigure(ext, redraw);
  if (update) el.querySelector('[data-a=upd]').onclick = (ev) => update.run(ev.currentTarget);
  el.querySelector('[data-a=rm]').onclick = async () => {
    if (!(await confirmDialog(t('ext_uninstall_confirm', { name: ext.display_name || ext.name }), { okLabel: t('ext_uninstall') }))) return;
    try { redraw(await call('extension_uninstall', { name: ext.name })); toast(t('ext_uninstalled', { name: ext.display_name || ext.name })); } catch (e) { errorToast(e); }
  };
  return el;
}
