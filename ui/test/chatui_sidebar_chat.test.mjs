// npm run test:ui — round 5 chat and sidebar helpers: sidebar groups and drops,
// research progress, fast mode, files Claude made, Mermaid frames.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage } from '../js/i18n.js';
import { groupConversations, dropTarget, isDrag } from '../js/sidebar_org.js';
import { researchLine, researchQuery } from '../js/research_ui.js';
import { fastNote, thinkingSupported, researchOffered, attachmentError } from '../js/composer.js';
import { outputFile } from '../js/parts.js';
import { mermaidDoc } from '../js/mermaid_block.js';

test('each conversation is drawn once: pinned, then its section, then starred, then recents', () => {
  const s1 = { id: 's1', name: 'Work' };
  const convs = [
    { id: 'a', pinned_at: 5, section_id: 's1', starred: true },
    { id: 'b', pinned_at: 9 },
    { id: 'c', section_id: 's1', starred: true },
    { id: 'd', starred: true },
    { id: 'e' },
    { id: 'f', section_id: 'gone' },
  ];
  const g = groupConversations(convs, [s1, { id: 's2', name: 'Empty' }]);
  const ids = (l) => l.map((c) => c.id);
  assert.deepEqual(ids(g.pinned), ['b', 'a'], 'newest pin first');
  assert.deepEqual(g.sections.map((x) => [x.section.id, ids(x.items)]), [['s1', ['c']], ['s2', []]]);
  assert.deepEqual(ids(g.starred), ['d']);
  assert.deepEqual(ids(g.recents), ['e', 'f'], 'a deleted section leaves its conversations in recents');
  assert.deepEqual(groupConversations([], []), { pinned: [], sections: [], starred: [], recents: [] });
});

test('a drop moves into a section, out of one, or does nothing', () => {
  assert.equal(dropTarget(null, 's1'), 's1');
  assert.equal(dropTarget('s1', 's2'), 's2');
  assert.equal(dropTarget('s1', ''), null, 'onto Recents: out of the section');
  assert.equal(dropTarget('s1', 's1'), undefined, 'its own section');
  assert.equal(dropTarget(null, ''), undefined, 'already in none');
  assert.equal(dropTarget(undefined, ''), undefined);
  assert.equal(dropTarget('s1', undefined), undefined, 'dropped on no zone');
  assert.ok(!isDrag(3, 4), 'a 5 px wobble is a click');
  assert.ok(isDrag(6, 0) && isDrag(0, -7));
});

test('the research line says the phase, the questions done and the spending (research.rs events)', () => {
  setLanguage('en');
  const ev = { conversation_id: 'c', phase: 'researching', done: 2, total: 5, queries: ['a', 'rust 2026 release'], spent_usd: 0.123, budget_usd: 2 };
  assert.equal(researchLine(ev), 'Searching the web · 2 of 5 questions · $0.12 of $2.00');
  assert.equal(researchQuery(ev), '“rust 2026 release”');
  assert.equal(researchLine({ phase: 'planning', done: 0, total: 0 }), 'Planning the research');
  assert.equal(researchLine({ phase: 'writing', done: 9, total: 5 }), 'Writing the report · 5 of 5 questions');
  assert.equal(researchLine({ phase: 'mystery' }), 'Researching');
  assert.equal(researchLine(null), 'Researching');
  assert.equal(researchQuery({ queries: [] }), '');
  assert.equal(researchQuery(null), '');
  setLanguage('tr');
  assert.equal(researchLine(ev), 'Web’de aranıyor · 5 sorunun 2 tanesi · $0.12 / $2.00');
  setLanguage('en');
});

