// npm run test:ui — the Code tab's panes: their keyboard rows and tabs, the
// side panel's width, binary files in a diff, and the words around counts,
// review comments and task times.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, t } from '../js/i18n.js';
import { nextRow, nextTab } from '../js/panes/rows.js';

// projects.js reaches the chat view, whose modules ask for media queries as they load
window.matchMedia ??= () => ({ matches: false, addEventListener() {}, addListener() {} });
globalThis.MutationObserver ??= class { observe() {} };
const { parseUnified, DIFF_ROWS_SHOWN } = await import('../js/panes/changes.js');
const { reviewPrompt } = await import('../js/panes/review.js');
const { clampWidth } = await import('../js/workspace.js');
const { chatCount } = await import('../js/projects.js');
const { elapsed } = await import('../js/panes/tasks.js');

test('row keys: ↑ ↓ stop at the ends, Home and End jump, other keys do not move', () => {
  assert.equal(nextRow(0, 4, 'ArrowDown'), 1);
  assert.equal(nextRow(3, 4, 'ArrowDown'), 3, 'the last row stays');
  assert.equal(nextRow(0, 4, 'ArrowUp'), 0, 'the first row stays');
  assert.equal(nextRow(2, 4, 'ArrowUp'), 1);
  assert.equal(nextRow(2, 4, 'Home'), 0);
  assert.equal(nextRow(0, 4, 'End'), 3);
  assert.equal(nextRow(1, 4, 'a'), -1);
  assert.equal(nextRow(0, 0, 'ArrowDown'), -1, 'no rows');
});

test('tab keys: ← → go round, Home and End jump, focus outside the tabs does nothing', () => {
  assert.equal(nextTab(0, 3, 'ArrowRight'), 1);
  assert.equal(nextTab(2, 3, 'ArrowRight'), 0, 'past the last comes the first');
  assert.equal(nextTab(0, 3, 'ArrowLeft'), 2, 'before the first comes the last');
  assert.equal(nextTab(1, 3, 'Home'), 0);
  assert.equal(nextTab(1, 3, 'End'), 2);
  assert.equal(nextTab(1, 3, 'ArrowDown'), -1);
  assert.equal(nextTab(-1, 3, 'ArrowRight'), -1, 'not on a tab');
  assert.equal(nextTab(0, 0, 'ArrowRight'), -1);
});

test('the side panel keeps 400 px for the chat and never gets narrower than 300 px', () => {
  assert.equal(clampWidth(460, 1240 - 288), 460, 'room enough: as wanted');
  assert.equal(clampWidth(800, 1000 - 288), 312, 'a wide saved width gives way to the chat');
  assert.equal(clampWidth(200, 1240), 300, 'the minimum');
  assert.equal(clampWidth(500, 600), 300, 'too little room: the minimum still');
  assert.equal(clampWidth(Infinity, 1000), 600, 'the widest it can be');
});

test('a binary file in a diff is a binary row, not a line numbered 0 that lost its first letter', () => {
  const rows = parseUnified('diff --git a/logo.bin b/logo.bin\nindex 1..2 100644\nBinary files a/logo.bin and b/logo.bin differ\n');
  assert.deepEqual(rows, [{ type: 'binary', text: 'Binary files a/logo.bin and b/logo.bin differ' }]);
  const text = parseUnified('@@ -1,2 +1,2 @@\n a\n-b\n+c\n');
  assert.deepEqual(text.map((r) => r.type), ['hunk', 'ctx', 'del', 'add'], 'text diffs are as before');
  assert.ok(DIFF_ROWS_SHOWN >= 1000, 'a long diff still shows a good part at once');
});

test('project cards count chats in each language ("1 chat", Turkish without a plural)', () => {
  setLanguage('en');
  assert.equal(chatCount(1), '1 chat');
  assert.equal(chatCount(0), '0 chats');
  assert.equal(chatCount(3), '3 chats');
  setLanguage('tr');
  assert.equal(chatCount(1), '1 sohbet');
  assert.equal(chatCount(3), '3 sohbet');
  setLanguage('en');
});

test('a review comment on a removed line says so in the UI language', () => {
  setLanguage('tr');
  const p = reviewPrompt([{ path: 'a.rs', line: 4, side: 'old', body: 'neden?', code: 'x' }]);
  assert.match(p, /`a\.rs:4` \(silinen satır\) — neden\?/);
  assert.doesNotMatch(p, /removed line/);
  setLanguage('en');
  assert.match(reviewPrompt([{ path: 'a.rs', line: 4, side: 'old', body: 'why?' }]), /\(removed line\)/);
});

test('task times are in the UI language ("sn." and "dk." in Turkish, not "s" and "m")', () => {
  setLanguage('en');
  assert.equal(elapsed(0, 45_000), '45 sec');
  assert.equal(elapsed(0, 184_000), '3 min 4 sec');
  assert.equal(elapsed(0, 7_500_000), '2 hr 5 min');
  assert.equal(elapsed(10_000, 5_000), '0 sec', 'never negative');
  setLanguage('tr');
  assert.equal(elapsed(0, 45_000), '45 sn.');
  assert.equal(elapsed(0, 184_000), '3 dk. 4 sn.');
  assert.equal(elapsed(0, 7_500_000), '2 sa. 5 dk.');
  setLanguage('en');
});

test('the Code tab\'s pane, project and onboarding keys have English and Turkish text', () => {
  const keys = ['ws_resize', 'changes_show_all', 'task_exit', 'hist_search_by', 'code_ask_ph', 'review_removed_line',
    'proj_chats_one', 'proj_chats_n', 'proj_options', 'instr_edit', 'proj_file_remove', 'onboard_need_key'];
  for (const lang of ['en', 'tr']) {
    setLanguage(lang);
    for (const k of keys) assert.notEqual(t(k), k, `${lang}: ${k}`);
  }
  setLanguage('tr');
  assert.notEqual(t('code_ask_ph'), (setLanguage('en'), t('code_ask_ph')), 'Turkish is its own text');
  setLanguage('en');
});
