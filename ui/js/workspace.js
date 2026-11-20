// Side workspace panel for code sessions: a resizable aside with tabs
// (Changes, Terminal, Plan, Tasks, Preview, …). Each tab is a small module
// registered with registerPane(); panes keep their DOM per conversation so
// switching tabs or conversations doesn't lose terminal state.
import { t } from './i18n.js';
import { icon } from './icons.js';
import { h, esc } from './util.js';
import { loadLocal, saveLocal } from './store.js';

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
  tabs.innerHTML = '';
  for (const p of panes) {
    if (p.visible && !p.visible(ctx)) continue;
    const badge = p.badge ? p.badge(ctx) : '';
    const b = h(`<button class="ws-tab ${p.id === active ? 'on' : ''}" data-pane="${p.id}" title="${esc(p.label())}">${icon(p.icon, 14)}<span>${esc(p.label())}</span>${badge ? `<span class="ws-badge">${esc(String(badge))}</span>` : ''}</button>`);
    b.onclick = () => { active = p.id; persist(); render(); };
    tabs.appendChild(b);
  }
}

function render() {
  const el = aside();
  if (!el) return;
  const visible = isOpen();
  const wasHidden = el.classList.contains('hidden');
  el.classList.toggle('hidden', !visible);
  // one side panel at a time: the artifact panel closes when this opens
  if (visible && wasHidden) window.dispatchEvent(new Event('workspace-opened'));
  document.querySelectorAll('.ws-toggle').forEach((b) => b.classList.toggle('on', visible));
  // hide every pane element; show the active one
  for (const [, pe] of els) pe.classList.add('hidden');
  if (!visible) { for (const p of panes) p.hide && p.hide(); return; }
  if (!el.querySelector('.ws-head')) {
    el.innerHTML = `<div class="resizer"></div><div class="ws-head"><div class="ws-tabs"></div><div class="grow"></div><button class="icon-btn ws-close" title="${t('close')}">${icon('x', 16)}</button></div><div class="ws-body"></div>`;
    el.querySelector('.ws-close').onclick = close;
    bindResizer(el, el.querySelector('.resizer'));
    const w = loadLocal().workspaceWidth;
    if (w) el.style.width = w + 'px';
  }
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
}

function bindResizer(el, r) {
  r.addEventListener('mousedown', (e) => {
    e.preventDefault();
    r.classList.add('drag');
    document.body.classList.add('resizing');
    const frames = document.querySelectorAll('iframe');
    frames.forEach((f) => (f.style.pointerEvents = 'none'));
    const move = (ev) => {
      const right = el.getBoundingClientRect().right;
      const w = Math.max(300, Math.min(window.innerWidth * 0.8, right - ev.clientX));
      el.style.width = w + 'px';
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
}

/** The topbar button that opens/closes the panel. */
export function toggleButton() {
  const b = h(`<button class="icon-btn ws-toggle ${isOpen() ? 'on' : ''}" title="${t('ws_toggle')} (Ctrl+\`)">${icon('sidebar', 17)}</button>`);
  b.style.transform = 'scaleX(-1)';
  b.onclick = () => toggle();
  return b;
}

window.addEventListener('artifact-opened', () => { if (open) { open = false; render(); } });

window.addEventListener('keydown', (e) => {
  if (!ctx) return;
  if ((e.ctrlKey || e.metaKey) && e.key === '`') { e.preventDefault(); toggle('terminal'); }
});