test('a refused attachment is said in the UI language from its kind (attachments.rs)', () => {
  setLanguage('tr');
  const e = Object.assign(new Error('“a.pdf” is not a valid PDF file.'), { kind: 'pdf_invalid', status: 0 });
  assert.equal(attachmentError(e, 'a.pdf'), '“a.pdf” geçerli bir PDF dosyası değil.');
  assert.equal(attachmentError(e), 'Bu dosya geçerli bir PDF dosyası değil.', 'no name known');
  assert.equal(attachmentError({ kind: 'unsupported_file', name: 'x.bin', message: 'English' }), '“x.bin” desteklenen bir dosya türü değil. Görsel, PDF, Word, Excel, PowerPoint, OpenDocument metin, EPUB, RTF ve metin dosyaları ekleyebilirsiniz.', 'an error object from stage_paths');
  assert.match(attachmentError({ kind: 'message_too_large' }), /30 MB/);
  assert.equal(attachmentError(Object.assign(new Error('boom'), { kind: 'network' })), null, 'not an attachment refusal');
  assert.equal(attachmentError(null), null);
  setLanguage('en');
  assert.equal(attachmentError({ kind: 'pdf_too_many_pages_for_model' }), 'A PDF here has more than 100 pages, the most this model reads. Choose a newer model, or attach fewer pages.');
});

test('research is offered in chats on the API engine only', () => {
  assert.ok(researchOffered({ chat_engine: 'api' }));
  assert.ok(researchOffered({ chat_engine: 'auto', has_api_key: true }));
  assert.ok(!researchOffered({ chat_engine: 'auto', has_api_key: false }), 'the account engine');
  assert.ok(!researchOffered({ chat_engine: 'claude-code', has_api_key: true }));
});

test('fast mode shows its price, and the thinking switch only where it works', () => {
  setLanguage('en');
  assert.equal(fastNote({ price: [5, 25], fast_price: [10, 50] }), 'Faster answers at 2× the price');
  assert.equal(fastNote({ price: [5, 25], fast: [12.5, 62.5] }), 'Faster answers at 2.5× the price');
  assert.equal(fastNote({ fast: true }), 'Faster answers at a higher price');
  assert.equal(fastNote({ price: [5, 25] }), '', 'no fast mode');
  assert.ok(thinkingSupported({ thinking: 'disableable' }));
  assert.ok(!thinkingSupported({ thinking: 'disableable', thinking_switch: false }), 'the engine cannot turn it off');
  // a code session follows its own engine, not the chats'
  const cliless = { thinking: 'disableable', thinking_switch: false, thinking_switch_cli: false };
  assert.ok(!thinkingSupported(cliless, 'claude-code'));
  assert.ok(thinkingSupported(cliless, 'builtin'), 'the built-in agent can');
  assert.ok(thinkingSupported({ thinking: 'disableable', thinking_switch: false, thinking_switch_cli: true }, 'claude-code'));
});

test('a file Claude made shows its size, or why it was not kept', () => {
  setLanguage('en');
  assert.deepEqual(outputFile({ name: 'q3.xlsx', size: 2048, path: '/x/q3.xlsx' }), { ok: true, name: 'q3.xlsx', sub: '2.0 KB' });
  assert.deepEqual(outputFile({ name: 'big.zip', error: 'too_large' }), { ok: false, name: 'big.zip', sub: 'Not kept: too large.' });
  assert.deepEqual(outputFile({ name: null, error: 'weird' }), { ok: false, name: 'Unnamed file', sub: 'Couldn’t be downloaded.' });
  assert.equal(outputFile({}).sub, '0 B');
});

test('a Mermaid frame gets its source as text only, strict, in the app theme', () => {
  const doc = mermaidDoc('graph TD; A["</pre><script>alert(1)</script>"]-->B', true);
  assert.ok(!doc.includes('<script>alert'), 'the source cannot end the <pre> or open a script');
  assert.ok(doc.includes('&lt;/pre&gt;&lt;script&gt;'));
  assert.ok(doc.includes("securityLevel: 'strict'") && doc.includes("theme: 'dark'") && doc.includes('color-scheme:dark'));
  assert.ok(mermaidDoc('graph TD; A-->B', false).includes("theme: 'default'"));
});
