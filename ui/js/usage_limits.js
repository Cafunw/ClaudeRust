// Plan usage limits of the Claude account (5-hour and weekly): a meter in
// the sidebar footer with a card of every limit, and a banner over the
// composer when one is close or reached. Sending stays possible either way:
// the server decides.
import { t, currentLanguage } from './i18n.js';
import { call, on } from './api.js';
import { esc, h, showMenu } from './util.js';
import { icon } from './icons.js';
import { loadLocal, saveLocal } from './store.js';
import { notRegistered } from './optional_cmd.js';

const LABELS = { five_hour: 'ul_5h', seven_day: 'ul_week', weekly: 'ul_week', seven_day_opus: 'ul_week_opus', seven_day_sonnet: 'ul_week_sonnet', overage: 'ul_extra' };
// the limit as a sentence names it ("your weekly limit for all models")
const PHRASES = { five_hour: 'ul_ph_5h', seven_day: 'ul_ph_week', weekly: 'ul_ph_week', seven_day_opus: 'ul_ph_week_opus', seven_day_sonnet: 'ul_ph_week_sonnet', overage: 'ul_ph_extra' };

/** A percentage from a fraction (`utilization`, which can pass 1) or a percent (`used_pct`). */
function pct(fraction, percent) {
  const v = fraction ?? percent;
  if (v === null || v === undefined || v === '') return null;
  const n = Number(v);
  if (!Number.isFinite(n)) return null;
  return Math.max(0, Math.min(100, fraction !== null && fraction !== undefined ? n * 100 : n));
}

/** A time as ms since the epoch, from seconds, ms or an ISO string. */
function when(v) {
  if (v === null || v === undefined || v === '') return null;
  const n = Number(v);
  if (Number.isFinite(n)) return n < 1e12 ? n * 1000 : n;
  const d = Date.parse(v);
  return Number.isNaN(d) ? null : d;
}

/** The limits in a usage_limits answer or usage-limits-changed event, whatever
 *  its shape (a list, `{limits}`, an object keyed by limit, or the CLI's own
 *  rate_limit_info), and how close they are: 'ok', 'near' or 'exceeded'. */
export function normalizeLimits(payload) {
  const p = payload && typeof payload === 'object' ? payload : {};
  const list = Array.isArray(p) ? p
    : Array.isArray(p.limits) ? p.limits
    : Array.isArray(p.entries) ? p.entries
    : p.rate_limit_info ? [p.rate_limit_info]
    : p.rateLimitType || p.rate_limit_type ? [p]
    : Object.entries(p).filter(([, v]) => v && typeof v === 'object' && !Array.isArray(v)).map(([k, v]) => ({ key: k, ...v }));
  const items = list.filter((e) => e && typeof e === 'object').map((e) => ({
    key: String(e.key || e.kind || e.type || e.rate_limit_type || e.rateLimitType || e.id || ''),
    label: typeof e.label === 'string' ? e.label : null,
    pct: pct(e.utilization ?? e.utilisation, e.used_pct ?? e.percent),
    status: String(e.status || '').toLowerCase(),
    resetsAt: when(e.resets_at ?? e.resetsAt ?? e.reset_at),
  })).filter((x) => x.key || x.pct !== null);
  const exceeded = typeof p.exceeded === 'boolean' ? p.exceeded : items.some((x) => /rejected|exceeded|blocked/.test(x.status) || x.pct >= 100);
  const near = !exceeded && (typeof p.near === 'boolean' ? p.near : items.some((x) => /warning|near/.test(x.status) || x.pct >= 80));
  return { items, level: exceeded ? 'exceeded' : near ? 'near' : 'ok' };
}

export function limitLabel(x) {
  return x.label || (LABELS[x.key] ? t(LABELS[x.key]) : x.key.replace(/_/g, ' '));
}

/** The banner's sentence for the limit shown first. */
export function bannerText(level, x) {
  const s = t(level === 'exceeded' ? 'ul_over' : 'ul_near', { limit: limitPhrase(x) });
  // a Turkish sentence starts with the limit's name ("haftalık …")
  return s.charAt(0).toLocaleUpperCase(currentLanguage()) + s.slice(1);
}

/** "your weekly limit for all models": the limit as a sentence names it. */
export function limitPhrase(x) {
  return PHRASES[x.key] ? t(PHRASES[x.key]) : t('ul_ph_other', { limit: limitLabel(x) });
}

/** What a dismissed banner stays hidden for: a new level, limit or window shows it again. */
export function bannerKey(level, x) {
  // payloads may restate the same reset a few seconds apart
  return `${level}|${x.key}|${x.resetsAt ? Math.round(x.resetsAt / 6e5) : ''}`;
}

/** "2 h 5 min", "35 min", "3 d 4 h" until `ms`. */
export function untilText(ms, now = Date.now()) {
  const min = Math.max(1, Math.round((ms - now) / 60000));
  if (min < 60) return t('ul_min', { m: min });
  const hr = Math.floor(min / 60);
  if (hr < 24) return min % 60 ? t('ul_h_min', { h: hr, m: min % 60 }) : t('ul_h', { h: hr });
  return hr % 24 ? t('ul_d_h', { d: Math.floor(hr / 24), h: hr % 24 }) : t('ul_d', { d: Math.floor(hr / 24) });
}

