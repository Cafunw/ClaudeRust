// npm run test:ui — message actions the backend refuses while Claude responds
// (engine.rs RunSlot: an edit, a retry, another version, a rewind, going on
// with an answer): each says why and offers Stop, and its button shows it is
// held up meanwhile, instead of a toast that only repeats "Stop response".
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, t } from '../js/i18n.js';
import { source } from './source.mjs';

const chat = source('../js/chat.js');
const body = (name) => {
  const at = chat.search(new RegExp(`\\n(?:export )?(?:async )?function ${name}\\(`));
  assert.ok(at >= 0, name);
  return chat.slice(at, chat.indexOf('\n}\n', at));
};
const WHAT = ['edit', 'retry', 'versions', 'rewind', 'resume'];

test('each held-up action says what to do in both languages, not just the button\'s label', () => {
  for (const lang of ['en', 'tr']) {
    setLanguage(lang);
    for (const w of WHAT) {
      const s = t(`busy_${w}`);
      assert.ok(s && s !== `busy_${w}` && s !== t('stop'), `${lang} busy_${w}: ${s}`);
    }
    assert.notEqual(t('errk_conversation_busy'), 'errk_conversation_busy', `${lang}: the backend's refusal, coded`);
  }
  setLanguage('en');
  assert.equal(t('busy_edit'), 'Stop the response to edit this message');
  assert.ok(!chat.includes("toast(t('stop'))"), 'no toast that only says "Stop response"');
});

test('the toast offers Stop for this conversation', () => {
  const w = body('waitForIdle');
  assert.match(w, /if \(!cur \|\| !cur\.running\) return false;/);
  assert.match(w, /toast\(t\(`busy_\$\{what\}`\), '', \{ action: t\('stop'\), onAction: \(\) => stopTurn\(id\) \}\);/);
});

test('every action the backend refuses during a response waits for it', () => {
  const guarded = { startEdit: 'edit', retry: 'retry', resumeReply: 'resume', rewindTo: 'rewind', openRewindPicker: 'rewind', branchNav: 'versions' };
  for (const [fn, what] of Object.entries(guarded)) assert.ok(body(fn).includes(`waitForIdle('${what}')`), fn);
  // and the retry-with-model menu before it opens
  assert.match(chat, /rm\.onclick = \(\) => !waitForIdle\('retry'\) && showMenu\(/);
});

test('their buttons show they are held up while Claude responds, and why', () => {
  for (const what of ['edit', 'retry', 'rewind']) assert.match(chat, new RegExp(`idleOnly\\(h\\(\`<button[^\`]+\`\\), '${what}'\\)`), what);
  assert.match(chat, /forEach\(\(b\) => idleOnly\(b, 'versions'\)/, 'the version arrows');
  const show = body('showBusy');
  assert.match(show, /setAttribute\('aria-disabled', 'true'\)/);
  assert.match(show, /b\.title = busy \? t\(`busy_\$\{b\.dataset\.busy\}`\) : b\.dataset\.tip;/);
  // when a response starts or ends, and when a conversation opens mid-response
  assert.equal(chat.match(/querySelectorAll\('\[data-busy\]'\)\.forEach\(\(b\) => showBusy\(b, /g).length, 2);
  // dimmed even where hovering shows a turn's actions (a rule of four classes)
  assert.match(source('../css/app.css'), /\.turn:is\(:hover, :focus-within, \.last\) \.turn-actions > \[aria-disabled="true"\], \.turn-actions \.branch-nav \[aria-disabled="true"\] \{ opacity: \.45; \}/);
});
