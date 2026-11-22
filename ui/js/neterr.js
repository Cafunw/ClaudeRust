// Failures said plainly, with the raw text kept behind "Details": the ones the
// backend names by a code (errors.js), and transport errors it passes on as
// text (reqwest's "error sending request…", timeouts, HTTP statuses, a program
// that isn't there).
import { codedMessage } from './errors.js';
import { t } from './i18n.js';
import { icon } from './icons.js';
import { esc, h } from './util.js';

const rawText = (e) => (e && typeof e === 'object' ? String(e.message || '') : String(e || ''));

/** A plain, translated message for a failure (an error object, or its text), or null when there's none plainer than its text. */
export function friendlyError(raw) {
  if (raw && typeof raw === 'object') return codedMessage(raw) || friendlyError(raw.message);
  const s = String(raw || '');
  // (os error 3) is Windows' "path not found", for a command given with a folder that isn't there
  let m = s.match(/Could not start `([^`]+)`: (?:program not found|.*\(os error [23]\))/);
  if (m) return t('neterr_no_program', { cmd: m[1] });
  if (/timed out|time ?out/i.test(s)) return t('neterr_timeout');
  if (/dns error|failed to lookup|no such host|name or service not known/i.test(s)) return t('neterr_dns');
  if (/certificate|\btls\b|\bssl\b/i.test(s)) return t('neterr_tls');
  m = s.match(/\bHTTP(?: status \w+ error \(| )(\d{3})\b/);
  if (m) {
    const code = Number(m[1]);
    if (code === 401 || code === 403) return t('neterr_auth', { code });
    if (code === 404) return t('neterr_not_found');
    return t(code >= 500 ? 'neterr_server' : 'neterr_http', { code });
  }
  m = s.match(/connection stalled: nothing arrived for (\d+) s/i);
  if (m) return t('neterr_stalled', { s: m[1] });
  // (before "connection closed": the answer had started, the connection broke while it came)
  if (/error decoding response body|error reading a body|unexpected end of file|end of file before message|closed before message completed/i.test(s)) return t('neterr_interrupted');
  if (/error sending request|error trying to connect|connection (?:refused|reset|closed)|^network error/i.test(s)) return t('neterr_connect');
  return null;
}

/** The text to show for a failure (or a coded notice): its plain message, else its own text. */
export function errorText(e) {
  return friendlyError(e) || rawText(e);
}

/** An error notice: the plain message with the raw text under "Details", or the raw text when there's no plainer one. */
export function errorNotice(raw) {
  const plain = friendlyError(raw);
  const text = rawText(raw);
  const details = text && text !== plain ? `<details class="err-raw"><summary>${esc(t('err_details'))}</summary><div class="mono">${esc(text)}</div></details>` : '';
  const body = plain ? `<div>${esc(plain)}</div>${details}` : `<div class="mono">${esc(text)}</div>`;
  return h(`<div class="notice error">${icon('alert', 15)}<div class="grow small">${body}</div></div>`);
}
