// Side workspace panel for code sessions: a resizable aside with tabs
// (Changes, Terminal, Plan, Tasks, Preview, …). Each tab is a small module
// registered with registerPane(); panes keep their DOM per conversation so
// switching tabs or conversations doesn't lose terminal state.
import * as keymap from './keymap.js';
import { t } from './i18n.js';
import { icon } from './icons.js';
import { h, esc } from './util.js';
import { loadLocal, saveLocal } from './store.js';
import { nextTab } from './panes/rows.js';

const panes = [];          // { id, label, icon, mount(el, ctx), show?(ctx), hide?(), dispose?(convId), badge?(ctx) }
let ctx = null;            // { convId, cwd, conv }
let open = false;
let active = null;
const els = new Map();     // `${convId}|${paneId}` -> element

function aside() { return document.getElementById('workspace-panel'); }

export function registerPane(p) {
  if (!panes.some((x) => x.id === p.id)) panes.push(p);
  panes.sort((a, b) => (a.order || 0) - (b.order || 0));
}

export function isOpen() { return open && !!ctx; }
export function context() { return ctx; }
export function activePane() { return active; }

/** Called when a conversation is shown; code sessions get the panel. */
export function setConversation(conv) {
  const prev = ctx;
  if (!conv || conv.kind !== 'code') {
    ctx = null;
    render();
    return;
  }
  ctx = { convId: conv.id, cwd: conv.cwd, conv };
  if (!prev || prev.convId !== conv.id) {
    const saved = loadLocal().workspace || {};
    open = !!saved.open;
    active = saved.tab || (panes[0] && panes[0].id);
  }
  render();
}

export function updateConversation(conv) {
  if (ctx && conv && conv.id === ctx.convId) { ctx.conv = conv; ctx.cwd = conv.cwd; }
}

function persist() {
  const w = loadLocal().workspace || {};
  saveLocal({ workspace: { ...w, open, tab: active } });
}

export function toggle(tab) {
  if (!ctx) return;
  if (tab && (!open || active !== tab)) { open = true; active = tab; }
  else open = !open;
  persist();
  render();
}

export function show(tab) {
  if (!ctx) return;
  open = true;
  if (tab) active = tab;
  persist();
  render();
}

export function close() { open = false; persist(); render(); }

/** Re-render the active pane (e.g. after an event). */
export function refresh(paneId) {
  if (!isOpen() || (paneId && paneId !== active)) return;
  const p = panes.find((x) => x.id === active);
  const el = els.get(`${ctx.convId}|${active}`);
  if (p && el && p.show) p.show(el, ctx);
  renderTabs();
}

export function disposeConversation(convId) {
  for (const p of panes) {
    const key = `${convId}|${p.id}`;
    if (els.has(key)) { p.dispose && p.dispose(convId, els.get(key)); els.get(key).remove(); els.delete(key); }
  }
}

export function updateTabs() { renderTabs(); }

function renderTabs() {
  const el = aside();
  const tabs = el && el.querySelector('.ws-tabs');
  if (!tabs || !ctx) return;
  const html = panes.filter((p) => !p.visible || p.visible(ctx)).map((p) => {
    const badge = p.badge ? p.badge(ctx) : '';
    const on = p.id === active;
    return `<button class="ws-tab ${on ? 'on' : ''}" role="tab" aria-selected="${on}" tabindex="${on ? 0 : -1}" data-pane="${p.id}" title="${esc(p.label())}">${icon(p.icon, 14)}<span>${esc(p.label())}</span>${badge ? `<span class="ws-badge">${esc(String(badge))}</span>` : ''}</button>`;
  }).join('');
  // drawn again only when something changed (the badges are asked for every
  // 2 s): a tab drawn again under the keyboard loses its focus
  if (tabs.__html === html) return;
  const focused = tabs.contains(document.activeElement) ? document.activeElement.dataset.pane : null;
  tabs.__html = html;
  tabs.innerHTML = html;
  const b = focused && tabs.querySelector(`[data-pane="${focused}"]`);
  if (b) b.focus();
}

