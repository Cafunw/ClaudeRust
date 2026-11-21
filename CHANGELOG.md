# Changelog

The release notes of each version; the app shows them when it offers an
update. Newest first.

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
