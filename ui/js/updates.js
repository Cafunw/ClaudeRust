// Updates: the section in Settings → About (check, download and install,
// restart), and a notice when the background check finds a newer version.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { call, on as onBackend } from './api.js';
import { emit } from './store.js';
import { h, esc, toast, errorToast } from './util.js';
import { friendlyError } from './neterr.js';

// idle | checking | none | available | downloading | ready | error
let state = { phase: 'idle', info: null, progress: null, error: '' };
const listeners = new Set();

function set(patch) {
  state = { ...state, ...patch };
  for (const f of [...listeners]) f();
}

/** The version on offer, when one was found and isn't installed yet. */
export function pendingUpdate() {
  return state.phase === 'available' || state.phase === 'ready' ? state.info : null;
}

export async function checkNow() {
  if (state.phase === 'checking' || state.phase === 'downloading') return;
  set({ phase: 'checking', error: '' });
  try {
    const info = await call('update_check');
    set({ phase: info.available ? 'available' : 'none', info });
  } catch (e) {
    set({ phase: 'error', error: e });
  }
}

export async function installNow() {
  if (state.phase === 'downloading') return;
  set({ phase: 'downloading', progress: { downloaded: 0, total: null }, error: '' });
  try {
    await call('update_install');
    set({ phase: 'ready' });
  } catch (e) {
    set({ phase: 'error', error: e });
  }
}

/** Open Settings → About, where the update can be installed. */
export function showUpdates() {
  emit('open-settings', 'about');
}

onBackend('update-progress', (p) => { if (state.phase === 'downloading') set({ progress: p }); });
onBackend('update-available', (info) => {
  if (state.phase === 'downloading' || state.phase === 'ready') return;
  set({ phase: 'available', info });
  toast(t('upd_toast', { v: info.version }), '', { action: t('upd_view'), onAction: showUpdates, duration: 30000 });
});

const mb = (n) => `${(n / 1048576).toFixed(1)} MB`;

/** The Updates block for Settings → About; it redraws itself as the state changes. */
export function updatesSection() {
  // sections that were shown and are gone (Settings closed, another tab):
  // they were only let go of at the next state change, which may not come
  for (const f of [...listeners]) if (f.gone && f.gone()) listeners.delete(f);
  // a check's outcome (none, a new version, an error) is said when it comes
  const el = h('<div class="upd" aria-live="polite" tabindex="-1"></div>');
  let shown = false;
  const draw = () => {
    if (shown && !el.isConnected) { listeners.delete(draw); return; }
    shown = true;
    const { phase, info, progress, error } = state;
    const releases = (info && info.releases_url) || '';
    let html = '';
    if (phase === 'checking') {
      html = `<div class="upd-line"><span class="spinner tiny"></span> ${esc(t('upd_checking'))}</div>`;
    } else if (phase === 'none') {
      html = `<div class="upd-line">${icon('check', 14)} ${esc(t('upd_none', { v: info.current }))}</div>
        <div class="row-btns"><button class="btn sm" data-a="check">${esc(t('upd_check'))}</button></div>`;
    } else if (phase === 'available') {
      html = `<div class="upd-line"><b>${esc(t('upd_available', { v: info.version }))}</b> <span class="muted">${esc(t('upd_have', { v: info.current }))}</span></div>
        ${info.notes ? `<pre class="upd-notes">${esc(info.notes)}</pre>` : ''}
        <div class="row-btns"><button class="btn sm primary" data-a="install">${icon('download', 13)} ${esc(t('upd_install'))}</button>${releases ? `<button class="btn sm ghost" data-a="page">${icon('external', 13)} ${esc(t('upd_page'))}</button>` : ''}</div>`;
    } else if (phase === 'downloading') {
      const { downloaded = 0, total = null } = progress || {};
      const pct = total ? Math.min(100, Math.round((downloaded / total) * 100)) : null;
      html = `<div class="upd-line"><span class="spinner tiny"></span> ${esc(t('upd_downloading'))} <span class="muted">${esc(total ? `${mb(downloaded)} / ${mb(total)}` : mb(downloaded))}</span></div>
        <div class="upd-bar ${pct === null ? 'indeterminate' : ''}"><span style="width:${pct === null ? 30 : pct}%"></span></div>`;
    } else if (phase === 'ready') {
      html = `<div class="upd-line">${icon('check', 14)} ${esc(t('upd_ready', { v: (info && info.version) || '' }))}</div>
        <div class="row-btns"><button class="btn sm primary" data-a="restart">${icon('refreshCw', 13)} ${esc(t('upd_restart'))}</button></div>`;
    } else if (phase === 'error') {
      // a coded failure in the UI's language, the backend's own words under Details
      const plain = friendlyError(error);
      const raw = (error && error.message) || String(error);
      html = `<div class="upd-line upd-err">${icon('alert', 14)} ${esc(plain || raw)}</div>
        ${plain && plain !== raw ? `<details class="err-raw"><summary>${esc(t('err_details'))}</summary><div class="mono">${esc(raw)}</div></details>` : ''}
        <div class="row-btns"><button class="btn sm" data-a="check">${esc(t('upd_retry'))}</button>${releases ? `<button class="btn sm ghost" data-a="page">${icon('external', 13)} ${esc(t('upd_page'))}</button>` : ''}</div>`;
    } else {
      html = `<div class="row-btns"><button class="btn sm" data-a="check">${esc(t('upd_check'))}</button></div>`;
    }
    // the button pressed is drawn away: the keyboard stays in the section; progress is said once it's done
    const had = el.contains(document.activeElement);
    el.setAttribute('aria-busy', String(phase === 'checking' || phase === 'downloading'));
    el.innerHTML = html;
    if (had) (el.querySelector('button') || el).focus();
    el.querySelectorAll('[data-a]').forEach((b) => {
      b.onclick = () => {
        const a = b.dataset.a;
        if (a === 'check') checkNow();
        else if (a === 'install') installNow();
        else if (a === 'restart') call('restart_app').catch(errorToast);
        else if (a === 'page') call('open_external', { url: releases }).catch(errorToast);
      };
    });
  };
  draw.gone = () => shown && !el.isConnected;
  listeners.add(draw);
  draw();
  return el;
}
