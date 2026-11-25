// npm run test:ui — the Artifacts gallery's thumbnails: a page that needs its
// code to draw anything (React, Mermaid, HTML with a script) is drawn by it,
// in the panel's sandbox but inert; a static page runs nothing; a page that
// throws becomes an error tile, and its own overlay is acked away.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

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
  const js = readFileSync(new URL('../js/artifacts.js', import.meta.url), 'utf8');
  assert.match(js, /if \(!en\.isIntersecting\) \{\s*if \(en\.target\.querySelector\('iframe\[sandbox="allow-scripts"\]'\)\) en\.target\.replaceChildren\(\);\s*return;\s*\}\s*if \(en\.target\.childElementCount\) return;/);
  const css = readFileSync(new URL('../css/app.css', import.meta.url), 'utf8');
  // danger on the tile's own background: a tint under it would take the light theme below 4.5:1
  assert.match(css, /\.art-thumb-error \{[^}]*color: var\(--danger\); \}/);
});