function bindTabs(tabs) {
  tabs.addEventListener('click', (e) => {
    const b = e.target.closest('.ws-tab');
    if (b) { active = b.dataset.pane; persist(); render(); }
  });
  // arrow keys move along the tabs and show each one
  tabs.addEventListener('keydown', (e) => {
    const list = [...tabs.querySelectorAll('.ws-tab')];
    const j = nextTab(list.indexOf(e.target), list.length, e.key);
    if (j < 0) return;
    e.preventDefault();
    const id = list[j].dataset.pane;
    active = id; persist(); render();
    tabs.querySelector(`[data-pane="${id}"]`).focus();
  });
}

const MIN_W = 300;
const MIN_CHAT = 400; // a code session's top bar needs about 400 px
const DEFAULT_W = 460;

/** The panel's width for a wanted `w` when panel and chat share `room` pixels: the chat keeps MIN_CHAT. */
export function clampWidth(w, room) {
  return Math.round(Math.max(MIN_W, Math.min(w, room - MIN_CHAT)));
}

/** The width the panel and the chat share: the window right of the sidebar. */
function room() {
  const sb = document.getElementById('sidebar');
  return window.innerWidth - (sb && !sb.classList.contains('hidden') ? sb.getBoundingClientRect().right : 0);
}

function setWidth(el, w) {
  const cw = clampWidth(w, room());
  el.style.width = cw + 'px';
  const r = el.querySelector('.resizer');
  if (r) { r.setAttribute('aria-valuenow', String(cw)); r.setAttribute('aria-valuemax', String(clampWidth(Infinity, room()))); }
  return cw;
}

// in a narrow window the panel is a sheet over the chat (CSS)
const sheet = () => window.matchMedia('(max-width: 900px)').matches;

function render() {
  const el = aside();
  if (!el) return;
  const visible = isOpen();
  const wasHidden = el.classList.contains('hidden');
  el.classList.toggle('hidden', !visible);
  // only shown in narrow windows (CSS), where the panel is a sheet over the chat
  let scrim = document.getElementById('workspace-scrim');
  if (!scrim) { scrim = h('<div class="ws-scrim" id="workspace-scrim"></div>'); scrim.onclick = close; el.before(scrim); }
  scrim.classList.toggle('hidden', !visible);
  // one side panel at a time: the artifact panel closes when this opens
  if (visible && wasHidden) window.dispatchEvent(new Event('workspace-opened'));
  document.querySelectorAll('.ws-toggle').forEach((b) => { b.classList.toggle('on', visible); b.setAttribute('aria-pressed', String(visible)); });
  // hide every pane element; show the active one
  for (const [, pe] of els) pe.classList.add('hidden');
  if (!visible) { for (const p of panes) p.hide && p.hide(); return; }
  if (!el.querySelector('.ws-head')) {
    el.innerHTML = `<div class="resizer" role="separator" aria-orientation="vertical" tabindex="0" aria-label="${esc(t('ws_resize'))}" aria-valuemin="${MIN_W}"></div><div class="ws-head"><div class="ws-tabs" role="tablist" aria-label="${esc(t('ws_toggle'))}"></div><div class="grow"></div><button class="icon-btn ws-close" title="${esc(t('close'))}" aria-label="${esc(t('close'))}">${icon('x', 16)}</button></div><div class="ws-body" role="tabpanel"></div>`;
    el.querySelector('.ws-close').onclick = closeAndReturn;
    bindTabs(el.querySelector('.ws-tabs'));
    bindResizer(el, el.querySelector('.resizer'));
    // Esc leaves the sheet a narrow window shows (a field, the terminal or a menu keeps its own Esc)
    el.addEventListener('keydown', (e) => {
      if (e.key !== 'Escape' || e.defaultPrevented || !sheet() || document.querySelector('.modal-scrim, .popover')) return;
      const tg = e.target;
      if (tg.closest('.term-host') || tg.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(tg.tagName)) return;
      e.preventDefault();
      closeAndReturn();
    });
  }
  if (!sheet()) setWidth(el, loadLocal().workspaceWidth || DEFAULT_W);
  if (!panes.some((p) => p.id === active)) active = panes[0] && panes[0].id;
  renderTabs();
  const p = panes.find((x) => x.id === active);
  if (!p) return;
  for (const q of panes) if (q !== p && q.hide) q.hide();
  const key = `${ctx.convId}|${p.id}`;
  let pe = els.get(key);
  const body = el.querySelector('.ws-body');
  if (!pe) {
    pe = h(`<div class="ws-pane" data-pane="${p.id}"></div>`);
    body.appendChild(pe);
    els.set(key, pe);
    p.mount(pe, ctx);
  } else if (pe.parentNode !== body) {
    body.appendChild(pe);
  }
  pe.classList.remove('hidden');
  if (p.show) p.show(pe, ctx);
  // a sheet that just opened over the chat takes the focus (a pane that took it, like the terminal, keeps it)
  if (wasHidden && sheet()) requestAnimationFrame(() => { if (!el.contains(document.activeElement)) { const t0 = el.querySelector('.ws-tab.on'); if (t0) t0.focus(); } });
}

