// npm run test:ui — tOr: static backend text (model descriptions) through i18n.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { tOr, setLanguage } from '../js/i18n.js';

test('tOr translates by key and falls back to the backend text', () => {
  setLanguage('tr');
  assert.equal(tOr('model_d_claude-haiku-5-5', 'Fastest model for daily tasks'), 'Günlük görevler için en hızlı model');
  assert.equal(tOr('model_d_claude-new-9', 'From the backend'), 'From the backend');
  setLanguage('en');
  assert.equal(tOr('model_d_claude-haiku-5-5', 'x'), 'Fastest model for daily tasks');
  assert.equal(tOr('model_d_claude-new-9', 'From the backend'), 'From the backend');
});
