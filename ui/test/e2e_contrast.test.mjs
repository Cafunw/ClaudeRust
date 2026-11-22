// The contrast check the e2e suite uses to tell whether artifact text is readable.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { contrast } from '../e2e/contrast.mjs';

const WHITE = 'rgb(255, 255, 255)';
const BLACK = 'rgb(0, 0, 0)';
const CLEAR = 'rgba(0, 0, 0, 0)';

test('black on white is 21:1 and a colour on itself 1:1', () => {
  assert.equal(contrast(BLACK, [WHITE]), 21);
  assert.equal(contrast(WHITE, [WHITE]), 1);
});

test('transparent backgrounds show the one under them', () => {
  // white text on a page with no background, in a white frame: unreadable
  assert.equal(contrast(WHITE, [CLEAR, CLEAR, WHITE]), 1);
  // the app's dark artifact background under it
  assert.ok(contrast(WHITE, [CLEAR, 'rgb(38, 38, 36)', WHITE]) > 14);
});

test('translucent colours are blended with what is under them', () => {
  // half-black on white is mid grey
  const grey = contrast(BLACK, ['rgba(0, 0, 0, 0.5)', WHITE]);
  assert.ok(grey > 4 && grey < 6, String(grey));
  // faint text on white
  assert.ok(contrast('rgba(0, 0, 0, 0.1)', [WHITE]) < 1.5);
});
