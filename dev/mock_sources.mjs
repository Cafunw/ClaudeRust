// A local stand-in for GitHub and Google Drive, to try "Add from GitHub" and
// "Add from a link" in a debug build without the internet:
//
//   node dev/mock_sources.mjs [port]          (default 8790)
//   $env:CLAUDERUST_GITHUB_BASE='http://127.0.0.1:8790'; $env:CLAUDERUST_GOOGLE_BASE='http://127.0.0.1:8790'; npx tauri dev ...
//
// (debug builds only read those variables: github_source.rs, link_source.rs)
//
// GitHub: https://github.com/octo/demo (also /tree/feature/x/src, a branch
// with a slash), https://github.com/octo/private (found only with a token),
// https://github.com/octo/limited (rate limited).
// Google, by the id in the link: DOC0000000 (a Doc), SHEET00000 (a Sheet),
// SLIDES0000 (Slides), FILE000000 (a Drive file), PRIVATE000 (not shared).
import http from 'node:http';

const SHA = '0123456789abcdef0123456789abcdef01234567';
const FILES = {
  'README.md': '# Demo\n\nA repository served by dev/mock_sources.mjs.\n',
  'src/main.rs': 'fn main() {\n    println!("hello");\n}\n',
  'src/lib.rs': 'pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n',
  'docs/guide.md': '# Guide\n\nStep one.\n',
  'assets/logo.png': '\u0089PNG',
  'data/huge.csv': 'x,y\n'.repeat(300_000),
  'node_modules/left-pad/index.js': 'module.exports = (s) => s;\n',
};
const GOOGLE = {
  DOC0000000: ['Project plan.md', 'text/markdown', '# Project plan\n\n- Ship it\n'],
  SHEET00000: ['Budget - Sheet1.csv', 'text/csv', 'item,cost\nrent,1000\n'],
  SLIDES0000: ['Kickoff.txt', 'text/plain', 'Kickoff\n\nGoals for the quarter\n'],
  FILE000000: ['notes.txt', 'text/plain', 'Notes kept on Drive.\n'],
};

function send(res, status, body = '', headers = {}) {
  res.writeHead(status, { 'content-length': Buffer.byteLength(body), ...headers });
  res.end(body);
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://x');
  const p = decodeURIComponent(url.pathname);
  const authed = !!req.headers.authorization;
  let m;
  if ((m = p.match(/^\/api\/repos\/octo\/(\w+)\/commits\/(.+)$/))) {
    if (m[1] === 'limited') return send(res, 403, '{}', { 'x-ratelimit-remaining': '0' });
    if (m[1] === 'private' && !authed) return send(res, 404, '{}');
    if (!['demo', 'private'].includes(m[1])) return send(res, 404, '{}');
    return ['HEAD', 'main', 'feature/x'].includes(m[2]) ? send(res, 200, SHA) : send(res, 422, '{}');
  }
  if ((m = p.match(/^\/api\/repos\/octo\/(demo|private)\/git\/trees\/(\w+)$/))) {
    const tree = Object.entries(FILES).map(([path, text]) => ({ path, type: 'blob', mode: '100644', size: Buffer.byteLength(text) }));
    return send(res, 200, JSON.stringify({ sha: SHA, truncated: false, tree }), { 'content-type': 'application/json' });
  }
  if ((m = p.match(/^\/raw\/octo\/(demo|private)\/(\w+)\/(.+)$/))) return FILES[m[3]] ? send(res, 200, FILES[m[3]]) : send(res, 404, '404: Not Found');
  const id = (p.match(/\/d\/(\w+)\//) || [])[1] || url.searchParams.get('id') || (p.match(/\/drive\/usercontent\/(\w+)/) || [])[1];
  if (id === 'PRIVATE000') return send(res, 302, '', { location: 'https://accounts.google.com/ServiceLogin?continue=x' });
  if (p === '/drive/uc' && GOOGLE[id]) return send(res, 303, '', { location: `/drive/usercontent/${id}` });
  if (GOOGLE[id] && (p.startsWith('/docs/') || p.startsWith('/drive/usercontent/'))) {
    const [name, type, text] = GOOGLE[id];
    return send(res, 200, text, { 'content-type': type, 'content-disposition': `attachment; filename="${name}"; filename*=UTF-8''${encodeURIComponent(name)}` });
  }
  send(res, 404, 'Not Found');
});

const port = Number(process.argv[2]) || 8790;
server.listen(port, '127.0.0.1', () => console.log(`GitHub and Google stand-in at http://127.0.0.1:${port}`));
