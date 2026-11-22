// "Add custom connector" (Settings → Developer): a remote MCP server by name
// and URL, with a bearer token for servers that take a fixed one; the rest
// sign in with OAuth in the browser (mcp_oauth.rs), which the dialog offers
// once the server asks for it. Written to the MCP config with
// mcp_save_config, which restarts the servers.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { call } from './api.js';
import { esc, h, modal, button, toast, confirmDialog, errorToast } from './util.js';
import { errorNotice } from './neterr.js';

const LOCAL = /^(localhost|127\.0\.0\.1|\[::1\])$|\.localhost$/;

/** Why connector `c` ({name, url}) can't be saved, as an i18n key, or null;
 *  `taken`: the names of the other servers. Plain http only for this computer. */
export function connectorProblem(c, taken) {
  const name = c.name.trim();
  if (!name) return 'cc_err_name';
  if (taken.includes(name)) return 'cc_err_taken';
  let u;
  try { u = new URL(c.url.trim()); } catch { return 'cc_err_url'; }
  if (u.protocol === 'https:' || (u.protocol === 'http:' && LOCAL.test(u.hostname))) return null;
  return u.protocol === 'http:' ? 'cc_err_https' : 'cc_err_url';
}

const authKey = (headers) => Object.keys(headers || {}).find((k) => k.toLowerCase() === 'authorization');

/** The mcpServers entry for connector `c` ({url, token, clientId}), keeping
 *  what the entry it replaces (`old`) has besides: other headers, options. */
export function connectorEntry(c, old = {}) {
  const e = { ...old, url: c.url.trim() };
  const headers = { ...(old.headers || {}) };
  const k = authKey(headers);
  if (k) delete headers[k];
  const token = (c.token || '').trim();
  if (token) headers.Authorization = /^(bearer|basic)\s/i.test(token) ? token : `Bearer ${token}`;
  if (Object.keys(headers).length) e.headers = headers; else delete e.headers;
  const clientId = (c.clientId || '').trim();
  const oauth = { ...(old.oauth || {}) };
  if (clientId) oauth.clientId = clientId; else delete oauth.clientId;
  if (Object.keys(oauth).length) e.oauth = oauth; else delete e.oauth;
  return e;
}

/** The token field's value for entry `e`: its Authorization header without "Bearer ". */
export function connectorToken(e) {
  const k = authKey(e.headers);
  return k ? String(e.headers[k]).replace(/^bearer\s+/i, '') : '';
}

const configWith = (status, servers) => JSON.stringify({ ...(status.config || {}), mcpServers: servers }, null, 2);

/** The dialog that adds a connector, or edits server `name`'s; `status` is
 *  mcp_status's reply and `redraw(status)` shows the servers again. */
export function connectorDialog(status, redraw, name = null) {
  if (status.config_problem) { toast(t('cc_config_broken'), 'error'); return; }
  let servers = { ...((status.config || {}).mcpServers || {}) };
  let old = name ? servers[name] || {} : {};
  const body = h(`<div class="cc-form">
    <div class="field"><label class="label" for="cc-name">${esc(t('cc_name'))}</label><input class="input" id="cc-name" placeholder="${esc(t('cc_name_ph'))}"></div>
    <div class="field"><label class="label" for="cc-url">${esc(t('cc_url'))}</label><input class="input mono" id="cc-url" placeholder="https://example.com/mcp" spellcheck="false"></div>
    <div class="field"><label class="label" for="cc-token">${esc(t('cc_token'))} <span class="muted">(${esc(t('optional'))})</span></label><input class="input mono" id="cc-token" type="password" autocomplete="off"><div class="help">${esc(t('cc_token_help'))}</div></div>
    <details class="help"><summary>${esc(t('advanced'))}</summary><div class="field" style="margin-top:10px"><label class="label" for="cc-client">${esc(t('cc_client_id'))}</label><input class="input mono" id="cc-client" spellcheck="false"><div class="help">${esc(t('cc_client_id_help'))}</div></div></details>
    <div class="cc-out" aria-live="polite"></div></div>`);
  const field = (id) => body.querySelector(`#cc-${id}`);
  field('name').value = name || '';
  field('url').value = old.url || '';
  field('token').value = connectorToken(old);
  field('client').value = (old.oauth && old.oauth.clientId) || '';
  const out = body.querySelector('.cc-out');
  const say = (el) => { out.replaceChildren(el); el.style.marginTop = '12px'; };
  const primary = (label) => { save.disabled = false; save.textContent = label; };

  const save = button(t(name ? 'save' : 'cc_add'), 'primary', async () => {
    if (save.dataset.signin) return signIn();
    const c = { name: field('name').value.trim(), url: field('url').value, token: field('token').value, clientId: field('client').value };
    const problem = connectorProblem(c, Object.keys(servers).filter((k) => k !== name));
    out.replaceChildren();
    if (problem) { say(h(`<div class="notice error">${icon('alert', 15)}<div class="grow small">${esc(t(problem))}</div></div>`)); return; }
    const next = { ...servers };
    if (name && name !== c.name) delete next[name];
    next[c.name] = connectorEntry(c, old);
    save.disabled = true;
    save.textContent = t('connecting');
    let st;
    try { st = await call('mcp_save_config', { config: configWith(status, next) }); } catch (e) { say(errorNotice(e.message)); primary(t(name ? 'save' : 'cc_add')); return; }
    // saved: from here on the dialog edits this server
    status = st; servers = next; name = c.name; old = next[c.name];
    redraw(st);
    const s = st.servers.find((x) => x.name === c.name) || {};
    if (s.status === 'needs_auth') {
      say(h(`<p class="small" style="margin:0">${esc(t('cc_needs_signin', { name: c.name }))}</p>`));
      save.dataset.signin = '1';
      primary(t('mcp_sign_in'));
    } else if (s.error) {
      say(errorNotice(s.error));
      primary(t('save'));
    } else {
      m.close();
      toast(s.status === 'connected' ? t('cc_added', { name: c.name }) : t('config_saved'));
    }
  });
  const signIn = async () => {
    save.disabled = true;
    save.textContent = t('mcp_signing_in');
    try { await call('mcp_oauth_sign_in', { server: name }); } catch (e) { say(errorNotice(e.message)); primary(t('mcp_sign_in')); return; }
    m.close();
    toast(t('cc_added', { name }));
    try { redraw(await call('mcp_status')); } catch (e) { errorToast(e); }
  };
  body.querySelectorAll('input').forEach((x) => x.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.isComposing) { e.preventDefault(); save.click(); } }));
  const m = modal({ title: t(name ? 'cc_edit_title' : 'cc_title'), body, footer: [button(t('cancel'), '', () => m.close()), save] });
  setTimeout(() => field(name ? 'url' : 'name').focus(), 30);
}

/** Remove remote server `s` (an mcp_status entry) from the config, and its sign-in. */
export async function removeConnector(status, s, redraw) {
  if (status.config_problem) { toast(t('cc_config_broken'), 'error'); return; }
  if (!(await confirmDialog(t('cc_remove_confirm', { name: s.name }), { okLabel: t('cc_remove') }))) return;
  // the sign-in is kept per server name and URL: it would come back with a server added again by that name
  if (s.signed_in) await call('mcp_oauth_sign_out', { server: s.name }).catch(() => {});
  const servers = { ...((status.config || {}).mcpServers || {}) };
  delete servers[s.name];
  redraw(await call('mcp_save_config', { config: configWith(status, servers) }));
  toast(t('cc_removed', { name: s.name }));
}
