// npm run test:ui — the typed artifacts' runtimes (ui/js/artifact_types): a
// sheet's formulas against hostile input, a deck's click steps and paging, a
// doc's markdown written back from its page, a dashboard's data and charts,
// and the gallery's thumbnails of them.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { parseCsv, toCsv, evaluate, display, cellType, literal, sortedRows, hasHeader, colName, isErr, FUNCTIONS } from '../js/artifact_types/sheet.js';
import { fragmentSteps, navigate, safeUrl } from '../js/artifact_types/slides.js';
import { htmlToMarkdown, wordCount } from '../js/artifact_types/doc.js';
import { parseDashboard, chartSvg, formatValue, niceTicks } from '../js/artifact_types/dashboard.js';
import { firstHeading, sheetCorner } from '../js/artifact_preview.js';

const value = (formula, rows = []) => evaluate([...rows, [formula]]).at(-1)[0];
const code = (v) => (isErr(v) ? v.error : v);

test('a sheet reads CSV as models write it, formulas with their commas', () => {
  assert.deepEqual(parseCsv('a,"b, c","d ""q"""\n1,=SUM(A2,B2),=IF(A2>1,"x, y","z")\r\n'), [['a', 'b, c', 'd "q"'], ['1', '=SUM(A2,B2)', '=IF(A2>1,"x, y","z")']]);
  assert.deepEqual(parseCsv('"multi\nline",x'), [['multi\nline', 'x']]);
  // written back as standard CSV, which reads the same
  const rows = [['a', 'b, c'], ['=IF(A1="a",1,2)', ''], ['', '']];
  assert.equal(toCsv(rows), 'a,"b, c"\n"=IF(A1=""a"",1,2)"');
  assert.deepEqual(parseCsv(toCsv(rows)), [['a', 'b, c'], ['=IF(A1="a",1,2)']]);
  assert.deepEqual(['A', 'Z', 'AA', 'AZ', 'BA'], [0, 25, 26, 51, 52].map(colName));
});

test('cells are typed: numbers, money, percents, dates, booleans and text', () => {
  const cases = [['12', 'number', 12], ['1,234.5', 'number', 1234.5], ['-3e2', 'number', -300], ['$1,200.50', 'currency', 1200.5], ['15 €', 'currency', 15], ['-₺7', 'currency', -7], ['12%', 'percent', 0.12], ['2026-11-24', 'date', '2026-11-24'], ['TRUE', 'bool', true], ['false', 'bool', false], ['Rent', 'text', 'Rent'], ['1.2.3', 'text', '1.2.3'], ['', 'empty', ''], ['=A1', 'formula']];
  for (const [raw, type, v] of cases) {
    assert.equal(cellType(raw), type, raw);
    if (type !== 'formula') assert.equal(literal(raw), v, raw);
  }
});

test('formulas: references, ranges, SUM AVERAGE MIN MAX COUNT IF and operators, as a spreadsheet works them out', () => {
  const rows = parseCsv('Item,Qty,Price\nA,2,$10\nB,3,4.5\nC,,x');
  const v = (f) => code(value(f, rows));
  assert.equal(v('=SUM(B2:B4)'), 5);
  assert.equal(v('=SUM(B2:C3)'), 19.5);
  assert.equal(v('=AVERAGE(C2:C4)'), 7.25, 'text and empty cells are left out');
  assert.equal(v('=MIN(B2:B4)'), 2);
  assert.equal(v('=MAX(C2:C4, 100)'), 100);
  assert.equal(v('=COUNT(A1:C4)'), 4);
  assert.equal(v('=B2*C2+B3*C3'), 33.5);
  assert.equal(v('=IF(B2>B3,"more","less")'), 'less');
  assert.equal(v('=IF(SUM(B2:B3)=5, 1)'), 1);
  assert.equal(v('=IF(FALSE, 1)'), false);
  assert.equal(v('=$B$2+B$3'), 5, 'absolute references');
  assert.equal(v('=-2^2'), 4, 'negation binds first');
  assert.equal(v('=2^3^2'), 64, 'left to right');
  assert.equal(v('=1+2*3-4/2'), 5);
  assert.equal(v('=50%*B3'), 1.5);
  assert.equal(v('="Total: "&SUM(B2:B3)'), 'Total: 5');
  assert.equal(v('=ROUND(2/3, 2)'), 0.67);
  assert.equal(v('=AND(B2>1, OR(FALSE, B3=3), NOT(FALSE))'), true);
  assert.equal(v('=ABS(-4)'), 4);
  assert.equal(v('=sum(b2:b3)'), 5, 'names and references in any case');
  assert.equal(display('=1/3', 1 / 3), '0.333333');
  assert.equal(display('=1/3', 1 / 3, 'tr'), '0,333333');
  assert.equal(display('12', 12), '12', 'a value typed in shows as typed');
});

