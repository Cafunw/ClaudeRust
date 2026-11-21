// Claude account sign-in. The official `claude` CLI does the OAuth sign-in in the browser;
// ClaudeRust only starts it and reads who is signed in, and never sees the token.
import { t } from './i18n.js';
import { call } from './api.js';
import { h, button, toast, errorToast } from './util.js';

export const CC_SETUP_URL = 'https://docs.claude.com/en/docs/claude-code/setup';

let cached = null;

/** `{ found, logged_in, email, plan }`: `found` is whether the CLI is installed. Each look starts the CLI, so a recent one is reused unless `fresh`. */
export async function accountStatus(fresh = false) {
  if (!fresh && cached && Date.now() - cached.at < 20_000) return cached.v;
  const v = await call('claude_auth_status').catch(() => ({ found: false }));
  cached = { v, at: Date.now() };
  return v;
}

export const planName = (a) => (a.plan ? a.plan[0].toUpperCase() + a.plan.slice(1) : '');

/** Sign-in button: opens the browser through the CLI, shows the wait with a Cancel, calls `onDone(status)` once signed in. */
export function signInButton(cls, onDone, { wide = false } = {}) {
  const box = h(wide ? '<div style="display:flex;flex-direction:column;gap:8px"></div>' : '<span class="row" style="gap:8px;align-items:center"></span>');
  const cancel = button(t('cancel'), 'ghost', () => call('claude_auth_login_cancel'));
  cancel.classList.add('hidden');
  const go = button(t('acct_signin'), cls);
  go.onclick = async () => {
    go.disabled = true;
    go.textContent = t('acct_waiting');
    cancel.classList.remove('hidden');
    try {
      const a = await call('claude_auth_login');
      if (a.logged_in) { cached = { v: a, at: Date.now() }; onDone(a); }
      else if (!a.cancelled) toast(t('acct_failed'), 'error');
    } catch (e) { errorToast(e); } finally {
      go.disabled = false;
      go.textContent = t('acct_signin');
      cancel.classList.add('hidden');
    }
  };
  box.append(go, cancel);
  return box;
}
