# ClaudeRust — Audit & Roadmap

Working log for the overnight autonomous run. Each item is updated as it is
finished; the git history has one commit (or more) per item.

Legend: `✅ Tam` done · `⚠️ Bozuk/Yarım` broken or partial · `❌ Eksik` missing ·
`[IN PROGRESS]` being worked on · `[BLOCKED]` could not be finished (reason noted).

Architecture rules followed throughout:
- Backend logic lives in focused Rust modules under `src-tauri/src/` (one
  module per subsystem), exposed to the UI through `#[tauri::command]`s in
  `commands.rs` and events emitted with `app.emit`.
- Git is driven through the `git` CLI (`shellenv::command`), not libgit2, to
  keep the binary small and behave exactly like the user's git.
- UI stays dependency-free ES modules in `ui/js` (no build step). Third-party
  browser code, if any, is vendored under `ui/vendor/`.
- Every change: `cargo check`, `cargo clippy`, `cargo test`, `node --check`
  on the UI, then a commit on `ccr-ae1e84d3-7xu1u8`.

---

## Initial audit (before this run)

| Area | State | Notes |
|---|---|---|
| Multi-session runs | ✅ Tam | `AppState.runs` / `live` keyed by conversation; runs in parallel across conversations; Stop cancels via `CancellationToken`. |
| Session cleanup on delete | ⚠️ Yarım | Deleting a conversation stops the run but leaves pending permission prompts and staged state behind. |
| Bash tool cancel/kill | ✅ Tam | Own process group, killed on Stop/timeout/2 MB cap (`kill_process_group`, `taskkill /T`). |
| Bash live output | ⚠️ Yarım | Output only appears when the command finishes; long builds look frozen. |
| Background commands | ❌ Eksik | No `run_in_background`, no BashOutput/KillShell, nothing to list or stop them. |
| PTY terminal | ❌ Eksik | No interactive terminal in the app. |
| Diff engine | ⚠️ Yarım | Unified diff only in permission cards (`preview_diff`); no session-wide change view. |
| Changed-file detection / revert | ❌ Eksik | No list of files changed by the agent, no revert. |
| Permission modes | ⚠️ Yarım | default / acceptEdits / plan / bypassPermissions enforced in `codetools::check_permission`; **Auto mode missing**; MCP tools ignore plan mode. |
| Resizable panes | ⚠️ Yarım | Only the artifact panel has a resizer; no side workspace panel. |
| `@` file mentions | ❌ Eksik | |
| `/` slash commands | ❌ Eksik | MCP prompts only reachable from the + menu. |
| Worktree isolation | ❌ Eksik | |
| Checkpoints / rewind | ❌ Eksik | Conversation branching exists, files are not rewound. |
| Inline review comments | ❌ Eksik | |
| Plan & Tasks panes | ❌ Eksik | Todos shown inline only. |
| CLAUDE.md / AGENTS.md | ⚠️ Yarım | CLAUDE.md hierarchy loaded (`load_memory`); AGENTS.md and quick memory (`#`) missing. |
| Compaction / cost counter | ⚠️ Yarım | Usage popover exists; no compaction, no live context meter. |
| MCP client | ✅ Tam | stdio + streamable HTTP, tools/prompts/resources, per-tool policy. |
| Hooks | ❌ Eksik | |
| Sub-agents | ❌ Eksik | |
| Slash commands /review /compact /init /doctor /cost | ❌ Eksik | |
| Live preview panel | ❌ Eksik | |
| Routines (scheduled tasks) | ❌ Eksik | |

---

## Faz 1 — FIX

