// npm run test:ui — Settings → Skills with many skills stays easy to scan: a filter on name and
// description (with a no-match state, emptied by Esc), the skills grouped by where they come from,
// and each description two lines until it is opened.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { source } from './source.mjs';
import { setLanguage, t } from '../js/i18n.js';
import { filterSkills, groupSkills, shortDescription } from '../js/skills_page.js';

const skills = [
  { name: 'pdf-tools', description: 'Fill and merge PDF forms', source: 'personal' },
  { name: 'İmage-gen', description: 'Make pictures', source: 'personal' },
  { name: 'deploy', description: 'Ship the site to staging', source: 'project' },
  { name: 'review', description: 'Review a pull request against the playbook', source: 'plugin', plugin: 'eng' },
  { name: 'notes', description: '', source: 'user' },
];

test('the filter matches every word in the name or the description, in any case', () => {
  assert.deepEqual(filterSkills(skills, 'pdf').map((s) => s.name), ['pdf-tools']);
  assert.deepEqual(filterSkills(skills, 'STAGING ship').map((s) => s.name), ['deploy'], 'words of the description, any order');
  assert.deepEqual(filterSkills(skills, 'image').map((s) => s.name), ['İmage-gen'], 'Turkish İ reads as i');
  assert.equal(filterSkills(skills, '').length, skills.length, 'an empty filter shows them all');
  assert.equal(filterSkills(skills, 'nothing like it').length, 0);
});

test('skills are grouped personal, project, plugin, only the groups that have some', () => {
  assert.deepEqual(groupSkills(skills).map(([g, l]) => [g, l.map((s) => s.name)]), [
    ['personal', ['pdf-tools', 'İmage-gen', 'notes']],
    ['project', ['deploy']],
    ['plugin', ['review']],
  ]);
  assert.deepEqual(groupSkills(skills.slice(0, 2)).map(([g]) => g), ['personal']);
});

test('a long description shows its start, cut at a word, until it is opened', () => {
  assert.equal(shortDescription('short one'), 'short one');
  const long = 'word '.repeat(80).trim();
  const s = shortDescription(long);
  assert.ok(s.length <= 171 && s.endsWith('…') && !s.includes('wor…'), s);
  assert.ok(shortDescription('x'.repeat(400)).length === 171, 'one unbroken token is cut where it must');
});

test('the page has the filter, the no-match state, the groups and an accessible expand control', () => {
  const s = source('../js/skills_page.js');
  assert.match(s, /<input class="input" type="search" placeholder="\$\{esc\(t\('sk_filter'\)\)\}" aria-label="\$\{esc\(t\('sk_filter'\)\)\}">/);
  assert.match(s, /role="status"[^`]*t\('sk_no_match', \{ q: input\.value\.trim\(\) \}\)/);
  assert.match(s, /<h4 class="sk-group">/);
  assert.match(s, /<button class="sk-more" type="button" aria-expanded="false" aria-controls="\$\{d\.id\}">/);
  assert.match(s, /b\.setAttribute\('aria-expanded', String\(open\)\);/);
  // two lines in CSS too, for a narrow Settings
  assert.match(source('../css/app.css'), /\.sk-desc \{[^}]*-webkit-line-clamp: 2;/);
  for (const lang of ['en', 'tr']) {
    setLanguage(lang);
    for (const k of ['sk_filter', 'sk_no_match', 'sk_group_plugin', 'sk_more', 'sk_less']) assert.notEqual(t(k), k, `${lang} ${k}`);
  }
  setLanguage('en');
});
