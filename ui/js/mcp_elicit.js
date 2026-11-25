// MCP elicitation: a server asks for a few values (a flat form), or to open one of its pages in the
// browser (URL mode). Each request is a dialog with the server's message and fields (or the URL, its
// host apart); Send/Open, Decline or closing it answers the server (mcp_elicit_respond).
import { t } from './i18n.js';
import { call, on as onBackend } from './api.js';
import { modal, button, h, esc, errorToast } from './util.js';

/** The choices of field `p` as [value, label]: its enum (with the old enumNames), or its titled oneOf/anyOf; null when it has none. */
export function choicesOf(p) {
  if (Array.isArray(p?.enum)) return p.enum.map((v, i) => [v, p.enumNames?.[i] ?? v]);
  const titled = p?.oneOf || p?.anyOf;
  return Array.isArray(titled) ? titled.filter((c) => c && 'const' in c).map((c) => [c.const, c.title ?? c.const]) : null;
}

/**
 * The answer to form `schema` from `values` (field -> an input's text, a checkbox's state, or the values
 * ticked of a list of choices), in the types the schema asks for; empty fields are left out. `missing`:
 * required fields left empty, `bad`: number fields that don't hold one, lists of a wrong length.
 */
export function formContent(schema, values) {
  const required = new Set(schema?.required || []);
  const out = { content: {}, missing: [], bad: [] };
  for (const [name, p] of Object.entries(schema?.properties || {})) {
    const v = values[name];
    if (p.type === 'boolean') { out.content[name] = Boolean(v); continue; }
    if (p.type === 'array') {
      const picked = Array.isArray(v) ? v : [];
      if (!picked.length && !p.minItems) { if (required.has(name)) out.missing.push(name); continue; }
      if (picked.length < (p.minItems ?? 0) || picked.length > (p.maxItems ?? Infinity)) out.bad.push(name);
      else out.content[name] = picked;
      continue;
    }
    const s = String(v ?? '');
    if (!s.trim()) { if (required.has(name)) out.missing.push(name); continue; }
    if (p.type === 'number' || p.type === 'integer') {
      const n = Number(s);
      if (Number.isFinite(n) && (p.type === 'number' || Number.isInteger(n))) out.content[name] = n; else out.bad.push(name);
    } else out.content[name] = s;
  }
  return out;
}

const INPUT_TYPES = { email: 'email', uri: 'url', date: 'date', 'date-time': 'datetime-local' };

function fieldHtml(name, p, required) {
  const label = `${esc(p.title || name)}${required ? ' *' : ''}`;
  const help = p.description ? `<div class="help">${esc(p.description)}</div>` : '';
  const f = `data-f="${esc(name)}"`;
  if (p.type === 'boolean') {
    return `<div class="field"><label class="label"><input type="checkbox" ${f}${p.default ? ' checked' : ''}> ${label}</label>${help}</div>`;
  }
  if (p.type === 'array') {
    const picked = new Set(Array.isArray(p.default) ? p.default : []);
    const boxes = (choicesOf(p.items) || []).map(([v, l]) => `<label class="elicit-choice"><input type="checkbox" data-m="${esc(name)}" value="${esc(v)}"${picked.has(v) ? ' checked' : ''}> ${esc(l)}</label>`).join('');
    return `<fieldset class="field elicit-choices"><legend class="label">${label}</legend>${boxes}${help}</fieldset>`;
  }
  let input;
  const choices = choicesOf(p);
  if (choices) {
    const options = choices.map(([v, l]) => `<option value="${esc(v)}"${v === p.default ? ' selected' : ''}>${esc(l)}</option>`).join('');
    input = `<select class="select" ${f}>${p.default === undefined ? '<option value=""></option>' : ''}${options}</select>`;
  } else {
    const num = p.type === 'number' || p.type === 'integer';
    const type = num ? 'number' : INPUT_TYPES[p.format] || 'text';
    const range = num ? `${p.minimum !== undefined ? ` min="${esc(p.minimum)}"` : ''}${p.maximum !== undefined ? ` max="${esc(p.maximum)}"` : ''} step="${p.type === 'integer' ? 1 : 'any'}"` : '';
    input = `<input class="input" type="${type}" ${f}${range} value="${esc(p.default ?? '')}">`;
  }
  return `<div class="field"><label class="label">${label}</label>${input}${help}</div>`;
}

