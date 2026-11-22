# Puanlayıcı 5: 90.75/100 (v0.1.6)

Bağımsız, salt okunur puanlayıcının tam raporu. Her kesinti kanıtıyla birlikte; PROJECT_PROGRESS.md backlog'u bu raporun EV-xx maddelerini izler.

| Kategori | Puan |
|---|---|
| Claude Desktop feature parity | 27/30 |
| Stability and crash resistance | 23.5/25 |
| Native performance and resource use | 18/20 |
| UI/UX polish and interaction quality | 13.25/15 |
| Code quality, tests and packaging | 9/10 |

## Kesintiler

### Claude Desktop feature parity (27/30)

- **-1** MCP: Node-based desktop extensions (.mcpb/.dxt) only start when the user has their own `node` on PATH. Claude Desktop ships a built-in Node.js runtime for MCP servers, so users without Node cannot run most extensions in ClaudeRust.
  - Kanıt: src-tauri/src/extensions.rs:369-370 (`"node" => {"command": "node"...}`, `"python" => "python3"`). Desktop app.asar strings: "Using built-in Node.js for MCP server: ${e}", "The built-in Node.js runtime cannot start ... as a Node.js subprocess".

- **-1** MCP: there is no Desktop Extensions directory to browse and install from in one click. ClaudeRust installs only from a local file and lists 48 hand-curated remote connectors.
  - Kanıt: Settings → Geliştirici offers only: Özel bağlayıcı ekle | Bağlayıcılara göz at | Yapılandırmayı düzenle | Dosyayı aç | Sunucuları yeniden başlat | Eklenti yükle… (screenshot r_dev.png). src-tauri/src/connector_directory.json holds remote servers only. Desktop app.asar has 16 hits for `ExtensionDirectory` plus `isDesktopExtensionDirectoryEnabled`.

- **-1** Attachments and project knowledge cannot come from GitHub or Google Drive, which Claude Desktop offers in the composer and in project knowledge.
  - Kanıt: The project 'İçerik ekle' menu shows only 'Dosya yükle | Metin içeriği ekle' (live CDP read). The composer + menu has upload, screenshot, and MCP prompts/resources (ui/js/composer.js:486-498, r_attach.png). Grep finds no Drive or GitHub source code in ui/js or src-tauri/src.

- **-0.5** System prompt / styles: a custom style is only a name plus a hand-written prompt. Claude Desktop can generate a style from uploaded writing samples.
  - Kanıt: ui/js/settings.js:175-201 (custom_styles entries are {id, name, prompt}). The composer tools menu (r_tools.png) lists Normal/Kısa/Açıklayıcı/Resmi and 'Stil oluştur ve düzenle'.

- **--0.5** Offset inside the MCP area: a remote connector can be added with a static bearer token, which is stored in the OS credential store. Desktop's custom-connector dialog offers only URL plus OAuth client settings.
  - Kanıt: The add-connector dialog (r_addconn.png) has Ad / URL / 'Bearer belirteci (isteğe bağlı)' and says the token is kept in the system credential store, not in the config file.

Kontrol edilemeyenler: Claude Desktop's window cannot be screenshotted, so its chat-side features (claude.ai web UI) were judged from app.asar strings and known product behaviour, not observed. GitHub/Drive and style-from-samples in Desktop come from that knowledge. Strengths offset gaps only inside their own area, and areas without gaps got nothing: model/API config (API key, base URL, OpenAI-compatible endpoints, effort, fast mode, overload fallback), branching/history (fork, outline, rewind, sections, FTS search) and stream recovery (Continue after a cut-off).

### Stability and crash resistance (23.5/25)

- **-1** Secrets handling is inconsistent. The OpenAI-compatible endpoint key (for example an OpenRouter key) is stored in plain text in the SQLite settings row and sent unmasked to the webview on every bootstrap/get_settings. The Anthropic key, by contrast, lives in Windows Credential Manager and reaches the UI only masked.
  - Kanıt: src-tauri/src/settings.rs:110 (`pub oai_api_key: String`); settings.rs:260-262 (`save` = `db.setting_set(KEY_SETTINGS, serde_json::to_string(self))`); src-tauri/src/commands.rs:38-44 (settings_json serializes every field and masks only `api_key`). Live: `invoke('get_settings')` returned `oai_api_key` as a string field.

