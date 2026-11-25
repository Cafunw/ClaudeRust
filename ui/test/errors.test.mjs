// npm run test:ui — failures the backend names by a code, said in the UI's language (ui/js/errors.js, ui/js/neterr.js).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { ERROR_TEXT, codedMessage } from '../js/errors.js';
import { errorText, friendlyError } from '../js/neterr.js';
import { setLanguage, t } from '../js/i18n.js';
import { source } from './source.mjs';

const SRC = new URL('../../src-tauri/src/', import.meta.url);
const rust = readdirSync(SRC).filter((f) => f.endsWith('.rs')).map((f) => [f, source(new URL(f, SRC))]);
const i18n = source('../js/i18n.js');
const [enBlock, trBlock] = i18n.split(/\nconst tr = \{/);
const ALL = { path: 'C:/p', cmd: 'commit', s: 900, n: 2, name: 'x', branch: 'b', repo: 'C:/r', reason: 'r', shell: 'pwsh', line: 3, column: 4, method: 'initialize', tool: 'T', server: 'S', io: 'NotFound', resets_at: 4102444800000 };

test('every code the backend gives has a sentence in English and Turkish', () => {
  const codes = new Set();
  for (const [, src] of rust) for (const m of src.matchAll(/\bcoded(?:_notice)?\(\s*"(\w+)"/g)) codes.add(m[1]);
  assert.ok(codes.size > 50, `found ${codes.size} codes`);
  const missing = [...codes].filter((c) => !Object.hasOwn(ERROR_TEXT, c) && !(enBlock.includes(`errk_${c}:`) && trBlock.includes(`errk_${c}:`)));
  assert.deepEqual(missing, [], 'add these codes to ERROR_TEXT in ui/js/errors.js (or errk_ keys to both blocks of i18n.js)');
});

test('each sentence is filled and translated', () => {
  for (const kind of Object.keys(ERROR_TEXT)) {
    setLanguage('en');
    const en = codedMessage({ kind, params: ALL });
    setLanguage('tr');
    const tr = codedMessage({ kind, params: ALL });
    for (const s of [en, tr]) assert.ok(s && !/\{\w+\}|undefined|NaN/.test(s), `${kind}: ${s}`);
    assert.notEqual(en, tr, kind);
  }
  setLanguage('en');
});

test('no Rust file says more failures in plain English than it did', () => {
  const allowed = JSON.parse(readFileSync(new URL('./errors_uncoded.json', import.meta.url), 'utf8'));
  const over = [];
  for (const [f, src] of rust) {
    if (f === 'error.rs') continue;
    const n = (src.match(/AppError::(?:msg|Msg)\(/g) || []).length;
    if (n > (allowed[f] || 0)) over.push(`${f}: ${n} AppError::msg (allowed ${allowed[f] || 0})`);
  }
  assert.deepEqual(over, [], 'give the new failures a code (AppError::coded and ui/js/errors.js); see ui/test/errors_uncoded.json');
});

test('a coded failure reads in the UI language, its English text the detail', () => {
  setLanguage('tr');
  const e = { message: 'Only web links can be opened.', kind: 'link_not_web', status: 0 };
  assert.equal(friendlyError(e), 'Yalnızca web bağlantıları açılabilir.');
  assert.equal(errorText({ kind: 'folder_not_found', params: { path: 'D:/x' }, message: 'Folder not found: D:/x' }), 'Klasör bulunamadı: D:/x');
  // a hook's notice, an I/O failure by its kind, the memory's own codes
  assert.equal(errorText({ kind: 'worktree_branch_kept', params: { branch: 'cr/1', n: 1 }, message: '' }), 'cr/1 dalı korundu: kendine ait 1 commit’i var.');
  assert.equal(codedMessage({ kind: 'io', params: { io: 'PermissionDenied' }, message: 'i/o error: Erişim engellendi. (os error 5)' }), 'Erişim reddedildi.');
  assert.equal(codedMessage({ kind: 'io', params: { io: 'Other' } }), 'Bir dosya okunamadı ya da yazılamadı.');
  assert.equal(codedMessage({ kind: 'memory_busy', message: 'x' }), t('errk_memory_busy'));
  // not coded: transport text as before, then the text itself
  assert.equal(friendlyError({ kind: 'network', message: 'network error: error sending request for url (http://h/)' }), t('neterr_connect'));
  assert.equal(friendlyError({ kind: 'error', message: 'Something else' }), null);
  assert.equal(errorText({ kind: 'error', message: 'Something else' }), 'Something else');
  assert.equal(errorText('plain text'), 'plain text');
  assert.equal(codedMessage({ kind: 'constructor', message: 'x' }), null, 'only the table’s own keys');
  // params lost on the way: the backend's own text, not a half-filled sentence
  assert.equal(codedMessage({ kind: 'folder_not_found', message: 'Folder not found: D:/x' }), null);
  assert.equal(codedMessage({ kind: 'worktree_branch_kept', message: 'Kept branch b' }), null);
  assert.equal(errorText({ kind: 'folder_not_found', message: 'Folder not found: D:/x' }), 'Folder not found: D:/x');
  setLanguage('en');
});

test('numbers and times read right in both languages', () => {
  setLanguage('en');
  assert.equal(codedMessage({ kind: 'worktree_branch_kept', params: { branch: 'b', n: 1 } }), 'Kept branch b: it has 1 commit of its own.');
  assert.equal(codedMessage({ kind: 'worktree_branch_kept', params: { branch: 'b', n: 3 } }), 'Kept branch b: it has 3 commits of its own.');
  assert.equal(codedMessage({ kind: 'git_timeout', params: { cmd: 'push', s: 900 } }), 'git push took longer than 15 minutes and was stopped.');
  assert.equal(codedMessage({ kind: 'git_timeout', params: { cmd: 'status', s: 60 } }), 'git status took longer than 60 seconds and was stopped.');
  assert.equal(codedMessage({ kind: 'routine_too_often', params: { n: 1 } }), 'Run at most every 1 minute.');
  assert.equal(codedMessage({ kind: 'cli_usage_limit' }), 'You’ve reached your Claude plan’s usage limit. Try again once it resets.');
  assert.match(codedMessage({ kind: 'cli_usage_limit', params: { resets_at: 4102444800000 } }), /It resets \w{3}/);
  setLanguage('tr');
  assert.equal(codedMessage({ kind: 'git_timeout', params: { cmd: 'push', s: 900 } }), 'git push 15 dakika içinde bitmedi ve durduruldu.');
  assert.match(codedMessage({ kind: 'cli_usage_limit', params: { resets_at: 4102444800000 } }), /Sınır .+ itibarıyla sıfırlanır\.$/);
  assert.equal(codedMessage({ kind: 'cli_not_signed_in', message: 'You’re not signed in…' }), 'Claude’da oturum açık değil. Ayarlar → Hesap bölümünden Claude hesabınızla oturum açın (veya bir API anahtarı ekleyin).');
  setLanguage('en');
});

test('a program the system cannot find, in its own language (os error 2 or 3)', () => {
  setLanguage('tr');
  assert.equal(friendlyError('Could not start `C:/nope/notthere.exe`: Sistem belirtilen yolu bulamıyor. (os error 3)'), t('neterr_no_program', { cmd: 'C:/nope/notthere.exe' }));
  // the server card's coded form (mcp.rs): not found says so, any other start failure is the general sentence
  const start = (io) => friendlyError({ kind: 'mcp_start_failed', params: { cmd: 'C:/nope/notthere.exe', io }, message: 'Could not start …' });
  assert.equal(start('NotFound'), t('neterr_no_program', { cmd: 'C:/nope/notthere.exe' }));
  assert.match(start('PermissionDenied'), /^“C:\/nope\/notthere\.exe” başlatılamadı\. Yapılandırmadaki/);
  setLanguage('en');
});
