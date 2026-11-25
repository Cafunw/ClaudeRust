// npm run test:ui — the layout in narrow windows: the header's title keeps
// its room before the crumbs, the Code home's mark and worktree card, the
// Sessions page head and the composer's note.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';

const read = source;
const css = read('../css/app.css');
// the declarations of the rule whose selector list is exactly `sel` (the last one wins, as in CSS)
const rule = (sel) => {
  const all = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter((m) => m[1].trim().endsWith(sel) && m[1].trim().split('\n').pop().trim() === sel);
  return all.length ? all[all.length - 1][2] : null;
};

test('the session title keeps its room; crumbs and the watch badge give way first', () => {
  const give = rule('.topbar .crumb, .topbar .active-tool.wt');
  assert.ok(give && /flex-shrink:\s*100/.test(give) && /min-width:\s*28px/.test(give), 'crumbs shrink a hundred times faster, down to their icon');
  assert.doesNotMatch(css, /\.topbar \.crumb \{[^}]*min-width:\s*64px/, 'no crumb keeps 64 px ahead of the title');
  assert.match(css, /@container \(max-width: 640px\) \{\s*\n\s*\.topbar \.watch-badge > span:not\(\.wb-ok\):not\(\.wb-err\) \{ display: none; \}/);
  assert.match(css, /@container \(max-width: 400px\) \{[^}]*\.topbar \.ctx-meter \.pct \{ display: none; \}/);
  const title = rule('.topbar .title-btn');
  assert.ok(title && /flex-shrink:\s*1\b/.test(title), 'the title shrinks last');
});

test('the Code home mark heads the stage, the worktree hint sits under its label', () => {
  const code = read('../js/code.js');
  assert.match(code, /<div class="code-stage"><span class="env-mark">\$\{spark\(24\)\}<\/span>\s*\n\s*<div class="env-card"><div class="env-list">/);
  const mark = rule('.env-mark');
  assert.ok(mark && !/position:\s*absolute/.test(mark), 'no longer pinned over the card\'s border');
  const wt = rule('.env-card.env-wt');
  assert.ok(wt && !/flex-direction:\s*row/.test(wt), 'label and hint stacked, not two narrow columns');
  assert.ok(!/white-space:\s*nowrap/.test(rule('.env-wt span') || ''), 'the hint wraps in a narrow window instead of being cut');
});

test('a narrow Sessions page keeps its head on one row (Routines and Import as icons, still named)', () => {
  const code = read('../js/code.js');
  for (const act of ['routines', 'import']) assert.match(code, new RegExp(`class="btn sess-sec" data-act="${act}" title="[^"]+" aria-label="[^"]+">\\$\\{icon\\([^)]*\\)\\}<span class="lbl">`));
  assert.match(css, /@media \(max-width: 600px\) \{ \.page-head \.sess-sec \.lbl \{ display: none; \} \}/);
});

test('a narrow chat keeps a gap under the composer\'s note', () => {
  // (the block of rules, not a one-line query before it: the slice ends at the block's own close)
  const narrow = css.slice(css.indexOf('@media (max-width: 760px) {\n'));
  assert.match(narrow.slice(0, narrow.indexOf('}\n}') + 3), /\.chat-bottom \{ padding: 0 12px 6px; \}/);
});
