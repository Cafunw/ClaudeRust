# ClaudeRust

**Claude masaüstü uygulamasının Rust ile yeniden yazılmış, hafif sürümü.**
_A lightweight rewrite of the Claude desktop app in Rust (Tauri 2)._ — [English below](#english)

![Ana ekran](docs/screenshots/home-light.png)

| Sohbet | Artifact | Kod modu |
|---|---|---|
| ![](docs/screenshots/chat-light.png) | ![](docs/screenshots/artifact.png) | ![](docs/screenshots/code-permission.png) |
| ![](docs/screenshots/web-search.png) | ![](docs/screenshots/project-dark.png) | ![](docs/screenshots/turkish-dark.png) |

| Claude'dan içe aktarma | Uzak Masaüstü / donanım hızlandırma |
|---|---|
| ![](docs/screenshots/import.png) | ![](docs/screenshots/desktop-settings.png) |

## Neden?

Resmî Claude Desktop bir Electron uygulamasıdır (yüzlerce MB disk, ayrı bir Chromium + Node.js çalışma zamanı). ClaudeRust aynı deneyimi şu şekilde sunar:

- **Tüm mantık Rust'ta**: Claude API istemcisi (SSE streaming, yeniden deneme), sohbet ağacı ve dallanma, SQLite depolama ve tam metin arama, Markdown → HTML ve sözdizimi renklendirme (syntect), MCP istemcisi, Kod modu ajanı ve araçları, artifact sunucusu.
- **Arayüz** işletim sisteminin kendi WebView'i ile çizilir (Windows'ta WebView2, macOS'ta WKWebView, Linux'ta WebKitGTK). Ek çalışma zamanı paketlenmez; JS çerçevesi ve derleme adımı yoktur.
- Kurulum paketi birkaç MB'tır, bellek kullanımı Electron uygulamasına göre çok daha düşüktür.

## Özellikler (Claude Desktop ile karşılaştırmalı)

