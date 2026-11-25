// npm run test:ui — MCP tools and servers as the server cards and the permission card show them
// (ui/js/mcp_tool_info.js): the title (else the name), the description, the hints, the icons.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { pickIcon, iconHtml, toolLabelHtml, toolHintsHtml, permissionInfo } from '../js/mcp_tool_info.js';
import { store } from '../js/store.js';
import { setLanguage } from '../js/i18n.js';

const add = { name: 'add', title: 'Add numbers', description: 'Adds <two> numbers', read_only: true, destructive: false, icons: [] };
const wipe = { name: 'delete_all', title: null, description: '', read_only: false, destructive: true, icons: [] };

test('a tool row shows its title, its hints, and its description and own name on demand', () => {
  setLanguage('en');
  const row = toolLabelHtml(add);
  assert.match(row, /^<details class="tool-info"><summary title="Adds &lt;two&gt; numbers">/);
  assert.match(row, /<span class="nm titled">Add numbers<\/span><span class="tool-hint">Read-only<\/span>/);
  assert.match(row, /<div class="tool-desc small muted"><span class="mono">add<\/span> Adds &lt;two&gt; numbers<\/div>/);
  // no title and no description: just the name and its hint, nothing to open
  assert.equal(toolLabelHtml(wipe), '<div class="tool-info"><span class="nm">delete_all</span><span class="tool-hint danger">Destructive</span></div>');
  assert.equal(toolHintsHtml({ name: 'x' }), '');
  setLanguage('tr');
  assert.equal(toolHintsHtml(wipe), '<span class="tool-hint danger">Veri silebilir</span>');
  setLanguage('en');
});

test('icons: the one made for the theme, else one for any theme, else any', () => {
  const light = { src: 'data:image/png;base64,AA', theme: 'light' };
  const dark = { src: 'https://e.example/d.png', theme: 'dark' };
  const any = { src: 'https://e.example/a.svg' };
  assert.equal(pickIcon([light, dark, any], 'dark'), dark);
  assert.equal(pickIcon([light, any], 'dark'), any);
  assert.equal(pickIcon([light], 'dark'), light);
  assert.equal(pickIcon([{ src: 5 }], 'light'), null);
  assert.equal(pickIcon(undefined, 'light'), null);
  assert.match(iconHtml([any], 16, 'plug'), /^<img class="mcp-icon" src="https:\/\/e\.example\/a\.svg" width="16" height="16" alt="" referrerpolicy="no-referrer"/);
  assert.match(iconHtml([], 16, 'plug'), /^<svg/);
  // a quote in a server's URL can't leave the attribute
  assert.doesNotMatch(iconHtml([{ src: 'https://e.example/"onerror="x' }], 16, 'plug'), /"onerror="/);
});

test('the permission card names the tool by its title, says its hints and description, and keeps its own name', () => {
  store.mcpServers = [{ name: 'calc', tools: [add], server_info: { serverInfo: { icons: [{ src: 'https://e.example/c.png' }] } } }];
  const p = permissionInfo({ kind: 'mcp', server: 'calc', tool_name: 'add', tool: 'calc__add' });
  assert.match(p.title, /^<img class="mcp-icon" src="https:\/\/e\.example\/c\.png"/);
  assert.match(p.title, /Claude wants to use <span class="perm-tool">Add numbers<\/span><span class="tool-hint">Read-only<\/span>$/);
  assert.equal(p.desc, 'from calc · add');
  assert.equal(p.about, '<p class="perm-tool-desc" title="Adds &lt;two&gt; numbers">Adds &lt;two&gt; numbers</p>');
  // a tool the app hasn't listed (yet): its name, as before
  const q = permissionInfo({ kind: 'mcp', server: 'other', tool_name: 'run', tool: 'other__run' });
  assert.match(q.title, /<svg[^]*Claude wants to use <span class="mono">run<\/span>$/);
  assert.equal(q.desc, 'from other');
  assert.equal(q.about, '');
});
