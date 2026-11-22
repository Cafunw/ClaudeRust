// MCP elicitation: a server asks for a few values (a flat form). Each request is a dialog with the
// server's message and fields; Send, Decline or closing it answers the server (mcp_elicit_respond).
import { t } from './i18n.js';
import { call, on as onBackend } from './api.js';
import { modal, button, h, esc, errorToast } from './util.js';

/**
 * The answer to form `schema` from `values` (field -> an input's text, or a checkbox's state), in the
 * types the schema asks for; empty fields are left out. `missing`: required fields left empty, `bad`:
 * number fields that don't hold one.
 */
export function formContent(schema, values) {
  const required = new Set(schema?.required || []);
  const out = { content: {}, missing: [], bad: [] };
  for (const [name, p] of Object.entries(schema?.properties || {})) {
    const v = values[name];
    if (p.type === 'boolean') { out.content[name] = Boolean(v); continue; }
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
  let input;
  if (Array.isArray(p.enum)) {
    const options = p.enum.map((v, i) => `<option value="${esc(v)}"${v === p.default ? ' selected' : ''}>${esc(p.enumNames?.[i] ?? v)}</option>`).join('');
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

function show(card) {
  if (open.has(card.id)) return;
  const props = card.schema?.properties || {};
  const required = new Set(card.schema?.required || []);
  // the note first: a long message from the server can't push it out of sight
  const body = h(`<div>
    <p class="help" style="margin:0 0 10px">${esc(t('elicit_note'))}</p>
    <p style="margin:0 0 16px;white-space:pre-wrap">${esc(card.message || '')}</p>
    ${Object.entries(props).map(([name, p]) => fieldHtml(name, p, required.has(name))).join('')}
  </div>`);
  const values = () => Object.fromEntries([...body.querySelectorAll('[data-f]')].map((el) => [el.dataset.f, el.type === 'checkbox' ? el.checked : el.value]));
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
