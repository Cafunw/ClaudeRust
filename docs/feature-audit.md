# ClaudeRust vs Claude Desktop 6.18355: feature-gap audit

Date: 2026-11-22. Read-only audit; nothing in D:\ClaudeRust was edited, no app was run or stopped.

## Evidence sources and notation

- **Desktop package** (read-only): `C:\Program Files\WindowsApps\Claude_6.18355.0.0_x64__pzs8sxrjxfjjc\app\resources`. Strings were read from the asar archives with a small header parser (scratchpad `fa/ax.py`, `fa/q.py`); nothing was launched. Only package files were read, no user data.
  - `ion:"…"` = a string in `ion-dist.asar /i18n/en-US.json` (the UI, 45,988 strings). `(n)` = number of strings that match.
  - `main:"…"` = a string in `app.asar /resources/i18n/en-US.json` (main process: menus, tray, dialogs; 1,552 strings).
  - `app:/…` = a file inside `app.asar`. `res:` = a file next to the asar (for example `cowork-svc.exe`).
  - `CL` = the public Desktop changelog, https://claude.com/docs/cowork/claude-desktop-changelog. `RN` = https://support.claude.com/en/articles/12138966-release-notes.
- **ClaudeRust**: `path:line` in the working tree at the time of the audit. Round 4 is editing files right now, so line numbers can drift. **R4** marks an item that round 4 is working on (EV3-01..22, CI-03); those items are listed and not re-planned here.
- Status shows what a user gets today. "WORSE" means the feature exists but is clearly weaker. "unverified" means I could not prove Desktop behaviour from the package or the docs.

## Feature table

