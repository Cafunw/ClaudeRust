# Puanlayıcı 2: 71/100 (v0.1.2)

Bağımsız, salt okunur puanlayıcının tam raporu. Her kesinti kanıtıyla birlikte; PROJECT_PROGRESS.md backlog'u bu raporun EV-xx maddelerini izler.

| Kategori | Puan |
|---|---|
| Claude Desktop feature parity | 16/30 |
| Stability and crash resistance | 19.5/25 |
| Native performance and resource use | 15.5/20 |
| UI/UX polish and interaction quality | 13/15 |
| Code quality, tests and packaging | 7/10 |

## Kesintiler

### Claude Desktop feature parity (16/30)

- **-3** No claude.ai account sync. Chats, projects and history live only in a local SQLite file, and account chats run as one-off `claude -p` CLI turns. A Desktop user's existing web/mobile conversations, projects, styles and memory are not there, and new chats never appear on their other devices. The claude.ai export importer only partly helps.
  - Kanıt: db.rs:130-187 (local schema only); claudecode.rs:236 and 263-267 (account chat runs the CLI in data_dir/chat-cli); claudecode.rs:920-938 (system prompt file per turn); import.rs import_claude_export is a one-time file import

- **-3** The current Desktop's agent surfaces are missing: the Cowork tab/VM, computer use, Claude in Chrome, SSH remote environments and cloud sessions.
  - Kanıt: The Desktop package ships resources/cowork-svc.exe, smol-bin.x64.vhdx, chrome-native-host.exe, claude-ssh-broker.exe and .vite/build/computerUseTeach.js. In ClaudeRust ui/js + src-tauri/src, grep finds 0 hits for cowork, 0 for computer.use, and the only chrome hit is display.rs:95 (an env var)

- **-2** No Agent Skills, file creation (docx/xlsx/pptx/pdf) or code-execution tool in chat.
  - Kanıt: Desktop app.asar has /resources/bundled-skills/{docx,pptx,xlsx,pdf,pdf-reading,frontend-design}.skill. ClaudeRust models.rs:317-323 offers only the web_search/web_fetch server tools. There is no skill loader: the only 'skill' hit is the comment at claudecode.rs:643

- **-1.5** There is no voice dictation, no incognito chat, no Research mode and no cross-chat memory. memory.rs only handles CLAUDE.md for code sessions.
  - Kanıt: grep -i in ui/js and src-tauri/src: dictat 0, microphone 0, incognito 0. memory.rs:1-4 says 'Project rules and memory for code sessions: CLAUDE.md files'. Desktop frame runtime ships dictation.C9bJoIZV.js