| Claude Desktop | ClaudeRust |
|---|---|
| Sohbet, streaming yanıtlar, Markdown, tablolar, LaTeX, kod blokları | ✅ (Rust'ta render, KaTeX ile matematik) |
| Model seçici, **genişletilmiş düşünme**, efor seviyesi | ✅ Opus 5.5 / Sonnet 5.5 / Haiku 5.5 / Fable 5.2 + diğer modeller; "Düşünce süreci" bloğu |
| Mesaj düzenleme, yeniden deneme, **sürüm gezintisi (‹ 1/2 ›)** | ✅ Tam sohbet ağacı |
| Dosya/görsel/PDF ekleme, yapıştırma, sürükle-bırak | ✅ Görseller otomatik küçültülür; .docx metni çıkarılır; uzun yapıştırmalar dosyaya dönüşür |
| **Artifact'ler** (HTML, React, SVG, Mermaid, Markdown, kod), sürümler, önizleme/kod, indirme | ✅ Ayrı ve sandbox'lı `artifact://` protokolünde |
| **Web araması** ve kaynak gösterme | ✅ Sunucu tarafı web_search / web_fetch araçları, kaynak rozetleri |
| **Projeler**: talimatlar + bilgi dosyaları | ✅ |
| Stiller (Normal, Kısa, Açıklayıcı, Resmi, özel stiller) | ✅ |
| Profil / kişisel tercihler | ✅ |
| **Geçmiş sohbetlerde arama ve referans** (hafıza) | ✅ `conversation_search` / `recent_chats` araçları |
| **MCP sunucuları** (`claude_desktop_config.json`), araç izinleri | ✅ Aynı format; stdio + uzak (Streamable HTTP); Claude Desktop'tan içe aktarma; araç bazında İzin ver / Sor / Engelle |
| **Claude Code** (Kod sekmesi) | ✅ İki motor: **Claude Code** (resmî `claude` CLI'ı çalıştırır, **Claude üyeliğinizle** giriş yapar; oturumlar devam ettirilebilir ve dallanabilir) ya da **API anahtarı** (Rust ile yazılmış yerel ajan: Bash, Read, Write, Edit, Glob, Grep, TodoWrite, ExitPlanMode). İzin modları (İzin iste / Düzenlemeleri otomatik kabul / Plan / İzinleri atla), diff önizlemeli izin kartları, CLAUDE.md |
| **Masaüstü uzantıları** (`.mcpb` / `.dxt`) | ✅ Tek tıkla kurulum, kullanıcı ayarları formu |
| **Hızlı Giriş** (global kısayol) | ✅ Varsayılan `Ctrl+Alt+Space` (macOS: `Option+Space`) |
| Sistem tepsisi, oturum açılışında başlatma, bildirimler | ✅ |
| Açık / koyu / sistem teması, sohbet yazı tipi, yakınlaştırma | ✅ |
| Sohbet arama (Ctrl/⌘+K), yıldızlama, yeniden adlandırma, dışa aktarma | ✅ Markdown / JSON dışa aktarma |
| Arayüz dili | ✅ English, **Türkçe** |
| — | ➕ **Claude'dan içe aktarma**: claude.ai dışa aktarımı (sohbetler, dallar, artifact'ler, projeler), Claude Code oturumları, Claude Desktop MCP sunucuları ve uzantıları |
| — | ➕ **Uzak Masaüstü / VM uyumu**: uzak oturum algılanınca yazılımla çizim, ekran dışında kalan pencereyi geri getirme, "Donanım hızlandırma" ayarı |

### Zorunlu farklar

- **Giriş**:
  - **Kod** sekmesi, resmî Claude Code CLI'ı üzerinden **Claude üyeliğinizle (Pro/Max/Team)** çalışır: giriş Claude Code'un kendi `/login` akışıyla yapılır, ClaudeRust kimlik bilgilerini hiç görmez.
  - **Sohbet** kısmı için **Anthropic API anahtarı** gerekir ([console.anthropic.com](https://console.anthropic.com/settings/keys)); bu kullanım API kredisinden düşer. Anthropic, üçüncü taraf uygulamaların claude.ai hesabıyla giriş sunmasına izin vermez. Bunu yapmanın tek yolu gizli uç noktaları kullanmak ya da oturum belirteçlerini kopyalamak olurdu. Bu güvenli olmaz ve hesabınızı riske atar. Özel bir proxy/ağ geçidi için temel URL değiştirilebilir.
- claude.ai hesabına bağlı özellikler yoktur: sohbetlerin bulut senkronu, paylaşım bağlantıları, claude.ai "Connectors" dizini, sesli mod, Research modu, Chrome entegrasyonu. Sohbetler yalnızca bu bilgisayarda (SQLite) tutulur. Eski sohbetler claude.ai dışa aktarımıyla **içe aktarılabilir** (aşağıya bakın).
- React/Mermaid artifact'leri kütüphaneleri herkese açık CDN'lerden yükler (internet gerekir).
- Sistem istemleri bu proje için yazılmıştır; claude.ai'nin istemlerinin kopyası değildir.

## Kurulum

### Hazır paketler
Her push'ta GitHub Actions üç platform için kurulum paketi üretir: depo → **Actions** → **Build installers** → son çalıştırma → **Artifacts**:

- **Windows**: `.msi` veya `.exe` (NSIS). SmartScreen uyarısında "Ek bilgi → Yine de çalıştır".
- **macOS** (Apple Silicon + Intel, universal): `.dmg`. İmzasız olduğu için ilk açılışta sağ tık → Aç, ya da `xattr -cr /Applications/ClaudeRust.app`.
- **Linux**: `.deb`, `.rpm`, `.AppImage`.

`v0.1.0` gibi bir etiket gönderilirse **Release** iş akışı, paketleri taslak bir GitHub Release'e ekler.

### Kaynaktan derleme
Gereksinimler: Rust (stable), Node.js 18+ (yalnızca Tauri CLI için), Linux'ta `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev`.

```bash
npm install          # Tauri CLI
npx tauri dev        # geliştirme modunda çalıştır
npx tauri build      # kurulum paketi üret (target/release/bundle)
cargo test -p clauderust --lib   # birim testleri
```

## Kullanım notları

- **İlk açılış**: adınızı ve API anahtarınızı girin. Anahtar macOS'ta Keychain'de, Windows'ta Credential Manager'da, Linux'ta uygulama veritabanında saklanır.
- **MCP**: Ayarlar → Geliştirici → *Yapılandırmayı düzenle*. Dosya konumu:
  - Windows: `%APPDATA%\dev.clauderust.desktop\claude_desktop_config.json`
  - macOS: `~/Library/Application Support/dev.clauderust.desktop/claude_desktop_config.json`
  - Linux: `~/.config/dev.clauderust.desktop/claude_desktop_config.json`

  ```json
  {
    "mcpServers": {
      "filesystem": {
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/Desktop"]
      },
      "uzak-sunucu": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer ..." } }
    }
  }
  ```
  Claude Desktop'ta zaten sunucularınız varsa *Claude Desktop'tan içe aktar* düğmesi onları kopyalar. GUI uygulamaları kabuk PATH'ini devralmadığı için ClaudeRust, `npx`/`uvx` gibi komutları giriş kabuğunuzun PATH'i ile çözer.
- **Kod modu**: Kenar çubuğu → Kod → klasör seçin (veya yolu yapıştırın) → görev yazın. Dosya düzenlemeleri ve komutlar izin kartıyla onaylanır. Proje kökündeki `CLAUDE.md` ve `~/.claude/CLAUDE.md` talimat olarak yüklenir.
  - Seçilen klasörün **dışındaki** her dosya erişimi (okuma dahil) izin ister; "Düzenlemeleri otomatik kabul et" modu yalnızca proje klasörü içinde geçerlidir.
  - Yalnızca okuma yapan basit komutlar (`ls`, `cat`, `git status`, `rg` …) sorulmadan çalışır; yönlendirme (`>`), komut ikamesi (`$(...)`), arka plan (`&`) ya da proje dışı yol içeren her komut izin ister. "Bu oturum için izin ver" kuralları zincirdeki her komutu ayrı ayrı denetler.
  - Komut çıktısı 2 MB ile, süre 2 dakika (en fazla 10) ile sınırlıdır; "Durdur" çalışan komutu ve alt süreçlerini sonlandırır.
- **Claude üyeliğiyle Kod**: [Claude Code](https://docs.claude.com/en/docs/claude-code/setup)'u kurun, Ayarlar → Kod → *Claude Code'a giriş yap* düğmesine basın (bir terminal açılır, orada `/login` ile Claude hesabınızı seçin). Yeni oturumlar Kod sayfasındaki **Claude Code / API anahtarı** seçimine göre çalışır; ilk açılışta "Claude üyeliğimi Claude Code ile kullan" seçilirse API anahtarı hiç gerekmez. CLI farklı bir yerdeyse yolunu Ayarlar → Kod'dan girebilirsiniz.
- **Claude'dan içe aktarma** (hesap menüsü → *Claude'dan içe aktar…* ya da Ayarlar → İçe aktar):
  - **Sohbetler ve projeler**: Claude Desktop sohbetleri bilgisayarda değil claude.ai'de tutar. claude.ai → Ayarlar → Gizlilik → *Verileri dışa aktar* ile gelen zip'i seçin ya da sekmeye sürükleyin. Düzenleme dalları, ekler, düşünce blokları, artifact'ler (güncellemeleriyle sürüm sürüm) ve proje talimatları/dosyaları gelir. Tarihler korunur, tekrar içe aktarmak kopya oluşturmaz.
  - **Claude Code oturumları** (`~/.claude/projects`; CLI ve Claude Desktop'ın Kod sekmesi): içe aktarılan oturumlar Claude Code motoruyla **kaldığı yerden devam eder**.
  - **Claude Desktop bağlayıcıları**: `claude_desktop_config.json` içindeki MCP sunucuları ve Claude'da kurulu masaüstü uzantıları (kayıtlı ayarlarıyla), ayrıca `~/.claude.json` içindeki Claude Code MCP sunucuları.
- **Uzak Masaüstü, VM ve GPU sorunları**: Uzak bir oturum (Windows RDP, xrdp, VNC, Chrome Uzak Masaüstü, X yönlendirme) algılanınca ClaudeRust yazılımla çizime geçer. Linux'ta boş/beyaz pencereye yol açan DMA-BUF çizicisi varsayılan olarak kapalıdır. Kayıtlı konumu artık var olmayan bir ekranda kalan pencere görünür ekrana taşınır ve ekrandan büyükse küçültülür. Pencere yine boş ya da siyahsa Ayarlar → Masaüstü uygulaması → **Donanım hızlandırma → Kapalı** seçin. Pencere hiç açılmıyorsa uygulamayı `--disable-gpu` ile ya da `CLAUDERUST_DISABLE_GPU=1` ortam değişkeniyle başlatın.
- **Klavye kısayolları**: Yeni sohbet `Ctrl/⌘+Shift+O`, arama `Ctrl/⌘+K`, kenar çubuğu `Ctrl/⌘+B`, ayarlar `Ctrl/⌘+,`, durdur `Esc`, son mesajı düzenle `↑`, yakınlaştır `Ctrl/⌘ + / − / 0`.

---

## English

ClaudeRust recreates the Claude desktop experience — chats, projects, artifacts, web search, MCP servers, a Claude Code–style coding agent, Quick Entry and the tray — as a small native app. All application logic is Rust; the UI is plain HTML/CSS/JS rendered by the operating system's WebView (no Electron, no bundled browser, no JS build step).

**Signing in:** Code sessions can run through the official Claude Code CLI, which signs in with your **Claude subscription** through its own `/login` (ClaudeRust never sees the credentials); sessions resume and branch like in Claude Desktop. A built-in Rust agent with the same tool set is available as the API-key engine. Chat needs an Anthropic API key: Anthropic doesn't allow third-party apps to offer claude.ai sign-in, and doing it anyway would mean private endpoints or copied session tokens.

**Import from Claude:** Settings → Import brings over a claude.ai data export (chats with their edit branches, attachments, thinking, versioned artifacts, and projects with their instructions and documents), Claude Code sessions from `~/.claude/projects` (they stay resumable), Claude Desktop's MCP servers and installed extensions, and Claude Code's user MCP servers.

**Remote Desktop / VMs:** in remote sessions (RDP, xrdp, VNC, Chrome Remote Desktop, X forwarding) the WebView uses software rendering; on Linux the DMA-BUF renderer is off by default; a window restored onto a missing monitor is moved back on screen. Settings → Desktop app → *Hardware acceleration* overrides this, and `--disable-gpu` / `CLAUDERUST_DISABLE_GPU=1` help when the window doesn't open at all.

**Other differences:** no cloud sync, chat sharing, voice mode, Research mode or claude.ai connector directory; React/Mermaid artifacts load their libraries from public CDNs.

**Install:** download the installer for your platform from the latest *Build installers* workflow run (Actions → Artifacts), or from a GitHub Release. macOS builds are unsigned (right-click → Open, or `xattr -cr`). **Build from source:** `npm install && npx tauri build`.

### Architecture

```
src-tauri/src
├── anthropic.rs    Claude API client: SSE parsing, retries, content-block accumulator
├── engine.rs       request building, streaming loop, tool execution, permissions, titles
├── render.rs       stored messages → UI turns (thinking, tools, citations, artifacts)
├── markdown.rs     safe Markdown → HTML (raw HTML escaped, safe links, math placeholders)
├── highlight.rs    syntect-based highlighting with themeable CSS classes
├── artifacts.rs    <antArtifact> parsing and sandboxed documents for artifact://
├── mcp.rs          MCP client (stdio + streamable HTTP), claude_desktop_config.json
├── codetools.rs    Bash / Read / Write / Edit / Glob / Grep / TodoWrite + permission rules
├── claudecode.rs   Claude Code CLI engine: stream-json, permission prompts, resume/fork
├── import.rs       import from claude.ai exports, Claude Code sessions, Claude Desktop
├── extensions.rs   .mcpb / .dxt desktop extensions
├── display.rs      WebView rendering flags (remote sessions, GPU), keep window on screen
├── db.rs           SQLite: conversation tree, projects, artifacts, FTS5 search
├── models.rs       model catalog, thinking/effort rules, server tool variants
├── prompts.rs      system prompts for chat and code sessions
├── attachments.rs  images (downscaling), PDFs, .docx, text files
├── commands.rs     Tauri commands used by the UI
└── lib.rs          windows, tray, Quick Entry, global shortcut, artifact protocol
ui/                 index.html, css/app.css, js/*.js (ES modules), vendor/katex
```

Conversation history is sent back to the API append-only and unchanged (thinking blocks, server tool results and fallback markers included), so prompt caching and preserved thinking keep working across tool loops, branches and model switches.

### Third-party notices
KaTeX is bundled under its MIT license (`ui/vendor/katex/LICENSE`). "Claude" is a trademark of Anthropic; this is an independent client for the Claude API.
