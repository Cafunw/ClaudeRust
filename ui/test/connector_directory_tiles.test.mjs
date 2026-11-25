// npm run test:ui — the connector directory's entries are told apart at a glance without shipping
// anyone's logo (ui/js/connector_directory.js): a monogram tile in one of eight colours per name,
// readable (WCAG AA) in the light and the dark theme (ui/css/app.css .cd-tile).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { monogram, tileColor, TILE_COLORS } from '../js/connector_directory.js';
import { contrast } from '../e2e/contrast.mjs';
import { source } from './source.mjs';

const DIRECTORY = JSON.parse(readFileSync(new URL('../../src-tauri/src/connector_directory.json', import.meta.url), 'utf8'));

test('a monogram: two words’ initials, a single word’s capitals, else its first two letters', () => {
  assert.equal(monogram('GitHub'), 'GH');
  assert.equal(monogram('Google Drive'), 'GD');
  assert.equal(monogram('monday.com'), 'MC');
  assert.equal(monogram('Linear'), 'Li');
  assert.equal(monogram('Context7'), 'Co');
  assert.equal(monogram('Çiçek'), 'Çi');
  assert.equal(monogram(''), '?');
});

test('every entry gets a tile, the same one every time, and the tiles of all entries differ', () => {
  const tiles = DIRECTORY.map((c) => `${monogram(c.name)}/${tileColor(c.name)}`);
  assert.deepEqual(DIRECTORY.map((c) => tileColor(c.name)), DIRECTORY.map((c) => tileColor(c.name)));
  assert.ok(DIRECTORY.every((c) => tileColor(c.name) >= 0 && tileColor(c.name) < TILE_COLORS));
  // every colour is used, and nearly no two entries look the same
  assert.equal(new Set(DIRECTORY.map((c) => tileColor(c.name))).size, TILE_COLORS);
  const same = tiles.length - new Set(tiles).size;
  assert.ok(same <= 2, `${same} entries share a tile with another`);
});

test('each tile colour is readable in both themes', () => {
  const CSS = source('../css/app.css');
  const hex = (v) => `rgb(${parseInt(v.slice(1, 3), 16)}, ${parseInt(v.slice(3, 5), 16)}, ${parseInt(v.slice(5, 7), 16)})`;
  for (const theme of ['', ':root[data-theme="dark"] ']) {
    for (let i = 0; i < TILE_COLORS; i++) {
      const m = new RegExp(`^${theme.replace(/[[\]()"]/g, '\\$&')}\\.cd-tile\\.t${i} \\{ background: (#[0-9a-f]{6}); color: (#[0-9a-f]{6}); \\}`, 'm').exec(CSS);
      assert.ok(m, `no ${theme}.cd-tile.t${i}`);
      const r = contrast(hex(m[2]), [hex(m[1])]);
      assert.ok(r >= 4.5, `${theme || 'light '}tile ${i}: ${r.toFixed(2)}`);
    }
  }
});
