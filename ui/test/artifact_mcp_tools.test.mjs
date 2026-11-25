// npm run test:ui — window.claude.callMcpTool's gate (ui/js/artifact_bridge.js):
// what a frame may send, the per-artifact, per-connector approval, and a
// tool whose setting asks still asking under that approval.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createGrants, createToolCall, toolCallArgs, toolScope, toolError } from '../js/artifact_bridge.js';

test('a grant is asked once per scope and can be taken back', async () => {
  const answers = ['always', 'once', undefined, 'decline'];
  const asked = [];
  const g = createGrants(async (scope) => { asked.push(scope); return answers.shift(); });
  assert.equal(await g.decide('a|notes'), 'always', 'asked: the answer covers this call');
  assert.equal(await g.decide('a|notes'), 'granted');
  assert.equal(g.chosen('a|notes'), 'always');
  assert.equal(await g.decide('a|weather'), 'once');
  assert.equal(await g.decide('a|weather'), false, 'dismissed: declined this time only');
  assert.equal(g.chosen('a|weather'), undefined);
  assert.equal(await g.decide('a|weather'), false, 'declined');
  assert.equal(await g.decide('a|weather'), false, 'and remembered');
  assert.deepEqual(asked, ['a|notes', 'a|weather', 'a|weather', 'a|weather']);
  // revoked: asked again
  g.revoke('a|notes');
  answers.push('once');
  assert.equal(await g.decide('a|notes'), 'once');
  assert.equal(asked.length, 5);
});

test('a frame sends a tool name and plain JSON arguments, nothing else', () => {
  assert.deepEqual(toolCallArgs(['notes__add', { text: 'hi', n: [1, 2] }]), { name: 'notes__add', input: { text: 'hi', n: [1, 2] } });
  assert.deepEqual(toolCallArgs(['mcp__notes__add']), { name: 'mcp__notes__add', input: {} });
  assert.deepEqual(toolCallArgs(['x', null]).input, {});
  // what isn't JSON is dropped or turned into JSON
  const odd = toolCallArgs(['x', { when: new Date(0), m: new Map([[1, 2]]), f: undefined }]).input;
  assert.deepEqual(odd, { when: '1970-01-01T00:00:00.000Z', m: {} });
  const bad = (args, re) => assert.throws(() => toolCallArgs(args), re);
  for (const name of [undefined, 42, '', 'a b', '../x', 'x/y', 'x"y', 'é', 'a'.repeat(129), { toString: () => 'x' }]) bad([name, {}], /takes a tool name/);
  for (const args of [null, 'notes__add', { 0: 'notes__add' }, []]) bad(args, /takes a tool name/);
  for (const input of ['text', 42, true, ['a']]) bad(['x', input], /must be an object/);
  const loop = {}; loop.self = loop;
  bad(['x', loop], /plain JSON/);
  bad(['x', { n: 1n }], /plain JSON/);
  bad(['x', { s: 'x'.repeat(256 * 1024) }], /too large/);
});

function setup({ asks = false, refuse = null, answers = [], callAnswers = [] } = {}) {
  const log = [];
  const grants = createGrants(async (scope, info) => { log.push(['grant', scope, info.server, info.title]); return answers.shift(); });
  const call = createToolCall({
    check: async (f, name) => {
      log.push(['check', name]);
      if (refuse) throw new Error(refuse);
      return { server: name.split('__')[0], tool: name.split('__')[1], read_only: false, asks };
    },
    grants,
    askCall: async (f, c) => { log.push(['askCall', c.tool]); return callAnswers.shift(); },
    run: async (f, name, input, approved) => { log.push(['run', name, approved]); return { content: [{ type: 'text', text: 'ok' }], isError: false }; },
  });
  return { call, log, grants };
}

const frame = { scope: 'c1/notes-app', title: 'Notes', conv: 'c1', artifact: 'notes-app' };

test('a refused tool asks no one and runs nothing', async () => {
  const s = setup({ refuse: 'add is blocked in your connector settings.' });
  await assert.rejects(s.call(frame, ['notes__add', {}]), /blocked/);
  assert.deepEqual(s.log, [['check', 'notes__add']]);
  // and a hostile message is refused before the check
  const h = setup();
  await assert.rejects(h.call(frame, ['../etc', {}]), /tool name/);
  assert.deepEqual(h.log, []);
});

test('the approval is per artifact and connector; "Allow once" asks again next time', async () => {
  const s = setup({ answers: ['once', 'always'] });
  await s.call(frame, ['notes__add', { t: 1 }]);
  await s.call(frame, ['notes__list', {}]);
  await s.call(frame, ['notes__add', { t: 2 }]);
  assert.deepEqual(s.log.filter((e) => e[0] !== 'check'), [
    ['grant', toolScope('c1/notes-app', 'notes'), 'notes', 'Notes'], ['run', 'notes__add', true],
    ['grant', toolScope('c1/notes-app', 'notes'), 'notes', 'Notes'], ['run', 'notes__list', true],
    // allowed for this artifact: the tool's setting runs it (approved: false, nothing to approve)
    ['run', 'notes__add', false],
  ]);
  // another connector, and the same connector in another artifact, ask on their own
  s.log.length = 0;
  await assert.rejects(s.call(frame, ['weather__get', {}]), /did not allow this artifact to use weather/);
  await assert.rejects(s.call({ ...frame, scope: 'c1/other' }, ['notes__add', {}]), /did not allow/);
  assert.deepEqual(s.log.filter((e) => e[0] === 'grant').map((e) => e[1]), [toolScope('c1/notes-app', 'weather'), toolScope('c1/other', 'notes')]);
});

test('a tool whose setting asks still asks each call under an earlier approval', async () => {
  const s = setup({ asks: true, answers: ['always'], callAnswers: [true, false] });
  await s.call(frame, ['notes__add', {}]);
  await s.call(frame, ['notes__add', {}]);
  await assert.rejects(s.call(frame, ['notes__add', {}]), /did not allow this call of add/);
  assert.deepEqual(s.log.filter((e) => e[0] !== 'check'), [
    // the connector's question was this call's yes
    ['grant', toolScope('c1/notes-app', 'notes'), 'notes', 'Notes'], ['run', 'notes__add', true],
    ['askCall', 'add'], ['run', 'notes__add', true],
    ['askCall', 'add'],
  ]);
});

test('a declined connector is never asked about again and never runs', async () => {
  const s = setup({ answers: ['decline'] });
  for (let i = 0; i < 3; i++) await assert.rejects(s.call(frame, ['notes__add', {}]), /did not allow/);
  assert.equal(s.log.filter((e) => e[0] === 'grant').length, 1);
  assert.equal(s.log.filter((e) => e[0] === 'run').length, 0);
  assert.equal(s.grants.chosen(toolScope('c1/notes-app', 'notes')), 'decline');
});

test('a refusal reaches the artifact as an MCP result with isError', () => {
  assert.deepEqual(toolError(new Error('nope')), { content: [{ type: 'text', text: 'nope' }], isError: true });
  assert.equal(toolError('x'.repeat(5000)).content[0].text.length, 2000);
});

test('two artifacts never share a grant, whatever their names hold', () => {
  // artifact "a|b" with connector "c" against artifact "a" with a connector named "b|c"
  assert.notEqual(toolScope('c1/a|b', 'c'), toolScope('c1/a', 'b|c'));
  assert.notEqual(toolScope('c1/a","b', 'c'), toolScope('c1/a', 'b","c'));
});