test('a hostile sheet ends in error values, never a crash, a hang or code run', () => {
  const v = (f, rows) => code(value(f, rows));
  assert.equal(v('=1/0'), '#DIV/0!');
  assert.equal(v('=A1+1', [['=A2+1'], ['=A1+1']]), '#CYCLE!');
  assert.equal(v('=A1'), '#CYCLE!', 'itself');
  assert.equal(v('=FOO(1)'), '#NAME?');
  for (const f of ['=constructor(1)', '=__proto__(1)', '=toString()', '=hasOwnProperty(1)']) assert.equal(v(f), '#NAME?', f);
  for (const f of ['=alert(1)', '=window.top', '=(1', '=1+', '=)', '=SUM(', '=1 2', '="unclosed', '=@A1', '=A1:', '=;']) assert.ok(isErr(value(f)), f);
  assert.equal(v('=' + '('.repeat(10000) + '1' + ')'.repeat(10000)), '#ERROR!', 'deep nesting');
  assert.equal(v('=' + '-'.repeat(5000) + '1'), '#ERROR!');
  assert.equal(v('=' + '1+'.repeat(1500) + '1'), '#ERROR!', 'too long');
  assert.equal(v('=SUM(A1:ZZZ9999999)'), '#REF!', 'a range too large to walk');
  // many ranges that each fit, together too much to walk: the rest end in #REF! quickly
  const wide = [Array(1000).fill('1'), ...Array.from({ length: 99 }, () => Array(1000).fill('1')), Array(1000).fill(`=SUM(${Array(20).fill('A1:ALL100').join(',')})`)];
  const t0 = Date.now(), heavy = evaluate(wide)[100];
  assert.ok(Date.now() - t0 < 5000 && heavy[0] === 2000000 && code(heavy[999]) === '#REF!');
  assert.equal(v('="a"*2'), '#VALUE!');
  assert.equal(v('=10^400'), '#NUM!');
  assert.equal(v('=IF(1/0, 1, 2)'), '#DIV/0!', 'errors pass on');
  assert.equal(v('=SUM(A1:A2)', [['=1/0'], ['1']]), '#DIV/0!');
  // a long chain each way is worked out, without overflowing the stack
  const down = Array.from({ length: 5000 }, (_, i) => [i ? `=A${i}+1` : '1']);
  assert.equal(evaluate(down)[4999][0], 5000);
  const up = Array.from({ length: 3000 }, (_, i) => [i === 2999 ? '1' : `=A${i + 2}+1`]);
  assert.equal(evaluate(up)[0][0], 3000);
  assert.deepEqual(FUNCTIONS.sort(), ['ABS', 'AND', 'AVERAGE', 'COUNT', 'IF', 'MAX', 'MIN', 'NOT', 'OR', 'ROUND', 'SUM']);
});

test('sorting reorders the view by a column, the header row staying on top', () => {
  const rows = parseCsv('Name,Score\nb,10\na,=5*3\nc,\nd,2');
  const values = evaluate(rows);
  assert.ok(hasHeader(rows));
  assert.deepEqual(sortedRows(rows, values, 1, 1), [0, 4, 1, 2, 3], 'ascending, empty last');
  assert.deepEqual(sortedRows(rows, values, 1, -1), [0, 2, 1, 4, 3]);
  assert.deepEqual(sortedRows(rows, values, 0, 1), [0, 2, 1, 3, 4], 'text by its letters');
  assert.deepEqual(sortedRows(rows, values, 1, 0), [0, 1, 2, 3, 4], 'not sorted: as written');
  assert.ok(!hasHeader(parseCsv('1,2\n3,4')), 'numbers on top are data');
  assert.deepEqual(sortedRows(parseCsv('3\n1\n2'), evaluate(parseCsv('3\n1\n2')), 0, 1), [1, 2, 0]);
});

