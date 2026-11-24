// npm run test:ui — an artifact's error is shown once: the app's banner above
// the frame says it (and offers "Try fixing"), so the page's own overlay
// (src-tauri/src/artifact_hook.js) stays hidden once the app acknowledged it.
// A frame without such a host (nothing answers) still shows its overlay.
import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { errorText } from '../js/artifact_bridge.js';

const HOOK = readFileSync(new URL('../../src-tauri/src/artifact_hook.js', import.meta.url), 'utf8');

function frame() {
  const listeners = {};
  const posted = [];
  const timers = [];
  let overlay = null;
  const parent = { postMessage: (m) => posted.push(structuredClone(m)) };
  const win = {
    parent,
    addEventListener: (type, fn) => { listeners[type] = fn; },
    setTimeout: (fn) => { timers.push(fn); return timers.length; },
    clearTimeout: (i) => { timers[i - 1] = null; },
    document: {
      getElementById: () => overlay,
      createElement: () => ({ style: {}, remove() { overlay = null; } }),
      body: { appendChild: (el) => { overlay = el; } },
    },
  };
  win.window = win;
  vm.createContext(win);
  vm.runInContext(HOOK, win);
  const flush = () => { while (timers.length) { const f = timers.shift(); if (f) f(); } };
  const fromHost = (data) => listeners.message({ source: parent, data });
  return { listeners, posted, flush, fromHost, overlay: () => overlay };
}

test('with no answer from the app, the page shows the error itself', () => {
  const f = frame();
  f.listeners.error({ message: 'boom' });
  assert.equal(f.posted[0].__artifact, 'error');
  assert.equal(f.overlay(), null, 'not at once: the app gets a moment to say it shows it');
  f.flush();
  assert.equal(f.overlay().textContent, 'Error: boom');
});

test('acknowledged by the app, the page draws no overlay, now or for later errors', () => {
  const f = frame();
  f.listeners.error({ message: 'boom' });
  f.fromHost({ __artifact: 'error-ack' });
  f.flush();
  assert.equal(f.overlay(), null);
  f.listeners.error({ message: 'again' });
  f.flush();
  assert.equal(f.overlay(), null);
  assert.equal(f.posted.length, 2, 'every error still goes to the app');
});

test('an acknowledgement that comes late removes the overlay already drawn', () => {
  const f = frame();
  f.listeners.error({ message: 'boom' });
  f.flush();
  assert.ok(f.overlay());
  f.fromHost({ __artifact: 'error-ack' });
  assert.equal(f.overlay(), null);
});

test('only the parent can acknowledge', () => {
  const f = frame();
  f.listeners.error({ message: 'boom' });
  f.listeners.message({ source: {}, data: { __artifact: 'error-ack' } });
  f.flush();
  assert.ok(f.overlay());
});

test('a blocked request says where it went, for the banner to say in the app\'s language', () => {
  const f = frame();
  f.listeners.securitypolicyviolation({ blockedURI: 'https://api.example.com/x', effectiveDirective: 'connect-src' });
  assert.deepEqual(f.posted[0].blocked, { uri: 'https://api.example.com/x', directive: 'connect-src' });
  const t = (k, v) => `${k}:${v.uri}:${v.directive}`;
  assert.equal(errorText({ message: 'raw', blocked: f.posted[0].blocked }, t), 'art_csp_blocked:https://api.example.com/x:connect-src');
  assert.equal(errorText({ message: 'raw', blocked: null }, t), 'raw', 'the page\'s own errors are shown as they are');
  // a library the CSP doesn't list is named as such, not as a network request
  assert.equal(errorText({ message: 'raw', blocked: { uri: 'https://esm.sh/x@1', directive: 'script-src-elem' } }, t), 'art_csp_blocked_script:https://esm.sh/x@1:script-src-elem');
});

test('the panel acknowledges every error its frame reports', () => {
  const src = readFileSync(new URL('../js/artifacts.js', import.meta.url), 'utf8');
  assert.match(src, /d\.__artifact === 'error'\) \{ e\.source\.postMessage\(\{ __artifact: 'error-ack' \}, '\*'\); frameError/);
});
