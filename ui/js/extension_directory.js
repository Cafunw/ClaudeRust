// "Browse extensions" (Settings → Developer): the curated public .mcpb
// releases shipped with the app (extension_directory command). Install
// downloads the bundle only then, checks its SHA-256 against the listed one
// and opens the usual preview (signature, what it runs, settings) before
// anything is installed; an installed one with a newer listed version offers
// the update.
import { t, currentLanguage } from './i18n.js';
import { icon } from './icons.js';
import { call, on as onBackend } from './api.js';
import { esc, h, modal, toast, errorToast, humanSize } from './util.js';
import { notRegistered } from './optional_cmd.js';
import { previewDialog } from './extensions_page.js';
import { errorText } from './neterr.js';

/** Entry `e` as the dialog shows it: description and category in language `lang`. */
export function localizeEntry(e, lang) {
  const d = e.description;
  const description = typeof d === 'string' ? d : (d && (d[lang] || d.en)) || '';
  return { ...e, description, categoryLabel: t('ed_cat_' + e.category) };
}

/** Entries whose name, publisher, description or category hold every word of `q`, in `category` when one is picked. */
export function filterExtensions(list, q, category = '') {
  const words = String(q || '').toLowerCase().split(/\s+/).filter(Boolean);
  return list.filter((e) => {
    const hay = [e.name, e.publisher, e.description, e.categoryLabel].join(' ').toLowerCase();
    return (!category || e.category === category) && words.every((w) => hay.includes(w));
  });
}

/** What entry `e`'s button does: 'none' (no build for this system), 'update', 'installed' or 'install'. */
export function entryAction(e) {
  if (!e.asset) return 'none';
  if (e.installed) return e.update ? 'update' : 'installed';
  return 'install';
}

