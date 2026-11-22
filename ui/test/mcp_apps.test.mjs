// npm run test:ui — the host side of the MCP Apps bridge (ui/js/mcp_apps.js):
// what an untrusted app page may send, and what it gets back, in order.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { validMessage, createBridge, PROTOCOL } from '../js/mcp_apps.js';

function bridge(over = {}) {
  const sent = [];
  const calls = [];
  const env = {
    send: (m) => sent.push(m),
    input: { city: 'Ankara' },
    result: { content: [{ type: 'text', text: '21°C' }], structuredContent: { c: 21 } },
    hostContext: () => ({ theme: 'dark', displayMode: 'inline' }),
    resize: (h) => calls.push(['resize', h]),
    callTool: async (name, args) => { calls.push(['call', name, args]); return { content: [{ type: 'text', text: 'ok' }] }; },
    openLink: async (url) => { calls.push(['link', url]); },
    ...over,
  };
  return { handle: createBridge(env), sent, calls };
}

const rpc = (method, params, id) => ({ jsonrpc: '2.0', method, ...(params ? { params } : {}), ...(id !== undefined ? { id } : {}) });

test('hostile or malformed messages are ignored', () => {
  const cyclic = rpc('ping', {}, 1);
  cyclic.params.self = cyclic;
  for (const bad of [null, 'x', 42, [], {}, { jsonrpc: '1.0', method: 'ping' }, rpc('', null, 1), rpc('x'.repeat(200)), { jsonrpc: '2.0', method: 'ping', id: {} },
    { jsonrpc: '2.0', method: 'ping', id: 'i'.repeat(300) }, { jsonrpc: '2.0', method: 'ping', params: [1] }, { jsonrpc: '2.0', method: 'ping', params: 'x' },
    rpc('tools/call', { blob: 'x'.repeat(1024 * 1024 + 1) }, 1), cyclic, { jsonrpc: '2.0', id: 1, result: {} }]) {
    assert.equal(validMessage(bad), null, JSON.stringify(bad && typeof bad === 'object' ? Object.keys(bad) : bad));
  }
  assert.ok(validMessage(rpc('ui/initialize', {}, 'a1')));
  assert.ok(validMessage(rpc('ui/notifications/initialized')));
});

test('nothing reaches the page before it is initialized, then its input and result once', async () => {
  const b = bridge();
  await b.handle(rpc('ui/initialize', { protocolVersion: PROTOCOL, appCapabilities: {} }, 1));
  assert.equal(b.sent.length, 1);
  const init = b.sent[0];
  assert.equal(init.id, 1);
  assert.equal(init.result.protocolVersion, PROTOCOL);
  assert.deepEqual(init.result.hostContext, { theme: 'dark', displayMode: 'inline' });
  assert.deepEqual(Object.keys(init.result.hostCapabilities).sort(), ['logging', 'openLinks', 'serverTools'], 'no resources, messages or model context offered');
  await b.handle(rpc('ui/notifications/initialized'));
  await b.handle(rpc('ui/notifications/initialized'));
  assert.deepEqual(b.sent.slice(1).map((m) => m.method), ['ui/notifications/tool-input', 'ui/notifications/tool-result']);
  assert.deepEqual(b.sent[1].params, { arguments: { city: 'Ankara' } });
  assert.equal(b.sent[2].params.structuredContent.c, 21);
});

test('tool calls wait for initialization and go to the app\'s own server through callTool', async () => {
  const b = bridge();
  await b.handle(rpc('tools/call', { name: 'refresh', arguments: {} }, 7));
  assert.equal(b.sent[0].error.code, -32000);
  assert.deepEqual(b.calls, [], 'not called before initialized');
  await b.handle(rpc('ui/notifications/initialized'));
  b.sent.length = 0;
  await b.handle(rpc('tools/call', { name: 'refresh', arguments: { n: 1 } }, 8));
  assert.deepEqual(b.calls, [['call', 'refresh', { n: 1 }]]);
  assert.deepEqual(b.sent[0], { jsonrpc: '2.0', id: 8, result: { content: [{ type: 'text', text: 'ok' }] } });
  // arguments that aren't an object become {}, a missing name is refused
  await b.handle(rpc('tools/call', { name: 'refresh', arguments: [1, 2] }, 9));
  assert.deepEqual(b.calls.at(-1), ['call', 'refresh', {}]);
  await b.handle(rpc('tools/call', { arguments: {} }, 10));
  assert.equal(b.sent.at(-1).error.code, -32602);
  // a notification asks for nothing, so it calls nothing
  await b.handle(rpc('tools/call', { name: 'refresh' }));
  assert.equal(b.calls.filter((c) => c[0] === 'call').length, 2);
});

test('a refused or failing call is an error for the page, not a crash', async () => {
  const b = bridge({ callTool: async () => { throw new Error('The user did not allow this tool call.'); } });
  await b.handle(rpc('ui/notifications/initialized'));
  b.sent.length = 0;
  await b.handle(rpc('tools/call', { name: 'delete_all' }, 3));
  assert.deepEqual(b.sent[0], { jsonrpc: '2.0', id: 3, error: { code: -32000, message: 'The user did not allow this tool call.' } });
});

test('links ask, sizes resize, unknown methods are refused', async () => {
  const b = bridge();
  await b.handle(rpc('ui/open-link', { url: 'https://example.com' }, 1));
  await b.handle(rpc('ui/notifications/size-changed', { width: 300, height: 480 }));
  await b.handle(rpc('ui/request-display-mode', { mode: 'fullscreen' }, 2));
  await b.handle(rpc('ui/message', { role: 'user', content: { type: 'text', text: 'pwn' } }, 3));
  await b.handle(rpc('ui/update-model-context', { content: [] }, 4));
  await b.handle(rpc('resources/read', { uri: 'file:///etc/passwd' }, 5));
  await b.handle(rpc('ping', null, 6));
  assert.deepEqual(b.calls, [['link', 'https://example.com'], ['resize', 480]]);
  const by = (id) => b.sent.find((m) => m.id === id);
  assert.deepEqual(by(1).result, {});
  assert.deepEqual(by(2).result, { mode: 'inline' });
  for (const id of [3, 4, 5]) assert.equal(by(id).error.code, -32601, `id ${id}`);
  assert.deepEqual(by(6).result, {});
});
