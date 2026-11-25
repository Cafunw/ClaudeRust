// npm run test:ui — a chat on the Claude account says the API's failures as a chat on an API key does: the
// backend (claudecode.rs turn_error) stores the CLI's failures with the API engine's kinds and statuses, and the
// error card turns those into the same sentence in the app's language; the CLI's own words stay under Details.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, t } from '../js/i18n.js';
import { errorMessage } from '../js/parts.js';
import { errorText } from '../js/neterr.js';

// what claudecode.rs stores for the failures Claude Code 2.1.326 ends a turn on
const stored = {
  overloaded: { kind: 'overloaded_error', status: 529, message: 'API Error: 529 Overloaded. This is a server-side issue, usually temporary — try again in a moment.' },
  server: { kind: 'api_error', status: 500, message: 'API Error: 500 Internal server error.' },
  limited: { kind: 'rate_limit_error', status: 429, message: 'API Error: 429 Rate limited.' },
  network: { kind: 'network', status: 0, message: 'API Error: Connection dropped (ECONNRESET). Check the gateway set in ANTHROPIC_BASE_URL, then try again.' },
  long: { kind: 'prompt_too_long', status: 0, message: 'Prompt is too long · the request is ~250000 tokens (limit 200000)' },
};

for (const lang of ['en', 'tr']) {
  test(`the account engine's API failures read as the API engine's (${lang})`, () => {
    setLanguage(lang);
    assert.equal(errorMessage(stored.overloaded), t('err_overloaded'));
    assert.equal(errorMessage(stored.server), t('neterr_server', { code: 500 }));
    assert.equal(errorMessage(stored.limited), t('err_rate_limit'));
    assert.equal(errorMessage(stored.network), t('err_network'));
    const long = errorMessage(stored.long);
    assert.ok(long && long !== stored.long.message && !/Claude Code/.test(long), long);
    for (const e of Object.values(stored)) assert.notEqual(errorMessage(e), e.message, 'the CLI’s words are the detail, not the sentence');
    setLanguage('en');
  });
}

test('a chat rebuilt from its history says so in the app’s language', () => {
  setLanguage('tr');
  const n = { kind: 'cc_history_replayed', message: 'This chat goes on in a new Claude Code session, rebuilt from its history.' };
  assert.match(errorText(n), /yeni bir Claude Code oturumunda/);
  assert.match(errorText({ ...n, kind: 'cc_history_partial' }), /eklerinden/);
  setLanguage('en');
  assert.equal(errorText(n), n.message);
});
