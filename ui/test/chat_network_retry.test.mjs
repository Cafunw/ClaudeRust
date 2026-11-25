// npm run test:ui — a request that is retried or a connection that broke:
// the retry line names the reason and counts down, and an interrupted
// answer is said plainly.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, t } from '../js/i18n.js';
import { retryLabel } from '../js/retry_ui.js';
import { friendlyError } from '../js/neterr.js';

test('the retry line names the reason, counts down and shows the attempt', () => {
  setLanguage('en');
  const ev = { conversation_id: 'c', attempt: 2, max_attempts: 5, wait_ms: 12000, reason: 'rate_limited' };
  assert.equal(retryLabel(ev, 12), 'Rate limited, retrying in 12 s (2/5)');
  assert.equal(retryLabel(ev, 0), 'Rate limited, retrying now (2/5)');
  assert.equal(retryLabel({ ...ev, reason: 'overloaded' }, 3), 'The API is overloaded, retrying in 3 s (2/5)');
  assert.equal(retryLabel({ ...ev, reason: 'server_error' }, 1), 'Server error, retrying in 1 s (2/5)');
  assert.equal(retryLabel({ ...ev, reason: 'network' }, -1), 'Connection lost, retrying now (2/5)');
  setLanguage('tr');
  assert.equal(retryLabel(ev, 12), 'Hız sınırına takıldı, 12 sn sonra yeniden denenecek (2/5)');
  setLanguage('en');
});

test('the retry line copes with missing or unknown fields', () => {
  setLanguage('en');
  assert.equal(retryLabel({ reason: 'weird' }, 4), 'Request failed, retrying in 4 s');
  assert.equal(retryLabel({ attempt: 1 }, 4), 'Request failed, retrying in 4 s', 'no total: no count');
  assert.equal(retryLabel(null, 0), 'Request failed, retrying now');
});

test('a connection that broke while the answer came is said plainly', () => {
  setLanguage('en');
  for (const raw of ['network error: error decoding response body', 'network error: error reading a body from connection: unexpected end of file', 'network error: connection closed before message completed']) {
    assert.equal(friendlyError(raw), t('neterr_interrupted'), raw);
  }
  assert.equal(friendlyError('network error: error sending request for url (https://api.anthropic.com/v1/messages)'), t('neterr_connect'));
  assert.equal(friendlyError('The connection stalled: nothing arrived for 90 s.'), 'The connection stalled: nothing arrived for 90 s.');
  setLanguage('tr');
  assert.equal(friendlyError('The connection stalled: nothing arrived for 90 s.'), 'Bağlantı takıldı: 90 sn boyunca hiçbir şey gelmedi.');
  setLanguage('en');
});
