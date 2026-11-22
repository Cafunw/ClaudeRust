// npm run test:ui — the pure parts of the extension preview and the extension directory.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { installedExtension, missingRequired, signatureLine } from '../js/extensions_page.js';
import { errorText } from '../js/neterr.js';
import { filterExtensions, entryAction, localizeEntry } from '../js/extension_directory.js';
import { setLanguage, t } from '../js/i18n.js';

test('an extension whose server could not start without a runtime says which', () => {
  const status = {
    config: { mcpServers: { a: { extension: { path: '/x', version: '1' } }, b: { extension: { path: '/y', version: '2' } } } },
    servers: [{ name: 'a', status: 'failed', runtime: 'node' }, { name: 'b', status: 'connected', runtime: null }],
  };
  assert.deepEqual(installedExtension(status, 'a'), { name: 'a', version: '1', display_name: undefined, runtime: 'node' });
  assert.equal('runtime' in installedExtension(status, 'b'), false);
});

test('a required setting left empty is named, unless it has a default or a saved secret', () => {
  const spec = { key: { title: 'API key', required: true, sensitive: true }, dir: { required: true, default: '/a' }, opt: { title: 'Optional' } };
  assert.equal(missingRequired(spec, {}), 'API key');
  assert.equal(missingRequired(spec, { key: 'k' }), null);
  assert.equal(missingRequired(spec, {}, ['key']), null, 'a saved secret stays');
  assert.equal(missingRequired({ list: { required: true, multiple: true } }, { list: [] }), 'list');
  assert.equal(missingRequired(undefined, {}), null);
});

test('only a trusted chain installs without an explicit yes, and a broken signature never', () => {
  setLanguage('en');
  const line = (status, publisher = 'Acme') => signatureLine({ status, publisher });
  assert.deepEqual([line('trusted').cls, line('trusted').confirm], ['ok', false]);
  assert.match(line('trusted').text, /Signed by Acme/);
  for (const s of ['untrusted', 'self_signed', 'unverifiable', 'unsigned']) assert.equal(line(s).confirm, true, s);
  assert.equal(line('invalid').cls, 'error');
  assert.match(line('self_signed').text, /Self-signed by Acme/);
  assert.equal(line('unverifiable', null).text, t('ext_sig_unverifiable_anon'));
  assert.equal(signatureLine(undefined).text, t('ext_sig_unsigned'));
  for (const s of ['untrusted', 'self_signed', 'unverifiable', 'unsigned']) assert.doesNotMatch(line(s).text, /trusted for code signing/, `${s} never claims a verification it did not make`);
});

test('failures read in the UI language by their code, else as given', () => {
  setLanguage('tr');
  assert.equal(errorText({ kind: 'ed_checksum', message: 'The download from … doesn’t have the SHA-256' }), t('errk_ed_checksum'));
  assert.match(errorText({ kind: 'needs_node', message: 'x' }), /Node\.js gerektiriyor/);
  assert.match(errorText({ kind: 'download_redirect', message: 'x' }), /yönlendirildi/);
  assert.equal(errorText({ kind: 'error', message: 'Something else' }), 'Something else');
  setLanguage('en');
});

test('the directory filters by words and category and knows what each button does', () => {
  setLanguage('en');
  const list = [
    { id: 'a', name: 'Apify', publisher: 'Apify', description: { en: 'Scrape websites', tr: 'Web sitelerini kazıyın' }, category: 'web', asset: { size: 1 }, installed: null, update: false },
    { id: 'k', name: 'Kubernetes', publisher: 'containers', description: { en: 'Clusters' }, category: 'cloud', asset: null, installed: null },
    { id: 'd', name: 'Desktop Commander', publisher: 'DC', description: { en: 'Terminal' }, category: 'dev', asset: { size: 1 }, installed: { name: 'desktop-commander', version: '0.3.3' }, update: true },
    { id: 'p', name: 'Pinot', publisher: 'StarTree', description: { en: 'SQL' }, category: 'data', asset: { size: 1 }, installed: { name: 'mcp-pinot', version: '5.0.0' }, update: false },
  ].map((e) => localizeEntry(e, 'tr'));
  assert.equal(list[0].description, 'Web sitelerini kazıyın');
  assert.equal(list[1].description, 'Clusters', 'English when there is no Turkish');
  assert.equal(list[2].categoryLabel, t('ed_cat_dev'));
  assert.deepEqual(filterExtensions(list, 'startree sql').map((e) => e.id), ['p']);
  assert.deepEqual(filterExtensions(list, '', 'cloud').map((e) => e.id), ['k']);
  assert.deepEqual(filterExtensions(list, 'nothing like it').map((e) => e.id), []);
  assert.deepEqual(list.map(entryAction), ['install', 'none', 'update', 'installed']);
});
