# Puanlayıcı 1: 71/100 (v0.1.1)

Bağımsız, salt okunur puanlayıcının tam raporu. Her kesinti kanıtıyla birlikte; PROJECT_PROGRESS.md backlog'u bu raporun EV-xx maddelerini izler.

| Kategori | Puan |
|---|---|
| Claude Desktop feature parity | 16/30 |
| Stability and crash resistance | 21.5/25 |
| Native performance and resource use | 17/20 |
| UI/UX polish and interaction quality | 11/15 |
| Code quality, tests and packaging | 5.5/10 |

## Kesintiler

### Claude Desktop feature parity (16/30)

- **-3.5** Chats signed in with the Claude account (the default when there is no API key, so the normal setup for a Claude Desktop subscriber) run through the CLI with every tool turned off. MCP tools, web search and chat search do nothing there, yet the composer still lists the MCP servers and the web-search switch as if they work.
  - Kanıt: claudecode.rs:789-807 passes --tools "" --strict-mcp-config --setting-sources "" in chat mode, and :793-797 forces web_search and chat_search off. composer.js:351 and :364-369 still show the web-search toggle and the MCP servers. Live test: the weather MCP server showed as connected and appeared in the tools menu as 'weather / 2 araç' (eval\s09_tools_mcp.png). Asked to call get_forecast, Haiku replied 'NO TOOL' (eval\s10_chat.png).

- **-2** Conversations and projects are not synced with the Claude account. History lives only in a local SQLite file, so a Desktop user's existing claude.ai chats and projects are missing, apart from a one-off import of the data export.
  - Kanıt: db.rs:21-45 (one local Connection is the only store). import.rs:1082 import_claude_export is the only way in. No account sync endpoint exists anywhere in src-tauri/src.

- **-2** Remote MCP support is basic: URL plus static headers only. There is no OAuth (custom connectors), no MCP Apps UI and no extension directory, while Claude Desktop ships mcpAppView, extensionWindow and webStorePage.
  - Kanıt: mcp.rs:1062-1081 reads only conf.url and conf.headers. The developer tab offers raw JSON editing and an 'install extension from file' button (eval\s08_mcp2.png). A grep for oauth/authorization in mcp.rs finds nothing.

- **-2** Current Desktop integrations are missing: dictation/voice, computer use, Claude in Chrome and Cowork.
  - Kanıt: A grep for dictation, getUserMedia, SpeechRecognition, computer use and cowork in ui/js and src-tauri/src finds nothing. The Claude Desktop asar listing contains dictation.*.js, computerUseTeach.js, computerUseWatchRecord.js, frame-shell-chrome.js and coworkArtifact.js.

- **-1** The Code tab does not render AskUserQuestion. It arrives as a generic can_use_tool permission, and 'allow' returns the input unchanged with no answers, so Claude's question is never answered. The bundled CLI 2.1.326 expects the host to answer through updatedInput.
  - Kanıt: A grep for AskUserQuestion in src-tauri/src and ui/js finds nothing. claudecode.rs:1248-1297 handles every tool the same way. claude.exe contains AskUserQuestion 75 times plus the string '…answers it with updatedInput'.

