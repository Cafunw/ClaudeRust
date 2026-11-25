// npm run test:ui — the chat says what went wrong in the app's language: an
// API error's own (English) words stay under "Details", and the few words
// the chat UI wrote in English are translated.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setLanguage, t } from '../js/i18n.js';
import { errorMessage, artifactKindText } from '../js/parts.js';

test('a server error is said in Turkish, with its HTTP code', () => {
  setLanguage('tr');
  assert.equal(errorMessage({ kind: 'api_error', status: 500, message: 'Internal server error' }), t('neterr_server', { code: 500 }));
  assert.equal(errorMessage({ kind: 'api_error', message: 'Internal server error' }), t('neterr_server', { code: 500 }), 'no status: still the server');
  assert.equal(errorMessage({ status: 503, message: 'upstream down' }), t('neterr_server', { code: 503 }));
  assert.equal(errorMessage({ kind: 'invalid_request_error', status: 400, message: 'prompt is too long' }), t('neterr_http', { code: 400 }));
  assert.doesNotMatch(errorMessage({ kind: 'api_error', status: 500, message: 'Internal server error' }), /Internal server error/);
});

test('the errors that had their own words keep them', () => {
  setLanguage('tr');
  assert.equal(errorMessage({ status: 401, message: 'x' }), t('err_auth'));
  assert.equal(errorMessage({ status: 529, message: 'Overloaded' }), t('err_overloaded'));
  assert.equal(errorMessage({ kind: 'network', message: 'boom' }), t('err_network'));
  // an app-side message with no status is shown as it is (it's already the app's)
  assert.equal(errorMessage({ message: 'Bir şey oldu' }), 'Bir şey oldu');
  assert.equal(errorMessage(null), '');
  setLanguage('en');
});

test('an artifact\'s kind is said in the app\'s language; names stay names', () => {
  setLanguage('tr');
  assert.equal(artifactKindText('Diagram'), 'Diyagram');
  assert.equal(artifactKindText('Document'), 'Belge');
  assert.equal(artifactKindText('Code'), 'Kod');
  assert.equal(artifactKindText('HTML'), 'HTML');
  assert.equal(artifactKindText('Python'), 'Python');
  setLanguage('en');
  assert.equal(artifactKindText('Diagram'), 'Diagram');
});

test('no English left in the chat UI\'s user card and copy fallback', () => {
  for (const [file, english] of [['sidebar.js', "|| 'You'"], ['parts.js', "toast('Copy failed'"]]) {
    const src = readFileSync(new URL(`../js/${file}`, import.meta.url), 'utf8');
    assert.ok(!src.includes(english), `${file}: ${english}`);
  }
  for (const lang of ['en', 'tr']) {
    setLanguage(lang);
    for (const k of ['you', 'copy_failed', 'chat_loading', 'art_back', 'img_view', 'art_csp_blocked', 'find_prev', 'find_next']) assert.notEqual(t(k), k, `${lang}: ${k}`);
  }
  setLanguage('en');
});
