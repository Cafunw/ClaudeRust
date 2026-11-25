// npm run test:ui — the Artifacts gallery's thumbnails: a page that needs its
// code to draw anything (React, Mermaid, HTML with a script) is drawn by it,
// in the panel's sandbox but inert; a static page runs nothing; a page that
// throws becomes an error tile, and its own overlay is acked away.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';

let onMessage = null;
window.addEventListener = (type, fn) => { if (type === 'message') onMessage = fn; };
window.__TAURI__.core.invoke = async (cmd) => { if (cmd === 'serve_artifact') return 'tok'; throw new Error(cmd); };
globalThis.ResizeObserver = class { observe() {} disconnect() {} };
const frames = [];
document.createElement = (tag) => {
  const el = { tag, attrs: {}, style: {}, contentWindow: { posted: [], postMessage(m) { this.posted.push(m); } }, setAttribute(k, v) { this.attrs[k] = v; }, getAttribute(k) { return this.attrs[k]; } };
  if (tag === 'iframe') frames.push(el);
  return el;
};
const box = () => ({ clientWidth: 300, clientHeight: 104, innerHTML: '', kids: [], appendChild(c) { this.kids.push(c); c.parentElement = this; } });
const { renderPreview, errorTile } = await import('../js/artifact_preview.js');
const opts = { dark: false, lang: 'en', iconName: 'code' };

test('pages that need their code run it, inert; a static page runs nothing', async () => {
  const cases = [
    ['application/vnd.ant.react', 'export default () => <b>Hi</b>', 'allow-scripts'],
    ['application/vnd.ant.mermaid', 'graph TD; A-->B', 'allow-scripts'],
    ['text/html', '<p>hi</p><script>draw()</script>', 'allow-scripts'],
    ['text/html', '<h1>Static</h1>', ''],
  ];
  for (const [kind, content, sandbox] of cases) {
    const el = box();
    await renderPreview(el, { kind, content, title: 't' }, opts);
    const f = el.kids[0];
    assert.equal(f.tag, 'iframe', kind);
    assert.equal(f.attrs.sandbox, sandbox, kind);
    assert.equal(f.inert, true, `${kind}: never takes focus or clicks`);
    assert.equal(f.attrs.tabindex, '-1');
  }
});

test('a thumbnail that throws becomes an error tile, and its page is told', async () => {
  const el = box();
  await renderPreview(el, { kind: 'text/html', content: '<script>throw new Error("boom")</script><p>hi</p>', title: 'Broken' }, opts);
  const f = frames.at(-1);
  document.querySelectorAll = (sel) => (sel === '.artifact-tile .preview iframe' ? [f] : []);
  onMessage({ data: { __artifact: 'error', message: 'boom' }, source: f.contentWindow });
  assert.deepEqual(f.contentWindow.posted, [{ __artifact: 'error-ack' }]);
  assert.match(el.innerHTML, /class="art-thumb-error"/);
  assert.match(el.innerHTML, /This artifact ran into an error\./);
  // a message from anything else (the panel's frame, another window) is not ours
  const other = box();
  errorTile(other);
  onMessage({ data: { __artifact: 'error' }, source: {} });
  assert.equal(f.contentWindow.posted.length, 1);
});

test('the gallery lets a running thumbnail go once it is out of reach', () => {
  const js = source('../js/artifacts.js');
  assert.match(js, /if \(!en\.isIntersecting\) \{[^}]*box\.__drawing = null;\s*if \(box\.querySelector\('iframe\[sandbox="allow-scripts"\]'\)\) box\.replaceChildren\(\);\s*return;\s*\}\s*if \(box\.childElementCount \|\| box\.__drawing\) return;/);
  // one picture on its way at a time, and only the latest may land (in, out and in again: one frame)
  assert.match(js, /const mine = box\.__drawing = \{\};\s*renderPreview\(box, a, \{[^}]*current: \(\) => box\.__drawing === mine \}\)/);
  const css = source('../css/app.css');
  // danger on the tile's own background: a tint under it would take the light theme below 4.5:1
  assert.match(css, /\.art-thumb-error \{[^}]*color: var\(--danger\); \}/);
});

test('a page is drawn 800 px wide in a window of the tile’s shape, so a small component is seen', async () => {
  for (const kind of ['application/vnd.ant.react', 'text/html']) {
    const el = box();
    await renderPreview(el, { kind, content: '<script>x()</script>', title: 't' }, opts);
    const css = el.kids[0].style.cssText;
    // a 300 x 104 px tile: scale 300/800, a window 104/0.375 px tall
    assert.match(css, /width:800px;height:277px;/, kind);
    assert.match(css, /transform:scale\(0\.375\)/, kind);
  }
  // a deck is still fitted whole, 1920 x 1080
  const deck = box();
  await renderPreview(deck, { kind: 'application/vnd.ant.slides', content: '<section></section>', title: 'd' }, opts);
  assert.match(deck.kids[0].style.cssText, /width:1920px;height:1080px;/);
});

test('a tile that went out of reach while its page was on its way gets no frame', async () => {
  const el = box();
  await renderPreview(el, { kind: 'application/vnd.ant.react', content: 'x', title: 't' }, { ...opts, current: () => false });
  assert.equal(el.kids.length, 0);
});
