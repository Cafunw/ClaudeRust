// npm run test:ui — round 6 front end: search snippets without Markdown, one
// name for an untitled chat, the artifact panel across branches, Continue on
// a cut-short answer, asking for a full turn after a missed stream event, and
// no hard-coded English next to translated text.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { setLanguage, t } from '../js/i18n.js';
import { snippetHtml, convTitle } from '../js/util.js';
import { panelStep, shownArtifact } from '../js/branch_panel.js';
import { resumable } from '../js/parts.js';
import { receive } from '../js/turnstream.js';

test('search snippets lose table pipes and rules, heading and list marks, and keep the match marks', () => {
  assert.equal(snippetHtml('…| 16 | 12 |\n## \u0001Section\u0002 5'), '… 16 12 <mark>Section</mark> 5');
  assert.equal(snippetHtml('| a | b |\n|---|:--:|\n| c | d |'), 'a b c d');
  assert.equal(snippetHtml('- item\n> quote\n1. first'), 'item quote 1. first');
  assert.equal(snippetHtml('**bold** and `code`, [a link](https://x.y/z) ![pic](a.png) ~~gone~~'), 'bold and code, a link pic gone');
  assert.equal(snippetHtml('```rust\nlet a = b || c;\n```'), 'let a = b || c;', 'pipes inside a line are kept');
  assert.equal(snippetHtml('<b>&'), '&lt;b&gt;&amp;', 'escaped');
  assert.equal(snippetHtml(''), '');
  assert.equal(snippetHtml(null), '');
});

test('an untitled chat is "New chat" everywhere; a code session stays "Untitled"', () => {
  setLanguage('tr');
  assert.equal(convTitle({ title: '', kind: 'chat' }), 'Yeni sohbet');
  assert.equal(convTitle({ title: null }), 'Yeni sohbet');
  assert.equal(convTitle({ title: '', kind: 'code' }), 'Adsız');
  assert.equal(convTitle({ title: 'Plan', kind: 'chat' }), 'Plan');
  setLanguage('en');
});

test('the artifact panel folds on a branch without artifacts and comes back only if it folded itself', () => {
  assert.equal(panelStep(false, true, false), 'close');
  assert.equal(panelStep(true, false, true), 'reopen');
  assert.equal(panelStep(true, false, false), null, 'closed by hand: stays closed');
  assert.equal(panelStep(true, true, false), null);
  assert.equal(panelStep(false, false, true), null);
  assert.equal(panelStep(true, true, true), null, 'already open again');
});

test('Continue is offered on a stopped or failed answer that has text', () => {
  const text = { kind: 'text', html: '<p>half</p>' };
  assert.ok(resumable([text, { kind: 'notice', code: 'error' }]));
  assert.ok(resumable([text, { kind: 'notice', code: 'stopped' }]));
  assert.ok(!resumable([{ kind: 'notice', code: 'error' }]), 'nothing came: only Retry');
  assert.ok(!resumable([text, { kind: 'notice', code: 'max_tokens' }]), 'max tokens has its own Continue');
  assert.ok(!resumable([text]));
  assert.ok(!resumable([text, { kind: 'tool', status: 'running' }, { kind: 'notice', code: 'stopped' }]), 'stopped in a tool call: only Retry');
  assert.ok(resumable([text, { kind: 'tool', status: 'done' }, text, { kind: 'notice', code: 'stopped' }]), 'text after the tool call');
});

test('the artifact panel keeps its artifact on a branch that has it, else shows the branch\'s latest', () => {
  assert.equal(shownArtifact(['a', 'b'], 'a'), 'a');
  assert.equal(shownArtifact(['b', 'c'], 'a'), 'c');
  assert.equal(shownArtifact([], 'a'), null);
});

test('the pasted-text chip counts lines in both languages', () => {
  setLanguage('en');
  assert.equal(t('pasted_lines', { n: 120001 }), 'Pasted (120001 lines)');
  assert.equal(t('pasted_line_one'), 'Pasted (1 line)');
  setLanguage('tr');
  assert.equal(t('pasted_lines', { n: 3 }), 'Yapıştırıldı (3 satır)');
  assert.equal(t('pasted_line_one'), 'Yapıştırıldı (1 satır)');
  setLanguage('en');
});

