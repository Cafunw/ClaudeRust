// npm run test:ui — adding a custom connector: what makes its name and URL
// acceptable, and the config entry it is saved as.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { connectorProblem, connectorEntry, connectorToken } from '../js/mcp_connector.js';

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
