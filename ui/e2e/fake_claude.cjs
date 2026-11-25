// A stand-in for the Claude Code CLI, for the e2e tests of a chat on the Claude account (account.test.mjs):
// the dev copy runs it through its Claude Code path setting (a claude.cmd shim beside a copy of this file),
// exactly as it runs an npm-installed CLI. No model runs and nothing is spent. It answers what the app asks:
//   --version, auth status --json      -> a signed-in CLI 2.1
//   -p --output-format json            -> a title (an error when CLAUDE_CODE_MAX_OUTPUT_TOKENS leaves no room for
//                                          the ~255 tokens Haiku 5.5 thinks, as the real CLI does)
//   -p --input-format stream-json      -> a turn per message of the person's, as long as its input is open:
//       "... artifact ..."             -> an HTML artifact
//       "... echo ..."                 -> calls the e2e MCP server's echo tool, after asking permission
//       anything else                  -> "Account answer to <text>. ..." in pieces
//     a message with shouldQuery false (an earlier conversation) is taken in, unanswered, and a
//     generate_session_title request gets "E2E account chat"
// Each start, with its arguments, what it read and its exit, is logged to log.jsonl beside it, with its pid.
const fs = require('fs');
const path = require('path');
const readline = require('readline');

const argv = process.argv.slice(2);
const log = (o) => fs.appendFileSync(path.join(__dirname, 'log.jsonl'), JSON.stringify({ pid: process.pid, ...o }) + '\n');
process.on('exit', (code) => log({ exit: code }));
const write = (o) => process.stdout.write(JSON.stringify(o) + '\n');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const has = (flag) => argv.includes(flag);
log({ argv, max_out: process.env.CLAUDE_CODE_MAX_OUTPUT_TOKENS || null });

if (has('--version')) {
  process.stdout.write('2.1.326 (Claude Code)\n');
} else if (argv[0] === 'auth') {
  write({ loggedIn: true, authMethod: 'claude.ai', email: 'e2e@example.com', subscriptionType: 'max' });
} else if (has('--output-format') && argv[argv.indexOf('--output-format') + 1] === 'json') {
  let input = '';
  process.stdin.on('data', (d) => (input += d)).on('end', () => {
    log({ one_shot: input.slice(0, 200) });
    const room = Number(process.env.CLAUDE_CODE_MAX_OUTPUT_TOKENS) >= 300;
    write(room
      ? { type: 'result', subtype: 'success', is_error: false, result: 'Title: E2E account chat', usage: { input_tokens: 120, output_tokens: 270 } }
      : { type: 'result', subtype: 'success', is_error: true, result: `API Error: Claude's response exceeded the ${process.env.CLAUDE_CODE_MAX_OUTPUT_TOKENS} output token maximum.` });
  });
} else {
  turns();
}

function turns() {
  const sid = `e2e-${process.pid}`;
  const waiting = new Map();
  let n = 0;
  // the app's answer to our request `id`
  const ask = (request) => new Promise((resolve) => {
    const id = `fake-${++n}`;
    waiting.set(id, resolve);
    write({ type: 'control_request', request_id: id, request });
  });
  const stream = (event) => write({ type: 'stream_event', event, parent_tool_use_id: null, session_id: sid });
  const message = (id, content) => write({ type: 'assistant', message: { id, type: 'message', role: 'assistant', model: 'claude-opus-5-5', content, usage: { input_tokens: 10, output_tokens: 5 } }, parent_tool_use_id: null, session_id: sid, uuid: `${id}-uuid` });
  let m = 0;
  const say = async (text, delay) => {
    const id = `msg_${process.pid}_${++m}`;
    stream({ type: 'message_start', message: { id, type: 'message', role: 'assistant', model: 'claude-opus-5-5', content: [], usage: { input_tokens: 10, output_tokens: 1 } } });
    stream({ type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } });
    for (const piece of text.split(/(?<=[ \n])/)) {
      stream({ type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: piece } });
      await sleep(delay);
    }
    stream({ type: 'content_block_stop', index: 0 });
    message(id, [{ type: 'text', text }]);
    stream({ type: 'message_stop' });
  };
  const turn = async (text) => {
    write({ type: 'system', subtype: 'init', session_id: sid, model: 'claude-opus-5-5', tools: [], mcp_servers: [] });
    if (/\bartifact\b/i.test(text)) {
      await say('Here it is.\n\n<antArtifact identifier="account-page" type="text/html" title="E2E account page">\n<h1>Made on the account</h1>\n</antArtifact>\n\nDone.', 40);
    } else if (/\becho\b/i.test(text)) {
      const call = { type: 'tool_use', id: 'toolu_echo', name: 'mcp__e2e__echo', input: { text: 'hi' } };
      message(`msg_${process.pid}_tool`, [{ type: 'text', text: 'Let me echo it.' }, call]);
      const answer = await ask({ subtype: 'can_use_tool', tool_name: call.name, input: call.input, tool_use_id: call.id });
      let result = 'The user denied this tool call.';
      if (answer.response?.behavior === 'allow') {
        const r = await ask({ subtype: 'mcp_message', server_name: 'e2e', message: { jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: 'echo', arguments: call.input } } });
        result = r.response?.mcp_response?.result?.content?.[0]?.text ?? 'no answer';
      }
      write({ type: 'user', message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: call.id, content: result }] }, parent_tool_use_id: null, session_id: sid, uuid: `${call.id}-result` });
      await say(`Done: ${result}`, 0);
    } else {
      await say(`Account answer to ${text}. The fake CLI streams this reply in pieces.`, 80);
    }
    write({ type: 'result', subtype: 'success', is_error: false, duration_ms: 1200, num_turns: 1, result: '', session_id: sid, total_cost_usd: 0.001, usage: { input_tokens: 10, output_tokens: 5 } });
  };
  readline.createInterface({ input: process.stdin }).on('line', (line) => {
    let v;
    try { v = JSON.parse(line); } catch { return; }
    log({ in: v.type, subtype: v.request?.subtype });
    if (v.type === 'control_response') {
      const resolve = waiting.get(v.response?.request_id);
      waiting.delete(v.response?.request_id);
      if (resolve) resolve(v.response);
    } else if (v.type === 'control_request') {
      const response = v.request?.subtype === 'get_settings' ? { applied: {} } : v.request?.subtype === 'generate_session_title' ? { title: 'E2E account chat' } : {};
      write({ type: 'control_response', response: { subtype: 'success', request_id: v.request_id, response } });
    } else if (v.type === 'user' && v.shouldQuery === false) {
      // a message of an earlier conversation, taken in without an answer
      write({ type: 'result', subtype: 'success', is_error: false, num_turns: 0, result: '', session_id: sid, total_cost_usd: 0, usage: { input_tokens: 0, output_tokens: 0 } });
    } else if (v.type === 'user') {
      const text = (v.message.content || []).filter((b) => b.type === 'text' && !b.text.startsWith('<')).map((b) => b.text).join('\n');
      turn(text);
    }
  }).on('close', () => process.exit(0));
}
