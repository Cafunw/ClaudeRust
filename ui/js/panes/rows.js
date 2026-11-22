// Keyboard for the panes' clickable rows (changed files, commits, the file
// tree): one Tab stop per list, the arrow keys move between rows, Enter or
// Space does what a click does.

/** The row to focus after `key` on row `i` of `n`, or -1 for a key that doesn't move. */
export function nextRow(i, n, key) {
  if (!n) return -1;
  if (key === 'ArrowDown') return Math.min(n - 1, i + 1);
  if (key === 'ArrowUp') return Math.max(0, i - 1);
  if (key === 'Home') return 0;
  if (key === 'End') return n - 1;
  return -1;
}

/** The tab to show after `key` on tab `i` of `n` (← and → go round), or -1. */
export function nextTab(i, n, key) {
  if (!n || i < 0) return -1;
  if (key === 'ArrowRight') return (i + 1) % n;
  if (key === 'ArrowLeft') return (i - 1 + n) % n;
  if (key === 'Home') return 0;
  if (key === 'End') return n - 1;
  return -1;
}

// pane element -> selector -> data-k of the row of that kind focused last in it
const lastKeys = new WeakMap();

// the current row and its own buttons are in the Tab order, the other rows' are not
function tabbable(row, on) {
  row.tabIndex = on ? 0 : -1;
  for (const b of row.querySelectorAll('button')) b.tabIndex = on ? 0 : -1;
}

/** Make the rows (`selector`, each with a `data-k`) of `list` keyboard rows;
 *  `activate(row)` is what Enter and Space do (a click), `role` the role
 *  rows without one get. Call it again after the list is drawn again: a
 *  redraw that replaced the focused row focuses its new copy, unless focus
 *  has gone elsewhere. */
export function keyRows(list, selector, { activate = (r) => r.click(), role = 'button' } = {}) {
  const pane = list.closest('.ws-pane') || list;
  if (!lastKeys.has(pane)) lastKeys.set(pane, new Map());
  const last = lastKeys.get(pane);
  const rows = () => [...list.querySelectorAll(selector)];
  const all = rows();
  const k = last.get(selector);
  const current = all.find((r) => r.dataset.k === k) || all[0];
  for (const r of all) {
    tabbable(r, r === current);
    if (role && !r.hasAttribute('role')) r.setAttribute('role', role);
  }
  list.__current = current;
  if (current && k != null && current.dataset.k === k && document.activeElement === document.body) current.focus({ preventScroll: true });
  if (list.__keyRows) return;
  list.__keyRows = true;
  list.addEventListener('focusin', (e) => {
    const row = e.target.closest(selector);
    if (!row || !list.contains(row)) return;
    last.set(selector, row.dataset.k);
    if (list.__current === row) return;
    if (list.__current) tabbable(list.__current, false);
    tabbable(row, true);
    list.__current = row;
  });
  list.addEventListener('focusout', (e) => {
    const row = e.target.closest(selector);
    // focus went somewhere else (the row wasn't redrawn away): nothing to give it back to
    if (row) queueMicrotask(() => { if (row.isConnected && !list.contains(document.activeElement)) last.delete(selector); });
  });
  list.addEventListener('keydown', (e) => {
    const row = e.target.closest(selector);
    // keys on a row's own buttons are theirs
    if (!row || e.target !== row || e.altKey || e.ctrlKey || e.metaKey) return;
    if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); activate(row); return; }
    const rs = rows();
    const j = nextRow(rs.indexOf(row), rs.length, e.key);
    if (j < 0) return;
    e.preventDefault();
    rs[j].focus();
  });
}