| # | Feature | Claude Desktop (evidence) | ClaudeRust today | Status | Buildable locally? | Value | Suggested approach and security risk |
|---|---|---|---|---|---|---|---|
| **Chat core** |||||||
| 1 | Streaming chat, Markdown, tables, LaTeX, code highlighting | core of `ion` | render.rs, markdown.rs (Rust renderer, KaTeX) | EQUAL | done | high | Keep. |
| 2 | Mermaid rendered inside reply code blocks (not only artifacts) | CL 2026-11-12 "Mermaid diagrams render for mermaid code blocks"; app:/resources/artifact-libs/npm_mermaid@11.17.2 | Mermaid only in artifacts (artifacts.rs, artifacts.js); no hit in render.rs/markdown.rs/parts.js | MISSING | S | medium | Render a ```mermaid block in a sandboxed frame through the existing artifact:// path. Vendor mermaid locally instead of the CDN. Risk: diagram text is untrusted; keep `securityLevel:'strict'` and no same-origin access. |
| 3 | Model picker | ion:"Select model", "Choose a model" | composer.js:336; models.rs catalog; `oai:` models | BETTER | done | high | Protect: local and OpenAI-compatible models are extra. |
| 4 | Extended thinking toggle and display | ion:"Toggle extended thinking" | models.rs:254 `thinking_params`; the "Thinking" block is shown. FP-02 (backlog): the switch does nothing on the CLI engine | EQUAL (API) / WORSE (account) | S | medium | Map the switch to the CLI (env `MAX_THINKING_TOKENS` or settings) where the model allows it to be off; on Opus 5.5 thinking is always on, so hide the switch there. |
| 5 | Effort level | ion:"Effort level", "Choose effort with a slider next to the model picker" | models.rs:27; claudecode.rs:733 `--effort` | EQUAL | done | medium | Keep. |
| 6 | Fast mode | ion:"Fast mode on" (42 strings) | absent (no `speed`/fast in src) | MISSING | S | medium | API: `speed:"fast"` on Opus 5.5 / Fable 5.2. CLI: its fast-mode setting. Show the 2x price beside the toggle. Risk: cost only. |
| 7 | Web search with citations | ion:"Web search" (29) | models.rs:33 (web_search_20260209); render.rs:164 citations; account chats get WebSearch through the CLI (claudecode.rs:768) | EQUAL | done | high | Keep. |
| 8 | Web fetch | CL 2026-11-12 (Web search and Web fetch under Connectors, Always ask/Blocked) | models.rs web_fetch; render.rs | EQUAL | done | medium | Optional: an "Always ask / Blocked" policy per server tool, like MCP tools. |
| 9 | Edit, retry, version arrows, fork | ion:"Branch", "Fork" | db.rs:65 message tree; commands.rs:3318 fork_conversation | EQUAL (fork BETTER) | done | high | Keep. |
| 10 | Queue messages while Claude answers | CL (queueing during Research) | composer.js:87 | EQUAL | done | medium | Keep. |
| 11 | Copy reply / Copy as Markdown | ion:"Copy as Markdown" | parts.js:136 copy_text | EQUAL (Markdown form unverified) | S | low | Add "Copy as Markdown" if missing. |
| 12 | Thumbs feedback / Report to Anthropic | ion:"Give feedback", "Report"; main:"Report Artifact…" | absent | MISSING | NO: Anthropic's feedback backend | low | Skip, or a link to Anthropic's feedback page. |
| 13 | Styles / output style | ion:"Output style" (25), "Create style" | settings.js:168 custom styles; prompts.rs:37 | EQUAL | done | medium | Keep. |
| 14 | Profile / custom instructions | ion:"Custom instructions" | settings.js:165 preferences; per-conversation instructions chat.js:228 | BETTER | done | medium | Protect the per-chat instructions. |
| 15 | Attachments: images, PDF, text, docx, paste, drag | ion "Upload a file" | attachments.rs (downscaling, docx text, long paste to file) | EQUAL | done | high | Keep. |
| 16 | xlsx/pptx and PDFs over 100 pages | ion "xlsx" (165), "pptx" (129) | office.rs:22 xlsx_text, :41 pptx_text being added | R4 (EV3-03) | in progress | high | Not re-planned. |
| 17 | Take a screenshot as an attachment | ion/main:"Take a screenshot" | composer.js:447 getDisplayMedia item being added | R4 | in progress | medium | Not re-planned. |
| 18 | Code execution and file creation in chat (make xlsx/pptx/docx/pdf, run analysis) | ion:"Code execution and file creation" (12); app:/resources/bundled-skills/{docx,pdf,pptx,xlsx}.skill | absent (FP-06) | MISSING | M (API engine, via Anthropic's server sandbox) | high | API engine: add the `code_execution` server tool plus Anthropic's pre-built skills (`container.skills`), and download outputs through the Files API into the chat as attachments. Do not copy Desktop's bundled .skill files (proprietary). Account engine: later, CLI Bash/Write restricted to a per-chat temp folder. Risk: none locally for the API sandbox; the local variant runs model-written code, so keep it behind permission prompts. |
| 19 | Skills: use, upload, create, toggle | ion:"Upload skill", "Create a skill", "Only turn on skills from people you trust"; main:"Record a skill" | CLI Code sessions pick up ~/.claude/skills by themselves; chats get none (claudecode.rs:764); no UI | WORSE | S/M | medium | A Skills page: list ~/.claude/skills and the project's .claude/skills, toggle them, install a .zip/.skill after a trust prompt, and add a "create skill" prompt. Built-in engine: load the SKILL.md frontmatter into the tool prompt. Risk: skills carry scripts, so show the contents and ask before enabling. |
| 20 | Memory synthesized from chats (view, edit, per project) | ion:"Generate memory from chat history", "Manage memory", "memory on" (286 "memory") | absent; memory.rs is only CLAUDE.md for Code (FP-09) | MISSING | M | high | A local memory file per profile and per project, regenerated by a cheap model from recent chats; shown and editable in Settings; injected into the system prompt (CLI engine) or exposed through the `memory_20250818` tool (API engine). Risk: privacy; it must be opt-in, easy to delete, and must exclude incognito chats. |
| 21 | Search and reference past chats | ion:"Search and reference chats" | engine.rs:276 conversation_search / recent_chats (API engine only) | EQUAL (API) / WORSE (account) | M | medium | On the account engine, serve the same two tools through the SDK-hosted MCP that already carries MCP tools to the CLI. |
| 22 | Import memory or history from another AI | ion:"Import memory from other AI tool providers", "Bring your history over from another AI" | absent | MISSING | S after #20 | low | Paste or import text into the memory file; parse the ChatGPT export into chats. Risk: untrusted import text becomes instructions, so mark it as data. |
| 23 | Incognito chats | ion:"Incognito" (24), "Incognito chats can't open that type of file" | absent | MISSING | S | medium | A chat flag: no DB rows (or deleted on close), no search, no memory, no title call, CLI transcript deleted at the end (reuse the EV3-02 cleanup). Risk: low; make sure partial saves and crash recovery skip it. |
| 24 | Research mode (multi-step web research report) | ion:"Research mode", "Stop research", "Deep research and extended thinking" | absent | MISSING | M | high | Orchestrate on the API engine: plan, then parallel subagents (subagent.rs) with web_search/web_fetch, then a cited report; a progress view and a hard budget. Account engine: CLI with WebSearch and Task. Risk: cost, and prompt injection from web pages (no write tools during research). |
| 25 | AI-powered artifacts (window.claude.complete), "Try fixing with Claude" | ion:"AI-powered artifacts", "Try fixing with Claude" | artifacts.rs:20; complete() rejects | R4 (EV3-06) | in progress | medium | Not re-planned. |
| 26 | Artifacts: HTML/React/SVG/Mermaid/code, versions, download | ion artifacts; app:/resources/artifact-libs (chart.js, gridjs, mermaid bundled) | artifacts.rs, artifacts.js:126 download; libraries come from public CDNs | EQUAL (offline WORSE) | S/M | medium | Bundle react, recharts, mermaid and lucide locally under artifact://, as Desktop bundles its libs. Risk: none new; adds about 2-4 MB. |
| 27 | Artifact update edits, real runtime prompt | (Desktop edits artifacts in place) | prompts.rs full rewrites | R4 (EV3-07) | in progress | medium | Not re-planned. |
| 28 | Persistent artifact storage | app:/resources/frame/runtime/_runtime/db.*.js | artifacts.rs:286 window.storage | EQUAL (local only) | done | low | Keep. |
| 29 | Publish or share artifacts with a link | ion:"Publish", "Republish", "Unpublish" | absent | MISSING | NO: needs Anthropic hosting | medium | Honest alternative: "Export as standalone HTML" (S). |
| 30 | Install an artifact as its own app window | main:"Install as App…", "Install {app} as an app?"; app:/.vite/build/artifactAppWindow.js | absent | MISSING | S | low | Open the artifact:// document in its own Tauri window with the same CSP; keep it in a local list. Risk: keep the sandbox (no IPC) in the new window. |
| 31 | Slides, Design and Docs artifact types | app:/resources/frame/types/slides, /design (SKILL.md, app.js); RN 2026-09-16 | absent | MISSING | M (single-user, local) | medium | A "slides" artifact kind (HTML deck runtime, present mode, PDF/PPTX export). Team collaboration on Claude Docs is cloud only. Risk: none beyond artifacts. |
| 32 | Office file viewer (docx/pptx preview) | app:/.vite/renderer/file_viewer/assets/{word,slides}-*.js | absent | MISSING | M | low | Render text and slides extracted by office.rs in the preview pane. |
| 33 | Share a chat | ion:"Share chat", "Share chats that use connectors" | absent | MISSING | NO: claude.ai sharing backend | medium | Alternative: the existing HTML report export. |
| 34 | Export chats | ion:"Export data" (account zip by email) | commands.rs:791 per chat Markdown/JSON/HTML | BETTER | done | medium | Protect. |
| 35 | Print or "Download as PDF" for a chat | ion:"Print", "Download as PDF" | absent; main.js:182 blocks the WebView print key on purpose | MISSING | S | low | Render the HTML report in a hidden window and call print. Risk: none. |
| **Projects** |||||||
| 36 | Projects with instructions and knowledge files | ion:"Project instructions", "Project knowledge" | projects.js; prompts.rs:111 | EQUAL | done | high | Keep. |
| 37 | Project knowledge search for large projects | ion:"Project knowledge search"; "project_knowledge_search" (13, eval 3) | everything inlined, projects.js:11 2M chars (EV3-13 deferred) | WORSE | M | medium | Above a token threshold, index project text with the existing FTS5 and expose a `project_knowledge_search` tool. Risk: low. |
| 38 | Project PDFs and images on the account engine | n/a | dropped | R4 (EV3-01) | in progress | high | Not re-planned. |
| 39 | Move a chat to or from a project | ion:"Move to project" | chat.js:210 | EQUAL | done | low | Keep. |
| 40 | Shared and team projects | ion (org strings) | absent | MISSING | NO: claude.ai account backend | low | None. |
| **Sidebar, history, search** |||||||
| 41 | Search chats and projects | ion:"Search chats and projects" | db.rs:345 FTS5, :819; search.js:15 (about 4 ms) | BETTER | done | high | Protect. |
| 42 | Star / rename / delete | ion:"Star", "Rename" | db.rs:54 starred | EQUAL | done | medium | Keep. |
| 43 | Pin, archive, mark unread, sidebar sections and groups | ion:"Pin", "Archive", "Mark as unread", "New section", "New group" | star only (FP-20) | WORSE | S | medium | Add columns (pinned, archived_at, unread, group_id) through the existing schema migration, plus sidebar menus. Risk: none. |
| 44 | Cloud sync with claude.ai and mobile | Desktop is a claude.ai client | local SQLite only | MISSING | NO: no public account API | high (for multi-device users) | Keep the claude.ai export import as the bridge. |
| 45 | Import claude.ai export and Claude Code sessions | ion:"That doesn't look like a claude.ai export…"; main:"Import Claude Code CLI Sessions…" | import.rs:269 (chats with branches, artifacts, projects; CLI sessions; Desktop MCP config and extensions) | EQUAL (MCP import BETTER) | done | medium | Keep. |
| **MCP, connectors, extensions** |||||||
| 46 | Local MCP servers (stdio) | standard | mcp.rs (supervisor, restarts) | EQUAL | done | high | Keep. |
| 47 | Remote MCP (HTTP, legacy SSE) with OAuth; "Add custom connector" | ion:"Add custom connector" (11) | mcp.rs, mcp_sse.rs, mcp_oauth.rs:252; form in ui/js/mcp_connector.js | R4 (EV3-11) | in progress | high | Not re-planned. |
| 48 | Connector directory ("Browse connectors") | ion:"Browse connectors", "Directory" | absent | MISSING | M | high | A curated list of vendor-hosted remote MCP servers (name, URL, OAuth), shipped in the binary, that opens the EV3-11 form pre-filled. Risk: the list is a trust decision; show the server's domain before sign-in and never fetch the list from an unsigned source. |
| 49 | Anthropic-run connectors (Gmail, Google Drive, Calendar, Microsoft 365) | ion:"Connect Gmail", "Microsoft 365"; res: app:/resources/office365-mcp, github-mcp-server.exe | absent | MISSING | NO for the Anthropic-hosted ones (Anthropic's OAuth clients and claude.ai account); vendor-hosted MCP servers work through #47/#48 | high | Point users to vendor-hosted or self-hosted servers in the directory. |
| 50 | Per-tool permissions (Allow / Ask / Block), per-chat toggles | CL 2026-11-12 | per-tool Ask/Allow/Block, per-chat server toggles (eval 3 strengths) | EQUAL | done | medium | Keep. |
| 51 | MCP prompts and resources | ion "Slash commands" | mcp.rs:1128, :1173; argument form in progress (EV3-16) | EQUAL (R4 form) | done | medium | Keep. |
| 52 | MCP elicitation | ion strings present (eval 3) | mcp_elicit.rs; mcp.rs:344 | EQUAL | done | low | Keep. |
| 53 | MCP Apps (interactive ui:// widgets, side panel) | ion:"text/html;profile=mcp-app" (9, eval 3), "MCP Apps widgets"; CL 2026-11-17 side panel | absent (EV3-04 deferred) | MISSING | L | medium (growing) | Render `_meta.ui.resourceUri` in the artifact sandbox with a whitelisted postMessage bridge (tools/call goes through the normal permission check). Risk: widget HTML is untrusted: no same-origin, no IPC, and every tool call from a widget is permission-checked. |
| 54 | MCP roots/list and sampling | unverified for Desktop | mcp.rs:1284 roots answers an empty list; no sampling | unverified | S | low | Return the Code session's folder as a root. Sampling: only behind a per-request prompt (cost and injection risk). |
| 55 | Desktop extensions (.mcpb/.dxt) install with a settings form | main:"Install Extension…", "Install Unpacked Extension…" | extensions.rs:44 | EQUAL | done | medium | Keep. |
| 56 | Extension directory, update, uninstall, reconfigure | ion:"Browse extensions"; main:"Uninstall…", "Organization Extension Update Available" | install only (FP-19) | WORSE | S/M | medium | Add uninstall and reconfigure first (S); version check against the manifest URL later. Risk: updates replace code, so verify the manifest and show permissions again. |
| 57 | Claude Code plugins and marketplaces | ion:"Browse plugins", "Marketplace", "Add plugin" ("plugin" 719); main:"Add {plugin} from {marketplace} to {host}?" | no UI; the CLI may load what is installed | MISSING | M | medium | A Plugins page that drives `claude plugin` (list, marketplace add, install, enable) and shows each plugin's hooks, MCP servers and commands before enabling. Risk: plugins run third-party code and hooks; require explicit trust. |
| **Code tab** |||||||
| 58 | Code sessions through the Claude Code CLI (resume, fork) | ion:"Claude Code" | claudecode.rs | EQUAL | done | high | Keep. |
| 59 | Second engine: built-in Rust agent on an API key | none | codetools.rs, engine.rs | BETTER | done | medium | Protect. |
| 60 | Permission modes: Ask, Auto, Accept edits, Plan, Bypass | ion:"Auto mode", "Accept edits", "Bypass permissions" | settings.js:360; codetools.rs Auto classifier | EQUAL | done | high | Keep. |
| 61 | Ultracode, workflows, Tasks pane | ion:"Ultracode", "Workflow" | claudecode.rs:717, :933; lib.rs:533 workflows_list | EQUAL | done | medium | Keep. |
| 62 | Changes, diff, review comments, commit with a Claude message | ion:"Diff", "Review comment" | ui/js/panes/changes.js, review.js, commit.js; secrets.rs:49 pre-commit scan; hunk revert, patch, stash | BETTER | done | high | Protect the extras. |
| 63 | Terminal pane | main:"Show Terminal" | terminal.rs:113 PTY | EQUAL | done | medium | Keep. |
| 64 | Preview of a local dev server | ion:"Preview" | ui/js/panes/preview.js:84 | EQUAL | done | medium | Keep. |
| 65 | Built-in browser that Claude can drive (page text, screenshots, network) | ion:"Built-in browser" (45), "Always allow reads on this host" | Claude cannot see or use the Preview pane (FP-15) | MISSING | L | medium (high for web devs) | Expose the preview webview to the agent as tools (navigate, read text, screenshot, click) through a local MCP; ask per host. Risk: the agent reads untrusted pages (prompt injection); allow localhost by default, ask for every other host. |
| 66 | Files panel, history pane, find in files | main:"Show Files", "Find in Files…" | ui/js/panes/files.js, history.js | EQUAL (blame, edited-by chats BETTER) | done | medium | Keep. |
| 67 | Create a pull request | ion:"Create PR" | pullrequest.rs:30 | EQUAL | done | medium | Keep. |
| 68 | PR CI status, review comments, auto-merge | ion:"Auto-merge" (44), "CI: all checks passed", "Open review comments" | absent (FP-16) | WORSE | M | medium | Poll `gh pr checks` and `gh pr view --json` for the session's branch; show status; auto-merge through `gh pr merge --auto`. Risk: low; merging must be a click, never automatic. |
| 69 | Worktree isolation | ion:"Worktree" | worktree.rs:104 | EQUAL | done | medium | Keep. |
| 70 | Checkpoints and rewind | standard | checkpoints.rs:31 (also on the built-in engine) | EQUAL | done | high | Keep. |
| 71 | Hooks, subagents, custom slash commands, CLAUDE.md memory | ion:"Hooks", "Subagents", "Slash commands" | hooks.rs:101, subagent.rs:24, slash.rs, memory.rs:156 | EQUAL | done | medium | Keep. |
| 72 | Output styles for Code sessions | ion:"Output style", "New output style" | absent for Code | MISSING | S | low | Pass the CLI's output-style setting per session; list ~/.claude/output-styles. |
| 73 | Monitor: a watched command starts a new turn | CL 2026-11-03 | watch.rs:27 watch mode | EQUAL (BETTER: detects the check command) | done | medium | Keep. |
| 74 | Side chat (ask beside a session without derailing it) | ion:"Side chat" (48); main:"New Side Chat", "Hide Side Chat" | absent (fork and quote/explain exist) | MISSING | M | medium | A side pane that forks the context read-only into a temporary chat with its own composer; nothing goes back into the main transcript unless the user copies it. Risk: none. |
| 75 | Split view and multiple windows | main:"Split View", "New Session in New Window", "Open in split view" | single main window (FP-14) | MISSING | M/L | medium | Start with "Open in new window" (a second webview on the same store); split view later. Risk: two views of one turn stream; guard concurrent sends. |
| 76 | SSH remote sessions | ion:"SSH", "Teleport sign-in needed"; res: claude-ssh-broker.exe, -proxy, -askpass | absent (FP-17) | MISSING | M/L | medium | Spawn `ssh host claude -p --output-format stream-json …` and reuse the CLI parser; files and diff over ssh later. Risk: host-key trust (refuse unknown keys), never store passwords; permission prompts still run on the remote CLI. |
| 77 | WSL sessions on Windows | main:"Restart WSL", "Update WSL?" | absent | MISSING | S/M | medium (Windows devs) | Run the CLI as `wsl.exe -d <distro> claude …` with path translation. Risk: low. |
| 78 | Cloud environments, Remote Control, Dispatch, Teleport, Ultrareview | ion:"Cloud environment", "Remote Control" (109), "Dispatch", "Run ultrareview in the cloud?" | absent | MISSING | NO: Anthropic cloud and account | medium | None. |
| 79 | iOS Simulator and Android emulator panes | ion:"Let Claude verify your changes in the iOS Simulator…"; app:/.vite/build/emulator-grpc-worker | absent | MISSING | L (iOS needs macOS and Xcode) | low | Skip for now. |
| 80 | Session cost and context meter | main:"Session cost (estimate): {cost}" | context_ui.js:9; per-answer time and cost; `/usage` cost dashboard usage_ui.js:28 | BETTER | done | medium | Protect. |
| 81 | Spending cap per turn | none found | settings.js:335; `--max-budget-usd` | BETTER | done | medium | Protect. |
| 82 | Plan usage limits (5-hour and weekly) | main:"5-hour limit", "Weekly · all models", "Your usage limits"; ion:"Plan usage limits"; CL 2026-11-05 | absent; the CLI's rate-limit events are dropped (FP-08) | MISSING | S | high (account users) | Keep the CLI's rate-limit info from stream-json, show a usage card and a sidebar meter, and warn near the limit. Risk: none. |
| 83 | Keep the computer awake while agents run | ion:"Keep your computer awake so long tasks can finish." | absent (FP-22) | MISSING | S | medium | SetThreadExecutionState (Windows), IOPMAssertion (macOS), systemd-inhibit (Linux) while a turn, workflow or routine runs. Risk: battery only; show it in the tray. |
| **Agents beyond code** |||||||
| 84 | Cowork (autonomous non-coding agent with files, connectors, scheduled tasks) | ion "cowork" (311); res: cowork-svc.exe, smol-bin.x64.vhdx; RN | absent (FP-07) | MISSING | L (a folder-based "lite" version is M; the VM sandbox is L+) | high (non-developers) | Lite version: a "Tasks" mode that runs the CLI agent in a chosen folder with a non-coding prompt, connectors, checkpoints and routines. A VM sandbox needs Hyper-V or Windows Sandbox. Risk: file changes outside Git: keep the checkpoints, ask before any delete. |
| 85 | Scheduled tasks for chat and Cowork, with run status and time zone | ion:"Scheduled tasks" (332), "Routines" (267); CL 2026-11-10 run status | routines.rs:1 Code sessions only, interval or daily | WORSE | S/M | medium | Let a routine target a chat or a project; record run status (ran, failed, needs input, skipped) and a time zone. Risk: unattended runs spend money; reuse the per-turn budget. |
| 86 | Computer use (operate desktop apps) | ion:"Computer use" (30); app:/.vite/build/computerUseTeach.js; package.json `@ant/computer-use-mcp` | absent | MISSING | L, high risk | medium | API `computer_toolset_20260801` plus a local executor (screen capture, SendInput) with a per-app grant and a kill switch. Risk: very high (it acts as the user); do it last. |
| 87 | Claude in Chrome | ion (77); res: chrome-native-host.exe | absent | MISSING | NO: Anthropic's extension and bridge.claudeusercontent.com | medium | Alternative: a browser-automation MCP entry in the directory (#48). |
| 88 | Claude Meetings (joins Google Meet) | RN 2026-11-12 | absent | MISSING | NO: cloud product | low | None. |
| **Voice** |||||||
| 89 | Dictation | ion:"Dictate", "Stop dictation" (44); app:/resources/frame/shell/frame-shell-dictate.js | absent (FP-11) | MISSING | M (OS speech APIs or a local Whisper model) | medium | Windows.Media.SpeechRecognition, Apple Speech, or bundled whisper.cpp as an opt-in download. Risk: microphone permission; keep audio local and say so. |
| 90 | Read aloud | ion:"Read aloud", "Read Aloud couldn't start." | absent | MISSING | S | medium | speechSynthesis in the WebView with OS voices, plus a stop button. Risk: none. |
| 91 | Voice mode (spoken conversation) | ion:"Voice mode" (14); app:/audio/voice/* | absent | MISSING | NO for an equal feature (claude.ai's real-time voice service; no public API found, unverified); a local STT+TTS loop is L | medium | Only after #89/#90, labelled as an approximation. |
| **App shell** |||||||
| 92 | Quick Entry | main:"Open Quick Entry", "Quick Entry keyboard shortcut" | lib.rs:81; ui/js/quick.js:12 text only (FE-20 drops text on a failed send) | WORSE | S/M | medium | Add screenshot and file attach to the quick window and keep the text on failure. Risk: screen capture permission. |
| 93 | Tray, launch at login, run in background | main:"Claude runs in the system tray" | lib.rs:63 TrayIconBuilder; settings.js:376-377 | EQUAL | done | medium | Keep. |
| 94 | Notifications | main:"Display notifications", "Scheduled task completed" | lib.rs:259 notification plugin; settings.js:378 | EQUAL (per-type settings unverified) | done | medium | Keep. |
| 95 | Keyboard shortcuts | ion:"Keyboard shortcuts" | keymap.js:21; settings.js:394 rebindable | BETTER (no rebinding strings found in Desktop, unverified) | done | medium | Protect. |
| 96 | Command palette, find in page | main:"Command Palette…", "Find…" | search.js:15 (`>` commands, `@` files); findbar.js:84 | EQUAL | done | medium | Keep. |
| 97 | Spell check | main:"Check spelling", "Add to dictionary", "Spelling languages" | WebView2's native menu stays in text fields (main.js:176); composer.js:94 spellcheck=true; no language choice | EQUAL (language choice missing) | S | low | Optional: expose the spelling language. |
| 98 | Interface languages | ion: 11 UI locales (de, en, es-419, es-ES, fr, hi, id, it, ja, ko, pt-BR); main: 43 menu/dialog locales including tr | i18n.js:1307 English and Turkish only | WORSE | M | medium-high | Add the Desktop 11 locales from translation files that a person reviews. Native menus and the tray are still hard-coded English (EV2-07). Risk: bad machine translation of permission prompts, so review those strings first. |
| 99 | Accessibility: screen reader, live regions, focus traps, reduced motion | ion:"Accessibility", "Accessibility reader" (Desktop settings detail unverified) | aria-live only in ui/js/mcp_connector.js:64; FE-15 and FP-18 open; reduced motion in progress (EV3-22) | WORSE (likely) | S/M | medium | A live region for streaming answers, toasts and errors; focus trap and restore in modals; labels on icon buttons; keyboard path for permission cards (FE-17). Risk: none. |
| 100 | Theme, chat font, zoom | ion:"Chat font", "UI font", "Code font", "Response font" | settings.js:202-213 theme, chat font incl. dyslexic, chat width, zoom | EQUAL (no separate UI/code font) | S | low | Optional code-font setting. |
| 101 | Auto-update | main:"Check for Updates…", "Restart to Update to {updateVersion}" | updates.rs:36 signed updater manifest | EQUAL (installer not Authenticode-signed: EV3-18, owner decision) | done | high | Keep. |
| 102 | Platforms | Windows, macOS; Linux beta on Ubuntu/Debian (RN, July 2026) | Windows MSI/NSIS, macOS universal, Linux deb/rpm/AppImage | BETTER on Linux | done | medium | Protect. |
| 103 | Deep links | ion:"claude://cowork/shared-artifact?uuid=…" | deeplink.rs:26 clauderust:// | EQUAL | done | low | Keep. |
| 104 | Diagnostics: logs, diagnostic report, MCP log file, clear cache | main:"Generate Diagnostic Report", "Show Logs in File Explorer", "Open MCP Log File", "Clear Cache and Restart" | applog.rs log file; About shows the data folder; no "Show logs" or MCP log view | WORSE | S | low | Add "Show logs" and "Copy diagnostics (redacted)" to About; keep per-MCP-server stderr. Risk: redact keys and paths. |
| 105 | Remote desktop and GPU fallback | main:"Disable Hardware Acceleration", "Graphics problems detected" | display.rs:29 detects RDP/VNC/xrdp itself; setting in settings.js:382 | BETTER | done | medium | Protect. |
| 106 | Settings backup and restore | none found | settings.js:450 About | BETTER | done | low | Protect. |
| 107 | Third-party inference (Bedrock, Vertex, Foundry, gateway) | main:"Configure Third-Party Inference…", "Third-party inference" | base URL only (settings.js:245) | WORSE | M | low (typical user) | Bedrock (SigV4) and Vertex (ADC) clients behind the existing API client. Risk: cloud credentials, use the keyring. |
| 108 | OpenAI-compatible and local models in chat | none | oai.rs:52 (Ollama, LM Studio, llama.cpp, vLLM, OpenRouter) | BETTER | done | medium | Protect. |
| 109 | Enterprise policies (ADMX, managed settings, config template) | main:"Group Policy template (ADMX)", "Export Configuration Template…" | absent | MISSING | M | low | Read a managed JSON policy file first; ADMX later. |
| 110 | Account, plan, billing and privacy pages | ion:"Billing", "Privacy", "Usage", "Account" | Account page = CLI sign-in and API key | MISSING | NO: claude.ai web account | low | Link out to claude.ai and the Console pages. |
| 111 | Hardware Buddy, 1Password logins, data-folder choice | main:"Hardware Buddy & Maker Devices", "Claude wants to use your 1Password logins", "Choose Claude data folder" | absent | MISSING | NO (Buddy, 1Password: Anthropic integrations) / S (data folder) | low | Skip; data-folder choice only if asked. |

## Notes

- **Counts** (by the first word of the Status column). Of the 111 rows: EQUAL 40 (rows 4 and 21 are WORSE on the account engine), MISSING 40, BETTER 13, WORSE 11, R4 in progress 6, unverified 1. Twelve MISSING rows cannot be built honestly (marked NO; list B).
- **Account engine vs API engine.** Several rows are EQUAL on the API engine and WORSE on the Claude-account (CLI) engine, which is the default without an API key: past-chat search (#21), the thinking switch (#4), project files (#38, R4). The gap is in the bridge, not the features, so these fixes are cheap.
- **Code execution is the key enabler.** #18 (server-side code execution plus Anthropic's pre-built skills through the API) also unlocks file creation, xlsx/pptx output, data analysis and most of what "Skills" means in chat. It needs no local sandbox, so it carries no local code-execution risk. Desktop's bundled `.skill` files (app:/resources/bundled-skills) are Anthropic's and must not be copied.
- **Cowork is the biggest deferred item.** Desktop ships a Hyper-V VM image (res: smol-bin.x64.vhdx, cowork-svc.exe) for it. A folder-based Cowork-lite (#84) is the honest local version and reuses the CLI engine, checkpoints, connectors and routines.
- **Desktop locale count.** The UI (ion) has 11 locales. The main process (menus and dialogs) has 43, including Turkish. "Eleven languages" in the backlog is right for the UI.
- **Measurements carried over from eval 3** (not re-measured here): ClaudeRust idle about 215 MB private over 7 processes and 0.08 CPU-s per 30 s; first paint 433-610 ms; exe 27 MB, NSIS 8 MB. Desktop, while busy, 2,199 MB private over 14 processes. Its package is claude.exe 250 MB + app.asar 100 MB + ion-dist.asar 221 MB + a 37.7 MB VM image. The conditions differ, so these numbers are indicative only.

### Where ClaudeRust is already better (protect in every round)

1. Engines: the Claude Code CLI, a built-in Rust agent on an API key, and OpenAI-compatible local or remote models in chat (oai.rs).
2. Footprint: about 10x less idle memory than busy Desktop, an 8 MB installer, and a first paint in about half a second.
3. Local-first data: FTS5 search in about 4 ms with snippets; per-chat Markdown, JSON and HTML export; import of claude.ai exports, Claude Code sessions, and Desktop's MCP config and extensions.
4. Code extras Desktop lacks or only partly has: watch mode with check-command detection, the architecture map, /tour, /todos, project scripts, the pre-commit secret scan, blame with "conversations that edited this file", hunk revert, patch copy and apply, stash, `-S` history search, a per-turn spending cap, the overload fallback model, checkpoints on the built-in engine, and a 40 MB log digest.
5. Chat extras: per-conversation instructions, Improve my message, quote and explain a selection, Ctrl+R prompt history, a conversation outline with Alt+Up/Down, `@https://` page mentions, fork from any answer.
6. Shell: rebindable shortcuts, settings backup and restore, automatic remote-desktop and GPU fallback, Linux deb, rpm and AppImage packages, open source (MIT), and no telemetry code (no telemetry or analytics hits in src-tauri/src).

