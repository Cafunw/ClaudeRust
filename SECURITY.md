# Security policy

ClaudeRust runs commands and edits files on your computer (in code sessions,
after the permission checks you choose) and installs its own updates, so
security reports are very welcome.

## Reporting a vulnerability

Please report it privately through GitHub: **Security → Report a
vulnerability** on this repository (private vulnerability reporting). Don't
open a public issue for it. Include what you found, how to reproduce it, and
the version / platform you used. You'll get an answer as soon as possible,
and credit in the release notes if you'd like.

## Supported versions

Only the latest release gets fixes. The app updates itself from GitHub
Releases (Settings → About → Updates); every update is signed, and the app
refuses one whose signature doesn't match the key built into it.

## Scope notes

- API keys are kept in the macOS Keychain / Windows Credential Manager; on
  Linux in the app's local database, readable by your user account.
- MCP servers, hooks and project commands run code you or a project chose to
  configure; project hooks run only after you approve them.
- Artifacts run in a separate, sandboxed `artifact://` origin.
