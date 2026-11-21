// Settings modal.
import { icon } from './icons.js';
import { t, LANGUAGES } from './i18n.js';
import { call, isMac, on as onBackend } from './api.js';
import { store, emit, on } from './store.js';
import { $, esc, h, modal, button, toast, errorToast, promptDialog, confirmDialog } from './util.js';
import { renderImport } from './importer.js';
import * as keymap from './keymap.js';
import { updatesSection } from './updates.js';
import { accountStatus, signInButton, planName, CC_SETUP_URL } from './account.js';
import { oauthControls } from './mcp_oauth_ui.js';

const TABS = [
  ['general', 'user', 's_profile'],
  ['appearance', 'sun', 's_appearance'],
  ['account', 'key', 's_account'],
  ['import', 'download', 's_import'],
  ['features', 'sparkle', 's_features'],
  ['developer', 'plug', 's_developer'],
  ['code', 'terminal', 's_code'],
  ['desktop', 'monitor', 's_desktop'],
  ['shortcuts', 'zap', 's_shortcuts'],
  ['about', 'info', 's_about'],
];

let current = null;

// the shortcut being recorded (Settings → Shortcuts): one at a time, and
// never left listening once Settings closes
let recorder = null;
function stopRecording() {
  if (recorder) window.removeEventListener('keydown', recorder, true);
  recorder = null;
}

export function openSettings(tab = 'general') {
  if (current) { current.show(tab); return; }
  const root = h(`<div class="settings"><nav class="settings-nav"></nav><div class="settings-body"></div></div>`);
  const nav = $('.settings-nav', root);
  const body = $('.settings-body', root);
  for (const [id, ic, label] of TABS) {
    const b = h(`<button class="nav-item" data-tab="${id}">${icon(ic, 16)}<span class="label">${t(label)}</span></button>`);
    b.onclick = () => show(id);
    nav.appendChild(b);
  }
  // what the shown tab subscribed to, undone when another tab is shown or Settings closes
  const leaving = [];
  const leave = () => { for (const f of leaving.splice(0)) f(); };
  const m = modal({ title: t('settings'), body: root, wide: true, cls: 'tall', onClose: () => { current = null; stopRecording(); off(); leave(); } });
  m.body.style.padding = '0';
  m.body.style.flex = '1';
  m.body.style.minHeight = '0';
  m.body.style.overflow = 'hidden';
  let active = tab;
  function show(id) {
    leave();
    active = id;
    nav.querySelectorAll('[data-tab]').forEach((b) => b.classList.toggle('active', b.dataset.tab === id));
    body.innerHTML = '';
    body.scrollTop = 0;
    (RENDER[id] || RENDER.general)(body, { onLeave: (f) => leaving.push(f) });
  }
  // redraw for changes made elsewhere, but not under the user's cursor: a
  // field being typed in would lose its text
  const typing = () => { const a = document.activeElement; return !!(a && body.contains(a) && a.matches('input, textarea, select')); };
  const offSettings = on('settings-changed', () => { if (['account', 'appearance'].includes(active) && !typing()) show(active); });
  // Slow MCP servers finish starting in the background.
  const offMcp = onBackend('mcp-changed', () => { if (active === 'developer' && !typing()) show(active); });
  const off = () => { offSettings(); offMcp.then((u) => u()); };
  current = { show };
  show(tab);
}

function save(patch) { emit('save-settings', patch); }

function row(title, desc, control) {
  const r = h(`<div class="setting-row"><div class="txt"><div class="t">${esc(title)}</div>${desc ? `<div class="d">${esc(desc)}</div>` : ''}</div></div>`);
  r.appendChild(control);
  return r;
}

function toggle(key) {
  const s = h(`<label class="switch"><input type="checkbox" ${store.settings[key] ? 'checked' : ''}><span class="track"></span></label>`);
  s.querySelector('input').addEventListener('change', (e) => save({ [key]: e.target.checked }));
  return s;
}

function select(key, options) {
  const sel = h(`<select class="select" style="width:auto;min-width:200px">${options.map(([v, l]) => `<option value="${esc(v)}" ${String(store.settings[key]) === String(v) ? 'selected' : ''}>${esc(l)}</option>`).join('')}</select>`);
  sel.addEventListener('change', () => save({ [key]: sel.value }));
  return sel;
}

