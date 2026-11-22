// A local stand-in for the Claude API and one MCP server, for the e2e smoke
// test (dev/mock_api.py has more canned replies; this one needs no Python and
// lets the test see what the app sent).
//
// Streaming Messages requests are answered by the last user message:
//   "slow ..."                    -> 400 words at 50 ms each (to test Stop)
//   "... echo ..."                -> calls the MCP server's echo tool, if offered
//   "... write the file ..."      -> calls Write on <cwd>/e2e.txt (Code sessions)
//   a tool result                 -> "Done: <the result>"
//   anything else                 -> "Answer <n> to <text>. ..." in pieces
import http from 'node:http';
import path from 'node:path';

export const TEST_KEY = 'sk-ant-test';
export const TITLE = 'E2E smoke chat';

const MODELS = [['claude-opus-5-5', 'Claude Opus 5.5'], ['claude-sonnet-5-5', 'Claude Sonnet 5.5'], ['claude-haiku-5-5', 'Claude Haiku 5.5']]
  .map(([id, display_name]) => ({ id, display_name, type: 'model', created_at: '2026-01-01T00:00:00Z' }));

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function json(res, status, value) {
  const body = JSON.stringify(value);
  res.writeHead(status, { 'content-type': 'application/json', 'content-length': Buffer.byteLength(body) });
  res.end(body);
}

/** The text of a user message, without the app's reminders. */
function userText(content) {
  if (typeof content === 'string') return content;
  return content.filter((b) => b.type === 'text' && !b.text.startsWith('<system-reminder>')).map((b) => b.text).join('\n');
}

function resultText(r) {
  return typeof r.content === 'string' ? r.content : (r.content || []).map((c) => c.text || '').join('');
}

function mcp(body, res, mock) {
  // notifications get no answer
  if (body.id === undefined || body.id === null) { res.writeHead(202).end(); return; }
  let result = {};
  if (body.method === 'initialize') result = { protocolVersion: body.params.protocolVersion, capabilities: { tools: {} }, serverInfo: { name: 'e2e', version: '1' } };
  else if (body.method === 'tools/list') result = { tools: [{ name: 'echo', description: 'Echo the text back', inputSchema: { type: 'object', properties: { text: { type: 'string' } } } }] };
  else if (body.method === 'tools/call') {
    mock.mcpCalls.push(body.params);
    result = { content: [{ type: 'text', text: String((body.params.arguments || {}).text ?? '') }] };
  }
  json(res, 200, { jsonrpc: '2.0', id: body.id, result });
}

async function stream(res, body, mock) {
  let gone = false;
  res.on('close', () => { if (!res.writableFinished) gone = true; });
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
  const send = (ev) => res.write(`event: ${ev.type}\ndata: ${JSON.stringify(ev)}\n\n`);
  let index = 0;
  const text = async (pieces, delay) => {
    send({ type: 'content_block_start', index, content_block: { type: 'text', text: '' } });
    for (const p of pieces) {
      if (gone) return;
      send({ type: 'content_block_delta', index, delta: { type: 'text_delta', text: p } });
      await sleep(delay);
    }
    send({ type: 'content_block_stop', index });
    index++;
  };
  const tool = (id, name, input) => {
    send({ type: 'content_block_start', index, content_block: { type: 'tool_use', id, name, input: {} } });
    send({ type: 'content_block_delta', index, delta: { type: 'input_json_delta', partial_json: JSON.stringify(input) } });
    send({ type: 'content_block_stop', index });
    index++;
  };
  const n = mock.requests.length;
  send({ type: 'message_start', message: { id: `msg_${n}`, type: 'message', role: 'assistant', model: body.model, content: [], usage: { input_tokens: 10, output_tokens: 1 } } });
  const last = body.messages[body.messages.length - 1];
  const result = Array.isArray(last.content) && last.content.find((b) => b.type === 'tool_result');
  const asked = userText(last.content);
  const tools = (body.tools || []).map((t) => t.name);
  const echo = tools.find((t) => t.endsWith('__echo'));
  let stop = 'end_turn';
  if (result) await text([`Done: ${resultText(result)}`], 0);
  else if (/^slow/i.test(asked)) await text(Array.from({ length: 400 }, (_, i) => `w${i} `), 50);
  else if (/\becho\b/i.test(asked) && echo) {
    await text(['Let me echo it.'], 0);
    tool('toolu_echo', echo, { text: 'hi' });
    stop = 'tool_use';
  } else if (/write the file/i.test(asked) && tools.includes('Write') && mock.cwd) {
    await text(["I'll write it."], 0);
    tool('toolu_write', 'Write', { file_path: path.join(mock.cwd, 'e2e.txt'), content: 'written by the e2e test\n' });
    stop = 'tool_use';
  } else await text(`Answer ${n} to ${asked}. The mock streams this reply in pieces.`.split(/(?<= )/), 80);
  if (gone) { mock.aborted++; return; }
  send({ type: 'message_delta', delta: { stop_reason: stop }, usage: { output_tokens: 5 } });
  send({ type: 'message_stop' });
  res.end();
}

/** Start the mock on a free port. `requests` gets the body of every
 *  streaming Messages request; set `cwd` for the Write tool's folder. */
export async function startMock() {
  const mock = { url: '', requests: [], mcpCalls: [], aborted: 0, rejected: 0, cwd: null, close: null };
  const server = http.createServer(async (req, res) => {
    let raw = '';
    for await (const c of req) raw += c;
    let body = {};
    try { body = raw ? JSON.parse(raw) : {}; } catch { json(res, 400, { error: 'not JSON' }); return; }
    if (req.url === '/mcp') {
      if (req.method === 'POST') mcp(body, res, mock); else res.writeHead(405).end();
      return;
    }
    if (!req.url.startsWith('/v1/')) { res.writeHead(404).end(); return; }
    // only the test key: anything else means the dev copy isn't set up for
    // this test (the key is compared, never kept)
    if (req.headers['x-api-key'] !== TEST_KEY) {
      mock.rejected++;
      json(res, 401, { type: 'error', error: { type: 'authentication_error', message: 'the e2e mock takes only the test key' } });
      return;
    }
    if (req.method === 'GET' && req.url.startsWith('/v1/models')) { json(res, 200, { data: MODELS, has_more: false, first_id: MODELS[0].id, last_id: MODELS[MODELS.length - 1].id }); return; }
    if (req.method !== 'POST') { res.writeHead(405).end(); return; }
    if (!body.stream) {
      json(res, 200, { id: 'msg_title', type: 'message', role: 'assistant', model: body.model, content: [{ type: 'text', text: TITLE }], stop_reason: 'end_turn', usage: { input_tokens: 5, output_tokens: 2 } });
      return;
    }
    mock.requests.push(body);
    await stream(res, body, mock);
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  mock.url = `http://127.0.0.1:${server.address().port}`;
  mock.close = () => new Promise((r) => { server.closeAllConnections(); server.close(r); });
  return mock;
}
