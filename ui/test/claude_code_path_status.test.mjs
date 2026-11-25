// npm run test:ui — Settings → Code: the status under the Claude Code path is looked up again when
// the path changes, for that path, and says what is (or isn't) there.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { cliProblem } from '../js/settings_values.js';
import { setLanguage, t } from '../js/i18n.js';
import { source } from './source.mjs';

test('the status names the path that was typed', () => {
  assert.equal(cliProblem('', { found: true, version: '2.1.326', path: 'C:/x/claude.exe' }), null);
  assert.equal(cliProblem('C:/x/claude.exe', { found: true, version: '2.1.326' }), null);
  assert.deepEqual(cliProblem('', { found: false }), { key: 'cc_missing' }, 'detected: not installed');
  assert.deepEqual(cliProblem('C:/nope/claude.exe', { found: false }), { key: 'cc_not_at', path: 'C:/nope/claude.exe' });
  assert.deepEqual(cliProblem('C:/Windows/notepad.exe', { found: true, version: '' }), { key: 'cc_not_cli', path: 'C:/Windows/notepad.exe' }, 'something that gives no version');
  assert.deepEqual(cliProblem('C:/x', null), { key: 'cc_not_at', path: 'C:/x' }, 'the look failed');
  for (const lang of ['en', 'tr']) {
    setLanguage(lang);
    assert.ok(t('cc_not_at', { path: 'C:/nope' }).includes('C:/nope'), lang);
    assert.ok(t('cc_not_cli', { path: 'C:/nope' }).includes('C:/nope'), lang);
  }
  setLanguage('en');
});

test('a changed path is detected again, for what the field says, and a slower earlier look is dropped', () => {
  const src = source('../js/settings.js');
  const code = src.slice(src.indexOf('  async code(body) {'), src.indexOf('  import(body) {'));
  assert.match(code, /const want = pathIn\.value\.trim\(\);/);
  assert.match(code, /call\('claude_code_info', \{ path: want \}\)/, 'not the saved setting, which may not be saved yet');
  assert.match(code, /if \(pathIn\.value\.trim\(\) !== want\) return;/);
  assert.match(code, /pathIn\.addEventListener\('change', drawStatus\);/);
  // the backend looks at the path it is given
  const rs = source('../../src-tauri/src/commands.rs');
  assert.match(rs, /pub async fn claude_code_info\(state: St<'_>, path: Option<String>\)/);
});

test('a long path in the status wraps instead of widening the page', () => {
  const src = source('../js/settings.js');
  const code = src.slice(src.indexOf('  async code(body) {'), src.indexOf('  import(body) {'));
  assert.equal((code.match(/<span class="small" style="[^"]*min-width:0;overflow-wrap:anywhere">/g) || []).length, 2, 'what is found and what is wrong both wrap');
});
