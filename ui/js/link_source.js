// "Add from a link": a Google Drive file, or a Google Doc, Sheet or Slides
// deck shared by link, added to the composer or to a project's knowledge
// (link_source.rs).
import { t } from './i18n.js';
import { call } from './api.js';
import { $, esc, h, modal, button, toast } from './util.js';
import { sourceError, sourceNotice } from './github_source.js';

/** The dialog. What is staged for the composer goes to `onAdd(attachment)`;
 *  with `projectId` it goes to that project's knowledge, then `onDone()`. */
export function openLinkSource({ onAdd, projectId, onDone } = {}) {
  const body = h(`<div><div class="field"><label class="label" for="gdrive-url">${esc(t('gdrive_label'))}</label>
      <input class="input" id="gdrive-url" spellcheck="false" autocomplete="off" placeholder="${esc(t('gdrive_ph'))}">
      <div class="help">${esc(t('gdrive_help'))}</div></div>
    <div data-f="result" aria-live="polite"></div></div>`);
  const input = $('#gdrive-url', body);
  const result = $('[data-f=result]', body);
  const add = button(t('gdrive_add'), 'primary', () => go());
  const m = modal({ title: t('add_from_link'), body, footer: [button(t('cancel'), '', () => m.close()), add] });

  async function go() {
    const url = input.value.trim();
    if (!url) { input.focus(); return; }
    if (add.disabled) return;
    add.disabled = true;
    result.innerHTML = `<div class="row small muted" style="padding:6px 2px"><span class="spinner"></span>${esc(t('gdrive_fetching'))}</div>`;
    try {
      const r = await call('link_add', { url, projectId: projectId || null });
      if (r.errors.length) throw r.errors[0];
      if (onAdd) r.added.forEach(onAdd);
      toast(t('gdrive_added', { name: r.added[0].name }));
      m.close();
      if (onDone) onDone();
    } catch (e) {
      result.innerHTML = '';
      // (an Error's own name is "Error": only a refused file's {kind, message, name} carries the file's)
      result.appendChild(sourceNotice(sourceError(e, e instanceof Error ? null : e.name)));
      input.focus();
    } finally {
      add.disabled = false;
    }
  }

  input.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.isComposing) { e.preventDefault(); go(); } });
  setTimeout(() => input.focus(), 30);
}
