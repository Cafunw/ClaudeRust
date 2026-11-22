// npm run test:ui — Settings → About → Copy diagnostics: nothing secret leaves.
import test from 'node:test';
import assert from 'node:assert/strict';
import { redact, diagnosticsReport } from '../js/diagnostics.js';

test('keys, tokens and credentials are redacted', () => {
  assert.equal(redact('key sk-ant-api03-AbCdEf_123-xyz used'), 'key sk-[redacted] used');
  assert.equal(redact('Authorization: Bearer abc.def-123'), 'Authorization: Bearer [redacted]');
  assert.equal(redact('"headers":{"Authorization":"Basic eHk6eg=="}'), '"headers":{"Authorization":"Basic [redacted]"}');
  assert.equal(redact('GET /cb?code=1&access_token=s3cr3t&x=1'), 'GET /cb?code=1&access_token=[redacted]&x=1');
  assert.equal(redact('api_key = "hunter2" ok'), 'api_key = "[redacted]" ok');
  assert.equal(redact('client_secret: abc password=pw'), 'client_secret: [redacted] password=[redacted]');
  assert.equal(redact('jwt eyJhbGciOi.eyJzdWIiOi.SflKxw end'), 'jwt [jwt] end');
});

test('URLs keep their host but lose their credentials', () => {
  assert.equal(redact('connect https://user:pa55@mcp.example.com/sse failed'), 'connect https://[redacted]@mcp.example.com/sse failed');
  assert.equal(redact('git://tok@host/repo'), 'git://[redacted]@host/repo');
  assert.equal(redact('https://mcp.example.com/mcp ok'), 'https://mcp.example.com/mcp ok');
});

test('home folders and e-mail addresses are taken out', () => {
  assert.equal(redact(String.raw`open C:\Users\Ayşe\AppData\Roaming\x`), String.raw`open ~\AppData\Roaming\x`);
  assert.equal(redact(String.raw`"C:\\Users\\bob\\proj"`), String.raw`"~\\proj"`);
  assert.equal(redact('c:/users/bob/x and /home/ann/y and /Users/cat/z'), '~/x and ~/y and ~/z');
  assert.equal(redact('signed in as ann.lee+x@mail.example.org'), 'signed in as [email]');
});

test('ordinary log text is left as it is', () => {
  const line = '2026-11-22 10:00:01.123 WARN  clauderust::mcp: server "files" exited with code 1; 12 tokens, max_tokens=4096';
  assert.equal(redact(line), line);
  assert.equal(redact(null), '');
});

test('the report: version, OS, engines, names of the servers that are on, redacted log', () => {
  const r = diagnosticsReport({
    version: '0.1.5', platform: 'windows', userAgent: 'Mozilla/5.0 (Windows NT 10.0)',
    settings: { chat_engine: 'claude-code', has_api_key: false, disabled_mcp_servers: ['off-by-setting'] },
    servers: [{ name: 'files', status: 'connected' }, { name: 'gone', status: 'disabled' }, { name: 'off-by-setting', status: 'failed' }],
    log: 'x Bearer t0k3n',
  });
  assert.ok(r.startsWith('ClaudeRust 0.1.5\nOS: windows (Mozilla/5.0 (Windows NT 10.0))\nChat engine: claude-code; code engine: auto; API key: no\nMCP servers on: files\n'), r);
  assert.ok(r.endsWith('x Bearer [redacted]'), r);
  assert.ok(diagnosticsReport({}).includes('MCP servers on: none') && diagnosticsReport({}).endsWith('(empty)'));
});
