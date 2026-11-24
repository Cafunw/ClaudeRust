// The preview of an attachment, staged in the composer or sent in a message:
// a PDF in the WebView's own viewer, an image at full size, text and code
// highlighted (an Office file, EPUB or RTF as the text taken out of it),
// with Open (in the system's app) and Download.
import { call, artifactUrl } from './api.js';
import { icon } from './icons.js';
import { t } from './i18n.js';
import { esc, h, modal, errorToast } from './util.js';
import { codedMessage } from './errors.js';

/** Files whose preview is the text taken out of them, not their source. */
const DOCUMENTS = ['docx', 'xlsx', 'pptx', 'odt', 'epub', 'rtf', 'doc'];

/** The highlighter's language for a file `name`: its extension, none for a document or plain text. */
export function langOf(name) {
  const ext = ((String(name).match(/\.([A-Za-z0-9+_-]{1,12})$/) || [])[1] || '').toLowerCase();
  return ext && ext !== 'txt' && !DOCUMENTS.includes(ext) ? ext : '';
}

/** Whether `name` is a document whose text was taken out of it. */
export const isDocument = (name) => DOCUMENTS.includes(((String(name).match(/\.([A-Za-z0-9]+)$/) || [])[1] || '').toLowerCase());

/** Where a sent attachment's data is: the `.att` element `el` of a user turn
 *  (its place among the turn's attachments is its place in the message). */
export function messageSource(el) {
  const turn = el.closest('.turn[data-id]');
  const list = el.parentElement;
  if (!turn || !list || String(turn.dataset.id).startsWith('pending-')) return null;
  return { messageId: turn.dataset.id, index: [...list.children].indexOf(el) };
}

if (typeof document !== 'undefined' && document.head && !document.querySelector('link[data-att-preview]')) {
  document.head.appendChild(h('<link rel="stylesheet" href="css/attachment_preview.css" data-att-preview>'));
}

/** Open the preview of the attachment at `source` ({ stagedId } or { messageId, index }) named `name`. */
export function openPreview(source, name) {
  const body = h('<div class="att-pv-body loading" tabindex="0" role="document"></div>');
  let closed = false;
  const m = modal({ title: name, body, wide: true, cls: 'tall att-preview', onClose: () => { closed = true; } });
  const act = (ic, label, fn) => {
    const b = h(`<button class="icon-btn" title="${esc(label)}" aria-label="${esc(label)}">${icon(ic, 16)}</button>`);
    b.addEventListener('click', () => fn().catch(errorToast));
    return b;
  };
  m.el.querySelector('.modal-head [data-close]').before(
    act('external', t('att_open_app'), () => call('attachment_open', source)),
    act('download', t('download'), () => call('attachment_save', source)),
  );
  body.focus();
  call('attachment_data', source).then(async (d) => {
    let html;
    if (d.kind === 'pdf') html = `<iframe class="att-pv-pdf" src="${esc(artifactUrl(d.token))}" title="${esc(name)}"></iframe>`;
    else if (d.kind === 'image') html = `<img class="att-pv-img" src="data:${esc(d.mime)};base64,${esc(d.data)}" alt="${esc(name)}">`;
    else {
      const lang = langOf(name);
      const code = lang ? await call('highlight_code', { code: d.data, language: lang }) : esc(d.data);
      const note = d.digest ? t('att_pv_digest') : isDocument(name) ? t('att_pv_extracted') : '';
      html = `${note ? `<p class="att-pv-note">${esc(note)}</p>` : ''}<pre class="att-pv-text"><code class="hl-code">${code}</code></pre>`;
    }
    if (closed) return;
    body.classList.remove('loading');
    body.dataset.kind = d.kind;
    // a document's text reads as prose, not code
    if (d.kind === 'text' && isDocument(name)) body.dataset.doc = '';
    body.innerHTML = html;
    const img = body.querySelector('.att-pv-img');
    // fitted to the box; a click shows it at its own size
    if (img) img.addEventListener('click', () => img.classList.toggle('actual'));
  }).catch((e) => {
    if (closed) return;
    body.classList.remove('loading');
    body.innerHTML = `<div class="att-pv-error" role="alert"><p>${esc(t('att_pv_failed'))}</p><p class="d">${esc(codedMessage(e) || e.message || String(e))}</p></div>`;
  });
  return m;
}

/** Make the attachment chip `chip` open its preview: a click on it (not on
 *  its own buttons), or Enter / Space on its name, which becomes a button. */
export function hookPreview(chip, source, name) {
  const info = chip.querySelector('.info') || chip;
  info.setAttribute('role', 'button');
  info.tabIndex = 0;
  info.setAttribute('aria-label', t('att_preview_x', { x: name }));
  chip.classList.add('previewable');
  chip.addEventListener('click', (e) => { if (!e.target.closest('button')) openPreview(source, name); });
  info.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openPreview(source, name); }
  });
}

// a sent message's attachments (chat.js draws them): a click or, where they can take focus, Enter or Space
const sentAtt = (e) => e.target.closest && e.target.closest('.user-atts > .att');
const openSent = (el) => {
  const source = messageSource(el);
  if (source) openPreview(source, (el.querySelector('.n') || el).textContent.trim());
};
if (typeof document !== 'undefined' && document.body) {
  document.addEventListener('click', (e) => { const el = sentAtt(e); if (el) openSent(el); });
  document.addEventListener('keydown', (e) => {
    const el = sentAtt(e);
    if (el && el === e.target && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); openSent(el); }
  });
}
