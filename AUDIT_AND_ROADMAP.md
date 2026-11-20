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
- [ ] F4. Git & diff engine: session change detection (`git status`/snapshot), per-file diff, revert file / revert all.
- [ ] F5. Permission layer: Auto mode (safe actions auto-approved, risky ones ask) in both engines; plan mode also gates MCP tools; tests.
- [ ] F6. Composer: `@` file mention picker (fuzzy, .gitignore-aware) and `/` slash command menu (built-ins + `.claude/commands/*.md` + MCP prompts).
- [x] F7. ✅ Tam — `workspace.js`: resizable side panel for code sessions with a pane registry (per-conversation pane DOM kept alive), width/open/tab persisted; Tasks pane lists background shells with live output, elapsed time and stop buttons (badge = running count).

## Faz 2 — IMPLEMENT

- [ ] 1. Git worktree isolation per code session (branch `clauderust/<slug>`), cleanup on delete.
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
