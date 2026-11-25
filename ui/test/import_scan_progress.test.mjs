// npm run test:ui — Settings → Import shows each source as soon as it is found, and the Claude
// Code sessions (each file is read) with a count that grows as they are read.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, t } from '../js/i18n.js';
import { source } from './source.mjs';

const SRC = source('../js/importer.js');
const render = SRC.slice(SRC.indexOf('export async function renderImport('), SRC.indexOf('function ccSessions('));

test('the quick sources are drawn before the sessions are read', () => {
  const quick = render.indexOf("await call('import_scan')");
  const drawn = render.indexOf('body.append(ccSec, desktopConnectors(scan.desktop), serverImport(');
  const slow = render.indexOf("await call('import_scan_sessions')");
  assert.ok(quick > 0 && drawn > quick && slow > drawn, 'import_scan, then the sections, then the sessions');
  assert.match(render, /onBackend\('import-scan-progress', \(p\) => \{\s*\n\s*if \(p && p\.total\) reading\.lastChild\.textContent = t\('imp_cc_reading', \{ done: p\.done, total: p\.total \}\);/);
  assert.match(render, /\} catch \(e\) \{ reading\.textContent = errorText\(e\); \} finally \{ unlisten\(\); \}/, 'a failed read says why, and the listener goes');
  assert.match(render, /<span class="spinner"><\/span>/, 'a wait shows it is working');
});

test('the backend splits the scan, and says how far the sessions are', () => {
  const rs = source('../../src-tauri/src/import.rs');
  const quick = rs.slice(rs.indexOf('pub async fn import_scan('), rs.indexOf('pub async fn import_scan_sessions('));
  assert.doesNotMatch(quick, /scan_cc_session/, 'the quick part reads no session file');
  assert.match(rs, /app\.emit\("import-scan-progress", json!\(\{"done": done, "total": listed\.len\(\)\}\)\)/);
  assert.match(source('../../src-tauri/src/lib.rs'), /import::import_scan_sessions,/);
  for (const lang of ['en', 'tr']) {
    setLanguage(lang);
    assert.ok(t('imp_cc_reading', { done: 20, total: 500 }).includes('20/500'), lang);
  }
  setLanguage('en');
});