/** Close, and give focus back to the button that opens the panel when it was inside. */
function closeAndReturn() {
  const inside = aside() && aside().contains(document.activeElement);
  close();
  const b = inside && document.querySelector('.ws-toggle');
  if (b) b.focus();
}

function bindResizer(el, r) {
  r.addEventListener('mousedown', (e) => {
    e.preventDefault();
    r.classList.add('drag');
    document.body.classList.add('resizing');
    const frames = document.querySelectorAll('iframe');
    frames.forEach((f) => (f.style.pointerEvents = 'none'));
    const move = (ev) => {
      setWidth(el, el.getBoundingClientRect().right - ev.clientX);
      window.dispatchEvent(new Event('workspace-resize'));
    };
    const up = () => {
      r.classList.remove('drag');
      document.body.classList.remove('resizing');
      frames.forEach((f) => (f.style.pointerEvents = ''));
      document.removeEventListener('mousemove', move);
      document.removeEventListener('mouseup', up);
      saveLocal({ workspaceWidth: parseInt(el.style.width, 10) });
      window.dispatchEvent(new Event('workspace-resize'));
    };
    document.addEventListener('mousemove', move);
    document.addEventListener('mouseup', up);
  });
  // the keyboard: ← widens the panel, → narrows it
  r.addEventListener('keydown', (e) => {
    const step = e.key === 'ArrowLeft' ? 24 : e.key === 'ArrowRight' ? -24 : 0;
    if (!step) return;
    e.preventDefault();
    saveLocal({ workspaceWidth: setWidth(el, el.getBoundingClientRect().width + step) });
    window.dispatchEvent(new Event('workspace-resize'));
  });
}

// a smaller window: the panel gives way so the chat keeps its room
window.addEventListener('resize', () => {
  const el = aside();
  if (el && isOpen() && !sheet()) setWidth(el, loadLocal().workspaceWidth || DEFAULT_W);
});

/** The topbar button that opens/closes the panel. */
export function toggleButton() {
  const b = h(`<button class="icon-btn ws-toggle ${isOpen() ? 'on' : ''}" title="${t('ws_toggle')} (Ctrl+\`)" aria-label="${esc(t('ws_toggle'))}" aria-pressed="${isOpen()}">${icon('sidebar', 17)}</button>`);
  b.style.transform = 'scaleX(-1)';
  b.onclick = () => toggle();
  return b;
}

window.addEventListener('artifact-opened', () => { if (open) { open = false; render(); } });

window.addEventListener('keydown', (e) => {
  if (!ctx) return;
  if (keymap.matches(e, 'workspace')) { e.preventDefault(); toggle('terminal'); }
});
