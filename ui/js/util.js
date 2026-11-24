// DOM helpers, menus, modals and toasts.
import { icon } from './icons.js';
import { t } from './i18n.js';
import { friendlyError } from './neterr.js';

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

/** The search icon of a page header: it shows and hides the page's search box (as in Claude). */
export function searchToggle(view) {
  const box = $('.search-box', view);
  const head = $('.page-head', view);
  if (!box || !head) return;
  box.classList.add('hidden');
  const b = h(`<button class="icon-btn" title="${esc(t('sc_search'))}" aria-label="${esc(t('sc_search'))}">${icon('search', 16)}</button>`);
  b.onclick = () => { box.classList.toggle('hidden'); if (!box.classList.contains('hidden')) $('input', box).focus(); };
  head.insertBefore(b, $('.btn', head));
}

export function debounce(fn, ms) {
  let tm;
  return (...a) => { clearTimeout(tm); tm = setTimeout(() => fn(...a), ms); };
}

// Markers for the shape of a snapshot (objects, so no leaf value can equal them).
const ARR = {};
const OBJ = {};

/**
 * A snapshot of JSON-like data for change detection: its keys, leaves and
 * shape, in order. Strings are immutable, so the snapshot keeps them as
 * they are: far cheaper than JSON.stringify (no escaping, no copying), and
 * comparing two snapshots compares strings, a memcmp at worst. Unlike
 * keeping the object itself, it can't change when the object is updated in
 * place.
 */
export function snapshot(v, out = []) {
  if (v === null || typeof v !== 'object') { out.push(v); return out; }
  if (Array.isArray(v)) {
    out.push(ARR, v.length);
    for (const x of v) snapshot(x, out);
    return out;
  }
  const keys = Object.keys(v);
  out.push(OBJ, keys.length);
  for (const k of keys) { out.push(k); snapshot(v[k], out); }
  return out;
}

