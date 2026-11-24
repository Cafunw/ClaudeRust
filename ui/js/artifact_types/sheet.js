// Sheet artifacts (application/vnd.ant.sheet): CSV with typed cells and
// formulas (=SUM(A1:A3), IF, cell references), drawn as a grid that sorts and
// edits in place. Runs in the artifact's frame (artifacts.rs build_typed);
// the pure part is imported by the gallery and ui/test too. No eval: formulas
// go through the parser below, and every limit a hostile sheet could push
// (length, nesting, range size, reference chains) ends in an error value.

const MAX_FORMULA = 2000;
const MAX_NEST = 64;
const MAX_RANGE = 100000;
// range cells walked in one sheet: many large ranges each fit MAX_RANGE but would hang the page together (the gallery evaluates in the app's)
const MAX_WORK = 20000000;
const MAX_CHAIN = 400;

/** RFC 4180 CSV: quoted fields with "" and line breaks inside. An unquoted
 *  formula keeps the commas of its calls and strings (=IF(A1>1,"a","b")), as
 *  models write them. */
export function parseCsv(text) {
  const rows = [];
  let row = [], field = '', q = false, depth = 0, str = false;
  const s = String(text).replace(/^﻿/, '');
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (q) {
      if (c === '"') { if (s[i + 1] === '"') { field += '"'; i++; } else q = false; } else field += c;
    } else if (c === '"' && field === '') q = true;
    else if (field[0] === '=' && (str || depth > 0) && c !== '\n' && c !== '\r') {
      if (c === '"') str = !str;
      else if (!str) depth += c === '(' ? 1 : c === ')' ? -1 : 0;
      field += c;
    } else if (field[0] === '=' && c === '"') { str = true; field += c; }
    else if (field[0] === '=' && c === '(') { depth = 1; field += c; }
    else if (c === ',') { row.push(field); field = ''; depth = 0; str = false; }
    else if (c === '\n' || c === '\r') {
      if (c === '\r' && s[i + 1] === '\n') i++;
      row.push(field); rows.push(row); row = []; field = ''; depth = 0; str = false;
    } else field += c;
  }
  if (field !== '' || row.length) { row.push(field); rows.push(row); }
  return rows;
}

