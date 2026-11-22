// npm run test:ui — round 4 front end: the retry line, the sidebar's room
// memory, parking far turns, custom connectors, MCP prompt arguments and
// interrupted connections.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, t } from '../js/i18n.js';
import { retryLabel } from '../js/retry_ui.js';
import { tight, roomStep } from '../js/sidebar_room.js';
import { parked } from '../js/turnwindow.js';
import { connectorProblem, connectorEntry, connectorToken } from '../js/mcp_connector.js';
import { promptArgValues } from '../js/composer.js';
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

test('the sidebar is short of room in a narrow window, or beside an artifact under 1500 px', () => {
  assert.ok(tight(700, false));
  assert.ok(tight(760, false));
  assert.ok(!tight(761, false));
  assert.ok(tight(1400, true));
  assert.ok(!tight(1500, true));
  assert.ok(!tight(1400, false));
});

test('the sidebar folds by itself and opens again when the room is back', () => {
  // an artifact opens in a 1200 px window: the open sidebar folds
  assert.equal(roomStep(false, true, false, false), 'fold');
  // the panel closes (or the window widens): it was folded by itself, so it opens
  assert.equal(roomStep(true, false, true, true), 'open');
  // still tight, or room unchanged: nothing
  assert.equal(roomStep(true, true, true, true), null);
  assert.equal(roomStep(false, false, false, false), null);
});

test('a fold or opening of yours wins over the room', () => {
  // you folded it yourself before the panel opened: it stays folded after
  assert.equal(roomStep(false, true, true, false), null);
  assert.equal(roomStep(true, false, true, false), null);
  // you opened it while the panel was open: closing the panel leaves it open
  assert.equal(roomStep(true, false, false, false), null);
});

test('far turns are parked, near ones drawn, and the band between keeps each as it is', () => {
  const view = 800;
  assert.equal(parked(0, view, true), false, 'on screen');
  assert.equal(parked(1000, view, true), false, 'within two screens');
  assert.equal(parked(4000, view, false), true, 'beyond four screens');
  assert.equal(parked(2500, view, false), false, 'between: drawn stays drawn');
  assert.equal(parked(2500, view, true), true, 'between: parked stays parked');
});

test('a custom connector needs a name of its own and an https URL (http only on this computer)', () => {
  const ok = (url, name = 'docs') => connectorProblem({ name, url }, ['taken']);
  assert.equal(ok('https://example.com/mcp'), null);
  assert.equal(ok('http://localhost:3000/mcp'), null);
  assert.equal(ok('http://127.0.0.1:8080/mcp'), null);
  assert.equal(ok('http://[::1]:8080/mcp'), null);
  assert.equal(ok('http://dev.localhost/mcp'), null);
  assert.equal(ok('http://example.com/mcp'), 'cc_err_https');
  assert.equal(ok('http://localhost.example.com/mcp'), 'cc_err_https');
  assert.equal(ok('ftp://example.com/'), 'cc_err_url');
  assert.equal(ok('example.com/mcp'), 'cc_err_url');
  assert.equal(ok('https://example.com', '  '), 'cc_err_name');
  assert.equal(ok('https://example.com', 'taken'), 'cc_err_taken');
});

test('a connector entry carries the token as a bearer header and keeps the rest of the old entry', () => {
  assert.deepEqual(connectorEntry({ url: ' https://a.example/mcp ', token: '' }), { url: 'https://a.example/mcp' });
  assert.deepEqual(connectorEntry({ url: 'https://a/mcp', token: 'abc' }), { url: 'https://a/mcp', headers: { Authorization: 'Bearer abc' } });
  assert.deepEqual(connectorEntry({ url: 'https://a/mcp', token: 'Basic eHk=' }).headers, { Authorization: 'Basic eHk=' });
  const old = { url: 'https://old/mcp', headers: { authorization: 'Bearer old', 'X-Team': 'red' }, oauth: { clientId: 'c1' }, note: 'kept' };
  assert.deepEqual(connectorEntry({ url: 'https://new/mcp', token: '', clientId: '' }, old), { url: 'https://new/mcp', headers: { 'X-Team': 'red' }, note: 'kept' });
  assert.deepEqual(connectorEntry({ url: 'https://new/mcp', token: 't', clientId: 'c2' }, old).oauth, { clientId: 'c2' });
  assert.equal(connectorToken(old), 'old');
  assert.equal(connectorToken({ url: 'x' }), '');
});

test('MCP prompt arguments: required ones must be filled, empty optional ones are left out', () => {
  const defs = [{ name: 'topic', required: true }, { name: 'tone' }, { name: 'length', required: true }];
  assert.deepEqual(promptArgValues(defs, ['cats', '', 'short']), { missing: -1, args: { topic: 'cats', length: 'short' } });
  assert.equal(promptArgValues(defs, ['cats', 'dry', '  ']).missing, 2);
  assert.equal(promptArgValues(defs, []).missing, 0);
  assert.deepEqual(promptArgValues([], []), { missing: -1, args: {} });
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
