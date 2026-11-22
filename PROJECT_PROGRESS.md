# PROJECT_PROGRESS.md

Bu dosya, ClaudeRust'ı en güncel **Claude Desktop** deneyimiyle kıyaslanabilir üretim olgunluğuna (**>= 90/100**) taşıyan döngünün hafızasıdır. Lider ajan her turda burayı okur ve günceller. Kanıtlar: [docs/audit-1.md](docs/audit-1.md) (Denetim 1), [docs/eval-1.md](docs/eval-1.md) (Puanlayıcı 1).

## Durum

| Alan | Değer |
|---|---|
| **Release sayacı** | **24/12** (10–15 düzeltmede bir otomatik release; 12 hedef orta nokta) |
| **Güncel puan** | **82.5/100 (Puanlayıcı 3, v0.1.3); Tur 4 + Tur 5 sonrası Puanlayıcı 4 (v0.1.5) bekliyor** |
| Hedef | >= 90/100 |
| Tur | 5 |
| Son yayın | v0.1.4 (2026-11-22); v0.1.5 hazırlanıyor (Tur 5) |
| Backlog | 180 madde: 74 açık, 60 yapıldı |

## Puanlama ölçeği (bağımsız puanlayıcı, 100 üzerinden)

1. Claude Desktop özellik paritesi: 30
2. Stabilite ve crash direnci: 25
3. Native performans ve kaynak verimliliği: 20
4. UI/UX cilası ve etkileşim kalitesi: 15
5. Kod kalitesi, test ve paketleme: 10

Şişirilmiş puan yasak: kırılan her puan dosya:satır kanıtıyla gerekçelendirilir.

## Döngü kuralları

- Her turda en kritik 3–5 görev grubu, **ayrık dosya sahipliğiyle** uzman alt ajanlara dağıtılır (Backend/Stabilite, Frontend/IPC, Performans, Test/Altyapı).
- Her turdan sonra kapı: `npm run lint`, `cargo check --no-default-features --all-targets`, `cargo clippy --no-default-features --all-targets -- -D warnings`, `cargo test --no-default-features`. Kırık build veya uyarı bırakılmaz.
- Release sayacı 10–15 düzeltmeye ulaşınca Release alt ajanı: sürümü artırır (Cargo.toml, Cargo.lock, tauri.conf.json, package.json), CHANGELOG'u günceller, imzasız tam derleme yapar (`npx tauri build --config src-tauri/tauri.no-updater.conf.json`), commit, `git tag` ve `gh release create` (Cafunw hesabı) ile yayınlar, sayaç sıfırlanır.
- Her turun sonunda geliştiricilerden bağımsız puanlayıcı çalışır. Puan < 90: eksikler backlog'a eklenir, yeni tur. Puan >= 90: final release.
- Alt ajan kuralları: commit/push/stash/reset yok (yalnızca Lider ve Release ajanı), kullanıcının çalışan release uygulamasına (target/release) dokunulmaz, anahtar/token okunmaz, canlı model çalıştırmaları Haiku düşük efor ve kısıtlı bütçe.

## Başlangıç ölçümleri (Denetim 1)

- Boşta RAM (kullanıcının çalışan sürümü, 7 süreç): ≈390 MB WS / ≈191 MB özel. Claude Desktop (Electron, 13 süreç, aktif kullanımda): ≈2038 MB WS / ≈1971 MB özel (aynı koşul değil, yaklaşık).
- Boşta CPU: 0 ms / 10 sn.
- Testler: 198 lib testi, 182 geçti, **6 düştü** (QA-01: test yalıtımı sorunu), 10 yok sayıldı. Clippy `-D warnings` temiz. Lint temiz.
- Üretim kodunda 48 unwrap/expect, hepsi elenmiş: kullanıcı/ağ verisiyle ulaşılabilir panik bulunamadı.
- IPC: 161 komut, 237 çağrı yeri, 0 kayıtsız komut, 0 hatalı argüman anahtarı; 28 olay, hepsinin dinleyicisi var.

## Bu turun planı

**Tur 5 tamamlandı** (özellik turu: 24 kalem, 8 devir maddesi). Kapı (kendim yeniden çalıştırdım): lint 0, test:ui 148/148, check 0, clippy -D warnings 0, cargo test lib 355 + artifact_frame 1 + artifact_serve 3 + branching 7 + chat_memory 2 + incognito_sweep 1 + turn_loop 18 geçti / 0 düştü; hermetik e2e 15/15 (kapı ajanı); 3 küçük canlı hesap-motoru turu. Sıradaki: v0.1.5 yayını (CI-04 günlük iyileştirmesiyle), ardından Puanlayıcı 4 (v0.1.5; özellik ölçütü kullanıcının tanımında, görünüm benzerliği puan kırmaz).

## Backlog

### Backend / Stabilite (35)

