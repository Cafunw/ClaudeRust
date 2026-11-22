# Puanlayıcı 3: 82.5/100 (v0.1.3)

Bağımsız, salt okunur puanlayıcının tam raporu. Her kesinti kanıtıyla birlikte; PROJECT_PROGRESS.md backlog'u bu raporun EV-xx maddelerini izler.

| Kategori | Puan |
|---|---|
| Claude Desktop feature parity (streaming, MCP, artifacts, history/branching, attachments, system prompt/project context, model/API config) | 21.5/30 |
| Stability and crash resistance | 22.25/25 |
| Native performance and resource use | 18.5/20 |
| UI/UX polish and interaction quality | 13/15 |
| Code quality, tests and packaging | 7.25/10 |

## Kesintiler

### Claude Desktop feature parity (streaming, MCP, artifacts, history/branching, attachments, system prompt/project context, model/API config) (21.5/30)

- **-1.25** MCP Apps are not supported. Desktop renders a server's interactive ui:// resources (text/html;profile=mcp-app) inline in the chat; ClaudeRust shows only the text of tool results.
  - Kanıt: There are no matches for mcp-app, ui:// or outputTemplate in src-tauri/src or ui/js. mcp.rs:1203-1243 (convert_result) keeps only text, image and resource text. mcp.rs:1279-1289 answers only ping, roots/list and elicitation/create. Desktop resources/ion-dist.asar contains 'text/html;profile=mcp-app' 9 times, 'ui/initialize' 8 times and 'ui/notifications' 28 times.

- **-0.75** MCP management has no connector or extension directory and no 'Add custom connector' form (name, URL, OAuth client). Remote servers can be added only by editing JSON by hand.
  - Kanıt: settings.js:311-314: the Developer page offers only Edit config, Open file, Reload and Install extension (from a file). Screenshot r05_mcp_servers.png. Desktop ion-dist.asar contains 'Add custom connector' 11 times, 'Browse connectors' 3 times and 'Browse extensions' 2 times.

- **-1** When the OS is in dark mode, the body text of an HTML artifact that is a fragment (no <html> or doctype) is white on a white frame, so it is invisible.
  - Kanıt: artifacts.rs:235: BASE_STYLE sets color-scheme:light dark without a background or text color. artifacts.js:161 adds 'dark-ok' only for non-HTML types. app.css:619 gives the frame background #fff. Measured in the artifact frame over CDP: computed color of <p id=js> rgb(255,255,255), body background transparent, iframe background rgb(255,255,255). Screenshot r11_html_first.png shows only the colored h1; the 'js ran' paragraph is in the DOM but invisible.

- **-1** AI-powered artifacts do not work and there is no 'Try fixing' when an artifact throws, unlike Desktop.
  - Kanıt: artifacts.rs:276: window.claude.complete always rejects. ERROR_HOOK (artifacts.rs:246-253) only shows a red overlay. ui/js has no fix or 'try fix' flow (grep found nothing). Desktop ion-dist.asar contains 'Try fixing'.

- **-1** Artifact edits are always full rewrites (no update or diff command), which makes small edits to large artifacts slow and costly. The artifact prompt also tells the model that only plain npm imports exist and that storage is unavailable, so the shadcn @/components/ui shim and window.storage the app provides are never used.
  - Kanıt: prompts.rs:27: 'To change an existing artifact, output it again in full'. prompts.rs:25: 'You may import only: react, lucide-react, …' and 'Browser storage … is not available'. The app nevertheless maps 16 shadcn modules (artifacts.rs:333-336) and window.storage (artifacts.rs:270-275).

- **-1.5** Attachment types and limits fall short of Desktop. .xlsx and .pptx are refused. PDFs are sent raw with no page-count check, so PDFs over the API's 100-page limit fail only after sending. The 'Add screenshot' attach item is permanently disabled, while Desktop has 'Take a screenshot'.
  - Kanıt: stage_paths results: sheet.xlsx and slides.pptx returned "isn't a supported file type"; png, pdf, csv, rtf and html were accepted. attachments.rs:204-218 passes any %PDF file under 32 MB with no page check (grep for '100 pages' found nothing). composer.js:384 has { label: t('add_screenshot'), icon: 'image', disabled: true }. Desktop asar contains 'xlsx' 165/84 times, 'pptx' 129/117 times and 'Take a screenshot' 10/13 times.

