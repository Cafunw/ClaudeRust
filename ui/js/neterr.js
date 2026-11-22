// Transport errors (the backend passes them on as text: reqwest's "error
// sending request…", timeouts, HTTP statuses, a program that isn't there)
// said plainly, with the raw text kept behind "Details".
import { t } from './i18n.js';
import { icon } from './icons.js';
import { esc, h } from './util.js';

/** A plain, translated message for a transport error's text, or null when it isn't one. */
export function friendlyError(raw) {
  const s = String(raw || '');
  let m = s.match(/Could not start `([^`]+)`: (?:program not found|.*\(os error 2\))/);
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

/** An error notice: the plain message with the raw text under "Details", or the raw text when there's no plainer one. */
export function errorNotice(raw) {
  const plain = friendlyError(raw);
  const body = plain
    ? `<div>${esc(plain)}</div><details class="err-raw"><summary>${esc(t('err_details'))}</summary><div class="mono">${esc(raw)}</div></details>`
    : `<div class="mono">${esc(raw)}</div>`;
  return h(`<div class="notice error">${icon('alert', 15)}<div class="grow small">${body}</div></div>`);
}
