// Failures the backend names by a code (`AppError::coded`, `error::coded_notice`):
// one sentence per code in each language, filled from the error's `params`. The
// backend's own English text stays the detail shown under "Details".
import { currentLanguage, t, tOr } from './i18n.js';

const fill = (s, p) => s.replace(/\{(\w+)\}/g, (m, k) => (p[k] === undefined || p[k] === null ? m : String(p[k])));
const many = (n, one, other) => (Number(n) === 1 ? one : other);
const took = (s, lang) => {
  const m = Math.round(Number(s) / 60);
  if (lang === 'tr') return Number(s) >= 120 ? `${m} dakika` : `${s} saniye`;
  return Number(s) >= 120 ? `${m} minutes` : `${s} seconds`;
};
const when = (ms, lang) => new Date(Number(ms)).toLocaleString(lang === 'tr' ? 'tr-TR' : 'en-US', { weekday: 'short', hour: '2-digit', minute: '2-digit' });
const IO = {
  NotFound: ['The file or folder wasn’t found.', 'Dosya ya da klasör bulunamadı.'],
  PermissionDenied: ['Access was denied.', 'Erişim reddedildi.'],
  AlreadyExists: ['That already exists.', 'Bu zaten var.'],
  StorageFull: ['The disk is full.', 'Disk dolu.'],
};