function textField(key, { multiline = false, placeholder = '', type = 'text' } = {}) {
  const el = multiline
    ? h(`<textarea class="textarea" rows="5" placeholder="${esc(placeholder)}"></textarea>`)
    : h(`<input class="input" type="${type}" placeholder="${esc(placeholder)}">`);
  el.value = store.settings[key] ?? '';
  el.addEventListener('change', () => {
    if (type !== 'number') { save({ [key]: el.value }); return; }
    // whole, non-negative numbers only (the settings are integers); an empty
    // or invalid entry goes back to the current value instead of saving 0
    const n = Number(el.value);
    if (el.value.trim() === '' || !Number.isInteger(n) || n < 0) { el.value = store.settings[key] ?? ''; toast(t('invalid_number'), 'error'); return; }
    save({ [key]: n });
  });
  return el;
}

function field(label, control, help) {
  const f = h(`<div class="field"><label class="label">${esc(label)}</label></div>`);
  f.appendChild(control);
  if (help) f.appendChild(h(`<div class="help">${esc(help)}</div>`));
  return f;
}

/** The Claude account block of Settings → Account: sign in / status / sign out, and which engine chats use. */
function claudeAccount(body) {
  body.appendChild(h(`<h4>${t('acct_title')}</h4>`));
  const box = h('<div></div>');
  body.appendChild(box);
  const group = (...els) => { const g = h('<div class="row" style="gap:8px;flex-wrap:wrap;justify-content:flex-end"></div>'); g.append(...els); return g; };
  const draw = async (fresh = false) => {
    box.innerHTML = '<div style="padding:6px 0"><span class="spinner"></span></div>';
    const a = await accountStatus(fresh);
    box.innerHTML = '';
    if (!a.found) {
      box.appendChild(row(t('acct_title'), t('acct_need_cc'), group(
        button(t('cc_install'), '', () => call('open_external', { url: CC_SETUP_URL })),
        button(t('cc_detect'), 'ghost', () => draw(true)))));
      return;
    }
    if (a.logged_in) {
      const who = [a.email, planName(a)].filter(Boolean).join(' · ');
      box.appendChild(row(who || t('acct_signed_in'), t('acct_signed_in'), button(t('acct_signout'), '', async () => {
        if (!(await confirmDialog(t('acct_signout_confirm'), { okLabel: t('acct_signout'), danger: false }))) return;
        await call('claude_auth_logout');
        // chats set to the account would only fail now
        if (store.settings.chat_engine === 'claude-code') save({ chat_engine: 'auto' });
        toast(t('acct_signedout'));
        draw(true);
      })));
    } else {
      box.appendChild(row(t('acct_title'), t('acct_signedout_d'), group(
        signInButton('primary', () => draw(true)),
        button(t('acct_terminal'), 'ghost', () => call('claude_code_login')))));
    }
    box.appendChild(row(t('chat_engine'), t('chat_engine_d'), select('chat_engine', [['auto', t('chat_engine_auto')], ['api', t('api_plan')], ['claude-code', t('acct_plan')]])));
    box.appendChild(h(`<p class="help" style="margin:-6px 0 18px">${esc(t('acct_chat_note'))}</p>`));
  };
  draw();
}

function modelOptions() {
  return store.models.filter((m) => m.available !== false).map((m) => [m.id, `${m.name}${m.legacy ? '' : ''}`]);
}

