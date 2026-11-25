// npm run test:ui — Settings pages: every page renders into its own container
// (a slow page never draws into the next one), Skills shows the folder picked
// last, and an outside edit of the MCP config is announced.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { setLanguage, t } from '../js/i18n.js';
import { source } from './source.mjs';

const read = source;
const SETTINGS = read('../js/settings.js');

test('each Settings page renders into a container of its own, replaced when another page is shown', () => {
  const show = SETTINGS.slice(SETTINGS.indexOf('function show(id)'), SETTINGS.indexOf('// redraw for changes made elsewhere'));
  assert.match(show, /const page = h\('<div class="settings-page"><\/div>'\);\s*body\.replaceChildren\(page\);/);
  assert.match(show, /RENDER\[id\]\(page, /, 'the page draws into its container, not the shared body');
  assert.doesNotMatch(show, /RENDER\[id\]\(body/);
});

test('Settings → Skills shows the list of the folder picked last, not of a slower earlier pick', () => {
  const src = read('../js/skills_page.js');
  const draw = src.slice(src.indexOf('const draw = async () => {'), src.indexOf('actions.append('));
  assert.match(draw, /const mine = \+\+drawn;/);
  // both ways out of the await (a list, an error) check that no newer draw started meanwhile
  assert.equal(draw.match(/if \(mine !== drawn\) return;/g).length, 2);
  assert.ok(draw.indexOf('if (mine !== drawn) return;') < draw.indexOf('list.replaceChildren('));
});

test('an outside edit of the MCP config is announced, in both languages', () => {
  assert.match(SETTINGS, /onBackend\('mcp-config-reloaded', \(\) => toast\(t\('mcp_config_reloaded'\)\)\)/);
  setLanguage('en');
  assert.equal(t('mcp_config_reloaded'), 'The MCP config file changed: servers reloaded.');
  setLanguage('tr');
  assert.equal(t('mcp_config_reloaded'), 'MCP yapılandırma dosyası değişti: sunucular yeniden yüklendi.');
  setLanguage('en');
});
