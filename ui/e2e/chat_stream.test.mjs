// End-to-end tests of the chat view while a reply streams, in a running dev
// copy of the app (see smoke.test.mjs for how it is started and skipped):
// the reveal of streamed text, message actions held up by a response, and
// web citations' cards. The API base URL points at a mock in this process and
// is restored afterwards.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { startMock, CITED } from './mock_api.mjs';
import { attach, PRELUDE, sleep } from './cdp.mjs';

const { cdp, skip } = await attach();

let mock = null;
let ev = null;
let restore = null;
const made = [];

async function waitFor(what, expr, ms = 20000) {
  const t0 = Date.now();
  let last;
  while (Date.now() - t0 < ms) {
    last = await ev(expr);
    if (last) return last;
    await sleep(50);
  }
  throw new Error(`timed out waiting for ${what}; last: ${JSON.stringify(last)}`);
}

const idle = () => waitFor('the turn to end', `!__e2e.running() && !__e2e.turns().some((t) => t.streaming)`, 40000);

async function newChat(text) {
  await ev(`__e2e.emit('navigate', { name: 'new' })`);
  await waitFor('the new chat composer', `!!document.querySelector('#view .welcome .composer textarea')`);
  assert.ok(await ev(`__e2e.type(${JSON.stringify(text)})`));
  made.push(await waitFor('an open conversation', `__e2e.store.route.name === 'chat' && __e2e.store.route.params.id`));
}

// where the covers over the answer's words not uncovered yet stand (ui/js/stream_reveal.js)
const CLIPS = `[...document.querySelectorAll('#view .reveal-cover:not([hidden])')].map((c) => c.children[0].style.transform)`;
const COVERS = `document.querySelectorAll('#view .reveal-cover').length`;

before(async () => {
  if (skip) return;
  ev = cdp.ev;
  mock = await startMock();
  await ev(PRELUDE);
  const settings = await ev(`__e2e.invoke('get_settings')`);
  const patch = { base_url: mock.url, chat_engine: 'api', notifications: false };
  const old = Object.fromEntries(Object.keys(patch).map((k) => [k, settings[k]]));
  restore = () => ev(`__e2e.invoke('save_settings', { patch: ${JSON.stringify(old)} })`);
  await ev(`__e2e.invoke('save_settings', { patch: ${JSON.stringify(patch)} })`);
  await cdp.send('Page.reload', { ignoreCache: true });
  await sleep(500);
  await waitFor('the app to start', `document.readyState === 'complete' && !!document.querySelector('#view .composer, #view .chat-col')`, 30000);
  await ev(PRELUDE);
});

after(async () => {
  if (skip) return;
  try {
    await cdp.send('Emulation.clearDeviceMetricsOverride');
    await cdp.send('Emulation.setEmulatedMedia', { features: [] });
    await cdp.send('Emulation.setFocusEmulationEnabled', { enabled: false });
    for (const id of made) await ev(`__e2e.invoke('delete_conversation', { id: ${JSON.stringify(id)} })`).catch(() => {});
    if (restore) await restore();
    await cdp.send('Page.reload', { ignoreCache: true });
  } finally {
    cdp.close();
    await mock.close();
  }
});

test('a streamed reply is uncovered in steps and shows whole the moment it ends', { skip, timeout: 60000 }, async () => {
  await newChat('long reply please');
  const seen = new Set();
  while (await ev('__e2e.running()')) {
    for (const c of await ev(CLIPS)) seen.add(c);
    await sleep(15);
  }
  assert.ok(seen.size > 20, `the reveal moved in steps (${seen.size} clips seen)`);
  assert.ok([...seen].every((c) => c.startsWith('translate(')));
  await idle();
  assert.equal(await ev(COVERS), 0, 'nothing covered after the end');
  assert.match(await ev(`__e2e.turns().at(-1).text`), /fn part_11/, 'the last part is there');
});

test('streamed thinking is uncovered the same way, and shows whole once it folds', { skip, timeout: 60000 }, async () => {
  await newChat('think it over');
  let covered = 0;
  while (await ev('__e2e.running()')) {
    if (await ev(`!!document.querySelector('#view .thinking.open') && document.querySelectorAll('#view .reveal-cover:not([hidden])').length > 0`)) covered++;
    await sleep(15);
  }
  assert.ok(covered > 0, 'covered while it thought');
  await idle();
  assert.equal(await ev(COVERS), 0);
  assert.ok(await ev(`!!document.querySelector('#view .thinking:not(.open)')`), 'folded');
  assert.match(await ev(`__e2e.turns().at(-1).text`), /the answer is 42\./);
});

test('Stop shows what had arrived at once, and reduced motion shows every batch at once', { skip, timeout: 60000 }, async () => {
  await newChat('slow e2e');
  await waitFor('some words', `__e2e.turns().at(-1).text.includes('w5 ')`);
  await ev(`document.querySelector('#view .composer [data-act=send].stop').click()`);
  await idle();
  assert.equal(await ev(COVERS), 0);
  await cdp.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
  await newChat('slow e2e');
  let clipped = 0;
  for (let i = 0; i < 40; i++) { clipped += (await ev(CLIPS)).length; await sleep(20); }
  assert.equal(clipped, 0, 'no reveal');
  await ev(`document.querySelector('#view .composer [data-act=send].stop').click()`);
  await idle();
  await cdp.send('Emulation.setEmulatedMedia', { features: [] });
});