const RENDER = {
  general(body, tab) {
    body.appendChild(h(`<h3>${t('s_profile')}</h3>`));
    body.appendChild(field(t('full_name'), textField('full_name')));
    body.appendChild(field(t('nickname'), textField('nickname')));
    body.appendChild(field(t('occupation'), textField('occupation')));
    body.appendChild(field(t('preferences'), textField('preferences', { multiline: true }), t('preferences_help')));
    body.appendChild(row(t('language'), '', select('language', LANGUAGES.map((l) => [l.id, l.name]))));
    // custom styles
    body.appendChild(h(`<h4>${t('style')}</h4>`));
    const list = h('<div></div>');
    const draw = () => {
      list.innerHTML = '';
      for (const s of store.settings.custom_styles || []) {
        const r = h(`<div class="setting-row"><div class="txt"><div class="t">${esc(s.name)}</div><div class="d ellipsis">${esc(s.prompt)}</div></div><button class="icon-btn" data-a="e">${icon('edit', 15)}</button><button class="icon-btn" data-a="d">${icon('trash', 15)}</button></div>`);
        r.querySelector('[data-a=e]').onclick = () => editStyle(s);
        r.querySelector('[data-a=d]').onclick = async () => {
          if (!(await confirmDialog(`${t('delete')} “${s.name}”?`))) return;
          save({ custom_styles: (store.settings.custom_styles || []).filter((x) => x.id !== s.id) });
        };
        list.appendChild(r);
      }
      const add = h(`<button class="btn sm" style="margin-top:8px">${icon('plus', 14)} ${t('create_style')}</button>`);
      add.onclick = () => editStyle(null);
      list.appendChild(add);
    };
    const editStyle = async (s) => {
      const name = await promptDialog(t('name'), s ? s.name : '');
      if (!name) return;
      const prompt = await promptDialog(t('style'), s ? s.prompt : '', { multiline: true, placeholder: 'e.g. Write like a friendly expert. Use short paragraphs and concrete examples.' });
      if (prompt === null) return;
      const styles = (store.settings.custom_styles || []).slice();
      if (s) Object.assign(styles.find((x) => x.id === s.id), { name, prompt });
      else styles.push({ id: 'custom-' + Date.now().toString(36), name, prompt });
      save({ custom_styles: styles });
    };
    draw();
    // (an observer of the whole document watched for the list going away:
    // it saw every change anywhere, a streaming answer's too, while open)
    tab.onLeave(on('settings-changed', draw));
    body.appendChild(list);
  },

  appearance(body) {
    body.appendChild(h(`<h3>${t('s_appearance')}</h3>`));
    const cur = store.settings.theme === 'light' || store.settings.theme === 'dark' ? store.settings.theme : 'system';
    const th = h(`<div class="segmented icons">${[['system', 'monitor', t('system')], ['light', 'sun', t('light')], ['dark', 'moon', t('dark')]].map(([id, ic, label]) => `<button data-th="${id}" title="${esc(label)}" aria-label="${esc(label)}" class="${cur === id ? 'on' : ''}">${icon(ic, 15)}</button>`).join('')}</div>`);
    th.querySelectorAll('button').forEach((b) => b.onclick = () => save({ theme: b.dataset.th }));
    body.appendChild(row(t('theme'), '', th));
    const fonts = [['default', t('font_default')], ['sans', t('font_sans')], ['system', t('font_system')], ['dyslexic', t('font_dyslexic')]];
    body.appendChild(row(t('chat_font'), '', select('chat_font', fonts)));
    body.appendChild(row(t('chat_width'), t('chat_width_d'), select('chat_width', [['normal', t('width_normal')], ['wide', t('width_wide')], ['full', t('width_full')]])));
    const z = h(`<div class="segmented">${[0.8, 0.9, 1, 1.1, 1.25].map((v) => `<button data-z="${v}" class="${Math.abs((store.settings.zoom || 1) - v) < 0.01 ? 'on' : ''}">${Math.round(v * 100)}%</button>`).join('')}</div>`);
    z.querySelectorAll('button').forEach((b) => b.onclick = () => emit('set-zoom', Number(b.dataset.z)));
    body.appendChild(row(t('zoom'), '', z));
  },

  account(body) {
    body.appendChild(h(`<h3>${t('s_account')}</h3>`));
    claudeAccount(body);
    body.appendChild(h(`<h4>${t('api_key')}</h4>`));
    const s = store.settings;
    if (s.has_api_key) {
      body.appendChild(row(t('current_key'), s.api_key_masked || '', h('<span></span>')));
    }
    const keyInput = h(`<input class="input mono" type="password" placeholder="sk-ant-…" autocomplete="off">`);
    const saveKey = button(s.has_api_key ? t('replace_key') : t('save'), 'primary', async () => {
      const k = keyInput.value.trim();
      if (!k) return;
      try { await call('set_api_key', { key: k }); toast(t('key_saved')); keyInput.value = ''; emit('reload-settings'); } catch (e) { errorToast(e); }
    });
    const f = field(t('api_key'), keyInput, t('api_key_help'));
    const actions = h('<div class="row" style="margin-top:10px"></div>');
    actions.appendChild(saveKey);
    const test = button(t('test_connection'), '', async () => {
      test.disabled = true;
      try { const r = await call('test_connection'); toast(t('connection_ok', { n: r.models })); emit('models-refresh'); } catch (e) { errorToast(e); } finally { test.disabled = false; }
    });
    actions.appendChild(test);
    if (s.has_api_key) actions.appendChild(button(t('remove_key'), 'ghost', async () => { await call('set_api_key', { key: null }); toast(t('key_removed')); emit('reload-settings'); }));
    const getKey = button(t('get_key'), 'ghost', () => call('open_external', { url: 'https://console.anthropic.com/settings/keys' }));
    actions.appendChild(getKey);
    f.appendChild(actions);
    body.appendChild(f);
    body.appendChild(field(t('base_url'), textField('base_url', { placeholder: 'https://api.anthropic.com' }), t('base_url_help')));
    {
      const sec = h(`<div class="section-title" style="margin-top:22px">${esc(t('oai_title'))}</div>`);
      body.appendChild(sec);
      body.appendChild(field(t('oai_url'), textField('oai_base_url', { placeholder: 'http://localhost:11434/v1' }), t('oai_url_help')));
      body.appendChild(field(t('oai_key'), textField('oai_api_key', { type: 'password', placeholder: t('optional') })));
      const st = h('<div class="row-inline" style="gap:10px;align-items:center"></div>');
      const btn = h(`<button class="btn sm">${esc(t('oai_load'))}</button>`);
      const out = h('<span class="muted small"></span>');
      btn.onclick = async () => {
        out.textContent = '…';
        try {
          await new Promise((r) => setTimeout(r, 150)); // let a just-edited field save
          const list = await call('oai_refresh');
          out.textContent = list.length ? t('oai_found', { n: list.length, list: list.slice(0, 6).join(', ') }) : t('oai_off');
          emit('refresh-models');
        } catch (e) { out.textContent = e.message || String(e); }
      };
      st.append(btn, out);
      body.appendChild(st);
    }
    body.appendChild(h(`<h4>${t('model')}</h4>`));
    body.appendChild(row(t('default_model'), '', select('default_model', modelOptions())));
    body.appendChild(row(t('title_model'), '', select('title_model', modelOptions())));
    const mt = textField('max_tokens', { type: 'number' });
    mt.style.width = '140px';
    body.appendChild(row(t('max_tokens_setting'), '', mt));
  },

  features(body) {
    body.appendChild(h(`<h3>${t('s_features')}</h3>`));
    body.appendChild(row(t('artifacts_toggle'), t('artifacts_toggle_d'), toggle('artifacts_enabled')));
    body.appendChild(row(t('latex_toggle'), t('latex_toggle_d'), toggle('latex_enabled')));
    body.appendChild(row(t('websearch_default'), t('websearch_default_d'), toggle('web_search_default')));
    body.appendChild(row(t('thinking_default'), t('thinking_default_d'), toggle('thinking_default')));
    // past-chat search runs only in the API engine (claudecode.rs leaves it off)
    const onAccount = store.settings.chat_engine === 'claude-code' || (store.settings.chat_engine !== 'api' && !store.settings.has_api_key);
    body.appendChild(row(t('chat_search_toggle'), onAccount ? `${t('chat_search_toggle_d')} ${t('chat_search_cc_note')}` : t('chat_search_toggle_d'), toggle('chat_search')));
    body.appendChild(row(t('send_enter'), t('send_enter_d'), toggle('send_on_enter')));
    body.appendChild(h(`<h4>API</h4>`));
    body.appendChild(row(t('fallbacks_toggle'), t('fallbacks_toggle_d'), toggle('refusal_fallbacks')));
    {
      const models = (store.models || []).filter((m) => m.featured && !String(m.id).startsWith('oai:'));
      body.appendChild(row(t('overload_setting'), t('overload_setting_d'), select('overload_fallback', [['auto', t('overload_auto')], ['off', t('overload_off')], ...models.map((m) => [m.id, m.name])])));
    }
    body.appendChild(row(t('caching_toggle'), t('caching_toggle_d'), toggle('prompt_caching')));
  },

  async developer(body) {
    body.appendChild(h(`<h3>${t('mcp_title')}</h3>`));
    body.appendChild(h(`<p class="muted" style="margin:-6px 0 16px">${esc(t('mcp_desc'))}</p>`));
    const actions = h('<div class="row" style="margin-bottom:16px;flex-wrap:wrap"></div>');
    const servers = h('<div></div>');
    body.append(actions, servers);
    let status;
    const draw = async (st) => {
      status = st || await call('mcp_status');
      store.mcpServers = status.servers;
      servers.innerHTML = '';
      // a config that can't be used starts no servers: say why
      if (status.config_problem) servers.appendChild(h(`<div class="notice error" style="margin:0 0 14px">${icon('alert', 15)}<div class="grow small">${esc(status.config_problem)}</div></div>`));
      if (!status.servers.length) servers.appendChild(h(`<div class="empty-state" style="padding:30px">${icon('plug', 28)}<div style="margin-top:8px">${t('no_servers')}</div></div>`));
      for (const s of status.servers) servers.appendChild(serverCard(s, () => draw()));
      actions.innerHTML = '';
      actions.appendChild(button(t('edit_config'), 'primary', () => editConfig(status, draw)));
      actions.appendChild(button(t('open_file'), '', () => call('open_path', { path: status.config_path }).catch(errorToast)));
      actions.appendChild(button(t('reload'), '', async () => { try { await draw(await call('mcp_reload')); } catch (e) { errorToast(e); } }));
      actions.appendChild(button(t('install_extension'), '', () => installExtension(draw)));
      if (status.claude_desktop_config) {
        actions.appendChild(button(t('import_claude'), '', () => emit('open-settings', 'import')));
      }
      actions.appendChild(h(`<span class="small muted mono ellipsis" style="max-width:100%" title="${esc(status.config_path)}">${esc(status.config_path)}</span>`));
    };
    await draw();
  },

  async code(body) {
    body.appendChild(h(`<h3>${t('s_code')}</h3>`));
    body.appendChild(row(t('code_engine'), t('code_engine_d'), select('code_engine', [['auto', t('engine_auto')], ['claude-code', t('engine_cc')], ['builtin', t('engine_builtin')]])));
    body.appendChild(row(t('editor_setting'), t('editor_setting_d'), select('editor', [['auto', t('editor_auto')], ['code', 'VS Code'], ['cursor', 'Cursor'], ['windsurf', 'Windsurf'], ['zed', 'Zed'], ['subl', 'Sublime Text'], ['idea', 'IntelliJ IDEA'], ['system', t('editor_system')]])));
    body.appendChild(row(t('checkpoints_setting'), t('checkpoints_setting_d'), toggle('code_checkpoints')));
    {
      const cur = Number(store.settings.auto_compact_pct ?? 85);
      const ac = h(`<select class="select" style="width:auto;min-width:200px">${[[0, t('worktree_never')], [70, '70%'], [85, '85%'], [95, '95%']].map(([v, l]) => `<option value="${v}" ${cur === v ? 'selected' : ''}>${esc(String(l))}</option>`).join('')}</select>`);
      ac.addEventListener('change', () => save({ auto_compact_pct: Number(ac.value) }));
      body.appendChild(row(t('auto_compact_setting'), t('auto_compact_setting_d'), ac));
    }
    {
      const cur = Number(store.settings.turn_budget_usd || 0);
      const bi = h(`<input class="input" type="number" min="0" step="0.5" style="width:120px" placeholder="${esc(t('budget_none'))}" value="${cur > 0 ? cur : ''}">`);
      bi.addEventListener('change', () => { const v = Number(bi.value); save({ turn_budget_usd: Number.isFinite(v) && v > 0 ? v : 0 }); });
      body.appendChild(row(t('budget_setting'), t('budget_setting_d'), bi));
    }
    body.appendChild(row(t('worktree_setting'), t('worktree_setting_d'), select('code_worktree', [['parallel', t('worktree_parallel')], ['always', t('worktree_always')], ['off', t('worktree_never')]])));
    const pathIn = textField('claude_code_path', { placeholder: t('cc_path_help') });
    pathIn.classList.add('mono');
    const ccRow = row(t('cc_path'), '', pathIn);
    body.appendChild(ccRow);
    const status = h('<div class="row" style="padding:4px 0 14px;flex-wrap:wrap"></div>');
    body.appendChild(status);
    const drawStatus = async () => {
      status.innerHTML = `<span class="spinner"></span>`;
      const info = await call('claude_code_info', {}).catch(() => ({ found: false }));
      status.innerHTML = info.found
        ? `<span class="small">${icon('check', 13)} ${esc(info.version || 'claude')} · <span class="mono muted">${esc(info.path)}</span></span>`
        : `<span class="small" style="color:var(--danger)">${esc(t('cc_missing'))}</span>`;
      const login = button(t('cc_login'), 'sm', () => emit('open-settings', 'account'));
      const detect = button(t('cc_detect'), 'sm ghost', drawStatus);
      const install = button(t('cc_install'), 'sm ghost', () => call('open_external', { url: CC_SETUP_URL }));
      if (info.found) status.append(login, detect); else status.append(install, detect);
    };
    drawStatus();
    body.appendChild(h(`<p class="help" style="margin:-6px 0 18px">${esc(t('cc_explain'))}</p>`));
    const modes = ['default', 'auto', 'acceptEdits', 'plan', 'bypassPermissions'].map((m) => [m, t('mode_' + m)]);
    body.appendChild(row(t('default_mode'), '', select('code_permission_mode', modes)));
    body.appendChild(row(t('code_model'), '', select('code_model', modelOptions())));
    const to = textField('bash_timeout_secs', { type: 'number' });
    to.style.width = '120px';
    body.appendChild(row(t('bash_timeout'), '', to));
  },

  import(body) { renderImport(body); },

  desktop(body) {
    body.appendChild(h(`<h3>${t('s_desktop')}</h3>`));
    body.appendChild(row(t('quick_entry'), t('quick_entry_d'), toggle('quick_entry_enabled')));
    const sc = textField('quick_entry_shortcut', { placeholder: isMac ? 'Alt+Space' : 'CommandOrControl+Alt+Space' });
    sc.style.width = '260px';
    body.appendChild(row(t('shortcut'), '', sc));
    body.appendChild(row(t('keep_tray'), t('keep_tray_d'), toggle('keep_in_tray')));
    body.appendChild(row(t('launch_login'), '', toggle('launch_at_login')));
    body.appendChild(row(t('notifications_toggle'), t('notifications_toggle_d'), toggle('notifications')));

    // Rendering: matters for Remote Desktop, VNC, VMs and some GPU drivers.
    const r = store.render || {};
    const ha = select('hardware_acceleration', [['auto', t('hw_auto')], ['on', t('hw_on')], ['off', t('hw_off')]]);
    const box = h('<div class="engine-status" style="justify-content:flex-end"></div>');
    box.appendChild(ha);
    const status = h(`<div class="d">${esc(t('hw_desc'))}${r.remote ? ' ' + esc(t('hw_remote')) : ''} ${esc(r.software ? t('hw_now_soft') : t('hw_now_gpu'))}</div>`);
    const restart = button(t('restart_now'), 'sm hidden', () => call('restart_app').catch(errorToast));
    ha.addEventListener('change', () => restart.classList.remove('hidden'));
    box.appendChild(restart);
    const hwRow = row(t('hw_accel'), '', box);
    hwRow.querySelector('.txt').appendChild(status);
    body.appendChild(hwRow);
  },

  shortcuts(body) {
    const mod = isMac ? '⌘' : 'Ctrl';
    body.appendChild(h(`<h3>${t('shortcuts_list')}</h3>`));
    body.appendChild(h(`<p class="muted small">${esc(t('kb_help'))}</p>`));
    const names = { new_chat: t('sc_new'), palette: t('sc_search'), sidebar: t('sc_sidebar'), settings: t('sc_settings'), workspace: t('ws_toggle'), find: t('sc_find'), history: t('sc_history'), shortcuts: t('sc_sheet') };
    const user = () => ({ ...(store.settings.keybindings || {}) });
    const draw = () => {
      stopRecording();
      body.querySelectorAll('.kb-row').forEach((r) => r.remove());
      const anchor = body.querySelector('.kb-fixed');
      for (const action of keymap.ACTIONS) {
        const changed = Object.prototype.hasOwnProperty.call(user(), action);
        const ctl = h(`<div class="kb-ctl"><span class="chip mono kb-chip">${esc(keymap.shown(action) || t('kb_off'))}</span><button class="btn sm" data-a="change">${esc(t('kb_change'))}</button>${changed ? `<button class="btn sm ghost" data-a="reset">${esc(t('kb_reset'))}</button>` : ''}</div>`);
        const r = row(names[action] || action, '', ctl);
        r.classList.add('kb-row');
        ctl.querySelector('[data-a=change]').onclick = () => record(action, ctl.querySelector('.kb-chip'));
        const rs = ctl.querySelector('[data-a=reset]');
        if (rs) rs.onclick = () => { const u = user(); delete u[action]; save({ keybindings: u }); setTimeout(draw, 300); };
        body.insertBefore(r, anchor);
      }
    };
    const record = (action, chip) => {
      stopRecording();
      chip.textContent = t('kb_press');
      chip.classList.add('rec');
      const onKey = (e) => {
        // Settings closed or redrawn since: stop, and let the key through
        if (!chip.isConnected) { stopRecording(); return; }
        e.preventDefault();
        e.stopPropagation();
        const done = () => { stopRecording(); chip.classList.remove('rec'); };
        if (e.key === 'Escape') { done(); draw(); return; }
        if (e.key === 'Backspace' || e.key === 'Delete') { done(); save({ keybindings: { ...user(), [action]: '' } }); setTimeout(draw, 300); return; }
        const b = keymap.fromEvent(e);
        if (!b) { if (!['Control', 'Shift', 'Alt', 'Meta'].includes(e.key)) chip.textContent = t('kb_need_mod'); return; }
        const p = keymap.parse(b);
        const other = keymap.ACTIONS.find((a) => a !== action && keymap.bindings(a).some((x) => { const q = keymap.parse(x); return q.key === p.key && q.mod === p.mod && q.ctrl === p.ctrl && q.alt === p.alt && q.shift === p.shift && q.meta === p.meta; }));
        done();
        if (other) { toast(t('kb_conflict', { key: keymap.label(b), action: names[other] || other }), 'error'); draw(); return; }
        save({ keybindings: { ...user(), [action]: b } });
        setTimeout(draw, 300);
      };
      window.addEventListener('keydown', onKey, true);
      recorder = onKey;
    };
    const fixed = h('<div class="kb-fixed"></div>');
    body.appendChild(fixed);
    const list = [
      [t('sc_send'), 'Enter'], [t('sc_newline'), 'Shift+Enter'], [t('sc_stop'), 'Esc'], [t('sc_edit_last'), '↑'],
      [t('sc_switch_conv'), 'Ctrl+Tab'], [t('sc_panes'), 'Alt+1 … Alt+8'],
      [t('sc_zoom'), `${mod} + / − / 0`], [t('quick_entry'), store.settings.quick_entry_shortcut],
    ];
    for (const [a, b] of list) body.appendChild(row(a, '', h(`<span class="chip mono" style="height:26px">${esc(b)}</span>`)));
    draw();
  },

  about(body) {
    body.appendChild(h(`<h3>ClaudeRust</h3>`));
    body.appendChild(h(`<p class="muted">${t('about_desc')}</p>`));
    body.appendChild(row(t('version_label'), '', h(`<span class="mono">${esc(store.version)}</span>`)));
    body.appendChild(row(t('upd_title'), t('upd_auto_d'), toggle('auto_update')));
    body.appendChild(updatesSection());
    const dl = h(`<button class="btn sm">${t('reveal')}</button>`);
    dl.onclick = () => call('open_path', { path: store.dataDir }).catch(errorToast);
    body.appendChild(row(t('data_location'), store.dataDir, dl));
    const backup = h(`<div class="row-btns"><button class="btn sm" data-a="exp">${esc(t('settings_backup'))}</button><button class="btn sm" data-a="imp">${esc(t('settings_restore'))}</button></div>`);
    backup.querySelector('[data-a=exp]').onclick = async () => {
      try { await call('save_text_file', { name: 'clauderust-settings.json', content: await call('settings_export') }); } catch (e) { errorToast(e); }
    };
    backup.querySelector('[data-a=imp]').onclick = async () => {
      try {
        const paths = await call('pick_files');
        if (!paths || !paths.length) return;
        if (!(await confirmDialog(t('settings_restore_confirm'), { okLabel: t('settings_restore') }))) return;
        await call('settings_import', { path: paths[0] });
        location.reload();
      } catch (e) { errorToast(e); }
    };
    body.appendChild(row(t('settings_backup_title'), t('settings_backup_d'), backup));
    const lic = h(`<button class="btn sm">${esc(t('oss_show'))}</button>`);
    lic.onclick = async () => {
      try {
        const text = await call('third_party_licenses');
        const pre = h('<pre class="licenses"></pre>');
        pre.textContent = text;
        modal({ title: t('oss_title'), body: pre, wide: true, cls: 'tall' });
      } catch (e) { errorToast(e); }
    };
    body.appendChild(row(t('oss_title'), t('oss_d'), lic));
    const ob = h(`<button class="btn sm">${t('reset_onboarding')}</button>`);
    ob.onclick = async () => { await call('reset_onboarding'); location.reload(); };
    body.appendChild(row(t('welcome_title'), '', ob));
  },
};