/** The directory dialog; `redraw(status)` redraws Settings → Developer after an install. */
export async function openExtensionDirectory(redraw) {
  let list = null;
  const load = async () => { list = (await call('extension_directory')).map((e) => localizeEntry(e, currentLanguage())); };
  try { await load(); } catch (e) { if (!notRegistered(e)) { errorToast(e); return; } }
  const body = h(`<div class="cd">
    <p class="small muted" style="margin:0 0 12px">${esc(t('ed_trust'))}</p>
    <div class="search-box">${icon('search', 15)}<input class="input" type="search" placeholder="${esc(t('ed_search'))}" aria-label="${esc(t('ed_search'))}"></div>
    <div class="cd-cats" role="group" aria-label="${esc(t('cd_categories'))}"></div>
    <div class="cd-list" role="list" aria-live="polite"></div></div>`);
  const input = body.querySelector('input');
  const out = body.querySelector('.cd-list');
  const cats = body.querySelector('.cd-cats');
  let category = '';
  // the entry being downloaded: {id, downloaded, total} or null; one at a time (the backend allows one)
  let busy = null;
  const errors = {};
  for (const c of list ? ['', ...new Set(list.map((e) => e.category))] : []) {
    const b = h(`<button class="chip" aria-pressed="${!c}">${esc(c ? t('ed_cat_' + c) : t('cd_all'))}</button>`);
    b.onclick = () => {
      category = c;
      cats.querySelectorAll('.chip').forEach((x) => x.setAttribute('aria-pressed', String(x === b)));
      draw();
    };
    cats.appendChild(b);
  }
  // the bar moves in place, so a focused Cancel stays focused
  const moved = () => {
    const el = out.querySelector('.ed-busy');
    if (!el || !busy.total) { draw(); return; }
    const pct = Math.min(100, Math.round((busy.downloaded / busy.total) * 100));
    el.querySelector('.pct').textContent = `${pct}%`;
    el.querySelector('.upd-bar').setAttribute('aria-valuenow', String(pct));
    el.querySelector('.upd-bar span').style.width = `${pct}%`;
  };
  const off = await onBackend('extension-download-progress', (p) => { if (busy && p && p.id === busy.id) { busy = { ...busy, ...p }; moved(); } });
  const m = modal({ title: t('ed_title'), body, wide: true, cls: 'tall', onClose: () => { off(); if (busy) call('extension_directory_cancel').catch(() => {}); } });

  const install = async (e) => {
    busy = { id: e.id, downloaded: 0, total: e.asset.size };
    delete errors[e.id];
    draw();
    let info;
    try { info = await call('extension_directory_download', { id: e.id }); } catch (err) {
      busy = null;
      if (err && err.kind !== 'cancelled') errors[e.id] = errorText(err);
      draw();
      return;
    }
    busy = null;
    draw();
    previewDialog(info, async (values) => {
      const r = await call('extension_directory_install', { id: e.id, path: info.path, values });
      toast(t('extension_installed', { name: r.installed }));
      redraw && redraw(r);
      await load().catch(errorToast);
      draw();
    });
  };

  const draw = () => {
    out.replaceChildren();
    if (!list) { out.appendChild(h(`<div class="empty-state" style="padding:30px">${esc(t('ed_unavailable'))}</div>`)); return; }
    const shown = filterExtensions(list, input.value, category);
    if (!shown.length) out.appendChild(h(`<div class="empty-state" style="padding:30px">${esc(t('ed_none'))}</div>`));
    for (const e of shown) {
      const act = entryAction(e);
      const mine = busy && busy.id === e.id;
      const pct = mine && busy.total ? Math.min(100, Math.round((busy.downloaded / busy.total) * 100)) : null;
      const right = mine
        ? `<div class="ed-busy small" style="min-width:140px"><div>${esc(t('ed_downloading'))} <span class="pct">${pct === null ? '' : `${pct}%`}</span></div>
            <div class="upd-bar ${pct === null ? 'indeterminate' : ''}" role="progressbar" aria-label="${esc(t('ed_downloading'))}" aria-valuemin="0" aria-valuemax="100" ${pct === null ? '' : `aria-valuenow="${pct}"`}><span style="width:${pct === null ? 30 : pct}%"></span></div>
            <button class="btn sm ghost" data-a="cancel" style="margin-top:4px">${esc(t('cancel'))}</button></div>`
        : act === 'installed' ? `<span class="st connected">${esc(t('ed_installed'))}</span>`
          : act === 'none' ? `<span class="small muted">${esc(t('ed_no_build_t'))}</span>`
            : `<button class="btn sm primary" data-a="install" ${busy ? 'disabled' : ''} aria-label="${esc(act === 'update' ? t('ed_update', { v: e.version }) : t('install'))}: ${esc(e.name)}">${esc(act === 'update' ? t('ed_update', { v: e.version }) : t('install'))}</button>`;
      const card = h(`<div class="server" role="listitem"><div class="head">${icon('download', 16)}<div class="grow" style="min-width:0">
        <div style="font-weight:600">${esc(e.name)} <span class="cd-cat">${esc(e.categoryLabel)}</span>${act === 'update' ? ` <span class="cd-cat">${esc(t('ed_update_available'))}</span>` : ''}</div>
        <div class="small">${esc(e.description)}</div>
        <div class="small muted">${esc(e.publisher)} · ${esc(e.license)} · v${esc(e.version)}${e.asset ? ` · ${esc(humanSize(e.asset.size))}` : ''}${e.runtime && e.runtime !== 'binary' ? ` · ${esc(t('ed_needs_' + e.runtime))}` : ''}${e.installed ? ` · ${esc(t('ed_installed_v', { v: e.installed.version || '?' }))}` : ''}</div>
        ${errors[e.id] ? `<div class="small" role="alert" style="color:var(--danger);margin-top:4px">${esc(errors[e.id])}</div>` : ''}</div>
        <button class="btn sm ghost" data-a="source">${esc(t('ed_source'))}</button>${right}</div></div>`);
      card.querySelector('[data-a=source]').onclick = () => call('open_external', { url: e.repository }).catch(errorToast);
      const b = card.querySelector('[data-a=install]');
      if (b) b.onclick = () => install(e);
      const c = card.querySelector('[data-a=cancel]');
      if (c) c.onclick = () => call('extension_directory_cancel').catch(errorToast);
      out.appendChild(card);
    }
  };
  input.addEventListener('input', draw);
  draw();
  setTimeout(() => input.focus(), 30);
  return m;
}