- **-1.5** The UI has 2 languages against Desktop's 42 locale files. Native tray menu items and the DB-recovery dialog are hard-coded English even when the UI is Turkish.
  - Kanıt: ui/js/i18n.js:1 'English and Turkish', :1267 dicts = { en, tr }. Desktop app.asar has 42 /resources/i18n/*.json files. lib.rs:148-152 has the tray labels 'Open ClaudeRust'/'New Chat'/'Quit ClaudeRust'; lib.rs:217-223 has the English dialog

- **-1.5** MCP protocol gaps. Remote servers support streamable HTTP only, with no legacy HTTP+SSE transport for older custom connectors. There is no elicitation or sampling, roots/list is answered empty, and there are no MCP Apps (UI resources).
  - Kanıt: mcp.rs:3 '(stdio) and remote (streamable HTTP) servers'; mcp.rs:1230 roots/list -> {roots: []}. grep elicit|sampling|ui:// in mcp.rs/engine.rs/claudecode.rs finds 0 hits. Desktop ships .vite/build/mcpAppView.js and mcp-runtime/directMcpHost.js

- **-1** Artifacts gap. window.claude.complete is deliberately absent. React and Mermaid artifacts pull their libraries from public CDNs, so they break offline. Desktop bundles these libraries.
  - Kanıt: artifacts.rs:239 ARTIFACT_CSP script-src cdn.jsdelivr.net unpkg.com esm.sh; artifacts.rs:241-245 says claude.complete 'only says it isn't there'. Desktop app.asar has /resources/artifact-libs/npm_mermaid@11.17.2, chart.js and gridjs

- **-0.5** Quick Entry accepts text only: no screenshot or window capture and no attachments.
  - Kanıt: ui/js/quick.js is 43 lines: a textarea plus submit -> commands.rs:1325 quick_entry_submit({text})

Kontrol edilemeyenler: Claude Desktop's window cannot be screenshotted, so its features come from its package (app.asar listing, resources) and known behaviour. Not exercised live: Quick Entry (the owner's copy holds Ctrl+Alt+Space), MCP OAuth sign-in (not allowed), the API-key engine (no key may be entered), opening a deep link, installing an update, OS notifications.

### Stability and crash resistance (19.5/25)

- **-2** 'Auto' permission mode is a static allowlist that replaces Claude Code's own auto-mode safety check, even for the CLI engine. In the project it auto-approves Write of any non-sensitive file, then running that file with python/node/bash/sh, plus make, cargo build (build.rs), and npm/yarn/pnpm run/test (package.json scripts). Model-written code, or code planted by a prompt injection from WebFetch, therefore runs with network access and no prompt. The UI promises 'ask before risky commands'.
  - Kanıt: codetools.rs:765-767 (auto: Write allowed unless sensitive_path); codetools.rs:378-394 (python/node/bash script inside the project = ok); codetools.rs:355-376 (make/cargo/npm run allowed); claudecode.rs:898-901 (CLI started in 'default' because 'Auto mode is ours'); claudecode.rs:1394-1398 (the app answers allow itself); i18n.js:571 mode_auto_d

- **-1** The secret-store service name is hard-coded, so any copy with another identifier (dev, portable, test build) reads and overwrites the production copy's API key and MCP OAuth tokens.
  - Kanıt: settings.rs:186 KEYRING_SERVICE = "dev.clauderust.desktop"; mcp_oauth.rs:85 SERVICE = "dev.clauderust.desktop". Neither depends on the tauri identifier

- **-1** Secrets on Linux: the API key falls back to plain text in the SQLite settings table, and MCP OAuth tokens are kept only in memory, so a remote-connector sign-in is lost at every quit.
  - Kanıt: settings.rs:300-330 (keyring only under cfg(macos|windows), else db.setting_get(KEY_API)); mcp_oauth.rs:127-140 'shortcut: Linux has no secret-store ... a sign-in lasts until the app quits'

- **-0.5** Each release start re-registers clauderust:// for its own exe. Running the isolated eval copy silently took the handler from the owner's copy and left it pointing at a binary that was later deleted. I restored it afterwards.
  - Kanıt: lib.rs:284-287 register_all() on every non-debug start. HKCU\Software\Classes\clauderust\shell\open\command read "D:\ClaudeRust\target-eval\release\clauderust.exe" "%1" after the eval run

- **-0.5** Tauri's globals are injected into the sandboxed artifact iframe. IPC is not reachable there (verified), but the protection rests only on the CSP and the origin check.
  - Kanıt: CDP eval inside the http://artifact.localhost/<token> frame gave typeof __TAURI_INTERNALS__ === 'object' and typeof __TAURI__ === 'object'. invoke('get_settings') and invoke('set_zoom',{zoom:1.3}) timed out and the main window zoom stayed 1, so IPC is blocked

- **-0.5** The served-artifact map is cleared wholesale once it holds more than 64 documents. Any open artifact frame that reloads (or prints, or navigates) then gets a 404.
  - Kanıt: commands.rs:1101-1104 if served.len() > 64 { served.clear(); }; lib.rs:198-202 returns 404 'artifact not found'

Kontrol edilemeyenler: No fuzzing. A dropped network connection mid-stream on the CLI engine was not reproduced live; for the API engine it is covered by tests/turn_loop.rs, which passed. MCP crash/restart was verified live: the fake server was killed and auto-restarted with 2 tools.

### Native performance and resource use (15.5/20)

- **-2.5** No virtualization or pagination for long conversations. The whole branch is rendered in Rust and every turn goes into the DOM. A 600-message conversation means about 98k DOM nodes, roughly 270 MB of renderer memory and a main-thread freeze of about half a second each time it opens.
  - Kanıt: Measured: first open settled in 1541 ms, reopen 550 ms, 98,441 nodes, longest tasks 477/493 ms, renderer private 270.6 MB, whole tree 510.7 MB private vs 200 MB idle. Code: view.rs:103-151 (all turns), chat.js renders every turn

- **-1.5** Chats on the account start a new Claude Code CLI process for every turn, so each reply pays the CLI start-up before the first token.
  - Kanıt: Measured on Haiku 5.5 low effort: first text 2298 ms (500-word answer) and 2088 ms (artifact). claudecode.rs:477-498 stops the CLI at the end of each turn; claudecode.rs:236 spawns it per turn

- **-0.5** While a reply streams, the whole reply JSON is rewritten to SQLite from inside the stream callback on an async-runtime worker, behind the single connection mutex. A slow disk stalls that worker and every DB reader waits.
  - Kanıt: engine.rs:1090-1096 (st2.db.msg_update inside on_event); db.rs:22 conn: Mutex<Connection>

Kontrol edilemeyenler: Claude Desktop's startup time was not measured because I could not restart the owner's running Desktop. Its RAM was sampled in its current, possibly busy state. API-engine streaming was not measured (no API key allowed).

### UI/UX polish and interaction quality (13/15)

- **-1** Narrow-window layout problems. Table cells break words in the middle ('Typica' / 'l use'). At 700 px the Code workspace panel lies over the transcript and top bar with no scrim, cutting the conversation text off.
  - Kanıt: Screenshots rel_narrow_dark.png (480x800, table) and rel_code_narrow.png (700x800, panel over chat) in scratchpad/eval2

- **-0.5** The chosen model is not kept for new chats. After picking Haiku 5.5 and chatting, Ctrl+N opened a new chat on Opus 5.5; Desktop starts new chats on the last-used model.
  - Kanıt: CDP: after Ctrl+N the model button read 'Opus 5.5', 0 turns

- **-0.5** Raw transport errors reach the user in Settings.
  - Kanıt: rel_settings_dev.png shows 'network error: error sending request for url (http://127.0.0.1:9/mcp)' for a remote MCP server

Kontrol edilemeyenler: The dev copy (a) was not started: all UI driving used the isolated release copy, which serves the same ui/. Quick Entry was not shown. macOS and Linux were not seen.

### Code quality, tests and packaging (7/10)

- **-1** Auto-update cannot deliver: the published latest release (v0.1.2) has no updater manifest, so every background check fails.
  - Kanıt: curl -L https://github.com/Cafunw/ClaudeRust/releases/latest/download/latest.json -> 404 at .../download/v0.1.2/latest.json. The eval copy's log has 'ERROR tauri_plugin_updater::updater: update endpoint did not respond with a successful status code' at each start

- **-1** The DOM-heavy critical UI flows have no tests: send, edit, retry, branch switch, permission card, settings, sidebar. There is no end-to-end UI test. Command-level edit/regenerate/switch_branch have no tests either.
  - Kanıt: ui/test imports only pure helpers (util, slash, keymap, turnstream, diffstat, artifact_storage, ask_question, deeplink, store, composer.thinkingSupported). Untested: chat.js (919 lines), parts.js (476), settings.js (595), sidebar.js (276), panes/*. commands.rs has 7 tests in 3291 lines, none for send_message edit, regenerate or switch_branch

- **-0.5** The installers are not Authenticode-signed (only the updater payload is signed), so users get SmartScreen warnings. Desktop's binary is signed.
  - Kanıt: Get-AuthenticodeSignature ClaudeRust_0.1.2_x64-setup.exe -> NotSigned; Claude Desktop claude.exe -> Valid; release.yml only uses TAURI_SIGNING_PRIVATE_KEY

- **-0.5** CI never runs the integration tests (stream drop, stop, crash recovery, retry, overload fallback) or the UI unit tests.
  - Kanıt: .github/workflows/ci.yml runs only 'cargo test -p clauderust --lib' and eslint/node --check. It does not run tests/turn_loop.rs or 'npm run test:ui'

Kontrol edilemeyenler: Only Windows x64 was built: no macOS or Linux bundles. The CI matrix itself was not run.

## Backlog maddeleri

### EV2-01 — Chats and projects don't sync with the claude.ai account

- Önem: **high**, efor: **L**, puan: **-3**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/claudecode.rs`, `src-tauri/src/db.rs`, `src-tauri/src/import.rs`
- Kanıt: History is stored only in a local SQLite file (db.rs:130-187). Account chats are one-off CLI turns (claudecode.rs:236, 263-267). Web/mobile history, projects and memory are absent.
- Düzeltme ipucu: Say clearly in onboarding that history stays local. Offer a recurring import (or a read-only view) of claude.ai conversations and projects through the export or the account's conversation listing, so switching users keep their history.

### EV2-02 — No Cowork, computer use, Claude in Chrome, SSH or cloud sessions

- Önem: **high**, efor: **L**, puan: **-3**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/claudecode.rs`, `ui/js/code.js`
- Kanıt: The Desktop package ships cowork-svc.exe, smol-bin.x64.vhdx, chrome-native-host.exe, claude-ssh-broker.exe and computerUseTeach.js. ClaudeRust grep finds 0 hits for cowork and computer.use.
- Düzeltme ipucu: Start with what the CLI already offers: pass --chrome and --cloud/--remote through the Code tab, and add SSH remote environments (run the CLI over ssh). Leave a Cowork-like tab for later.

### EV2-03 — Long conversations render every turn: about 98k DOM nodes and a half-second freeze on open

- Önem: **medium**, efor: **M**, puan: **-2.5**, kategori: Native performance and resource use
- Dosyalar: `src-tauri/src/view.rs`, `ui/js/chat.js`
- Kanıt: 600 messages: 98,441 nodes, first open 1541 ms, reopen 550 ms, longest tasks 477/493 ms, renderer private 270.6 MB (tree 510.7 MB vs 200 MB idle).
- Düzeltme ipucu: Return only the last N turns from conversation_view, load older ones on scroll-up, and put content-visibility:auto on .turn. Alternatively render turns in idle-time chunks so no task exceeds 50 ms.

### EV2-04 — No Agent Skills, file creation or code execution in chat

- Önem: **medium**, efor: **L**, puan: **-2**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/models.rs`, `src-tauri/src/engine.rs`
- Kanıt: Desktop bundles docx/pptx/xlsx/pdf skills (resources/bundled-skills). models.rs:317-323 has only the web_search and web_fetch server tools.
- Düzeltme ipucu: Enable the code_execution server tool and the Skills API (or load SKILL.md folders) for API chats. Let account chats run the CLI with its skills, and add a download card for the files it creates.

### EV2-05 — Auto mode auto-runs model-written scripts (permission bypass)

- Önem: **high**, efor: **M**, puan: **-2**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/codetools.rs`, `src-tauri/src/claudecode.rs`
- Kanıt: codetools.rs:765-767 auto-allows Write; :378-394 auto-allows `python|node|bash x` for project files; :355-376 auto-allows make/cargo/npm run. claudecode.rs:898-901 and 1394-1398 replace the CLI's auto mode with this allowlist.
- Düzeltme ipucu: In Auto mode, ask before running interpreters, build tools or package scripts on files written during the session (or that changed since the last approval). Better, pass --permission-mode auto to the CLI and keep its own safety check.

### EV2-06 — No voice dictation, incognito chats, Research mode or chat memory

- Önem: **medium**, efor: **L**, puan: **-1.5**, kategori: Claude Desktop feature parity
- Dosyalar: `ui/js/composer.js`, `src-tauri/src/memory.rs`
- Kanıt: grep in ui/js and src-tauri/src finds 0 hits for dictat, microphone and incognito. memory.rs covers CLAUDE.md for code sessions only.
- Düzeltme ipucu: Add an incognito flag that skips the DB write and title generation, and a mic button that uses the WebView's speech API or Whisper. Add an opt-in memory file that is injected into the chat system prompt.

### EV2-07 — Only 2 UI languages; tray and native dialogs hard-coded in English

- Önem: **medium**, efor: **M**, puan: **-1.5**, kategori: Claude Desktop feature parity
- Dosyalar: `ui/js/i18n.js`, `src-tauri/src/lib.rs`
- Kanıt: i18n.js:1267 has dicts {en, tr}; Desktop has 42 locale files. lib.rs:148-152 tray labels and lib.rs:217-223 DB dialog are in English.
- Düzeltme ipucu: Move the strings to per-locale JSON, add the main Desktop locales, and pass translated tray and dialog labels from Rust using the settings language.

### EV2-08 — MCP: no legacy SSE transport, elicitation, sampling or MCP Apps

- Önem: **medium**, efor: **M**, puan: **-1.5**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/mcp.rs`
- Kanıt: mcp.rs:3 supports streamable HTTP only; mcp.rs:1230 answers roots/list with []. No elicitation or sampling handlers. Desktop ships mcpAppView.js.
- Düzeltme ipucu: Fall back to GET + 'endpoint' event SSE when a POST gets 4xx/405. Answer elicitation/create with a form card, and render ui:// resources in the sandboxed artifact frame.

### EV2-09 — Account chats start a new CLI process every turn (about 2.1-2.3 s to first text)

- Önem: **medium**, efor: **M**, puan: **-1.5**, kategori: Native performance and resource use
- Dosyalar: `src-tauri/src/claudecode.rs`
- Kanıt: Measured on Haiku 5.5 low effort: first text 2298 ms and 2088 ms. claudecode.rs:477-498 ends the CLI after each turn.
- Düzeltme ipucu: Keep the stream-json CLI process alive per conversation between turns (stdin stays open) and kill it after idling. Pre-spawn one when the composer gets focus.

### EV2-10 — Hard-coded keyring service shares secrets between app copies

- Önem: **medium**, efor: **S**, puan: **-1**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/settings.rs`, `src-tauri/src/mcp_oauth.rs`
- Kanıt: settings.rs:186 and mcp_oauth.rs:85 both use "dev.clauderust.desktop" whatever the bundle identifier.
- Düzeltme ipucu: Build the service name from app.config().identifier, migrating the existing entry once for the production identifier.

### EV2-11 — Linux: API key stored in plain text; MCP OAuth sign-in lost on every quit

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/settings.rs`, `src-tauri/src/mcp_oauth.rs`
- Kanıt: settings.rs:300-330 falls back to db.setting_get(KEY_API). mcp_oauth.rs:127-140 keeps tokens in memory only.
- Düzeltme ipucu: Enable keyring's linux-native/secret-service feature, keep the DB fallback only when no secret service exists, and warn the user when it is used.

### EV2-12 — Published v0.1.2 release has no updater manifest, so auto-update fails

- Önem: **high**, efor: **S**, puan: **-1**, kategori: Code quality, tests and packaging
- Dosyalar: `.github/workflows/release.yml`, `src-tauri/tauri.conf.json`
- Kanıt: releases/latest/download/latest.json redirects to v0.1.2/latest.json, which is 404. The app log shows 'update endpoint did not respond with a successful status code' at every start.
- Düzeltme ipucu: Upload latest.json for v0.1.2. Add a release-job step that fails when latest.json and the .sig files are not attached before the draft is published.

### EV2-13 — No tests for the DOM-heavy critical UI flows or the command-level branch/edit/regenerate paths

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: Code quality, tests and packaging
- Dosyalar: `ui/test`, `ui/js/chat.js`, `ui/js/parts.js`, `src-tauri/src/commands.rs`
- Kanıt: ui/test covers pure helpers only. chat.js (919 lines), parts.js, settings.js, sidebar.js and panes/* are untested. commands.rs has 7 tests, none for send_message edit, regenerate or switch_branch.
- Düzeltme ipucu: Add a CDP-driven smoke test against `tauri dev` with dev/mock_api.py (send, stop, edit, retry, branch nav, permission card). Add command tests for edit, regenerate and switch_branch using MockRuntime.

### EV2-14 — Narrow-window layout: mid-word table breaks; Code panel covers the transcript

- Önem: **low**, efor: **S**, puan: **-1**, kategori: UI/UX polish and interaction quality
- Dosyalar: `ui/css/app.css`, `ui/js/workspace.js`
- Kanıt: rel_narrow_dark.png ('Typica' / 'l use'); rel_code_narrow.png (panel over chat and top bar at 700 px).
- Düzeltme ipucu: Use overflow-wrap:normal plus horizontal scroll in .table-wrap. Below about 900 px make the workspace a full-width sheet with a scrim, or push the chat aside instead of overlaying it.

### EV2-15 — Artifacts load their libraries from public CDNs (break offline); no window.claude.complete

- Önem: **low**, efor: **M**, puan: **-1**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/artifacts.rs`
- Kanıt: artifacts.rs:239 CSP script-src lists jsdelivr, unpkg and esm.sh; artifacts.rs:241-245. Desktop bundles mermaid, chart.js and gridjs.
- Düzeltme ipucu: Vendor react, react-dom, mermaid and recharts into ui/vendor and serve them from the artifact protocol. Optionally offer claude.complete behind a per-artifact consent prompt.

### EV2-16 — Every release start takes over the clauderust:// handler

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/lib.rs`
- Kanıt: lib.rs:284-287. After the eval copy ran, HKCU\Software\Classes\clauderust\shell\open\command pointed at target-eval\release\clauderust.exe.
- Düzeltme ipucu: Register only when no handler exists or the existing one points at a missing file, or only from the installer. Skip registration when the identifier is not the production one.

### EV2-17 — Tauri globals injected into sandboxed artifact iframes

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/lib.rs`, `src-tauri/src/artifacts.rs`
- Kanıt: In the artifact.localhost frame, __TAURI_INTERNALS__ and __TAURI__ are objects. invoke timed out, so IPC is blocked.
- Düzeltme ipucu: Set withGlobalTauri false and import the API in ui/js. Keep the IPC origin check, and add a test that the artifact frame cannot invoke.

### EV2-18 — Served artifacts map cleared wholesale after 64 entries

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/commands.rs`
- Kanıt: commands.rs:1101-1104 served.clear() means open frames get a 404 on reload.
- Düzeltme ipucu: Use an LRU that evicts the oldest token, or key the map by artifact id+version so it is overwritten instead of growing.

### EV2-19 — Streaming saves run on async-runtime workers behind one DB mutex

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: Native performance and resource use
- Dosyalar: `src-tauri/src/engine.rs`, `src-tauri/src/db.rs`
- Kanıt: engine.rs:1090-1096 msg_update inside on_event; db.rs:22 Mutex<Connection>.
- Düzeltme ipucu: Send partial saves to a dedicated writer thread or spawn_blocking with coalescing.

### EV2-20 — Model choice not kept for new chats

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: UI/UX polish and interaction quality
- Dosyalar: `ui/js/composer.js`, `ui/js/store.js`
- Kanıt: After picking Haiku 5.5, Ctrl+N showed 'Opus 5.5'.
- Düzeltme ipucu: Store the last picked chat model in settings (as Desktop does) and use it for new chats.

### EV2-21 — Raw transport errors shown in Settings

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: UI/UX polish and interaction quality
- Dosyalar: `src-tauri/src/mcp.rs`, `ui/js/settings.js`
- Kanıt: rel_settings_dev.png shows 'network error: error sending request for url (http://127.0.0.1:9/mcp)'.
- Düzeltme ipucu: Map connect/timeout/DNS/TLS errors to plain, translated messages and put the raw text behind a details toggle.

### EV2-22 — Quick Entry is text-only

- Önem: **low**, efor: **M**, puan: **-0.5**, kategori: Claude Desktop feature parity
- Dosyalar: `ui/js/quick.js`, `src-tauri/src/commands.rs`
- Kanıt: quick.js is 43 lines: a textarea that submits {text} only (commands.rs:1325).
- Düzeltme ipucu: Add screenshot/window capture and paste or drop of files, staged with stage_bytes and passed along with the text.

### EV2-23 — Installers are not Authenticode-signed

- Önem: **low**, efor: **M**, puan: **-0.5**, kategori: Code quality, tests and packaging
- Dosyalar: `.github/workflows/release.yml`
- Kanıt: Get-AuthenticodeSignature on the NSIS setup -> NotSigned; Desktop's claude.exe -> Valid.
- Düzeltme ipucu: Sign through Azure Trusted Signing or a code-signing certificate in the release job (bundle.windows.signCommand), and notarize the macOS build.

### EV2-24 — CI skips the integration and UI tests

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: Code quality, tests and packaging
- Dosyalar: `.github/workflows/ci.yml`
- Kanıt: ci.yml runs `cargo test -p clauderust --lib` only. tests/turn_loop.rs and `npm run test:ui` never run in CI.
- Düzeltme ipucu: Drop --lib, or add `cargo test --test turn_loop`, and add an `npm run test:ui` step.

## Güçlü yanlar (bozulmamalı)

- Very light compared with Desktop. Release exe is 25.6 MB, NSIS installer 7.94 MiB, MSI 10.67 MiB. Idle tree (7 processes) is 200 MB private with about 0.07% total CPU; first contentful paint is 549-665 ms warm and 774 ms cold. Claude Desktop's 14 processes held 2,119 MB private at the same time.
- Streaming IPC is well designed. turnstream.rs sends a keyframe then string-append deltas: 109 events and 83.5 KB for a 3.4 KB answer, largest event 9.4 KB, no long tasks, frame p95 6 ms while streaming.
- The Code tab looks and works like Desktop's. Live run: permission card with inline diff, approve once, file edited, Changes pane with commit/PR, a working PowerShell PTY terminal, worktree option, plan/tasks/preview/map/files panes.
- Artifacts are sandboxed correctly. The iframe has an opaque origin (no allow-same-origin) and a strict CSP. Invoking app commands from the artifact frame was verified to be blocked: invoke timed out and set_zoom had no effect.
- MCP is robust. Clear failed/connected states, per-tool Ask/Allow/Block, OAuth with RFC 9728/8707 discovery and PKCE, and the stdio supervisor restarted a killed server live.
- Markdown rendering escapes raw HTML and only allows http/https/mailto links (markdown.rs:28-43, 273-275). open_external, open_path and reveal_path are guarded by scheme and pathguard checks.
- No panics on user or network data: non-test unwraps are only constant regexes or invariants, and string slicing respects char boundaries. Partial replies are saved during streaming and marked on crash or stop (turn_loop tests plus a live Stop test).
- The Bash safety parser is careful about PowerShell syntax, UNC/NTLM paths, .git/.claude short names and streams, with many tests.
- Gates are clean. clippy --all-targets -D warnings passes, all 250 Rust tests pass (241 unit + 9 integration, 16 ignored benches), all 56 UI tests pass, eslint is clean, and the tauri build has 0 warnings.
- The UI is polished in both themes. Clean empty states (projects, search), a clear MCP error panel, a shortcut sheet and command palette; the sidebar collapses to a rail and there is no horizontal scroll at 480 px.

## Yöntem

Read-only on D:\ClaudeRust at v0.1.2 (f71361e); the working tree differs only in PROJECT_PROGRESS.md, which I did not read, nor docs/audit-1.md, docs/eval-*.md or AUDIT_AND_ROADMAP.md.

Reference: the Claude Desktop 6.18355 package (app.asar listing parsed with my own reader, resources, Electron 45.3.3), Claude Code CLI 2.1.326 --help, and known Desktop features.

Code review: lib.rs, the codetools permission engine, the claudecode CLI bridge, the engine turn loop, anthropic SSE, MCP and OAuth, db, view, markdown, artifacts, turnstream and UI modules. I grepped for unwraps, slicing, locks and block_on, and for missing features.

Build and gates: tauri build --config {identifier dev.clauderust.evalcopy, no updater artifacts} into D:\ClaudeRust\target-eval, then clippy -D warnings, cargo test (lib and integration), eslint and node UI tests.

Driving the app: I launched the release copy with WebView2 CDP on port 9334 and drove it with my own helpers in scratchpad\eval2. That covered onboarding (continue with the existing CLI login, no OAuth), a synthetic 600-message conversation plus 200 short ones inserted with Python sqlite3, timing and memory sampling (Win32_Process tree sums of WS and private bytes, CPU deltas), and 4 Haiku 5.5 turns: a chat, an artifact, a stop test and a Code-tab edit with permission. Also the Code panes and terminal, MCP config with fake, broken and unreachable servers, a live MCP kill, artifact-iframe IPC probes, and screenshots in dark and light themes, at 1240, 700 and 480 px, plus empty and error states. Claude Desktop was sampled the same way by executable path.

Each category started at its maximum, and only deductions backed by a file:line, a command output or a screenshot were taken. Each flaw was counted once.

Cleanup: I stopped only the eval process trees and deleted %APPDATA% and %LOCALAPPDATA%\dev.clauderust.evalcopy and D:\ClaudeRust\target-eval. The eval copy had re-pointed HKCU clauderust:// to its own exe; I set that back to D:\ClaudeRust\target\release\clauderust.exe, the value the owner's copy writes when it starts. The owner's process (PID 13716) and target\release were untouched.

Not done: the dev copy (a) was not started; UI driving used the isolated release copy, which serves the same ui/. Also not done: Quick Entry (the shortcut is held by the owner's copy), API-key engine paths and OAuth sign-in (forbidden), macOS/Linux builds, and Desktop startup timing. The Code-tab test turn may have left a Claude Code session transcript for the scratch project under ~/.claude/projects; I left it there.
