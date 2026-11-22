# Development helpers

## Run the app against a local mock of the Claude API

`mock_api.py` speaks the Messages API (SSE streaming) and returns canned
responses, so the whole UI can be exercised without an API key or network:

```bash
python3 dev/mock_api.py 8765 /tmp/mock_requests.jsonl &
npx tauri dev
```

In onboarding use the key `sk-ant-test` and, under *Advanced*, the base URL
`http://127.0.0.1:8765`. Every request the app sends is appended to the JSONL
log, which is handy for checking request shapes (system prompt, tools,
thinking/effort settings, tool results).

Prompts that trigger specific behaviours:

| Message contains | Mock response |
|---|---|
| `think` | thinking block + answer |
| `app` / `artifact` | an HTML artifact |
| `search` | web search tool use, results and citations |
| `slow` | a slow stream (to test Stop) |
| `refuse` | `stop_reason: "refusal"` |
| `list` / `files` (Code mode) | Glob tool call, then a summary |
| `edit` (Code mode) | Write tool call (permission prompt) |
| `weather` (with the MCP server below) | MCP `get_forecast` tool call |
| anything else | rich Markdown (table, math, code) |

## End-to-end smoke test (Windows)

`npm run test:e2e` drives a running dev copy over the Chrome DevTools
Protocol (WebView2 only) through the critical flows: send a message and see
the reply stream in, Stop a slow reply, edit a message, Retry, move between
the versions, a permission card for an MCP tool call and for a Code session's
file write, Settings, and the sidebar search. It starts nothing itself: its
mock of the API (`ui/e2e/mock_api.mjs`, in the test's own process) answers
on a free port, and with no dev copy listening on `127.0.0.1:9333` every
test is skipped.

Start the dev copy with its own identifier (its own settings and chats, not
the installed app's), the test key and the debugging port, then run the test:

```powershell
$env:CLAUDERUST_API_KEY = 'sk-ant-test'
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9333'
npx tauri dev --config dev/e2e.conf.json
# in another terminal, once the window is up
npm run test:e2e
```

For the run the test points that copy's API base URL and MCP configuration
at its mock and turns notifications, worktrees and checkpoints off; it puts
them back and deletes the conversations it made at the end. It refuses a copy
on the installed app's data folder or without an API key. The Code session
step is skipped when `~/.claude/settings.json` has hooks, since a built-in
Code session would run them. Remove `%APPDATA%\dev.clauderust.e2e` when done.

## Fake MCP server

`fake_mcp_server.py` is a tiny stdio MCP server with two tools and a prompt:

```json
{ "mcpServers": { "weather": { "command": "python3", "args": ["/abs/path/dev/fake_mcp_server.py"] } } }
```
