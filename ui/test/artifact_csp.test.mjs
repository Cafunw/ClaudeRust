// npm run test:ui — artifacts reach no host but the libraries' (ARTIFACT_CSP in
// src-tauri/src/artifacts.rs). A blocked request fails quietly, so the page's
// hook (src-tauri/src/artifact_hook.js) reports it as the artifact's error:
// the panel shows it and "Try fixing" hands it to Claude.
import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const HOOK = readFileSync(new URL('../../src-tauri/src/artifact_hook.js', import.meta.url), 'utf8');

function frame() {
  const listeners = {};
  const posted = [];
  const overlay = { style: {} };
  const win = {
    parent: { postMessage: (m) => posted.push(structuredClone(m)) },
    addEventListener: (type, fn) => { listeners[type] = fn; },
    document: { getElementById: () => null, createElement: () => overlay, body: { appendChild: () => {} } },
  };
  win.window = win;
  vm.createContext(win);
  vm.runInContext(HOOK, win);
  return { listeners, posted, overlay };
}

test('a request the sandbox blocked is reported, with where it went and why', () => {
  const f = frame();
  f.listeners.securitypolicyviolation({ blockedURI: 'https://api.example.com/data', effectiveDirective: 'connect-src' });
  assert.equal(f.posted.length, 1);
  const { __artifact, message, stack } = f.posted[0];
  assert.equal(__artifact, 'error');
  assert.match(message, /blocked https:\/\/api\.example\.com\/data \(connect-src\)/);
  assert.match(message, /cannot reach the network/);
  assert.equal(stack, '');
  assert.equal(f.overlay.textContent, `Error: ${message}`, 'the page shows it too');
});

test('the page\'s own errors are reported as before, with the top of their stack', () => {
  const f = frame();
  const error = new Error('boom');
  error.stack = 'Error: boom\n' + Array.from({ length: 9 }, (_, i) => `    at f${i}`).join('\n');
  f.listeners.error({ message: 'boom', error });
  assert.equal(f.posted[0].message, 'boom');
  assert.equal(f.posted[0].stack.split('\n').length, 6);
});
