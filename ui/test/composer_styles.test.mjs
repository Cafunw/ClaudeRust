// npm run test:ui — the response styles offered in the composer.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, t } from '../js/i18n.js';

test('the Learning style is offered next to the others, labelled in both languages', async () => {
  const { PRESET_STYLES } = await import('../js/composer.js');
  assert.deepEqual(PRESET_STYLES, ['normal', 'concise', 'explanatory', 'formal', 'learning']);
  setLanguage('en');
  assert.equal(t('style_learning'), 'Learning');
  setLanguage('tr');
  assert.equal(t('style_learning'), 'Öğrenme');
  setLanguage('en');
});
