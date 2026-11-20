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
- [ ] 2. Checkpoints & rewind (shadow-git snapshots per turn; rewind code + conversation).
- [ ] 3. Inline review comments in the diff viewer → batched revision prompt.
- [ ] 4. Plan pane + Tasks pane (background shells, sub-agents, stop buttons).
- [ ] 5. Rules files (CLAUDE.md, AGENTS.md) + persistent memory (`#` quick memory, memory editor).
- [ ] 6. Auto-compaction + `/compact` + live token/cost meter.
- [ ] 7. Hooks (PreToolUse / PostToolUse / UserPromptSubmit / Stop, Claude Code settings format).
- [ ] 8. Sub-agents (`Task` tool, parallel) + `/review` `/compact` `/init` `/doctor` `/cost`.
- [ ] 9. Live preview browser panel + Routines (scheduled local tasks).

## Faz 3 — Keşif / İnovasyon

(Filled in as features are discovered.)

---

## Log
- F1+F2 done. Also added a debug-only UI automation hook (`devtools.rs`, `CLAUDERUST_EVAL_DIR`) used to drive the app in tests; clippy is clean.
- F3+F7 done: verified typing into the PTY (`echo`, `ls`) and stopping a dev server from the Tasks pane.
- F4 done. Note for later work: the UI is embedded in the binary, so UI changes need a `cargo build` (touch `src-tauri/build.rs`).