- **-0.5** Desktop extensions (.mcpb/.dxt run arbitrary local code) are installed with no signature or publisher verification. Claude Desktop checks extension signatures and can require them.
  - Kanıt: `grep -i 'signature|signed|verify' src-tauri/src/extensions.rs` finds nothing. Desktop app.asar has `isDesktopExtensionSignatureRequired`.

Kontrol edilemeyenler: Linux and macOS builds were not run, and there was no power-loss or kill -9 test mid-stream. A hard crash can lose at most ~2 s of streamed text (engine.rs:31 SAVE_EVERY); I judged that acceptable and did not deduct. The MCP stdio/HTTP protocol was not fuzzed beyond the crashing and missing servers used live. Verified without finding a defect: clippy await_holding_lock is clean; unwraps outside tests are on static regexes or invariants; an artifact iframe has `__TAURI_INTERNALS__` but its invoke never completes and has no side effect (the save_settings nickname stayed unchanged), and fetch to ipc.localhost is blocked by the artifact CSP; the postMessage bridge checks e.source; markdown images become links and raw HTML is escaped; open_path/reveal_path go through pathguard; deep links only prefill; the CLI child exits after each turn; deleting a chat removes its CLI transcript; a mid-stream socket cut keeps the partial reply with Retry/Continue; 529 retries visibly 2/4 and then succeeds; an unreachable API ends in a clear error; non-ASCII paths work.

### Native performance and resource use (18/20)

- **-1.5** Chats on a Claude account (the default for subscription users) spawn a fresh Claude Code CLI process every turn. That adds about a second to first token and a ~435 MB private-memory process per active chat turn, multiplied by parallel chats. Desktop's chat calls the API from its already running renderer.
  - Kanıt: src-tauri/src/claudecode.rs:1027 run_once and :1203 cmd.spawn() per turn (with --resume). Live Haiku 5.5 turns, low effort: first text 1877 ms and 1807 ms after send. During the turn the tree peaked at 725 MB private, of which claude.exe was 435 MB private / 222 MB WS. It fell back to 250 MB after the CLI exited.

- **-0.5** Streaming IPC still carries several times the reply's size, because the backend re-sends rendered and highlighted HTML per changed block instead of only the new text. No jank resulted, but it is avoidable work.
  - Kanıt: Measured with a listener on the 'turn' event: a 25 KB reply took 175 events / 157 KB (max 1.3 KB); a 100 KB reply streamed in 18 s took 211 events / 431 KB (max 2.5 KB), 0 long tasks, ~34% of one core, mostly WebView GPU/renderer, with clauderust.exe at 0.38 s CPU (src-tauri/src/turnstream.rs).

Kontrol edilemeyenler: Claude Desktop was busy during measurement (it hosts this session), so its numbers are not idle. Its startup time and time-to-first-token could not be measured (a second instance cannot be started and its window cannot be captured). WebView2 numbers come from an off-screen window.

### UI/UX polish and interaction quality (13.25/15)

- **-0.5** A stale, wrong note in Settings tells Claude-account users that past-chat search is unavailable, but the account engine does provide it, so users are misled into leaving it off or into thinking they need an API key.
  - Kanıt: ui/js/settings.js:293-295: the comment 'past-chat search runs only in the API engine (claudecode.rs leaves it off)' and chat_search_cc_note shown when onAccount; i18n.js:598 'Not available yet in chats that run on your Claude account'. Versus src-tauri/src/claudecode.rs:1111, which adds past_chats::mcp_tools() to account chats when chat_search is on, and :828-829, which auto-allows them.