const open = new Map(); // id -> closes its dialog without answering

/** URL `url` as HTML with its `host` set apart, so a look-alike address shows for what it is. */
export function urlHtml(url, host) {
  const at = url.indexOf(host, url.indexOf('//') + 2);
  if (!host || at < 0) return esc(url);
  return `${esc(url.slice(0, at))}<b class="elicit-host">${esc(host)}</b>${esc(url.slice(at + host.length))}`;
}

/** A URL request: the page opens in the browser only from the Open button, which accepts. */
function showUrl(card) {
  const body = h(`<div>
    <p class="help" style="margin:0 0 10px">${esc(t('elicit_url_note'))}</p>
    <p style="margin:0 0 12px;white-space:pre-wrap">${esc(card.message || '')}</p>
    <div class="elicit-url mono">${urlHtml(card.url, card.host)}</div>
    <p class="small muted" style="margin:8px 0 0">${esc(t('elicit_url_site', { host: card.host }))}</p>
    ${card.punycode ? `<p class="notice warn small" style="margin:10px 0 0">${esc(t('elicit_url_punycode'))}</p>` : ''}
  </div>`);
  let answered = false;
  const answer = async (action) => {
    try {
      if (action === 'accept') await call('open_external', { url: card.url });
      await call('mcp_elicit_respond', { id: card.id, action });
    } catch (e) {
      errorToast(e);
      return;
    }
    answered = true;
    dialog.close();
  };
  const dialog = modal({
    title: t('elicit_title', { server: card.server }),
    body,
    footer: [button(t('elicit_decline'), 'ghost', () => answer('decline')), button(t('elicit_open'), 'primary', () => answer('accept'))],
    onClose: () => {
      open.delete(card.id);
      if (!answered) call('mcp_elicit_respond', { id: card.id, action: 'cancel' }).catch(() => {});
    },
  });
  open.set(card.id, () => { answered = true; dialog.close(); });
}

function show(card) {
  if (open.has(card.id)) return;
  if (card.mode === 'url') { showUrl(card); return; }
  const props = card.schema?.properties || {};
  const required = new Set(card.schema?.required || []);
  // the note first: a long message from the server can't push it out of sight
  const body = h(`<div>
    <p class="help" style="margin:0 0 10px">${esc(t('elicit_note'))}</p>
    <p style="margin:0 0 16px;white-space:pre-wrap">${esc(card.message || '')}</p>
    ${Object.entries(props).map(([name, p]) => fieldHtml(name, p, required.has(name))).join('')}
  </div>`);
  const values = () => {
    const v = Object.fromEntries([...body.querySelectorAll('[data-f]')].map((el) => [el.dataset.f, el.type === 'checkbox' ? el.checked : el.value]));
    for (const el of body.querySelectorAll('[data-m]')) (v[el.dataset.m] ||= []).push(...(el.checked ? [el.value] : []));
    return v;
  };
  let answered = false;
  const answer = async (action, content) => {
    try {
      await call('mcp_elicit_respond', { id: card.id, action, content });
    } catch (e) {
      errorToast(e);
      if (action === 'accept') return;
    }
    answered = true;
    dialog.close();
  };
  const send = button(t('elicit_send'), 'primary', () => answer('accept', formContent(card.schema, values()).content));
  const decline = button(t('elicit_decline'), 'ghost', () => answer('decline'));
  const update = () => {
    const { missing, bad } = formContent(card.schema, values());
    send.disabled = missing.length > 0 || bad.length > 0;
  };
  body.addEventListener('input', update);
  body.addEventListener('change', update);
  const dialog = modal({
    title: t('elicit_title', { server: card.server }),
    body,
    footer: [decline, send],
    onClose: () => {
      open.delete(card.id);
      // dismissed without a choice
      if (!answered) call('mcp_elicit_respond', { id: card.id, action: 'cancel' }).catch(() => {});
    },
  });
  open.set(card.id, () => { answered = true; dialog.close(); });
  update();
}

onBackend('mcp-elicit', show);
// answered elsewhere, or expired
onBackend('mcp-elicit-done', ({ id }) => open.get(id)?.());
// what arrived before this window loaded
call('mcp_elicit_pending').then((cards) => cards.forEach(show), () => {});
