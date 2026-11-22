// npm run test:ui — window.claude.complete's gate (ui/js/artifact_bridge.js):
// where the engine can't answer, the person is never asked.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createComplete } from '../js/artifact_bridge.js';

function gate({ supported = true, answer = true } = {}) {
  const log = [];
  const complete = createComplete({
    check: async (f) => { log.push(['check', f.conv]); if (!supported) throw new Error('window.claude.complete needs an Anthropic API key'); },
    allowed: async (scope) => { log.push(['ask', scope]); return answer; },
    run: async (f, prompt) => { log.push(['run', prompt]); return 'answer'; },
  });
  return { complete, log };
}

const frame = { scope: 'c1/calc', title: 'Calc', conv: 'c1', artifact: 'calc' };

test('an engine that cannot answer rejects before any question', async () => {
  const g = gate({ supported: false });
  await assert.rejects(g.complete(frame, ['hi']), /needs an Anthropic API key/);
  assert.deepEqual(g.log, [['check', 'c1']], 'no consent dialog, no call');
});

test('a supported engine asks, then calls; a refusal calls nothing', async () => {
  const ok = gate();
  assert.equal(await ok.complete(frame, ['hi']), 'answer');
  assert.deepEqual(ok.log, [['check', 'c1'], ['ask', 'c1/calc'], ['run', 'hi']]);
  const no = gate({ answer: false });
  await assert.rejects(no.complete(frame, ['hi']), /did not allow/);
  assert.deepEqual(no.log.map((e) => e[0]), ['check', 'ask']);
});

test('a prompt that is not a string is refused before the check', async () => {
  const g = gate();
  for (const args of [[], [42], null, 'hi']) await assert.rejects(g.complete(frame, args), /takes a prompt string/);
  assert.deepEqual(g.log, []);
});
