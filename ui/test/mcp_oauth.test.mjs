// npm run test:ui
import test from 'node:test';
import assert from 'node:assert/strict';

// api.js reads the Tauri bridge when it loads
globalThis.window = globalThis.window || {};
const { oauthActions } = await import('../js/mcp_oauth_ui.js');

test('a remote server that needs a sign-in offers one', () => {
  assert.deepEqual(oauthActions({ transport: 'http', status: 'needs_auth', signed_in: false }), ['sign_in']);
});

test('a signed-in server offers to sign out, also when its token stopped working', () => {
  assert.deepEqual(oauthActions({ transport: 'http', status: 'connected', signed_in: true }), ['sign_out']);
  assert.deepEqual(oauthActions({ transport: 'http', status: 'failed', signed_in: true }), ['sign_out']);
  assert.deepEqual(oauthActions({ transport: 'http', status: 'needs_auth', signed_in: true }), ['sign_in', 'sign_out']);
});

test('local servers and remote ones without OAuth offer nothing', () => {
  assert.deepEqual(oauthActions({ transport: 'stdio', status: 'failed', signed_in: false }), []);
  assert.deepEqual(oauthActions({ transport: 'http', status: 'connected', signed_in: false }), []);
  assert.deepEqual(oauthActions({ transport: '', status: 'starting' }), []);
});
