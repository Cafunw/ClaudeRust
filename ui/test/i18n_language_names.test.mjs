// npm run test:ui — the language picker's names.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, LANGUAGES } from '../js/i18n.js';

test('the "Auto" language is named in the app’s language', () => {
  setLanguage('tr');
  assert.equal(LANGUAGES.find((l) => l.id === 'auto').name, 'Otomatik (sistem)');
  setLanguage('en');
  assert.equal(LANGUAGES.find((l) => l.id === 'auto').name, 'Auto (system)');
});
