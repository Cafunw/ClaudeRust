# Changelog

The release notes of each version; the app shows them when it offers an
update. Newest first.

## [0.1.2] - 2026-11-22

Sign-in for remote MCP servers, tools in chats on your Claude account,
Claude's questions in the Code tab, and fewer leftovers after a crash.

**MCP**
- Remote MCP servers that need a sign-in show *Sign in* in Settings →
  Developer: you sign in in your browser (OAuth) and the server connects.
  The sign-in is kept in the system's credential store on Windows and macOS;
  on Linux it lasts until the app quits.
- A local (stdio) MCP server that stops is started again by itself, up to
  five times in a row with a growing wait, and a server's changed tool list
  shows without a Reload.
- MCP tool calls show the server and the tool's name.

**Chats on your Claude account**
- They can use the MCP servers you enabled and web search. MCP tools follow
  their Allow / Ask / Block setting, and web fetch asks first. Searching
  your past chats is not available in them yet.
- They get a generated title, like chats that use an API key.

**Code**
- Claude's questions (AskUserQuestion) show as a card in the Code tab: pick
  an option or type your own answer. Before, they came up as a permission
  request that could not be answered.
- A turn in a resumed Claude Code session shows its own cost, no longer the
  session's running total.

**Stability**
- Windows: the Claude Code CLI, MCP servers, terminals and shells now end
  with the app, also when it crashes or is killed. On macOS and Linux they
  still end only when the app quits normally.
- An answer is saved while it streams in, so a crash keeps what had
  arrived; it shows as stopped on the next start.

**Faster**
- Streaming sends only the new text to the window, not the whole answer
  again on every update (a 5,000-character answer: 2.29 MB down to 0.21 MB).

**Links**
- `clauderust://` links open a conversation (`clauderust://chat/<id>`), a
  new chat or code session with a prompt filled in
  (`clauderust://new?prompt=...`, `clauderust://code?prompt=...`) or
  Settings. The prompt only waits in the message box: nothing is sent by
  itself.

**Artifacts**
- React artifacts can import 16 common shadcn/ui components from
  `@/components/ui/...` (button, card, dialog, tabs, select and others) and
  `cn` from `@/lib/utils`: ClaudeRust's stand-ins with the same names and
  props, in the app's colours.
- `window.storage` works in artifacts; the app keeps what they store (about
  2 million characters for all artifacts together). `window.claude.complete`
  is not available.

**Security**
- The window can only have the system open the app's own data folders and
  the MCP config file, or show files in those and in your sessions'
  folders.

**Interface**
- The terminal's Ask Claude bar sits above the terminal instead of covering
  its output.
- The "N files edited" summary counts only the lines that changed and shows
  paths relative to the project, also on Windows.
- Costs are written the same way everywhere; a narrow top bar keeps the
  engine's icon and the context meter (details in their tooltips); in a
  narrow window the open sidebar dims the page and a click beside it closes
  it.
- Model descriptions and the common error messages (no API key, rejected
  key, rate limit, overload, network) are translated into Turkish.

## [0.1.1] - 2026-11-22

Fixes for security, stability, start-up time and rendering.

**Security**
- Windows: PowerShell syntax could hide any command inside one that looked
  like a safe, read-only command, so it ran without asking, even in Plan
  mode. Such commands now ask first.
- Read-only commands no longer run without asking when they can write files
  or run programs (some options of `sort`, `uniq`, `tree`, `date`, `file`
  and `rg`), reach outside the project (globs, braces, network paths), or
  name a file in a `.git` or `.claude` folder.
- Reverting a file in the Changes panel can no longer delete or change a
  file outside the repository.

**Code**
- The built-in agent's Edit and Write keep a file's Windows (CRLF) line
  endings and its encoding: multi-line edits in CRLF files now match, and
  UTF-8 files with a byte-order mark and UTF-16 files (what PowerShell often
  writes) can be edited.

