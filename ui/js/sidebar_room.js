// The sidebar folds by itself to make room (a narrow window, or an artifact
// panel in a window under 1500 px) and opens again once the room is back,
// unless you folded or opened it yourself in between.

/** Whether the window has no room for the open sidebar. */
export function tight(width, panelOpen) {
  return width <= 760 || (panelOpen && width < 1500);
}

/** What the sidebar does as the room goes from `was` to `now` (tight or
 *  not): 'fold' when room just ran out and it's open, 'open' when room came
 *  back and it was folded by itself (`auto`), else null. */
export function roomStep(was, now, collapsed, auto) {
  if (now && !was && !collapsed) return 'fold';
  if (!now && was && collapsed && auto) return 'open';
  return null;
}
