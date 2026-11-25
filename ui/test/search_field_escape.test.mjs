// npm run test:ui — Esc in a search field with text in it empties the field (and the list it filters)
// before it closes the dialog the field is in (Settings → Skills, the connector and extension
// directories); in an empty one it closes the dialog as before.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';
import { clearsOnEscape } from '../js/a11y.js';

test('only a search field with text in it is emptied by Esc', () => {
  assert.ok(clearsOnEscape({ tagName: 'INPUT', type: 'search', value: 'pdf' }));
  assert.ok(!clearsOnEscape({ tagName: 'INPUT', type: 'search', value: '' }), 'empty: Esc closes the dialog');
  assert.ok(!clearsOnEscape({ tagName: 'INPUT', type: 'text', value: 'pdf' }), 'another field keeps its own Esc');
  assert.ok(!clearsOnEscape({ tagName: 'TEXTAREA', type: 'textarea', value: 'x' }));
  assert.ok(!clearsOnEscape(null));
});

test('it runs before a dialog sees the key, and tells the list', () => {
  const s = source('../js/a11y.js');
  // a window listener in the capture phase comes before the dialog's document one
  assert.match(s, /window\.addEventListener\('keydown', \(e\) => \{\n\s*if \(e\.key !== 'Escape' \|\| !clearsOnEscape\(e\.target\) \|\| document\.querySelector\('\.popover'\)\) return;\n\s*e\.preventDefault\(\);\n\s*e\.stopPropagation\(\);\n\s*e\.target\.value = '';\n\s*e\.target\.dispatchEvent\(new Event\('input', \{ bubbles: true \}\)\);\n\s*\}, true\);/);
  for (const f of ['../js/connector_directory.js', '../js/extension_directory.js', '../js/skills_page.js']) assert.match(source(f), /type="search"/, f);
});
