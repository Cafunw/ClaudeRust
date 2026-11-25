// npm run test:ui — the tests that search source text pass on a Windows checkout too, where
// core.autocrlf gives every file "\r\n" line ends: they read it through source.mjs.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, writeFileSync, rmSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { source } from './source.mjs';

test('source() gives "\\n" line ends whatever the file has', () => {
  const dir = mkdtempSync(join(tmpdir(), 'cr-eol-'));
  const f = join(dir, 'x.css');
  writeFileSync(f, '.a {\r\n  b: c;\r\n}\r\n}\n');
  assert.equal(source(pathToFileURL(f)), '.a {\n  b: c;\n}\n}\n');
  rmSync(dir, { recursive: true });
});

test('no UI test reads a source file around source.mjs', () => {
  const dir = new URL('./', import.meta.url);
  for (const f of readdirSync(dir).filter((x) => x.endsWith('.mjs') && x !== 'source.mjs')) {
    const text = source(new URL(f, dir));
    // (a JSON fixture is parsed, never searched: its line ends don't matter)
    for (const m of text.matchAll(/readFileSync\(([^;]*?)'utf8'\)/g)) assert.match(m[1], /\.json['`]/, `${f}: ${m[0]}`);
  }
});
