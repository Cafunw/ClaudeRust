// npm run test:ui — Continue on a cut-short answer.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { resumable } from '../js/parts.js';

test('Continue is offered on a stopped or failed answer that has text', () => {
  const text = { kind: 'text', html: '<p>half</p>' };
  assert.ok(resumable([text, { kind: 'notice', code: 'error' }]));
  assert.ok(resumable([text, { kind: 'notice', code: 'stopped' }]));
  assert.ok(!resumable([{ kind: 'notice', code: 'error' }]), 'nothing came: only Retry');
  assert.ok(!resumable([text, { kind: 'notice', code: 'max_tokens' }]), 'max tokens has its own Continue');
  assert.ok(!resumable([text]));
  assert.ok(!resumable([text, { kind: 'tool', status: 'running' }, { kind: 'notice', code: 'stopped' }]), 'stopped in a tool call: only Retry');
  assert.ok(resumable([text, { kind: 'tool', status: 'done' }, text, { kind: 'notice', code: 'stopped' }]), 'text after the tool call');
});