- **-1.5** Project knowledge PDFs and images are silently dropped on the Claude-account (CLI) chat engine, which is the default whenever there is no API key. Only the text-file part of the system prompt reaches the model.
  - Kanıt: engine.rs:191-203 puts non-text project files into rc.prefix_blocks, which only build_messages (engine.rs:489-493) uses on the API path. claudecode.rs:924-937 writes only rc.system to system-<id>.txt and never sends prefix_blocks (grep: prefix_blocks is not referenced in claudecode.rs).

- **-0.5** There is no project knowledge search (RAG). All project text, up to the 2M-character 'capacity', is inlined into every request. Desktop switches to project_knowledge_search for large projects.
  - Kanıt: projects.js:11: CAPACITY_CHARS = 2_000_000. prompts.rs:102-113 inlines every text file in <project_knowledge>. Desktop ion-dist.asar contains 'project_knowledge_search' 13 times.

Kontrol edilemeyenler: Desktop's window cannot be screenshotted, so Desktop behaviour was taken from asar string tables and known features. Whether Desktop answers MCP sampling requests could not be confirmed (the SDK strings are present either way). MCP OAuth was not run against a real server (forbidden). Web-search citations and thinking display were not exercised with a real model; only 2 live Haiku turns were run, on the account path.

### Stability and crash resistance (22.25/25)

- **-1.5** Deleting a chat that ran on the Claude account does not delete its data. The full CLI transcript and the per-conversation system-prompt file (with project knowledge and profile preferences) stay on disk indefinitely. This is a data-retention and privacy defect.
  - Kanıt: claudecode.rs:263-266 and 925 write <data>/chat-cli/system-<conv_id>.txt, which is never removed (grep: no remove_file in claudecode.rs). delete_conversation (commands.rs:719-742) does not touch the CLI session. ~/.claude/projects/C--Users-xldyt-AppData-Roaming-dev-clauderust-designdev-chat-cli/ still held 9 transcripts from earlier runs whose app data had already been deleted, plus the one my live turn created.

- **-0.75** Stalled streams are not detected. A connection that dies silently (sleep/resume, network switch) keeps the reply spinning for up to 10 minutes. There is no idle or ping watchdog.
  - Kanıt: anthropic.rs:72: .read_timeout(Duration::from_secs(600)) is the only stream timeout. My mock 'hang' stream (message_start, then silence) stayed 'responding' until I pressed Stop.

- **-0.25** A Tauri plugin init script is injected into every sandboxed artifact frame and throws on each load. It cannot reach the IPC (no invoke key, CSP, opaque origin), but app scripts should not run in untrusted frames.
  - Kanıt: The artifact iframe console on every load showed: "TypeError: Cannot read properties of undefined (reading 'plugins') at <anonymous>:5:50" (fr.mjs, both dev and release copies).

- **-0.25** Text sniffing accepts NUL-heavy binary files as text, so binary data can be sent to the model as a text block.
  - Kanıt: stage_paths on photo.heic (an ftyp header plus 100 NUL bytes) returned kind 'text' with mime image/heic. attachments.rs:226-236 accepts anything that util::decode_text reads as UTF-8.

Kontrol edilemeyenler: There was no fuzzing of the SSE or MCP parsers beyond the existing hostile-input unit tests. OAuth token refresh against a real server is forbidden. The Code-mode permission classifier (codetools.rs) was read in part (split_for_shell, simple_is_safe, git_auto_ok) and not exhaustively.

### Native performance and resource use (18.5/20)

- **-1** Turns loaded by scrolling up are never unloaded. A long chat scrolled to the top keeps every turn in the DOM, memory grows by about 250 MB, and each later streamed reply costs about twice the CPU.
  - Kanıt: In the release build, a synthetic 600-exchange chat opened with 40 turns and about 2.5k nodes. Scrolling to the top loaded 1,200 turns and about 72k nodes, and private bytes went from 257 MB to 507 MB. A long streamed reply then cost 2.5 CPU-s over 5.6 s (45% of a core) with 257 ms of style recalculation, against 1.66 CPU-s over 6.4 s (26%) in a small chat.

