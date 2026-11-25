// npm run test:ui — no hard-coded English next to translated text: a template
// literal that has a t() call in it carries no English words of its own.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

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
