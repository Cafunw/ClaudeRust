# Changelog

The release notes of each version; the app shows them when it offers an
update. Newest first.

## [0.1.13] - 2026-11-26

Smoother streaming, cards for web citations, theme changes without a
reload, a filter for skills, syncing GitHub files in a project, and
fixes for the keyboard and screen readers.

**Improved**
- Streamed replies appear smoothly instead of in bursts. With reduced
  motion or forced colours, text shows as it arrives, as before.
- Web citations show the cited passage in a card when you hover over
  them, focus them with the keyboard or tap them.
- Mermaid diagrams, MCP App frames and Doc, Sheet and Deck artifacts
  follow a theme change without reloading, and an artifact keeps the
  edits you haven't saved.
- The tools menu has *Add connectors…* and *Manage connectors*, and
  Settings → Developer is easier to scan.
- Settings → Skills has a filter, groups the skills by where they come
  from, and shows each description in two lines until you show more.
- GitHub files in a project can be synced to the latest commit of their
  branch. Changed files are replaced; files gone from the repository are
  kept and named.
- Escape in a search field with text in it clears the field first; the
  next Escape closes the dialog.
- Releases also get unsigned macOS (universal) and Linux installers.

**Fixed**
- Message actions that can't run while Claude responds, such as *Edit*
  and *Retry*, now look disabled and say why, with a *Stop* button.
- Keyboard focus no longer gets lost after tabbing through an artifact,
  and Tab no longer gets stuck in a Sheet.
- In Settings → Developer, Tab reaches the controls after a server's
  error details.
- MCP server card headers no longer overflow at narrow widths.
- Task-list checkboxes in replies and the New project fields have names
  for screen readers.
- Citation numbers no longer end up in copied text.
- The update check says that no signed update has been published yet
  instead of showing an error.
- Claude's "still responding" messages are shown in Turkish in the
  Turkish interface.

**Known limits**
- Some features of Claude Desktop can't be offered: Anthropic's own
  account connectors, publishing an artifact, and picking private Google
  Drive files (the app has no Google sign-in). MCP sampling is not
  supported.
- On your Claude account a reply doesn't go on after the app quits.
- On your Claude account `window.claude.complete` still starts a Claude
  Code process of its own for each call.
- If you send a new chat's first message before its early Claude Code
  process is up, the message starts its own; the early one then stays
  idle until another chat's process is kept or 90 seconds pass.
- Rebuilding a chat whose Claude Code transcript is gone, and finishing
  an artifact with *Continue* on your Claude account, were tried only
  with a stand-in for Claude Code.
- Opening a very long chat with many code blocks still has one short
  stutter.
- A very small component that isn't centred on its page still looks
  small in its gallery tile.
- The exported chat page's colours and the Turkish desktop notifications
  were checked by tests, not by hand.
- An unfinished artifact now ends where the next artifact starts, so an
  artifact whose own text contains an artifact's opening tag is split
  there.
- There is no *Design* artifact, decks are 16:9 only, and the version
  menu doesn't mark a version you edited as yours.
- After you edit an artifact in place, the next reply on your Claude
  account starts a new Claude Code session with the conversation sent
  again, so that reply doesn't use the prompt cache.
- In a doc saved as .docx, lists are typed bullets and numbers rather
  than Word lists, and links are styled text that can't be clicked.
- Printing decks and docs to PDF, the save dialogs of downloads, *Open*
  and *Download* in an attachment's preview, and the tray menu itself
  were not tried by hand, because they open the system's own windows.
- The window's own memory use while idle is unchanged. Giving memory back
  when the window is hidden or minimised, and ending the kept Claude Code
  process 30 seconds after that, were checked in the code and tests, not
  by hand.
- An MCP config file edited outside the app that isn't valid JSON is
  ignored without a notice until it is valid again; the servers keep
  running as they were.
- In a form an MCP server asks you to fill in, a choice of several
  numbers is refused; choices of text work.
- An MCP resource attached to a message that is never sent stays in
  memory until the app quits.
- Searching a large project on your Claude account was tried only with a
  stand-in for Claude Code, and so was `window.claude.complete` there.
- What you allow an artifact (a connector, or asking Claude) lasts until
  the app restarts or the page reloads; then the artifact asks again. An
  artifact can't be opened in its own window yet.
- With both an API key and the Claude account as the chat engine, titles
  are made with the API key first, so they use API credit.
- The Node.js download was tried only against a local test server, not
  against nodejs.org. Installing and updating from the extension
  directory were tried only against local test servers; the listed
  releases' addresses, sizes and SHA-256 come from GitHub's release API
  and were not downloaded. Update notices cover only extensions the
  directory lists.
- The GitHub and Google link sources, and syncing GitHub files in a
  project, were tried only against local test servers, and Google's
  export formats were not checked against Google. Files that aren't
  shared by link can't be read.
- GitHub files added to a project before this version can't be synced;
  add them from GitHub again to sync them.
- Drafting a style was tried with an API key against a stand-in, not on
  your Claude account.
- Some messages are still English in the Turkish interface: the engine's
  crash text, errors when continuing a reply, the success text when a
  worktree's changes are applied, the diagnostics' labels, and the error
  detail in the dialog shown when the database can't be opened.
- While a reply streams, the window still parses the reply's whole HTML
  again on every update: about 18 % of a core in a development build
  during a long reply, and the smooth reveal adds up to about 3 % more.
  A release build was not measured.
- While a reply streams, the background of a code block or table appears
  line by line.
- On your Claude account a continued reply's repeated start is not
  trimmed.
- An MCP App page can reach the hosts its server declares without asking
  you first. MCP Apps are shown inline only: no side panel or full
  screen, and an app can't update what Claude knows.
- A plain `npx tauri build` (or `npm run build`) still needs the updater
  signing key; use `npm run build:unsigned`.
- If GitHub answers the update check with a server error, the app also
  says that no signed update has been published yet; it checks again six
  hours later.
- Skills run only in Code sessions on the Claude Code engine; chats and
  the API-key engine don't use them yet.
- A PDF over the limits still has no text-only fallback.
- Mermaid diagrams and the artifact libraries are still loaded from public
  CDNs (at fixed versions), so they need the internet. Redrawing Mermaid
  diagrams on a theme change was tried only with a stand-in for Mermaid.
- The citation card and the keyboard stops around artifacts were not
  tried with a screen reader.
- The macOS and Linux installers are unsigned, and the app was not tried
  by hand on macOS or Linux. Keeping the computer awake on macOS and
  Linux, and keeping tokens in the config file on Linux, are untested on
  those systems.
- Fast mode, replies with thinking switched off, memory written through
  Claude Code, Research and code execution were tested against stand-ins
  and at the configuration level, not yet against live replies.

## [0.1.12] - 2026-11-25

Less memory on your Claude account, support for the newer MCP protocol,
artifact storage in the app's database, and fixes for the keyboard,
contrast and Settings.

**Improved**
- On your Claude account only one Claude Code process is kept: that of
  the chat in front. It ends after 90 seconds idle, or 30 seconds with the
  window hidden or minimised, so memory stays low. A new chat's process
  starts while you type its first message, so its first reply starts
  sooner.
- A reply on your Claude account is finished as soon as Claude Code
  reports it done.
- MCP servers on the newer protocol (2025-11-25) work. Settings and the
  permission card show the servers' and tools' icons, the tools' titles
  and descriptions, and whether a tool only reads or can destroy data.
- MCP resources keep their pictures, PDFs and files, with their own
  names, and resource templates can be filled in.
- An MCP server can ask you to open a link: it is shown with its site,
  with a warning for look-alike addresses, and opens only when you click
  *Open*.