/** code -> [English, Turkish]: a sentence with {param} places, or a function of the params and the language. */
export const ERROR_TEXT = {
  // the app's own data, files and links
  io: (p, lang) => (IO[p.io] || ['A file couldn’t be read or written.', 'Bir dosya okunamadı ya da yazılamadı.'])[lang === 'tr' ? 1 : 0],
  database: ['The app’s database reported an error.', 'Uygulamanın veritabanı bir hata bildirdi.'],
  json: ['Some data couldn’t be read (it isn’t valid JSON).', 'Bazı veriler okunamadı (geçerli JSON değil).'],
  path_refused: ['ClaudeRust only opens its own data and the folders of your sessions, not {path}.', 'ClaudeRust yalnızca kendi verilerini ve oturumlarınızın klasörlerini açar; {path} açılamaz.'],
  path_invalid: ['Invalid path: {path}', 'Geçersiz yol: {path}'],
  link_not_web: ['Only web links can be opened.', 'Yalnızca web bağlantıları açılabilir.'],
  open_failed: ['The system couldn’t open it.', 'Sistem bunu açamadı.'],
  clipboard_failed: ['Couldn’t copy to the clipboard.', 'Panoya kopyalanamadı.'],
  folder_required: ['Choose a folder first.', 'Önce bir klasör seçin.'],
  folder_not_found: ['Folder not found: {path}', 'Klasör bulunamadı: {path}'],
  file_not_found: ['File not found: {path}', 'Dosya bulunamadı: {path}'],
  no_project_folder: ['This conversation has no project folder.', 'Bu sohbetin proje klasörü yok.'],
  timed_out: ['That took too long.', 'Bu çok uzun sürdü.'],
  // conversations
  message_not_found: ['Message not found in this conversation.', 'Mesaj bu sohbette bulunamadı.'],
  message_empty: ['Message is empty.', 'Mesaj boş.'],
  attachment_gone: ['An attachment is no longer available. Remove it and attach it again.', 'Bir ek artık kullanılamıyor. Kaldırıp yeniden ekleyin.'],
  regenerate_not_user: ['Can only regenerate from a user message.', 'Yalnızca bir kullanıcı mesajından yeniden oluşturulabilir.'],
  request_not_pending: ['This request is no longer pending.', 'Bu istek artık beklemede değil.'],
  project_name_required: ['Project name is required.', 'Proje adı gerekli.'],
  rewind_not_user: ['Pick one of your messages to rewind to.', 'Geri sarmak için kendi mesajlarınızdan birini seçin.'],
  no_checkpoint: ['There is no checkpoint for this message.', 'Bu mesaj için kontrol noktası yok.'],
  checkpoint_gone: ['This checkpoint is no longer available.', 'Bu kontrol noktası artık yok.'],
  compact_needs_claude: ['Compaction uses a Claude model; switch this conversation to a Claude model first.', 'Sıkıştırma bir Claude modeli kullanır; önce bu sohbeti bir Claude modeline geçirin.'],
  compact_nothing: ['There is nothing to compact yet.', 'Henüz sıkıştırılacak bir şey yok.'],
  draft_empty: ['Write a message first.', 'Önce bir mesaj yazın.'],
  draft_too_long: ['This draft is too long to rewrite in one go.', 'Bu taslak tek seferde yeniden yazılamayacak kadar uzun.'],
  draft_no_reply: ['No rewrite came back.', 'Yeniden yazılmış bir metin gelmedi.'],
  // the API's own kinds, for the places that aren't a chat's error card (parts.js says those)
  no_api_key: () => t('err_no_api_key'),
  authentication_error: () => t('err_auth'),
  rate_limit_error: () => t('err_rate_limit'),
  overloaded_error: () => t('err_overloaded'),
  credit_low: ['Your credit balance is too low. Add credits to your Anthropic account, or use your Claude account.', 'Kredi bakiyeniz çok düşük. Anthropic hesabınıza kredi ekleyin veya Claude hesabınızı kullanın.'],
  prompt_too_long: ['This conversation is now longer than the model can take in. Start a new chat, or remove some large attachments.', 'Bu sohbet artık modelin alabileceğinden uzun. Yeni bir sohbet başlatın ya da büyük ekleri kaldırın.'],
  // Claude Code (the Claude account's engine)
  claude_code: ['Claude Code stopped with an error.', 'Claude Code bir hatayla durdu.'],
  cli_timeout: (p, lang) => (lang === 'tr' ? `Claude ${took(p.s, lang)} içinde yanıt vermedi.` : `Claude didn’t answer within ${took(p.s, lang)}.`),
  cli_not_installed: ['Claude Code isn’t installed (or wasn’t found). Install it from claude.com/claude-code, or set its path in Settings → Code.', 'Claude Code kurulu değil (ya da bulunamadı). claude.com/claude-code adresinden kurun veya yolunu Ayarlar → Kod bölümünde girin.'],
  cli_start_failed: ['Could not start Claude Code ({path}).', 'Claude Code başlatılamadı ({path}).'],
  cli_not_signed_in: ['You’re not signed in to Claude. Open Settings → Account and sign in with your Claude account (or add an API key).', 'Claude’da oturum açık değil. Ayarlar → Hesap bölümünden Claude hesabınızla oturum açın (veya bir API anahtarı ekleyin).'],
  cli_usage_limit: (p, lang) => (lang === 'tr'
    ? `Claude planınızın kullanım sınırına ulaştınız. ${p.resets_at ? `Sınır ${when(p.resets_at, lang)} itibarıyla sıfırlanır.` : 'Sıfırlandığında yeniden deneyin.'}`
    : `You’ve reached your Claude plan’s usage limit. ${p.resets_at ? `It resets ${when(p.resets_at, lang)}.` : 'Try again once it resets.'}`),
  cli_session_lost: ['Claude Code no longer has this conversation’s session. Send the message again to start a new one.', 'Claude Code bu sohbetin oturumunu artık bulamıyor. Yeni bir oturum için mesajı yeniden gönderin.'],
  cc_history_replayed: ['This chat goes on in a new Claude Code session, rebuilt from its history.', 'Bu sohbet, geçmişinden yeniden kurulan yeni bir Claude Code oturumunda sürüyor.'],
  cc_history_partial: ['This chat goes on in a new Claude Code session, rebuilt from its history; some of its oldest messages or attachments were too much to include.', 'Bu sohbet, geçmişinden yeniden kurulan yeni bir Claude Code oturumunda sürüyor; en eski mesajlarından ya da eklerinden bazıları eklenemeyecek kadar fazlaydı.'],
  cli_no_session: ['This conversation has no Claude Code session yet: send a message first.', 'Bu sohbetin henüz bir Claude Code oturumu yok: önce bir mesaj gönderin.'],
  cli_sign_in_failed: ['Sign-in failed.', 'Oturum açılamadı.'],
  cli_sign_in_timeout: (p, lang) => (lang === 'tr' ? `Oturum açma ${p.n} dakika içinde tamamlanmadı.` : `Sign-in didn’t finish within ${p.n} ${many(p.n, 'minute', 'minutes')}.`),
  cli_sign_out_failed: ['Sign-out failed.', 'Oturum kapatılamadı.'],
  no_terminal_app: ['No terminal app found. Open a terminal and run: {cmd}', 'Terminal uygulaması bulunamadı. Bir terminal açıp şunu çalıştırın: {cmd}'],
  prompt_write_failed: ['Could not write the chat’s instructions for Claude Code (in {path}).', 'Sohbetin Claude Code yönergeleri yazılamadı ({path} içinde).'],
  // code sessions: git, worktrees, terminals, hooks
  not_git_repo: ['This folder is not a git repository.', 'Bu klasör bir git deposu değil.'],
  git_failed: (p, lang) => (p.cmd ? (lang === 'tr' ? `git ${p.cmd} başarısız oldu.` : `git ${p.cmd} failed.`) : (lang === 'tr' ? 'Bir git komutu başarısız oldu.' : 'A git command failed.')),
  git_start_failed: ['Git couldn’t be started. Check that it is installed.', 'Git başlatılamadı. Kurulu olduğunu kontrol edin.'],
  git_timeout: (p, lang) => (lang === 'tr' ? `git ${p.cmd || ''} ${took(p.s, lang)} içinde bitmedi ve durduruldu.` : `git ${p.cmd || ''} took longer than ${took(p.s, lang)} and was stopped.`),
  git_nothing_to_commit: ['There are no changes to commit.', 'Commit edilecek değişiklik yok.'],
  git_no_changes: ['There are no changes.', 'Değişiklik yok.'],
  git_bad_commit: ['Invalid commit id.', 'Geçersiz commit kimliği.'],
  branch_name_required: ['Give a branch name.', 'Bir dal adı girin.'],
  branch_name_invalid: ['“{name}” isn’t a valid branch name.', '“{name}” geçerli bir dal adı değil.'],
  commit_message_required: ['Write a commit message.', 'Bir commit mesajı yazın.'],
  file_outside_repo: ['That file is outside the repository.', 'Bu dosya deponun dışında.'],
  file_not_in_git: ['This file isn’t in git yet.', 'Bu dosya henüz git’te değil.'],
  original_unknown: ['The version of this file from before the session wasn’t kept (it was larger than 20 MB or couldn’t be read), so it can’t be compared or reverted.', 'Bu dosyanın oturumdan önceki hâli saklanmadı (20 MB’tan büyüktü ya da okunamadı), bu yüzden karşılaştırılamaz ve geri alınamaz.'],
  original_none: ['No earlier version of this file is known.', 'Bu dosyanın daha önceki bir sürümü bilinmiyor.'],
  revert_new_file: ['This is a new file: revert it as a whole.', 'Bu yeni bir dosya: tamamını geri alın.'],
  diff_stale: ['The file changed since the diff was shown. Refresh and try again.', 'Fark gösterildikten sonra dosya değişti. Yenileyip yeniden deneyin.'],
  patch_invalid: ['That doesn’t look like a valid patch.', 'Bu geçerli bir yama gibi görünmüyor.'],
  patch_conflict: ['The patch doesn’t apply here.', 'Yama burada uygulanamıyor.'],
  no_worktree: ['This session doesn’t use a separate worktree.', 'Bu oturum ayrı bir çalışma ağacı kullanmıyor.'],
  worktree_not_git: ['This folder is not in a git repository, so it can’t get its own worktree.', 'Bu klasör bir git deposunda değil, bu yüzden kendi çalışma ağacı olamaz.'],
  worktree_no_commits: ['The repository has no commits yet; make a first commit to use worktrees.', 'Depoda henüz commit yok; çalışma ağaçlarını kullanmak için ilk commit’i yapın.'],
  worktree_kept: ['The session’s worktree was kept at {path}.', 'Oturumun çalışma ağacı {path} konumunda bırakıldı.'],
  worktree_repo_missing: ['The repository {repo} can’t be found (is its drive connected?), so the session’s worktree at {path} was kept.', '{repo} deposu bulunamıyor (sürücüsü bağlı mı?), bu yüzden oturumun {path} konumundaki çalışma ağacı bırakıldı.'],
  worktree_saved: ['Saved the session’s uncommitted changes on branch {branch}.', 'Oturumun commit edilmemiş değişiklikleri {branch} dalına kaydedildi.'],
  worktree_branch_kept: (p, lang) => (lang === 'tr' ? `${p.branch} dalı korundu: kendine ait ${p.n} commit’i var.` : `Kept branch ${p.branch}: it has ${p.n} ${many(p.n, 'commit', 'commits')} of its own.`),
  worktree_branch_unchecked: ['Kept branch {branch}: whether it has commits of its own couldn’t be checked.', '{branch} dalı korundu: kendine ait commit’i olup olmadığı denetlenemedi.'],
  terminal_open_failed: ['Could not open a terminal.', 'Terminal açılamadı.'],
  terminal_start_failed: ['Could not start {shell} in the terminal.', 'Terminalde {shell} başlatılamadı.'],
  terminal_closed: ['This terminal has closed.', 'Bu terminal kapandı.'],
  terminal_busy: ['The program in this terminal isn’t reading input right now.', 'Bu terminaldeki program şu anda girdi okumuyor.'],
  shell_gone: ['This background command is gone.', 'Bu arka plan komutu artık yok.'],
  subagent_gone: ['This sub-agent is gone.', 'Bu alt ajan artık yok.'],
  watch_failed: ['Can’t watch the files in {path}.', '{path} içindeki dosyalar izlenemiyor.'],
  hook_blocked: ['A hook blocked this message: {reason}', 'Bir kanca bu mesajı engelledi: {reason}'],
  hooks_file_unreadable: ['{path} can’t be read, so its hooks don’t run.', '{path} okunamıyor, bu yüzden kancaları çalışmıyor.'],
  hooks_file_invalid: ['{path} isn’t valid JSON (line {line}), so its hooks don’t run.', '{path} geçerli bir JSON değil (satır {line}), bu yüzden kancaları çalışmıyor.'],
  hooks_changed: ['The project’s hooks changed; review them again.', 'Projenin kancaları değişti; yeniden inceleyin.'],
  no_check_command: ['No check command found for this project; enter one (for example: npm test).', 'Bu proje için bir denetim komutu bulunamadı; bir tane girin (örneğin: npm test).'],
  // routines
  routines_unreadable: ['The saved routines can’t be read, so they weren’t changed.', 'Kayıtlı rutinler okunamıyor, bu yüzden değiştirilmedi.'],
  routine_incomplete: ['A routine needs a name and a prompt.', 'Bir rutinin adı ve istemi olmalı.'],
  routine_too_often: (p, lang) => (lang === 'tr' ? `Bir rutin en sık ${p.n} dakikada bir çalışabilir.` : `Run at most every ${p.n} ${many(p.n, 'minute', 'minutes')}.`),
  routine_bad_time: ['Use a time like 09:30.', 'Saati 09:30 gibi yazın.'],
  routine_bad_weekday: ['Weekdays are 1 (Monday) to 7 (Sunday).', 'Hafta günleri 1 (Pazartesi) ile 7 (Pazar) arasındadır.'],
  routine_not_found: ['Routine not found.', 'Rutin bulunamadı.'],
  // MCP servers and their configuration
  config_invalid_json: ['The configuration isn’t valid JSON (line {line}, column {column}).', 'Yapılandırma geçerli bir JSON değil (satır {line}, sütun {column}).'],
  config_no_servers: ['The configuration needs an "mcpServers" object.', 'Yapılandırmada bir "mcpServers" nesnesi olmalı.'],
  // a program that isn't there (Windows' "path not found" too) is said as such
  mcp_start_failed: (p, lang) => (p.io === 'NotFound' ? t('neterr_no_program', { cmd: p.cmd })
    : fill(lang === 'tr' ? '“{cmd}” başlatılamadı. Yapılandırmadaki komutu ve yolunu kontrol edin.' : 'Could not start “{cmd}”. Check the command and its path in the configuration.', p)),
  mcp_no_command: ['The server’s configuration has no "command" (or "url").', 'Sunucunun yapılandırmasında "command" (veya "url") yok.'],
  mcp_not_running: ['The MCP server “{name}” is not running. Reload it in Settings → Developer.', '“{name}” MCP sunucusu çalışmıyor. Ayarlar → Geliştirici bölümünden yeniden yükleyin.'],
  mcp_timeout: ['The MCP server “{name}” didn’t answer in time ({method}).', '“{name}” MCP sunucusu zamanında yanıt vermedi ({method}).'],
  mcp_disconnected: ['The MCP server “{name}” disconnected.', '“{name}” MCP sunucusunun bağlantısı kesildi.'],
  mcp_session_failed: ['The MCP server “{name}” ended its session, and a new one couldn’t be started.', '“{name}” MCP sunucusu oturumunu sonlandırdı ve yenisi başlatılamadı.'],
  mcp_gave_up: (p, lang) => (lang === 'tr' ? `“${p.name}” MCP sunucusu durmaya devam etti; ${p.n} kez yeniden başlatıldıktan sonra durdurulmuş bırakıldı.` : `The MCP server “${p.name}” kept stopping; it was left stopped after ${p.n} ${many(p.n, 'restart', 'restarts')}.`),
  mcp_not_connected: ['The MCP server isn’t connected.', 'MCP sunucusu bağlı değil.'],
  mcp_no_response: ['The MCP server returned no response.', 'MCP sunucusu yanıt döndürmedi.'],
  mcp_error: ['The MCP server reported an error.', 'MCP sunucusu bir hata bildirdi.'],
  mcp_tool_missing: ['The tool {tool} isn’t available.', '{tool} aracı kullanılamıyor.'],
  mcp_app_call_denied: ['{server} doesn’t let its app call {tool}.', '{server}, uygulamasının {tool} aracını çağırmasına izin vermiyor.'],
  // an artifact's window.claude.callMcpTool (mcp/artifact_tools.rs)
  artifact_tool_server_off: ['{server} is turned off in this chat.', '{server} bu sohbette kapalı.'],
  artifact_tool_off: ['The tool {tool} is turned off in this chat.', '{tool} aracı bu sohbette kapalı.'],
  artifact_tool_plan_mode: ['{tool} can make changes, and plan mode doesn’t run such tools.', '{tool} değişiklik yapabilir; plan modu böyle araçları çalıştırmaz.'],
  artifact_tool_blocked: ['{tool} is blocked in your connector settings.', '{tool}, bağlayıcı ayarlarınızda engellenmiş.'],
  artifact_tool_unknown: ['There’s no connected tool named {name}.', '{name} adında bağlı bir araç yok.'],
  artifact_tool_args_not_object: ['The tool’s arguments must be an object.', 'Aracın bağımsız değişkenleri bir nesne olmalı.'],
  artifact_tool_args_too_large: ['The tool’s arguments are larger than 256 KB.', 'Aracın bağımsız değişkenleri 256 KB’tan büyük.'],
  artifact_tool_not_allowed: ['You didn’t allow this call.', 'Bu çağrıya izin vermediniz.'],
  artifact_tool_result_too_large: ['The tool’s result is too large for the artifact (over 2 MB).', 'Aracın sonucu artifact için fazla büyük (2 MB’tan fazla).'],
  mcp_config_not_object: ['{path} must contain a JSON object.', '{path} bir JSON nesnesi içermeli.'],
  mcp_secret_missing: ['A saved secret of this server isn’t in the system’s credential store any more: enter it again in its settings.', 'Bu sunucunun kayıtlı bir gizli bilgisi artık sistemin kimlik deposunda yok: ayarlarından yeniden girin.'],
  needs_node: ['This server needs Node.js, which isn’t installed. Download it in Settings → Developer, or install it from https://nodejs.org.', 'Bu sunucu Node.js gerektiriyor ve Node.js kurulu değil. Ayarlar → Geliştirici bölümünden indirin veya https://nodejs.org adresinden kurun.'],
  needs_npx: ['This server runs npx, which comes with Node.js. Install Node.js from https://nodejs.org, then restart the servers.', 'Bu sunucu Node.js ile gelen npx’i çalıştırıyor. https://nodejs.org adresinden Node.js’i kurun, sonra sunucuları yeniden başlatın.'],
  needs_python: ['This server needs Python 3, which isn’t installed. Install it from https://www.python.org/downloads/, then restart the servers.', 'Bu sunucu Python 3 gerektiriyor ve Python kurulu değil. https://www.python.org/downloads/ adresinden kurun, sonra sunucuları yeniden başlatın.'],
  needs_uv: ['This server needs uv, which isn’t installed. Install it from https://docs.astral.sh/uv/, then restart the servers.', 'Bu sunucu uv gerektiriyor ve uv kurulu değil. https://docs.astral.sh/uv/ adresinden kurun, sonra sunucuları yeniden başlatın.'],
  node_busy: ['Node.js is already being downloaded.', 'Node.js zaten indiriliyor.'],
  node_unsupported: ['nodejs.org has no Node.js build for this computer’s system.', 'nodejs.org’da bu bilgisayarın sistemi için bir Node.js derlemesi yok.'],
  node_bad_archive: ['The Node.js archive isn’t as expected; nothing was installed.', 'Node.js arşivi beklendiği gibi değil; hiçbir şey kurulmadı.'],
  node_checksum_list: ['nodejs.org lists another checksum than this app was built with; nothing was downloaded.', 'nodejs.org, bu uygulamanın beklediğinden farklı bir sağlama toplamı listeliyor; hiçbir şey indirilmedi.'],
  node_checksum: ['The download doesn’t have the checksum it should; it was deleted and nothing was installed.', 'İndirilen dosyanın sağlama toplamı olması gerektiği gibi değil; dosya silindi ve hiçbir şey kurulmadı.'],
  download_too_large: ['The download is larger than expected and was stopped.', 'İndirme beklenenden büyük olduğu için durduruldu.'],
  // desktop extensions
  ext_not_zip: ['Not a valid extension file (not a zip archive, or a damaged one).', 'Geçerli bir eklenti dosyası değil (zip arşivi değil ya da bozuk).'],
  ext_no_manifest: ['The extension has no manifest.json.', 'Eklentinin manifest.json dosyası yok.'],
  ext_manifest_large: ['The extension’s manifest.json is too large (over 1 MB).', 'Eklentinin manifest.json dosyası çok büyük (1 MB’tan fazla).'],
  ext_manifest_json: ['The extension’s manifest.json isn’t valid JSON.', 'Eklentinin manifest.json dosyası geçerli bir JSON değil.'],
  ext_manifest_fields: ['manifest.json must define "name" and "server".', 'manifest.json "name" ve "server" alanlarını tanımlamalı.'],
  ext_too_large: ['The extension unpacks to more than the allowed size; it was not installed.', 'Eklenti açıldığında izin verilen boyutu aşıyor; kurulmadı.'],
  ext_signature_invalid: ['The extension’s signature doesn’t match the file: it was changed after it was signed. It was not installed.', 'Eklentinin imzası dosyayla eşleşmiyor: imzalandıktan sonra değiştirilmiş. Kurulmadı.'],
  ext_no_command: ['The extension does not define a command to run.', 'Eklenti çalıştırılacak bir komut tanımlamıyor.'],
  // updates and settings files
  update_unavailable: ['Updates aren’t available in this build.', 'Bu sürümde güncellemeler kullanılamıyor.'],
  update_check_failed: ['Couldn’t check for updates.', 'Güncellemeler denetlenemedi.'],
  update_busy: ['An update is already being installed.', 'Bir güncelleme zaten kuruluyor.'],
  update_none: ['ClaudeRust is up to date.', 'ClaudeRust güncel.'],
  update_install_failed: ['Couldn’t install the update.', 'Güncelleme kurulamadı.'],
  settings_file_too_large: ['That file is too large to be a settings file.', 'Bu dosya bir ayarlar dosyası olamayacak kadar büyük.'],
  settings_file_invalid: ['That isn’t a ClaudeRust settings file.', 'Bu bir ClaudeRust ayarlar dosyası değil.'],
};

/** The sentence for a failure (or notice) the backend names by its code, in the UI's language; null for one it doesn't name. */
export function codedMessage(e) {
  if (!e || typeof e !== 'object' || typeof e.kind !== 'string') return null;
  const p = e.params && typeof e.params === 'object' ? e.params : {};
  const lang = currentLanguage();
  const entry = Object.hasOwn(ERROR_TEXT, e.kind) ? ERROR_TEXT[e.kind] : null;
  // the codes memory_synth.rs gives are worded in i18n.js
  const s = typeof entry === 'function' ? entry(p, lang) : entry ? fill(entry[lang === 'tr' ? 1 : 0], p) : tOr(`errk_${e.kind}`, null) && t(`errk_${e.kind}`, p);
  // a sentence its params didn't reach (an older caller dropped them) is worse than the backend's own text
  return s && !/\{\w+\}|undefined|NaN/.test(s) ? s : null;
}