| ID | Önem | Efor | Başlık | Dosyalar | Durum |
|---|---|---|---|---|---|
| BE-01 | critical | S | Windows: the 'safe command' auto-allow can be bypassed with PowerShell subexpressions, so any command runs without asking, even in plan mode | `codetools.rs` | YAPILDI (Tur 1) |
| EV3-02 | high | S | Deleted account chats leave transcripts and system-prompt files on disk | `commands.rs` `claudecode.rs` | YAPILDI (Tur 4): silinen hesap sohbetinin CLI kaydı ve system-<id>.txt silinir; başka sohbetin kullandığı kayıt korunur; açılışta eski system-*.txt süpürülür (canlı doğrulandı) |
| BE-03 | high | M | Built-in Edit/Write break on CRLF and non-UTF-8 files (multi-line edits never match; line endings get mixed) | `codetools.rs` | YAPILDI (Tur 1) |
| EV2-05 | high | M | Auto mode auto-runs model-written scripts (permission bypass) | `codetools.rs` `claudecode.rs` | YAPILDI (Tur 3): Auto modu seçeneklerle/yorumlayıcılarla verilen komut ve kodu sorar (46 saldırı komutu sorar, 31 güvenli komut sormaz); kalan: projenin kendi test/derleme komutları üzerinden çalışan model yazımı kaynak |
| R4-01 | high | M | Özel bağlayıcının taşıyıcı jetonu claude_desktop_config.json içinde düz yazı (anahtar zincirine taşınmalı) | `mcp.rs` `ui/js/mcp_connector.js` | YAPILDI (Tur 5): özel bağlayıcı jetonu OS kimlik deposuna taşındı, config'te yalnızca başvuru (Windows'ta canlı doğrulandı; Linux'ta dosyada kalır ve form söyler) |
| BE-02 | high | L | Windows: the 'Bash' tool and hooks actually run Windows PowerShell 5.1, unannounced, so bash syntax fails and cd never persists | `codetools.rs` `hooks.rs` `prompts.rs` | AÇIK |
| BE-04 | medium | S | `cargo test` fails on Windows: 6 tests are not hermetic (they read the user's `/.claude and are fooled by the python3 Store stub) | `hooks.rs` `mcp.rs` | YAPILDI (Tur 1, = QA-01) |
| BE-05 | medium | S | No logger and no panic hook: every log::warn/error and every panic in the release app vanishes | `lib.rs` `main.rs` `src-tauri/Cargo.toml` … | YAPILDI (Tur 1, = QA-02) |
| BE-06 | medium | S | Staged attachments leak in AppState.staged when the composer is dropped without sending; the workflows map is not cleared on delete | `commands.rs` `state.rs` `ui/js/composer.js` … | AÇIK |
| BE-09 | medium | S | Chats that run on the Claude account (CLI chat engine) silently drop project PDFs and images | `claudecode.rs` `engine.rs` | AÇIK |
| BE-11 | medium | S | change_revert can delete any file on disk: abs_in accepts absolute paths outside the repo | `gitops.rs` `commands.rs` | YAPILDI (Tur 1) |
| EV-06 | medium | S | A reply being streamed is not saved until it finishes, so a crash loses it | `engine.rs` | YAPILDI (Tur 2) |
| EV-08 | medium | S | MCP connections do not recover, and tool-list changes are ignored | `mcp.rs` | YAPILDI (Tur 2) |
| EV2-10 | medium | S | Hard-coded keyring service shares secrets between app copies | `settings.rs` `mcp_oauth.rs` | YAPILDI (Tur 3) |
| EV3-12 | medium | S | Stalled streams wait up to 600 s; no idle/ping watchdog | `anthropic.rs` | YAPILDI (Tur 4): Anthropic ve OpenAI-uyumlu akışlarda durağanlık bekçisi |
| BE-07 | medium | M | Code sessions keep every edited file's original bytes, and finished background shells keep 1 MB buffers, for the whole app lifetime | `codetools.rs` `state.rs` `shells.rs` … | AÇIK |
| BE-08 | medium | M | Checkpoint shadow repos grow forever: one commit per code turn, never pruned, not removed with their sessions | `checkpoints.rs` `engine.rs` `commands.rs` | AÇIK |
| BE-10 | medium | M | MCP: a crashed stdio server is neither reported nor restarted, and tools/list_changed notifications are ignored | `mcp.rs` `commands.rs` | AÇIK |
| EV-07 | medium | M | Child processes are orphaned if the app crashes or is killed | `lib.rs` `codetools.rs` `mcp.rs` … | YAPILDI (Tur 2): Windows Job Object; açık kalan: Linux/macOS ebeveyn-ölümü sinyali belgelendi |
| EV2-11 | medium | M | Linux: API key stored in plain text; MCP OAuth sign-in lost on every quit | `settings.rs` `mcp_oauth.rs` | AÇIK (ertelendi: Linux gizli depo) |
| BE-12 | low | S | CLI chats leave the full system prompt (project instructions and knowledge) on disk forever, even after the chat is deleted | `claudecode.rs` `commands.rs` | AÇIK |
| BE-13 | low | S | Fork is not transactional: a failure mid-copy leaves a partial, untitled conversation | `db.rs` `commands.rs` | AÇIK |
| BE-14 | low | S | A stalled API stream (dead TCP connection, no RST) hangs the turn for 10 minutes before erroring | `anthropic.rs` | AÇIK |
| BE-15 | low | S | Terminal pane: unbounded channel between PTY reader and UI pump, so a fast producer grows memory without limit | `terminal.rs` | AÇIK |
| BE-16 | low | S | Project files (full base64) are re-read from SQLite and re-hashed on every tool step of a chat turn, on the async runtime | `engine.rs` `db.rs` | AÇIK |
| BE-17 | low | S | Whole-file reads before size checks: archmap and claude.ai import can load huge inputs into memory | `archmap.rs` `import.rs` | AÇIK |
| BE-18 | low | S | Dead IPC surface: 4 registered commands are never called, so PTYs are orphaned after a webview reload | `lib.rs` `commands.rs` `ui/js/panes/terminal.js` | KISMEN: term_list kullanımda (kalan 3 komut: delete_artifact, code_info, worktree_commit) |
| EV-20 | low | S | open_path and reveal_path accept any path from the webview | `commands.rs` | YAPILDI (Tur 2) |
| EV2-16 | low | S | Every release start takes over the clauderust:// handler | `lib.rs` | KISMEN (Tur 3): Linux protokol işleyicisi eklendi; Windows'ta derlenemez, CI doğrulayacak |
| EV2-17 | low | S | Tauri globals injected into sandboxed artifact iframes | `lib.rs` `artifacts.rs` | AÇIK (ertelendi: Tauri kümesi tüm çerçevelere enjekte edilir) |
| EV2-18 | low | S | Served artifacts map cleared wholesale after 64 entries | `commands.rs` | YAPILDI (Tur 3) |
| EV3-19 | low | S | A Tauri plugin init script runs in sandboxed artifact frames and throws on each load | `lib.rs` | AÇIK: Tauri başlatma betiği artifact çerçevelerine giriyor; window.confirm Promise döndürüyor (if(confirm()) hep doğru), alert hiçbir şey yapmıyor |
| EV3-20 | low | S | NUL-heavy binary files are accepted as text attachments | `attachments.rs` `util.rs` | YAPILDI (Tur 4): NUL ağırlıklı ikili dosya metin sayılmaz |
| R5-07 | low | S | Araştırma modunda paralel ajanların uçuştaki istekleri %80 bütçe eşiğini aşabilir; gizli sohbette CLI çıkış beklemesinden (2,5 sn) sonra yazılan kayıt öksüz kalabilir | `research.rs` `incognito.rs` | AÇIK (sonraki tur) |
| R5-08 | low | S | Bilgisayarı uyanık tutma macOS/Linux kolları yalnızca CI'da derlenir, Windows powercfg doğrulaması yapılmadı | `keepawake.rs` | AÇIK (sonraki tur) |

### Frontend / IPC (37)

| ID | Önem | Efor | Başlık | Dosyalar | Durum |
|---|---|---|---|---|---|
| FE-01 | high | L | Remote MCP servers have no OAuth sign-in, so Claude Desktop-style custom connectors cannot connect | `mcp.rs` `ui/js/settings.js` | AÇIK |
| FE-03 | medium | S | Esc anywhere outside a modal stops the running turn, including in side-pane inputs and when dismissing the quote bar | `ui/js/main.js` `ui/js/composer.js` | YAPILDI (Tur 1) |
| FE-05 | medium | S | Stale route renders: Project and Sessions pages can overwrite the view after you have already navigated elsewhere | `ui/js/projects.js` `ui/js/code.js` `ui/js/main.js` | YAPILDI (Tur 1) |
| FE-06 | medium | S | Esc on a nested dialog closes every stacked modal, the bottom one first (for example Settings closes with the confirm) | `ui/js/util.js` | YAPILDI (Tur 1) |
| FE-07 | medium | S | t() interpolation fills only the first occurrence of a placeholder and expands `$&`/`$'` patterns, so the History 'Explain' prompt sent to Claude is wrong | `ui/js/i18n.js` | YAPILDI (Tur 1) |
| FE-08 | medium | S | Opening a conversation shows no loading state: the previous view stays on screen and partly live while the new one renders | `ui/js/main.js` `ui/js/chat.js` | AÇIK |
| FE-04 | medium | M | WebView2 browser defaults leak into the app: F5 / Ctrl+R / right-click 'Refresh' reload the whole UI, losing queued messages and orphaning terminal PTYs | `ui/js/main.js` `ui/js/panes/terminal.js` `lib.rs` | KISMEN (Tur 1): F5/Ctrl+R/bağlam menüsü UI katmanında engellendi; iframe içindeki tuşlar için WebView2 SetAreBrowserAcceleratorKeysEnabled gerekir (2 yeni bağımlılık + unsafe COM) |
| FE-09 | medium | M | Sidebar and history lists scale with the total number of conversations and artifacts on every refresh | `ui/js/sidebar.js` `ui/js/code.js` `ui/js/artifacts.js` … | AÇIK |
| FE-10 | medium | M | Per-conversation workspace panes, xterm instances and PTYs are only released when a conversation is deleted | `ui/js/workspace.js` `ui/js/panes/terminal.js` `ui/js/chat.js` | AÇIK |
| FE-11 | medium | M | No automated UI tests: pure, critical JS logic is untested and the turnstream JS fixture is not wired up | `package.json` `ui/js/turnstream.js` `ui/js/keymap.js` … | AÇIK |
| R4-03 | medium | M | Artifact içinde confirm/alert bozuk (Tauri başlatma betiği; confirm Promise döndürür) | `artifacts.rs` `src-tauri/tauri.conf.json` | YAPILDI (Tur 5): artifact çerçevesinde confirm false döner, alert etkisiz (canlı doğrulandı) |
| FE-02 | medium | L | Long conversations: no virtualization, collapsed tool cards keep their full output in the DOM, and the whole rendered conversation is re-fetched after every turn | `ui/js/parts.js` `ui/js/chat.js` `ui/css/app.css` … | AÇIK |
| FE-12 | low | S | Async search UIs lack request sequencing, so stale results can replace newer ones | `ui/js/search.js` `ui/js/code.js` `ui/js/suggest.js` … | AÇIK |
| FE-13 | low | S | Files dropped on Projects, Chats, Artifacts or Sessions pages go into a detached composer and silently vanish | `ui/js/main.js` `ui/js/composer.js` | AÇIK |
| FE-14 | low | S | Text contrast below WCAG AA for --text-3 and dark-mode error/danger surfaces, and no reduced-motion support | `ui/css/app.css` | AÇIK |
| FE-16 | low | S | Shortcut hints are hard-coded (ignore rebinding and macOS), and the default workspace toggle Mod+` cannot be typed on some layouts | `ui/js/sidebar.js` `ui/js/search.js` `ui/js/workspace.js` … | AÇIK |
| FE-17 | low | S | Permission prompts have no keyboard path, and the card is removed before the response is confirmed | `ui/js/chat.js` | AÇIK |
| FE-19 | low | S | Worktree base-branch name is interpolated into HTML unescaped | `ui/js/panes/worktree_actions.js` | AÇIK |
| FE-20 | low | S | Quick Entry drops the typed text on a failed submit and never follows theme changes | `ui/js/quick.js` | AÇIK |
| EV-14 | low | S | The terminal 'Ask Claude' bar covers terminal output | `ui/js/panes/terminal.js` `ui/css/app.css` | YAPILDI (Tur 2) |
| EV-15 | low | S | The 'N files edited' summary miscounts lines and shows absolute Windows paths | `ui/js/chat.js` | YAPILDI (Tur 2) |
| EV-22 | low | S | Account-mode chats get no generated titles | `engine.rs` | YAPILDI (Tur 2) |
| EV-23 | low | S | Layout nits: clipped engine chip, inconsistent cost display, narrow-window sidebar without backdrop | `ui/css/app.css` `ui/js/chat.js` `ui/js/sidebar.js` | YAPILDI (Tur 2) |
| EV2-14 | low | S | Narrow-window layout: mid-word table breaks; Code panel covers the transcript | `ui/css/app.css` `ui/js/workspace.js` | YAPILDI (Tur 3) |
| EV2-20 | low | S | Model choice not kept for new chats | `ui/js/composer.js` `ui/js/store.js` | YAPILDI (Tur 3) |
| EV2-21 | low | S | Raw transport errors shown in Settings | `mcp.rs` `ui/js/settings.js` | YAPILDI (Tur 3) |
| EV3-15 | low | S | Silent automatic retries with no visible status | `anthropic.rs` `ui/js/chat.js` | YAPILDI (Tur 4): yeniden deneme durumu görünür (API olayı + CLI api_retry) |
| EV3-16 | low | S | MCP prompt arguments use native window.prompt dialogs | `ui/js/composer.js` | YAPILDI (Tur 4): MCP prompt argümanları uygulama içi form |
| EV3-17 | low | S | Sidebar auto-collapses on artifact open or narrow window and is never restored | `ui/js/artifacts.js` `ui/js/sidebar.js` | YAPILDI (Tur 4): kenar çubuğu otomatik daralınca geri açılır |
| EV3-21 | low | S | Raw transport errors shown to the user | `ui/js/neterr.js` `error.rs` | YAPILDI (Tur 4): ham taşıma hataları çevrilir, ayrıntı genişletilebilir |
| EV3-22 | low | S | No prefers-reduced-motion support | `ui/css/app.css` | YAPILDI (Tur 4): prefers-reduced-motion desteği |
| R4-02 | low | S | Hesap motorunda window.claude.complete reddedilmeden önce onay penceresi çıkıyor; desteklenmiyorsa hiç sorulmamalı | `ui/js/artifact_bridge.js` `artifact_ai.rs` | YAPILDI (Tur 5): desteklenmeyen motorda window.claude.complete onay sormadan reddedilir |
| R4-04 | low | S | Artifact güncelleme komutu sonrası galeri son tam sürümü gösteriyor | `engine.rs` `ui/js/artifacts.js` | YAPILDI (Tur 5): galeri uygulanan güncellemeyi gösterir |
| R5-06 | low | S | Küçük arayüz: bash_code_execution araç kartı ham adı gösteriyor; arşivli sohbetler için palette filtresi yok | `ui/js/parts.js` `ui/js/search.js` | AÇIK (sonraki tur) |
| FE-15 | low | M | Accessibility basics: modals without focus trap or restore, no live regions, non-focusable 'more' menus, unlabeled icon buttons | `ui/js/util.js` `ui/js/sidebar.js` `ui/js/code.js` … | AÇIK |
| FE-18 | low | M | Composer stubs: 'Add screenshot' is a permanently disabled item, and MCP prompt arguments use window.prompt() | `ui/js/composer.js` | AÇIK |
| EV-16 | low | M | Mixed-language UI: English model descriptions and backend errors in a localized UI | `models.rs` `error.rs` `ui/js/i18n.js` | KISMEN (Tur 2): model açıklamaları ve hata türleri çevrildi; kalan sabit İngilizce arka uç metinleri açık |

### Performans (21)

| ID | Önem | Efor | Başlık | Dosyalar | Durum |
|---|---|---|---|---|---|
| PERF-01 | high | M | Highlighted code HTML is 21-37x the source size (about 35 nested scope spans per line), which bloats every payload, the DOM and the render caches | `highlight.rs` `markdown.rs` | YAPILDI (Tur 1) |
| PERF-02 | high | L | Opening a conversation and every turn end re-render and ship the whole conversation with no windowing; long sessions cost seconds and hundreds of MB | `view.rs` `render.rs` `commands.rs` … | AÇIK |
| PERF-06 | medium | S | Every launch runs a full-table JSON scan and UPDATE of messages synchronously in setup(), before the window is shown | `lib.rs` `db.rs` `claudecode.rs` | YAPILDI (Tur 1) |
| PERF-03 | medium | M | Full-resolution base64 images go inline into every conversation view payload as data URLs | `render.rs` `attachments.rs` `lib.rs` … | AÇIK |
| PERF-04 | medium | M | Claude Code engine (also Chat on a Claude subscription) deep-clones and re-renders the whole turn every 45 ms on a runtime worker | `claudecode.rs` `engine.rs` `render.rs` | AÇIK |
| PERF-05 | medium | M | Turn stream sends a full keyframe every second, about 70% of the streamed bytes and several MB/s in long agentic turns | `turnstream.rs` `ui/js/turnstream.js` | AÇIK |
| PERF-07 | medium | M | WebView2 holds about 315 MB working set at idle, and nothing lowers it when the window is hidden to the tray | `lib.rs` | AÇIK |
| EV2-03 | medium | M | Long conversations render every turn: about 98k DOM nodes and a half-second freeze on open | `view.rs` `ui/js/chat.js` | YAPILDI (Tur 3): uzun sohbet penceresi (son 40 tur, yukarı kaydırınca öncekiler); 600 mesajda açılış 12 sn → 0,9 sn (debug), 98k → 6,9k DOM düğümü, 3,75 MB → 251 KB yük |
| EV2-09 | medium | M | Account chats start a new CLI process every turn (about 2.1-2.3 s to first text) | `claudecode.rs` | AÇIK (ertelendi: kalıcı CLI süreci büyük yeniden tasarım) |
| EV3-08 | medium | M | Turns loaded by scrolling up are never unloaded (memory and streaming cost grow) | `ui/js/turnwindow.js` `ui/js/chat.js` | YAPILDI (Tur 4): yukarı kaydırılan turlar park edilir: 1.200 turdan 1.192 park, 1.805 düğüm, +36 MB |
| PERF-08 | medium | L | Chat on the Claude subscription starts a new Claude Code CLI process for each message and ends it at the result | `claudecode.rs` | AÇIK |
| PERF-10 | low | S | Ctrl+R prompt history repeats a 305 ms full scan on each keystroke, although the SQL does not depend on the query | `db.rs` `ui/js/prompt_history.js` `commands.rs` | YAPILDI (Tur 1) |
| PERF-11 | low | S | The sidebar re-reads and serializes every conversation after each answer to draw about 30 rows | `ui/js/sidebar.js` `commands.rs` `db.rs` | AÇIK |
| PERF-12 | low | S | Two HTTP client stacks and two HTML parsers are linked into the binary | `src-tauri/Cargo.toml` `Cargo.lock` | AÇIK |
| EV2-19 | low | S | Streaming saves run on async-runtime workers behind one DB mutex | `engine.rs` `db.rs` | YAPILDI (Tur 3) |
| R5-05 | low | S | 600 mesajlık sohbette ilk açılıştan hemen sonra bir get_conversation 14 sn sürdü (debug, soğuk önbellek); sonraki 8 çağrı 170 ms | `view.rs` `commands.rs` | AÇIK (sonraki tur) |
| PERF-09 | low | M | The built-in agent keeps each edited file's original bytes for the app's lifetime with no total cap, and the Changes pane deep-copies all of them on every refresh | `codetools.rs` `gitops.rs` `state.rs` | AÇIK |
| PERF-13 | low | M | No code splitting in the UI: 51 of 53 modules (530 KB) load and run at startup | `ui/js/main.js` `ui/js/settings.js` `ui/js/importer.js` … | AÇIK |
| EV-12 | low | M | Streaming IPC re-sends the whole growing text part on every tick | `turnstream.rs` `ui/js/turnstream.js` | YAPILDI (Tur 2) |
| EV-13 | low | M | A new Claude Code CLI process is spawned for every turn | `claudecode.rs` | AÇIK (ertelendi: sonraki tur) |
| EV3-14 | low | L | The account engine spawns a CLI process and resumes the transcript on every turn | `claudecode.rs` | AÇIK (ertelendi: kalıcı CLI süreci büyük yeniden tasarım (L)) |

### Test / Build / Kalite (32)

| ID | Önem | Efor | Başlık | Dosyalar | Durum |
|---|---|---|---|---|---|
| QA-01 | high | S | cargo test is red on the owner's Windows machine: 6 failures from 2 test-isolation bugs, and only 1 of them involves Python | `mcp.rs` `hooks.rs` | YAPILDI (Tur 1) |
| QA-02 | high | S | No log backend and no panic hook: every log::warn!/error!/info! is dropped, and release panics leave no trace | `src-tauri/Cargo.toml` `lib.rs` | YAPILDI (Tur 1) |
| CI-01 | high | S | GitHub CI on main is red: powershell_code_is_never_a_safe_command fails on Ubuntu and macOS (C:/ is not absolute on Unix) | `codetools.rs` | KISMEN (Tur 3): platform bağımsız yapıldı; GitHub Ubuntu/macOS koşucularında doğrulanacak |
| CI-02 | high | S | GitHub CI on main is red: mcp::a_server_that_keeps_exiting_is_given_up fails on Ubuntu (restart loop or test race) | `mcp.rs` | YAPILDI (Tur 3): Linux yarışı giderildi (çıkış "reconnecting" olarak işaretlenir); koşucuda doğrulanacak |
| EV2-12 | high | S | Published v0.1.2 release has no updater manifest, so auto-update fails | `.github/workflows/release.yml` `src-tauri/tauri.conf.json` | AÇIK (KULLANICI KARARI: güncelleyici imza anahtarı sahibinde) |
| CI-03 | high | M | turn_loop::a_reply_cut_off_by_a_crash_is_kept_and_shown_stopped Ubuntu ve macOS CI'da düşüyor (ikinci bağlantı kısmi yanıtı 30 sn içinde görmüyor); Windows'ta geçiyor | `src-tauri/tests/turn_loop.rs` `engine.rs` `db.rs` | YAPILDI (Tur 4) ve doğrulandı: GitHub Ubuntu ve macOS test işleri yeşil (v0.1.4 koşusu) |
| QA-03 | high | L | The core turn loops and the permission round trip have no tests, and cannot have any as written (concrete Wry AppHandle) | `engine.rs` `claudecode.rs` `commands.rs` | AÇIK |
| QA-06 | medium | S | Release workflow gates nothing: no tests and no tag/version check, and the version is kept in 4 places | `.github/workflows/release.yml` `src-tauri/tauri.conf.json` `updates.rs` … | AÇIK |
| QA-07 | medium | S | 0.1.0 was never tagged or released: the updater manifest and the README download link return 404 | `CHANGELOG.md` `README.md` | AÇIK |
| QA-11 | medium | S | On Windows, dropping files onto a project's knowledge panel probably does nothing, because dragDropEnabled is on (not confirmed in the app) | `ui/js/main.js` `ui/js/projects.js` `src-tauri/tauri.conf.json` | AÇIK |
| QA-04 | medium | M | No schema versioning or migrations in the SQLite store, although the app auto-updates itself | `db.rs` | YAPILDI (Tur 1) |
| QA-05 | medium | M | cargo audit reports 7 vulnerabilities, and two zip and two reqwest stacks are linked into the binary | `src-tauri/Cargo.toml` `Cargo.lock` `.github/workflows/ci.yml` … | AÇIK |
| QA-08 | medium | M | Windows, the owner's platform, has no coverage for process-tree kill, hooks execution or the PTY terminal | `codetools.rs` `hooks.rs` `terminal.rs` … | AÇIK |
| QA-09 | medium | M | Import flow: branch trees, projects and the MCP merge have no tests | `import.rs` | AÇIK |
| QA-10 | medium | M | UI has zero automated tests; README claims e2e checks that are not in the repo, and the JS turnstream decoder is never checked against the Rust encoder | `ui/js/turnstream.js` `turnstream.rs` `.github/workflows/ci.yml` … | AÇIK |
| EV-17 | medium | M | No end-to-end test of the turn loop (stream, persist, tools, drop, stop) | `engine.rs` `claudecode.rs` `dev/mock_api.py` | YAPILDI (Tur 2) |
| EV-18 | medium | M | About 10k lines of UI JavaScript have almost no tests | `ui/js/turnstream.js` `ui/js/chat.js` `ui/js/composer.js` … | YAPILDI (Tur 2) |
| EV-19 | medium | M | Installers are not code-signed (Authenticode, notarization) | `.github/workflows/release.yml` `src-tauri/tauri.conf.json` | AÇIK (ertelendi: dış bağımlılık/çok büyük) |
| EV2-13 | medium | M | No tests for the DOM-heavy critical UI flows or the command-level branch/edit/regenerate paths | `ui/test` `ui/js/chat.js` `ui/js/parts.js` … | YAPILDI (Tur 3): komut düzeyinde Rust testleri (branching.rs 6) + CDP uçtan uca duman testi (9/10, 10. adım sahibin hook'ları yüzünden atlanır) |
| EV3-09 | medium | M | e2e UI suite not in CI; Code-session e2e skipped when hooks exist | `.github/workflows/ci.yml` `ui/e2e/smoke.test.mjs` | KISMEN (Tur 4): hermetik e2e (npm run e2e, 15/15, Code testi artık atlanmıyor) + CI işi yazıldı; CI işi GitHub'da henüz koşmadı |
| EV3-10 | medium | M | Critical flows without tests: artifact preview pipeline, account-engine project context, attachment types | `artifacts.rs` `claudecode.rs` `attachments.rs` … | YAPILDI (Tur 4): artifact boru hattı, hesap motoru proje bağlamı ve ek türü matrisi testleri; run_once döngüsü hâlâ testsiz (QA-03) |
| CI-04 | medium | M | CI e2e işi (windows-latest) GitHub koşucusunda koşmadı: geliştirme kopyasının penceresi 25 dakikada açılmadı (e2e-logs artifact'ında dev.log); iş 25 dk bekleyip kırmızı b | `.github/workflows/ci.yml` `ui/e2e/run.mjs` `dev/e2e.conf.json` | TUR 5 sonu (Lider): başarısızlıkta dev.log kuyruğunu iş günlüğüne yazdır, bekleme süresini kısalt, sebebi çöz |
| QA-12 | low | S | Declared MSRV is false: rust-version 1.80, but the resolved dependencies need Rust 1.91 | `src-tauri/Cargo.toml` | YAPILDI (Tur 1) |
| QA-13 | low | S | `npm run build` fails without the signing key; the local release build must use its own target dir | `package.json` `README.md` | AÇIK |
| QA-14 | low | S | Tests call std::env::set_var while other test threads run | `anthropic.rs` `filesearch.rs` `mcp.rs` … | AÇIK |
| QA-15 | low | S | CI/release hygiene: floating action tag in the signing job, no concurrency cancel, redundant parse step, no fmt or audit gate | `.github/workflows/ci.yml` `.github/workflows/build.yml` `.github/workflows/release.yml` | AÇIK |
| QA-16 | low | S | Four Tauri commands are registered but never called from the UI | `commands.rs` `lib.rs` | GEÇERSİZ OLDU: term_list artık terminal.js tarafından kullanılıyor, kaldırılmamalı |
| QA-17 | low | S | CSP img-src is wider than the app needs | `src-tauri/tauri.conf.json` | AÇIK |
| EV2-24 | low | S | CI skips the integration and UI tests | `.github/workflows/ci.yml` | YAPILDI (Tur 3) |
| EV2-23 | low | M | Installers are not Authenticode-signed | `.github/workflows/release.yml` | AÇIK (KULLANICI KARARI: kod imzalama sertifikası gerekir) |
| EV3-18 | low | M | Windows installers not Authenticode-signed | `.github/workflows/release.yml` `src-tauri/tauri.conf.json` | AÇIK (KULLANICI KARARI: Authenticode sertifikası gerekir) |
| EV3-23 | low | L | Oversized modules (commands.rs 3,352 lines, 151 commands) | `commands.rs` `codetools.rs` `engine.rs` … | AÇIK (ertelendi: büyük modüllerin bölünmesi (L), düşük etki) |

### Claude Desktop paritesi (55)

| ID | Önem | Efor | Başlık | Dosyalar | Durum |
|---|---|---|---|---|---|
| FP-02 | high | S | The Extended thinking switch does nothing on the Claude Code engine (chats and code sessions) | `claudecode.rs` `ui/js/composer.js` | AÇIK |
| EV3-05 | high | S | HTML artifact fragments render white-on-white when the OS is in dark mode | `artifacts.rs` `ui/js/artifacts.js` `ui/css/app.css` | YAPILDI (Tur 4): HTML parçaları koyu ve açık temada okunur (CDP ile ölçüldü) |
| FP-01 | high | M | Chats on the Claude-account engine (the onboarding default) run with web search, MCP connectors, past-chat search and project PDFs/images off, but the UI still shows thos | `claudecode.rs` `engine.rs` `ui/js/composer.js` | AÇIK |
| FP-03 | high | M | Claude's questions (AskUserQuestion) cannot be answered in Code sessions; other CLI dialog requests are rejected | `claudecode.rs` `ui/js/chat.js` `ui/js/parts.js` | AÇIK |
| FP-05 | high | M | MCP servers configured in ClaudeRust are missing from Claude Code engine sessions (the default code engine) | `claudecode.rs` `mcp.rs` | AÇIK |
| FP-06 | high | M | Chat cannot read spreadsheets or presentations and cannot create files (no code execution / file creation) | `attachments.rs` `models.rs` `engine.rs` … | AÇIK |
| EV-01 | high | M | Account-mode chat has no tools: MCP, web search and chat search are dead, but the composer still offers them | `claudecode.rs` `ui/js/composer.js` `engine.rs` | YAPILDI (Tur 2) |
| EV3-01 | high | M | Project PDFs and images are dropped on the Claude-account (CLI) chat engine | `claudecode.rs` `engine.rs` | YAPILDI (Tur 4): proje PDF/görselleri hesap motorunun ilk satırında belge/görsel bloğu olarak gider; --resume turlarında yeniden gönderilmez (canlı doğrulandı) |
| FP-04 | high | L | No OAuth and no 'Add custom connector' flow for remote MCP servers; any server that needs sign-in fails with HTTP 401 | `mcp.rs` `ui/js/settings.js` `commands.rs` | AÇIK |
| FP-07 | high | L | Desktop's Cowork mode (autonomous non-coding agent in a sandboxed workspace) has no equivalent | `ui/js/sidebar.js` `engine.rs` `codetools.rs` | AÇIK |
| EV-02 | high | L | No sync of chats and projects with the Claude account | `db.rs` `import.rs` | AÇIK (ertelendi: dış bağımlılık/çok büyük) |
| EV-03 | high | L | Remote MCP lacks OAuth, custom connectors, MCP Apps and an extension directory | `mcp.rs` `ui/js/settings.js` | YAPILDI (Tur 2): OAuth tam akış + SSRF/yönlendirme açıkları kapatıldı; açık kalan: "özel bağlayıcı ekle" penceresi, MCP Apps, Linux'ta kalıcı anahtarlık |
| EV2-01 | high | L | Chats and projects don't sync with the claude.ai account | `claudecode.rs` `db.rs` `import.rs` | AÇIK (ertelendi: claude.ai hesap API'si yok) |
| EV2-02 | high | L | No Cowork, computer use, Claude in Chrome, SSH or cloud sessions | `claudecode.rs` `ui/js/code.js` | AÇIK (ertelendi: Cowork/bilgisayar kullanımı/Chrome/SSH kapsam dışı ve çok büyük) |
| EV-05 | medium | S | AskUserQuestion is treated as a permission and never answered in the Code tab | `claudecode.rs` `ui/js/chat.js` | YAPILDI (Tur 2) |
| FP-08 | medium | M | No subscription usage-limit display: the CLI's rate_limit_event is dropped and the usage page only shows API cost | `claudecode.rs` `ui/js/usage_ui.js` `ui/js/sidebar.js` | AÇIK |
| FP-09 | medium | M | No chat memory (synthesized, editable, per-project) and no incognito chats | `settings.rs` `engine.rs` `prompts.rs` … | AÇIK |
| FP-10 | medium | M | 'Add screenshot' is a permanently disabled menu item; Quick Entry is text-only | `ui/js/composer.js` `ui/js/quick.js` `ui/quick.html` … | AÇIK |
| FP-11 | medium | M | No voice input (dictation) or read-aloud | `ui/js/composer.js` `ui/js/chat.js` `ui/js/parts.js` | AÇIK |
| FP-12 | medium | M | Skills and plugins are absent: no discovery, no management UI, and the built-in engine and chats cannot use them | `slash.rs` `ui/js/slash.js` `claudecode.rs` … | AÇIK |
| FP-13 | medium | M | Artifacts lack AI-powered artifacts, persistent artifact storage, and 'Try fixing with Claude' for runtime errors | `artifacts.rs` `ui/js/artifacts.js` `lib.rs` | AÇIK |
| FP-16 | medium | M | Pull requests end at creation: no CI/check status, review comments, or auto-merge | `pullrequest.rs` `ui/js/panes/changes.js` `ui/js/panes/pr_util.js` | AÇIK |
| FP-18 | medium | M | Screen-reader support is minimal: no live regions for streaming answers, toasts or errors | `ui/index.html` `ui/js/chat.js` `ui/js/util.js` … | AÇIK |
| FP-19 | medium | M | Desktop extensions can be installed but not uninstalled, reconfigured, updated or browsed | `extensions.rs` `commands.rs` `ui/js/settings.js` | AÇIK |
| EV-09 | medium | M | React artifacts cannot import shadcn/ui and have no window.claude or storage APIs | `artifacts.rs` | YAPILDI (Tur 2) |
| EV2-07 | medium | M | Only 2 UI languages; tray and native dialogs hard-coded in English | `ui/js/i18n.js` `lib.rs` | AÇIK (ertelendi: 42 dil kapsam dışı) |
| EV2-08 | medium | M | MCP: no legacy SSE transport, elicitation, sampling or MCP Apps | `mcp.rs` | KISMEN (Tur 3): eski SSE taşıması + elicitation (soru kartı) eklendi; açık: roots/list, sampling, MCP Apps |
| EV3-03 | medium | M | Attachments: xlsx/pptx refused, PDFs over 100 pages fail at the API, 'Add screenshot' always disabled | `attachments.rs` `ui/js/composer.js` | KISMEN (Tur 4): xlsx/pptx metni, 22 MB/600 sayfa üstü PDF erken reddi, ikili dosya, ekran görüntüsü öğesi; açık: aşırı sınır PDF için metin yedeği, 100 sayfa sınırı, birden çok PDF toplam boyutu, Türkçe arayüzde İngilizce ek hataları |
| EV3-06 | medium | M | No AI-powered artifacts (window.claude.complete) and no 'Try fixing' on artifact errors | `artifacts.rs` `ui/js/artifacts.js` | KISMEN (Tur 4): "Try fixing" ve API motorunda window.claude.complete (onay + sınırlar) çalışır; açık: hesap motorunda onay penceresi reddedilmeden önce çıkıyor |
| EV3-07 | medium | M | Artifact edits are full rewrites; the prompt hides the shadcn shim and window.storage | `prompts.rs` `render.rs` | KISMEN (Tur 4): gerçek çalışma zamanı istemi ve güncelleme komutu uygulanır; açık: galeri son tam sürümü gösterir, gerçek model yanıtıyla uçtan uca denenmedi |
| EV3-11 | medium | M | No connector/extension directory and no 'Add custom connector' form | `ui/js/settings.js` `mcp.rs` | KISMEN (Tur 4): özel bağlayıcı formu (ekle/düzenle/kaldır, OAuth düğmesi); AÇIK GÜVENLİK: taşıyıcı jeton claude_desktop_config.json içinde düz yazı olarak saklanıyor |
| R5-02 | medium | M | Skills yalnızca Claude Code CLI oturumlarında çalışır; sohbet ve API motoru kullanmaz | `skills.rs` `engine.rs` | AÇIK (sonraki tur) |
| FP-14 | medium | L | Single main window: no 'Open in new window' and no split view | `lib.rs` `ui/js/main.js` `ui/js/store.js` | AÇIK |
| FP-15 | medium | L | Claude cannot see or operate the Code tab's Preview pane, so it cannot check its own UI changes | `ui/js/panes/preview.js` `codetools.rs` `claudecode.rs` | AÇIK |
| FP-17 | medium | L | Code sessions are local only: no SSH hosts, cloud sessions or Remote Control | `ui/js/code.js` `claudecode.rs` `terminal.rs` | AÇIK |
| EV-04 | medium | L | Missing desktop integrations: dictation, computer use, Claude in Chrome, Cowork | `ui/js/composer.js` `lib.rs` | AÇIK (ertelendi: dış bağımlılık/çok büyük) |
| EV2-04 | medium | L | No Agent Skills, file creation or code execution in chat | `models.rs` `engine.rs` | AÇIK (ertelendi: sandbox/kod yürütme gerekir) |
| EV2-06 | medium | L | No voice dictation, incognito chats, Research mode or chat memory | `ui/js/composer.js` `memory.rs` | AÇIK (ertelendi: kapsam dışı (kullanıcının puan listesinde yok)) |
| EV3-04 | medium | L | MCP Apps (interactive ui:// resources) not supported | `mcp.rs` `render.rs` `ui/js/parts.js` | AÇIK (ertelendi: MCP Apps (ui:// etkileşimli kaynaklar) büyük (L); puan 90 altında kalırsa sonraki tur) |
| R5-01 | medium | L | MCP Apps eksikleri: yan panel/tam ekran, ui/message, model bağlamı güncelleme, sayfadan resources/read, tema bildirimi, hesap motoru araç adları için uygulamalar | `mcp_apps.rs` `ui/js/mcp_apps.js` | AÇIK (sonraki tur) |
| FP-20 | low | S | Session lists have star and delete only: no archive, no pinned code sessions, no unread markers | `ui/js/sidebar.js` `ui/js/code.js` `db.rs` | AÇIK |
| FP-21 | low | S | No fast mode toggle | `claudecode.rs` `ui/js/composer.js` `models.rs` | AÇIK |
| FP-22 | low | S | No 'keep computer awake' while agents, workflows or routines run | `engine.rs` `claudecode.rs` `routines.rs` … | AÇIK |
| FP-24 | low | S | MCP tools/list_changed notifications are ignored, so a server's new tools appear only after a manual Reload | `mcp.rs` | AÇIK |
| EV-10 | low | S | No deep links | `lib.rs` `src-tauri/Cargo.toml` `src-tauri/tauri.conf.json` | YAPILDI (Tur 2): clauderust:// derin bağlantıları |
| FP-23 | low | M | No URL protocol handler (deep links) | `src-tauri/Cargo.toml` `src-tauri/tauri.conf.json` `lib.rs` | AÇIK |
| FP-25 | low | M | Only two interface languages (English, Turkish) against Desktop's eleven | `ui/js/i18n.js` | AÇIK |
| EV-11 | low | M | Only English and Turkish | `ui/js/i18n.js` | AÇIK (ertelendi: sonraki tur) |
| EV2-15 | low | M | Artifacts load their libraries from public CDNs (break offline); no window.claude.complete | `artifacts.rs` | AÇIK (ertelendi: kütüphaneleri paketlemek ikili boyutu büyütür) |
| EV2-22 | low | M | Quick Entry is text-only | `ui/js/quick.js` `commands.rs` | AÇIK (ertelendi) |
| R4-05 | low | M | Ek sınırları: aşırı büyük PDF için metin yedeği yok, 100 sayfa sınırı ve çok PDF toplamı denetlenmiyor, ek hataları Türkçe arayüzde İngilizce | `attachments.rs` `commands.rs` | KISMEN (Tur 5): çevrilebilir ek hataları, 100 sayfa ve 30 MB mesaj sınırı yapıldı; aşırı PDF için metin yedeği yok (Cargo.lock'ta PDF ayrıştırıcı yok) |
| R5-03 | low | M | Eklenti dizini ve güncelleme denetimi yok (kaldırma ve yeniden yapılandırma var); MCP örnekleme (sampling) yok | `extensions.rs` `mcp.rs` | AÇIK (sonraki tur) |
| R5-04 | low | M | Artifact kütüphaneleri (react, react-dom, mermaid) hâlâ CDN'den: yerel paketleme için npm'den indirme onayı gerekir (kullanıcı kararı) | `artifacts.rs` | AÇIK (sonraki tur; KULLANICI KARARI: npm indirme) |
| EV-21 | low | L | Code tab has no remote, SSH or cloud environments | `claudecode.rs` `ui/js/code.js` | AÇIK (ertelendi: dış bağımlılık/çok büyük) |
| EV3-13 | low | L | No project knowledge search; the whole knowledge base is inlined every request | `prompts.rs` `ui/js/projects.js` | AÇIK (ertelendi: proje bilgisi araması (RAG) büyük (L)) |

## Tur günlüğü

- **Denetim 1**: beş bağımsız denetçi, 93 madde (1 kritik, 13 yüksek).
- **Tur 1**: 5 uzman + kapı ajanı. Düzeltilenler: BE-01 (kritik, Windows PowerShell izin atlatması + 5 ek delik), BE-03 (CRLF/UTF-16), BE-11 (change_revert yol sınırı), QA-01 (6 düşen test), QA-02 (dosya günlüğü + panik kancası), QA-04 (şema sürümleme), QA-12 (rust-version 1.91), PERF-01 (vurgulanmış kod HTML'i 17-34x → 2.5-3.4x), PERF-06 (başlangıç taraması 73 ms → 11 ms @48k mesaj), PERF-10, FE-03, FE-05, FE-06, FE-07. FE-04 kısmen. Lider bağımsız doğrulama: lint, check, clippy -D warnings, test (199/0) ve geliştirme kopyasında açılış + günlük dosyası yazımı temiz.
- **Release v0.1.1** yayınlandı (Latest): iki yükleyici (8.16 MB, 10.96 MB), etiket 3c2543c. release.yml etiketten tetiklenen iki çalıştırma iptal edildi (imza ortamı yok); GitHub boş bir release ortamı oluşturdu. v0.1.0 taslağına dokunulmadı.
- **Puanlayıcı 1 (v0.1.1): 71/100.** Claude Desktop feature parity 16/30; Stability and crash resistance 21.5/25; Native performance and resource use 17/20; UI/UX polish and interaction quality 11/15; Code quality, tests and packaging 5.5/10. En büyük kayıplar: hesap sohbetinde araçlar yok (-3.5), hesap senkronu (-2, ertelendi), uzak MCP OAuth yok (-2), entegrasyonlar (-2), imzasız yükleyici (-1). Puanlayıcının "ölçülemedi" sınırları (release derlemesi çalıştırılamadı, tam derleme yasaktı) bir sonraki puanlamada ayrı hedef dizinle giderilecek. Tur 2 başladı.
- **Tur 2** (5 uzman + kapı + canlı QA + maliyet düzeltmesi): EV-01 hesap sohbetinde MCP + web araması (CLI'a SDK-barındırmalı MCP; canlı doğrulandı), EV-03 uzak MCP OAuth (PKCE, RFC 9728/8414/7591, loopback, SSRF korumalı), EV-05 AskUserQuestion kartı (canlı doğrulandı), EV-06 kısmi yanıt kaydı, EV-07 Job Object, EV-08 MCP yeniden bağlanma + list_changed, EV-09 shadcn/ui + window.storage, EV-10 derin bağlantılar, EV-12 IPC delta (2.29 MB → 0.21 MB), EV-14/15/23 arayüz düzeltmeleri, EV-17 tur döngüsü entegrasyon testleri (9), EV-18 UI testleri (3 → 56), EV-20 open_path sınırı, EV-22 başlık. Canlı QA iki hata buldu ve düzeltti (araçla başlayan yanıtta başlık, MCP araç etiketi). Maliyet hatası (devam edilen oturumda toplam kaydediliyor) düzeltildi. Testler: 241 + 9, 0 düştü.
- **Release v0.1.2** yayınlandı (Latest): iki yükleyici (8.32 MB, 11.19 MB), etiket f71361e. Etiketten tetiklenen iki çalıştırma iptal edildi. **GitHub CI (main) kırmızı**: Ubuntu/macOS birim testleri düşüyor: codetools::powershell_code_is_never_a_safe_command (C:/ yolu Unix'te mutlak sayılmıyor; 0.1.1 CI'ında da vardı) ve mcp::a_server_that_keeps_exiting_is_given_up (yalnızca Ubuntu). Tur 3 backlog'una eklendi.
- **Puanlayıcı 2 (v0.1.2): 71/100.** Claude Desktop feature parity 16/30; Stability and crash resistance 19.5/25; Native performance and resource use 15.5/20; UI/UX polish and interaction quality 13/15; Code quality, tests and packaging 7/10. Yeni bulgular: uzun sohbet pencerelemesi yok (98k DOM düğümü, 600 mesajda 0,5 sn donma, -2.5), Auto izin modu model yazdığı betikleri çalıştırıyor (-2), anahtarlık hizmet adı sabit (-1), güncelleme manifesti yok (sahip kararı), CI kırmızı. Ölçülenler: çıkış dosyası 25.6 MB, kurulum 7.94 MiB, boşta 200 MB özel / %0.07 CPU, ilk boyama 549-66x ms, akış 109 olay / 83.5 KB.
- **Tur 3** (5 uzman + kapı): EV2-03 uzun sohbet pencerelemesi (arka uç `conversation_window` + `turnwindow.js`; 600 mesajda açılış 12 sn → 0,9 sn debug, DOM 98k → 6,9k, yük 3,75 MB → 251 KB), EV2-05 Auto modu sertleştirmesi (yaklaşık 80 saldırı dizisi denendi, hepsi kartla yakalanıyor), EV2-10 anahtarlık hizmet adı kimliğe göre, EV2-19 kısmi kayıtlar çalışma zamanı dışına (PartialSaves yazıcısı), EV2-08 eski SSE + elicitation, EV2-13 komut düzeyinde testler + CDP uçtan uca test, EV2-24 CI'da tüm test hedefleri + ui işi, EV2-14/20/21 arayüz, EV2-16/18 protokol işleyicisi ve artifact haritası, CI-01/CI-02. Testler: 265 + 6 + 10, 0 düştü.
- **Yayın v0.1.3** (2026-11-22): `2f321ee`, etiket v0.1.3, iki imzasız Windows kurulumu (NSIS 8,38 MB, MSI 11,27 MB), Latest. Build installers: 3 işletim sistemi yeşil. CI: ui + test(windows) yeşil; test(ubuntu) ve test(macos) tek testte kırmızı (CI-03). Tag'in başlattığı Release koşusu iptal edildi (imzalama sırları yok).
- **Puanlayıcı 3 (v0.1.3): 82.5/100.** Claude Desktop feature parity 21.5/30; Stability and crash resistance 22.25/25; Native performance and resource use 18.5/20; UI/UX polish and interaction quality 13/15; Code quality, tests and packaging 7.25/10. Ölçülenler: soğuk ilk boyama 610 ms, sıcak 433-527 ms, boşta 7 süreç 215 MB özel / 0,08 sn CPU (30 sn), Claude Desktop (meşgul, 14 süreç) 2199 MB özel; zengin yanıt akışı 120 olay / 312 KB IPC, Rust tarafı 0,16 CPU-sn; 600 turluk sohbet 54 ms'de açılıyor ama en üste kaydırınca 1200 tur / 72k düğüm / özel bellek 257 → 507 MB; FTS araması 4 ms; hesap motoru ilk metin 2,4 sn. Yeni bulgular: proje PDF/görselleri hesap motorunda düşüyor (-1.5), silinen hesap sohbetinin CLI kaydı kalıyor (-1.5), xlsx/pptx reddi (-1.5), koyu temada HTML artifact parçası görünmez (-1), uzun sohbette boşaltılmayan turlar (-1), e2e CI'da değil (-1).
- **Tur 4** (6 uzman + kapı): EV3-01 hesap motoruna proje PDF/görselleri (canlı: model PDF'nin içeriğini okudu), EV3-02 silinen sohbetin CLI kaydı/sistem dosyası temizliği, EV3-03 xlsx/pptx/PDF sınırı/ikili dosya, EV3-05/06/07 artifact okunabilirlik + Try fixing + AI artifact + güncelleme komutu, EV3-08 tur park etme, EV3-11 özel bağlayıcı formu, EV3-12/15 durağanlık bekçisi + yeniden deneme durumu, CI-03 Unix testi, EV3-09/10 hermetik e2e + artifact boru hattı testleri. Testler: 294 + 3 + 6 + 12, UI 94, e2e 15/15.
- **Yayın v0.1.4** (2026-11-22): `9eac12f`, etiket v0.1.4, NSIS 8,45 MB, MSI 11,35 MB, Latest. Build installers: 3 işletim sistemi yeşil. CI: ui + test(ubuntu/windows/macos) yeşil, CI-03 doğrulandı; e2e işi kırmızı: koşucuda pencere 25 dk içinde açılmadı, hiçbir e2e adımı koşmadı (CI-04). Yayın ayrı git çalışma ağacında (D:cr-release) derlendi.
- **Tur 5** (6 uzman + kapı; kullanıcının "özellikte eşit ya da üstün" kararı, özellik denetimi 111 satır: 40 eşit, 13 üstün, 11 zayıf, 40 eksik): hesap motoru plan kullanım limiti (#82, gerçek CLI satırıyla), düşünme anahtarı (#4), hızlı mod (#6), geçmiş sohbet araması (#21), uyanık tutma (#83); API motoru kod yürütme + dosya oluşturma (#18), araştırma modu (#24), proje bilgisi araması (#37); gizli sohbet (#23), sabitle/arşivle/okunmadı/bölümler (#43), bellek (#20, #22); Mermaid (#2), sesli okuma (#90), erişilebilirlik (#99), yazdır/PDF (#35); Skills (#19), bağlayıcı dizini (15 doğrulanmış kayıt, #48), eklenti kaldırma/yeniden yapılandırma (#56), tanılama (#104), MCP Apps (#53, kısmi), MCP kökleri (#54); R4-01..04. Kapı ajanı sözleşme hatalarını düzeltti (fast_mode/fast anahtarı, kullanım ölçeri >%100, usage.rs taşma paniği, Skills büyük/küçük harf tuzağı, kod yürütme dosya adı bidi). OLAY: Tur 5 sırasında `06:51'de kullanıcının çalışan clauderust.exe (PID 13716) kapandı; hangi ajanın yaptığı belirlenemedi.
