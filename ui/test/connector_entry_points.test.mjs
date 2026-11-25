// npm run test:ui — a way to connectors from the chat (the composer's tools menu: "Add connectors…",
// and "Manage connectors" once there are some), and Settings → Developer led by the ways to add a
// server, with the rest of what it does under ⋯.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';
import { setLanguage, t } from '../js/i18n.js';
import { connectorItems } from '../js/composer.js';

const add = () => 'add';
const manage = () => 'manage';

test('the tools menu offers to add connectors, and to manage them once there are some', () => {
  setLanguage('en');
  const none = connectorItems(false, [], add, manage);
  assert.deepEqual(none.map((i) => i.label || (i.sep && '—')), ['—', 'Add connectors…']);
  assert.equal(none[1].onClick, add);
  const some = connectorItems(false, [{ name: 'evalmcp', status: 'failed' }], add, manage);
  assert.deepEqual(some.map((i) => i.label || (i.sep && '—')), ['—', 'Add connectors…', 'Manage connectors']);
  assert.equal(some[2].onClick, manage);
  setLanguage('tr');
  assert.equal(connectorItems(false, [], add, manage)[1].label, 'Bağlayıcı ekle…');
  setLanguage('en');
});

test('a Claude Code session, whose CLI has its own servers, has no such entry', () => {
  assert.deepEqual(connectorItems(true, [{ name: 'x' }], add, manage), []);
});

test('a menu draws a line only between groups (the tools menu of a Code session with no servers had two in a row)', () => {
  const util = source('../js/util.js');
  assert.match(util, /if \(it\.sep\) \{ if \(el\.lastElementChild && !el\.lastElementChild\.matches\('\.menu-sep'\)\) el\.appendChild/);
  assert.match(util, /if \(el\.lastElementChild && el\.lastElementChild\.matches\('\.menu-sep'\)\) el\.lastElementChild\.remove\(\);\n\s+document\.body\.appendChild\(el\);/);
});

test('the tools menu puts them after the servers, the directory opening from the chat', () => {
  const s = source('../js/composer.js');
  assert.match(s, /items\.push\(\.\.\.connectorItems\(ccCode, store\.mcpServers, addConnectors, \(\) => emit\('open-settings', 'developer'\)\)\);/);
  assert.match(s, /await openDirectory\(await call\('mcp_status'\)/);
});

test('Settings → Developer leads with three ways to add a server; the rest is under ⋯', () => {
  const s = source('../js/settings.js');
  const page = s.slice(s.indexOf('  async developer(body) {'), s.indexOf('  async code(body) {'));
  const lead = page.slice(page.indexOf('actions.append('), page.indexOf('const more'));
  assert.deepEqual([...lead.matchAll(/button\(t\('(\w+)'\), '(\w*)'/g)].map((m) => `${m[1]}${m[2] ? ':' + m[2] : ''}`), ['cd_title:primary', 'cc_title', 'ed_title']);
  assert.match(page, /aria-haspopup="menu" title="\$\{esc\(t\('more_actions'\)\)\}" aria-label="\$\{esc\(t\('more_actions'\)\)\}"/);
  const menu = page.slice(page.indexOf('more.onclick'), page.indexOf('actions.appendChild(more)'));
  assert.deepEqual([...menu.matchAll(/label: t\('(\w+)'\)/g)].map((m) => m[1]), ['install_extension', 'edit_config', 'open_file', 'reload', 'import_claude']);
  assert.doesNotMatch(page, /actions\.appendChild\(button\(/, 'no more rows of equal buttons');
});
