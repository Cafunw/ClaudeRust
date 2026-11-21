// Sign in to and out of remote MCP servers that use OAuth (the server cards
// in Settings → Developer). Signing in opens the system browser.
import { t } from './i18n.js';
import { call } from './api.js';
import { button, errorToast } from './util.js';

/** What server `s` (an mcp_status entry) offers: 'sign_in', 'sign_out'. */
export function oauthActions(s) {
  if (s.transport !== 'http') return [];
  return [s.status === 'needs_auth' && 'sign_in', s.signed_in && 'sign_out'].filter(Boolean);
}

/** Buttons for `oauthActions(s)`; `redraw` runs once one has done its work. */
export function oauthControls(s, redraw) {
  const actions = oauthActions(s);
  if (!actions.length) return document.createDocumentFragment();
  const box = document.createElement('div');
  box.className = 'row';
  box.style.cssText = 'margin-top:10px;gap:8px';
  for (const a of actions) {
    const b = button(t('mcp_' + a), a === 'sign_in' ? 'sm primary' : 'sm ghost', async () => {
      b.disabled = true;
      // the browser has the user until they finish (or five minutes pass)
      if (a === 'sign_in') b.textContent = t('mcp_signing_in');
      try { await call('mcp_oauth_' + a, { server: s.name }); } catch (e) { errorToast(e); }
      redraw();
    });
    box.appendChild(b);
  }
  return box;
}
