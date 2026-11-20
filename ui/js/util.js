// DOM helpers, menus, modals and toasts.
import { icon } from './icons.js';
import { t } from './i18n.js';

export const $ = (sel, root = document) => root.querySelector(sel);
export const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

export function esc(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

export function h(html) {
  const tpl = document.createElement('template');
  tpl.innerHTML = html.trim();
  return tpl.content.firstElementChild;
}

export function debounce(fn, ms) {
  let tm;
  return (...a) => { clearTimeout(tm); tm = setTimeout(() => fn(...a), ms); };
}

export function initials(name) {
  const parts = String(name || '').trim().split(/\s+/).filter(Boolean);
  if (!parts.length) return 'Y';
  return (parts[0][0] + (parts.length > 1 ? parts[parts.length - 1][0] : '')).toUpperCase();
}

export function relTime(ms) {
  const d = new Date(ms);
  const now = new Date();
  const day = 86400000;
  const startToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (ms >= startToday) return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  if (ms >= startToday - day) return t('yesterday');
  const days = Math.floor((startToday - ms) / day) + 1;
  if (days < 7) return t('days_ago', { n: days });
  return d.toLocaleDateString([], { month: 'short', day: 'numeric', year: d.getFullYear() === now.getFullYear() ? undefined : 'numeric' });
}

export function humanSize(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function fmtTokens(n) {
  if (n >= 1e6) return (n / 1e6).toFixed(2) + 'M';
  if (n >= 1e3) return (n / 1e3).toFixed(1) + 'k';
  return String(n);
}

// ------------------------------------------------------------------ toasts
export function toast(msg, kind = '') {
  let root = $('.toast-root');
  if (!root) { root = h('<div class="toast-root"></div>'); document.body.appendChild(root); }
  const el = h(`<div class="toast ${kind}">${esc(msg)}</div>`);
  root.appendChild(el);
  setTimeout(() => { el.style.opacity = '0'; el.style.transition = 'opacity .3s'; }, kind === 'error' ? 4500 : 2200);
  setTimeout(() => el.remove(), kind === 'error' ? 4900 : 2600);
}

export function errorToast(e) { toast((e && e.message) || String(e), 'error'); }

// ------------------------------------------------------------------- menus
let openMenu = null;

export function closeMenu() {
  if (openMenu) {
    const m = openMenu;
    openMenu = null;
    m.el.remove();
    if (m.anchor) m.anchor.setAttribute('aria-expanded', 'false');
    document.removeEventListener('mousedown', m.outside, true);
    document.removeEventListener('keydown', m.key, true);
    if (m.onClose) m.onClose();
  }
}

/**
 * Show a popover menu anchored to an element.
 * items: [{label, sub, icon, onClick, danger, checked, right, sep, header, html, keepOpen}]
 */
export function showMenu(anchor, items, opts = {}) {
  closeMenu();
  const el = h('<div class="popover" role="menu"></div>');
  if (opts.width) el.style.minWidth = opts.width + 'px';
  const btns = [];
  for (const it of items) {
    if (!it) continue;
    if (it.sep) { el.appendChild(h('<div class="menu-sep"></div>')); continue; }
    if (it.header) { el.appendChild(h(`<div class="menu-label">${esc(it.header)}</div>`)); continue; }
    if (it.element) { el.appendChild(it.element); continue; }
    const b = h(`<button class="menu-item ${it.danger ? 'danger' : ''}" role="menuitem">
      ${it.icon ? icon(it.icon, 16) : ''}
      <span class="mtext">${esc(it.label)}${it.sub ? `<span class="sub">${esc(it.sub)}</span>` : ''}</span>
      ${it.right ? `<span class="right">${esc(it.right)}</span>` : ''}
      ${it.checked ? `<span class="check">${icon('check', 16)}</span>` : ''}
    </button>`);
    if (it.disabled) b.disabled = true;
    b.addEventListener('click', (e) => {
      e.stopPropagation();
      if (!it.keepOpen) closeMenu();
      it.onClick && it.onClick(e);
    });
    btns.push(b);
    el.appendChild(b);
  }
  document.body.appendChild(el);
  position(el, anchor, opts.placement || 'bottom-start');
  let idx = -1;
  const outside = (e) => { if (!el.contains(e.target) && !(anchor && anchor.contains(e.target))) closeMenu(); };
  const key = (e) => {
    if (e.key === 'Escape') { e.preventDefault(); closeMenu(); anchor && anchor.focus && anchor.focus(); }
    else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      btns.forEach((b) => b.classList.remove('kb'));
      idx = (idx + (e.key === 'ArrowDown' ? 1 : -1) + btns.length) % btns.length;
      btns[idx].classList.add('kb'); btns[idx].focus();
    }
  };
  setTimeout(() => {
    document.addEventListener('mousedown', outside, true);
    document.addEventListener('keydown', key, true);
  });
  if (anchor) anchor.setAttribute('aria-expanded', 'true');
  openMenu = { el, anchor, outside, key, onClose: opts.onClose };
  return el;
}

export function position(el, anchor, placement) {
  const r = anchor ? anchor.getBoundingClientRect() : { left: innerWidth / 2, right: innerWidth / 2, top: innerHeight / 2, bottom: innerHeight / 2, width: 0 };
  const w = el.offsetWidth, hgt = el.offsetHeight;
  let left = placement.endsWith('end') ? r.right - w : r.left;
  let top = placement.startsWith('top') ? r.top - hgt - 6 : r.bottom + 6;
  if (top + hgt > innerHeight - 8) top = Math.max(8, r.top - hgt - 6);
  if (top < 8) top = Math.min(innerHeight - hgt - 8, r.bottom + 6);
  left = Math.max(8, Math.min(left, innerWidth - w - 8));
  el.style.left = left + 'px';
  el.style.top = Math.max(8, top) + 'px';
}

// ------------------------------------------------------------------ modals
export function modal({ title, body, footer, wide = false, onClose, cls = '' }) {
  const scrim = h('<div class="modal-scrim"></div>');
  const m = h(`<div class="modal ${wide ? 'wide' : ''} ${cls}" role="dialog" aria-modal="true">
    ${title !== undefined ? `<div class="modal-head"><h2>${esc(title)}</h2><button class="icon-btn" data-close aria-label="${t('close')}">${icon('x', 18)}</button></div>` : ''}
    <div class="modal-body"></div>
    ${footer ? '<div class="modal-foot"></div>' : ''}
  </div>`);
  const bodyEl = m.querySelector('.modal-body');
  if (typeof body === 'string') bodyEl.innerHTML = body; else if (body) bodyEl.appendChild(body);
  if (footer) {
    const f = m.querySelector('.modal-foot');
    for (const b of footer) f.appendChild(b);
  }
  scrim.appendChild(m);
  document.body.appendChild(scrim);
  const close = () => {
    scrim.remove();
    document.removeEventListener('keydown', key, true);
    onClose && onClose();
  };
  const key = (e) => { if (e.key === 'Escape' && !$('.popover')) { e.preventDefault(); e.stopPropagation(); close(); } };
  document.addEventListener('keydown', key, true);
  scrim.addEventListener('mousedown', (e) => { if (e.target === scrim) close(); });
  m.querySelectorAll('[data-close]').forEach((b) => b.addEventListener('click', close));
  return { el: m, body: bodyEl, close, scrim };
}

export function button(label, cls, onClick) {
  const b = h(`<button class="btn ${cls || ''}">${esc(label)}</button>`);
  if (onClick) b.addEventListener('click', onClick);
  return b;
}

export function confirmDialog(message, { okLabel, danger = true } = {}) {
  return new Promise((resolve) => {
    let done = false;
    const finish = (v) => { if (!done) { done = true; resolve(v); } };
    const ok = button(okLabel || t('delete'), danger ? 'danger' : 'primary', () => { finish(true); m.close(); });
    const cancel = button(t('cancel'), '', () => { finish(false); m.close(); });
    const m = modal({ title: '', body: `<p style="margin:4px 0 6px;font-size:14.5px;line-height:1.5">${esc(message)}</p>`, footer: [cancel, ok], onClose: () => finish(false) });
    m.el.querySelector('.modal-head h2').remove();
    setTimeout(() => ok.focus(), 30);
  });
}

export function promptDialog(title, value = '', { multiline = false, placeholder = '', okLabel } = {}) {
  return new Promise((resolve) => {
    let done = false;
    const finish = (v) => { if (!done) { done = true; resolve(v); } };
    const input = multiline
      ? h(`<textarea class="textarea" rows="8" placeholder="${esc(placeholder)}"></textarea>`)
      : h(`<input class="input" placeholder="${esc(placeholder)}">`);
    input.value = value;
    const ok = button(okLabel || t('save'), 'primary', () => { finish(input.value); m.close(); });
    const cancel = button(t('cancel'), '', () => { finish(null); m.close(); });
    const m = modal({ title, body: input, footer: [cancel, ok], onClose: () => finish(null) });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && (!multiline || e.ctrlKey || e.metaKey)) { e.preventDefault(); ok.click(); }
    });
    setTimeout(() => { input.focus(); input.select && input.select(); }, 30);
  });
}

