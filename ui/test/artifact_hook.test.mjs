// npm run test:ui — the first script of every artifact page
// (src-tauri/src/artifact_guard.js) undoes what the webview's start-up scripts
// change in each frame: confirm/alert that ask the app, and the link handler
// that opens pages in the browser. Run here after stand-ins for those scripts.
import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const HOOK = readFileSync(new URL('../../src-tauri/src/artifact_guard.js', import.meta.url), 'utf8');

function frame() {
  const listeners = [];
  const posted = [];
  const asked = [];
  const win = {
    URL,
    location: { href: 'http://artifact.localhost/tok', origin: 'http://artifact.localhost' },
    // copied out of the frame's realm, as postMessage would
    parent: { postMessage: (m) => posted.push(structuredClone(m)) },
    addEventListener: (type, fn, capture) => listeners.push({ type, fn, capture: !!capture }),
  };
  win.window = win;
  // the dialog plugin's start-up script: confirm is async (always truthy), alert asks the app
  win.alert = (m) => asked.push(['alert', m]);
  win.confirm = async () => true;
  // the opener plugin's: a bubbling click handler that opens links unless the default was prevented
  win.addEventListener('click', (e) => { if (!e.defaultPrevented) asked.push(['open', e.composedPath()[0].href]); });
  vm.createContext(win);
  vm.runInContext(HOOK, win);
  // capture listeners first, then the bubbling ones, as the browser does at the window
  const click = (a) => {
    const e = { defaultPrevented: false, composedPath: () => [a, { nodeName: 'BODY' }], preventDefault() { this.defaultPrevented = true; } };
    for (const l of [...listeners.filter((x) => x.capture), ...listeners.filter((x) => !x.capture)]) if (l.type === 'click') l.fn(e);
    return e;
  };
  return { win, posted, asked, click };
}

const link = (href, attr = href) => ({ nodeName: 'A', href, getAttribute: () => attr });

test('confirm is false and alert and prompt do nothing, as in a sandbox without allow-modals', () => {
  const f = frame();
  assert.equal(f.win.confirm('Delete everything?'), false);
  assert.equal(f.win.alert('hi'), undefined);
  assert.equal(f.win.prompt('name?'), null);
  assert.deepEqual(f.asked, [], 'nothing reached the app');
});

test('a link to the web is not opened by the plugin: the app is asked to, once', () => {
  const f = frame();
  const e = f.click(link('https://example.com/a'));
  assert.equal(e.defaultPrevented, true);
  assert.deepEqual(f.asked, [], 'the opener plugin saw a prevented click');
  assert.deepEqual(f.posted, [{ __artifact: 'link', url: 'https://example.com/a' }]);
  f.click(link('mailto:a@b.c'));
  assert.equal(f.posted.at(-1).url, 'mailto:a@b.c');
});

test('in-page and non-web links are left alone', () => {
  const f = frame();
  for (const a of [link('http://artifact.localhost/tok#x', '#x'), link('http://artifact.localhost/other'), link('javascript:void(0)'), link('blob:x'), { nodeName: 'DIV' }]) {
    assert.equal(f.click(a).defaultPrevented, false, a.href);
  }
  assert.deepEqual(f.posted, []);
});

test('window.open asks the app and opens nothing itself', () => {
  const f = frame();
  assert.equal(f.win.open('https://example.com/b'), null);
  assert.equal(f.win.open(), null);
  assert.equal(f.win.open('/page2'), null);
  assert.equal(f.win.open('http://[bad'), null);
  assert.deepEqual(f.posted.map((m) => m.url), ['https://example.com/b'], 'only a web address leaves');
});

test('the frame gets neither modals nor popups', () => {
  const src = readFileSync(new URL('../js/artifacts.js', import.meta.url), 'utf8');
  const sandbox = src.match(/setAttribute\('sandbox', '([^']*)'\)/)[1].split(' ');
  for (const flag of ['allow-modals', 'allow-popups', 'allow-popups-to-escape-sandbox', 'allow-same-origin']) assert.ok(!sandbox.includes(flag), flag);
  // the one exception: a deck or doc prints itself (PDF); its page is the app's runtime and its content runs no script
  assert.deepEqual(src.match(/sandbox\.add\('[^']*'\)/g), ["sandbox.add('allow-modals')"]);
  assert.match(src, /if \(a\.type === SLIDES \|\| a\.type === DOC\) frame\.sandbox\.add\('allow-modals'\)/);
});