- **-0.5** Backend errors show up in English inside the Turkish UI (mixed-language error states).
  - Kanıt: MCP card in Turkish UI: 'Could not start `C:/nope/notthere.exe`: Sistem belirtilen yolu bulamıyor.' (r_mcp2.png). Account chat with signed-out CLI: 'Not logged in · Please run /login … You're not signed in to Claude. Open Settings → Account…'. commands.rs:1354 'Only web links can be opened.'

- **-0.5** Small layout clipping and narrow-width behaviour: the effort segment label wraps, placeholder and card text are cut, and at ≤760 px an open artifact covers the whole conversation with only Close to get back.
  - Kanıt: r_model.png: 'Çok yüksek' wraps to two lines in the effort control. r_ws.png: the code composer placeholder is cut mid-sentence. r_code.png: 'Depoda başka oturum çalışır…' truncated. r_mid.png and r_narrow.png (760 and 420 px): the artifact panel fills the viewport and the chat is hidden.

- **-0.25** An artifact runtime error is shown twice: the app's banner above the frame and the in-frame red bar both carry the same message.
  - Kanıt: r_perm.png: banner 'Bu artifact bir hatayla karşılaştı … (connect-src)' at the top of the panel, plus the bottom bar 'Error: The artifact sandbox blocked http://ipc.localhost/…' inside the frame.

Kontrol edilemeyenler: No screen-reader pass, no real mouse or touchpad, no high-DPI scaling test: screenshots came from CDP on an off-screen window. Seen working: both themes, 420/760/1240 px widths, Ctrl+K search, Ctrl+, settings, Ctrl+` workspace, Escape closes menus and returns focus, inline edit with a 2/2 branch navigator, MCP Ask/Allow/Block prompt, retry indicator, error cards with Retry/Continue, empty states for projects and MCP.

### Code quality, tests and packaging (9/10)

- **-0.5** Critical startup, exit and security glue has no tests, and some data-loss or security modules have very few.
  - Kanıt: No #[test] in src-tauri/src/lib.rs (659 lines: artifact_protocol 201-220, offer_fresh_db DB-recovery rename 226-258, close/tray 397-416, exit cleanup 629-656) or display.rs (245). pathguard.rs has 1 test, checkpoints.rs (rewind) 2, compact.rs 1 (counts from the cargo test log).

- **-0.25** Several very large modules make the code hard to maintain.
  - Kanıt: commands.rs 3453 lines with 155 #[tauri::command]s (149 async, 6 sync); engine.rs 2923; claudecode.rs 2635; codetools.rs 2618; mcp.rs 2510.

- **-0.25** Duplicate dependency versions are compiled into the release binary.
  - Kanıt: The release build log compiled both reqwest v0.12.28 and v0.13.5. `cargo tree -d` (windows target) shows base64 x2, getrandom x3, hashbrown x3, html5ever x2, markup5ever x2, phf x2, indexmap x2, digest x2.

Kontrol edilemeyenler: macOS and Linux bundles were not built. The 2 real-CLI tests are #[ignore]d, but 11 fake-CLI turn tests cover that path. Every gate I ran was green: clippy --all-targets -D warnings (exit 0), ESLint (0 problems), cargo test 410 passed / 0 failed / 19 ignored, npm run test:ui 164/164, npm run e2e 15/15 (46 s), and a clean `tauri build` with no warnings.

## Backlog maddeleri

### EV5-01 — Account-engine chats spawn the Claude Code CLI on every turn

- Önem: **high**, efor: **L**, puan: **-1.5**, kategori: Native performance and resource use
- Dosyalar: `src-tauri/src/claudecode.rs`
- Kanıt: claudecode.rs:1027 run_once / :1203 cmd.spawn() per turn. Live Haiku 5.5 low-effort turns: first text at 1877 ms and 1807 ms; claude.exe 435 MB private / 222 MB WS while the turn ran (tree peak 725 MB private), 250 MB after it exited.
- Düzeltme ipucu: Keep one CLI process per open chat alive in stream-json input mode, the way the Agent SDK does: send each new user message on its stdin, reap it after an idle timeout, and fall back to --resume only when it has died. Optionally pre-warm one process when the composer gets focus.

### EV5-02 — No built-in Node.js runtime for Node-based desktop extensions

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: Claude Desktop feature parity
- Dosyalar: `src-tauri/src/extensions.rs`
- Kanıt: extensions.rs:369-370 starts `node` (and `python3`) from PATH. Desktop app.asar: 'Using built-in Node.js for MCP server', 'The built-in Node.js runtime cannot start …'.
- Düzeltme ipucu: Offer to download and verify a pinned Node LTS into the app data dir on first install of a node-type extension, and use it when `node` is not on PATH. On Windows, resolve `python`/`py` rather than `python3`. Tell the user clearly which runtime is missing.

### EV5-03 — No Desktop Extensions directory (browse and one-click install)

- Önem: **medium**, efor: **M**, puan: **-1**, kategori: Claude Desktop feature parity
- Dosyalar: `ui/js/extensions_page.js`, `ui/js/settings.js`, `src-tauri/src/connector_directory.json`
- Kanıt: Settings → Geliştirici has only file install ('Eklenti yükle…') and 'Bağlayıcılara göz at' (48 remote servers). Desktop app.asar has ExtensionDirectory and isDesktopExtensionDirectoryEnabled.
- Düzeltme ipucu: Add a curated, signed index of public .mcpb releases (for example from vendors' GitHub releases) with download, signature/hash check, and the existing extension_install preview flow.

### EV5-04 — No GitHub or Google Drive sources for attachments and project knowledge

- Önem: **medium**, efor: **L**, puan: **-1**, kategori: Claude Desktop feature parity
- Dosyalar: `ui/js/projects.js`, `ui/js/composer.js`, `src-tauri/src/attachments.rs`
- Kanıt: Project 'İçerik ekle' menu = 'Dosya yükle | Metin içeriği ekle'; composer + menu = upload / screenshot / MCP prompts and resources (composer.js:486-498).
- Düzeltme ipucu: At minimum, add 'Add from GitHub': a repo URL plus branch, cloned or archived through `gh`/git or the public API, with a file picker, stored as project files. Drive can go through an MCP connector shortcut.

### EV5-05 — OpenAI-compatible API key stored in plain text and sent unmasked to the webview

- Önem: **medium**, efor: **S**, puan: **-1**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/settings.rs`, `src-tauri/src/commands.rs`
- Kanıt: settings.rs:110 oai_api_key is a Settings field saved as JSON in SQLite (settings.rs:260-262). commands.rs:38-44 returns it to the UI, masking only the Anthropic key. Live get_settings returned oai_api_key as a string.
- Düzeltme ipucu: Keep it in the keyring next to the Anthropic key (secrets.rs), add `oai_has_key`/`oai_key_masked` to settings_json, and migrate existing values out of the settings row.

