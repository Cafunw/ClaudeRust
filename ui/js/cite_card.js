// A web citation's card (the chips render.rs draws: a.cite with data-title and
// data-quote, the page's title and the passage the answer cites): shown on
// hover and on keyboard focus, and on a tap or a click when it isn't up yet,
// so it can be read without leaving the app; a click on the chip while its
// card is up opens the page. Escape, leaving it or scrolling closes it.
import { esc } from './util.js';
import { t } from './i18n.js';

const GAP = 6; // between the chip and its card
const EDGE = 8; // the least room the card keeps from the window's edges
let card = null;
let chip = null;
let closing = 0;

/** Where a `w` x `h` card goes for a chip at `r` in a `vw` x `vh` window:
 *  under the chip, above it when there's no room below, always inside the window. */
export function cardPlace(r, w, h, vw, vh) {
  const left = Math.max(EDGE, Math.min(r.left, vw - w - EDGE));
  let top = r.bottom + GAP;
  if (top + h > vh - EDGE) top = r.top - GAP - h >= EDGE ? r.top - GAP - h : Math.max(EDGE, vh - h - EDGE);
  return { left, top };
}

/** The card's inner HTML for chip `a`: its title, its domain, the passage cited. */
export function cardHtml(a) {
  const title = a.dataset.title || a.textContent;
  const quote = a.dataset.quote;
  return `<div class="cite-title">${esc(title)}</div><div class="cite-domain">${esc(a.textContent)}</div>${quote ? `<blockquote class="cite-quote" aria-label="${esc(t('cite_passage'))}">${esc(quote)}</blockquote>` : ''}`;
}

function show(a) {
  clearTimeout(closing);
  if (chip === a) return;
  hide();
  chip = a;
  card = document.createElement('div');
  card.className = 'cite-card';
  card.id = 'cite-card';
  card.setAttribute('role', 'tooltip');
  card.innerHTML = cardHtml(a);
  card.addEventListener('pointerenter', () => clearTimeout(closing));
  card.addEventListener('pointerleave', hideSoon);
  document.body.appendChild(card);
  place();
  a.setAttribute('aria-describedby', card.id);
}

/** Put the card by its chip; false when the chip is gone or out of sight. */
function place() {
  const r = chip.getBoundingClientRect();
  if (!chip.isConnected || r.bottom < 0 || r.top > innerHeight) return false;
  const { left, top } = cardPlace(r, card.offsetWidth, card.offsetHeight, innerWidth, innerHeight);
  card.style.left = `${left}px`;
  card.style.top = `${top}px`;
  return true;
}

function hide() {
  clearTimeout(closing);
  if (chip) chip.removeAttribute('aria-describedby');
  if (card) card.remove();
  card = chip = null;
}

// time to move the pointer from the chip onto its card
function hideSoon() {
  clearTimeout(closing);
  closing = setTimeout(hide, 200);
}

const citeOf = (e) => (e.target instanceof Element ? e.target.closest('a.cite') : null);

/** For the app's link handling (parts.js): whether a click on `a` only showed its card. */
export function citeClick(a) {
  if (chip === a) return false;
  show(a);
  return true;
}

document.addEventListener('pointerover', (e) => { const a = citeOf(e); if (a && e.pointerType !== 'touch') show(a); });
document.addEventListener('pointerout', (e) => { const a = citeOf(e); if (a && a === chip && !a.contains(e.relatedTarget)) hideSoon(); });
document.addEventListener('focusin', (e) => { const a = citeOf(e); if (a) show(a); else if (chip) hide(); });
document.addEventListener('focusout', (e) => { if (citeOf(e) === chip && chip) hideSoon(); });
// before the app's own Escape (it would stop a response)
window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && card) { e.preventDefault(); e.stopPropagation(); hide(); }
}, true);
// (a streamed answer scrolls the chat as it grows)
document.addEventListener('scroll', () => { if (card && !place()) hide(); }, true);
document.addEventListener('pointerdown', (e) => { if (card && !card.contains(e.target) && !citeOf(e)) hide(); });