/** Whether two snapshots (see `snapshot`) are of equal data. */
export function sameSnapshot(a, b) {
  if (!a || !b || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
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
  const loc = document.documentElement.lang || undefined;
  if (ms >= startToday) return d.toLocaleTimeString(loc, { hour: '2-digit', minute: '2-digit' });
  if (ms >= startToday - day) return t('yesterday');
  const days = Math.floor((startToday - ms) / day) + 1;
  if (days < 7) return t('days_ago', { n: days });
  return d.toLocaleDateString(loc, { month: 'short', day: 'numeric', year: d.getFullYear() === now.getFullYear() ? undefined : 'numeric' });
}

/** A conversation's name; an untitled chat is "New chat" everywhere, as in the sidebar. */
export function convTitle(c) {
  return c.title || (c.kind === 'code' ? t('untitled') : t('new_chat'));
}

/** One line of a message without its Markdown: code fences, table pipes and
 *  rules, heading, quote and bullet marks, bold, code ticks, link targets. */
function plainLine(line) {
  if (/^\s*(```|~~~)/.test(line) || /^[\s|:…-]*-{3,}[\s|:…-]*$/.test(line)) return '';
  // a table row (the snippet may cut it at either end, after "…")
  if (/^[\s…]*\||\|[\s…]*$/.test(line)) line = line.replace(/\s*\|\s*/g, ' ');
  return line
    .replace(/^([\s…]*)(?:#{1,6}|>+|[-*+])\s+/, '$1')
    .replace(/!?\[([^\]\n]*)\]\([^)\n]*\)/g, '$1')
    .replace(/\*\*|__|~~|`/g, '');
}

/** A search snippet as HTML: plain text, with the backend's match markers
 *  (\u0001…\u0002) as <mark>. */
export function snippetHtml(s) {
  const plain = String(s || '').split('\n').map(plainLine).join(' ').replace(/\s+/g, ' ').trim();
  return esc(plain).replace(/\u0001/g, '<mark>').replace(/\u0002/g, '</mark>');
}

/** `s` as search compares it (as db.rs fold does): lower case, I/İ/ı all i, no accents. */
export const fold = (s) => String(s ?? '').replace(/[Iİı]/g, 'i').normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();

/** Whether `text` holds every word of `query`, in any order (an empty query matches). */
export function hasWords(text, query) {
  const hay = fold(text);
  return fold(query).split(/\s+/).filter(Boolean).every((w) => hay.includes(w));
}

/** Size line for an attachment chip; notes when a large file was digested. */
export function attachmentSub(a) {
  const size = humanSize(a.size || 0);
  if (!a.digest) return esc(size);
  return `${esc(size)} · <span class="att-digest" title="${esc(t('att_digest_tip', { lines: a.digest.lines, errors: a.digest.errors, warnings: a.digest.warnings }))}">${esc(t('att_digest'))}</span>`;
}

/** Dollar amount: cents, or more digits for amounts under a cent. */
export function fmtUsd(n) {
  const v = Number(n) || 0;
  return `$${v > 0 && v < 0.01 ? v.toPrecision(2) : v.toFixed(2)}`;
}

/** `p` relative to the folder `root` ('.' for the folder itself), with '/'
 *  separators; `p` as it is when it lies outside. */
export function relPath(p, root) {
  const norm = (s) => String(s || '').replace(/\\/g, '/').replace(/\/+$/, '');
  const f = norm(p);
  const r = norm(root);
  if (!r) return p;
  // Windows paths compare without case (C:\Src and c:/src are one folder)
  const win = /^[a-z]:\//i.test(r);
  const fc = win ? f.toLowerCase() : f;
  const rc = win ? r.toLowerCase() : r;
  if (fc === rc) return '.';
  return fc.startsWith(rc + '/') ? f.slice(r.length + 1) : p;
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
export function toast(msg, kind = '', { action, onAction, duration } = {}) {
  let root = $('.toast-root');
  if (!root) { root = h('<div class="toast-root"></div>'); document.body.appendChild(root); }
  const el = h(`<div class="toast ${kind}">${esc(msg)}${action ? ` <button class="toast-action">${esc(action)}</button>` : ''}</div>`);
  if (action) el.querySelector('.toast-action').onclick = () => { el.remove(); onAction && onAction(); };
  root.appendChild(el);
  const ms = duration || (kind === 'error' ? 4500 : action ? 8000 : 2200);
  setTimeout(() => { el.style.opacity = '0'; el.style.transition = 'opacity .3s'; }, ms);
  setTimeout(() => el.remove(), ms + 400);
}

/** A failure as a toast: a transport error said plainly, its raw text a click away. */
export function errorToast(e) {
  const raw = (e && e.message) || String(e);
  const plain = friendlyError(e);
  if (!plain || plain === raw) { toast(plain || raw, 'error'); return; }
  toast(plain, 'error', { action: t('err_details'), onAction: () => modal({ title: t('err_details'), body: h(`<div class="mono small" style="white-space:pre-wrap;word-break:break-word">${esc(raw)}</div>`) }) });
}

/** How a scroll the app makes moves: at once when the system asks for reduced motion. */
export function scrollBehavior() {
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth';
}

// ------------------------------------------------------------------- menus
let openMenu = null;

/** Where `key` moves the focus among the `n` controls of a menu from the one
 *  at `at` (-1: none of them): an index, or -1 when the key doesn't move it. */
export function menuStep(n, at, key) {
  if (!n) return -1;
  if (key === 'Home') return 0;
  if (key === 'End') return n - 1;
  if (key === 'ArrowDown') return at < 0 ? 0 : (at + 1) % n;
  if (key === 'ArrowUp') return at < 0 ? n - 1 : (at - 1 + n) % n;
  return -1;
}

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
  for (const it of items) {
    if (!it) continue;
    if (it.sep) { el.appendChild(h('<div class="menu-sep"></div>')); continue; }
    if (it.header) { el.appendChild(h(`<div class="menu-label">${esc(it.header)}</div>`)); continue; }
    if (it.element) { el.appendChild(it.element); continue; }
    // a choice among several (a model, a version): which one is on is said, not only drawn
    const role = 'checked' in it ? `role="menuitemradio" aria-checked="${!!it.checked}"` : 'role="menuitem"';
    const b = h(`<button class="menu-item ${it.danger ? 'danger' : ''}" ${role}>
      ${it.icon ? icon(it.icon, 16) : ''}
      <span class="mtext">${esc(it.label)}${it.sub ? `<span class="sub">${esc(it.sub)}</span>` : ''}</span>
      ${it.right ? `<span class="right">${esc(it.right)}</span>` : ''}
      ${it.checked ? `<span class="check">${icon('check', 16)}</span>` : ''}
    </button>`);
    if (it.disabled) b.disabled = true;
    b.addEventListener('click', (e) => {
      e.stopPropagation();
      if (!it.keepOpen) closeMenu();
      // async actions report their failure instead of rejecting unseen
      if (it.onClick) (async () => it.onClick(e))().catch(errorToast);
    });
    el.appendChild(b);
  }
  document.body.appendChild(el);
  position(el, anchor, opts.placement || 'bottom-start');
  const outside = (e) => { if (!el.contains(e.target) && !(anchor && anchor.contains(e.target))) closeMenu(); };
  const key = (e) => {
    if (e.key === 'Escape') { e.preventDefault(); closeMenu(); anchor && anchor.focus && anchor.focus(); return; }
    // a text field keeps its own Home and End
    if ((e.key === 'Home' || e.key === 'End') && e.target.matches && e.target.matches('input:not([type=checkbox]), textarea')) return;
    // every control of the menu, its switches and effort levels too
    const all = [...el.querySelectorAll('button:not(:disabled), input[type=checkbox]:not(:disabled)')];
    const to = menuStep(all.length, all.indexOf(document.activeElement), e.key);
    if (to < 0) return;
    e.preventDefault();
    all.forEach((b) => b.classList.remove('kb'));
    all[to].classList.add('kb');
    all[to].focus();
  };
  const entry = { el, anchor, outside, key, onClose: opts.onClose };
  setTimeout(() => {
    // closed (or replaced by another menu) before this ran: closeMenu has
    // already been, and listeners added now would never be removed
    if (openMenu !== entry) return;
    document.addEventListener('mousedown', outside, true);
    document.addEventListener('keydown', key, true);
  });
  if (anchor) anchor.setAttribute('aria-expanded', 'true');
  openMenu = entry;
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
    ${title !== undefined ? `<div class="modal-head"><h2>${esc(title)}</h2><button class="icon-btn" data-close title="${t('close')}" aria-label="${t('close')}">${icon('x', 18)}</button></div>` : ''}
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
  const opener = document.activeElement;
  document.body.appendChild(scrim);
  const close = () => {
    scrim.remove();
    document.removeEventListener('keydown', key, true);
    if (opener && opener.isConnected) opener.focus({ preventScroll: true });
    onClose && onClose();
  };
  // every open dialog listens here, the bottom one first: Esc closes only the top one
  const isTop = () => Array.from(document.querySelectorAll('.modal-scrim')).pop() === scrim;
  const key = (e) => { if (e.key === 'Escape' && !$('.popover') && isTop()) { e.preventDefault(); e.stopPropagation(); close(); } };
  document.addEventListener('keydown', key, true);
  scrim.addEventListener('mousedown', (e) => { if (e.target === scrim) close(); });
  m.querySelectorAll('[data-close]').forEach((b) => b.addEventListener('click', close));
  return { el: m, body: bodyEl, close, scrim };
}

export function button(label, cls, onClick) {
  const b = h(`<button class="btn ${cls || ''}">${esc(label)}</button>`);
  if (onClick) b.addEventListener('click', (e) => { (async () => onClick(e))().catch(errorToast); });
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
  // empty: as tall as its placeholder, so a long one isn't cut off in a narrow window
  const need = !ta.value && ta.placeholder ? Math.max(ta.scrollHeight, placeholderHeight(ta)) : ta.scrollHeight;
  ta.style.height = Math.min(need, limit) + 'px';
  ta.style.overflowY = ta.scrollHeight > limit ? 'auto' : 'hidden';
}

// a hidden copy measures the placeholder (setting the field's own value would lose its undo history)
let mirror = null;
function placeholderHeight(ta) {
  if (!mirror) { mirror = document.createElement('textarea'); mirror.tabIndex = -1; mirror.setAttribute('aria-hidden', 'true'); }
  const s = getComputedStyle(ta);
  mirror.style.cssText = `position:fixed;left:-10000px;top:0;visibility:hidden;height:0;overflow:hidden;width:${ta.offsetWidth}px;box-sizing:${s.boxSizing};padding:${s.padding};border:${s.border};font:${s.font};line-height:${s.lineHeight};letter-spacing:${s.letterSpacing}`;
  mirror.value = ta.placeholder;
  document.body.appendChild(mirror);
  const hgt = mirror.scrollHeight;
  mirror.remove();
  return hgt;
}

export function fileToBase64(file) {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result).split(',')[1] || '');
    r.onerror = reject;
    r.readAsDataURL(file);
  });
}

export function lightbox(src, alt = '') {
  const el = h(`<div class="lightbox" role="dialog" aria-modal="true" aria-label="${esc(alt || t('img_view'))}" tabindex="-1"><img src="${esc(src)}" alt="${esc(alt)}"></div>`);
  const opener = document.activeElement;
  const close = () => {
    el.remove();
    document.removeEventListener('keydown', key, true);
    if (opener && opener.isConnected) opener.focus({ preventScroll: true });
  };
  const key = (e) => { if (e.key === 'Escape' || e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.stopPropagation(); close(); } };
  el.addEventListener('click', close);
  document.addEventListener('keydown', key, true);
  document.body.appendChild(el);
  el.focus();
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
