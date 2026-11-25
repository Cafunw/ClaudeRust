// npm run test:ui — project files added from GitHub: grouped by the address
// they came from, and what "Sync from GitHub" did said in full
// (ui/js/github_source.js; the sync itself is tested in github_source.rs).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { knowledgeRows, repoLabel, syncSummary } from '../js/github_source.js';
import { setLanguage } from '../js/i18n.js';

const A = 'https://github.com/a/b';
const B = 'https://github.com/a/b/tree/feature/x/my%20dir';
const gh = (id, url) => ({ id, name: `b/${id}`, github: { url, path: id, sha: '0'.repeat(40) } });

test('a repository’s files are listed together under one heading', () => {
  const files = [{ id: 'notes' }, gh('r1', A), { id: 'up' }, gh('s1', B), gh('r2', A)];
  const rows = knowledgeRows(files);
  assert.deepEqual(rows.map((r) => r.group ? `[${r.repo}]` : r.id), ['notes', '[a/b]', 'r1', 'r2', 'up', '[a/b · feature/x/my dir]', 's1']);
  assert.deepEqual(rows[1].files.map((f) => f.id), ['r1', 'r2']);
  assert.equal(rows[1].group, A);
  // files added before sources were kept (or another way): no heading, no Sync
  assert.deepEqual(knowledgeRows([{ id: 'x' }, { id: 'y' }]).map((r) => r.id), ['x', 'y']);
  assert.equal(repoLabel('https://github.com/o/r/tree/v1.0'), 'o/r · v1.0');
});

test('a sync says what changed, and what it kept and why', () => {
  setLanguage('en');
  const r = { repo: 'a/b', up_to_date: false, updated: [], unchanged: 0, missing: [], errors: [] };
  assert.deepEqual(syncSummary({ ...r, up_to_date: true, unchanged: 3 }), [['The files from a/b are up to date.', '']]);
  assert.deepEqual(syncSummary({ ...r, unchanged: 2 }), [['Synced from a/b: none of the files changed.', '']]);
  assert.deepEqual(syncSummary({ ...r, updated: ['b/x.md'] }), [['Synced from a/b: 1 file changed and was replaced (b/x.md).', '']]);
  const many = syncSummary({ ...r, updated: ['1', '2', '3', '4'], missing: ['b/gone.md'], errors: [{ kind: 'github_not_text', message: 'x', name: 'b/pic.txt' }] });
  assert.deepEqual(many, [
    ['Synced from a/b: 4 files changed and were replaced (1, 2, 3, …).', ''],
    ['1 file is no longer in the repository and was kept: b/gone.md.', ''],
    ['1 not synced, kept as before. b/pic.txt: This isn’t a text file.', 'error'],
  ]);
  setLanguage('tr');
  assert.deepEqual(syncSummary({ ...r, missing: ['p', 'q'] }), [['2 dosya artık depoda yok, projede bırakıldı: p, q.', '']]);
  setLanguage('en');
});
