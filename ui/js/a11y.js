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

/** Say `text` to a screen reader (`assertive`: interrupting, for errors). */
export function announce(text, assertive = false) {
  const el = region(assertive);
  if (!el || !text) return;
  // emptied first, so the same words said twice are announced twice
  el.textContent = '';
  setTimeout(() => { el.textContent = text; }, 30);
}

const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]):not([type=hidden]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), iframe';

function focusables(root) {
  return [...root.querySelectorAll(FOCUSABLE)].filter((el) => el.getClientRects().length > 0 || el === document.activeElement);
}

const topDialog = () => { const s = document.querySelectorAll('.modal-scrim'); return s.length ? s[s.length - 1].querySelector('.modal') : null; };

// (not in the Node tests, which have no DOM)
if (typeof MutationObserver !== 'undefined') {
  // Tab and Shift+Tab stay inside the top dialog
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Tab' || e.defaultPrevented || document.querySelector('.popover')) return;
    const dlg = topDialog();
    if (!dlg) return;
    const list = focusables(dlg);
    const to = trapStep(list.length, list.indexOf(document.activeElement), e.shiftKey);
    if (!list.length) { e.preventDefault(); return; }
    if (to >= 0) { e.preventDefault(); list[to].focus(); }
  }, true);

  // the last place focus was outside menus and dialogs: where it goes back
  // to when a dialog closes and the element that opened it is gone (an item
  // of a menu, say)
  let lastOutside = null;
  document.addEventListener('focusin', (e) => {
    if (e.target instanceof Element && !e.target.closest('.popover, .modal-scrim')) lastOutside = e.target;
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
          const to = d ? focusables(d)[0] : lastOutside && lastOutside.isConnected ? lastOutside : null;
          if (to) to.focus({ preventScroll: true });
        });
      }
    }
  }).observe(document.body || document.documentElement, { childList: true });
}

// toasts are said as they appear: errors at once, the rest politely
function watchToasts(root) {
  const say = (n) => { if (n.nodeType === 1 && n.classList.contains('toast')) announce(n.firstChild ? n.firstChild.textContent : n.textContent, n.classList.contains('error')); };
  // (the first toast goes in with its root, before this saw the root)
  [...root.children].forEach(say);
  new MutationObserver((muts) => { for (const m of muts) m.addedNodes.forEach(say); }).observe(root, { childList: true });
}
