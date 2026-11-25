// npm run test:ui — a release published without the updater signing key has
// no latest.json: the update check says "no signed update yet", not an error,
// and asks only when asked (ui/js/updates.js; updates.rs maps the plugin's answer).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { checkNow, pendingUpdate, noneText } from '../js/updates.js';
import { setLanguage } from '../js/i18n.js';

test('a release without a signed manifest is no update to offer, said as such', async () => {
  const calls = [];
  window.__TAURI__.core.invoke = async (cmd) => {
    calls.push(cmd);
    if (cmd === 'update_check') return { available: false, current: '0.1.12', no_manifest: true, releases_url: 'https://github.com/o/r/releases/latest' };
    throw new Error(`no backend for ${cmd}`);
  };
  await checkNow();
  assert.deepEqual(calls, ['update_check'], 'one check, no retry loop');
  assert.equal(pendingUpdate(), null, 'nothing to install, nothing in the menu');
  const info = { available: false, current: '0.1.12', no_manifest: true };
  setLanguage('en');
  assert.equal(noneText(info), 'No signed update has been published for this system yet (you have 0.1.12).');
  assert.equal(noneText({ ...info, no_manifest: false }), 'ClaudeRust 0.1.12 is up to date.');
  setLanguage('tr');
  assert.equal(noneText(info), 'Bu sistem için henüz imzalı bir güncelleme yayımlanmadı (sizdeki: 0.1.12).');
  setLanguage('en');
  await checkNow();
  assert.equal(calls.length, 2, 'checked again only when asked');
});