test('Edit while Claude responds says to stop first and offers Stop', { skip, timeout: 60000 }, async () => {
  await newChat('slow e2e');
  await waitFor('the reply', `__e2e.turns().at(-1).text.includes('w3 ')`);
  const busy = await ev(`__e2e.t('busy_edit')`);
  const edit = `document.querySelector('#view .turn.user .turn-actions [data-busy=edit]')`;
  assert.deepEqual(await ev(`[${edit}.getAttribute('aria-disabled'), ${edit}.title]`), ['true', busy], 'held up, and the tooltip says why');
  await ev(`${edit}.click()`);
  assert.equal(await ev(`!!document.querySelector('#view .edit-box')`), false, 'no edit box');
  const toast = await waitFor('the toast', `[...document.querySelectorAll('.toast')].map((t) => t.textContent).find((s) => s.includes(${JSON.stringify(busy)}))`);
  assert.ok(toast.includes(await ev(`__e2e.t('stop')`)), toast);
  await ev(`[...document.querySelectorAll('.toast')].find((t) => t.textContent.includes(${JSON.stringify(busy)})).querySelector('.toast-action').click()`);
  await idle();
  assert.deepEqual(await ev(`[${edit}.getAttribute('aria-disabled'), ${edit}.title]`), [null, await ev(`__e2e.t('edit')`)], 'usable again');
  await ev(`${edit}.click()`);
  await waitFor('the edit box', `!!document.querySelector('#view .edit-box textarea')`);
  await ev(`document.querySelector('#view .edit-box [data-a=cancel]').click()`);
});

test('a web citation shows its title, domain and passage on hover, focus and tap', { skip, timeout: 60000 }, async () => {
  await newChat('cite this');
  await idle();
  const chip = `document.querySelector('#view .md a.cite')`;
  assert.equal(await ev(`document.querySelectorAll('#view .md a.cite').length`), 2);
  assert.equal(await ev(`${chip}.getAttribute('data-quote')`), CITED.text, 'the passage, as text');
  assert.equal(await ev(`${chip}.title`), '', 'no native tooltip on top of the card');
  const card = `document.querySelector('.cite-card')`;
  const cardText = `${card} && [${card}.querySelector('.cite-title').textContent, ${card}.querySelector('.cite-domain').textContent, ${card}.querySelector('.cite-quote').textContent]`;
  // hover
  await ev(`${chip}.dispatchEvent(new PointerEvent('pointerover', { bubbles: true, pointerType: 'mouse' }))`);
  assert.deepEqual(await ev(cardText), [CITED.title, 'blog.rust-lang.org', CITED.text]);
  assert.equal(await ev(`${chip}.getAttribute('aria-describedby')`), 'cite-card');
  await ev(`document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`);
  assert.equal(await ev(`!!${card}`), false, 'Escape closes it');
  assert.equal(await ev('__e2e.running()'), false);
  // keyboard focus, then leaving it (the test copy's window is never focused, and a page that isn't fires
  // no focus events: it is told it is, as a person's window would be)
  await cdp.send('Emulation.setFocusEmulationEnabled', { enabled: true });
  await ev(`${chip}.focus()`);
  assert.ok(await ev(`!!${card}`), 'focus shows it');
  await ev(`document.querySelector('#view .composer textarea').focus()`);
  await waitFor('the card to close', `!${card}`, 2000);
  // a tap (no hover first) only shows the card; with it up, the next goes on to open the page. Asked of
  // the link handling directly: a click here would reach open_external, which opens the system browser
  // (and __TAURI__ is frozen, so that can't be stubbed)
  const tap = await ev(`import('./js/cite_card.js').then((m) => { const a = ${chip}; const first = m.citeClick(a); return [first, !!document.querySelector('.cite-card'), m.citeClick(a)]; })`);
  assert.deepEqual(tap, [true, true, false]);
  await ev(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))`);
  assert.equal(await ev(`!!${card}`), false, 'a press elsewhere closes it');
  // at 420 px, in both themes, the card stays in the window and keeps its colours
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 420, height: 760, deviceScaleFactor: 1, mobile: false });
  for (const dark of [false, true]) {
    await cdp.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: dark ? 'dark' : 'light' }] });
    const last = `[...document.querySelectorAll('#view .md a.cite')].pop()`;
    await ev(`${last}.scrollIntoView({ block: 'center' })`);
    await sleep(100);
    await ev(`${last}.dispatchEvent(new PointerEvent('pointerover', { bubbles: true, pointerType: 'mouse' }))`);
    const box = await ev(`(() => { const r = ${card}.getBoundingClientRect(); const cs = getComputedStyle(${card}); return { l: r.left, r: r.right, t: r.top, b: r.bottom, w: innerWidth, h: innerHeight, bg: cs.backgroundColor, fg: cs.color, quote: ${card}.querySelector('.cite-quote').textContent.length }; })()`);
    assert.ok(box.l >= 8 && box.r <= box.w - 8 && box.t >= 0 && box.b <= box.h, JSON.stringify(box));
    assert.notEqual(box.bg, box.fg);
    assert.ok(box.quote <= 301, `the long passage is capped (${box.quote})`);
    await ev(`${last}.dispatchEvent(new PointerEvent('pointerout', { bubbles: true, pointerType: 'mouse' }))`);
    await waitFor('the card to close when the pointer leaves', `!${card}`, 2000);
  }
  // drawn again from the history, the chips keep their passages
  await cdp.send('Page.reload', { ignoreCache: true });
  await sleep(500);
  await waitFor('the app to start', `document.readyState === 'complete' && !!document.querySelector('#view .composer, #view .chat-col')`, 30000);
  await ev(PRELUDE);
  await ev(`__e2e.emit('navigate', { name: 'chat', params: { id: ${JSON.stringify(made.at(-1))} } })`);
  await waitFor('the chat again', `!!document.querySelector('#view .md a.cite')`, 30000);
  assert.equal(await ev(`document.querySelector('#view .md a.cite').getAttribute('data-quote')`), CITED.text);
});

test('no uncaught errors in the page', { skip }, () => {
  assert.deepEqual(cdp.exceptions, []);
});