function serverCard(s, redraw) {
  const disabled = (store.settings.disabled_mcp_servers || []).includes(s.name) || s.status === 'disabled';
  const card = h(`<div class="server"><div class="head">${icon('plug', 16)}<div class="grow"><div style="font-weight:600">${esc(s.name)}</div>
    <div class="small muted">${esc(s.transport || '')}${s.server_info && s.server_info.serverInfo ? ' · ' + esc(s.server_info.serverInfo.name || '') + ' ' + esc(s.server_info.serverInfo.version || '') : ''}${s.tools.length ? ' · ' + t('tools_count', { n: s.tools.length }) : ''}</div></div>
    <span class="st ${esc(s.status)}">${esc(t('st_' + s.status))}</span>
    <label class="switch" title="${t('enabled')}"><input type="checkbox" ${disabled ? '' : 'checked'}><span class="track"></span></label></div></div>`);
  card.querySelector('input').addEventListener('change', (e) => {
    const set = new Set(store.settings.disabled_mcp_servers || []);
    if (e.target.checked) set.delete(s.name); else set.add(s.name);
    save({ disabled_mcp_servers: [...set] });
    setTimeout(redraw, 1500);
  });
  if (s.error) card.appendChild(h(`<div class="notice error" style="margin:10px 0 0">${icon('alert', 15)}<div class="grow small mono">${esc(s.error)}</div></div>`));
  card.appendChild(oauthControls(s, redraw));
  if (s.tools.length) {
    const tl = h('<div class="tools-list"></div>');
    for (const tool of s.tools) {
      const key = `${s.name}/${tool.name}`;
      const pol = (store.settings.tool_policies || {})[key] || 'ask';
      const r = h(`<div class="tool-policy"><span class="nm" title="${esc(tool.description)}">${esc(tool.name)}</span>
        <div class="segmented">${['ask', 'allow', 'deny'].map((p) => `<button data-p="${p}" class="${p === pol ? 'on' : ''}">${t('policy_' + p)}</button>`).join('')}</div></div>`);
      r.querySelectorAll('button').forEach((b) => b.onclick = () => {
        r.querySelectorAll('button').forEach((x) => x.classList.toggle('on', x === b));
        // just this tool: the backend merges per tool, so an "always allow"
        // given meanwhile from a prompt isn't overwritten by this copy
        save({ tool_policies: { [key]: b.dataset.p } });
      });
      tl.appendChild(r);
    }
    card.appendChild(tl);
  }
  if (s.logs && s.logs.length) {
    const lb = h(`<button class="btn sm ghost" style="margin-top:6px">${t('logs')}</button>`);
    const logs = h(`<div class="logs hidden">${esc(s.logs.join('\n'))}</div>`);
    lb.onclick = () => logs.classList.toggle('hidden');
    card.append(lb, logs);
  }
  return card;
}

