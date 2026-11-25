// npm run test:ui — Turkish wording that reads naturally.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, t } from '../js/i18n.js';

test('the Turkish wording the evaluator named reads naturally', () => {
  setLanguage('tr');
  assert.equal(t('from_server', { server: 'evalsrv' }), 'evalsrv sunucusundan');
  // "commit" is the Turkish block's word throughout: the same in the singular and the plural
  assert.equal(t('hist_commit_one'), `1 ${t('hist_commits')}`);
  assert.equal(t('of_capacity', { pct: 40 }), 'Kullanılan proje kapasitesi: %40');
  for (const k of ['welcome_title', 'project_name', 'nickname', 'preferences_help', 'click_to_open', 'mode_plan_d']) {
    assert.ok(!/(geldin|çalışıyorsun|\bsana\b|Tercihlerin|tıkla|planla)$/.test(t(k).replace(/[?.!]$/, '')), `${k}: ${t(k)}`);
  }
  setLanguage('en');
  assert.equal(t('from_server', { server: 'evalsrv' }), 'from evalsrv');
  assert.equal(t('of_capacity', { pct: 40 }), '40% of project capacity used');
});
