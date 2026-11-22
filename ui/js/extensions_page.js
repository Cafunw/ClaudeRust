// Desktop extensions (Settings → Developer): install a .mcpb/.dxt with its
// settings form, and on an installed one's card its version, Reconfigure
// (the same form filled in, secrets never shown) and Uninstall.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { call } from './api.js';
import { esc, h, modal, button, toast, errorToast, confirmDialog } from './util.js';

/** Server `name` of `status` (mcp_status's reply) as an installed extension
 *  ({name, version, display_name}, from its config entry), or null. */
export function installedExtension(status, name) {
  const x = ((((status && status.config) || {}).mcpServers || {})[name] || {}).extension;
  return x && typeof x === 'object' && x.path ? { name, version: x.version, display_name: x.display_name } : null;
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
      el = h(`<label class="switch"><input type="checkbox" ${cur ? 'checked' : ''}><span class="track"></span></label>`);
      read[key] = () => fieldValue(def, el.querySelector('input').checked);
    } else {
      el = def.multiple
        ? h(`<textarea class="textarea mono" rows="3" placeholder="${esc(t('ext_one_per_line'))}"></textarea>`)
        : h(`<input class="input ${def.sensitive ? 'mono' : ''}" type="${def.sensitive ? 'password' : type === 'number' ? 'number' : 'text'}" autocomplete="off">`);
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

export async function installExtension(redraw) {
  let path;
  try { path = await call('pick_extension'); } catch (e) { errorToast(e); return; }
  if (!path) return;
  let info;
  try { info = await call('extension_inspect', { path }); } catch (e) { errorToast(e); return; }
  const body = h(`<div><p style="margin:0 0 4px;font-weight:600">${esc(info.display_name || info.name)} <span class="muted small">${esc(info.version || '')}</span></p>
    ${info.author ? `<p class="small muted" style="margin:0 0 10px">${esc(info.author)}</p>` : ''}
    ${info.description ? `<p class="small" style="margin:0 0 14px">${esc(info.description)}</p>` : ''}</div>`);
  const values = configForm(body, info.user_config);
  const install = button(t('install'), 'primary', async () => {
    install.disabled = true;
    try {
      const r = await call('extension_install', { path, values: values() });
      toast(t('extension_installed', { name: r.installed }));
      m.close();
      redraw(r);
    } catch (e) { errorToast(e); install.disabled = false; }
  });
  const m = modal({ title: t('install_extension'), body, footer: [button(t('cancel'), '', () => m.close()), install] });
}

async function reconfigure(ext, redraw) {
  let cfg;
  try { cfg = await call('extension_settings', { name: ext.name }); } catch (e) { errorToast(e); return; }
  const spec = cfg.user_config || {};
  const body = h(`<div><p style="margin:0 0 14px;font-weight:600">${esc(ext.display_name || ext.name)} <span class="muted small">${esc(ext.version || '')}</span></p></div>`);
  if (!Object.keys(spec).length) body.appendChild(h(`<p class="small muted">${esc(t('ext_no_settings'))}</p>`));
  const values = configForm(body, spec, cfg.values || {}, cfg.secrets_set || []);
  const saveB = button(t('save_restart'), 'primary', async () => {
    saveB.disabled = true;
    try { const r = await call('extension_reconfigure', { name: ext.name, values: values() }); toast(t('config_saved')); m.close(); redraw(r); } catch (e) { errorToast(e); saveB.disabled = false; }
  });
  const m = modal({ title: t('ext_reconfigure'), body, footer: [button(t('cancel'), '', () => m.close()), saveB] });
}

/** Version, Reconfigure and Uninstall for installed extension `ext` (see installedExtension). */
export function extensionControls(ext, redraw) {
  const el = h(`<div class="row ext-ctl" style="gap:6px;margin-top:8px;flex-wrap:wrap">
    <span class="cd-cat">${icon('download', 12)} ${esc(t('ext_version', { v: ext.version || '?' }))}</span>
    <button class="btn sm ghost" data-a="conf">${esc(t('ext_reconfigure'))}</button>
    <button class="btn sm ghost" data-a="rm">${esc(t('ext_uninstall'))}</button></div>`);
  el.querySelector('[data-a=conf]').onclick = () => reconfigure(ext, redraw);
  el.querySelector('[data-a=rm]').onclick = async () => {
    if (!(await confirmDialog(t('ext_uninstall_confirm', { name: ext.display_name || ext.name }), { okLabel: t('ext_uninstall') }))) return;
    try { redraw(await call('extension_uninstall', { name: ext.name })); toast(t('ext_uninstalled', { name: ext.display_name || ext.name })); } catch (e) { errorToast(e); }
  };
  return el;
}
