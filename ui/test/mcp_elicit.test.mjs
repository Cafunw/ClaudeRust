// npm run test:ui — the MCP elicitation form's answer (ui/js/mcp_elicit.js; the dialog needs a DOM).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { formContent, choicesOf, urlHtml } from '../js/mcp_elicit.js';

const schema = {
  type: 'object',
  properties: {
    name: { type: 'string' },
    age: { type: 'integer' },
    score: { type: 'number' },
    subscribe: { type: 'boolean' },
    plan: { type: 'string', enum: ['free', 'pro'] },
  },
  required: ['name'],
};

test('values are sent in the types the form asks for', () => {
  const r = formContent(schema, { name: ' Ada ', age: '36', score: '1.5', subscribe: true, plan: 'pro' });
  assert.deepEqual(r, { content: { name: ' Ada ', age: 36, score: 1.5, subscribe: true, plan: 'pro' }, missing: [], bad: [] });
});

test('empty fields are left out, and a required one is missing', () => {
  assert.deepEqual(formContent(schema, { name: '  ', age: '', plan: '' }), { content: { subscribe: false }, missing: ['name'], bad: [] });
});

test('a number field must hold a number, an integer field a whole one', () => {
  assert.deepEqual(formContent(schema, { name: 'x', age: '3.5', score: 'abc' }).bad, ['age', 'score']);
  assert.deepEqual(formContent(schema, { name: 'x', age: '-2', score: '1e3' }).content, { name: 'x', age: -2, score: 1000, subscribe: false });
});

test('a form without fields answers with nothing', () => {
  assert.deepEqual(formContent({ type: 'object', properties: {} }, {}), { content: {}, missing: [], bad: [] });
  assert.deepEqual(formContent(undefined, {}), { content: {}, missing: [], bad: [] });
});

test('choices: an enum (with the old enumNames), or titled ones (oneOf / anyOf)', () => {
  assert.deepEqual(choicesOf({ type: 'string', enum: ['a', 'b'], enumNames: ['A'] }), [['a', 'A'], ['b', 'b']]);
  assert.deepEqual(choicesOf({ type: 'string', oneOf: [{ const: 's', title: 'Small' }, { const: 'l' }, { title: 'no value' }] }), [['s', 'Small'], ['l', 'l']]);
  assert.deepEqual(choicesOf({ anyOf: [{ const: 'x', title: 'X' }] }), [['x', 'X']]);
  assert.equal(choicesOf({ type: 'string' }), null);
});

test('a list of choices is sent as the values ticked, within its bounds', () => {
  const s = { type: 'object', properties: { tags: { type: 'array', items: { enum: ['a', 'b', 'c'] }, minItems: 1, maxItems: 2 }, more: { type: 'array', items: { enum: ['x'] } } }, required: ['more'] };
  assert.deepEqual(formContent(s, { tags: ['a', 'c'], more: ['x'] }), { content: { tags: ['a', 'c'], more: ['x'] }, missing: [], bad: [] });
  assert.deepEqual(formContent(s, { tags: [], more: [] }), { content: {}, missing: ['more'], bad: ['tags'] });
  assert.deepEqual(formContent(s, { tags: ['a', 'b', 'c'], more: ['x'] }).bad, ['tags']);
});

test('a URL is shown with its host apart, look-alikes included', () => {
  assert.equal(urlHtml('https://auth.example.com/connect?u=auth.example.com', 'auth.example.com'), 'https://<b class="elicit-host">auth.example.com</b>/connect?u=auth.example.com');
  assert.equal(urlHtml('https://user@xn--pple-43d.com/a', 'xn--pple-43d.com'), 'https://user@<b class="elicit-host">xn--pple-43d.com</b>/a');
  assert.equal(urlHtml('https://a.example/<b>', 'other.example'), 'https://a.example/&lt;b&gt;');
});