## (A) The 15 best value-per-effort items to build next

All 15 are MISSING or WORSE, can be built locally, and are not in round 4.

| Rank | Item (row) | Effort | Value | Why it ranks here |
|---|---|---|---|---|
| 1 | Plan usage-limit card and meter from the CLI's rate-limit events (#82) | S | high | The default engine is the subscription; users cannot see how much of their limit is left. |
| 2 | Incognito chats (#23) | S | medium | Small, private; reuses the EV3-02 cleanup. |
| 3 | Mermaid in reply code blocks (#2) | S | medium | Reuses the artifact sandbox; Desktop added it in November. |
| 4 | Pin, archive, mark unread, sidebar sections (#43) | S | medium | Daily list hygiene; schema migrations already exist. |
| 5 | Keep the computer awake during turns, workflows and routines (#83) | S | medium | Long agent runs die on sleep today. |
| 6 | Read aloud (#90) | S | medium | One WebView API; also helps accessibility. |
| 7 | Fast mode toggle (#6) | S | medium | One request field (API) and one CLI setting. |
| 8 | Code execution and file creation in chat, via the API server tool and Anthropic's pre-built skills (#18) | M | high | The biggest chat gap; Anthropic's sandbox, so no local risk. |
| 9 | Synthesized chat memory, editable and per project (#20; then #22 import) | M | high | A headline Desktop feature; local and opt-in. |
| 10 | Connector directory on top of the R4 custom-connector form (#48) | M | high | Turns MCP from JSON editing into one click. |
| 11 | Research mode with subagents and web tools (#24) | M | high | Reuses subagent.rs and the web tools; needs a budget cap. |
| 12 | Skills page: list, toggle, install with trust prompt, create (#19) | S/M | medium | Skills already work in CLI sessions; there is just no UI. |
| 13 | Screen-reader live regions, modal focus and permission-card keyboard path (#99) | S/M | medium | Accessibility is a hard requirement; the cost is small. |
| 14 | Dictation, local and opt-in (#89) | M | medium | Second most visible input feature after attachments. |
| 15 | Desktop's 11 UI locales, reviewed (#98) | M | medium-high | Most of the world does not use English or Turkish. |

Next tier (just outside the 15): chat-target routines with run status (#85), extension uninstall and reconfigure (#56), PR CI status and auto-merge (#68), WSL sessions (#77), past-chat search on the account engine (#21), Quick Entry attachments (#92), side chat (#74), project knowledge search (#37), bundled artifact libraries for offline use (#26), Cowork-lite (#84, M/L), plugins page (#57).

## (B) Features that cannot be built honestly, and why

| Feature (row) | Why not |
|---|---|
| Chat and project sync with claude.ai and mobile (#44) | No public account API. The only ways are private endpoints or copied session tokens, which breaks Anthropic's terms and risks the user's account. |
| Share chat, publish artifacts with a public link (#33, #29) | Needs Anthropic's hosting and claude.ai identity. A self-hosted share would not be "Claude sharing". Honest substitute: HTML export. |
| Anthropic-run connectors (Gmail, Drive, Calendar, Microsoft 365 through claude.ai) (#49) | Their OAuth clients and proxies belong to Anthropic and claude.ai accounts. Vendor-hosted MCP servers through the directory are the substitute. |
| Remote Control, Dispatch, Teleport, cloud environments and sessions, Ultrareview in the cloud, cloud scheduled tasks (#78) | They run on Anthropic's cloud and need claude.ai device trust. |
| Claude in Chrome (#87) | Anthropic's own extension and native host talk to bridge.claudeusercontent.com. Substitute: a browser-automation MCP. |
| Voice mode equal to Desktop's (#91) | claude.ai's real-time voice service; no public API found (unverified). A local STT+TTS loop would be a different, weaker feature. |
| Thumbs feedback and content reports to Anthropic (#12) | Needs Anthropic's feedback backend. |
| Account, plan, billing, privacy, org admin, SSO, analytics (#110) | These are claude.ai and Console web functions; ClaudeRust can only link to them. |
| Claude Meetings (#88) | A cloud meeting bot. |
| Hardware Buddy and 1Password logins (#111) | Anthropic-specific integrations (device protocol and partner integration not public; unverified). |
| Desktop's bundled Anthropic skills (app:/resources/bundled-skills/*.skill) | Proprietary content. Use the API's pre-built skills instead (#18). |
| Calling the Messages API with a subscription from chat | Anthropic allows subscription use only in its own apps and the CLI. ClaudeRust correctly routes subscription chats through the CLI. |

Buildable but large or risky (not in A): MCP Apps (#53, L), a built-in browser Claude can drive (#65, L, injection risk), computer use (#86, L, very high risk), Cowork with a VM sandbox (#84, L+), split view (#75, M/L), SSH sessions (#76, M/L), iOS and Android emulators (#79, L).