### EV5-06 — Desktop extensions installed without signature or publisher verification

- Önem: **medium**, efor: **M**, puan: **-0.5**, kategori: Stability and crash resistance
- Dosyalar: `src-tauri/src/extensions.rs`, `ui/js/extensions_page.js`
- Kanıt: No signature/verify code in extensions.rs. Desktop app.asar has isDesktopExtensionSignatureRequired.
- Düzeltme ipucu: Verify the .mcpb signature block (mcpb sign format) and show 'Signed by …' or 'Unsigned' in the install preview, with an extra confirmation for unsigned bundles.

### EV5-07 — Streaming IPC sends 4-6x the reply size as rendered HTML

- Önem: **low**, efor: **M**, puan: **-0.5**, kategori: Native performance and resource use
- Dosyalar: `src-tauri/src/turnstream.rs`, `src-tauri/src/engine.rs`
- Kanıt: 100 KB reply → 211 'turn' events, 431 KB. 25 KB reply → 175 events, 157 KB (no long tasks).
- Düzeltme ipucu: While a text block is open, send raw text appends and render or highlight only the closed blocks (or render on the page), and send the HTML of a block once it closes.

### EV5-08 — Custom styles cannot be generated from writing samples

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: Claude Desktop feature parity
- Dosyalar: `ui/js/settings.js`
- Kanıt: settings.js:175-201: a custom style = {name, prompt} typed by hand.
- Düzeltme ipucu: Add 'Create from sample': upload or paste text, ask a small model to write the style prompt, then let the user edit it.

