// First-run screen: connect an API key.
import { icon } from './icons.js';
import { t, LANGUAGES } from './i18n.js';
import { call } from './api.js';
import { store, emit } from './store.js';
import { $, esc, h, errorToast } from './util.js';

export function renderOnboarding(view, done) {
  view.innerHTML = `<div class="onboard"><div class="onboard-card">
    <img class="logo-mark" src="icon.png" alt="">
    <h1>${t('welcome_title')}</h1>
    <p>${t('welcome_desc')}</p>
    <div class="field"><label class="label">${t('your_name')}</label><input class="input" data-f="name" value="${esc(store.settings.nickname || '')}"></div>
    <div class="field"><label class="label">${t('api_key')}</label><input class="input mono" data-f="key" type="password" placeholder="sk-ant-…" autocomplete="off">
      <div class="help">${t('api_key_help')} <a href="https://console.anthropic.com/settings/keys">${t('get_key')} ${icon('external', 12)}</a></div></div>
    <details style="margin-bottom:18px"><summary class="muted small" style="cursor:pointer">${t('advanced')}</summary>
      <div class="field" style="margin-top:10px"><label class="label">${t('base_url')}</label><input class="input mono" data-f="base" value="${esc(store.settings.base_url || 'https://api.anthropic.com')}"><div class="help">${t('base_url_help')}</div></div>
      <div class="field"><label class="label">${t('language')}</label><select class="select" data-f="lang">${LANGUAGES.map((l) => `<option value="${l.id}" ${l.id === (store.settings.language || 'auto') ? 'selected' : ''}>${l.name}</option>`).join('')}</select></div>
    </details>
    <div class="notice error hidden" data-f="err"></div>
    <button class="btn accent lg" data-f="go" style="width:100%">${t('continue_btn')}</button>
  </div></div>`;
  const go = $('[data-f=go]', view);
  const err = $('[data-f=err]', view);
  $('[data-f=lang]', view).addEventListener('change', (e) => emit('save-settings', { language: e.target.value }));
  const submit = async () => {
    const key = $('[data-f=key]', view).value.trim();
    const name = $('[data-f=name]', view).value.trim();
    const base = $('[data-f=base]', view).value.trim();
    err.classList.add('hidden');
    if (!key && !store.settings.has_api_key) { $('[data-f=key]', view).focus(); return; }
    go.disabled = true;
    go.textContent = t('connecting');
    try {
      await call('save_settings', { patch: { nickname: name, base_url: base } });
      if (key) await call('set_api_key', { key });
      await call('test_connection');
      await call('save_settings', { patch: { onboarded: true } });
      done();
    } catch (e) {
      err.textContent = e.message;
      err.classList.remove('hidden');
      go.disabled = false;
      go.textContent = t('continue_btn');
      if (e.status !== 401 && key) {
        // Key stored but connection failed (e.g. offline): allow continuing.
        const skip = h(`<button class="btn ghost" style="width:100%;margin-top:8px">${t('continue_btn')} →</button>`);
        skip.onclick = async () => { await call('save_settings', { patch: { onboarded: true } }).catch(errorToast); done(); };
        if (!view.querySelector('.skip')) { skip.classList.add('skip'); go.after(skip); }
      }
    }
  };
  go.onclick = submit;
  view.querySelectorAll('input').forEach((i) => i.addEventListener('keydown', (e) => { if (e.key === 'Enter') submit(); }));
  $('[data-f=key]', view).focus();
}
