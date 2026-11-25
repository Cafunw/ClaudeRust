// npm run test:ui — a card's header in Settings (an MCP server, a skill, a directory entry) never
// runs out of its card in a narrow window: its name and line are cut short with the whole text
// in a tooltip, and below about 480 px its badge and buttons wrap under the name.
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';

test('a server card cuts its name and its line short, with all of it in a tooltip', () => {
  const s = source('../js/settings.js');
  const card = s.slice(s.indexOf('function serverCard('), s.indexOf('function editConfig('));
  assert.match(card, /<div class="ellipsis" style="font-weight:600" title="\$\{esc\(s\.name\)\}">\$\{esc\(s\.name\)\}<\/div>/);
  assert.match(card, /<div class="small muted ellipsis" title="\$\{sub\}">\$\{sub\}<\/div>/);
});

test('a narrow card wraps its badge and buttons under the name, and its title block can shrink', () => {
  const css = source('../css/app.css');
  assert.match(css, /\.server \{[^}]*container-type: inline-size;/);
  assert.match(css, /\.server \.head > \.grow \{ min-width: 0; \}/);
  const narrow = css.slice(css.indexOf('@container (max-width: 480px) {'), css.indexOf('}\n', css.indexOf('margin-left: auto; }', css.indexOf('@container (max-width: 480px) {'))) + 1);
  assert.match(narrow, /\.server \.head \{ flex-wrap: wrap; row-gap: 6px; \}/);
  assert.match(narrow, /\.server \.head > \.grow \{ flex: 1 1 calc\(100% - 44px\); \}/);
  assert.match(css, /\.server \.st \{[^}]*white-space: nowrap;/, 'a badge never breaks in two');
});
