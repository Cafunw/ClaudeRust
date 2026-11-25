// npm run test:ui — the first-run screen puts focus on its first field (the
// name), or on "Continue as …" once a signed-in account is found if the user
// hasn't started typing; never on the API-key field below them.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const src = readFileSync(new URL('../js/onboarding.js', import.meta.url), 'utf8');

test('focus starts on the name field, before the account check', () => {
  const at = src.indexOf('nameIn.focus();');
  assert.ok(at > 0 && at < src.indexOf('accountStatus().then'), 'focused at once, not after the account check');
  assert.doesNotMatch(src, /\$\('\[data-f=key\]', view\)\.focus\(\)/, 'the key field is focused only when the key is missing');
  assert.match(src, /keyIn\.setAttribute\('aria-invalid', 'true'\);[\s\S]{0,200}keyIn\.focus\(\);/);
});

test('a signed-in account takes focus unless the user is typing; Enter in the name follows it', () => {
  assert.match(src, /nameIn\.addEventListener\('input', \(\) => \{ typed = true; \}, \{ once: true \}\);/);
  assert.match(src, /acct\.appendChild\(b\);\s*primary = b;\s*if \(document\.activeElement === nameIn && !typed\) b\.focus\(\);/);
  assert.match(src, /if \(i === nameIn && primary\) primary\.click\(\); else submit\(\);/);
});