async function installExtension(redraw) {
  let path;
  try { path = await call('pick_extension'); } catch (e) { errorToast(e); return; }
  if (!path) return;
  let info;
  try { info = await call('extension_inspect', { path }); } catch (e) { errorToast(e); return; }
  const spec = info.user_config || {};
  const body = h(`<div><p style="margin:0 0 4px;font-weight:600">${esc(info.display_name || info.name)} <span class="muted small">${esc(info.version || '')}</span></p>
    ${info.author ? `<p class="small muted" style="margin:0 0 10px">${esc(info.author)}</p>` : ''}
    ${info.description ? `<p class="small" style="margin:0 0 14px">${esc(info.description)}</p>` : ''}</div>`);
  const inputs = {};
  for (const [key, def] of Object.entries(spec)) {
    const type = def.type || 'string';
    let el;
    if (type === 'boolean') {
      el = h(`<label class="switch"><input type="checkbox" ${def.default ? 'checked' : ''}><span class="track"></span></label>`);
      inputs[key] = () => el.querySelector('input').checked;
    } else {
      const dflt = Array.isArray(def.default) ? def.default.join('\n') : (def.default ?? '');
      el = def.multiple
        ? h(`<textarea class="textarea mono" rows="3" placeholder="one per line"></textarea>`)
        : h(`<input class="input ${def.sensitive ? 'mono' : ''}" type="${def.sensitive ? 'password' : type === 'number' ? 'number' : 'text'}">`);
      el.value = dflt;
      inputs[key] = () => {
        const v = el.value.trim();
        if (def.multiple) return v ? v.split('\n').map((x) => x.trim()).filter(Boolean) : null;
        if (type === 'number') return v === '' ? null : Number(v);
        return v === '' ? null : v;
      };
    }
    const f = h(`<div class="field"><label class="label">${esc(def.title || key)}${def.required ? ' *' : ''}</label></div>`);
    f.appendChild(el);
    if (def.description) f.appendChild(h(`<div class="help">${esc(def.description)}</div>`));
    body.appendChild(f);
  }
  const install = button(t('install'), 'primary', async () => {
    const values = {};
    for (const [k, get] of Object.entries(inputs)) values[k] = get();
    install.disabled = true;
    try {
      const r = await call('extension_install', { path, values });
      toast(t('extension_installed', { name: r.installed }));
      m.close();
      redraw(r);
    } catch (e) { errorToast(e); install.disabled = false; }
  });
  const m = modal({ title: t('install_extension'), body, footer: [button(t('cancel'), '', () => m.close()), install] });
}

function editConfig(status, redraw) {
  const ta = h(`<textarea class="textarea code" spellcheck="false" style="height:52vh"></textarea>`);
  // a file that doesn't parse is opened as it is, to be fixed, never
  // replaced by the empty config it reads as
  ta.value = status.config_text ?? JSON.stringify(status.config, null, 2);
  const example = `{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/folder"]
    },
    "remote-example": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer …" } }
  }
}`;
  const body = h('<div></div>');
  if (status.config_problem) body.appendChild(h(`<div class="notice error" style="margin:0 0 10px">${icon('alert', 15)}<div class="grow small">${esc(status.config_problem)}</div></div>`));
  body.append(ta, h(`<details class="help"><summary>Example</summary><pre class="mono" style="white-space:pre-wrap">${esc(example)}</pre></details>`));
  const saveB = button(t('save_restart'), 'primary', async () => {
    try { const r = await call('mcp_save_config', { config: ta.value }); toast(t('config_saved')); m.close(); redraw(r); } catch (e) { errorToast(e); }
  });
  const m = modal({ title: 'claude_desktop_config.json', body, footer: [button(t('cancel'), '', () => m.close()), saveB], wide: true, cls: 'tall' });
  m.el.style.height = 'auto';
}