- What artifacts store is kept in the app's database, up to 20 MB for
  each artifact; what they stored before is moved over once.
- The connector directory shows a distinct tile for each connector.
- The Import page shows its progress while it reads Claude Code sessions.
- The thinking effort you pick is remembered for the next new chat, and
  separately for the next Code session.
- A chat with very long replies opens on fewer of its last replies, so it
  opens sooner; the earlier ones come as you scroll up.
- In the Artifacts gallery, React and HTML thumbnails are drawn at about
  the panel's width, so a small component shows larger.

**Fixed**
- Escape closes the search palette and dialogs from any control in them.
- The Settings navigation works with the arrow keys, Home and End.
- The Quick Entry shortcut is set with the key recorder.
- Changing the Claude Code path updates its status at once, and chat
  errors about Claude Code link to the right Settings page.
- Switches and field borders are clearly visible in both themes.
- Code no longer turns `<=` or `=>` into symbols.
- Settings rows are aligned, and long navigation labels are no longer cut.
- In Doc artifacts, task checkboxes no longer look faded.
- `window.claude.complete` no longer stops after 100 calls in a
  conversation.
- A chat title that Claude Code is slow to write no longer makes the next
  reply start a new Claude Code process.
- A gallery tile scrolled away and back while it was being drawn no
  longer gets two thumbnails.

**Known limits**
- Some features of Claude Desktop can't be offered: Anthropic's own
  account connectors, publishing an artifact, and picking private Google
  Drive files (the app has no Google sign-in). MCP sampling is not
  supported.
- On your Claude account a reply doesn't go on after the app quits.
- On your Claude account `window.claude.complete` still starts a Claude
  Code process of its own for each call.
- If you send a new chat's first message before its early Claude Code
  process is up, the message starts its own; the early one then stays
  idle until another chat's process is kept or 90 seconds pass.
- Rebuilding a chat whose Claude Code transcript is gone, and finishing
  an artifact with *Continue* on your Claude account, were tried only
  with a stand-in for Claude Code.
- Opening a very long chat with many code blocks still has one short
  stutter.
- A very small component that isn't centred on its page still looks
  small in its gallery tile.
- The exported chat page's colours and the Turkish desktop notifications
  were checked by tests, not by hand.
- An unfinished artifact now ends where the next artifact starts, so an
  artifact whose own text contains an artifact's opening tag is split
  there.
- There is no *Design* artifact, decks are 16:9 only, and the version
  menu doesn't mark a version you edited as yours.
- After you edit an artifact in place, the next reply on your Claude
  account starts a new Claude Code session with the conversation sent
  again, so that reply doesn't use the prompt cache.
- In a doc saved as .docx, lists are typed bullets and numbers rather
  than Word lists, and links are styled text that can't be clicked.
- Printing decks and docs to PDF, the save dialogs of downloads, *Open*
  and *Download* in an attachment's preview, and the tray menu itself
  were not tried by hand, because they open the system's own windows.
- The window's own memory use while idle is unchanged. Giving memory back
  when the window is hidden or minimised, and ending the kept Claude Code
  process 30 seconds after that, were checked in the code and tests, not
  by hand.
- An MCP config file edited outside the app that isn't valid JSON is
  ignored without a notice until it is valid again; the servers keep
  running as they were.
- In a form an MCP server asks you to fill in, a choice of several
  numbers is refused; choices of text work.
- An MCP resource attached to a message that is never sent stays in
  memory until the app quits.
- Searching a large project on your Claude account was tried only with a
  stand-in for Claude Code, and so was `window.claude.complete` there.
- What you allow an artifact (a connector, or asking Claude) lasts until
  the app restarts or the page reloads; then the artifact asks again. An
  artifact can't be opened in its own window yet, and Tab pressed inside
  an artifact can still leave the narrow sheet.
- With both an API key and the Claude account as the chat engine, titles
  are made with the API key first, so they use API credit.
- The Node.js download was tried only against a local test server, not
  against nodejs.org. Installing and updating from the extension
  directory were tried only against local test servers; the listed
  releases' addresses, sizes and SHA-256 come from GitHub's release API
  and were not downloaded. Update notices cover only extensions the
  directory lists.
- The GitHub and Google link sources were tried only against local test
  servers, and Google's export formats were not checked against Google.
  Files that aren't shared by link can't be read.
- Drafting a style was tried with an API key against a stand-in, not on
  your Claude account.
- Some messages are still English in the Turkish interface: the engine's
  busy texts, its crash text, errors when continuing a reply, the success
  text when a worktree's changes are applied, the diagnostics' labels,
  and the error detail in the dialog shown when the database can't be
  opened.
- While a reply streams, the window still parses the reply's whole HTML
  again on every update: about 18 % of a core in a development build
  during a long reply. A release build was not measured.
- On your Claude account a continued reply's repeated start is not
  trimmed.
- An MCP App page can reach the hosts its server declares without asking
  you first. MCP Apps are shown inline only: no side panel or full
  screen, and an app can't update what Claude knows.
- A plain `npx tauri build` (or `npm run build`) still needs the updater
  signing key; use `npm run build:unsigned`.
- Skills run only in Code sessions on the Claude Code engine; chats and
  the API-key engine don't use them yet.
- A PDF over the limits still has no text-only fallback.
- Mermaid diagrams and the artifact libraries are still loaded from public
  CDNs (at fixed versions), so they need the internet.
- Keeping the computer awake on macOS and Linux, and keeping tokens in the
  config file on Linux, are untested on those systems.
- Fast mode, replies with thinking switched off, memory written through
  Claude Code, Research and code execution were tested against stand-ins
  and at the configuration level, not yet against live replies.

## [0.1.11] - 2026-11-25

Faster and more faithful replies on your Claude account, an immediate
*Stop*, readable code colours, and fixes for the keyboard, search, read
aloud and the Artifacts gallery. No new features.

**Improved**
- On your Claude account a chat keeps its Claude Code process running
  between messages, so the next reply starts sooner, and the chat's title
  is asked of the same process. It is kept for up to two chats, until it
  has been idle for five minutes, and not for Code sessions, incognito
  chats or chats with a spending limit.
- *Stop* ends a reply at once; what was written so far stays as it was.
- A chat whose Claude Code transcript is gone goes on with its whole
  history, pictures and PDFs included, instead of a text summary, and
  says so. If the history is too large, its oldest parts are left out
  and the notice says that too.