**Stability**
- A log file, `logs\clauderust.log` in the app's data folder (on Windows
  `%APPDATA%\dev.clauderust.desktop`), records warnings, errors and crashes
  with where they happened. It is kept under 2 MB plus the previous file,
  and your home folder is written as `~`, so it can be attached to a bug
  report.
- The database now carries a schema version, so new versions upgrade it
  step by step; a database written by a newer version is left untouched
  instead of being misread.
- `cargo test` passes on Windows: six tests no longer depend on your own
  `~/.claude` settings or on the Microsoft Store `python3` stub. Building
  from source needs Rust 1.91 (the declared minimum was wrong).

**Faster**
- Code blocks render with far smaller HTML, about 2.5 to 3.4 times the size
  of the code instead of 17 to 34 times, with the same colours.
- Start-up: the check for answers cut off by a quit no longer reads every
  message (73 ms down to 11 ms with 48,000 messages).
- Ctrl+R prompt history no longer searches all messages again on every
  keystroke.

**Interface**
- Esc stops a running answer only from the conversation and the message
  box, no longer from a side panel's fields or when it closes the quote bar.
  With dialogs stacked, Esc closes only the top one, and a closed dialog
  gives the focus back to what opened it.
- F5, Ctrl+R and the right-click page menu (Refresh, Print) no longer
  reload the window, which lost queued messages and open terminals, and
  Ctrl+P no longer prints the app (pressed inside an artifact or preview
  frame, these keys can still reload it). After a reload, a conversation's
  terminal reconnects to its running shell.
- Moving on quickly no longer lets a slow Project or Sessions page replace
  the page you moved to.
- Translated text fills in every placeholder, which fixes the prompt that
  History's Explain sends to Claude.

## [0.1.0] - 2026-11-21

The first public release of ClaudeRust, an unofficial, lightweight desktop
client for the Claude API, written in Rust.

**Chat**
- Streaming answers with Markdown, tables, LaTeX and highlighted code; edit,
  retry and branch any message; fork a conversation from any answer.
- Artifacts (HTML, React, SVG, Mermaid, Markdown, code) in a sandboxed panel,
  with versions.
- Projects with instructions and files, styles, web search, extended
  thinking, conversation instructions, search across conversations.
- MCP servers (`claude_desktop_config.json` format, stdio and remote) and
  desktop extensions (`.mcpb` / `.dxt`).
- Quick Entry, tray, notifications, light / dark theme, English and Turkish.

**Code**
- Two engines: the official Claude Code CLI (signs in with your Claude
  subscription) or a built-in Rust agent (API key).
- Effort levels for the Claude Code engine, and Ultracode (standing
  multi-agent workflows) on the models that support it: a switch in the model
  menu, `/effort` and `/ultracode`. A workflow's report is followed to the end
  instead of cut at the first answer; workflows show in the Tasks pane (phases,
  agents, cost; stop one), and you can keep talking while one runs. The effort
  menu shows the level the CLI really runs at.
- As in Claude Code: Stop ends the turn and leaves running workflows alone,
  messages queued meanwhile go in once the turn is done, the `ultracode`
  keyword opts a message into a workflow, Ultracode set on in Claude Code's
  settings shows and can be turned off, `/effort current` and `/workflows`.
  Each message shows its own cost, and `/cost` and the cost meter include
  what workflows and sub-agents spent.
- Side panel with Changes (diffs, revert a file or a hunk, review comments,
  commit with a suggested message and a secret scan, pull requests),
  History, Terminal, Plan, Tasks, Preview, Map and Files (tree, search,
  blame).
- Checkpoints and rewind, git worktrees for parallel sessions, permission
  modes (including Auto), hooks, sub-agents, compaction, watch mode,
  routines, `/` commands, `@` file and symbol mentions.

**App**
- Sign in with your Claude account (Chat and Code, through the official
  `claude` CLI; ClaudeRust never touches the token) and an interface that
  follows claude.ai's current look.
- Import from Claude: claude.ai data exports, Claude Code sessions, Claude
  Desktop MCP servers and extensions.
- Automatic updates from GitHub Releases, verified by signature.
- Works over Remote Desktop and in VMs (software rendering when needed).