- **-0.5** The account engine starts a new Claude Code CLI process for every turn and re-reads the session with --resume, so it carries per-turn spawn latency that Desktop's chat does not have.
  - Kanıt: claudecode.rs:892-1019 spawns the CLI with --resume <sid> on each turn. A live Haiku 5.5 low-effort reply of 3 words took 2,425 ms to first text in the UI, against a reported model duration of 1.9 s. The same UI against the local mock took 102 ms to first text.

Kontrol edilemeyenler: Desktop's startup time could not be measured: the owner's Desktop instance is running and hosts this session, so it cannot be restarted. Desktop memory was sampled while busy with many Code sessions (14 processes), so it is not an idle-to-idle comparison. Per-thread tokio blocking was not profiled; it was judged from the code instead (151 async commands, with heavy work in spawn_blocking).

### UI/UX polish and interaction quality (13/15)

- **-0.5** Automatic retries are silent. On a retryable error the user sees only the spinning sparkle for many seconds (up to minutes with retry-after, capped at 60 s per wait) with no 'retrying' status.
  - Kanıt: Mock HTTP 500: 12.5 s of a blank turn, then the error (d07_retrying.png, first text at 12,520 ms). anthropic.rs:159-170 retries up to 4 times with only log::warn. ui/js has no 'retry' or 'attempt' status (grep found nothing).

- **-0.5** MCP prompt arguments are collected with native window.prompt dialogs, one per argument, instead of an in-app form.
  - Kanıt: composer.js:404-411: window.prompt(`${a.name}${a.description ? ' — ' + a.description : ''}`). A CDP test confirmed a native prompt dialog opens in the webview.

- **-0.5** The sidebar collapses by itself when an artifact opens (window under 1500 px) or the window is narrowed, and is not restored when the panel closes or the window widens again.
  - Kanıt: artifacts.js:47 emits 'collapse-sidebar' on open; close() (artifacts.js:62-68) does not restore it. In d06_err500.png the new chat after the artifact panel closed still had a collapsed sidebar, and r02_light.png still showed it collapsed after the window was widened from 460 px.

- **-0.25** Raw transport errors are shown to the user.
  - Kanıt: A mid-stream socket drop showed "Ağ hatası: network error: error decoding response body" (d05_drop.png). The partial text and the Retry button themselves are good.

- **-0.25** The app does not honour reduced-motion preferences.
  - Kanıt: app.css has 0 matches for prefers-reduced-motion.

Kontrol edilemeyenler: The first-run onboarding was not seen, because the test key env var skipped it. macOS and Linux renderings were not checked. Screen-reader behaviour was not tested beyond counting aria attributes (22 aria-label uses, role=dialog on modals).

### Code quality, tests and packaging (7.25/10)

- **-1** The end-to-end UI suite is not in CI and runs only by hand against a running dev copy. Its Code-session test skips itself whenever ~/.claude/settings.json has hooks, as happened here.
  - Kanıt: .github/workflows/ci.yml runs only cargo test, clippy, npm run lint and test:ui. npm run test:e2e: 9 passed, 1 skipped ('a Code session asks before it writes a file').

