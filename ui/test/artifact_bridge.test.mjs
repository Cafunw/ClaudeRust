// npm run test:ui — consent to window.claude.complete and the "Try fixing"
// message (ui/js/artifact_bridge.js).
import test from 'node:test';
import assert from 'node:assert/strict';
import { createConsent, fixMessage } from '../js/artifact_bridge.js';

function asker(answers) {
  const asked = [];
  return { asked, ask: async (scope) => { asked.push(scope); return answers.shift(); } };
}

test('"Allow for this artifact" and "Decline" are kept for that artifact', async () => {
  const a = asker(['always', 'decline']);
  const allowed = createConsent(a.ask);
  assert.equal(await allowed('c/one'), true);
  assert.equal(await allowed('c/one'), true);
  assert.equal(await allowed('c/two'), false);
  assert.equal(await allowed('c/two'), false);
  assert.deepEqual(a.asked, ['c/one', 'c/two'], 'each artifact was asked once');
});

test('"Allow once" and a dismissed question ask again next time', async () => {
  const a = asker(['once', undefined, 'once']);
  const allowed = createConsent(a.ask);
  assert.equal(await allowed('s'), true);
  assert.equal(await allowed('s'), false, 'dismissed: declined, this time only');
  assert.equal(await allowed('s'), true);
  assert.equal(a.asked.length, 3);
});

test('calls made together are asked about one at a time', async () => {
  let open = 0;
  let most = 0;
  const allowed = createConsent(async () => {
    open++; most = Math.max(most, open);
    await new Promise((r) => setTimeout(r, 5));
    open--;
    return 'once';
  });
  assert.deepEqual(await Promise.all([allowed('s'), allowed('s'), allowed('s')]), [true, true, true]);
  assert.equal(most, 1);
  // a question that fails declines that call and doesn't block the next
  const flaky = createConsent(async () => { throw new Error('no dialog'); });
  await assert.rejects(flaky('s'));
});

test('"Try fixing" sends the error and its place, or asks for a full rewrite', () => {
  const a = { identifier: 'calc', title: 'Calculator', version: 3 };
  const runtime = fixMessage(a, { kind: 'runtime', message: 'Uncaught TypeError: x is undefined', stack: 'TypeError: x is undefined\n    at App (blob:1:2)' });
  assert.equal(runtime.key, 'art_fix_msg');
  assert.deepEqual({ ...runtime.vars, error: undefined }, { title: 'Calculator', id: 'calc', version: 3, error: undefined });
  assert.equal(runtime.vars.error, 'Uncaught TypeError: x is undefined\nTypeError: x is undefined\n    at App (blob:1:2)');
  // a stack that starts with the message isn't repeated, a long one is cut
  assert.equal(fixMessage(a, { message: 'Boom', stack: 'Error: Boom\n at f' }).vars.error, 'Error: Boom\n at f');
  assert.equal(fixMessage(a, { message: 'Boom', stack: '' }).vars.error, 'Boom');
  assert.equal(fixMessage(a, { message: 'x'.repeat(5000), stack: '' }).vars.error.length, 1500);
  const update = fixMessage({ identifier: 'calc', title: '', version: 4 }, { kind: 'update', message: 'old_str was not found in the current version.' });
  assert.equal(update.key, 'art_rewrite_msg');
  assert.deepEqual(update.vars, { title: 'calc', id: 'calc', version: 4, error: 'old_str was not found in the current version.' });
});
