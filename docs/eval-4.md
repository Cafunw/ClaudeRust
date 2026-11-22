# Puanlayıcı 4: 87/100 (v0.1.5)

Bağımsız, salt okunur puanlayıcının tam raporu. Her kesinti kanıtıyla birlikte; PROJECT_PROGRESS.md backlog'u bu raporun EV-xx maddelerini izler.

| Kategori | Puan |
|---|---|
| Claude Desktop feature parity | 24.5/30 |
| Stability and crash resistance | 24/25 |
| Native performance and resource use | 18/20 |
| UI/UX polish and interaction quality | 12.5/15 |
| Code quality, tests and packaging | 8/10 |

## Kesintiler

### Claude Desktop feature parity (24.5/30)

- **-2** MCP: the connector directory is small, and there is no extension directory or account connectors. Desktop users get a large directory with one-click OAuth connectors, first-party Google Drive/Gmail/Calendar/Microsoft 365, a browsable Desktop Extensions directory, and remote connectors synced from their account. ClaudeRust can only add custom servers and install .mcpb files from disk.
  - Kanıt: src-tauri/src/connector_directory.json lists 15 entries (GitHub, Linear, Notion, Sentry, Atlassian, Asana, Stripe, PayPal, Intercom, Square, Figma, Supabase, Neon, Context7, DeepWiki). Desktop 6.18355 ships resources/office365-mcp and resources/github-mcp in app.asar, plus the claude.ai connector directory. Settings > Developer screenshot (mcp_dark.png) shows only Add custom / Browse / Install extension from file.

