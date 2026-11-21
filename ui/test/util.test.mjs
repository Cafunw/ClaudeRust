// npm run test:ui — the pure helpers of ui/js/util.js (escaping, formatting,
// change detection, paths).
import './stub_globals.mjs';
import test, { mock } from 'node:test';
import assert from 'node:assert/strict';
import { t } from '../js/i18n.js';
import { debounce, esc, fmtTokens, fmtUsd, humanSize, initials, relPath, relTime, sameSnapshot, snapshot } from '../js/util.js';

test('esc makes any value safe in HTML text and attributes', () => {
  assert.equal(esc(`<a href="x" title='y'>&</a>`), '&lt;a href=&quot;x&quot; title=&#39;y&#39;&gt;&amp;&lt;/a&gt;');
  assert.equal(esc('&amp;'), '&amp;amp;', 'escaped once more, never read as an entity');
  assert.equal(esc(null), '');
  assert.equal(esc(undefined), '');
  assert.equal(esc(0), '0');
});

test('fmtUsd shows cents, and two significant digits under a cent', () => {
  assert.equal(fmtUsd(0), '$0.00');
  assert.equal(fmtUsd(1.234), '$1.23');
  assert.equal(fmtUsd(0.0042), '$0.0042');
  assert.equal(fmtUsd('3'), '$3.00');
  assert.equal(fmtUsd(undefined), '$0.00');
  assert.equal(fmtUsd(NaN), '$0.00');
});

test('fmtTokens and humanSize pick the unit', () => {
  assert.deepEqual([999, 1500, 2_500_000].map(fmtTokens), ['999', '1.5k', '2.50M']);
  assert.deepEqual([0, 1023, 1536, 5 * 1024 * 1024].map(humanSize), ['0 B', '1023 B', '1.5 KB', '5.0 MB']);
});

test('initials of a name', () => {
  assert.equal(initials('Ada Lovelace'), 'AL');
  assert.equal(initials('  grace  '), 'G');
  assert.equal(initials('ada byron lovelace'), 'AL');
  assert.equal(initials(''), 'Y');
  assert.equal(initials(null), 'Y');
});

test('snapshots are equal exactly when the data is', () => {
  const v = { parts: [{ kind: 'text', text: 'hi' }, { kind: 'tool', input: { n: 1 } }], done: false };
  const s = snapshot(v);
  assert.ok(sameSnapshot(s, snapshot(structuredClone(v))));
  v.parts[1].input.n = 2;
  assert.ok(!sameSnapshot(s, snapshot(v)), 'a change in place is seen');
  assert.ok(!sameSnapshot(snapshot([1, [2]]), snapshot([[1], 2])), 'the shape counts');
  assert.ok(!sameSnapshot(snapshot(['1']), snapshot([1])));
  assert.ok(!sameSnapshot(snapshot([]), snapshot({})));
  assert.ok(!sameSnapshot(snapshot({ a: 1, b: 2 }), snapshot({ b: 2, a: 1 })), 'keys in order');
  assert.ok(!sameSnapshot(null, s));
});

test('relPath makes a path relative to its folder', () => {
  assert.equal(relPath('C:\\Src\\app\\main.rs', 'c:/src/'), 'app/main.rs', 'Windows paths without case');
  assert.equal(relPath('/home/u/p/a.txt', '/home/u/p'), 'a.txt');
  assert.equal(relPath('/home/u/p', '/home/u/p/'), '.');
  assert.equal(relPath('/home/u/px/a', '/home/u/p'), '/home/u/px/a', 'a sibling folder is outside');
  assert.equal(relPath('/Home/u/p/a', '/home/u/p'), '/Home/u/p/a', 'case counts elsewhere');
  assert.equal(relPath('a/b', ''), 'a/b');
});

test('relTime says yesterday and days ago', () => {
  const now = new Date();
  const startToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const day = 86400000;
  assert.equal(relTime(startToday - 1), t('yesterday'));
  assert.equal(relTime(startToday - 2 * day - 1), t('days_ago', { n: 3 }));
});

test('debounce calls once, with the last arguments, after the quiet time', () => {
  mock.timers.enable({ apis: ['setTimeout'] });
  try {
    const calls = [];
    const f = debounce((...a) => calls.push(a), 100);
    f(1);
    mock.timers.tick(60);
    f(2, 'b');
    mock.timers.tick(99);
    assert.deepEqual(calls, []);
    mock.timers.tick(1);
    assert.deepEqual(calls, [[2, 'b']]);
  } finally {
    mock.timers.reset();
  }
});
