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

## Fake MCP server

`fake_mcp_server.py` is a tiny stdio MCP server with two tools and a prompt:

```json
{ "mcpServers": { "weather": { "command": "python3", "args": ["/abs/path/dev/fake_mcp_server.py"] } } }
```