### EV5-09 — Settings wrongly says past-chat search is unavailable on the Claude account

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: UI/UX polish and interaction quality
- Dosyalar: `ui/js/settings.js`, `ui/js/i18n.js`, `src-tauri/src/claudecode.rs`
- Kanıt: settings.js:293-295 and i18n.js:598/1353 show chat_search_cc_note on the account engine, but claudecode.rs:1111 and :828-829 provide past_chats tools in account chats.
- Düzeltme ipucu: Remove the note and the stale comment, or gate it on an actual capability flag returned by the backend.

### EV5-10 — Backend error messages appear in English inside the Turkish UI

- Önem: **low**, efor: **M**, puan: **-0.5**, kategori: UI/UX polish and interaction quality
- Dosyalar: `src-tauri/src/mcp.rs`, `src-tauri/src/claudecode.rs`, `src-tauri/src/commands.rs`, `ui/js/i18n.js`
- Kanıt: r_mcp2.png 'Could not start `…`: Sistem belirtilen yolu bulamıyor'. CLI signed-out reply 'Not logged in · Please run /login … You're not signed in…'. commands.rs:1354 'Only web links can be opened.'
- Düzeltme ipucu: Return error kinds/codes from commands (as attachments.rs already does with `refuse(kind, …)`) and localize them in i18n.js, keeping the raw text under 'Ayrıntılar'.

### EV5-11 — Layout clipping and narrow-width artifact takeover

- Önem: **low**, efor: **S**, puan: **-0.5**, kategori: UI/UX polish and interaction quality
- Dosyalar: `ui/css/app.css`, `ui/js/artifacts.js`, `ui/js/code.js`
- Kanıt: r_model.png 'Çok yüksek' wraps in the effort control. r_ws.png code composer placeholder cut. r_code.png worktree card text truncated. r_mid.png/r_narrow.png: at 760/420 px the artifact hides the chat.
- Düzeltme ipucu: Let the segmented control shrink its font or use short labels; allow the composer placeholder to wrap or shorten it; below ~900 px show the artifact as a toggleable sheet with a 'Back to chat' control.

### EV5-12 — Startup, exit and security glue untested; thin tests on rewind and pathguard

- Önem: **low**, efor: **M**, puan: **-0.5**, kategori: Code quality, tests and packaging
- Dosyalar: `src-tauri/src/lib.rs`, `src-tauri/src/display.rs`, `src-tauri/src/pathguard.rs`, `src-tauri/src/checkpoints.rs`, `src-tauri/src/compact.rs`
- Kanıt: 0 tests in lib.rs (artifact_protocol, offer_fresh_db, close/exit cleanup) and display.rs. pathguard 1 test, checkpoints 2, compact 1.
- Düzeltme ipucu: Move artifact_protocol and DB-recovery rename into testable functions; add rewind round-trip tests (files plus conversation), compaction boundary tests, and pathguard cases (symlink, junction, 8.3 names, case).

### EV5-13 — Artifact error shown twice (app banner and in-frame bar)

- Önem: **low**, efor: **S**, puan: **-0.25**, kategori: UI/UX polish and interaction quality
- Dosyalar: `src-tauri/src/artifact_hook.js`, `ui/js/artifacts.js`
- Kanıt: r_perm.png: the same CSP error appears in the panel banner and in the frame's bottom bar.
- Düzeltme ipucu: When the host has acknowledged the error message, have artifact_hook.js hide its own overlay (or skip drawing it inside the app).

### EV5-14 — Very large modules

