// npm run test:ui — the Settings nav: one tab stop (a tablist, the arrows move and show each
// page, Tab goes into the page), and its names wrap instead of being cut at any width.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { menuStep } from '../js/util.js';
import { source } from './source.mjs';

const SRC = source('../js/settings.js');
const CSS = source('../css/app.css');
const open = SRC.slice(SRC.indexOf('export function openSettings('), SRC.indexOf('function save(patch)'));

test('the nav is a tablist with one tab stop on the page shown', () => {
  assert.match(open, /<nav class="settings-nav" role="tablist" aria-orientation="vertical"><\/nav><div class="settings-body" role="tabpanel">/);
  assert.match(open, /<button class="nav-item" role="tab" id="st-tab-\$\{id\}" aria-selected="false" tabindex="-1" data-tab="\$\{id\}"/);
  const show = open.slice(open.indexOf('function show(id)'), open.indexOf('// redraw for changes made elsewhere'));
  assert.match(show, /b\.setAttribute\('aria-selected', String\(on\)\);\s*\n\s*b\.tabIndex = on \? 0 : -1;/);
  assert.match(show, /if \(on && inNav\) b\.focus\(\{ preventScroll: true \}\);/, 'the keyboard moves with the page shown');
  assert.match(show, /body\.setAttribute\('aria-labelledby', `st-tab-\$\{id\}`\);/);
});

test('the arrows, Home and End move along the tabs and show each page', () => {
  const keys = open.slice(open.indexOf("nav.addEventListener('keydown'"), open.indexOf('// what the shown tab subscribed to'));
  assert.match(keys, /const to = menuStep\(tabs\.length, tabs\.indexOf\(document\.activeElement\), e\.key\);/);
  assert.match(keys, /show\(tabs\[to\]\.dataset\.tab\);/);
  // 12 pages: the steps the nav takes
  assert.equal(menuStep(12, 0, 'ArrowDown'), 1);
  assert.equal(menuStep(12, 11, 'ArrowDown'), 0);
  assert.equal(menuStep(12, 0, 'ArrowUp'), 11);
  assert.equal(menuStep(12, 5, 'Home'), 0);
  assert.equal(menuStep(12, 5, 'End'), 11);
  assert.equal(menuStep(12, 5, 'Tab'), -1, 'Tab is the browser’s: on into the page');
});

test('a long page name wraps in a narrow nav instead of being cut', () => {
  const rule = CSS.match(/\n\.settings-nav \.nav-item \{([^}]*)\}/)[1];
  assert.match(rule, /white-space: normal/);
  assert.match(rule, /height: auto; min-height: 32px/);
  assert.match(CSS, /\n\.settings-nav \.nav-item \.label \{ overflow-wrap: anywhere; \}/);
  // below 560 px the names give way to the icons (each keeps its name as its title)
  assert.match(CSS, /@media \(max-width: 560px\) \{\n {2}\.settings-nav \{ width: 52px;[^}]*\}\n {2}\.settings-nav \.nav-item \{ justify-content: center; padding: 0; \}\n {2}\.settings-nav \.nav-item \.label \{ display: none; \}/);
  assert.match(open, /title="\$\{esc\(t\(label\)\)\}" aria-label="\$\{esc\(t\(label\)\)\}"/);
});
