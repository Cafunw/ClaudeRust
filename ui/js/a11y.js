// Screen-reader and keyboard support shared by the whole UI: live regions
// (answers as they stream, toasts, errors), and focus kept inside the top
// dialog and given back when it closes.

/** Where focus goes on Tab (`shift`: Shift+Tab) from the element at `at` of
 *  `n` focusable ones in a dialog: an index to focus, or -1 to let the
 *  browser move it (it stays inside). `at` is -1 when focus is outside. */
export function trapStep(n, at, shift) {
  if (!n) return -1;
  if (at < 0) return shift ? n - 1 : 0;
  if (shift && at === 0) return n - 1;
  if (!shift && at === n - 1) return 0;
  return -1;
}

/** The part of `text` (all said so far: `said` characters) to announce now:
 *  whole sentences only while it grows, the rest at the end. */
export function nextAnnouncement(text, said, final) {
  if (text.length < said) said = 0; // a new or rewritten answer
  const rest = text.slice(said);
  if (final) return { say: rest.trim(), said: text.length };
  const m = rest.match(/^[\s\S]*[.!?:\n](?=\s|$)/);
  return m ? { say: m[0].trim(), said: said + m[0].length } : { say: '', said };
}

/** Announces a growing answer to `say` at most every `wait` ms (not per
 *  token); `prepare` turns the answer into what is said (once per announcement). */
export function streamAnnouncer(say, { wait = 2500, timers = globalThis, prepare = (x) => x } = {}) {
  let text = '';
  let said = 0;
  let key = null;
  let finished = false;
  let timer = null;
  const stop = () => { if (timer) timers.clearTimeout(timer); timer = null; };
  const flush = (final) => {
    timer = null;
    const r = nextAnnouncement(prepare(text), said, final);
    said = r.said;
    if (r.say) say(r.say);
  };
  const api = {
    /** The answer `k` now reads `t`; `final` once it's finished (later updates of it are ignored). */
    update(k, t, final = false) {
      if (k !== key) { stop(); key = k; said = 0; finished = false; }
      if (finished) return;
      text = t;
      if (final) { api.finish(); return; }
      if (!timer) timer = timers.setTimeout(() => flush(false), wait);
    },
    /** The answer under way is finished: the rest of it is said now. */
    finish() {
      if (key === null || finished) return;
      stop();
      flush(true);
      finished = true;
    },
    reset() { stop(); text = ''; said = 0; key = null; finished = false; },
  };
  return api;
}

let regions = null;
function region(assertive) {
  if (!regions && typeof document !== 'undefined' && document.body) {
    const mk = (live) => {
      const el = document.createElement('div');
      el.className = 'sr-only';
      el.setAttribute('aria-live', live);
      el.setAttribute('aria-atomic', 'true');
      if (live === 'assertive') el.setAttribute('role', 'alert');
      return el;
    };
    regions = { polite: mk('polite'), assertive: mk('assertive') };
  }
  if (!regions) return null;
  const el = assertive ? regions.assertive : regions.polite;
  // (the app's layout replaces the body's contents at start)
  if (!el.isConnected) document.body.appendChild(el);
  return el;
}

/** How long words stay in a live region after they were put there: long enough to be read out. */
export const SAID_MS = 7000;

/** Say `text` to a screen reader (`assertive`: interrupting, for errors). */
export function announce(text, assertive = false) {
  const el = region(assertive);
  if (!el || !text) return;
  // emptied first, so the same words said twice are announced twice
  el.textContent = '';
  setTimeout(() => { el.textContent = text; }, 30);
  // and again once said: an answered prompt (in the language of the moment) doesn't stay in the page
  clearTimeout(el.saidTimer);
  el.saidTimer = setTimeout(() => { el.textContent = ''; }, SAID_MS);
}

/** Empty the live regions (the language changed: what they hold is in the old one). */
export function clearAnnouncements() {
  if (regions) for (const el of Object.values(regions)) { clearTimeout(el.saidTimer); el.textContent = ''; }
}

/** The control Tab (`shift`: Shift+Tab) moves to from `at` when it would leave the page (on to
 *  <body>): the first (last) of `list` when `at` is its last (first) one, else null (the browser's own step). */
export function wrapStep(list, at, shift) {
  if (!list.length) return null;
  const [first, last] = [list[0], list[list.length - 1]];
  if (shift ? at === first : at === last) return shift ? last : first;
  return null;
}

/** How a control is told from its neighbours, the same for a copy of it drawn again: its kind
 *  (tag, first class), data, name and role (`withTitle`: and its tooltip). */
export function controlKey(el, withTitle) {
  const attrs = [...el.attributes].filter((a) => a.name.startsWith('data-') || a.name === 'aria-label' || a.name === 'role' || (withTitle && a.name === 'title'));
  return [el.tagName, el.classList[0] || '', ...attrs.map((a) => `${a.name}=${a.value}`).sort()].join(' ');
}

/** The one control of `list` that is a copy of `old` (same key with its tooltip, else without), or null. */
export function copyOf(old, list) {
  for (const withTitle of [true, false]) {
    const k = controlKey(old, withTitle);
    const same = list.filter((x) => x !== old && controlKey(x, withTitle) === k);
    if (same.length) return same.length === 1 ? same[0] : null;
  }
  return null;
}