- Errors from Claude Code (servers overloaded, a server error, a lost
  connection, a conversation too long for the model) show the same
  translated message and *Retry*/*Continue* as with an API key.
- In the Artifacts gallery, React, Mermaid and HTML artifacts with scripts
  are drawn by their own code in a sandbox, so the thumbnail shows the
  real artifact; one whose page fails shows an error tile.
- Long chats open and scroll more smoothly: older replies are drawn a
  little at a time instead of all in one go.

**Fixed**
- Code colours are readable in both themes, also on an exported chat
  page, which now colours its code.
- In the Code tab's terminal, Alt+1 to Alt+8 go to the shell instead of
  switching panes.
- On first run the focus starts in the right field, and Enter takes the
  signed-in account's way when there is one.
- Task lists show only their checkbox, without a bullet, in chats and in
  Doc artifacts; in chats the checkbox is no longer faded.
- Search snippets no longer show Markdown marks, and the matched words
  are readable in the dark theme.
- Read aloud no longer reads a task's checkbox, and no longer drops the
  underscores or asterisks from names like `snake_case_name` or `2*3*4`.
- Desktop notifications are in Turkish when the app is in Turkish.

**Known limits**
- Some features of Claude Desktop can't be offered: Anthropic's own
  account connectors, publishing an artifact, and picking private Google
  Drive files (the app has no Google sign-in). MCP sampling is not
  supported.
- On your Claude account a reply doesn't go on after the app quits.
- If a chat's title takes Claude Code more than ten seconds, the next
  reply starts a new Claude Code process for that chat.
- Rebuilding a chat whose Claude Code transcript is gone, and finishing
  an artifact with *Continue* on your Claude account, were tried only
  with a stand-in for Claude Code.
- Opening a very long chat with many code blocks still has one short
  stutter.
- A React thumbnail is drawn at a full page's width, so a very small
  component is hard to see in its tile.
- In Doc artifacts, task checkboxes look faded.
- The exported chat page's colours and the Turkish desktop notifications
  were checked by tests, not by hand.
- An unfinished artifact now ends where the next artifact starts, so an
  artifact whose own text contains an artifact's opening tag is split
  there.
- There is no *Design* artifact, decks are 16:9 only, and the version
  menu doesn't mark a version you edited as yours.
- After you edit an artifact in place, the next reply on your Claude
  account starts a new Claude Code session with the conversation sent
  again, so that reply doesn't use the prompt cache.
- In a doc saved as .docx, lists are typed bullets and numbers rather
  than Word lists, and links are styled text that can't be clicked.
- Printing decks and docs to PDF, the save dialogs of downloads, *Open*
  and *Download* in an attachment's preview, and the tray menu itself
  were not tried by hand, because they open the system's own windows.
- The memory use of an idle window is unchanged. Giving memory back when
  the window is hidden or minimised was checked in the code and tests,
  not by hand.
- An MCP config file edited outside the app that isn't valid JSON is
  ignored without a notice until it is valid again; the servers keep
  running as they were.
- Searching a large project on your Claude account was tried only with a
  stand-in for Claude Code, and so was `window.claude.complete` there.
- What you allow an artifact (a connector, or asking Claude) lasts until
  the app restarts or the page reloads; then the artifact asks again. An
  artifact can't be opened in its own window yet, and Tab pressed inside
  an artifact can still leave the narrow sheet.
- With both an API key and the Claude account as the chat engine, titles
  are made with the API key first, so they use API credit.
- The Node.js download was tried only against a local test server, not
  against nodejs.org. Installing and updating from the extension
  directory were tried only against local test servers; the listed
  releases' addresses, sizes and SHA-256 come from GitHub's release API
  and were not downloaded. Update notices cover only extensions the
  directory lists.
- The GitHub and Google link sources were tried only against local test
  servers, and Google's export formats were not checked against Google.
  Files that aren't shared by link can't be read.
- Drafting a style was tried with an API key against a stand-in, not on
  your Claude account.
- Some messages are still English in the Turkish interface: the engine's
  busy texts, its crash text, errors when continuing a reply, the success
  text when a worktree's changes are applied, the diagnostics' labels,
  and the error detail in the dialog shown when the database can't be
  opened.
- While a reply streams, the window still parses the reply's whole HTML
  again on every update: about 18 % of a core in a development build
  during a long reply. A release build was not measured.
- On your Claude account a continued reply's repeated start is not
  trimmed.
- An MCP App page can reach the hosts its server declares without asking
  you first. MCP Apps are shown inline only: no side panel or full
  screen, and an app can't update what Claude knows.
- A plain `npx tauri build` (or `npm run build`) still needs the updater
  signing key; use `npm run build:unsigned`.
- Skills run only in Code sessions on the Claude Code engine; chats and
  the API-key engine don't use them yet.
- A PDF over the limits still has no text-only fallback.
- Mermaid diagrams and the artifact libraries are still loaded from public
  CDNs (at fixed versions), so they need the internet.
- Keeping the computer awake on macOS and Linux, and keeping tokens in the
  config file on Linux, are untested on those systems.
- Fast mode, replies with thinking switched off, memory written through
  Claude Code, Research and code execution were tested against stand-ins
  and at the configuration level, not yet against live replies.

## [0.1.10] - 2026-11-25

Fixes for artifacts that were cut off, Settings, import from Claude
Desktop, the Code tab, the keyboard and memory use. No new features.

**Fixed**
- An artifact that a reply finished after *Continue* now appears in the
  Artifacts page, both with an API key and on your Claude account.
- An artifact cut off by *Stop*, an error or closing the app no longer
  says *Generating…* forever: its card and its version menu say
  *Interrupted*, also after the window reloads.
- An unfinished artifact opens in its normal preview (a document
  rendered, not its source), with a note that it was cut off and a
  *Preview*/*Code* switch.
- A Settings page no longer shows the results of another page that was
  still loading, such as Import's scan on the Developer page. The skills
  list shows the folder you picked last.