- **-1** Several critical flows have no tests. Nothing tests the artifact preview pipeline (serve_artifact → artifact:// → iframe), so the dark-mode invisible text went unnoticed. Nothing checks that project files reach the account engine, so the PDF drop went unnoticed. Attachment coverage is thin (4 tests, with no oversize-PDF or office-format behaviour). chat.js (1,012 lines), settings.js (600) and projects.js have no unit tests.
  - Kanıt: Test count per module: artifacts.rs 6 (document strings only), prompts.rs 1, attachments.rs 4. ui/test has no chat, settings, projects or artifacts-panel tests.

- **-0.5** Windows installers are not Authenticode-signed; only the updater manifest is signed. Users get SmartScreen 'unknown publisher' warnings, while Desktop is signed.
  - Kanıt: release.yml:5-49 and 85-86 handle only TAURI_SIGNING_PRIVATE_KEY (updater). There is no certificate or signtool step.

- **-0.25** Several god-modules are hard to review.
  - Kanıt: wc -l: commands.rs 3,352 lines (151 commands), codetools.rs 2,617, engine.rs 2,567, claudecode.rs 2,232.

Kontrol edilemeyenler: macOS and Linux builds and bundles were not run locally (CI covers tests on all three OSes, not bundling).

## Backlog maddeleri

### EV3-01 — Project PDFs and images are dropped on the Claude-account (CLI) chat engine

- Önem: **high**, efor: **M**, puan: **-1.5**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/claudecode.rs`, `src-tauri/src/engine.rs`
- Kanıt: engine.rs:191-203 puts non-text project files in rc.prefix_blocks, which only build_messages (engine.rs:489) sends. claudecode.rs:924-937 passes only rc.system (via --system-prompt-file), and claudecode.rs never references prefix_blocks. Account mode is the default without an API key (claudecode.rs:280-285).
- Düzeltme ipucu: On the first turn of a CLI chat (and on with_history replays), prepend rc.prefix_blocks to the first stream-json user message's content, as build_messages does. Add a test that a project PDF reaches the CLI's first line.

### EV3-02 — Deleted account chats leave transcripts and system-prompt files on disk

- Önem: **high**, efor: **S**, puan: **-1.5**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/commands.rs`, `src-tauri/src/claudecode.rs`
- Kanıt: delete_conversation (commands.rs:719-742) does not delete <data>/chat-cli/system-<id>.txt (written at claudecode.rs:925, never removed) or ~/.claude/projects/<chat-cli>/<cc_session>.jsonl. 9 earlier transcripts were still present after their app data had been deleted.
- Düzeltme ipucu: Before conv_delete, collect every cc_session from the conversation's messages and remove the matching .jsonl files under the chat-cli project folder. Delete system-<id>.txt on delete, or remove it after each turn.

### EV3-03 — Attachments: xlsx/pptx refused, PDFs over 100 pages fail at the API, 'Add screenshot' always disabled

- Önem: **medium**, efor: **M**, puan: **-1.5**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/attachments.rs`, `ui/js/composer.js`
- Kanıt: stage_paths: sheet.xlsx and slides.pptx returned "isn't a supported file type". attachments.rs:204-218 has no page-count check. composer.js:384 has disabled: true on add_screenshot.
- Düzeltme ipucu: Extract xlsx (sheets to CSV) and pptx (slide text) like docx_text does. Count /Type /Page and fall back to extracted text above 100 pages. Implement a screen capture (tauri-plugin-screenshots or the OS snipping tool) or remove the dead menu item.

### EV3-04 — MCP Apps (interactive ui:// resources) not supported

- Önem: **medium**, efor: **L**, puan: **-1.25**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/mcp.rs`, `src-tauri/src/render.rs`, `ui/js/parts.js`
- Kanıt: There are no mcp-app or ui:// references in the repo. convert_result (mcp.rs:1203) flattens results to text and images. Desktop ion-dist.asar has text/html;profile=mcp-app and ui/initialize.
- Düzeltme ipucu: When a tool's _meta.ui.resourceUri is set, read the resource and render it in a sandboxed iframe (reuse the artifact:// protocol and CSP). Bridge ui/* postMessage calls (tools/call, ui/notifications) through a whitelist.

### EV3-05 — HTML artifact fragments render white-on-white when the OS is in dark mode

- Önem: **high**, efor: **S**, puan: **-1**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/artifacts.rs`, `ui/js/artifacts.js`, `ui/css/app.css`
- Kanıt: artifacts.rs:235 BASE_STYLE sets color-scheme:light dark. The frame background is #fff (app.css:619), and 'dark-ok' is not applied to text/html (artifacts.js:161). Measured text color rgb(255,255,255) on a white frame; r11_html_first.png.
- Düzeltme ipucu: For text/html, either drop 'dark' from color-scheme (use :root{color-scheme:light}) or set an explicit body background and color through theme_attr(dark) and add dark-ok, as for SVG and Mermaid. Add a test on build_document output.

### EV3-06 — No AI-powered artifacts (window.claude.complete) and no 'Try fixing' on artifact errors

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/artifacts.rs`, `ui/js/artifacts.js`
- Kanıt: artifacts.rs:276: window.claude.complete always rejects. The error overlay (artifacts.rs:246-253) offers no action. Desktop has 'Try fixing'.
- Düzeltme ipucu: Post the error to the parent over postMessage and show a 'Try fixing' button that sends the error and artifact id as a user message. Offer window.claude.complete behind a per-artifact confirmation with a token cap.

### EV3-07 — Artifact edits are full rewrites; the prompt hides the shadcn shim and window.storage

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/prompts.rs`, `src-tauri/src/render.rs`
- Kanıt: prompts.rs:27 says 'output it again in full'. prompts.rs:25 limits imports and says storage is unavailable, while artifacts.rs:333-336 and 270-275 provide @/components/ui/* and window.storage.
- Düzeltme ipucu: Add an update form (<antArtifact identifier=… command="update" old_str/new_str>) applied by ArtifactTracker to the latest version. Mention @/components/ui/* and window.storage in ARTIFACTS_INFO.

### EV3-08 — Turns loaded by scrolling up are never unloaded (memory and streaming cost grow)

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: Native performance and resource use
- Dosyalar: `ui/js/turnwindow.js`, `ui/js/chat.js`
- Kanıt: 600-exchange chat: after scrolling to the top, 1,200 turns and about 72k nodes were in the DOM, private bytes went from 257 to 507 MB, and a streamed reply then cost 45% of a core and 257 ms of style recalculation, against 26% in a small chat.
- Düzeltme ipucu: Unload pages far from the viewport (keep height placeholders, or use content-visibility:auto with contain-intrinsic-size per turn), and re-window to the last page when a new turn starts.

### EV3-09 — e2e UI suite not in CI; Code-session e2e skipped when hooks exist

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: Code quality, tests and packaging
- Dosyalar: `.github/workflows/ci.yml`, `ui/e2e/smoke.test.mjs`
- Kanıt: ci.yml runs no test:e2e. A local run gave 9 passed and 1 skipped (hooks in ~/.claude/settings.json).
- Düzeltme ipucu: Add a windows-latest job that starts `tauri dev --config dev/e2e.conf.json` with a temporary HOME/USERPROFILE (so no user hooks) and the CDP port, then runs npm run test:e2e.

### EV3-10 — Critical flows without tests: artifact preview pipeline, account-engine project context, attachment types

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: Code quality, tests and packaging
- Dosyalar: `src-tauri/src/artifacts.rs`, `src-tauri/src/claudecode.rs`, `src-tauri/src/attachments.rs`, `ui/js/chat.js`
- Kanıt: artifacts.rs has 6 tests (string checks only), prompts.rs 1, attachments.rs 4. There are no UI tests for chat.js, settings.js or projects.js. EV3-01 and EV3-05 went unnoticed.
- Düzeltme ipucu: Add e2e steps that open an HTML artifact in dark mode and assert the text is readable, and that start an account chat in a project with a PDF and assert the CLI's first line carries a document block (with a fake CLI). Add attachment tests for xlsx/pptx and oversize PDFs.

### EV3-11 — No connector/extension directory and no 'Add custom connector' form

- Önem: **medium**, efor: **M**, puan: **-0.75**, kategori: Claude Desktop feature parity
- Dosyalar: `ui/js/settings.js`, `src-tauri/src/mcp.rs`
- Kanıt: settings.js:311-314 offers only Edit config, Open file, Reload and Install extension from a file. Desktop has 'Add custom connector', 'Browse connectors' and 'Browse extensions'.
- Düzeltme ipucu: Add a form (name, URL, optional OAuth client id/secret) that writes the mcpServers entry and starts OAuth. Optionally list a curated directory of .mcpb extensions.

### EV3-12 — Stalled streams wait up to 600 s; no idle/ping watchdog

- Önem: **medium**, efor: **S**, puan: **-0.75**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/anthropic.rs`
- Kanıt: anthropic.rs:72 read_timeout(600 s). A mock stream that went silent after message_start kept spinning until Stop.
- Düzeltme ipucu: Track the time since the last SSE event (the API sends ping events). After 60-90 s of silence, abort with a retryable 'connection stalled' error that keeps the partial reply.

### EV3-13 — No project knowledge search; the whole knowledge base is inlined every request

- Önem: **low**, efor: **L**, puan: **-0.5**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/prompts.rs`, `ui/js/projects.js`
- Kanıt: prompts.rs:102-113 inlines every text file. projects.js:11 sets capacity at 2,000,000 chars. Desktop has project_knowledge_search.
- Düzeltme ipucu: Above a token threshold, index project files (SQLite FTS5 already exists) and expose a project_knowledge_search tool instead of inlining.

### EV3-14 — The account engine spawns a CLI process and resumes the transcript on every turn

- Önem: **low**, efor: **L**, puan: **-0.5**, kategori: Native performance and resource use
- Dosyalar: `src-tauri/src/claudecode.rs`
- Kanıt: claudecode.rs:892-1019 spawns per turn with --resume. A live 3-word Haiku reply took 2.4 s to first text, against a reported duration of 1.9 s.
- Düzeltme ipucu: Keep one stream-json CLI process alive per open chat (input-format stream-json supports multiple user messages) and end it after an idle period.

### EV3-15 — Silent automatic retries with no visible status

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: UI/UX polish and interaction quality
- Dosyalar: `src-tauri/src/anthropic.rs`, `ui/js/chat.js`
- Kanıt: HTTP 500: 12.5 s of a blank spinner (d07_retrying.png). anthropic.rs:165 only logs.
- Düzeltme ipucu: Emit a 'retrying' event (attempt n of 4, wait in seconds, reason) and show it under the streaming turn with a Stop link.

### EV3-16 — MCP prompt arguments use native window.prompt dialogs

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: UI/UX polish and interaction quality
- Dosyalar: `ui/js/composer.js`
- Kanıt: composer.js:404-411.
- Düzeltme ipucu: Build one modal form from p.arguments (name, description, required) using the existing modal() and field helpers.

### EV3-17 — Sidebar auto-collapses on artifact open or narrow window and is never restored

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: UI/UX polish and interaction quality
- Dosyalar: `ui/js/artifacts.js`, `ui/js/sidebar.js`
- Kanıt: artifacts.js:47 collapses it; close() at artifacts.js:62-68 does not restore it. Screenshots d06_err500.png and r02_light.png.
- Düzeltme ipucu: Remember that the collapse was automatic and re-expand on panel close or when the width grows back, unless the user toggled it meanwhile.

### EV3-18 — Windows installers not Authenticode-signed

- Önem: **low**, efor: **M**, puan: **-0.5**, kategori: Code quality, tests and packaging
- Dosyalar: `.github/workflows/release.yml`, `src-tauri/tauri.conf.json`
- Kanıt: release.yml signs only the updater manifest (TAURI_SIGNING_PRIVATE_KEY). There is no certificateThumbprint or signCommand.
- Düzeltme ipucu: Add bundle.windows.signCommand (Azure Trusted Signing or a code-signing certificate) in the release job.

### EV3-19 — A Tauri plugin init script runs in sandboxed artifact frames and throws on each load

- Önem: **low**, efor: **S**, puan: **-0.25**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/lib.rs`
- Kanıt: The artifact frame console showed "TypeError: Cannot read properties of undefined (reading 'plugins')" on every load.
- Düzeltme ipucu: Find the plugin that registers a non-main-frame init script (window-state, deep-link or dialog) and limit it to the main frame, or guard it upstream.

### EV3-20 — NUL-heavy binary files are accepted as text attachments

- Önem: **low**, efor: **S**, puan: **-0.25**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/attachments.rs`, `src-tauri/src/util.rs`
- Kanıt: photo.heic (ftyp header plus NUL bytes) was staged as kind 'text' with mime image/heic.
- Düzeltme ipucu: Refuse non-text extensions whose first 8 KB contain NUL bytes or control characters above a small ratio, as git's binary heuristic does.

### EV3-21 — Raw transport errors shown to the user

- Önem: **low**, efor: **S**, puan: **-0.25**, kategori: UI/UX polish and interaction quality
- Dosyalar: `ui/js/neterr.js`, `src-tauri/src/error.rs`
- Kanıt: "network error: error decoding response body" after a dropped stream (d05_drop.png).
- Düzeltme ipucu: Map reqwest body and decode errors to 'The connection was interrupted; the partial answer was kept', with the details collapsed.

### EV3-22 — No prefers-reduced-motion support

- Önem: **low**, efor: **S**, puan: **-0.25**, kategori: UI/UX polish and interaction quality
- Dosyalar: `ui/css/app.css`
- Kanıt: 0 matches for prefers-reduced-motion in app.css.
- Düzeltme ipucu: Add @media (prefers-reduced-motion: reduce) that disables the sparkle spin, panel slide and other transitions.

### EV3-23 — Oversized modules (commands.rs 3,352 lines, 151 commands)

- Önem: **low**, efor: **L**, puan: **-0.25**, kategori: Code quality, tests and packaging
- Dosyalar: `src-tauri/src/commands.rs`, `src-tauri/src/codetools.rs`, `src-tauri/src/engine.rs`, `src-tauri/src/claudecode.rs`
- Kanıt: wc -l: commands.rs 3,352, codetools.rs 2,617, engine.rs 2,567, claudecode.rs 2,232.
- Düzeltme ipucu: Split commands.rs by domain (chat, projects, git, terminal, settings) into a commands/ module.

## Güçlü yanlar (bozulmamalı)

- Crash safety is real. I killed the release process 3.5 s into a streamed reply; on the next start the 7,086 characters already received were still there, marked 'stopped' (PartialSaves every 2 s plus mark_interrupted). No WebView2 orphans were left.
- Stream failures are handled well. A mid-stream socket drop keeps the partial text and offers Retry. Stop on a stalled stream ends it at once. The SSE accumulator rejects hostile block indexes and shapes without panicking, and there are no unwrap/expect calls on network data.
- Performance is low and steady: warm first paint about 450 ms after process start (cold 610 ms), idle CPU 0.08 s per 30 s, about 215 MB private for 7 processes, a 27 MB exe, an 8.0 MiB NSIS and 10.75 MiB MSI installer, and about 5 minutes for a clean release build with 0 warnings.
- Streaming is efficient. The turnstream keyframe/delta protocol sent 120 events (312 KB) for a 19.5 KB rich reply; the Rust side used 0.16 CPU-s across the 5.6 s stream; a 600-exchange chat opens in 54 ms through turn windowing.
- MCP is solid: stdio, streamable HTTP and legacy SSE transports, OAuth, a supervisor that restarts crashed servers with backoff, readable failure messages (r05_mcp_servers.png), per-tool Ask/Allow/Block policies, per-chat server toggles, prompts, resources, elicitation and .mcpb extension install.
- Security posture is strong. The main window CSP has script-src 'self' only; raw HTML is escaped in the Rust markdown renderer; artifact frames run without allow-same-origin under their own default-src 'none' CSP; deep links are whitelisted; open_external accepts only http, https and mailto; open_path and reveal_path go through pathguard.
- Branching and history work like Desktop: edit and Retry create versions with arrows, and FTS5 search answers in about 4 ms with highlighted snippets. Both themes are clean and Desktop-like (r02_light.png, d02_short.png), and keyboard shortcuts are comprehensive and rebindable (r07_shortcuts.png).
- Code health is high: clippy -D warnings is clean, 265 lib + 6 branching + 10 turn-loop Rust tests and 69 UI tests pass, and the existing e2e harness passed 9 of 10 against a dev copy.

## Yöntem

I read the code first, without opening PROJECT_PROGRESS.md, AUDIT_AND_ROADMAP.md, docs/audit-1.md, docs/eval-*.md or the builders' scratchpad notes. I focused on the engine, anthropic, artifacts, attachments, MCP, claudecode, db, deeplink, commands, settings, prompts and the main UI modules.
I ran clippy, cargo test, eslint, the UI tests and the project's own e2e suite against an isolated dev copy (identifier dev.clauderust.designdev, CDP port 9333).
To drive streaming without spending money, I wrote my own Messages-API mock in the scratchpad (scenarios: long, artifacts, drop, hang, err500) and pointed the dev copy and the release copy at it, using the repo's documented test key in the CLAUDERUST_API_KEY env var. No real key was entered and nothing was written to the keyring. I also ran 2 live Haiku 5.5 low-effort turns through the Claude-account (CLI) path.
I built an unsigned release copy (identifier dev.clauderust.evalcopy, target-eval) and launched it with CDP on port 9334. On it I measured first paint, working set, private bytes and CPU of the process tree, and IPC volume, by counting 'turn' events in the page. I tested a synthetic 600-exchange conversation inserted with SQLite, killed the process mid-stream to check data loss, took screenshots of both themes and a narrow window, and checked MCP settings with working, broken and unreachable servers, keyboard shortcuts, search, and the artifact panel, inspecting the artifact iframe over CDP.
I compared against Claude Desktop 6.18355 through its process memory and CPU and grep -a of app.asar and ion-dist.asar for feature strings (MCP Apps, custom connectors, Try fixing, xlsx/pptx, screenshots, project_knowledge_search).
Cleanup: I stopped only my own dev and eval process trees and the mock, and deleted %APPDATA% and %LOCALAPPDATA% \dev.clauderust.designdev and \dev.clauderust.evalcopy as well as D:\ClaudeRust\target-eval. The owner's target\release process (pid 13716) was never touched, and the repository is unchanged except for the owner's pre-existing PROJECT_PROGRESS.md modification. My working files are in C:\Users\xldyt\AppData\Local\Temp\claude\b19ddb4b09d05f83\747baec6-ad7e-4440-baa9-57ceec9c11a3\scratchpad\eval2\ev3.

## Puansız notlar (kapsam dışı Claude Desktop özellikleri)

- There is no account or cloud sync. Chats live only in local SQLite and are not shared with claude.ai or mobile.
- Cowork, computer use, Claude in Chrome (Desktop ships chrome-native-host.exe) and remote, SSH or cloud sessions (Desktop ships claude-ssh-broker, -proxy and -askpass) are absent.
- Skills and file creation (the code-execution sandbox that produces xlsx, pptx and docx) are absent, as is Research mode.
- Voice mode and dictation, incognito chats, and Desktop's memory feature are absent. ClaudeRust has its own past-chat search tool, on the API engine only.
- There are no first-party connectors (Gmail, Drive, Calendar and others) and no artifact publishing or sharing.
- The UI has 2 languages (en, tr) against Desktop's larger set.
- Quick Entry, tray, clauderust:// deep links and the Tauri auto-updater do exist, so they are not gaps.
- Housekeeping: my 2 live Haiku turns (about $0.0006 in total) left one CLI transcript, 4214fb1f-….jsonl, in ~/.claude/projects/C--Users-xldyt-AppData-Roaming-dev-clauderust-designdev-chat-cli/, a folder of the owner's that already held 9 files from earlier runs. I did not delete anything there.

## Ölçümler

Release build (commit v0.1.3, CARGO_TARGET_DIR=target-eval, unsigned): 4m56s, 0 warnings. clauderust.exe 27,060,224 B; MSI 10.75 MiB; NSIS 8.00 MiB. Desktop 6.18355: claude.exe 250 MB, app.asar 100 MB, ion-dist.asar 221 MB.
Startup, process start to first-contentful-paint: cold 610 ms; warm 451, 433, 466 and 527 ms (the last with 1,200 extra messages in the database).
Idle, after about 50 s: 7 processes, working set 393-402 MB, private 215 MB, CPU 0.08 s over 30 s.
Claude Desktop (busy, hosting this orchestration): 14 processes, working set 2,337 MB, private 2,199 MB, CPU 1.17 s over 30 s.
Streaming a rich reply from the local mock (about 3,600 words, 30 tables, 30 code blocks):
- 5.6 s; 120 'turn' events; 312,002 B of IPC.
- All processes: 1.66 CPU-s over 6.36 s. The Rust process alone: 0.16 CPU-s.
- Page task time 359 ms; private memory from 215 to 289 MB.
600-exchange synthetic chat:
- Opens in 54 ms with 40 turns drawn (2,531 nodes).
- Scrolling to the top took 4.6 s and ended with 1,200 turns, about 72k nodes, a 13 MB JS heap, and private memory from 257 to 507 MB.
- A long streamed reply inside it: first text at 282 ms, 2.5 CPU-s over 5.6 s, style recalculation 257 ms.
FTS search: 4 ms.
Account (CLI) engine, live Haiku 5.5 at low effort: first text at 2,425 ms for a 3-word reply (reported 1.9 s), $0.00031 per turn, no leftover CLI processes.
Mock HTTP 500: retried silently for 12.5 s, then the error and Retry button were shown.
Tests and lint:
- cargo clippy --all-targets -D warnings: clean.
- cargo test: 265 passed and 18 ignored (benchmarks and real-CLI tests), plus 6 branching and 10 turn-loop tests.
- npm run lint: clean. npm run test:ui: 69/69.
- npm run test:e2e: 9 passed, 1 skipped.
Attachments (stage_paths):
- Accepted: png (big PNG downscaled to JPEG), pdf, csv, rtf, html.
- Refused: xlsx, pptx, random binary.
- A NUL-filled .heic was accepted as text.