const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]):not([type=hidden]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), iframe';

/** A stop of Tab: shown, and not taken out of the order (a list's rows other than its current one). */
export const tabStop = (el) => el.tabIndex >= 0 && el.getClientRects().length > 0;

function focusables(root) {
  return [...root.querySelectorAll(FOCUSABLE)].filter((el) => tabStop(el) || el === document.activeElement);
}

const topDialog = () => { const s = document.querySelectorAll('.modal-scrim'); return s.length ? s[s.length - 1].querySelector('.modal') : null; };
// a panel a narrow window shows over a dimmed page (the sidebar, the side panel, the artifact sheet): its scrim is shown
const openSheet = () => {
  const s = [...document.querySelectorAll('.sidebar-scrim, .ws-scrim, .ap-scrim')].find((x) => getComputedStyle(x).display !== 'none');
  return s ? (s.classList.contains('sidebar-scrim') ? s.previousElementSibling : s.nextElementSibling) : null;
};

// (not in the Node tests, which have no DOM)
if (typeof MutationObserver !== 'undefined') {
  // Tab and Shift+Tab stay inside the top dialog, or a sheet over the dimmed page
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Tab' || e.defaultPrevented || document.querySelector('.popover')) return;
    const dlg = topDialog() || openSheet();
    if (!dlg) {
      // past the page's last control Tab comes round to its first (and Shift+Tab back): never on to <body>
      const all = [...document.querySelectorAll(FOCUSABLE)];
      const to = wrapStep([all.find(tabStop), all.findLast(tabStop)].filter(Boolean), document.activeElement, e.shiftKey);
      if (to) { e.preventDefault(); to.focus(); }
      return;
    }
    const list = focusables(dlg);
    const to = trapStep(list.length, list.indexOf(document.activeElement), e.shiftKey);
    if (!list.length) { e.preventDefault(); return; }
    if (to >= 0) { e.preventDefault(); list[to].focus(); }
  }, true);

  // the last place focus was outside menus and dialogs: where it goes back
  // to when a dialog closes and the element that opened it is gone (an item
  // of a menu, say)
  // (both weak: a chat closed with its composer focused must not keep its whole DOM alive)
  let lastOutside = null;
  // where the focused control sits: when a redraw replaces it (a title that arrived, a list
  // drawn again), focus goes to its new copy instead of falling to <body>
  let ancestors = [];
  document.addEventListener('focusin', (e) => {
    if (e.target instanceof Element && !e.target.closest('.popover, .modal-scrim')) lastOutside = new WeakRef(e.target);
    ancestors = [];
    for (let p = e.target.parentElement; p; p = p.parentElement) ancestors.push(new WeakRef(p));
  }, true);
  document.addEventListener('focusout', (e) => {
    const lost = e.target;
    const path = ancestors.map((r) => r.deref()).filter(Boolean);
    // (menus and dialogs give focus back themselves)
    if (e.relatedTarget || !(lost instanceof Element) || path.some((p) => p.matches('.popover, .modal-scrim'))) return;
    requestAnimationFrame(() => {
      if (lost.isConnected || (document.activeElement || document.body) !== document.body) return;
      const scope = path.find((p) => p.isConnected);
      const to = scope && copyOf(lost, focusables(scope));
      if (to) to.focus({ preventScroll: true });
    });
  }, true);

  new MutationObserver((muts) => {
    for (const m of muts) {
      for (const n of m.addedNodes) {
        if (n.nodeType !== 1) continue;
        // a dialog opened: focus into it, unless it put it somewhere itself (after its own setTimeout)
        if (n.classList.contains('modal-scrim')) setTimeout(() => { const d = n.querySelector('.modal'); if (d && n.isConnected && !n.contains(document.activeElement)) (focusables(d).find((x) => !x.matches('[data-close]')) || focusables(d)[0] || d).focus({ preventScroll: true }); }, 60);
        if (n.classList.contains('toast-root')) watchToasts(n);
      }
      for (const n of m.removedNodes) {
        if (n.nodeType !== 1 || !n.classList.contains('modal-scrim')) continue;
        requestAnimationFrame(() => {
          if (document.activeElement && document.activeElement !== document.body) return;
          const d = topDialog();
          const last = lastOutside && lastOutside.deref();
          const to = d ? focusables(d)[0] : last && last.isConnected ? last : null;
          if (to) to.focus({ preventScroll: true });
        });
      }
    }
  }).observe(document.body || document.documentElement, { childList: true });
  new MutationObserver(clearAnnouncements).observe(document.documentElement, { attributes: true, attributeFilter: ['lang'] });
}

// toasts are said as they appear: errors at once, the rest politely
function watchToasts(root) {
  const say = (n) => { if (n.nodeType === 1 && n.classList.contains('toast')) announce(n.firstChild ? n.firstChild.textContent : n.textContent, n.classList.contains('error')); };
  // (the first toast goes in with its root, before this saw the root)
  [...root.children].forEach(say);
  new MutationObserver((muts) => { for (const m of muts) m.addedNodes.forEach(say); }).observe(root, { childList: true });
}