- In the dark theme the chosen option of a segmented control (the
  Developer page's tool policy, for example) is visible again; in both
  themes it has a clear outline.
- Import from Claude Desktop finds the MCP servers and extensions of the
  current Claude Desktop (the Microsoft Store version), and no longer
  offers ClaudeRust's own chat transcripts as Claude Code sessions to
  import.
- Editing the MCP config file outside the app reloads its servers, with
  a notice. Saving it from the app reloads them without one.
- Code tab: a renamed session shows its new title in the header, its
  tooltip and the window title; the header keeps the title whole and
  shortens the folder and branch first; the home page's mark and the
  worktree card no longer overlap or cut their text.
- Tab and Shift+Tab move through the window in the order it is drawn,
  come round from the last control to the first, and never stop on the
  page itself. When a control is drawn again (a title arriving, a list
  redrawn), the keyboard focus stays on it.

**Improved**
- Fewer background requests per reply: about half as many as before.
- The app gives memory back when you close a long chat and when its
  window is hidden or minimised, and a chat you closed no longer stays in
  memory.
- The Todos filters and the Usage page's day range tell a screen reader
  which option is chosen, and the model button's extended-thinking mark
  is read out; the terminal's input is named in the app's language;
  screen-reader announcements are cleared once read and when the
  language changes.
- In a narrow window the Sessions page keeps its head on one row (Routines
  and Import show only their icon).

**Known limits**
- Some features of Claude Desktop can't be offered: Anthropic's own
  account connectors, publishing an artifact, and picking private Google
  Drive files (the app has no Google sign-in). MCP sampling is not
  supported.
- On your Claude account each reply starts Claude Code again (it isn't
  kept running for the chat), and a reply doesn't go on after the app
  quits.
- Finishing an artifact with *Continue* on your Claude account was tried
  only with a stand-in for Claude Code.
- An unfinished artifact now ends where the next artifact starts, so an
  artifact whose own text contains an artifact's opening tag is split
  there.
- There is no *Design* artifact, decks are 16:9 only, and the version
  menu doesn't mark a version you edited as yours.
- After you edit an artifact in place, the next reply on your Claude
  account starts a new Claude Code session with the conversation sent
  again, so that reply doesn't use the prompt cache.
- In a doc saved as .docx, lists are typed bullets and numbers rather
  than Word lists, and links are styled text that can't be clicked.
- Printing decks and docs to PDF, the save dialogs of downloads, *Open*
  and *Download* in an attachment's preview, and the tray menu itself
  were not tried by hand, because they open the system's own windows.
- The memory use of an idle window is unchanged. Giving memory back when
  the window is hidden or minimised was checked in the code and tests,
  not by hand.
- An MCP config file edited outside the app that isn't valid JSON is
  ignored without a notice until it is valid again; the servers keep
  running as they were.
- Searching a large project on your Claude account was tried only with a
  stand-in for Claude Code, and so was `window.claude.complete` there.
- What you allow an artifact (a connector, or asking Claude) lasts until
  the app restarts or the page reloads; then the artifact asks again. An
  artifact can't be opened in its own window yet, and Tab pressed inside
  an artifact can still leave the narrow sheet.
- With both an API key and the Claude account as the chat engine, titles
  are made with the API key first, so they use API credit.
- The Node.js download was tried only against a local test server, not
  against nodejs.org. Installing and updating from the extension
  directory were tried only against local test servers; the listed
  releases' addresses, sizes and SHA-256 come from GitHub's release API
  and were not downloaded. Update notices cover only extensions the
  directory lists.
- The GitHub and Google link sources were tried only against local test
  servers, and Google's export formats were not checked against Google.
  Files that aren't shared by link can't be read.
- Drafting a style was tried with an API key against a stand-in, not on
  your Claude account.
- Some messages are still English in the Turkish interface: the engine's
  busy texts, its crash text, errors when continuing a reply, its
  notifications, the success text when a worktree's changes are applied,
  the diagnostics' labels, and the error detail in the dialog shown when
  the database can't be opened.
- While a reply streams, the window still parses the reply's whole HTML
  again on every update: about 18 % of a core in a development build
  during a long reply. A release build was not measured.
- On your Claude account a continued reply's repeated start is not
  trimmed.
- An MCP App page can reach the hosts its server declares without asking
  you first. MCP Apps are shown inline only: no side panel or full
  screen, and an app can't update what Claude knows.
- A plain `npx tauri build` (or `npm run build`) still needs the updater
  signing key; use `npm run build:unsigned`.
- Skills run only in Code sessions on the Claude Code engine; chats and
  the API-key engine don't use them yet.
- A PDF over the limits still has no text-only fallback.
- Mermaid diagrams and the artifact libraries are still loaded from public
  CDNs (at fixed versions), so they need the internet.
- Keeping the computer awake on macOS and Linux, and keeping tokens in the
  config file on Linux, are untested on those systems.
- Fast mode, replies with thinking switched off, memory written through
  Claude Code, Research and code execution were tested against stand-ins
  and at the configuration level, not yet against live replies.

## [0.1.9] - 2026-11-24

Slides, docs, sheets and dashboards as artifacts, artifacts that read the
chat's files, previews of attachments, generated titles and project
search on your Claude account, tools loaded when needed, and the app's
own menus and dialogs in your language.

**Artifacts**
- Four new kinds of artifact. *Slides*: a deck you page through, with
  click steps and speaker notes, presented full screen (Esc returns) and
  printed to PDF. *Docs*: a formatted document. *Sheets*: a spreadsheet
  with formulas that sorts by column. *Dashboard*: KPI tiles, charts and
  tables drawn from Claude's data.
- Slides, docs and sheets are edited in place: change the words, notes or
  cells and Save, and every save is a new version in the version menu.
  Claude sees your edited version in the next reply.
- Each kind downloads in its own formats: a deck as PDF or a presentation
  (.html), a doc as Word (.docx), PDF, Markdown or a web page, a sheet as
  CSV, a dashboard as JSON.
- The Artifacts page shows what each artifact looks like instead of its
  source: pages and decks drawn with nothing in them running, a sheet's
  first cells, a dashboard's figures and chart, a doc's start. React
  components and diagrams show their title and first heading.
- Artifacts load only the libraries the app lists, each at its one
  version. Anything else is blocked, and the artifact says which address
  it tried.
- Artifacts can read the files attached to their chat
  (`window.fs.readFile`), by name, as text or bytes. Files of other chats
  and files on the disk can't be read.

**Attachments**
- A click on an attachment, in the composer or in a sent message, opens a
  preview: a PDF in the built-in viewer, an image at full size, text and
  code highlighted, and Word, Excel, PowerPoint, OpenDocument, EPUB and RTF
  files as the text Claude reads. The preview has *Open* (in the system's
  app) and *Download*; sent attachments also open with the keyboard.
- OpenDocument texts (.odt), EPUB books and RTF files can be attached;
  Claude gets their text.

**Chats**
- Chats on your Claude account get a generated title. Claude Code makes it
  with a small model, which counts toward your plan's usage limits.
- On your Claude account, a project too large to send whole is searched,
  as with an API key: Claude gets its index and a search tool.
- On your Claude account, an image an MCP tool returns shows as the image,
  without Claude Code's note about where it stored it.
- *Tool access* in a chat's *Search and tools* menu: *Load tools when
  needed* sends only each MCP tool's name, and Claude loads the ones it
  needs, so they take less of the context; *Load all tools up front*
  sends them all. A chat with more than 20 MCP tools loads them when
  needed unless you choose otherwise. A loaded tool still asks for your
  permission as before.
- A *Learning* style: Claude acts as a tutor, guiding you step by step
  with questions and hints, and gives the full answer when you ask.
- The arrows between a message's versions are always shown, not only on
  hover.
- Searching chats finds those that hold every word you type, in any
  order, ignoring case and accents; I, İ and ı match each other.

**Extensions**
- Installed extensions from the directory are checked for a newer version
  after the app starts and then once a day, and a notice says how many
  have one.
- *Update extensions automatically* (Settings → Developer, off by
  default) installs those updates by itself, but only for bundles signed
  by a trusted publisher that ask for no new settings. The others you
  update from their card, with the preview as before.

**Language**
- The tray menu, Quick Entry's title, the dialog shown when the database
  can't be opened and the file types in the system's file pickers follow
  the app's language, and the tray menu changes as soon as you change it.
- Turkish wording fixes: much more of the interface addresses you
  formally, and sentences such as the project capacity and the permission
  card's server read naturally.

**Development**
- Test copies of the app open in the background, off the screen, and
  never take the focus from the window you are using.
- End-to-end tests now also cover chats on the Claude account, against a
  stand-in for Claude Code: streaming, the generated title, an artifact
  and a permission card.
- The CI end-to-end job ran on a GitHub runner for 0.1.8 and passed, so
  0.1.8's note that it had not run yet is out of date.

**Known limits**
- Some features of Claude Desktop can't be offered: Anthropic's own
  account connectors, publishing an artifact, and picking private Google
  Drive files (the app has no Google sign-in). MCP sampling is not
  supported.
- On your Claude account each reply starts Claude Code again (it isn't
  kept running for the chat), and a reply doesn't go on after the app
  quits.
- There is no *Design* artifact, decks are 16:9 only, and the version
  menu doesn't mark a version you edited as yours.
- After you edit an artifact in place, the next reply on your Claude
  account starts a new Claude Code session with the conversation sent
  again, so that reply doesn't use the prompt cache.
- In a doc saved as .docx, lists are typed bullets and numbers rather
  than Word lists, and links are styled text that can't be clicked.
- Printing decks and docs to PDF, the save dialogs of downloads, *Open*
  and *Download* in an attachment's preview, and the tray menu itself
  were not tried by hand, because they open the system's own windows.
- Searching a large project on your Claude account was tried only with a
  stand-in for Claude Code, and so was `window.claude.complete` there.
- What you allow an artifact (a connector, or asking Claude) lasts until
  the app restarts or the page reloads; then the artifact asks again. An
  artifact can't be opened in its own window yet, and Tab pressed inside
  an artifact can still leave the narrow sheet.
- With both an API key and the Claude account as the chat engine, titles
  are made with the API key first, so they use API credit.
- The Node.js download was tried only against a local test server, not
  against nodejs.org. Installing and updating from the extension
  directory were tried only against local test servers; the listed
  releases' addresses, sizes and SHA-256 come from GitHub's release API
  and were not downloaded. Update notices cover only extensions the
  directory lists.
- The GitHub and Google link sources were tried only against local test
  servers, and Google's export formats were not checked against Google.
  Files that aren't shared by link can't be read.
- Drafting a style was tried with an API key against a stand-in, not on
  your Claude account.
- Some messages are still English in the Turkish interface: the engine's
  busy texts, its crash text, errors when continuing a reply, its
  notifications, the success text when a worktree's changes are applied,
  the diagnostics' labels, and the error detail in the dialog shown when
  the database can't be opened.
- While a reply streams, the window still parses the reply's whole HTML
  again on every update: about 18 % of a core in a development build
  during a long reply. A release build was not measured.
- On your Claude account a continued reply's repeated start is not
  trimmed.
- An MCP App page can reach the hosts its server declares without asking
  you first. MCP Apps are shown inline only: no side panel or full
  screen, and an app can't update what Claude knows.
- A plain `npx tauri build` (or `npm run build`) still needs the updater
  signing key; use `npm run build:unsigned`.
- Skills run only in Code sessions on the Claude Code engine; chats and
  the API-key engine don't use them yet.
- A PDF over the limits still has no text-only fallback.
- Mermaid diagrams and the artifact libraries are still loaded from public
  CDNs (at fixed versions), so they need the internet.
- Keeping the computer awake on macOS and Linux, and keeping tokens in the
  config file on Linux, are untested on those systems.
- Fast mode, replies with thinking switched off, memory written through
  Claude Code, Research and code execution were tested against stand-ins
  and at the configuration level, not yet against live replies.

## [0.1.8] - 2026-11-24

Artifacts, thinking summaries and web sources in chats on your Claude
account, artifacts that can use your connectors, a switch for each tool,
larger extension and connector directories, and a sweep of the interface.

**Chats**
- Chats on your Claude account save their artifacts: the Artifacts page
  lists them, and deleting the chat removes them.
- On your Claude account, thinking shows its summary instead of an empty
  block. A Claude Code too old to send summaries runs without them.
- A finished thinking block says how long it took ("Thought for 12s"),
  with an API key and on your Claude account. Thinking that came without
  any text can't be opened.
- On your Claude account a web search shows as a "Searched the web" step,
  and the answer gets citation chips and a list of sources.
- Generated chat titles no longer start with a "Title:" label.
- In a chat's *Search and tools* menu, each MCP server's row opens its
  tools, with a switch for each. A tool switched off isn't offered to
  Claude in that chat, and the row says how many are off.

**Artifacts**
- Artifacts that ask Claude questions (`window.claude.complete`) also work
  on your Claude account. Each question runs on a small model with no
  tools and counts toward your plan's usage limits; the dialog that asks
  your permission says so.
- Artifacts can call the tools of your MCP connectors
  (`window.claude.callMcpTool`). The first call to a connector asks
  whether this artifact may use it: Deny, Allow once or Allow for this
  artifact. Tools blocked in your connector settings stay blocked, tools
  set to Ask still ask at every call, and a connector or tool switched off
  in the chat is refused. The plug button in the artifact's toolbar shows
  *Tools this artifact can use*, where each permission can be revoked.
- An artifact can be shown full screen (Esc leaves it), and a *Reload*
  button starts it again.

**Directories**
- *Browse extensions* lists 50 releases (was 16) in 7 categories, adding
  *Design and engineering*. Each is a stable release whose bundle carries
  its own code; two that fetched their server from npm or PyPI when they
  started were removed.
- *Browse connectors* lists 78 connectors (was 48) in 13 categories,
  adding *Automation* and *Audio and video*.

**Interface**
- The icons of the folded sidebar have names and tooltips with their
  shortcuts, and its user card says who you are and which plan you use.
- The find bar (Ctrl+F) no longer covers the match it shows.
- Below 900 px the Code side panel opens as a sheet over a dimmed chat,
  with *Back to chat*. Tab stays inside it, and inside the artifact sheet
  and the narrow sidebar.
- The usage-limit banner reads as a sentence ("You're close to your
  weekly limit for all models.") and can be dismissed; it comes back at a
  new level, for another limit or in a new window.
- Changing the language in Settings redraws Settings in the new language
  at once, keeping your place. "Auto (system)" is named in the interface
  language.
- Alt+1 to Alt+8 open the Code side panel's tabs in the order they are
  shown, also on macOS, and the tabs' tooltips and the shortcut sheet say
  which key opens which.
- A reply that failed has no Good/Bad buttons, and an error that names an
  install site (claude.com/claude-code, nodejs.org, python.org, uv's docs)
  links to it.
- Sidebar rows and shortcut hints meet WCAG AA contrast in both themes, at
  rest, on hover and when selected.
- Smaller fixes: tooltips on the Send/Stop button (with its key), the
  pages' search buttons and the dialogs' close buttons; tooltips and menus
  show your own shortcuts instead of fixed ones; counts of one read "1
  tool", "Added 1 server."; the search palette offers @ for files only in
  Code sessions; the Code top bar no longer overflows at 420 px; the
  composer no longer takes the focus from an open dialog; screen readers
  hear the current page and chat, the onboarding fields' labels and the
  names of Settings' icon-only tabs.

**Development**
- The end-to-end test copy gets its WebView2 debugging port through its
  window configuration instead of an environment variable, which the
  WebView2 of a GitHub runner ignored. A normal build has no debugging
  port. The CI end-to-end job counts toward the result again.

**Known limits**
- Some features of Claude Desktop can't be offered: Anthropic's own
  account connectors, publishing an artifact, and picking private Google
  Drive files (the app has no Google sign-in). MCP sampling is not
  supported.
- The CI end-to-end job had not yet run on a GitHub runner with the new
  port delivery when this was written. The same 15 tests pass on a
  development machine.
- `window.claude.complete` on your Claude account was tried only with a
  stand-in for Claude Code, not against the account.
- What you allow an artifact (a connector, or asking Claude) lasts until
  the app restarts or the page reloads; then the artifact asks again. An
  artifact can't be opened in its own window yet, and Tab pressed inside
  an artifact can still leave the narrow sheet.
- With both an API key and the Claude account as the chat engine, titles
  are made with the API key first, so they use API credit.
- The Node.js download was tried only against a local test server, not
  against nodejs.org. Installing from the extension directory was tried
  only against local test servers; the listed releases' addresses, sizes
  and SHA-256 come from GitHub's release API and were not downloaded.
- The GitHub and Google link sources were tried only against local test
  servers, and Google's export formats were not checked against Google.
  Files that aren't shared by link can't be read.
- Drafting a style was tried with an API key against a stand-in, not on
  your Claude account.
- Some messages from the engine are still English in the Turkish
  interface: the engine-busy texts, its crash text, errors when
  continuing a reply, its notifications, the success text when a
  worktree's changes are applied, and the diagnostics' labels.
- Update notices cover only extensions the directory lists.
- While a reply streams, the window still parses the reply's whole HTML
  again on every update: about 18 % of a core in a development build
  during a long reply. A release build was not measured.
- On your Claude account a continued reply's repeated start is not
  trimmed.
- An MCP App page can reach the hosts its server declares without asking
  you first. MCP Apps are shown inline only: no side panel or full
  screen, and an app can't update what Claude knows.
- A plain `npx tauri build` (or `npm run build`) still needs the updater
  signing key; use `npm run build:unsigned`.
- Skills run only in Code sessions on the Claude Code engine; chats and
  the API-key engine don't use them yet.
- A PDF over the limits still has no text-only fallback.
- Mermaid diagrams and the artifact libraries are still loaded from public
  CDNs, so they need the internet.
- Keeping the computer awake on macOS and Linux, and keeping tokens in the
  config file on Linux, are untested on those systems.
- Fast mode, replies with thinking switched off, memory written through
  Claude Code, Research and code execution were tested against stand-ins
  and at the configuration level, not yet against live replies.

## [0.1.7] - 2026-11-22

A Node.js for desktop extensions, an extension directory and signature
checks, files from GitHub and from Google links, styles drafted from
your writing, error messages in your language, and a sweep of the
interface.

**Extensions**
- Desktop extensions that start with `node` can use a Node.js the app
  downloads: when an extension needs Node.js and the computer has none,
  the app offers it (version 26.13.1 from nodejs.org, about 41 MB on
  Windows). It downloads only when you click, and only if the file's
  SHA-256 is the one built into the app and the one nodejs.org lists.
  Settings → Developer shows which Node.js the extensions use and
  removes the downloaded one. Servers that start with `npx` still need
  Node.js installed on the computer.
- On Windows, servers that need Python 3 are started with `py` or
  `python`, not the Microsoft Store's stand-in `python3`.
- *Browse extensions* (Settings → Developer) lists 16 public `.mcpb`
  releases, each a publisher's own GitHub release, in 6 categories with
  a search. *Install* downloads the bundle only when you press it, checks
  its SHA-256 against the listed one and shows the preview below before
  anything is installed. An installed extension with a newer listed
  version offers the update.
- The preview of an extension shows its publisher, where it comes from,
  its SHA-256 and the bundle's signature: trusted (it chains to the
  system's code-signing roots), self-signed, untrusted, unverifiable,
  invalid or unsigned. A self-signed, untrusted, unverifiable or
  unsigned bundle needs an extra confirmation, and an invalid one (changed
  after it was signed) can't be installed.

**Files from GitHub and Google**
- *Add from GitHub*, in a chat's attach menu and a project's menu: pick
  text files from a repository's tree to add to the chat or to the
  project's knowledge. Public repositories need no sign-in (GitHub
  allows 60 requests an hour); an optional access token, kept in the
  system's credential store, reads private repositories and raises the
  limit to 5,000 an hour.
- *Add from a link*: a Google Drive file, or a Google Doc, Sheet or
  Slides deck shared as "anyone with the link". A Doc comes in as
  Markdown, a Sheet (the one the link shows) as CSV and a deck as text.
  There is no Google sign-in, so a file that isn't shared by link is
  refused.

**Chats and styles**
- Settings → Profile: a custom style can be drafted from up to 3
  writing samples (6,000 characters each, at least 200 in all). A fast
  model describes how they are written; you edit the draft and save it.
  The samples are sent once and not stored. Up to 20 drafts a day, with
  an API key or on your Claude account.
- Below 900 px the artifact panel opens as a sheet over the chat, with
  *Back to chat* in place of *Close*.
- An artifact's error is shown once; before, it showed in the app and
  again inside the artifact.
- Settings no longer says that searching past chats is unavailable on
  your Claude account.

**Keys and errors**
- The key of an OpenAI-compatible endpoint is kept in the system's
  credential store (an existing key is moved there) and Settings shows
  it masked. Where the system has no credential store it stays in the
  app's database.
- Many error messages from the engine now appear in the interface
  language (English or Turkish); the original English text stays under
  *Details*.

**Interface**
- Lists, tabs and menus can be driven with the arrow keys, Home and End.
- The Code side panel can be resized: it keeps at least 300 px, and the
  chat keeps at least 400 px.
- In a diff, a binary file is a note instead of a broken line, and a
  diff of more than 1,500 lines shows the first 1,500 with a button for
  the rest.
- Text contrast meets WCAG AA in both themes (body, secondary and status
  text, and accent buttons).
- Nothing is clipped at 420 px wide.
- Onboarding keeps what you typed when you change the language, and says
  why it can't go on without a key. Project cards count chats in each
  language, and task times and review comments on removed lines are in
  the interface language.

**Known limits**
- The Node.js download was tried only against a local test server, not
  against nodejs.org.
- The extension directory and the GitHub and Google link sources were
  tried only against local test servers. Google's export formats were
  not checked against Google, and files that aren't shared by link
  can't be read: that would need a Google sign-in, and the app has no
  Google client registered for it.
- Drafting a style was tried with an API key against a stand-in, not on
  your Claude account.
- Some messages from the engine are still English in the Turkish
  interface: the engine-busy texts, its crash text, errors when
  continuing a reply, its notifications, the success text when a
  worktree's changes are applied, and the diagnostics' labels.
- Update notices cover only extensions the directory lists. The
  connector directory still has none of Anthropic's own account
  connectors, and MCP sampling is not supported.
- While a reply streams, the window still parses the reply's whole HTML
  again on every update: about 18 % of a core in a development build
  during a long reply. A release build was not measured.
- On your Claude account a continued reply's repeated start is not
  trimmed.
- An MCP App page can reach the hosts its server declares without asking
  you first.
- MCP Apps are shown inline only: no side panel or full screen, and an
  app can't update what Claude knows.
- A plain `npx tauri build` (or `npm run build`) still needs the updater
  signing key; use `npm run build:unsigned`.
- The CI end-to-end job has still not been seen passing on GitHub: the
  WebView's debugging port doesn't open on the runner. The same 15 tests
  pass on a development machine.
- Skills run only in Code sessions on the Claude Code engine; chats and
  the API-key engine don't use them yet.
- A PDF over the limits still has no text-only fallback.
- Mermaid diagrams and the artifact libraries are still loaded from public
  CDNs, so they need the internet.
- Keeping the computer awake on macOS and Linux, and keeping tokens in the
  config file on Linux, are untested on those systems.
- Fast mode, replies with thinking switched off, memory written through
  Claude Code, Research and code execution were tested against stand-ins
  and at the configuration level, not yet against live replies.

## [0.1.6] - 2026-11-22

Continue a reply that was cut off, much less work while a reply streams,
no white flash at start, artifacts kept off the network, and a larger
connector directory.

**Chats**
- *Continue* on the last reply when it was stopped or broke off with an
  error: Claude goes on from where it stopped instead of starting over
  (*Retry* still writes it again). With an API key the new text is added
  to the same message, less what it repeats of the end. On your Claude
  account the Claude Code session is resumed with the reply's last words,
  and the rest is kept as a second message in the same answer. A reply
  that stopped in a tool call, a Research report and replies of
  OpenAI-compatible models can still only be retried.
- On your Claude account, what had streamed in of a reply is kept when
  you stop it or when Claude Code exits in the middle of it.
- Much less data goes from the engine to the window while a reply
  streams: about 7 times the size of the text instead of about 80 times
  (measured on a 77,900-character reply with 30 code blocks), and in that
  test the longest pause between frames fell from 59 ms to 16 ms. The
  reply is redrawn up to 10 times a second (before, about 20).
- The window no longer flashes white when it opens in a dark theme: it
  starts in the colour of the theme you used last.

**Artifacts**
- Artifacts can no longer reach other hosts: requests, and images, media
  and fonts from other sites, are blocked, as in Claude Desktop's
  artifact sandbox. Scripts from the usual CDNs and Google Fonts still
  load, so React and Mermaid, `window.storage` and
  `window.claude.complete` work as before, and Claude is told to put data
  and images in the artifact itself. A blocked request shows as the
  artifact's error, with *Try fixing*.
- The artifact panel closes when you switch to a branch of the chat that
  has no artifact, and opens again when you come back to one that has.

**Connectors**
- *Browse connectors* lists 48 remote MCP servers run by their vendors
  (before, 15), each checked against its vendor's documentation, with a
  filter for their 11 categories. New among them: GitLab, Vercel,
  Netlify, Cloudflare, Hugging Face, monday.com, ClickUp, Airtable,
  Webflow, PostHog, Datadog and PagerDuty.

**Interface**
- Labels that were still in English in the Turkish interface (pasted
  text, token counts, the MCP config example, the palette's Developer
  entry) are translated. An untitled chat is *New chat* (*Yeni sohbet*)
  everywhere and an untitled Code session *Untitled* (*Adsız*); a chat
  whose first message has no text stays untitled instead of being named
  "New chat" in English.
- Search snippets no longer show raw Markdown (table pipes, headings,
  code fences, bold marks, link targets), and a table that follows a
  code block no longer touches it.

**Development**
- `npm run build:unsigned` builds the installers without the updater's
  signing key.
- The Claude account engine's turn loop has 11 end-to-end tests against a
  stand-in Claude Code CLI: a plain turn, a permission allowed and
  denied, resume and fork, a CLI that dies mid-reply, continuing a cut-off
  reply, a rate-limit event, Stop, a CLI that stays after its result, a
  lost session and a workflow.
- Test copies of the app (a development build started with the WebView
  debugging port, or any build started with `CLAUDERUST_OFFSCREEN=1`)
  open off-screen and never take focus.
- The end-to-end launcher picks a debugging port the machine lets it bind
  (9333 first). When it fails it prints that port's state, the app's log,
  its processes and folders, and the reserved port ranges.
- The test that failed on the Windows and macOS CI runners with 0.1.5 now
  waits for what it checks; those jobs passed on the last run.

**Known limits**
- While a reply streams, the window still parses the reply's whole HTML
  again on every update: about 18 % of a core in a development build
  during a long reply. A release build was not measured.
- On your Claude account a continued reply's repeated start is not
  trimmed.
- The connector directory has no Desktop extension directory and none of
  Anthropic's own account connectors. There is no extension update
  check, and MCP sampling is not supported.
- An MCP App page can reach the hosts its server declares without asking
  you first.
- A plain `npx tauri build` (or `npm run build`) still needs the updater
  signing key; use `npm run build:unsigned`.
- The CI end-to-end job has not yet been seen passing on GitHub: its last
  run failed because the WebView's debugging port never opened on the
  runner. This release changes how the port is chosen.
- MCP Apps are shown inline only: no side panel or full screen, and an app
  can't update what Claude knows.
- Skills run only in Code sessions on the Claude Code engine; chats and
  the API-key engine don't use them yet.
- A PDF over the limits still has no text-only fallback.
- Mermaid diagrams and the artifact libraries are still loaded from public
  CDNs, so they need the internet.
- Keeping the computer awake on macOS and Linux, and keeping tokens in the
  config file on Linux, are untested on those systems.
- Fast mode, replies with thinking switched off, memory written through
  Claude Code, Research and code execution were tested against stand-ins
  and at the configuration level, not yet against live replies.

## [0.1.5] - 2026-11-22

Plan usage limits, thinking and Fast mode on your Claude account, code
execution and Research with an API key, incognito chats, chat memory,
Skills, a connector directory and MCP Apps.

**Chats on your Claude account**
- The sidebar shows how much of your plan's usage limits (the 5-hour and
  weekly windows) is used, as Claude Code reports it. It appears after the
  first answer on the account since the app started. A banner over the
  message box says when a limit is close (80 % or the server's warning) or
  reached; sending stays possible either way.
- The extended thinking switch now applies here too, on the models where
  Claude Code lets thinking be turned off.
- Fast mode, on Opus 5.5 and Fable 5.2: faster answers at twice the price
  per token, which the switch shows. It works here and with an API key, and
  Settings can turn it on for new chats. When the account or the API can't
  give it, the reply runs at standard speed (on the account a note says
  so).
- With *Search and reference chats* on, these chats can search your
  earlier chats too; before, only chats on an API key could.

**With an Anthropic API key**
- Code execution and file creation (Settings, on by default): Claude can
  run code in Anthropic's sandbox, for example to analyse data or make a
  spreadsheet, slides, a document or a PDF. The files it makes are saved in
  a folder of the chat's, never opened by themselves, and listed under the
  reply as *Files Claude made*, with *Save a copy* and *Show in its folder*.
- *Research*: a message sent with it on is split into up to 5
  sub-questions, each searched by an agent that can only search and read
  the web, and the answer is a report with its sources. It spends at most
  the turn's budget (2 USD when none is set); Stop ends all of it.
- A project whose text files add up to more than 150,000 characters is no
  longer sent whole with every message: Claude gets the list of files and
  searches or reads them as it needs. Smaller projects work as before.

**Chats and the sidebar**
- *New incognito chat*: nothing of it is kept once you close it (the chat,
  its Claude Code transcript, files its code made), also after a crash. It
  stays out of search, memory, the past-chat tools, usage totals, the
  Artifacts page, export and print.
- Conversations can be pinned, archived and marked unread, and put in
  sidebar sections (groups in the Code tab) by dragging them there. An
  answer that ends while you are in another chat marks its chat unread.
- Chat memory, off until you turn it on in Settings → Memory: a small
  Claude model keeps short notes about you from your recent chats, which
  you can read, edit, add to and delete, and new chats get them. One memory
  for chats outside projects and, unless you turn it off, one per project;
  a project can be left out. Incognito chats are never read for it.
- Mermaid code blocks in replies are drawn as diagrams, in a sandboxed
  frame; the source stays a click away.
- *Read aloud* speaks an answer with the system's voices, where the system
  has any.
- *Print or save as PDF…* in a chat's menu.
- Screen readers hear answers as they stream, notices and errors; focus
  stays inside an open dialog and goes back where it was when it closes;
  a permission card can be answered from the keyboard (Enter allows once,
  Esc denies).

**Skills, connectors and extensions**
- Settings → Skills lists your personal skills, a project's, and those of
  installed plugins, and turns them on or off. *Install skill…* shows what
  a .zip or .skill holds before it installs, and *Create skill…* starts a
  chat in which Claude writes one.
- *Browse connectors*: 15 MCP servers run by their vendors (GitHub,
  Linear, Notion, Sentry, Atlassian, Asana, Stripe, PayPal, Intercom,
  Square, Figma, Supabase, Neon, Context7, DeepWiki); picking one fills in
  the custom connector form.
- Extensions can be uninstalled, and their settings changed.
- MCP Apps: a tool that comes with an interactive page shows it with its
  result, in the artifact sandbox. The page's tool calls follow the same
  Allow, Ask or Block setting as Claude's own (Ask unless you changed it).
- MCP servers in a Code session get the session's folder as their root.
- On Windows and macOS a custom connector's bearer token and an
  extension's secret settings are kept in the system's credential store,
  with only a reference in the config file; a token already in the file
  moves there the next time the app saves the MCP settings. On Linux they
  stay in the file, and the connector form says so.

**Artifacts**
- `confirm`, `alert` and `prompt` behave as in a sandboxed page: `confirm`
  answers no, `alert` shows nothing, `prompt` gives no text.
- In chats where an artifact can't ask Claude (`window.claude.complete`),
  such as those on your Claude account, the call is refused without first
  asking you to allow it.
- The Artifacts page shows Claude's later edits to an artifact.

**Attachments**
- Refusals of an attachment are shown in the app's language.
- A message is refused before sending when its attachments add up to more
  than about 30 MB, or when a PDF has more than 100 pages for a model with
  a 200K context; the message says why.

**Other**
- Settings → About has *Show logs* and *Copy diagnostics*, which copies
  the end of the app's log with keys, tokens, e-mail addresses and home
  folders taken out.
- The computer doesn't go to sleep while Claude works (an answer, a Code
  session, a workflow or a routine); Settings → *Keep awake while Claude
  works*, on by default. The display may still turn off.

**Development**
- The end-to-end test launcher prints a line every minute while it waits
  for the development copy's window, and that copy's last output when it
  fails, so a CI log shows why. The CI end-to-end job's first run, with
  0.1.4, timed out before the window came up.

**Known limits**
- MCP Apps are shown inline only: no side panel or full screen, and an app
  can't update what Claude knows.
- Skills run only in Code sessions on the Claude Code engine; chats and
  the API-key engine don't use them yet.
- A PDF over the limits still has no text-only fallback.
- There is no extension directory and no extension update check. MCP
  sampling is not supported.
- Mermaid diagrams and the artifact libraries are still loaded from public
  CDNs, so they need the internet.
- Keeping the computer awake on macOS and Linux, and keeping tokens in the
  config file on Linux, are untested on those systems.
- Fast mode, replies with thinking switched off, memory written through
  Claude Code, Research and code execution were tested against stand-ins
  and at the configuration level, not yet against live replies.

## [0.1.4] - 2026-11-22

Project files and cleanup for chats on your Claude account, Excel and
PowerPoint attachments, artifacts that can be fixed and can ask Claude,
custom connectors, and streams that no longer hang.

**Chats on your Claude account**
- A project's PDFs and images now reach these chats; before, they were
  silently left out. They go with the first message of a Claude Code
  session and are not sent again on later turns, except files added to the
  project since.
- Deleting such a chat also deletes its Claude Code transcript (unless a
  fork of it still uses it) and its system-prompt file. That file is now
  also removed when each turn ends, and the ones earlier versions left
  behind are removed at start.

**Attachments**
- Excel (.xlsx) and PowerPoint (.pptx) files can be attached: their text
  goes to Claude, each sheet as CSV (up to 10,000 rows and 200 columns),
  each slide as its lines of text.
- A PDF over 22 MB or over 600 pages is refused when you attach it, with
  the reason, instead of failing when the message is sent.
- A binary file that happens to read as text (for example one padded with
  NULs) is no longer attached as text. Text file types such as logs still
  are.
- *Add screenshot* is no longer greyed out: it attaches a still of the
  screen or window you pick in the system's picker.

**Artifacts**
- An HTML artifact that sets no colours of its own shows dark text on a
  light background in the dark theme too; before, it could be white on
  white.
- When an artifact throws an error, or Claude's edit to it cannot be
  applied, a notice above it offers *Try fixing*: a click sends Claude the
  error and asks for a fix.
- With an Anthropic API key, an artifact can ask Claude
  (`window.claude.complete`) after you allow it, once or always for that
  artifact. Answers come from the model set for chat titles and use your
  API key. Limits: prompts of up to 20,000 characters, 10 calls a minute
  for each artifact and 100 for a conversation's artifacts until the app
  restarts.
- Claude can change a long artifact by sending only the passages it
  replaces instead of writing it again in full. If such an edit cannot be
  applied, the version before it stays on screen.

**Long conversations**
- Turns you loaded by scrolling up are unloaded again when they are far off
  screen (they keep their place and are drawn again as you come back), so
  memory stays bounded in very long chats. Find in the conversation still
  searches all of it.

**MCP**
- *Add custom connector* in Settings → Developer adds a remote MCP server
  by name and URL, with an optional bearer token or OAuth client ID; a
  server that needs a sign-in offers *Sign in* in your browser. Remote
  servers' cards have Edit and Remove.
- An MCP prompt's arguments are filled in a form in the app, with the
  required ones marked, instead of one system dialog per argument.

**Stability**
- A stream that goes silent is detected instead of waiting up to 10
  minutes: with an API key the turn stops after 90 s without data while
  text is coming in (5 minutes otherwise, for example while Claude thinks),
  and a request that stalls before anything arrived is tried again.
  OpenAI-compatible providers stop after 90 s of silence once text has
  started.
- A reply that stalls part way is still saved up to where it stopped.

**Interface**
- Automatic retries show a line under the message box with the reason and
  a countdown, such as "Rate limited, retrying in 12 s (2/4)", also in
  chats on your Claude account.
- The sidebar that folded by itself to make room (a narrow window, or an
  artifact open in a window under 1500 px wide) opens again when the room
  is back, unless you folded or opened it yourself in between.
- Connection errors in chats, notifications and the first-start setup are
  said plainly, with the raw error text under *Details*.
- The system's reduce-motion setting is honoured: animations and smooth
  scrolling are turned off; spinners still turn, slowly.

**Development**
- The end-to-end UI test starts its own development copy (test identifier,
  test key, no user hooks) with `npm run e2e`, and CI runs it on Windows.
  That CI job first runs with this release.

**Known limits**
- A custom connector's bearer token is stored as plain text in the MCP
  config file, not in the system's credential store.
- AI-powered artifacts do not work in chats on your Claude account: they
  need an Anthropic API key. There the permission question still shows
  before the call is refused.
- `window.confirm` and `window.alert` inside artifacts do not behave as in
  a browser: `confirm` gives no plain yes or no (so `if (confirm(...))`
  always goes on), and `alert` shows nothing.
- A PDF over the limits is refused with no text-only fallback. The 100-page
  limit of the 200K-context models and the total size of several PDFs are
  not checked before sending.
- The Artifacts page shows an artifact's last full version, without
  Claude's later edits to it.

## [0.1.3] - 2026-11-22

Long conversations open fast, Auto mode asks in more cases, older MCP
servers connect and can ask you questions, and clearer network errors.

**Long conversations**
- A long conversation opens quickly: only the latest 40 turns (fewer when
  they are long) are drawn, and older ones load as you scroll up or press
  *Show earlier messages*. Find in the conversation still searches all of
  it, and the outline has *Show earlier messages* to list every turn.

**Code**
- Auto permission mode now also asks before a command that hands a program
  code or a command to run through its options or stdin (for example
  `python -c`, `node --eval`, `make -f -`, `cargo --config`, compiler
  plugins), before installing a named package, and before git commands that
  copy hooks in (`--template`).
- It also asks before running scripts and build files that changed during
  the session, before git runs hooks the session changed, and before editing
  scripts, build files, CI workflows or hook files.

**MCP**
- Remote MCP servers that only speak the older HTTP+SSE transport connect:
  when a server turns the newer transport down, the app tries the older one.
- A server can ask you for information while it works (elicitation): a
  dialog shows its message and fields, and you send, decline or close it.
  What you type there goes to that server.
- Requests to remote MCP servers follow a redirect only within the same
  site, as their sign-in already did.

**Security**
- Windows and macOS: the credential-store entries (API key, MCP sign-ins)
  are named after the app's identifier, so a development or test copy keeps
  its own instead of sharing the installed app's. The installed app keeps
  the entries it already had.
- A copy of the app registers itself for `clauderust://` links only with the
  installed app's identifier and only when no working handler is
  registered, so starting another copy no longer takes the links over
  (Windows and Linux; the Linux check is not yet tried on Linux).

**Interface**
- New chats and new code sessions start on the model you picked last, kept
  separately for each; after a change of the default model in Settings, or
  when that model is no longer offered, they start on the default.
- Network errors in Settings (Test connection, Load models), on MCP server
  cards and in Updates are said plainly, with the raw error text under
  *Details*.
- Words in table cells stay whole; a wide table scrolls sideways.
- In a narrow window the Code workspace opens as a sheet over the dimmed
  chat; a click beside it closes it.

**Stability**
- An answer saved while it streams in is written in the background, so a
  slow disk no longer holds up the stream.
- At most 64 artifact documents are kept for display, the oldest going
  first, instead of all of them being dropped at once.

**Development**
- CI runs every Rust test target (unit and integration tests) on Windows,
  Ubuntu and macOS, and the UI lint and tests; these jobs first run with
  this release. The new end-to-end UI test drives a development copy on
  Windows and is not part of the installer.

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