test('a deck goes through each slide\'s click steps before the next slide, and back with them shown', () => {
  assert.deepEqual(fragmentSteps([null, null, null]), { stepOf: [0, 1, 2], count: 3 });
  assert.deepEqual(fragmentSteps([2, 1, 2, null]), { stepOf: [1, 0, 1, 2], count: 3 }, 'one index, one step; the rest after');
  assert.deepEqual(fragmentSteps([]), { stepOf: [], count: 0 });
  const counts = [0, 2, 0];
  let at = { slide: 0, step: 0 };
  const seen = [];
  for (let i = 0; i < 6; i++) { at = navigate(at, 'next', counts); seen.push(`${at.slide}.${at.step}`); }
  assert.deepEqual(seen, ['1.0', '1.1', '1.2', '2.0', '2.0', '2.0'], 'stops at the end');
  assert.deepEqual(navigate({ slide: 2, step: 0 }, 'prev', counts), { slide: 1, step: 2 }, 'back to a slide with its steps shown');
  assert.deepEqual(navigate({ slide: 1, step: 1 }, 'prev', counts), { slide: 1, step: 0 });
  assert.deepEqual(navigate({ slide: 0, step: 0 }, 'prev', counts), { slide: 0, step: 0 });
  assert.deepEqual(navigate({ slide: 1, step: 1 }, 'first', counts), { slide: 0, step: 0 });
  assert.deepEqual(navigate({ slide: 0, step: 0 }, 'last', counts), { slide: 2, step: 0 });
  assert.deepEqual(navigate({ slide: 99, step: 99 }, 'stay', counts), { slide: 2, step: 0 }, 'a slide from a link is held to the deck');
  assert.deepEqual(navigate({ slide: 0, step: 0 }, 'next', []), { slide: 0, step: 0 });
});

test('a slide keeps no link that runs script', () => {
  for (const u of ['javascript:alert(1)', ' JavaScript:alert(1)', 'java\tscript:x', 'vbscript:x', 'data:text/html,<script>', 'data:application/x', '\u0000javascript:x']) assert.ok(!safeUrl(u), u);
  for (const u of ['https://example.com', '#slide-2', 'data:image/png;base64,AA==', 'mailto:a@b.c', 'cover.png']) assert.ok(safeUrl(u), u);
});

// a tiny stand-in for the page's nodes: what htmlToMarkdown reads
const el = (name, attrs, ...kids) => ({ nodeType: 1, nodeName: name, getAttribute: (k) => (attrs && k in attrs ? attrs[k] : null), hasAttribute: (k) => !!attrs && k in attrs, childNodes: nodeList(kids.map((k) => (typeof k === 'string' ? { nodeType: 3, nodeValue: k, textContent: k } : k))), get textContent() { return [...this.childNodes].map((c) => c.textContent).join(''); } });
// as the page's NodeList: iterable, without the array methods
function nodeList(list) { return { length: list.length, [Symbol.iterator]: () => list[Symbol.iterator]() }; }

test('a doc edited in place is written back as the markdown it came from', () => {
  const doc = el('ARTICLE', null,
    el('H1', null, 'Plan'),
    el('P', null, 'Some ', el('STRONG', null, 'bold'), ' and ', el('EM', null, 'italic '), el('CODE', null, 'x`y'), ' ', el('A', { href: 'https://example.com/a b' }, 'link'), '.'),
    el('UL', null, el('LI', null, 'one'), el('LI', null, 'two', el('OL', null, el('LI', null, 'inner')))),
    el('OL', { start: '3' }, el('LI', null, el('P', null, 'three'))),
    el('BLOCKQUOTE', null, el('P', null, 'said'), el('P', null, 'twice')),
    el('PRE', null, el('CODE', null, 'let x = 1;\n')),
    el('TABLE', null, el('THEAD', null, el('TR', null, el('TH', null, 'A'), el('TH', null, 'B|C'))), el('TBODY', null, el('TR', null, el('TD', null, '1'), el('TD', null, '2')))),
    el('HR', null),
    el('DIV', null, 'typed after ', el('B', null, ''), 'Enter*'),
    el('UL', null, el('LI', null, el('INPUT', { type: 'checkbox', checked: '' }), 'done')),
  );
  assert.equal(htmlToMarkdown(doc), [
    '# Plan',
    'Some **bold** and *italic* ``x`y`` [link](https://example.com/a%20b).',
    '- one\n- two\n   1. inner',
    '3. three',
    '> said\n>\n> twice',
    '```\nlet x = 1;\n```',
    '| A | B\\|C |\n| --- | --- |\n| 1 | 2 |',
    '---',
    'typed after Enter\\*',
    '- [x] done',
  ].join('\n\n') + '\n');
  assert.equal(htmlToMarkdown(el('ARTICLE', null, el('H2', null, '  '), el('P', null, '_a_ [b]'))), '\\_a\\_ \\[b\\]\n', 'empty blocks go, markdown signs stay text');
  assert.equal(wordCount('Merhaba dünya, it’s 2026 — ok'), 5);
});