test('a delta that cannot be expanded asks once for a full turn, until one comes', () => {
  const state = new Map();
  const waiting = new Set();
  const asked = [];
  const ask = (c) => asked.push(c);
  const turn = { id: 't', streaming: true, parts: [{ kind: 'text', html: 'a' }], artifacts: [] };
  assert.ok(receive({ conversation_id: 'c', seq: 1, turn }, ask, state, waiting), 'a keyframe');
  const missed = { conversation_id: 'c', seq: 5, base: 4, turn: { ...turn, parts: [null] } };
  assert.equal(receive(missed, ask, state, waiting), null);
  assert.equal(receive({ ...missed, seq: 6, base: 5 }, ask, state, waiting), null);
  assert.deepEqual(asked, ['c'], 'asked once');
  assert.ok(receive({ conversation_id: 'c', seq: 7, turn }, ask, state, waiting), 'the full turn');
  assert.equal(receive({ ...missed, seq: 9, base: 8 }, ask, state, waiting), null);
  assert.deepEqual(asked, ['c', 'c'], 'missed again: asked again');
  assert.equal(receive(null, ask, state, waiting), null);
  assert.deepEqual(asked, ['c', 'c'], 'nothing to ask about');
});

// Words that read the same in every language: product and key names, a git
// term the Turkish UI keeps.
const SAME_IN_ALL = new Set(['Claude', 'Code', 'Esc', 'esc', 'Worktree']);

/** "line: words" for each template literal in `src` that has a t() call in it
 *  and English words in its own text (outside ${…}, HTML tags, entities and
 *  file extensions). */
function englishBesideT(src) {
  const espree = createRequire(import.meta.url)('espree');
  const isT = (n) => n.type === 'CallExpression' && n.callee.type === 'Identifier' && (n.callee.name === 't' || n.callee.name === 'tOr');
  const hasT = (n) => isT(n) || Object.entries(n).some(([k, v]) => k !== 'loc' && (Array.isArray(v) ? v : [v]).some((x) => typeof x?.type === 'string' && hasT(x)));
  const found = [];
  const visit = (n) => {
    if (n.type === 'TemplateLiteral' && n.expressions.some(hasT)) {
      const text = n.quasis.map((q) => q.value.cooked ?? '').join('\u0000').replace(/<[^>]*>/g, ' ').replace(/&\w+;/g, ' ');
      const words = (text.match(/(?<![.\w])[A-Za-z][a-z]{2,}\b/g) || []).filter((w) => !SAME_IN_ALL.has(w));
      if (words.length) found.push(`${n.loc.start.line}: ${words.join(' ')}`);
    }
    for (const [k, v] of Object.entries(n)) if (k !== 'loc') (Array.isArray(v) ? v : [v]).forEach((x) => typeof x?.type === 'string' && visit(x));
  };
  visit(espree.parse(src, { ecmaVersion: 'latest', sourceType: 'module', loc: true }));
  return found;
}

test('the English check finds a word beside t() and leaves markup, extensions and key names alone', () => {
  assert.deepEqual(englishBesideT('const a = `${t(\'pasted\')} (${n} lines)`;'), ['1: lines']);
  assert.deepEqual(englishBesideT('const a = `<b class="x">${esc(t(\'k\'))}</b>${n}.png <kbd>Esc</kbd>`;'), []);
  assert.deepEqual(englishBesideT('const a = `plain words ${n}`;'), [], 'only text beside translated text');
});

test('no template literal in ui/js puts visible English beside translated text', () => {
  const files = [];
  const walk = (dir) => { for (const e of readdirSync(dir, { withFileTypes: true })) { const p = join(dir, e.name); if (e.isDirectory()) walk(p); else if (p.endsWith('.js')) files.push(p); } };
  walk(fileURLToPath(new URL('../js/', import.meta.url)));
  assert.ok(files.length > 50, `${files.length} files`);
  const found = files.flatMap((f) => englishBesideT(readFileSync(f, 'utf8')).map((x) => `${f}:${x}`));
  assert.deepEqual(found, []);
});