- Önem: **low**, efor: **L**, puan: **-0.25**, kategori: Code quality, tests and packaging
- Dosyalar: `src-tauri/src/commands.rs`, `src-tauri/src/engine.rs`, `src-tauri/src/claudecode.rs`, `src-tauri/src/codetools.rs`, `src-tauri/src/mcp.rs`
- Kanıt: commands.rs 3453 lines / 155 commands; engine.rs 2923; claudecode.rs 2635; codetools.rs 2618; mcp.rs 2510.
- Düzeltme ipucu: Split commands.rs by domain (git, terminal, projects, settings) and claudecode.rs into spawn/args, event loop, and SDK-MCP relay.

### EV5-15 — Duplicate dependency versions in the release build

- Önem: **low**, efor: **S**, puan: **-0.25**, kategori: Code quality, tests and packaging
- Dosyalar: `src-tauri/Cargo.toml`, `Cargo.lock`
- Kanıt: Release build compiled reqwest 0.12.28 and 0.13.5. cargo tree -d: base64 x2, getrandom x3, hashbrown x3, html5ever x2, phf x2.
- Düzeltme ipucu: Move to reqwest 0.13 (used by the updater plugin) and base64 0.22 to drop the second copies; recheck with `cargo tree -d`.

## Güçlü yanlar (bozulmamalı)

- Startup: 0.80 s from launch to first contentful paint on a cold profile and 0.50-0.58 s warm (3 runs), restoring a 400-message chat.
- Footprint: 7 processes, 412 MB WS / 212 MB private when idle, ~0% CPU idle (0.17 s CPU in 30 s). Claude Desktop meanwhile: 14 processes, 1.41 GB WS / 2.2 GB private (busy). Installers are 8.39 MiB (NSIS) and 11.29 MiB (MSI) against Desktop's 250 MB claude.exe plus 321 MB of asars.
- Long conversations: a 400-message chat imported in 90 ms and first drew in 27 ms; turns are windowed and parked (395/400 parked). Scrolling the whole chat had no frame over 50 ms (worst 32 ms) and no long tasks; JS heap stayed at 4-7 MB. 50 chat switches showed no leak (private memory returned to baseline plus the terminal).
- Streaming robustness: a socket cut mid-reply keeps the partial text with Retry and Continue, and Continue appends to the same message. A 529 retries visibly ('2/4') and then succeeds. An unreachable API ends in a clear localized error card. Streams of 25 KB, 100 KB and 210 KB ran without jank.
- Artifact sandbox verified live: the iframe has __TAURI_INTERNALS__, but invoke never completes and has no side effect, and fetch to ipc.localhost is blocked by the CSP. The postMessage bridge checks e.source, served documents are capped at 64, and CSP violations surface with a 'Düzeltmeyi dene' action.
- MCP management: stdio, streamable HTTP and SSE transports; OAuth; bearer tokens kept in the keychain; per-tool Ask/Allow/Block; live status with explained failures and logs; restart with backoff (unit-tested); a permission prompt offering once / this chat / always; MCP tools relayed into Claude-account chats.
- Branching and history: inline edit produces a 2/2 navigator, plus fork, rewind, outline, sections/pinned/archive, and Ctrl+K search with filters. Deleting a chat also removes its CLI transcript (verified on disk).
- Model and API configuration go beyond Desktop: API key or Claude account, base URL, OpenAI-compatible endpoints, effort levels, fast mode, overload fallback, and per-reply time and cost.
- Quality gates all green: clippy --all-targets -D warnings, ESLint, cargo test (410 passed), test:ui (164), e2e (15/15), and a warning-free release build.
- Non-ASCII and space-containing Windows paths (Turkish characters) work in the file, symbol and @-mention tools.

## Yöntem

I read the evaluator brief in full and did not read PROJECT_PROGRESS.md, AUDIT_AND_ROADMAP.md, docs/audit-1.md, docs/eval-*.md or docs/feature-audit.md. The README was used only to find features to verify.

Static review: lib.rs, the anthropic/engine/turnstream streaming paths, claudecode.rs (account engine), mcp.rs and extensions, artifacts, CSP and protocol, markdown sanitizing, db, settings and secrets, pathguard, deeplink, attachments, and the UI rendering and escaping (showMenu, artifacts bridge). I grepped every unwrap/expect/panic outside tests.