- **-1** React artifacts have no shadcn/ui (@/components/ui/*) import mapping and no window.claude or storage APIs, so common Claude-generated artifacts fail to import.
  - Kanıt: artifacts.rs:299-321: the import map has react, lucide-react, recharts, d3 and others, but no '@/components/ui' entry.

- **-1** No deep links: there is no URL-scheme handler or deep-link plugin.
  - Kanıt: lib.rs:241-266 registers single-instance, window-state, opener, dialog, clipboard, notification, updater, autostart, global-shortcut and the 'artifact' protocol only. tauri.conf.json has no deep-link section.

- **-1** Two UI languages against about 50 in Claude Desktop.
  - Kanıt: i18n.js:1259-1263 lists auto, en and tr. The Claude Desktop asar has i18n/*.json for de-DE, es-ES, fr-FR, ja-JP, ko-KR, pt-BR, zh-Hans, zh-Hant, ru and about 40 more.

- **-0.5** The Code tab has local sessions only: no cloud or SSH environments, which Claude Desktop offers (it ships claude-ssh-broker).
  - Kanıt: A grep for ssh/remote/cloud session in src-tauri/src finds only test strings (codetools.rs:1855, :2129). The Code session picker offers only a local folder (eval\s14_code.png).

Kontrol edilemeyenler: The Claude Desktop window cannot be screenshotted, so Desktop-side behaviour comes from its asar listing and general knowledge. The account-chat path was checked live with Haiku and Opus turns. The API-key path (MCP and web search in chat) was not run, because entering an API key was not allowed.

### Stability and crash resistance (21.5/25)

- **-1** A reply is stored only after its step finishes streaming. If the app crashes or the machine loses power mid-reply, everything streamed so far is lost.
  - Kanıt: engine.rs:998 inserts the assistant row with content [] before the stream. engine.rs:1141-1158 writes the content only after stream_with_recovery returns. Nothing writes to the database inside on_event (engine.rs:1062-1068).

- **-1** No Windows Job Object and no parent-death signal. If ClaudeRust crashes or is killed, the Claude Code CLI, MCP servers, PTY shells and background shells it started keep running, because only the orderly RunEvent::Exit path kills them.
  - Kanıt: lib.rs:521-541 does all cleanup on the orderly exit path. A grep for JobObject or JOB_OBJECT in src-tauri/src finds nothing. codetools.rs:962-968 runs taskkill only on an explicit stop. An orderly close of the dev copy left no orphans (verified with Get-CimInstance).

- **-1** When an MCP connection drops it stays down: a stdio server that exits is marked failed until the user reloads by hand, and notifications/tools/list_changed is ignored, so the tool list goes stale.
  - Kanıt: mcp.rs:1160-1165 sets state 'failed' with no retry. mcp.rs:1032-1046 returns early for notifications (method with no id), including tools/list_changed. refresh_tools (mcp.rs:546) is called only from initialize.

- **-0.5** open_path and reveal_path accept any path from the webview and pass it to the OS default handler, which is enough to run a program if script injection ever got in (defence in depth).
  - Kanıt: commands.rs:1284-1293 has no allow-list. The UI only needs the MCP config path and the data dir (settings.js:304, :449).

Kontrol edilemeyenler: Crash recovery could not be tested by killing the process: the owner's copy must not be touched, and killing the dev copy was avoided.

### Native performance and resource use (17/20)

- **-1** Streaming sends far more over IPC than the reply itself. The growing text part is re-sent whole every 45 ms tick; only unchanged parts are left out.
  - Kanıt: Measured in the dev copy with a listener on 'turn': 355 events and 2,370,554 bytes for a 5,247-character reply (max 13,912 bytes per event, 24 keyframes) over 26 s. turnstream.rs:1-22 drops only unchanged elements of parts and artifacts. No long tasks were recorded.

- **-1** Every account chat or Code turn starts a new Claude Code CLI process and resumes the transcript, instead of keeping one process alive per session.
  - Kanıt: claudecode.rs:894-911 spawns in run_once for each user message, and :1331 closes stdin after the result so the CLI exits. The CLI took 670 ms, 666 ms and 670 ms to first stdout with no model call (eval\clistart.mjs), on top of the transcript resume.

- **-1** Capped: the 0.1.1 release build could not be measured under load (it may not be launched), and Claude Desktop was busy, so there is no idle-to-idle comparison.
  - Kanıt: Release copy PID 13716 at idle (started 17:46; the image was later renamed clauderust-running.exe, v0.1.0): 386.9 MB working set / 191.4 MB private including 6 WebView2 processes, 0 ms CPU over 15 s. Claude Desktop's Electron processes, busy: 2,128 MB working set / 1,880 MB private, 47 ms CPU over 15 s.

Kontrol edilemeyenler: Release build under load (only the debug dev copy could be driven). An idle Claude Desktop baseline: it was hosting active Code sessions.

### UI/UX polish and interaction quality (11/15)

- **-1** In the Code tab, the floating 'Claude'a sor' (Ask Claude) bar sits on top of the terminal output and covers the shell prompt path, in both themes.
  - Kanıt: panes/terminal.js:105 (term-actions bar). Screenshots eval\s18_perm.png and eval\s28_dark_code.png.

- **-1** The 'N files edited' summary under a turn shows wrong line counts, and Windows paths stay absolute instead of project-relative.
  - Kanıt: eval\s20_done.png: the summary shows +5 -2 while the Edit row shows +3 -0 and git diff --stat shows 3 insertions. chat.js:623-624 counts every line of old_string and new_string. chat.js:620 makes paths relative only when they use '/'.

- **-1** The UI mixes languages: with the UI set to Turkish, model descriptions and backend error messages are still in English.
  - Kanıt: models.rs:86 hard-codes 'Most capable for complex work' (eval\s05_model.png). The error banner 'Add your Anthropic API key in Settings → Account…' (eval\s26_apierr.png) and the MCP errors (eval\s08_mcp2.png) are English too.

- **-0.5** With the Claude account, chats get no generated title; the sidebar shows the truncated first message.
  - Kanıt: engine.rs:1751-1752: title_via_api needs st.client(), which needs an API key. Sidebar title 'Use the weather get_forecast tool for Paris. If you do not h…' (eval\s10_chat.png).

- **-0.5** Layout nits: the engine chip in the Code header is clipped to 'Claude C'; the header cost shows $0.000 while the turn footer shows $0.00039; at narrow widths the open sidebar covers the content with no backdrop.
  - Kanıt: eval\s18_perm.png, eval\s10_chat.png, eval\s12_narrow.png

Kontrol edilemeyenler: Real hover and focus behaviour under a physical mouse and keyboard; everything was driven over CDP. The Quick Entry window and the tray menu were not opened.

### Code quality, tests and packaging (5.5/10)

- **-1** No integration test drives the critical send-a-message flow end to end, against a mock SSE server or a fake CLI: stream, persist, tool loop, mid-stream drop and stop. Only the pieces (SSE parser, accumulator, message building) are unit-tested.
  - Kanıt: The engine.rs tests are reservation, options, build_messages, sanitize and budget (awk over the test module). No test calls engine::run or claudecode::run_once. dev/mock_api.py is not used by any test.

- **-1** About 10,000 lines of UI JavaScript have one 20-line test file covering i18n.t only. The delta expansion in turnstream.js, the composer and the permission flows are untested.
  - Kanıt: cat ui/js/*.js ui/js/panes/*.js | wc -l = 10002. ui/test contains only i18n.test.mjs (20 lines). npm run test:ui: 3 tests passed.

- **-1.5** Capped: a clean 'tauri build' could not be checked, because building the release is forbidden for this evaluation.
  - Kanıt: Not run, by rule. The debug build through 'npx tauri dev' compiled and ran cleanly.

- **-1** Releases are not code-signed for Windows (Authenticode) or macOS (notarization), so installers will trigger SmartScreen and Gatekeeper warnings, unlike Claude Desktop.
  - Kanıt: release.yml signs only the updater manifest (TAURI_SIGNING_PRIVATE_KEY, lines 14-15 and 85-86). A grep for APPLE_, notar, certificateThumbprint, signCommand or codesign in .github and tauri.conf.json finds nothing.

Kontrol edilemeyenler: tauri build and the installer bundles (release build forbidden). Passing: cargo clippy --all-targets -- -D warnings exits 0; cargo test passes 199, fails 0, 12 ignored benchmarks; npm run lint exits 0.

## Backlog maddeleri

### EV-01 — Account-mode chat has no tools: MCP, web search and chat search are dead, but the composer still offers them

- Önem: **high**, efor: **M**, puan: **-3.5**, kategori: parity
- Dosyalar: `src-tauri/src/claudecode.rs`, `ui/js/composer.js`, `src-tauri/src/engine.rs`
- Kanıt: claudecode.rs:789-807 passes --tools "" --strict-mcp-config --setting-sources "" for chats; :793-797 force web_search and chat_search off. Live: the weather MCP server was connected and listed in the composer tools menu (eval\s09_tools_mcp.png), and the model replied 'NO TOOL' (eval\s10_chat.png).
- Düzeltme ipucu: In chat mode, write the app's mcpServers to a temporary --mcp-config, allow WebSearch/WebFetch and mcp__* through --tools/--allowedTools, and answer can_use_tool with the same per-tool Ask/Allow/Block policy. Expose conversation_search/recent_chats as a small in-process MCP server. Until then, hide or disable the MCP and web-search controls in account mode.

### EV-02 — No sync of chats and projects with the Claude account

- Önem: **high**, efor: **L**, puan: **-2**, kategori: parity
- Dosyalar: `src-tauri/src/db.rs`, `src-tauri/src/import.rs`
- Kanıt: Storage is local SQLite only (db.rs:21-45). claude.ai history arrives only through a one-time export import (import.rs:1082).
- Düzeltme ipucu: At minimum, offer a periodic or incremental import of the account's conversations and projects, and say clearly in onboarding that history is local. Full two-way sync would need account APIs.

### EV-03 — Remote MCP lacks OAuth, custom connectors, MCP Apps and an extension directory

- Önem: **high**, efor: **L**, puan: **-2**, kategori: parity
- Dosyalar: `src-tauri/src/mcp.rs`, `ui/js/settings.js`
- Kanıt: mcp.rs:1062-1081 supports only url plus static headers. The settings UI is raw JSON editing plus install from file (eval\s08_mcp2.png).
- Düzeltme ipucu: Implement the MCP authorization spec (protected-resource metadata, dynamic client registration, PKCE, loopback redirect, token refresh in the keyring), add an 'Add custom connector (URL)' dialog, and render ui:// resources for MCP Apps.

### EV-04 — Missing desktop integrations: dictation, computer use, Claude in Chrome, Cowork

- Önem: **medium**, efor: **L**, puan: **-2**, kategori: parity
- Dosyalar: `ui/js/composer.js`, `src-tauri/src/lib.rs`
- Kanıt: A grep for dictation, getUserMedia and computer use finds nothing. The Claude Desktop asar ships dictation.*.js, computerUseTeach.js, frame-shell-chrome.js and coworkArtifact.js.
- Düzeltme ipucu: Start with dictation (the WebView2 Web Speech API, or a local whisper build). Computer use and Chrome need native capture and input layers; scope them explicitly in the roadmap.

### EV-05 — AskUserQuestion is treated as a permission and never answered in the Code tab

- Önem: **medium**, efor: **S**, puan: **-1**, kategori: parity
- Dosyalar: `src-tauri/src/claudecode.rs`, `ui/js/chat.js`
- Kanıt: No AskUserQuestion handling anywhere. claudecode.rs:1248-1297 returns updatedInput=input with no answers. The CLI 2.1.326 binary expects the host to answer through updatedInput.
- Düzeltme ipucu: For tool_name=='AskUserQuestion', render the questions and options as a card and return updatedInput {...input, answers:{question: choice}}.

### EV-06 — A reply being streamed is not saved until it finishes, so a crash loses it

- Önem: **medium**, efor: **S**, puan: **-1**, kategori: stability
- Dosyalar: `src-tauri/src/engine.rs`
- Kanıt: engine.rs:998 inserts an empty assistant row. engine.rs:1141-1158 writes the content only after the stream ends. on_event (engine.rs:1062-1068) never writes to the database.
- Düzeltme ipucu: Throttle a msg_update of the live content (for example every 2 s, or every N KB) inside on_event, or from live_tick, on the blocking pool.

### EV-07 — Child processes are orphaned if the app crashes or is killed

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: stability
- Dosyalar: `src-tauri/src/lib.rs`, `src-tauri/src/codetools.rs`, `src-tauri/src/mcp.rs`, `src-tauri/src/claudecode.rs`, `src-tauri/src/terminal.rs`
- Kanıt: Cleanup runs only in RunEvent::Exit (lib.rs:521-541). No Job Object anywhere. kill_process_tree (codetools.rs:962) runs only on an explicit stop.
- Düzeltme ipucu: On Windows, create one Job Object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE at startup and assign every spawned child to it. On Linux, set PR_SET_PDEATHSIG in pre_exec.

### EV-08 — MCP connections do not recover, and tool-list changes are ignored

- Önem: **medium**, efor: **S**, puan: **-1**, kategori: stability
- Dosyalar: `src-tauri/src/mcp.rs`
- Kanıt: mcp.rs:1160-1165 marks the server failed when its process exits, with no retry. mcp.rs:1032-1046 drops notifications, including notifications/tools/list_changed.
- Düzeltme ipucu: Restart a crashed stdio server with backoff (1, 2, 4… s, capped). On tools/list_changed (and prompts/resources), call refresh_tools and emit mcp-changed.

### EV-09 — React artifacts cannot import shadcn/ui and have no window.claude or storage APIs

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: parity
- Dosyalar: `src-tauri/src/artifacts.rs`
- Kanıt: artifacts.rs:299-321: the import map has no '@/components/ui/*' entries.
- Düzeltme ipucu: Map @/components/ui/* to a prebuilt shadcn-compatible ESM bundle, and add a postMessage bridge for window.claude.complete and storage, handled by the parent.

### EV-10 — No deep links

- Önem: **low**, efor: **S**, puan: **-1**, kategori: parity
- Dosyalar: `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`
- Kanıt: No tauri-plugin-deep-link; the plugin list is at lib.rs:241-266.
- Düzeltme ipucu: Add tauri-plugin-deep-link with an app scheme (open conversation, new chat with prompt, install MCP extension) and route URLs through the existing single-instance handler.

### EV-11 — Only English and Turkish

- Önem: **low**, efor: **M**, puan: **-1**, kategori: parity
- Dosyalar: `ui/js/i18n.js`
- Kanıt: i18n.js:1259-1263 lists auto, en and tr. Claude Desktop ships about 50 locales.
- Düzeltme ipucu: Move strings to per-locale JSON files and add at least de, fr, es, ja, ko, pt-BR, zh.

### EV-12 — Streaming IPC re-sends the whole growing text part on every tick

- Önem: **low**, efor: **M**, puan: **-1**, kategori: performance
- Dosyalar: `src-tauri/src/turnstream.rs`, `ui/js/turnstream.js`
- Kanıt: Measured 355 'turn' events and 2.37 MB for a 5,247-character reply (max 13.9 KB per event).
- Düzeltme ipucu: For the part that is streaming, send an append-only delta (new markdown source or new HTML suffix plus the last re-rendered block) instead of the full part HTML.

### EV-13 — A new Claude Code CLI process is spawned for every turn

- Önem: **low**, efor: **M**, puan: **-1**, kategori: performance
- Dosyalar: `src-tauri/src/claudecode.rs`
- Kanıt: claudecode.rs:894-911 spawns per user message; :1331 closes stdin so the CLI exits. CLI boot measured at about 670 ms (3 runs), plus the --resume transcript load.
- Düzeltme ipucu: Keep one CLI per session alive in stream-json input mode between turns (idle timeout), and send the next user line on the same stdin.

### EV-14 — The terminal 'Ask Claude' bar covers terminal output

- Önem: **low**, efor: **S**, puan: **-1**, kategori: ux
- Dosyalar: `ui/js/panes/terminal.js`, `ui/css/app.css`
- Kanıt: panes/terminal.js:105. Screenshots eval\s18_perm.png and eval\s28_dark_code.png.
- Düzeltme ipucu: Give the bar its own row above the xterm, or reserve right padding and a top inset for it.

### EV-15 — The 'N files edited' summary miscounts lines and shows absolute Windows paths

- Önem: **low**, efor: **S**, puan: **-1**, kategori: ux
- Dosyalar: `ui/js/chat.js`
- Kanıt: eval\s20_done.png: the summary shows +5 -2, the Edit row +3 -0, and git diff --stat 3 insertions. chat.js:620 checks only '/'; chat.js:623-624 count whole old and new strings.
- Düzeltme ipucu: Reuse the per-tool diff stat the tool rows already show, and normalize '\\' to '/' before making paths relative.

### EV-16 — Mixed-language UI: English model descriptions and backend errors in a localized UI

- Önem: **low**, efor: **M**, puan: **-1**, kategori: ux
- Dosyalar: `src-tauri/src/models.rs`, `src-tauri/src/error.rs`, `ui/js/i18n.js`
- Kanıt: models.rs:86. Screenshots eval\s05_model.png, eval\s26_apierr.png and eval\s08_mcp2.png.
- Düzeltme ipucu: Return error codes with parameters from Rust and translate them in i18n.js; translate the model descriptions by id.

### EV-17 — No end-to-end test of the turn loop (stream, persist, tools, drop, stop)

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: tests
- Dosyalar: `src-tauri/src/engine.rs`, `src-tauri/src/claudecode.rs`, `dev/mock_api.py`
- Kanıt: No test calls engine::run or claudecode::run_once. Only the parts are unit-tested.
- Düzeltme ipucu: Add tokio tests with a local SSE server: normal reply, tool_use round trip, connection dropped mid-stream, overloaded fallback, and Stop. Add a fake-CLI script for run_once.

### EV-18 — About 10k lines of UI JavaScript have almost no tests

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: tests
- Dosyalar: `ui/js/turnstream.js`, `ui/js/chat.js`, `ui/js/composer.js`, `ui/test`
- Kanıt: 10,002 lines of UI JS. ui/test/i18n.test.mjs is the only test file (20 lines, 3 tests).
- Düzeltme ipucu: Add node --test suites for turnstream expansion (keyframe and delta, a missed seq), editedFiles, and the composer state, using jsdom or pure functions.

### EV-19 — Installers are not code-signed (Authenticode, notarization)

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: packaging
- Dosyalar: `.github/workflows/release.yml`, `src-tauri/tauri.conf.json`
- Kanıt: Only the updater signing key is configured. There are no signing identities or notarization steps.
- Düzeltme ipucu: Add Windows signing (Azure Trusted Signing, or signCommand with a certificate) and an Apple Developer ID with notarytool in release.yml.

### EV-20 — open_path and reveal_path accept any path from the webview

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: security
- Dosyalar: `src-tauri/src/commands.rs`
- Kanıt: commands.rs:1284-1293 has no allow-list. The UI uses them only for the MCP config and the data dir (settings.js:304, :449), plus reveal for session folders.
- Düzeltme ipucu: Accept an enum of known targets (config, data_dir) or check paths against the data dir and the session roots. Use reveal_item_in_dir instead of open for anything else.

### EV-21 — Code tab has no remote, SSH or cloud environments

- Önem: **low**, efor: **L**, puan: **-0.5**, kategori: parity
- Dosyalar: `src-tauri/src/claudecode.rs`, `ui/js/code.js`
- Kanıt: The session picker offers only a local folder (eval\s14_code.png). There is no ssh or remote code path.
- Düzeltme ipucu: Run the CLI over ssh (stdio passthrough) for a remote environment.

### EV-22 — Account-mode chats get no generated titles

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: ux
- Dosyalar: `src-tauri/src/engine.rs`
- Kanıt: engine.rs:1751-1752 needs an API key. The sidebar shows the truncated first message (eval\s10_chat.png).
- Düzeltme ipucu: Generate the title through a short CLI call (Haiku, no tools), or have the main turn emit a title.

### EV-23 — Layout nits: clipped engine chip, inconsistent cost display, narrow-window sidebar without backdrop

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: ux
- Dosyalar: `ui/css/app.css`, `ui/js/chat.js`, `ui/js/sidebar.js`
- Kanıt: eval\s18_perm.png ('Claude C' clipped), eval\s10_chat.png ($0.000 in the header against $0.00039 in the footer), eval\s12_narrow.png.
- Düzeltme ipucu: Give the chip a min-width or an icon-only fallback, use the same cost formatter in header and footer, and add a scrim with auto-collapse below about 700 px.

## Güçlü yanlar (bozulmamalı)

- Clean build hygiene: cargo clippy --all-targets -- -D warnings exits 0, and cargo test passes 199 with 12 ignored benchmarks, including tests for hostile SSE streams, MCP servers that exit or flood stderr, migrations, and permission parsing.
- Panic hardening: turns run under catch_unwind (engine.rs:728). Outside tests, unwrap and expect appear only on static regexes and piped stdio. parking_lot guards cannot be held across awaits in Send tasks, so locks cannot cross await points.
- Security basics are solid. Markdown escapes raw HTML and allows only http, https and mailto links (markdown.rs:28, :273); the main CSP is script-src 'self'. Artifacts run in an opaque-origin sandboxed iframe with their own CSP. The permission engine uses allow-list parsing, PowerShell and bash quirks, protected .git and .claude folders, and a UNC/NTLM guard (codetools.rs:308-802).
- Orderly shutdown is clean: closing the dev copy stopped the MCP server, the PTY PowerShell and the CLI with no orphans (checked with Get-CimInstance).
- Lightweight runtime: the release copy idles at 0% CPU and about 387 MB working set / 191 MB private including WebView2, against 2.1 GB for the busy Electron Claude Desktop. A 400-message conversation opens in 182 ms; scrolling shows 6.1 ms per frame at p95; streaming produced no long tasks; process to first paint took about 0.6 s in the debug build.
- Database durability: WAL plus a separate read-only reader connection, versioned migrations in one transaction that refuse newer schemas, cleanup of unfinished imports, and a recovery dialog that keeps an unreadable database file (lib.rs:205-237, db.rs:297-355).
- The Code tab drives the real Claude Code CLI through its stdio control protocol: permission cards with diffs, a session allow option, a git changes pane with commit and PR, a PTY terminal that reattaches after reload, worktrees, preview, and rebindable shortcuts.
- MCP clients cover stdio and Streamable HTTP with session renewal, cancellation notifications, size caps, and per-tool Ask/Allow/Block in a clear status UI.

## Yöntem

I scored HEAD 3c2543c (tag v0.1.1) read-only. Commands run: cargo clippy --all-targets -- -D warnings (exit 0), cargo test --no-fail-fast (199 passed, 12 ignored), npm run lint (exit 0) and npm run test:ui (3 passed). I read lib.rs, engine.rs, anthropic.rs, mcp.rs, claudecode.rs, codetools.rs, commands.rs, db.rs, markdown.rs, artifacts.rs, terminal.rs and attachments.rs, plus parts of the UI JS, grepping for unwrap and slicing panics, lock types, process cleanup, and XSS sinks.

UI checks: I started an isolated dev copy (identifier dev.clauderust.designdev) under npx tauri dev with CDP on port 9333 and drove it with the cdp.mjs helper plus small scripts of my own in scratchpad\eval. I signed in through the CLI account button and set up a stdio, a broken and an unreachable MCP server. I ran 4 Haiku turns and 1 Opus turn, all on the account (the Opus one was $0.06). Each turn was measured for streaming events and bytes and checked for long tasks. I triggered a Code-tab edit permission and checked the Changes pane, terminal and preview, plus light and dark themes, a 420 px width, and the error and empty states. I imported a synthetic export of 301 conversations, one of them with 400 messages, and timed IPC, open and scroll. CLI cold start was measured without any model call.

Measurements: the owner's release process (PID 13716, read only) and Claude Desktop's processes were measured through Win32_Process and CPU-time deltas.

Reference: the Claude Desktop asar listing (locales, feature modules) and the strings in the claude.exe 2.1.326 binary.

Cleanup: I closed the dev copy normally, deleted %APPDATA%\dev.clauderust.designdev, and confirmed git status unchanged. The rules ruled out tauri build and the release build, so those areas are capped.
