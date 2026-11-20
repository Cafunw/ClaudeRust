// Inline suggestions for the composer: "@" file mentions (code sessions)
// and "/" commands at the start of the message.
import { call } from './api.js';
import { icon } from './icons.js';
import { esc, h } from './util.js';
import { allCommands } from './slash.js';

/**
 * attachSuggest(textarea, { context: () => ({ kind, conversationId, cwd, engine }), anchor, onCommand })
 * Returns { isOpen(), close() } and handles keys itself while open.
 */
export function attachSuggest(ta, { context, anchor, onPick }) {
  let pop = null;
  let items = [];
  let sel = 0;
  let token = null; // { type: '@'|'/', start, end, query }
  let seq = 0;

  function close() {
    if (pop) pop.remove();
    pop = null;
    items = [];
    token = null;
  }

  function currentToken() {
    const pos = ta.selectionStart;
    if (pos !== ta.selectionEnd) return null;
    const before = ta.value.slice(0, pos);
    const ctx = context();
    // "/" command: only as the first thing in the message, no spaces yet
    const sm = /^\/([\w:.-]*)$/.exec(before);
    if (sm) return { type: '/', start: 0, end: pos, query: sm[1] };
    if (ctx.kind !== 'code') return null;
    const am = /(^|[\s(])@("?)([^\s"]*)$/.exec(before);
    if (am) return { type: '@', start: pos - am[3].length - am[2].length - 1, end: pos, query: am[3] };
    return null;
  }

  async function update() {
    const tok = currentToken();
    if (!tok) { close(); return; }
    token = tok;
    const my = ++seq;
    const ctx = context();
    let list = [];
    if (tok.type === '/') {
      const q = tok.query.toLowerCase();
      list = (await allCommands(ctx))
        .filter((c) => c.name.toLowerCase().includes(q))
        .sort((a, b) => (b.name.toLowerCase().startsWith(q) - a.name.toLowerCase().startsWith(q)) || a.name.localeCompare(b.name))
        .slice(0, 30)
        .map((c) => ({ label: '/' + c.name, sub: c.description, hint: c.hint, icon: c.kind === 'custom' ? 'fileText' : 'zap', value: c }));
    } else {
      try {
        const args = { conversationId: ctx.conversationId || null, cwd: ctx.cwd || null, query: tok.query };
        const [r, syms] = await Promise.all([
          call('file_search', args),
          tok.query.length >= 2 && !tok.query.includes('/') ? call('symbol_search', args).catch(() => []) : Promise.resolve([]),
        ]);
        list = r.slice(0, syms.length ? 10 : 30).map((f) => {
          const name = f.path.split('/').pop();
          return { label: name + (f.dir ? '/' : ''), sub: f.path, icon: f.dir ? 'folder' : 'file', value: f };
        });
        for (const sy of syms) list.push({ label: sy.name, hint: sy.kind, sub: `${sy.path}:${sy.line}`, icon: 'code', value: { path: `${sy.path}#L${sy.line}`, symbol: true } });
      } catch { list = []; }
    }
    if (my !== seq) return;
    items = list;
    sel = 0;
    render();
  }

  function render() {
    if (!items.length) { if (pop) pop.remove(); pop = null; return; }
    if (!pop) {
      pop = h('<div class="suggest-pop" role="listbox"></div>');
      (anchor || ta.parentNode).appendChild(pop);
    }
    pop.innerHTML = items.map((it, i) => `<div class="sg-item ${i === sel ? 'on' : ''}" data-i="${i}" role="option">${icon(it.icon, 14)}<span class="sg-label">${esc(it.label)}</span>${it.hint ? `<span class="sg-hint">${esc(it.hint)}</span>` : ''}<span class="sg-sub ellipsis">${esc(it.sub || '')}</span></div>`).join('');
    pop.querySelectorAll('.sg-item').forEach((el) => {
      el.addEventListener('mousedown', (e) => { e.preventDefault(); sel = +el.dataset.i; pick(); });
    });
    const on = pop.querySelector('.sg-item.on');
    if (on) on.scrollIntoView({ block: 'nearest' });
  }

  function pick() {
    const it = items[sel];
    if (!it || !token) return;
    const v = ta.value;
    let insert;
    if (token.type === '/') insert = `/${it.value.name} `;
    else {
      const p = it.value.path + (it.value.dir ? '/' : '');
      insert = /\s/.test(p) ? `@"${p}" ` : `@${p} `;
    }
    ta.value = v.slice(0, token.start) + insert + v.slice(token.end);
    const caret = token.start + insert.length;
    ta.setSelectionRange(caret, caret);
    const picked = { type: token.type, item: it.value };
    close();
    ta.dispatchEvent(new Event('input'));
    if (onPick) onPick(picked);
    if (picked.type === '@' && picked.item.dir) update();
  }

  ta.addEventListener('input', () => update());
  ta.addEventListener('click', () => update());
  ta.addEventListener('blur', () => setTimeout(close, 120));
  ta.addEventListener('keydown', (e) => {
    if (!pop || !items.length) return;
    if (e.key === 'ArrowDown') { e.preventDefault(); e.stopImmediatePropagation(); sel = (sel + 1) % items.length; render(); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); e.stopImmediatePropagation(); sel = (sel - 1 + items.length) % items.length; render(); }
    else if (e.key === 'Enter' || e.key === 'Tab') { e.preventDefault(); e.stopImmediatePropagation(); pick(); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopImmediatePropagation(); close(); }
  }, true);

  return { isOpen: () => !!pop, close };
}
