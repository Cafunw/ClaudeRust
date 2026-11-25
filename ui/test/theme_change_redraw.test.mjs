// npm run test:ui — what is drawn with the theme baked in (a Mermaid diagram in a reply, an
// MCP App, a server icon made for each theme, the gallery's pictures) follows a theme change
// without a reload; and only a change counts (every settings save sets the theme again).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';
import { mermaidDoc } from '../js/mermaid_block.js';
import { createBridge } from '../js/mcp_apps.js';
import { iconHtml } from '../js/mcp_tool_info.js';

/** Runs the diagram frame's script of `mermaidDoc(src, dark)` with a stand-in Mermaid. */
async function frame(dark, { failFirst = false } = {}) {
  const doc = mermaidDoc('graph TD; A-->B', dark);
  const start = doc.indexOf('<script type="module">') + '<script type="module">'.length;
  const script = doc.slice(start, doc.lastIndexOf('</script>')).replace(/await import\('[^']+'\)/, 'await __lib');
  const posts = [];
  const parent = { postMessage: (m) => posts.push(m) };
  let onMessage = null;
  const out = { innerHTML: '', getBoundingClientRect: () => ({ height: 120 }) };
  const document = { documentElement: { style: {} }, getElementById: (id) => (id === 'out' ? out : { textContent: 'graph TD; A-->B' }) };
  let theme = null, renders = 0;
  const lib = { default: {
    initialize: (o) => { theme = o.theme; },
    render: async (id) => { renders++; if (failFirst && renders === 1) throw new Error('bad'); return { svg: `<svg id="${id}" data-theme="${theme}"></svg>` }; },
  } };
  const AsyncFunction = (async () => {}).constructor;
  await new AsyncFunction('parent', 'addEventListener', 'document', '__lib', script)(parent, (type, f) => { if (type === 'message') onMessage = f; }, document, lib);
  const tell = async (data, source = parent) => { onMessage({ source, data }); for (let i = 0; i < 5; i++) await Promise.resolve(); };
  return { posts, out, document, tell };
}

test('a diagram draws itself again in the theme the app moves to, and says so', async () => {
  const f = await frame(false);
  assert.deepEqual(f.posts, [{ __mermaid: 1, ok: true, height: 120, dark: false }]);
  assert.match(f.out.innerHTML, /data-theme="default"/);
  await f.tell({ __mermaid: 'theme', dark: true });
  assert.deepEqual(f.posts.at(-1), { __mermaid: 1, ok: true, height: 120, dark: true });
  assert.match(f.out.innerHTML, /data-theme="dark"/);
  assert.equal(f.document.documentElement.style.colorScheme, 'dark');
  // a new id each time: the diagram shown stays until the next one is ready
  assert.match(f.out.innerHTML, /id="d1"/);
  await f.tell({ __mermaid: 'theme', dark: true });
  assert.equal(f.posts.length, 2, 'the theme it already has draws nothing');
  await f.tell({ __mermaid: 'theme', dark: false }, { postMessage() {} });
  assert.equal(f.posts.length, 2, 'only the app may ask');
});

test('a diagram that could not be drawn still answers a theme change', async () => {
  const f = await frame(true, { failFirst: true });
  assert.deepEqual(f.posts, [{ __mermaid: 1, ok: false }]);
  await f.tell({ __mermaid: 'theme', dark: false });
  assert.deepEqual(f.posts.at(-1), { __mermaid: 1, ok: true, height: 120, dark: false });
});

test('the app re-themes every diagram on the page, and one that reports the old theme', () => {
  const s = source('../js/mermaid_block.js');
  assert.match(s, /new MutationObserver\(\(\) => document\.querySelectorAll\('iframe\.mermaid-frame'\)\.forEach\(retheme\)\)\s*\.observe\(document\.documentElement, \{ attributes: true, attributeFilter: \['data-theme'\] \}\)/);
  assert.match(s, /if \(d\.ok && 'dark' in d && d\.dark !== isDark\(\)\) retheme\(frame\);/);
  // a later report (drawn again) sets the frame's height, with no reload of the frame
  assert.match(s, /else if \(d\.ok && d\.height > 0\) frame\.style\.height/);
});

test('every settings save sets the theme only when it changed (watchers redraw their frames on it)', () => {
  assert.match(source('../js/main.js'), /if \(document\.documentElement\.dataset\.theme !== theme\) document\.documentElement\.dataset\.theme = theme;/);
});

test('the gallery draws its pictures again in a new theme, keeping its cards', () => {
  const s = source('../js/artifacts.js');
  assert.match(s, /if \(redrawGallery\) redrawGallery\(\);/);
  assert.match(s, /box\.__drawing = null; box\.replaceChildren\(\); seen\.unobserve\(box\); seen\.observe\(box\);/);
});

test('an MCP App is told of a new theme once it is initialized', async () => {
  const sent = [];
  const handle = createBridge({ send: (m) => sent.push(m), input: {}, result: null, hostContext: () => ({ theme: 'light' }), resize() {}, callTool() {}, openLink() {} });
  handle.contextChanged({ theme: 'dark' });
  assert.equal(sent.length, 0, 'nothing goes to the page before it is initialized');
  await handle({ jsonrpc: '2.0', method: 'ui/notifications/initialized' });
  sent.length = 0;
  handle.contextChanged({ theme: 'dark' });
  assert.deepEqual(sent, [{ jsonrpc: '2.0', method: 'ui/notifications/host-context-changed', params: { theme: 'dark' } }]);
  assert.match(source('../js/mcp_apps.js'), /for \(const \[frame, handle\] of frames\) if \(frame\.isConnected\) handle\.contextChanged\(\{ theme \}\);/);
});

test('a server icon made for each theme keeps both, to follow a theme change', () => {
  const both = iconHtml([{ src: 'https://e.example/l.svg', theme: 'light' }, { src: 'https://e.example/d.svg', theme: 'dark' }], 16, 'plug');
  assert.match(both, /src="https:\/\/e\.example\/l\.svg"/);
  assert.match(both, /data-light="https:\/\/e\.example\/l\.svg" data-dark="https:\/\/e\.example\/d\.svg"/);
  assert.doesNotMatch(iconHtml([{ src: 'https://e.example/a.svg' }], 16, 'plug'), /data-dark/);
  assert.match(source('../js/mcp_tool_info.js'), /querySelectorAll\('img\.mcp-icon\[data-dark\]'\)\.forEach\(\(img\) => \{ img\.src = img\.dataset\[theme\]; \}\)/);
});

test('a doc, sheet or deck in the panel takes a new theme where it is, so an edit under way stays', () => {
  const s = source('../js/artifacts.js');
  assert.match(s, /const RETHEMED = new Set\(\[SLIDES, DOC, SHEET\]\);/);
  // only a frame that has loaded (one still loading has the old theme in it: drawn again)
  assert.match(s, /aside\(\)\.querySelector\('\.ap-body:not\(\.loading\) iframe'\);\n\s*if \(f && f\.contentWindow && RETHEMED\.has\(f\.dataset\.kind\)\) f\.contentWindow\.postMessage\(\{ __artifact: 'theme', dark: isDark\(\) \}, '\*'\);\n\s*else if \(panel\) \{ panel\.lastDocKey = null; render\(\); \}/);
  assert.match(s, /\$\{view === 'preview' && RETHEMED\.has\(a\.type\) \? '' : isDark\(\)\}/, 'and its next render does not reload it for the theme');
  const take = /if \(e\.data\.__artifact === 'theme'\) \{ document\.body\.classList\.toggle\('dark', !!e\.data\.dark\); document\.body\.classList\.toggle\('light', !e\.data\.dark\); \}/;
  for (const f of ['doc', 'sheet', 'slides']) assert.match(source(`../js/artifact_types/${f}.js`), take, f);
});
