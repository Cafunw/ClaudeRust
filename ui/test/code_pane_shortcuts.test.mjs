// npm run test:ui — Alt+1… open the Code tab's panes in the order of their tabs.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';

window.matchMedia ??= () => ({ matches: false, addEventListener() {}, addListener() {} });
const workspace = await import('../js/workspace.js');

test('Alt+1… follow the tabs as they are shown, and the tooltips say them', () => {
  for (const [id, order] of [['files', 65], ['plan', 30], ['changes', 10], ['terminal', 20], ['history', 15]]) {
    workspace.registerPane({ id, order, label: () => id, icon: 'x', mount() {} });
  }
  assert.deepEqual(workspace.tabOrder(), ['changes', 'history', 'terminal', 'plan', 'files']);
  assert.equal(workspace.paneKey(3), 'Alt+3');
  assert.equal(workspace.paneKey(9), '', 'only eight have a key');
  assert.equal(workspace.paneHints(), 'Alt+1 changes · Alt+2 history · Alt+3 terminal · Alt+4 plan · Alt+5 files');
});
