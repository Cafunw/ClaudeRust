// Mermaid code blocks in replies, drawn as diagrams. Each one renders in its
// own sandboxed frame on the artifact:// path (scripts only: no same-origin,
// no popups, no modals), once it comes near the view; the source stays a
// click away, and a block that can't be drawn stays a code block.
import { call, artifactUrl } from './api.js';
import { t } from './i18n.js';
import { esc, h } from './util.js';
import { icon } from './icons.js';

// the build Mermaid artifacts use (artifacts.rs); the frame's CSP allows its CDN
// shortcut: loaded from the CDN like every artifact library, so offline the block stays code; vendor it with the artifact libraries (#26)
const MERMAID = 'https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.esm.min.mjs';
const GIVE_UP_MS = 20000;

/** The frame's document for diagram `src`: it reports `{__mermaid, ok, height}` to the app. */
export function mermaidDoc(src, dark) {
  // (the app's colour scheme, so the frame stays see-through)
  return `<!doctype html><html><head><meta charset="utf-8"><style>:root{color-scheme:${dark ? 'dark' : 'light'}}html,body{margin:0;background:transparent}body{display:flex;justify-content:center}svg{max-width:100%;height:auto}</style></head>
<body><pre id="src" hidden>${esc(src)}</pre><div id="out"></div><script type="module">
const post = (m) => parent.postMessage(Object.assign({ __mermaid: 1 }, m), '*');
try {
  const { default: mermaid } = await import('${MERMAID}');
  mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: '${dark ? 'dark' : 'default'}' });
  const { svg } = await mermaid.render('d', document.getElementById('src').textContent);
  document.getElementById('out').innerHTML = svg;
  post({ ok: true, height: Math.ceil(document.getElementById('out').getBoundingClientRect().height) });
} catch (e) { post({ ok: false }); }
</script></body></html>`;
}

let io = null;
function observer() {
  io ||= new IntersectionObserver((entries) => {
    for (const en of entries) if (en.isIntersecting) { io.unobserve(en.target); draw(en.target); }
  }, { rootMargin: '400px 0px' });
  return io;
}

/** Draw `block` (a finished ```mermaid code block) as a diagram when it comes into view. */
export function watchMermaid(block) {
  if (block.dataset.mermaid || block.hasAttribute('data-open')) return;
  block.dataset.mermaid = 'wait';
  observer().observe(block);
}

function failed(block, frame) {
  frame.remove();
  block.dataset.mermaid = 'failed';
}

async function draw(block) {
  const src = (block.querySelector('code') || {}).textContent || '';
  if (!block.isConnected || !src.trim()) return;
  const dark = document.documentElement.dataset.theme === 'dark';
  let token;
  try { token = await call('serve_artifact', { kind: 'text/html', content: mermaidDoc(src, dark), dark }); } catch { block.dataset.mermaid = 'failed'; return; }
  if (!block.isConnected) return;
  const frame = document.createElement('iframe');
  frame.className = 'mermaid-frame';
  frame.setAttribute('sandbox', 'allow-scripts');
  frame.setAttribute('referrerpolicy', 'no-referrer');
  frame.title = t('mermaid_title');
  frame.src = artifactUrl(token);
  const giveUp = setTimeout(() => failed(block, frame), GIVE_UP_MS);
  frame.__done = (d) => {
    clearTimeout(giveUp);
    if (!d.ok || !(d.height > 0)) { failed(block, frame); return; }
    frame.style.height = `${Math.min(d.height, 2000)}px`;
    block.dataset.mermaid = 'drawn';
    block.classList.add('diagram');
    addToggle(block);
  };
  block.appendChild(frame);
}

/** "Show source" / "Show diagram" in the block's header. */
function addToggle(block) {
  const head = block.querySelector('.code-header');
  if (!head || head.querySelector('.mermaid-toggle')) return;
  const b = h(`<button class="code-run mermaid-toggle" type="button" aria-pressed="false">${icon('code', 14)}<span>${esc(t('mermaid_source'))}</span></button>`);
  b.onclick = () => {
    const src = block.classList.toggle('diagram') === false;
    b.setAttribute('aria-pressed', String(src));
    b.querySelector('span').textContent = t(src ? 'mermaid_diagram' : 'mermaid_source');
  };
  head.querySelector('.code-copy').before(b);
}

// the frames' reports (only from a diagram frame of ours)
window.addEventListener('message', (e) => {
  const d = e.data;
  if (!d || d.__mermaid !== 1) return;
  const frame = [...document.querySelectorAll('iframe.mermaid-frame')].find((f) => f.contentWindow === e.source);
  if (frame && frame.__done) { const done = frame.__done; frame.__done = null; done(d); }
});
