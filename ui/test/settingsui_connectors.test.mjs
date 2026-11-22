// npm run test:ui — connector directory search and marks, the saved-token
// rules of the custom connector form, the missing-command check.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { filterDirectory, freeName, localize } from '../js/connector_directory.js';
import { connectorEntry } from '../js/mcp_connector.js';
import { notRegistered } from '../js/optional_cmd.js';
import { setLanguage } from '../js/i18n.js';

// as connector_directory.json has them
const raw = [
  { name: 'Linear', description: { en: 'Issues and projects', tr: 'Konular ve projeler' }, category: 'projects', domain: 'mcp.linear.app', url: 'https://mcp.linear.app/mcp', auth: 'oauth' },
  { name: 'Sentry', description: { en: 'Error monitoring', tr: 'Hata izleme' }, category: 'dev', domain: 'mcp.sentry.dev', url: 'https://mcp.sentry.dev/mcp', auth: 'oauth' },
  { name: 'Context7', description: { en: 'Library docs' }, category: 'dev', domain: 'mcp.context7.com', url: 'https://mcp.context7.com/mcp', auth: 'none' },
];

test('entries show the description and category in the UI language, English when one is missing', () => {
  setLanguage('tr');
  const tr = raw.map((c) => localize(c, 'tr'));
  assert.deepEqual(tr.map((c) => [c.description, c.categoryLabel]), [['Konular ve projeler', 'Proje yönetimi'], ['Hata izleme', 'Geliştirici araçları'], ['Library docs', 'Geliştirici araçları']]);
  assert.deepEqual(filterDirectory(tr, 'hata').map((c) => c.name), ['Sentry']);
  setLanguage('en');
  assert.equal(localize({ name: 'x', description: 'plain' }, 'en').description, 'plain');
  assert.equal(localize({ name: 'x' }, 'en').description, '');
});

test('the directory search matches every word in name, description, category or domain', () => {
  setLanguage('en');
  const dir = raw.map((c) => localize(c, 'en'));
  assert.deepEqual(filterDirectory(dir, '').map((c) => c.name), ['Linear', 'Sentry', 'Context7']);
  assert.deepEqual(filterDirectory(dir, '  DEVELOPER ').map((c) => c.name), ['Sentry', 'Context7']);
  assert.deepEqual(filterDirectory(dir, 'developer docs').map((c) => c.name), ['Context7']);
  assert.deepEqual(filterDirectory(dir, 'linear.app').map((c) => c.name), ['Linear']);
  assert.deepEqual(filterDirectory(dir, 'nothing-like-it'), []);
  assert.deepEqual(filterDirectory([{ name: 'Bare', url: 'https://x' }], 'bare').length, 1);
  // a picked category, with or without words
  assert.deepEqual(filterDirectory(dir, '', 'dev').map((c) => c.name), ['Sentry', 'Context7']);
  assert.deepEqual(filterDirectory(dir, 'docs', 'dev').map((c) => c.name), ['Context7']);
  assert.deepEqual(filterDirectory(dir, 'linear', 'dev'), []);
});

test('an added entry gets a name no configured server has', () => {
  assert.equal(freeName(raw[0], { config: { mcpServers: { Linear: {}, 'Linear 2': {} } } }), 'Linear 3');
  assert.equal(freeName(raw[1], { config: { mcpServers: { Linear: {} } } }), 'Sentry');
  assert.equal(freeName(raw[1], {}), 'Sentry');
});

test('a saved token is kept unless a new one is typed or it is removed', () => {
  const old = { url: 'https://a/mcp', headers: { Authorization: 'Bearer keyring:ref', 'X-Team': 'red' } };
  assert.deepEqual(connectorEntry({ url: 'https://a/mcp', token: '', keepToken: true }, old).headers, old.headers);
  assert.deepEqual(connectorEntry({ url: 'https://a/mcp', token: 'new', keepToken: true }, old).headers, { 'X-Team': 'red', Authorization: 'Bearer new' });
  assert.deepEqual(connectorEntry({ url: 'https://a/mcp', token: '', keepToken: false }, old).headers, { 'X-Team': 'red' });
  assert.equal(connectorEntry({ url: 'https://a/mcp', token: '', keepToken: true }, {}).headers, undefined);
});

test('only Tauri’s "not registered" rejection counts as a missing command', () => {
  assert.equal(notRegistered(new Error('Command skills_list not found')), true);
  assert.equal(notRegistered('Command memory_list not found'), true);
  assert.equal(notRegistered(new Error('Skill not found')), false);
  assert.equal(notRegistered(new Error('Command x not found: and more')), false);
  assert.equal(notRegistered(null), false);
});
