// npm run test:ui — the chip for long text pasted into the composer.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, t } from '../js/i18n.js';

test('the pasted-text chip counts lines in both languages', () => {
  setLanguage('en');
  assert.equal(t('pasted_lines', { n: 120001 }), 'Pasted (120001 lines)');
  assert.equal(t('pasted_line_one'), 'Pasted (1 line)');
  setLanguage('tr');
  assert.equal(t('pasted_lines', { n: 3 }), 'Yapıştırıldı (3 satır)');
  assert.equal(t('pasted_line_one'), 'Yapıştırıldı (1 satır)');
  setLanguage('en');
});