Commands run: clippy -D warnings, cargo test, npm run lint, npm run test:ui, and npm run e2e (the project's own launcher; it started and stopped its own dev.clauderust.e2e copy with a temporary CLAUDE_CONFIG_DIR).

I built the release copy into D:\ClaudeRust\target-eval with identifier dev.clauderust.evalcopy and no updater artifacts. I launched it with CLAUDERUST_OFFSCREEN=1 and the debugging port 9334; it was confirmed off-screen at (-11992,-11969). I drove it over CDP with my own helpers in scratchpad\eval5:
- mock Anthropic API with long, drop, overload, huge and marathon replies plus an MCP endpoint;
- the project's e2e mock for an artifact and an HTTP MCP tool;
- a crashing stdio MCP server and a missing one;
- a synthetic 400-message claude.ai export.

I measured the process tree (root plus descendants) for WS, private bytes and CPU, plus in-page performance via PerformanceObserver(longtask), rAF and a turn-event listener. I took screenshots in the dark and light themes, at 420, 760 and 1240 px, and of the settings, MCP, artifact, permission, error, project and code-workspace screens.

Account engine: I relaunched without the test key and with the owner's CLI login, and ran two Haiku 5.5 low-effort turns. The CLI transcript they created (session a2becf7b-d05b-4847-8224-5db4a4e19f89 in ~/.claude/projects/...-evalcopy-chat-cli) was removed by the app's own delete_conversation, and I verified it is gone. The two older transcripts in that folder predate my run and were left alone.

Reference: the Claude Desktop package Claude_6.18355.0.0 (process list and memory, app/resources listing, app.asar string greps for built-in Node, ExtensionDirectory and signature settings) and Claude Code CLI 2.1.326 --help. Desktop's window was not captured.

Cleanup:
- I stopped only my own processes by PID: the app trees 39540, 46756, 48500, 51768, 41460 and 22792, all with paths under target-eval, and the node mocks 40108, 32128 and 43388.
- I deleted %APPDATA%\dev.clauderust.evalcopy, %LOCALAPPDATA%\dev.clauderust.evalcopy (both created at 10:43 by my first launch) and D:\ClaudeRust\target-eval.
- I never started the designdev dev copy and never touched target\release. The repository is unchanged.

## Puansız notlar (kapsam dışı Claude Desktop özellikleri, durumlarıyla)

- Quick Entry: EQUAL, global-shortcut floating window (lib.rs:101-161); the eval copy logged the expected shortcut conflict with the owner's copy
- Tray: EQUAL, tray menu Open/New Chat/Quick Entry/Quit (lib.rs:163-199)
- Auto-update: EQUAL (code only), tauri-plugin-updater with a minisign pubkey in tauri.conf.json; not exercised
- Deep links: EQUAL, clauderust://chat|new|code|settings, which only open or prefill and never send (deeplink.rs:1-60)
- Usage limits: EQUAL, live on the account engine: weekly-limit banner and sidebar meter at 83% (r_cli.png)
- Incognito: EQUAL, incognito.rs with a sweep at start and exit; not exercised live
- Memory: EQUAL (local), memory page and memory_synth.rs; not exercised live
- Skills: EQUAL, Settings → Beceriler (skills.rs: list, enable, install, uninstall)
- Research: EQUAL (API engine), 'Araştırma' toggle in the tools menu (research.rs); not exercised
- Code tab: EQUAL, Claude Code and built-in engines, real PTY terminal pane (r_ws.png), worktrees, changes/history panes
- Sidebar organisation: BETTER, sections, pin, archive, unread, project and code grouping
- Languages: WORSE, 2 UI languages (ui/js/i18n.js:1537-1538 en/tr)
- Account sync / chat sharing: MISSING, local SQLite only; a claude.ai export can be imported (verified with a 400-message synthetic export)
- Voice / dictation: MISSING, read-aloud TTS only; no SpeechRecognition in ui/js
- Remote/SSH/cloud sessions: MISSING, no SSH session code; Desktop ships claude-ssh-broker.exe
- Cowork: MISSING, Desktop ships cowork-svc.exe
- Computer use: MISSING
- Claude in Chrome: MISSING, Desktop ships chrome-native-host.exe
- File creation (docx/xlsx/pptx): WORSE/partial, only through the API code-execution tool (code_exec.rs save_output_file); not available or verified on the account engine

## Ölçümler

Repo state: v0.1.6 = HEAD 59d249d; git status clean before and after.

Gates:
- cargo clippy --all-targets --locked -D warnings: exit 0 (19.9 s).
- npm run lint: 0 problems.
- cargo test --locked: 410 passed (364 lib + 46 integration), 0 failed, 19 ignored (benches and 2 real-CLI tests), 1m38s.
- npm run test:ui: 164/164.
- npm run e2e: 15/15 in 46 s.

Release build (CARGO_TARGET_DIR=target-eval, identifier dev.clauderust.evalcopy, no updater artifacts): compile 5m12s, total 328.6 s, no warnings. clauderust.exe 28,579,328 B; MSI 11.29 MiB; NSIS 8.39 MiB. Both reqwest 0.12.28 and 0.13.5 were compiled.

Startup (launch to first-contentful-paint, from process start time vs performance.timeOrigin + FCP): cold first profile 801 ms (navigation started at +481 ms, FCP 320 ms). Warm, restoring a 400-message chat: 583 / 500 / 516 ms. CDP endpoint up in 634 ms.

Idle, empty chat: 7 processes, WS 412 MB, private 212 MB, 0.00 s CPU over 20 s. After the instrumentation's rAF loop was stopped: 0.17 s CPU in 30 s.

Claude Desktop 6.18355 at the same time (busy, it hosts this session): 14 processes, WS 1408-1422 MB, private 2203-2214 MB.

Streaming (API engine against a local mock with test key sk-ant-test):
- 25 KB markdown reply in ~14 s: 175 turn events, 157 KB IPC, max event 1.3 KB, 0 long tasks, 1/9654 frames over 50 ms; tree CPU ~4.2 s (≈30% of one core); peak private 325 MB.
- 100 KB reply in 18 s: 211 events, 431 KB, max 2.5 KB, 0 long tasks; CPU 6.1 s (renderer +2.6 s, GPU +3.0 s, clauderust.exe +0.38 s); private 365 → 376 MB.
- 210 KB burst reply: 13 events, 720 KB, max 77 KB, done in 1.3 s, 0 long tasks.

Large conversation (synthetic export of 400 messages / 635 KB):
- Import 90 ms; first draw 27 ms after click; 40 turns drawn initially, 782 DOM nodes.
- Full scroll up and down: 547 + 386 frames, 0 slow, worst 32 ms; 400 turns, 395 parked, 1153 nodes; JS heap 4 MB; private 307 → 375 MB.
- 50 chat switches: max open 78 ms; private 348 → 549 MB peak, settled at 445 MB including a 58 MB PowerShell PTY.

Account engine, live Haiku 5.5 at low effort, 2 turns ($0.00061 + $0.00012): first text 1877 / 1807 ms. During the turn claude.exe used 435 MB private / 222 MB WS (tree peak 725 MB private); it exited after the turn (tree back to 250 MB).

Error paths:
- Socket cut mid-reply: partial kept, 'Bağlantı… kesildi' with Yeniden dene / Devam et; Continue appended to the same message.
- 529 x2: auto-retry shown '(2/4)', then success in 3.2 s.
- Unreachable base URL: error card after about 23 s of retries.

Security probe: from the artifact iframe, `__TAURI_INTERNALS__.invoke('get_settings')` and `invoke('save_settings', {nickname})` both timed out with no effect. fetch('http://ipc.localhost/…') failed with 'Failed to fetch' (CSP). The iframe has sandbox 'allow-scripts allow-forms allow-downloads'.
