// npm run test:ui — the MCP elicitation form's answer (ui/js/mcp_elicit.js; the dialog needs a DOM).
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { formContent } from '../js/mcp_elicit.js';

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