test('a dashboard is checked and drawn with everything it writes escaped', () => {
  assert.match(parseDashboard('{').error, /isn't valid JSON/);
  assert.match(parseDashboard('[1]').error, /JSON object/);
  const d = parseDashboard(JSON.stringify({
    title: 'Q3', kpis: [{ label: 'Revenue', value: '1,250,000', format: 'currency', currency: 'EUR', delta: -0.04 }, null, { label: 'x', value: 'n/a', format: 'weird' }],
    charts: [{ type: 'line', title: '<b>Trend</b>', x: ['Jan', 'Feb', '<i>'], series: [{ name: '"><script>alert(1)</script>', data: [1, 'x', 3] }] }, { type: 'nope', series: 'bad' }],
    tables: [{ columns: ['A'], rows: [[1], 'bad'] }],
  }));
  assert.equal(d.kpis.length, 2);
  assert.deepEqual(d.kpis[0], { label: 'Revenue', value: 1250000, format: 'currency', currency: 'EUR', delta: -0.04, deltaLabel: '' });
  assert.equal(d.kpis[1].format, 'number');
  assert.deepEqual(d.charts[0].series[0].data, [1, null, 3], 'a value that isn\'t a number is a gap');
  assert.equal(d.charts[1].type, 'bar');
  assert.deepEqual(d.charts[1].series, []);
  assert.deepEqual(d.tables[0].rows, [[1], []]);
  for (const c of [d.charts[0], { ...d.charts[0], type: 'bar' }, { ...d.charts[0], type: 'area' }, { ...d.charts[0], type: 'pie' }, { ...d.charts[0], type: 'donut' }, d.charts[1]]) {
    const svg = chartSvg(c, { dark: true });
    assert.ok(svg.startsWith('<svg') && svg.endsWith('</svg>'), c.type);
    assert.ok(!svg.includes('<script') && !svg.includes('<b>') && !svg.includes('<i>'), `${c.type}: ${svg.slice(0, 200)}`);
  }
  assert.match(chartSvg(d.charts[0]), /&lt;script&gt;/);
  assert.equal(formatValue(0.125, 'percent'), '12.5%');
  assert.equal(formatValue(1250000, 'currency', { currency: 'EUR', compact: true }), '€1.3M');
  assert.equal(formatValue(1234.5, 'number', { lang: 'tr' }), '1.234,5');
  assert.equal(formatValue('n/a'), 'n/a');
  assert.deepEqual(niceTicks(0, 87), [0, 20, 40, 60, 80, 100]);
  assert.deepEqual(niceTicks(-3, 3), [-4, -2, 0, 2, 4]);
  assert.deepEqual(niceTicks(5, 5), [0, 2, 4, 6, 8, 10], 'one value still gets an axis');
});

test('the gallery shows a sheet\'s first cells worked out, and a heading for what it can\'t draw', () => {
  const c = sheetCorner('Item,Cost\nRent,1000\nFood,250\nTotal,=SUM(B2:B3)\nBad,=1/0', 6, 2);
  assert.ok(c.head);
  assert.deepEqual(c.rows.map((r) => r.map((x) => x.text)), [['Item', 'Cost'], ['Rent', '1000'], ['Food', '250'], ['Total', '1,250'], ['Bad', '#DIV/0!']]);
  assert.ok(c.rows[3][1].num && c.rows[4][1].err);
  assert.equal(sheetCorner('=1000+250', { lang: 'tr' }).rows[0][0].text, '1.250', 'in the language of the UI');
  assert.equal(firstHeading('application/vnd.ant.react', 'export default () => <div><h1 className="x">Tip calculator</h1></div>'), 'Tip calculator');
  assert.equal(firstHeading('application/vnd.ant.react', 'export default () => <h2>{title}</h2>'), '');
  assert.equal(firstHeading('application/vnd.ant.mermaid', '%% a comment\ngraph TD; A-->B'), 'graph TD; A-->B');
});