- [x] F1. ✅ Tam — Session lifecycle: deleting a conversation now stops its run, kills its background shells, drops its live state and pending permission prompts.
- [x] F2. ✅ Tam — Bash output streams live into the tool card (`tool-progress` event, 120 ms throttle); new `shells.rs` registry with `run_in_background`, `BashOutput` (incremental, regex filter), `KillShell` (conversation-scoped); shells killed on delete and app exit; `shells_*` commands for the Tasks pane. Tests: `background_shell_output_and_kill`, `bash_reports_live_output`, shells buffer tests. Verified in the app (live steps, dev server started/listed/killed).
- [x] F3. ✅ Tam — Real PTY terminal: `terminal.rs` (portable-pty, user's login shell, UTF-8-safe `term-output` events, resize, close on delete/exit) + Terminal pane with vendored xterm.js 6 (lazy-loaded, theme-aware, restart on exit). Ctrl+` toggles it.
- [x] F4. ✅ Tam — `gitops.rs`: changes vs `HEAD` in git repos (porcelain v1 + numstat, untracked counted), vs recorded originals outside git (`CodeSession::originals`, captured before the agent's first Write/Edit); per-file unified diff; revert file / revert all (`git restore`, untracked deleted, path traversal refused). Changes pane: branch, +/- totals, line-numbered diff viewer (hookable for inline comments), revert with confirmation, auto-refresh on `files-changed` and turn end. Tests: porcelain parsing, real git repo diff/revert, non-repo originals.
- [x] F5. ✅ Tam — Auto mode in `check_permission`: edits inside the project run (except `.env`, keys, `.git/` …), Bash runs unless `bash_risk` flags it (sudo, push, force, reset --hard, publish, rm outside the project, writes outside via redirection, network tools …), read-only MCP tools (`readOnlyHint`) run. Plan mode now denies non-read-only MCP tools. Claude Code engine: auto maps to the CLI's default mode and we approve safe `can_use_tool` requests ourselves. UI pickers + i18n. Test: `auto_mode_allows_safe_and_asks_risky`; verified a build ran unprompted in the app.
- [x] F6. ✅ Tam — `filesearch.rs` (cached .gitignore-aware walk, fuzzy scorer favouring file names/boundaries/runs) + `@` picker in the composer; on send, `@path` mentions add the file (numbered) or directory listing as hidden reminders and mark the file as read (the Claude Code CLI expands mentions itself). `slash.rs`: custom commands from `.claude/commands` and `~/.claude/commands` (namespaces, front matter, `$ARGUMENTS`/`$1..$9`). `slash.js` registry + `suggest.js` popup (keyboard navigation); built-ins `/help /model /permissions /clear /terminal /changes /tasks /cost`. Tests: fuzzy, mention parsing, expansion, front matter, namespaced commands. Verified both popups and both expansions against the mock API.
- [x] F7. ✅ Tam — `workspace.js`: resizable side panel for code sessions with a pane registry (per-conversation pane DOM kept alive), width/open/tab persisted; Tasks pane lists background shells with live output, elapsed time and stop buttons (badge = running count).

## Faz 2 — IMPLEMENT

- [x] 1. ✅ Tam — `worktree.rs`: per-session `git worktree add -b clauderust/<id>` under the app data folder (keeps the sub-folder the session started in); setting `code_worktree` = parallel (default: only when another session in the same repo is running) / always / off, plus a per-session chip (auto/on/off) on the Code page. Deleting the session removes the worktree and its branch (kept if it has its own commits). Changes pane gets Commit (on the session branch) and Apply to <base> (`git apply --3way` of committed + uncommitted + new files). Topbar shows the branch. Tests: create/apply/remove, branch kept with commits. Verified end to end in the app.
- [x] 2. ✅ Tam — `checkpoints.rs`: shadow git repo per project folder in the app data dir (own GIT_DIR, work tree = project; project .gitignore honoured, heavy folders excluded; project's .git untouched; works outside git). A snapshot is taken before every code turn (both engines) and stored on the user message. Rewind (button on each user message + `/rewind`): restore code and conversation / conversation only / code only, with a preview of changed files; the conversation rewind puts the message back in the composer (and makes the CLI fork); code rewinds snapshot the current state first so they can be undone from the toast. Setting to turn checkpoints off. Test: snapshot/restore/undo roundtrip. Verified in the app.
- [x] 3. ✅ Tam — `panes/review.js`: hover a diff line → comment editor (Ctrl+Enter), comments shown under their lines with edit/delete, stored per session; header button "Send N comments" composes one revision prompt (`file:line`, quoted line, comment) and sends it through the composer. Verified the prompt reached the API.
- [x] 4. ✅ Tam — Plan pane (`panes/plan.js` + `plan_state`): latest todo list with progress bar and badge (done/total), latest plan rendered as Markdown with its status (awaiting approval / approved / not approved), live refresh on TodoWrite/ExitPlanMode turns, "Switch to plan mode" when empty. Tasks pane (from F7) tracks background commands with live output and stop; sub-agents join it in item 8. Test: `latest_todos_and_plan_status`; verified in the app incl. approval.
- [x] 5. ✅ Tam — `memory.rs`: user `~/.claude/CLAUDE.md`, every ancestor's `CLAUDE.md`, `.claude/CLAUDE.md`, `AGENTS.md`, `CLAUDE.local.md`, with `@path` imports (outside code fences, depth 4, each file once, size caps). Memory is re-read on every message (edits apply mid-session). `# note` in the composer offers to save it to project / private / user memory; `/memory` opens an editor for the session's memory files (writes restricted to those files). Tests: imports + dedupe, append/save limits. Verified the note reached the system prompt.
- [x] 6. ✅ Tam — `compact.rs`: summary stored on the user message it precedes (`meta.compact_summary`); `build_messages` drops everything before it and prefixes the summary (history and branches stay in the DB). `/compact [instructions]` (built-in engine; forwarded to the CLI for the Claude Code engine) and automatic compaction at the start of a turn when the last response's context ≥ `auto_compact_pct` (default 85%, setting with Never/70/85/95). Topbar context meter (ring + % + cost, tooltip with tokens; click → `/cost`), `usage_stats` now reports context tokens/window. Collapsible "Earlier conversation summarized" divider. Test: apply/context/transcript. Verified the next request contained only the summary + new message.
  - Limitation: compaction runs between turns, not in the middle of a long tool loop.
- [x] 7. ✅ Tam — MCP client was already complete (stdio + streamable HTTP, tools/prompts/resources). New `hooks.rs`: Claude Code's `hooks` format from `~/.claude/settings.json` and the project's `.claude/settings(.local).json`; PreToolUse (block / allow / ask, regex matchers, `mcp__` names), PostToolUse (feedback to Claude), UserPromptSubmit (context or block), SessionStart (context), Stop (keep going, max 3×). Exit 2 / JSON output semantics, timeouts kill the process group. Project hooks run only after the user trusts that exact configuration (hash; asked again when it changes); `/hooks` shows active hooks. Tests: parsing/matching, exit-code + JSON interpretation, real commands with trust. Verified in the app: untrusted hooks didn't run, after trusting the prompt hook added context and the PreToolUse hook blocked a Write.
- [x] 8. ✅ Tam — `subagent.rs`: `Task` tool for the built-in agent (general-purpose or read-only `explore`), fresh context with the session's system prompt + sub-agent note, same permissions (prompts go through the normal permission card), live tool log in the Task card, all `Task` calls of a response run in parallel (`join_all`), registry shown in the Tasks pane with stop, sub-agent tokens added to the response's cost (`subagent_usage`). Slash: `/review [focus]` and `/init` (prompts; forwarded to the CLI for the Claude Code engine), `/doctor` (API key + live connection, Claude Code CLI, git, shell, MCP servers, checkpoints size, project/repo/memory/hooks/engine), `/compact`, `/cost` (earlier items). Test: registry/usage. Verified two agents ran in parallel and reported back; fixed a bug found while testing (sub-agent tool calls were dropped by the sanitizer).
- [x] 9. ✅ Tam — Preview pane: embedded browser for localhost / 127.0.0.1 / [::1] (CSP allows only those), address bar (bare port ok), reload, open externally; local URLs printed by background commands are detected (`preview_urls`) and opened automatically; agent shells get `PYTHONUNBUFFERED=1`. Routines (`routines.rs`): prompts on a schedule (every N ≥ 5 min, or daily at HH:MM on chosen weekdays), each run a new code session in the routine's folder (engine, permission mode, optional worktree), scheduler every 30 s with one make-up run after downtime, manager on the Code page and `/schedule` (create/edit/enable/run now/open last run/delete). Tests: URL detection, interval/daily/weekday schedules, validation. Verified: dev server page shown in the pane; run-now and a scheduled run both created sessions.

## Faz 3 — Keşif / İnovasyon

Discovered while reviewing the app after Phase 2 (ordered by value / effort):

- [x] D1. ✅ Tam — Watch mode (`watch.rs`, `notify` crate): file events in the project (build/dependency folders ignored), 900 ms debounce, waits while Claude is mid-turn, no overlapping runs; check command auto-detected (cargo check / tsc / go build+vet / npm scripts / python compileall / make) or user-set; error-line extraction; topbar badge (Watch / Checking… / Check passed / N errors); panel with command, output, Run now, Fix with Claude, auto-fix after Claude's turns (max 2 in a row); resumes after restart; `/watch`. Tests: detection, error lines, ignored paths, real check runs. Verified: a broken file turned the badge red, Fix with Claude sent the errors, fixing the file turned it green.
- [x] D2. ✅ Tam — Changes pane "Commit" (git sessions, incl. worktree branches): Claude (title model) writes the message from the diff + recent log style (fallback when no API key), editable; optional push (sets upstream when missing). Verified the commit landed with the suggested message.
- [x] D3. ✅ Tam — `notify_attention`: when a permission prompt (built-in agent, Claude Code CLI or a sub-agent) or a plan approval appears while the window isn't focused, a desktop notification names the session and the tool/command (respects the notifications setting). Not visible under Xvfb (no notification daemon); same code path as the existing completion notifications.
- [x] D4. ✅ Tam — Ctrl+K palette: `>` lists actions (new chat/code session, pages, settings tabs, import, routines, theme toggle, side panel + each pane, every slash command of the current session incl. custom ones), `@` searches the session's files and inserts a mention, plain text mixes the best actions with chat/project search; ranked by substring/word-start score. Also fixed: the artifact panel and the side panel now close each other (with both open the chat column was squeezed to a few pixels), and the chat column keeps a minimum width.
- [x] D5. ✅ Tam — `archmap.rs`: file-level import graph for Rust (`mod`, `use crate::`, `crate::x::` paths, per crate root), JS/TS (`import`/`export from`/`require`/dynamic import with extension + index resolution), Python (absolute and relative `from`/`import`), Go (module-internal packages); up to 600 most-connected files. Map pane: force-directed SVG layout grouped/coloured by top folder, auto-fit, zoom/pan, filter, hover/click shows imports & importers, double-click mentions the file in the prompt. Test: multi-language fixture graph. Verified on this repository (86 files, 426 imports).
- [x] D6. ✅ Tam — "Save as /command" on your messages in code sessions: name (a:b → sub-folder), description, editable prompt with `$ARGUMENTS`/`$1…` help, scope project (`.claude/commands`) or all projects (`~/.claude/commands`); overwrite confirmation; the command appears in the `/` menu and the palette right away (Claude Code-compatible files). Test: save + expand + name validation. Verified in the app.
- [x] D7. ✅ Tam — `symbols.rs`: definitions index (Rust fn/struct/enum/trait/type/const, JS/TS function/class/arrow const/interface/type, Python def/class, Go func/type), cached 20 s per folder; the `@` popup lists matching symbols under the files; picking one inserts `@path#L<line>` and sending includes lines line-10…line+80 of that file. Tests: scanning per language, ranking, `#L` expansion. Verified: `@build_mess` → `build_messages fn src-tauri/src/engine.rs:357`.

---

## Log
- F1+F2 done. Also added a debug-only UI automation hook (`devtools.rs`, `CLAUDERUST_EVAL_DIR`) used to drive the app in tests; clippy is clean.
- F3+F7 done: verified typing into the PTY (`echo`, `ls`) and stopping a dev server from the Tasks pane.
- F4 done. Note for later work: the UI is embedded in the binary, so UI changes need a `cargo build` (touch `src-tauri/build.rs`).
- Phase 2 complete (items 1-9).
- CI note: since commit 5b72e07 GitHub Actions jobs are not starting ("recent account payments have failed or your spending limit needs to be increased"). This is an account billing setting, not a code failure; all checks pass locally on Linux (cargo test, clippy, node --check). Windows/macOS-only code added tonight was reviewed by hand (no cross C toolchain here for sqlite/ring).