- **-1.5** Artifacts: ClaudeRust has only the classic in-chat types. Current Desktop also ships typed artifacts (Slides, Design) with downloads, comments and shared db/room, plus publishing and sharing. ClaudeRust cannot publish, share, comment on or remix an artifact. I verified that these types ship in Desktop; I could not verify which Desktop chat flows create them.
  - Kanıt: src-tauri/src/artifacts.rs:58-68 and 229-259 list only code/html/svg/react/mermaid/markdown. Desktop app.asar has /resources/frame/types/slides/slides.artifact.json ("Presentation decks... downloads, comments, db, room") and /resources/frame/types/design/*. ui/js/artifacts.js has no publish or share code.

- **-1** Streaming: the reply is produced only on the client. If the network drops or the app quits mid-reply, the answer stays cut off, and the only fix is Retry, which regenerates the whole answer. There is no continue-from-partial. Desktop's generation keeps running on the server and the full reply is there when the client reconnects.
  - Kanıt: Live test: killed the mock API 5 s into a 78 KB stream. The partial reply was kept with "Bağlantı, yanıt tamamlanmadan kesildi" and one "Yeniden dene" button (drop2.png). src-tauri/src/anthropic.rs:1016 test a_stream_that_goes_quiet_in_the_reply_ends_with_what_came; lib.rs:336 mark_interrupted on restart.

- **-0.5** Artifacts: React, Mermaid and Tailwind artifacts load their libraries from public CDNs at runtime, so they break offline or behind a CDN block. Desktop bundles these libraries locally.
  - Kanıt: src-tauri/src/artifacts.rs:350 (cdn.jsdelivr mermaid), :393 (cdn.tailwindcss.com), :406-412 (esm.sh react, react-dom, recharts, lodash). Desktop app.asar /resources/artifact-libs/ holds mermaid 11.17.2, chart.js 4.5.0 and gridjs.

- **-0.5** Attachments: the only sources are a local file, a screen capture and MCP prompts/resources. Desktop also attaches from GitHub repositories and Google Drive.
  - Kanıt: ui/js/composer.js:494-505 attachMenu items: upload_file, add_screenshot, prompts_resources.

Kontrol edilemeyenler: I could not drive Claude Desktop's own window, so its behaviours come from its shipped resources (app.asar listing, resources/frame/types, artifact-libs, office365-mcp) and known product features. I did not check whether Desktop chat creates Slides/Design artifacts. Streaming, MCP permission and reconnect, artifacts, branching, search, the model menu and attachments were all exercised live in ClaudeRust. MCP tools in account-engine chats were verified only in code (claudecode.rs:806-860), not live.

### Stability and crash resistance (24/25)

- **-0.5** The artifact sandbox allows network egress anywhere, so an artifact can send its contents or window.storage data to any https host or image URL. Desktop's artifact sandbox blocks external network calls.
  - Kanıt: src-tauri/src/artifacts.rs:288 ARTIFACT_CSP has connect-src https: data: blob:; img-src *; media-src *. The iframe sandbox is ui/js/artifacts.js:165.

- **-0.5** Secrets on Linux: the API key falls back to the app's SQLite database in plain text. MCP OAuth tokens are kept only in memory, so they are lost on every restart.
  - Kanıt: src-tauri/src/settings.rs:337-370: the keyring path is cfg'd to macOS/Windows, otherwise db.setting_set(KEY_API, k). src-tauri/src/mcp_oauth.rs:130-132: "shortcut: Linux has no secret-store dependency... a sign-in lasts until the app quits".

Kontrol edilemeyenler: Linux and macOS were not run. I did not test disk-full or a corrupted database (the offer_fresh_db dialog exists at lib.rs:224). Lock-order deadlocks were reviewed only by reading: MCP uses tokio RwLock held briefly, and the parking_lot guards are !Send, so they cannot be held across an await in spawned tasks. Live checks all passed: unreachable server (4 retries, then a clear error with Retry), API killed mid-stream (partial reply kept), MCP stdio server killed (reconnecting, then connected in under 4 s), a 5.5 MB paste (became a digested attachment in 16 ms), and the CLI process exited after each turn. Outside test modules, unwrap/expect are almost all static Regex compiles.

### Native performance and resource use (18/20)

- **-1** The account engine, which is the default path for subscription users without an API key, starts a new Claude Code CLI process for every turn. This adds about 0.7 s of overhead to first-token time and a process of about 200 MB per turn. Desktop's chat streams directly. Desktop's own first-token time was not measurable.
  - Kanıt: Live Haiku 5.5, low effort: first text in the DOM 3.45 s after send, turn total 4.9 s, while the turn's own meta reported 2.7 s. CLI child process 209 MB working set. Spawn per turn at src-tauri/src/claudecode.rs:1057-1190 (args, cmd.spawn, stdout take).

- **-1** Streaming sends far more IPC than the text it carries. The full rendered turn is re-sent as a keyframe every second, which grows quadratically on long replies, and the renderer takes about 28% of a core while streaming.
  - Kanıt: Mock stream of a 77,900-character reply with 30 code blocks over 21.1 s: 401 turn events and 6.1 MB of IPC (about 79 times the text). Per-process CPU during the stream: renderer 28%, GPU 10%, Rust 4%, WebView browser 2%, total 44% of one core. src-tauri/src/turnstream.rs:39 KEYFRAME_EVERY = 1000 ms.

Kontrol edilemeyenler: Desktop's startup time and streaming CPU were not measured. It is the live host of this session, so it was not restarted or driven. Its memory was sampled while it was active.

### UI/UX polish and interaction quality (12.5/15)

- **-1** White flash at startup in dark theme: the main window is shown in setup before the WebView paints, and no background colour is set.
  - Kanıt: Screen capture of the release copy (dark theme): the main window became visible 623 ms after launch with a pure white client area (RGB 255,255,255 at launch+633 ms) and was dark (25,25,25) at launch+773 ms. src-tauri/src/lib.rs:376-384 calls w.show() in setup; tauri.conf.json windows[0] has no backgroundColor.

- **-0.5** The artifact panel stays open and empty, taking about half the window, after you switch to a branch that has no artifact.
  - Kanıt: branch.png: after editing the message (branch 2/2), the right panel shows only "Henüz artifact yok" next to a narrow chat column.

- **-0.5** Mixed languages and inconsistent naming in the UI.
  - Kanıt: ui/js/composer.js:580 hard-codes English: `${t('pasted')} (${n} lines)`, which renders as "Yapıştırıldı (120001 lines)" (paste_big.png). The same untitled chat is "Yeni sohbet" in the sidebar but "Adsız" in search results (palette.png; i18n.js:1341).

- **-0.5** Small rendering-polish issues.
  - Kanıt: Search results show raw markdown table pipes and headings ("...16 | 12 | ## Section 5", palette.png). A code block sits flush against a following table with about 2 px gap, in both themes (big1000.png, light_chat.png).

Kontrol edilemeyenler: I did not test with a screen reader. Drag-and-drop from Explorer was not tested; only paste and the attach menu were. I drove the UI over CDP on the isolated release copy rather than the dev copy, which avoided a third concurrent compile. The UI is otherwise the same.

### Code quality, tests and packaging (8/10)

- **-1** The account-engine turn loop has no end-to-end test. claudecode::run spawns the CLI and parses its stream-json, permission requests, resume/fork and partial messages, and it is the main path for subscription users. Only its helpers are unit-tested; the real-CLI tests are #[ignore] and there is no fake-CLI driver for run().
  - Kanıt: src-tauri/src/claudecode.rs:600 pub async fn run. None of the 24 tests in its test module call run() (names checked). a_real_cli_connects_the_chat_tools (:2161) and a_real_cli_writes_a_title (:2221) are #[ignore]. tests/chat_memory.rs uses a fake CLI only for memory synthesis.

- **-0.5** Several modules are very large and mix many concerns, which raises the cost of changes and review.
  - Kanıt: commands.rs is 3,432 lines with 211 fns; engine.rs 2,713; codetools.rs 2,617; claudecode.rs 2,600; mcp.rs 2,510. ui/js has about 13,100 lines of untyped JS with 136 innerHTML assignments that depend on manual esc() (I spot-checked these and found them escaped).

- **-0.5** Packaging and tooling friction.
  - Kanıt: tauri.conf.json sets createUpdaterArtifacts: true, so a plain `npx tauri build` needs the private signing key; contributors must pass tauri.no-updater.conf.json. `npm run test:ui` prints MODULE_TYPELESS_PACKAGE_JSON warnings for every module because package.json has no "type": "module".

Kontrol edilemeyenler: I did not run `npm run e2e` (ui/e2e): the brief does not list it, and it launches windows while the machine is in use. macOS and Linux bundles were not built.

## Backlog maddeleri

### EV4-01 — MCP connector directory is small; no extension directory or account connectors

- Önem: **high**, efor: **L**, puan: **-2**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/connector_directory.json`, `src-tauri/src/connector_directory.rs`, `ui/js/connector_directory.js`
- Kanıt: Only 15 remote connectors are bundled. There is no browsable Desktop Extensions directory and no first-party Drive/Gmail/Calendar/M365 equivalents. Desktop ships office365-mcp and github-mcp, plus the claude.ai directory.
- Düzeltme ipucu: Fetch the public MCP registry, or a maintained JSON list, at runtime with search and categories. Add an extensions directory that downloads and installs .mcpb files through the existing extensions.rs. Make the OAuth one-click flow the default for remote entries.

### EV4-02 — Artifacts: no publish/share and no typed artifacts (slides/design)

- Önem: **medium**, efor: **L**, puan: **-1.5**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/artifacts.rs`, `ui/js/artifacts.js`
- Kanıt: artifacts.rs:58-68 has only classic types. Desktop app.asar ships resources/frame/types/slides and design, with downloads, comments and db/room capabilities. ClaudeRust has no publish, share or remix action.
- Düzeltme ipucu: Add a slide-deck artifact type (16:9 pages, paging, PDF/PPTX export) rendered in the same sandbox. Add an 'Export as standalone HTML/zip' or 'Publish to gist/static host' action as the local equivalent of publishing.

### EV4-03 — A dropped or interrupted reply can only be fully regenerated

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/engine.rs`, `src-tauri/src/anthropic.rs`, `ui/js/retry_ui.js`
- Kanıt: Killing the API mid-stream kept the partial reply with a 'connection cut' error and only a Retry button (drop2.png). Desktop's server-side generation completes regardless.
- Düzeltme ipucu: Offer 'Continue': send the partial assistant content as a prefill (the API engine) or resume the CLI session at the last uuid (the account engine). Retry the stream automatically once on a mid-reply transport error before showing the error.

### EV4-04 — Account engine starts a new CLI process per turn (slower first token, about 200 MB spike)

- Önem: **medium**, efor: **L**, puan: **-1**, kategori: Native performance and resource use
- Dosyalar: `src-tauri/src/claudecode.rs`
- Kanıt: Haiku 5.5, low effort: first text 3.45 s after send against 2.7 s of model time; CLI child 209 MB working set. Spawn at claudecode.rs:1057-1190.
- Düzeltme ipucu: Keep one warm CLI process per active conversation (stream-json input already supports several user messages) and reuse it for the next turn. Kill it after an idle timeout. Alternatively, pre-spawn the CLI when the composer gets focus.

### EV4-05 — Streaming IPC is about 79x the text, and the renderer takes about 28% of a core

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: Native performance and resource use
- Dosyalar: `src-tauri/src/turnstream.rs`, `src-tauri/src/engine.rs`, `ui/js/turnstream.js`
- Kanıt: A 77.9 KB reply produced 401 events and 6.1 MB of IPC. Total CPU was 44% of a core (renderer 28%, GPU 10%). KEYFRAME_EVERY = 1000 ms re-sends the full rendered turn (turnstream.rs:39).
- Düzeltme ipucu: Send keyframes only on resync request, or when no delta is possible, instead of every second. Diff the rendered HTML per markdown block so finished code blocks are never re-sent or re-parsed.

### EV4-06 — White flash at startup in dark theme

- Önem: **medium**, efor: **S**, puan: **-1**, kategori: UI/UX polish and interaction quality
- Dosyalar: `src-tauri/src/lib.rs`, `src-tauri/tauri.conf.json`
- Kanıt: The window was visible at 623 ms with a white client area (RGB 255,255,255), turning dark at about 773 ms. lib.rs:382 calls show() in setup before the page paints, and there is no backgroundColor.
- Düzeltme ipucu: Set the window and webview background colour from the saved theme before show(). Or show the window from the page after the first frame (invoke a 'ready' command on DOMContentLoaded), with a short fallback timer.

### EV4-07 — No fake-CLI end-to-end test of claudecode::run

- Önem: **high**, efor: **M**, puan: **-1**, kategori: Code quality, tests and packaging
- Dosyalar: `src-tauri/src/claudecode.rs`, `src-tauri/tests/`
- Kanıt: run() (claudecode.rs:600) is untested; the 2 real-CLI tests are #[ignore]. It is the main turn loop for subscription users (spawn, stream-json, permissions, resume/fork, stop).
- Düzeltme ipucu: Add a fake 'claude' script, like the one in tests/chat_memory.rs, that replays recorded stream-json (partial messages, a permission control_request, result, error and crash). Assert the stored tree, live events, permission round-trip and process cleanup.

### EV4-08 — Artifact libraries load from public CDNs at runtime

- Önem: **low**, efor: **M**, puan: **-0.5**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/artifacts.rs`
- Kanıt: artifacts.rs:350 (mermaid on jsdelivr), :393 (tailwind CDN), :406-412 (esm.sh react, recharts, lodash). Desktop bundles resources/artifact-libs.
- Düzeltme ipucu: Vendor the pinned React, Mermaid, Tailwind and recharts builds and serve them through the artifact:// protocol. Keep the CDN only as a fallback.

### EV4-09 — No GitHub or Google Drive attachment sources

- Önem: **low**, efor: **L**, puan: **-0.5**, kategori: Claude Desktop feature parity
- Dosyalar: `ui/js/composer.js`
- Kanıt: attachMenu (composer.js:494-505) offers only a local file, a screenshot and MCP prompts/resources.
- Düzeltme ipucu: Add 'Add from GitHub' (repo/branch/file picker over the GitHub API or gh) that stages files as project or message attachments. Reuse the MCP resources picker for Drive-like connectors.

### EV4-10 — Artifact sandbox allows network egress anywhere

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/artifacts.rs`
- Kanıt: ARTIFACT_CSP (artifacts.rs:288) has connect-src https:; img-src *; media-src *. An injected artifact can send its content or window.storage data to any host.
- Düzeltme ipucu: Restrict connect-src and img-src to data:/blob: plus the vendored library origin. Proxy any allowed fetches through the parent with a confirmation, as is done for links.

### EV4-11 — Linux stores the API key in plain text in SQLite

- Önem: **low**, efor: **M**, puan: **-0.5**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/settings.rs`, `src-tauri/src/mcp_oauth.rs`
- Kanıt: settings.rs:337-370 falls back to db.setting_set(KEY_API). mcp_oauth.rs:130 keeps Linux MCP tokens only in memory.
- Düzeltme ipucu: Enable keyring's Secret Service (linux-native) backend for both the API key and MCP tokens, and fall back to the database only with a visible warning.

### EV4-12 — Artifact panel stays open and empty after a branch switch

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: UI/UX polish and interaction quality
- Dosyalar: `ui/js/artifacts.js`, `ui/js/chat.js`
- Kanıt: branch.png: after editing a message, the panel shows only 'Henüz artifact yok' at half the window width.
- Düzeltme ipucu: When the current branch has no artifacts, close the panel, or collapse it to a reopenable tab.

### EV4-13 — Mixed-language strings and inconsistent untitled-chat name

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: UI/UX polish and interaction quality
- Dosyalar: `ui/js/composer.js`, `ui/js/i18n.js`, `ui/js/search.js`
- Kanıt: composer.js:580 hard-codes 'lines' ('Yapıştırıldı (120001 lines)'). An untitled chat is 'Yeni sohbet' in the sidebar and 'Adsız' in search.
- Düzeltme ipucu: Move the line count into i18n with a {n} placeholder. Use a single untitled label everywhere. Add an i18n lint that flags literal English inside template strings.

### EV4-14 — Raw markdown in search snippets; code block flush against a following table

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: UI/UX polish and interaction quality
- Dosyalar: `ui/js/search.js`, `src-tauri/src/db.rs`, `ui/css/app.css`
- Kanıt: palette.png snippet: '...16 | 12 | ## Section 5'. big1000.png and light_chat.png: about 2 px between a code block and the next table.
- Düzeltme ipucu: Index rendered plain text (strip markdown and table pipes) for FTS snippets. Give .code-block and .table-wrap a consistent block margin.

### EV4-15 — Oversized modules

- Önem: **low**, efor: **L**, puan: **-0.5**, kategori: Code quality, tests and packaging
- Dosyalar: `src-tauri/src/commands.rs`, `src-tauri/src/engine.rs`, `src-tauri/src/codetools.rs`, `src-tauri/src/claudecode.rs`, `src-tauri/src/mcp.rs`
- Kanıt: commands.rs 3,432 lines with 211 fns; engine.rs 2,713; codetools.rs 2,617; claudecode.rs 2,600; mcp.rs 2,510.
- Düzeltme ipucu: Split commands.rs by domain (chat, project, git, mcp, code) and engine.rs into request building, stream loop and tool execution, without changing the command names.

### EV4-16 — Build needs a signing key by default; test runner warnings

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: Code quality, tests and packaging
- Dosyalar: `src-tauri/tauri.conf.json`, `package.json`
- Kanıt: createUpdaterArtifacts: true makes a plain `npx tauri build` require TAURI_SIGNING_PRIVATE_KEY. `npm run test:ui` prints MODULE_TYPELESS_PACKAGE_JSON for each ESM module.
- Düzeltme ipucu: Default to createUpdaterArtifacts: false and turn it on in the release workflow's config. Add "type": "module" to package.json, or rename the test helpers to .mjs.

## Güçlü yanlar (bozulmamalı)

- Resource use is far below Desktop. The release copy idles at 7 processes and about 172-189 MB private (the Rust process itself 8 MB) at about 0.5-1% of one core. Desktop 6.18355 measured 14 processes, 2,119 MB private and 1,260 MB working set.
- The installers are small: NSIS 8.37 MiB, MSI 11.25 MiB, exe 28.5 MB. Desktop ships claude.exe (250 MB), app.asar (100 MB) and ion-dist.asar (221 MB).
- Long conversations render fast. A 1000-turn (2000-message) chat with code and tables opens in 122 ms; it is paged 40 turns at a time with bounded DOM (1,844 nodes after paging 640 turns).
- Streaming is smooth. During a 78 KB code-heavy reply there were no long tasks and the largest frame gap was 59 ms. The Rust backend used only 4% of a core while re-rendering markdown and syntect.
- Error handling is robust and visible: retry countdown (1/4), clear final error with Retry, partial reply kept on a mid-stream drop, and stdio MCP servers restarted with backoff (reconnected in under 4 s).
- MCP support is broad: stdio, streamable HTTP and SSE, OAuth, per-tool Ask/Allow/Block, a permission card with once / this chat / always / deny, prompts and resources, elicitation, MCP Apps, roots, list_changed, cancellation, .mcpb extensions, and import of Desktop's config.
- Model and API configuration goes beyond Desktop: API key or Claude account (via the official CLI), custom base URL, OpenAI-compatible endpoints, effort levels, fast mode, and per-turn cost shown. This is a strength over Desktop in this area.
- Chat history goes beyond Desktop: edit/retry branches with version navigation, fork from any answer, FTS5 search across message content, per-conversation instructions, projects with FTS-based knowledge search for large projects, and styles including custom ones.
- Security work is careful: markdown escapes raw HTML and renders images as links (no auto-loading exfiltration); artifacts run in an opaque sandbox with a strict main-window CSP; open_path/reveal_path go through pathguard; secrets use the OS keyring on Windows/macOS; and each copy's identifier gets its own keyring service.
- The engineering bar is high: clippy -D warnings clean on all targets, 387 Rust tests and 148 UI tests passing, a clean release build with no warnings, and partial-save, migration and transaction tests.

## Yöntem

I did not read PROJECT_PROGRESS.md, AUDIT_AND_ROADMAP.md, docs/audit-1.md, docs/eval-*.md or docs/feature-audit.md. I read the code and the README only to find claims to check, and verified each feature in code or live.

Static review: lib.rs, state.rs, engine/anthropic retry and stall handling, claudecode.rs (flags, chat tools, spawn), mcp.rs (locks, supervise, HTTP session renewal), artifacts.rs (CSP, CDN libraries), markdown.rs (sanitiser), pathguard.rs, settings and secrets, attachments.rs, db.rs (migrations), and the UI innerHTML sinks. I listed unwrap/expect outside test modules.

Tests: cargo clippy (all targets, -D warnings), cargo test, npm run lint and npm run test:ui, all with CARGO_TARGET_DIR=target-eval.

Release build: an isolated release copy (identifier dev.clauderust.evalcopy, CDP port 9334). I used it for both measurements and UI driving instead of the dev copy, which avoided a third concurrent compile; the UI is the same.

Live runs: the repo's dev/mock_api.py and my own long-stream mock, reached through a non-persisted dummy key in the CLAUDERUST_API_KEY env var and base_url set to 127.0.0.1. No keyring writes and no real API key. Then the Claude account engine through the onboarding 'continue as' button, which uses the existing CLI login with no OAuth, for 2 Haiku low-effort turns.

Driving: CDP scripts for screenshots in dark, light and 480 px narrow layouts, empty, error and retry states, artifact panel, branching, MCP settings and permission card, search palette, model menu and a large paste. Memory and CPU came from the process tree of my PID (Win32_Process), with per-process CPU deltas. Startup came from performance paint entries plus a screen capture of the main window at fixed offsets. Desktop features came from the app.asar listing and string greps of the main bundle, plus known product behaviour; uncertainties are stated in each category.

Cleanup: I stopped only my own processes by PID: the eval copy tree, the mocks, and the fake MCP server it started. I deleted %APPDATA%\dev.clauderust.evalcopy, %LOCALAPPDATA%\dev.clauderust.evalcopy (WebView data), D:\ClaudeRust\target-eval, the one CLI transcript I created, and the startup screen captures, because they caught other screen content. I never ran or touched the owner's target\release copy or the dev.clauderust.designdev profile.

## Puansız notlar (kapsam dışı Claude Desktop özellikleri, durumlarıyla)

- Account sync (chats on web/mobile): MISSING. History is local SQLite only (db.rs).
- Cowork: MISSING. No equivalent; Desktop ships cowork-svc.exe.
- Computer use: MISSING. No computer-use code; Desktop ships computerUseTeach.js and computerUseWatchRecord.js.
- Claude in Chrome: MISSING. Desktop ships chrome-native-host.exe; ClaudeRust has nothing comparable.
- Remote/SSH/cloud sessions: MISSING. Desktop ships claude-ssh-broker/proxy; ClaudeRust has only local sessions and worktrees.
- Code tab: EQUAL or BETTER. It runs the same Claude Code CLI 2.1.326 or a built-in Rust agent, with changes, terminal, plan, tasks, preview and map panes, checkpoints and worktrees (code reviewed, not exercised).
- Skills: EQUAL. skills.rs lists, enables, installs and uninstalls Claude Code skills (Settings > Beceriler).
- File creation / code execution in chats: WORSE. code_exec.rs works only with the API-key engine; subscription (account) chats cannot create files.
- Research mode: WORSE. research.rs runs only on the API engine.
- Memory: EQUAL. Chat memory with list/edit/import (memory_cmds.rs, Settings > Bellek).
- Incognito: EQUAL. incognito.rs plus the toolbar toggle (eye icon in screenshots).
- Voice: WORSE. Read-aloud only (readaloud.js, speechSynthesis); no voice mode or dictation.
- Usage limits: EQUAL. A weekly-limit banner and sidebar meter (80%) were shown live in account mode.
- Sidebar organisation: BETTER. Sections, pin, archive and unread markers (sidebar_cmds.rs).
- Languages: WORSE. English and Turkish only (i18n.js); Desktop main bundle references 12 locales (de-DE, en-US, es-419, es-ES, fr-FR, hi-IN, id-ID, it-IT, ja-JP, ko-KR, pt-BR, zh-TW).
- Quick Entry: EQUAL. quick.html window plus a global shortcut (lib.rs toggle_quick_entry).
- Deep links: EQUAL. clauderust:// registered only for the production identifier (deeplink.rs:104).
- Tray: EQUAL. build_tray in lib.rs with close-to-tray.
- Auto-update: EQUAL in code (signed tauri updater, updates.rs). Not verified live; the README says releases are not downloadable while the repo is private.

## Ölçümler

Repository state: c85a76b (v0.1.5 + test/e2e commit), working tree clean before and after.
Build: `npx tauri build` (release, LTO, codegen-units=1) into D:\ClaudeRust\target-eval with identifier dev.clauderust.evalcopy, run concurrently with cargo test. Exit 0 in 10m36s with no warnings. clauderust.exe 28,511,744 B; NSIS ClaudeRust_0.1.5_x64-setup.exe 8.37 MiB; MSI 11.25 MiB. For comparison, Desktop 6.18355.0.0 ships claude.exe 250,259,792 B, app.asar 100 MB and ion-dist.asar 221 MB.
Lint and tests: `cargo clippy -p clauderust --all-targets -- -D warnings`: exit 0, 0 warnings (3m09s). `cargo test --no-default-features`: 355 lib + 32 integration passed, 0 failed, 18 ignored (4m31s). `npm run lint`: clean. `npm run test:ui`: 148/148 pass.
Startup (release copy): main window visible 623 ms after launch, white RGB(255,255,255) at +633 ms, dark at +773 ms. First contentful paint 1.23 s after process start on the first run (WebView2 profile creation), 0.81 s and 0.89 s on warm runs.
Idle: first run 7 processes, 172 MB private (424 MB working set), 0.36 s CPU per 30 s. After all tests: 181 MB private excluding the MCP child, about 0.2 s CPU per 30 s. Rust main process 7.8 MB private.
Claude Desktop (running, hosting this session): 14 processes, 1,260 MB working set, 2,119 MB private, 0.17 s CPU per 10 s.
Large conversation: 1000 turns opened with first turns at 122 ms and settled at 184 ms, 40 turns in the DOM. Scroll-back adds 40 turns per step; with 640 turns paged in, 1,844 DOM nodes and 323 MB private total.
Streaming (local mock, 77,900 characters, 30 code blocks, 21.1 s): 401 turn events, 6.1 MB IPC, 0 long tasks, largest frame gap 59 ms. CPU 44% of one core (renderer 28%, GPU 10%, Rust 4%, browser 2%). Private memory peaked at 483 MB and fell to 354 MB after leaving the chat; renderer main thread did 0 ms of work at idle afterwards.
Error paths: unreachable server gave a retry notice (1/4) and then an error with Retry (turn meta 23 s). Mock killed mid-stream: partial reply kept with an error and Retry. MCP stdio server killed: 'reconnecting (try 1 of 5)', connected again within 4 s. 5.5 MB paste became a digested attachment chip in 16 ms.
Account engine (Claude Code CLI 2.1.326, Haiku 5.5, low effort; 2 live turns costing $0.00067 + $0.00017): first text in the DOM 3.45 s after send, turn 4.9 s, model time 2.7 s. The CLI child used 209 MB working set and exited after each turn. Its transcript f57b559f-39a2-4652-bbce-8e59f66d168d.jsonl was deleted afterwards.