export function autoGrow(ta, max) {
  ta.style.height = 'auto';
  const limit = max || Math.round(window.innerHeight * 0.4);
  ta.style.height = Math.min(ta.scrollHeight, limit) + 'px';
  ta.style.overflowY = ta.scrollHeight > limit ? 'auto' : 'hidden';
}

export function fileToBase64(file) {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result).split(',')[1] || '');
    r.onerror = reject;
    r.readAsDataURL(file);
  });
}

export function lightbox(src) {
  const el = h(`<div class="lightbox"><img src="${esc(src)}" alt=""></div>`);
  el.addEventListener('click', () => el.remove());
  document.body.appendChild(el);
}

/** Replace children of `container` with `html`, keeping unchanged leading nodes. */
export function morph(container, html) {
  const tpl = document.createElement('template');
  tpl.innerHTML = html;
  const next = Array.from(tpl.content.childNodes);
  const prev = Array.from(container.childNodes);
  let i = 0;
  for (; i < next.length; i++) {
    const n = next[i];
    const src = n.nodeType === 1 ? n.outerHTML : n.textContent;
    const p = prev[i];
    if (p && p.__src === src) continue;
    n.__src = src;
    if (p) container.replaceChild(n, p); else container.appendChild(n);
    n.__fresh = true;
  }
  for (let j = prev.length - 1; j >= next.length; j--) prev[j].remove();
  return next.filter((n) => n.__fresh && !(n.__fresh = false) && n.nodeType === 1);
}
