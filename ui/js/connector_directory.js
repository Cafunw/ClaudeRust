// "Browse connectors" (Settings → Developer): the curated remote MCP servers
// shipped with the app (connector_directory command). "Add" opens the custom
// connector form filled in; the server's domain shows before any sign-in.
import { t, currentLanguage } from './i18n.js';
import { icon } from './icons.js';
import { call } from './api.js';
import { esc, h, modal, errorToast } from './util.js';
import { connectorDialog } from './mcp_connector.js';
import { notRegistered } from './optional_cmd.js';

/** Entry `c` of the directory as the dialog shows it: description and category in language `lang`. */
export function localize(c, lang) {
  const d = c.description;
  const description = typeof d === 'string' ? d : (d && (d[lang] || d.en)) || '';
  return { ...c, description, categoryLabel: t('cd_cat_' + c.category) };
}

/** Entries whose name, description, category or domain hold every word of `q`,
 *  in `category` when one is picked. */
export function filterDirectory(list, q, category = '') {
  const words = String(q || '').toLowerCase().split(/\s+/).filter(Boolean);
  return list.filter((c) => {
    const hay = [c.name, c.description, c.categoryLabel, c.domain].join(' ').toLowerCase();
    return (!category || c.category === category) && words.every((w) => hay.includes(w));
  });
}

/** The letters of entry `name`'s tile (no third-party logos are shipped): the initials of its first
 *  two words, of a single word's capitals ("GitHub" → "GH"), else its first two letters ("Linear" → "Li"). */
export function monogram(name) {
  const words = String(name || '').match(/[\p{L}\p{N}]+/gu) || [];
  if (words.length > 1) return (words[0][0] + words[1][0]).toUpperCase();
  const w = [...(words[0] || '?')];
  const cap = w.slice(1).find((c) => /\p{Lu}/u.test(c));
  return cap ? (w[0] + cap).toUpperCase() : w[0].toUpperCase() + w.slice(1, 2).join('').toLowerCase();
}

/** Which of the TILE_COLORS colours the tile of `name` gets: the same for a name every time. */
export const TILE_COLORS = 8;
export function tileColor(name) {
  let n = 0;
  for (const ch of String(name || '')) n = (n * 31 + ch.codePointAt(0)) >>> 0;
  return n % TILE_COLORS;
}

/** A name for entry `c` that no configured server has yet. */
export function freeName(c, status) {
  const taken = new Set(Object.keys(((status && status.config) || {}).mcpServers || {}));
  let n = c.name;
  for (let i = 2; taken.has(n); i++) n = `${c.name} ${i}`;
  return n;
}

/** The directory dialog; `status`/`redraw` as for connectorDialog. */
export async function openDirectory(status, redraw) {
  let list = null;
  try { list = (await call('connector_directory')).map((c) => localize(c, currentLanguage())); } catch (e) {
    if (!notRegistered(e)) { errorToast(e); return; }
  }
  const body = h(`<div class="cd">
    <p class="small muted" style="margin:0 0 12px">${esc(t('cd_trust'))}</p>
    <div class="search-box">${icon('search', 15)}<input class="input" type="search" placeholder="${esc(t('cd_search'))}" aria-label="${esc(t('cd_search'))}"></div>
    <div class="cd-cats" role="group" aria-label="${esc(t('cd_categories'))}"></div>
    <div class="cd-list" role="list"></div></div>`);
  const input = body.querySelector('input');
  const out = body.querySelector('.cd-list');
  const cats = body.querySelector('.cd-cats');
  let category = '';
  for (const c of list ? ['', ...new Set(list.map((e) => e.category))] : []) {
    const b = h(`<button class="chip" aria-pressed="${!c}">${esc(c ? t('cd_cat_' + c) : t('cd_all'))}</button>`);
    b.onclick = () => {
      category = c;
      cats.querySelectorAll('.chip').forEach((x) => x.setAttribute('aria-pressed', String(x === b)));
      draw();
    };
    cats.appendChild(b);
  }
  const m = modal({ title: t('cd_title'), body, wide: true, cls: 'tall' });
  const draw = () => {
    out.replaceChildren();
    if (!list) { out.appendChild(h(`<div class="empty-state" style="padding:30px">${esc(t('cd_unavailable'))}</div>`)); return; }
    const shown = filterDirectory(list, input.value, category);
    if (!shown.length) out.appendChild(h(`<div class="empty-state" style="padding:30px">${esc(t('cd_none'))}</div>`));
    for (const c of shown) {
      const card = h(`<div class="server" role="listitem"><div class="head"><span class="cd-tile t${tileColor(c.name)}" aria-hidden="true">${esc(monogram(c.name))}</span><div class="grow" style="min-width:0">
        <div style="font-weight:600">${esc(c.name)} <span class="cd-cat">${esc(c.categoryLabel)}</span></div>
        <div class="small">${esc(c.description)}</div>
        <div class="small muted"><b class="mono">${esc(c.domain)}</b> · ${esc(t('cd_auth_' + (c.auth || 'none')))}</div></div>
        ${c.docs_url ? `<button class="btn sm ghost" data-a="docs" aria-label="${esc(t('cd_docs'))}: ${esc(c.name)}">${esc(t('cd_docs'))}</button>` : ''}
        ${c.configured ? `<span class="st connected" title="${esc(c.configured)}">${esc(t('cd_added'))}</span>` : `<button class="btn sm primary" data-a="add" aria-label="${esc(t('cc_add'))}: ${esc(c.name)}">${esc(t('cc_add'))}</button>`}</div></div>`);
      const docs = card.querySelector('[data-a=docs]');
      if (docs) docs.onclick = () => call('open_external', { url: c.docs_url }).catch(errorToast);
      const add = card.querySelector('[data-a=add]');
      if (add) add.onclick = () => { m.close(); connectorDialog(status, redraw, null, { name: freeName(c, status), url: c.url }); };
      out.appendChild(card);
    }
  };
  input.addEventListener('input', draw);
  draw();
  setTimeout(() => input.focus(), 30);
}
