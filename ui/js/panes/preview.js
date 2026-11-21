// Preview pane: an in-app browser for the session's local dev server.
// Addresses printed by background commands are offered automatically.
import { t } from '../i18n.js';
import { icon } from '../icons.js';
import { call, on } from '../api.js';
import { esc, errorToast } from '../util.js';
import { registerPane, context, updateTabs, show } from '../workspace.js';

const state = new Map(); // convId -> { url, found: [] }
const LOCAL = /^https?:\/\/(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(\/|$)/i;

function st(convId) {
  if (!state.has(convId)) state.set(convId, { url: '', found: [] });
  return state.get(convId);
}

async function detect(ctx) {
  const s = st(ctx.convId);
  try { s.found = await call('preview_urls', { conversationId: ctx.convId }); } catch { s.found = []; }
  if (!s.url && s.found.length) s.url = (s.found.find((f) => f.running) || s.found[0]).url;
  updateTabs();
  return s;
}

function mount(el, ctx) {
  el.classList.add('preview-pane');
  el.innerHTML = `<div class="pv-bar">
      <button class="icon-btn sm" data-a="reload" title="${t('reload')}">${icon('refreshCw', 14)}</button>
      <input class="pv-url" spellcheck="false" placeholder="http://localhost:3000">
      <button class="icon-btn sm" data-a="pick" title="${t('preview_detected')}">${icon('chevDown', 14)}</button>
      <button class="icon-btn sm" data-a="ext" title="${t('open_browser')}">${icon('external', 14)}</button>
    </div>
    <div class="pv-body"><iframe class="pv-frame" sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads"></iframe><div class="pv-empty pane-empty"></div></div>`;
  const input = el.querySelector('.pv-url');
  const frame = el.querySelector('.pv-frame');
  const go = (u) => {
    let url = (u || '').trim();
    if (/^\d+$/.test(url)) url = `http://localhost:${url}`;
    if (url && !/^https?:\/\//i.test(url)) url = `http://${url}`;
    if (url && !LOCAL.test(url)) { errorToast(new Error(t('preview_local_only'))); return; }
    st(ctx.convId).url = url;
    input.value = url;
    render(el, ctx);
  };
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); go(input.value); } });
  el.querySelector('[data-a=reload]').onclick = () => { const u = st(ctx.convId).url; if (u) { frame.src = 'about:blank'; setTimeout(() => (frame.src = u), 30); } };
  el.querySelector('[data-a=ext]').onclick = () => { const u = st(ctx.convId).url; if (u) call('open_external', { url: u }).catch(errorToast); };
  el.querySelector('[data-a=pick]').onclick = async (ev) => {
    const s = await detect(ctx);
    const { showMenu } = await import('../util.js');
    showMenu(ev.currentTarget, s.found.length ? s.found.map((f) => ({ label: f.url, sub: `${f.source || ''}${f.running ? '' : ' · ' + t('stopped')}`, icon: 'globe', onClick: () => go(f.url) })) : [{ label: t('preview_none'), disabled: true }], { placement: 'bottom-end', width: 320 });
  };
  el.__go = go;
}

function render(el, ctx) {
  const s = st(ctx.convId);
  const frame = el.querySelector('.pv-frame');
  const empty = el.querySelector('.pv-empty');
  el.querySelector('.pv-url').value = s.url;
  if (s.url) {
    if (frame.dataset.src !== s.url) { frame.src = s.url; frame.dataset.src = s.url; }
    frame.classList.remove('hidden');
    empty.classList.add('hidden');
  } else {
    frame.classList.add('hidden');
    empty.classList.remove('hidden');
    empty.innerHTML = `${icon('globe', 22)}<div>${esc(t('preview_empty'))}</div>`;
  }
}

registerPane({
  id: 'preview',
  order: 50,
  icon: 'globe',
  label: () => t('ws_preview'),
  badge: (ctx) => (st(ctx.convId).found.some((f) => f.running) && !st(ctx.convId).url ? '•' : ''),
  // show() comes right after: it looks for addresses and draws
  mount: (el, ctx) => { mount(el, ctx); },
  show: async (el, ctx) => { await detect(ctx); render(el, ctx); },
});

/** Open the preview pane at a URL (used by links to localhost). */
export function openPreview(url) {
  const ctx = context();
  if (!ctx) return false;
  st(ctx.convId).url = url;
  show('preview');
  return true;
}

on('shells-changed', ({ conversation_id }) => {
  const c = context();
  if (c && c.convId === conversation_id) setTimeout(() => detect(c), 1500);
});
