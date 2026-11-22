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
