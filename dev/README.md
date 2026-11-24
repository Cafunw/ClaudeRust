# Development helpers

## Tests

What CI (`.github/workflows/ci.yml`) runs, from the repository root:

```powershell
# Rust: unit, integration (src-tauri/tests) and doc tests, then lints
cd src-tauri
cargo test --no-default-features
cargo clippy --no-default-features --all-targets -- -D warnings
cd ..
# UI: lint and the unit tests of ui/js (ui/test, plain node --test)
npm ci --ignore-scripts
npm run lint
npm run test:ui
# end to end, Windows: builds, starts and stops a dev copy (see below)
npm run e2e
```

`cargo test` needs no API key or network: the integration tests talk to a
local mock of the API. Tests that need a real Claude Code CLI, and benchmarks,
are `#[ignore]`d.

## Installers

```powershell
npm run build:unsigned   # installers in target/release/bundle, no signing key
```

`npm run build` (plain `npx tauri build`) also makes the update files the
in-app updater downloads (`createUpdaterArtifacts` in `tauri.conf.json`), which
must be signed, so it stops without `TAURI_SIGNING_PRIVATE_KEY` (and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). Only the Release workflow has that key;
the *Build installers* workflow builds unsigned like `build:unsigned`.

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
file write, Settings, the sidebar search, an HTML artifact from a reply
opening in the preview panel and readable in the dark and the light theme,
"Try fixing" on an artifact that throws, deleting a conversation from the sidebar, and a long (300-turn) conversation
loading its earlier turns as you scroll up. Its mock of the API
(`ui/e2e/mock_api.mjs`, in the test's own process) answers on a free port.

One command builds and starts the dev copy, runs the suite and stops the
copy, as the `e2e` CI job does:

```powershell
npm run e2e        # or: node ui/e2e/run.mjs [tauri config]
```

`ui/e2e/run.mjs` starts `npx tauri dev --no-watch --config <config>` with
`dev/e2e.conf.json` (its own identifier, so its own settings and chats, not
the installed app's) plus the window entries of `tauri.conf.json`, whose
`additionalBrowserArgs` carry the debugging port (9333, or the next free one)
next to wry's own WebView2 arguments; it writes that config to
`target/e2e/tauri.e2e.conf.json`. The port does not go through
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, which the copy runs without: the
WebView2 of a GitHub Windows runner never applied that variable. It starts
the copy with these variables, waits for the window (up to 12 minutes for a
first build), prints the debugging flag on the copy's browser process, runs
the tests and ends the copy's process tree:

| Variable | Value | Why |
|---|---|---|
| `CLAUDERUST_API_KEY` | `sk-ant-test` | the only key the mock takes; the keyring is not touched |
| `CLAUDERUST_OFFSCREEN` | `1` | the window opens off-screen and never takes focus |
| `CLAUDE_CONFIG_DIR` | an empty temporary folder | a built-in Code session runs your own Claude Code hooks from `<CLAUDE_CONFIG_DIR or ~/.claude>/settings.json`; on Windows `HOME` and `USERPROFILE` do not move that folder (the app asks Windows for the profile folder) |
| `CLAUDERUST_E2E_REQUIRED` | `1` (for the tests only) | a test that would be skipped fails instead |

The copy's output goes to `target/e2e/dev.log`, and on a failure the app's
own log is copied next to it (`app.log`; CI uploads both as the `e2e-logs`
artifact). It refuses to start while something else listens on port 9333.

To drive a copy you started yourself instead, set the same variables (the
debugging port in the environment variable here, which a local WebView2
applies; a development build with that port also opens off-screen), start it,
and run the tests once the window is up:

```powershell
$env:CLAUDERUST_API_KEY = 'sk-ant-test'
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9333'
$env:CLAUDE_CONFIG_DIR = (New-Item -ItemType Directory -Force "$env:TEMP\clauderust-e2e-config").FullName
npx tauri dev --no-watch --config dev/e2e.conf.json
# in another terminal
npm run test:e2e
```

There, with no dev copy listening on `127.0.0.1:9333` every test is skipped,
and the Code session step is skipped when the copy would run hooks of your
Claude Code settings (it asks the app before the session's first message).

For the run the test points that copy's API base URL and MCP configuration
at its mock, follows the system theme, and turns notifications, worktrees and
checkpoints off; it puts them back and deletes the conversations it made at
the end. It refuses a copy on the installed app's data folder or without an
API key. Remove `%APPDATA%\dev.clauderust.e2e` and
`%LOCALAPPDATA%\dev.clauderust.e2e` when done.

## Fake MCP server

`fake_mcp_server.py` is a tiny stdio MCP server with two tools and a prompt:

```json
{ "mcpServers": { "weather": { "command": "python3", "args": ["/abs/path/dev/fake_mcp_server.py"] } } }
```
