// npm run test:ui — transport errors said plainly (ui/js/neterr.js).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, t } from '../js/i18n.js';
import { friendlyError } from '../js/neterr.js';

test('the common transport errors get a plain message', () => {
  setLanguage('en');
  const cases = [
    ['network error: error sending request for url (http://127.0.0.1:9/mcp)', t('neterr_connect')],
    ['network error: error trying to connect: tcp connect error: Connection refused (os error 10061)', t('neterr_connect')],
    ["MCP server 'remote' timed out on initialize", t('neterr_timeout')],
    ['network error: error sending request for url (https://x.invalid/): dns error: failed to lookup address information', t('neterr_dns')],
    ['network error: invalid peer certificate: UnknownIssuer', t('neterr_tls')],
    ['MCP HTTP 401: unauthorized', t('neterr_auth', { code: 401 })],
    ['network error: HTTP status client error (403 Forbidden) for url (https://a/)', t('neterr_auth', { code: 403 })],
    ['MCP HTTP 404: not found', t('neterr_not_found')],
    ['MCP HTTP 502: bad gateway', t('neterr_server', { code: 502 })],
    ['MCP HTTP 418: teapot', t('neterr_http', { code: 418 })],
    ['Could not start `definitely-not-a-real-binary-xyz`: program not found', t('neterr_no_program', { cmd: 'definitely-not-a-real-binary-xyz' })],
    ['Could not start `npx`: The system cannot find the file specified. (os error 2)', t('neterr_no_program', { cmd: 'npx' })],
  ];
  for (const [raw, want] of cases) assert.equal(friendlyError(raw), want, raw);
});

test('anything else is left as it is', () => {
  for (const raw of ['Invalid JSON in claude_desktop_config.json at line 3', '', null, 'http://127.0.0.1:9/mcp is fine']) assert.equal(friendlyError(raw), null, String(raw));
});

test('the messages are translated', () => {
  setLanguage('tr');
  assert.match(friendlyError('network error: error sending request for url (http://h/)'), /Sunucuya bağlanılamadı/);
  setLanguage('en');
});