/** "Resets in 2 h 5 min (Mon 14:30)", in local time; '' when unknown. */
export function resetText(ms, now = Date.now(), locale = undefined) {
  if (!ms) return '';
  if (ms <= now) return t('ul_reset_now');
  const d = new Date(ms);
  const sameDay = d.toDateString() === new Date(now).toDateString();
  const at = sameDay ? d.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' }) : d.toLocaleString(locale, { weekday: 'short', hour: '2-digit', minute: '2-digit' });
  return t('ul_resets', { in: untilText(ms, now), at });
}

/** The limit to show first: a reached one, else the most used. */
export function headline(state) {
  const items = state.items.filter((x) => x.pct !== null || x.status);
  const rank = (x) => (/rejected|exceeded|blocked/.test(x.status) ? 1000 : 0) + (x.pct ?? (/warning/.test(x.status) ? 80 : 0));
  return items.sort((a, b) => rank(b) - rank(a))[0] || null;
}

let state = { items: [], level: 'ok' };
let supported = true;

const loc = () => document.documentElement.lang || undefined;
// "86%", "%86" in Turkish
const pctText = (v) => new Intl.NumberFormat(loc(), { style: 'percent' }).format(v / 100);

function bar(x) {
  const v = Math.round(x.pct ?? 0);
  return `<div class="ul-bar ${x.pct >= 100 || /rejected|exceeded/.test(x.status) ? 'over' : x.pct >= 80 ? 'near' : ''}" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${v}" aria-label="${esc(limitLabel(x))}"><span style="width:${v}%"></span></div>`;
}

function card() {
  const rows = state.items.map((x) => `<div class="ul-row">
    <div class="ul-top"><span>${esc(limitLabel(x))}</span><span class="muted">${x.pct === null ? '' : pctText(Math.round(x.pct))}</span></div>
    ${x.pct === null ? '' : bar(x)}
    <div class="small muted">${esc(resetText(x.resetsAt, Date.now(), loc()))}</div></div>`).join('');
  return h(`<div class="ul-card"><div class="menu-label">${esc(t('ul_title'))}</div>${rows || `<div class="small muted">${esc(t('ul_none'))}</div>`}<p class="small muted ul-explain">${esc(t('ul_explain'))}</p></div>`);
}

function drawMeter(btn) {
  const top = headline(state);
  btn.classList.toggle('hidden', !top);
  if (!top) return;
  btn.className = `ul-meter ${state.level}`;
  const v = top.pct === null ? null : Math.round(top.pct);
  btn.innerHTML = `<span class="ul-name">${esc(limitLabel(top))}</span>${v === null ? '' : `<span class="ul-mini"><span style="width:${v}%"></span></span><span class="ul-pct">${pctText(v)}</span>`}`;
  const tip = t('ul_meter_tip', { limit: limitPhrase(top), pct: v === null ? '—' : pctText(v) });
  btn.title = tip;
  btn.setAttribute('aria-label', tip);
}

function drawBanner(el) {
  const top = headline(state);
  const key = top && bannerKey(state.level, top);
  const off = state.level === 'ok' || !top || loadLocal().limitBannerDismissed === key;
  el.classList.toggle('hidden', off);
  if (off) { el.innerHTML = ''; return; }
  const reset = resetText(top.resetsAt, Date.now(), loc());
  el.className = `limit-banner ${state.level}`;
  el.innerHTML = `<span class="grow">${esc(bannerText(state.level, top))}${reset ? ` ${esc(reset)}.` : ''}</span><button class="icon-btn sm" type="button" title="${esc(t('ul_dismiss'))}" aria-label="${esc(t('ul_dismiss'))}">${icon('x', 14)}</button>`;
  el.querySelector('button').onclick = () => { saveLocal({ limitBannerDismissed: key }); redraw(); };
}

function redraw() {
  document.querySelectorAll('.ul-meter').forEach(drawMeter);
  document.querySelectorAll('.limit-banner').forEach(drawBanner);
}

function set(payload) {
  state = normalizeLimits(payload);
  redraw();
}

/** The sidebar footer's meter (hidden until there are limits to show). */
export function usageMeter() {
  const btn = h('<button class="ul-meter hidden" type="button"></button>');
  btn.onclick = () => showMenu(btn, [{ element: card() }], { placement: 'top-start', width: 280 });
  drawMeter(btn);
  return btn;
}

/** The banner over a composer (shown when a limit is close or reached). */
export function limitBanner() {
  const el = h('<div class="limit-banner hidden" role="status"></div>');
  drawBanner(el);
  return el;
}

export async function refreshLimits() {
  if (!supported) return;
  try { set(await call('usage_limits')); } catch (e) { if (notRegistered(e)) supported = false; }
}

on('usage-limits-changed', set);