export function toCsv(rows) {
  const cell = (v) => (/[",\r\n]|^\s|\s$/.test(v) ? `"${v.replace(/"/g, '""')}"` : v);
  // trailing empty cells and rows are layout, not data
  const out = rows.map((r) => { const t = r.slice(); while (t.length && t[t.length - 1] === '') t.pop(); return t.map(cell).join(','); });
  while (out.length && out[out.length - 1] === '') out.pop();
  return out.join('\n');
}

/** Column name of index `i` (0 → A, 26 → AA). */
export function colName(i) {
  let s = '';
  for (let n = i + 1; n > 0; n = Math.floor((n - 1) / 26)) s = String.fromCharCode(65 + ((n - 1) % 26)) + s;
  return s;
}

function colIndex(letters) {
  let n = 0;
  for (const ch of letters.toUpperCase()) n = n * 26 + (ch.charCodeAt(0) - 64);
  return n - 1;
}

const NUM = /^[-+]?(?:\d{1,3}(?:,\d{3})+|\d+)?(?:\.\d+)?(?:[eE][-+]?\d+)?$/;
const CUR = /^([-+]?)([$€£¥₺])\s?(.+)$|^([-+]?)(.+?)\s?([$€£¥₺])$/;

/** What a cell holds: formula, number, currency, percent, date, bool, text or empty. */
export function cellType(raw) {
  const s = String(raw).trim();
  if (!s) return 'empty';
  if (s[0] === '=' && s.length > 1) return 'formula';
  if (/^(true|false)$/i.test(s)) return 'bool';
  if (/^\d{4}-\d{2}-\d{2}(?:[ T]\d{2}:\d{2}(?::\d{2})?)?$/.test(s)) return 'date';
  if (/\d/.test(s) && NUM.test(s)) return 'number';
  if (/^[-+]?[\d.,]+\s?%$/.test(s) && NUM.test(s.replace(/\s?%$/, ''))) return 'percent';
  const m = s.match(CUR);
  if (m && NUM.test(m[3] || m[5]) && /\d/.test(m[3] || m[5])) return 'currency';
  return 'text';
}

/** The value of a cell that isn't a formula. */
export function literal(raw) {
  const s = String(raw).trim();
  switch (cellType(s)) {
    case 'empty': return '';
    case 'bool': return /^true$/i.test(s);
    case 'number': return Number(s.replace(/,/g, ''));
    case 'percent': return Number(s.replace(/[\s%,]/g, '')) / 100;
    case 'currency': { const m = s.match(CUR); return Number(((m[2] ? m[1] : m[4]) + (m[2] ? m[3] : m[5])).replace(/,/g, '')); }
    default: return s;
  }
}

const err = (code) => ({ error: code });
export const isErr = (v) => v !== null && typeof v === 'object' && 'error' in v;

// ----------------------------------------------------------------- parser

const TOKEN = /\s*(?:(\d+(?:\.\d+)?(?:[eE][-+]?\d+)?|\.\d+)|("(?:[^"]|"")*")|(\$?[A-Za-z]{1,3}\$?\d{1,7}(?::\$?[A-Za-z]{1,3}\$?\d{1,7})?)(?!\s*\()|([A-Za-z_][A-Za-z0-9_.]*)|(<=|>=|<>|[-+*/^&=<>(),%]))/y;

function tokenize(src) {
  const out = [];
  TOKEN.lastIndex = 0;
  let at = 0;
  while (at < src.length) {
    if (/^\s*$/.test(src.slice(at))) break;
    TOKEN.lastIndex = at;
    const m = TOKEN.exec(src);
    if (!m) throw new Error('#ERROR!');
    at = TOKEN.lastIndex;
    if (m[1] !== undefined) out.push({ t: 'num', v: Number(m[1]) });
    else if (m[2] !== undefined) out.push({ t: 'str', v: m[2].slice(1, -1).replace(/""/g, '"') });
    else if (m[3] !== undefined) out.push({ t: 'ref', v: m[3].replace(/\$/g, '').toUpperCase() });
    else if (m[4] !== undefined) out.push({ t: 'name', v: m[4].toUpperCase() });
    else out.push({ t: 'op', v: m[5] });
  }
  return out;
}

function refNode(s) {
  const [a, b] = s.split(':');
  const one = (x) => { const m = x.match(/^([A-Z]+)(\d+)$/); return { c: colIndex(m[1]), r: Number(m[2]) - 1 }; };
  const p = one(a);
  if (!b) return { t: 'ref', ...p };
  const q = one(b);
  return { t: 'range', r1: Math.min(p.r, q.r), c1: Math.min(p.c, q.c), r2: Math.max(p.r, q.r), c2: Math.max(p.c, q.c) };
}

/** The tree of formula `src` (without its `=`); throws an error code. */
export function parseFormula(src) {
  if (src.length > MAX_FORMULA) throw new Error('#ERROR!');
  const toks = tokenize(src);
  let i = 0, depth = 0;
  const peek = () => toks[i];
  const isOp = (v) => peek() && peek().t === 'op' && peek().v === v;
  const take = (v) => { if (!isOp(v)) throw new Error('#ERROR!'); i++; };
  const level = (ops, next) => () => {
    let a = next();
    while (peek() && peek().t === 'op' && ops.includes(peek().v)) { const op = toks[i++].v; a = { t: 'bin', op, a, b: next() }; }
    return a;
  };
  const primary = () => {
    if (++depth > MAX_NEST) throw new Error('#ERROR!');
    try {
      const k = toks[i++];
      if (!k) throw new Error('#ERROR!');
      if (k.t === 'num') return { t: 'val', v: k.v };
      if (k.t === 'str') return { t: 'val', v: k.v };
      if (k.t === 'ref') return refNode(k.v);
      if (k.t === 'op' && (k.v === '-' || k.v === '+')) { const a = postfix(); return k.v === '-' ? { t: 'neg', a } : a; }
      if (k.t === 'op' && k.v === '(') { const e = expr(); take(')'); return e; }
      if (k.t === 'name') {
        if ((k.v === 'TRUE' || k.v === 'FALSE') && !isOp('(')) return { t: 'val', v: k.v === 'TRUE' };
        take('(');
        const args = [];
        if (!isOp(')')) { args.push(expr()); while (isOp(',')) { i++; args.push(expr()); } }
        take(')');
        return { t: 'fn', name: k.v, args };
      }
      throw new Error('#ERROR!');
    } finally { depth--; }
  };
  const postfix = () => { let a = primary(); while (isOp('%')) { i++; a = { t: 'pct', a }; } return a; };
  const power = level(['^'], postfix);
  const mul = level(['*', '/'], power);
  const add = level(['+', '-'], mul);
  const cat = level(['&'], add);
  const expr = level(['=', '<>', '<', '>', '<=', '>='], cat);
  const tree = expr();
  if (i < toks.length) throw new Error('#ERROR!');
  return tree;
}

// -------------------------------------------------------------- evaluation

function num(v) {
  if (isErr(v)) return v;
  if (v === '' || v === null || v === undefined) return 0;
  if (typeof v === 'boolean') return v ? 1 : 0;
  if (typeof v === 'number') return v;
  const t = cellType(v);
  return ['number', 'percent', 'currency'].includes(t) ? literal(v) : err('#VALUE!');
}

function truth(v) {
  if (isErr(v)) return v;
  if (typeof v === 'boolean') return v;
  if (typeof v === 'number') return v !== 0;
  if (v === '') return false;
  if (/^true$/i.test(v)) return true;
  if (/^false$/i.test(v)) return false;
  return err('#VALUE!');
}

const finite = (x) => (Number.isFinite(x) ? x : err('#NUM!'));

/** The values a function's arguments give: ranges spread (their text and empty cells left out). */
function numbers(args, ev) {
  const out = [];
  for (const a of args) {
    if (a.t === 'range') {
      for (const v of ev.range(a)) { if (isErr(v)) return v; if (typeof v === 'number') out.push(v); }
    } else {
      const v = ev.node(a);
      if (Array.isArray(v)) return err('#VALUE!');
      if (isErr(v)) return v;
      // a referenced cell counts when it holds a number; a value written in the call is made one
      if (a.t === 'ref') { if (typeof v === 'number') out.push(v); continue; }
      const n = num(v);
      if (isErr(n)) return n;
      out.push(n);
    }
  }
  return out;
}

const FUNCS = new Map(Object.entries({
  SUM: (args, ev) => { const n = numbers(args, ev); return isErr(n) ? n : finite(n.reduce((s, x) => s + x, 0)); },
  AVERAGE: (args, ev) => { const n = numbers(args, ev); if (isErr(n)) return n; return n.length ? finite(n.reduce((s, x) => s + x, 0) / n.length) : err('#DIV/0!'); },
  MIN: (args, ev) => { const n = numbers(args, ev); return isErr(n) ? n : n.length ? Math.min(...n) : 0; },
  MAX: (args, ev) => { const n = numbers(args, ev); return isErr(n) ? n : n.length ? Math.max(...n) : 0; },
  COUNT: (args, ev) => { const n = numbers(args, ev); return isErr(n) ? n : n.length; },
  IF: (args, ev) => {
    if (args.length < 2 || args.length > 3) return err('#N/A');
    const c = truth(ev.node(args[0]));
    if (isErr(c)) return c;
    // only the branch taken is worked out
    return c ? ev.node(args[1]) : args.length > 2 ? ev.node(args[2]) : false;
  },
  ROUND: (args, ev) => {
    const x = num(ev.node(args[0])), d = args[1] ? num(ev.node(args[1])) : 0;
    if (isErr(x)) return x; if (isErr(d)) return d;
    const f = 10 ** Math.max(-15, Math.min(15, Math.trunc(d)));
    return finite(Math.round(x * f) / f);
  },
  ABS: (args, ev) => { const x = num(ev.node(args[0])); return isErr(x) ? x : Math.abs(x); },
  AND: (args, ev) => { for (const a of args) { const v = truth(ev.node(a)); if (isErr(v) || !v) return v; } return true; },
  OR: (args, ev) => { for (const a of args) { const v = truth(ev.node(a)); if (isErr(v) || v) return v; } return false; },
  NOT: (args, ev) => { const v = truth(ev.node(args[0])); return isErr(v) ? v : !v; },
}));

/** The function names formulas may call. */
export const FUNCTIONS = [...FUNCS.keys()];

function compare(op, a, b) {
  const na = typeof a === 'number' || typeof a === 'boolean' ? Number(a) : null;
  const nb = typeof b === 'number' || typeof b === 'boolean' ? Number(b) : null;
  const [x, y] = na !== null && nb !== null ? [na, nb] : [String(a).toLowerCase(), String(b).toLowerCase()];
  switch (op) {
    case '=': return x === y;
    case '<>': return x !== y;
    case '<': return x < y;
    case '>': return x > y;
    case '<=': return x <= y;
    default: return x >= y;
  }
}

/** Every cell's value: a number, string, boolean or `{error}`. */
export function evaluate(rows) {
  const cache = new Map();
  const busy = new Set();
  const trees = new Map();
  let chain = 0, work = 0;
  const cell = (r, c) => {
    const key = r * 4194304 + c;
    if (cache.has(key)) return cache.get(key);
    const raw = (rows[r] && rows[r][c]) || '';
    if (cellType(raw) !== 'formula') { const v = literal(raw); cache.set(key, v); return v; }
    if (busy.has(key)) return err('#CYCLE!');
    // a chain this deep is refused rather than overflow the stack
    if (chain >= MAX_CHAIN) return err('#DEPTH!');
    busy.add(key); chain++;
    let v;
    try {
      let tree = trees.get(raw);
      if (!tree) { tree = parseFormula(String(raw).trim().slice(1)); trees.set(raw, tree); }
      v = ev.node(tree);
      if (Array.isArray(v)) v = err('#VALUE!');
    } catch (e) { v = err(String(e.message).startsWith('#') ? e.message : '#ERROR!'); }
    busy.delete(key); chain--;
    // a value that leans on one still being worked out is worked out again from the top
    if (!(isErr(v) && v.error === '#DEPTH!')) cache.set(key, v);
    return v;
  };
  const ev = {
    range: (n) => {
      if ((n.r2 - n.r1 + 1) * (n.c2 - n.c1 + 1) > MAX_RANGE) return [err('#REF!')];
      const out = [];
      const last = Math.min(n.r2, rows.length - 1);
      work += Math.max(0, last - n.r1 + 1) * (n.c2 - n.c1 + 1);
      if (work > MAX_WORK) return [err('#REF!')];
      for (let r = n.r1; r <= last; r++) for (let c = n.c1; c <= n.c2; c++) out.push(cell(r, c));
      return out;
    },
    node: (n) => {
      switch (n.t) {
        case 'val': return n.v;
        case 'ref': return cell(n.r, n.c);
        case 'range': return ev.range(n);
        case 'neg': { const x = num(ev.node(n.a)); return isErr(x) ? x : -x; }
        case 'pct': { const x = num(ev.node(n.a)); return isErr(x) ? x : x / 100; }
        case 'fn': {
          const f = FUNCS.get(n.name);
          return f ? f(n.args, ev) : err('#NAME?');
        }
        default: {
          const a = ev.node(n.a), b = ev.node(n.b);
          if (Array.isArray(a) || Array.isArray(b)) return err('#VALUE!');
          if (isErr(a)) return a;
          if (isErr(b)) return b;
          if (n.op === '&') return fmtPlain(a) + fmtPlain(b);
          if (['=', '<>', '<', '>', '<=', '>='].includes(n.op)) return compare(n.op, a === '' ? 0 : a, b === '' ? 0 : b);
          const x = num(a), y = num(b);
          if (isErr(x)) return x;
          if (isErr(y)) return y;
          if (n.op === '/' && y === 0) return err('#DIV/0!');
          return finite(n.op === '+' ? x + y : n.op === '-' ? x - y : n.op === '*' ? x * y : n.op === '/' ? x / y : x ** y);
        }
      }
    },
  };
  // row by row: a running total down a long column then never recurses deep;
  // what still ran too deep is worked out again from the bottom up
  const out = rows.map((row, r) => row.map((_, c) => cell(r, c)));
  for (let r = out.length - 1; r >= 0; r--) {
    for (let c = out[r].length - 1; c >= 0; c--) if (isErr(out[r][c]) && out[r][c].error === '#DEPTH!') out[r][c] = cell(r, c);
  }
  return out;
}

function fmtPlain(v) {
  if (typeof v === 'boolean') return v ? 'TRUE' : 'FALSE';
  if (typeof v === 'number') return String(Math.round(v * 1e10) / 1e10);
  return String(v);
}

/** How a cell shows: its own text for a value typed in, the result for a formula. */
export function display(raw, v, lang = 'en') {
  if (cellType(raw) !== 'formula') return String(raw);
  if (isErr(v)) return v.error;
  if (typeof v === 'number') return v.toLocaleString(lang === 'tr' ? 'tr-TR' : 'en-US', { maximumFractionDigits: 6 });
  return fmtPlain(v);
}

/** Whether the first row names the columns: all text, over a row that isn't. */
export function hasHeader(rows) {
  if (rows.length < 2) return false;
  const kinds = (r) => r.map(cellType).filter((k) => k !== 'empty');
  const top = kinds(rows[0]);
  return top.length > 0 && top.every((k) => k === 'text') && kinds(rows[1]).some((k) => k !== 'text');
}

/** The order rows are shown in, sorted by column `col` (`dir` 1 or -1; 0 keeps
 *  them as they are), the header row first. The data itself keeps its order. */
export function sortedRows(rows, values, col, dir) {
  const idx = rows.map((_, i) => i);
  if (!dir) return idx;
  const head = hasHeader(rows) ? 1 : 0;
  const key = (i) => { const v = values[i] ? values[i][col] : ''; return v === undefined || v === '' || isErr(v) ? null : v; };
  const body = idx.slice(head).sort((a, b) => {
    const x = key(a), y = key(b);
    if (x === null || y === null) return x === y ? a - b : x === null ? 1 : -1;
    const c = typeof x === 'number' && typeof y === 'number' ? x - y : String(x).localeCompare(String(y), undefined, { numeric: true, sensitivity: 'base' });
    return c * dir || a - b;
  });
  return [...idx.slice(0, head), ...body];
}

// ---------------------------------------------------------------- the frame

const STRINGS = {
  en: { save: 'Save', saved: 'Saved as a new version', unsaved: 'Unsaved changes', row: '+ Row', col: '+ Column', sort: 'Sort by column {c}', cell: 'Cell', bad: 'This sheet couldn’t be read.', discard: 'Discard' },
  tr: { save: 'Kaydet', saved: 'Yeni sürüm olarak kaydedildi', unsaved: 'Kaydedilmemiş değişiklikler', row: '+ Satır', col: '+ Sütun', sort: '{c} sütununa göre sırala', cell: 'Hücre', bad: 'Bu tablo okunamadı.', discard: 'Vazgeç' },
};

function boot(content) {
  const lang = /(^|[#&])lang=tr\b/.test(location.hash) ? 'tr' : 'en';
  const S = STRINGS[lang];
  const editable = window.parent !== window;
  let rows = parseCsv(content);
  if (!rows.length) rows = [['']];
  let values = evaluate(rows);
  let sort = { col: -1, dir: 0 };
  let sel = { r: 0, c: 0 };
  let dirty = false;
  document.body.insertAdjacentHTML('afterbegin', `<div class="tb"><button data-a="row"></button><button data-a="col"></button><span class="grow"></span><span class="status" role="status"></span><button data-a="discard" hidden></button><button data-a="save" class="primary" disabled></button></div>
    <div class="fbar"><span class="ref"></span><input aria-label="${S.cell}" spellcheck="false"></div><div class="sheet-wrap"><table class="grid" role="grid"></table></div>`);
  const $ = (q) => document.querySelector(q);
  const tb = $('.tb'), table = $('table.grid'), fin = $('.fbar input'), status = $('.tb .status');
  tb.querySelector('[data-a=row]').textContent = S.row;
  tb.querySelector('[data-a=col]').textContent = S.col;
  tb.querySelector('[data-a=save]').textContent = S.save;
  tb.querySelector('[data-a=discard]').textContent = S.discard;
  if (!editable) { tb.hidden = true; fin.readOnly = true; }
  const width = () => Math.max(1, ...rows.map((r) => r.length));
  const raw = (r, c) => (rows[r] && rows[r][c]) || '';
  const setDirty = (on) => {
    dirty = on;
    tb.querySelector('[data-a=save]').disabled = !on;
    tb.querySelector('[data-a=discard]').hidden = !on;
    status.textContent = on ? S.unsaved : '';
  };
  const set = (r, c, v) => {
    while (rows.length <= r) rows.push([]);
    while (rows[r].length <= c) rows[r].push('');
    if (rows[r][c] === v) return;
    rows[r][c] = v;
    values = evaluate(rows);
    setDirty(true);
  };
  const draw = () => {
    const w = width(), order = sortedRows(rows, values, sort.col, sort.dir), head = hasHeader(rows);
    let html = '<thead><tr><th></th>';
    for (let c = 0; c < w; c++) {
      const mark = sort.col === c ? (sort.dir > 0 ? ' ▲' : ' ▼') : '';
      html += `<th aria-sort="${sort.col === c ? (sort.dir > 0 ? 'ascending' : 'descending') : 'none'}"><button data-sort="${c}" title="${S.sort.replace('{c}', colName(c))}">${colName(c)}${mark}</button></th>`;
    }
    html += '</tr></thead><tbody>';
    for (const r of order) {
      html += `<tr><th>${r + 1}</th>`;
      for (let c = 0; c < w; c++) {
        const s = raw(r, c), v = values[r] ? values[r][c] : '', k = cellType(s);
        const cls = ['grid-cell', isErr(v) ? 'err' : '', typeof v === 'number' && k !== 'date' ? 'num' : '', k === 'bool' ? 'bool' : '', head && r === 0 ? 'head' : '', sel.r === r && sel.c === c ? 'sel' : ''].filter(Boolean).join(' ');
        const text = display(s, v, lang);
        html += `<td class="${cls}" data-r="${r}" data-c="${c}" role="gridcell" tabindex="${sel.r === r && sel.c === c ? 0 : -1}" title="${esc(s)}">${esc(text)}</td>`;
      }
      html += '</tr>';
    }
    table.innerHTML = html + '</tbody>';
    $('.fbar .ref').textContent = colName(sel.c) + (sel.r + 1);
    if (document.activeElement !== fin) fin.value = raw(sel.r, sel.c);
  };
  const cellEl = () => table.querySelector(`td[data-r="${sel.r}"][data-c="${sel.c}"]`);
  const focusSel = () => { const el = cellEl(); if (el) { el.focus(); el.scrollIntoView({ block: 'nearest', inline: 'nearest' }); } };
  const move = (dr, dc) => {
    const order = sortedRows(rows, values, sort.col, sort.dir);
    const at = order.indexOf(sel.r);
    const next = order[Math.max(0, Math.min(order.length - 1, at + dr))];
    sel = { r: next === undefined ? sel.r : next, c: Math.max(0, Math.min(width() - 1, sel.c + dc)) };
    draw(); focusSel();
  };
  const edit = (initial) => {
    if (!editable) return;
    const el = cellEl();
    if (!el) return;
    const input = document.createElement('input');
    input.value = initial === undefined ? raw(sel.r, sel.c) : initial;
    input.spellcheck = false;
    el.textContent = '';
    el.appendChild(input);
    input.focus();
    let done = false;
    const finish = (keep, dr, dc) => {
      if (done) return;
      done = true;
      if (keep) set(sel.r, sel.c, input.value);
      draw();
      if (dr || dc) move(dr, dc); else focusSel();
    };
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); finish(true, 1, 0); }
      else if (e.key === 'Tab') { e.preventDefault(); finish(true, 0, e.shiftKey ? -1 : 1); }
      else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); finish(false); }
    });
    input.addEventListener('blur', () => finish(true));
  };
  const save = () => {
    if (!dirty || !editable) return;
    window.parent.postMessage({ __artifact: 'edit', content: toCsv(rows) }, '*');
    setDirty(false);
    status.textContent = S.saved;
  };
  table.addEventListener('click', (e) => {
    const sortB = e.target.closest('[data-sort]');
    if (sortB) {
      const c = Number(sortB.dataset.sort);
      sort = sort.col !== c ? { col: c, dir: 1 } : sort.dir > 0 ? { col: c, dir: -1 } : { col: -1, dir: 0 };
      draw();
      const b = table.querySelector(`[data-sort="${c}"]`);
      if (b) b.focus();
      return;
    }
    const td = e.target.closest('td[data-r]');
    if (td) { sel = { r: Number(td.dataset.r), c: Number(td.dataset.c) }; draw(); focusSel(); }
  });
  table.addEventListener('dblclick', (e) => { if (e.target.closest('td[data-r]')) edit(); });
  table.addEventListener('keydown', (e) => {
    if (e.target.tagName === 'INPUT' || !e.target.closest('td[data-r]')) return;
    const k = e.key;
    const moves = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] };
    if (moves[k]) { e.preventDefault(); move(...moves[k]); }
    else if (k === 'Tab') { e.preventDefault(); move(0, e.shiftKey ? -1 : 1); }
    else if (k === 'Enter' || k === 'F2') { e.preventDefault(); edit(); }
    else if ((k === 'Delete' || k === 'Backspace') && editable) { e.preventDefault(); set(sel.r, sel.c, ''); draw(); focusSel(); }
    else if (k.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey && editable) { e.preventDefault(); edit(k); }
  });
  fin.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { e.preventDefault(); set(sel.r, sel.c, fin.value); draw(); focusSel(); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); fin.value = raw(sel.r, sel.c); focusSel(); }
  });
  tb.addEventListener('click', (e) => {
    const a = e.target.closest('button') && e.target.closest('button').dataset.a;
    if (a === 'row') { rows.push(new Array(width()).fill('')); values = evaluate(rows); setDirty(true); sel = { r: rows.length - 1, c: 0 }; draw(); focusSel(); }
    else if (a === 'col') { const w = width(); rows.forEach((r) => { while (r.length < w + 1) r.push(''); }); values = evaluate(rows); setDirty(true); draw(); }
    else if (a === 'save') save();
    else if (a === 'discard') { rows = parseCsv(content); if (!rows.length) rows = [['']]; values = evaluate(rows); setDirty(false); draw(); }
  });
  document.addEventListener('keydown', (e) => { if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') { e.preventDefault(); save(); } });
  draw();
}

function esc(s) { return String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c])); }

// only in an artifact's own page (the gallery and the tests import the functions above)
const DATA = typeof document !== 'undefined' && typeof document.getElementById === 'function' && document.getElementById('art-data');
if (DATA && DATA.dataset.type === 'sheet') boot(JSON.parse(DATA.textContent));
