// Long conversations draw only their newest turns; older ones come a page at
// a time as you scroll up (chat.js), so opening one costs what's on screen.

export const PAGE_TURNS = 40;
const PAGE_CHARS = 250_000;

/** About how much a turn puts on screen: its rendered text. */
function weight(turn) {
  let n = 200;
  for (const p of turn.parts || []) n += (p.html || p.text || '').length;
  return n;
}

/** Where the page of `turns` that ends before index `end` starts: up to
 *  PAGE_TURNS turns or PAGE_CHARS of content (at least one turn), and never
 *  on an answer without its question when the question is there. */
export function pageStart(turns, end) {
  let i = end;
  let chars = 0;
  while (i > 0 && end - i < PAGE_TURNS && (i === end || chars + weight(turns[i - 1]) <= PAGE_CHARS)) {
    i--;
    chars += weight(turns[i]);
  }
  if (i > 0 && turns[i].role === 'assistant' && turns[i - 1].role === 'user') i--;
  return i;
}

// Turns drawn by scrolling up stay in the DOM; the ones far off screen are
// parked (emptied, keeping their height) and drawn again from their data
// when they come near, so what's mounted stays about a few screens' worth.
const PARK_FAR = 4;
const PARK_NEAR = 2;

/** Whether a drawn turn `dist` px off screen (0 when on screen) is parked,
 *  in a viewport `view` px tall, given whether it is now (`was`): beyond
 *  PARK_FAR screens it is, within PARK_NEAR it's drawn, and in between it
 *  stays as it is, so scrolling to and fro doesn't redraw turns at the edge. */
export function parked(dist, view, was) {
  if (dist > PARK_FAR * view) return true;
  if (dist < PARK_NEAR * view) return false;
  return was;
}
