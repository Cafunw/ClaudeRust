//! The files attached to a chat, read back: for the preview of an
//! attachment (staged in the composer or sent), its Open and Download, and
//! for window.fs.readFile in the chat's artifacts. Only what the app holds
//! for that attachment is read (the composer's staged copy, or the stored
//! message), never a path on disk.

use crate::attachments::Attachment;
use crate::db::{Db, Message};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use base64::Engine;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, State};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
/// The most one window.fs.readFile returns (a PDF, the largest attachment, is at most 22 MB).
const MAX_READ_BYTES: usize = 24 * 1024 * 1024;
/// What the system's app may be given as it is; any other attachment opens as text.
const OPENS_AS_IS: &[&str] = &["pdf", "png", "jpg", "jpeg", "gif", "webp", "txt", "md", "markdown", "csv", "tsv", "json", "log"];
/// Files written for Open are removed after this long (the app showing them has read them by then).
const OPENED_KEPT_MS: u128 = 60 * 60 * 1000;

/// An attachment as the app keeps it.
#[derive(Debug, Clone, PartialEq)]
struct Stored {
    name: String,
    /// image | pdf | text
    kind: String,
    mime: String,
    /// base64 of an image or PDF, else the text
    data: String,
    /// the text is a digest of a larger one
    digest: bool,
}

impl Stored {
    fn of(a: &Attachment) -> Stored {
        Stored { name: a.name.clone(), kind: a.kind.clone(), mime: a.mime.clone(), data: a.data.clone(), digest: a.digest.is_some() }
    }

    fn bytes(&self) -> AppResult<Vec<u8>> {
        if self.kind == "text" {
            return Ok(self.data.clone().into_bytes());
        }
        B64.decode(&self.data).map_err(|_| AppError::coded("attachment_damaged", Value::Null, "This attachment's data is damaged."))
    }

    /// The name of the file saved or opened: an image's extension is its
    /// format's (a downscaled PNG can be a JPEG), and text taken out of a
    /// document, or a digest, gets ".txt" (it isn't the original file).
    fn file_name(&self) -> String {
        let (stem, ext) = self.name.rsplit_once('.').unwrap_or((&self.name, ""));
        let ext = ext.to_ascii_lowercase();
        match self.kind.as_str() {
            "image" => {
                let want = self.mime.strip_prefix("image/").unwrap_or("png").replace("jpeg", "jpg");
                if ext == want || (ext == "jpeg" && want == "jpg") { self.name.clone() } else { format!("{stem}.{want}") }
            }
            "pdf" => self.name.clone(),
            _ if self.digest => format!("{stem} (digest).txt"),
            _ if mime_guess::from_path(&self.name).first_or_text_plain().essence_str() == self.mime => self.name.clone(),
            _ => format!("{}.txt", self.name),
        }
    }
}

/// The attachments of a stored message, in the order its meta lists them:
/// its first content blocks hold them. None for one without its data (an
/// import's missing file) or whose block isn't of its kind.
fn of_message(m: &Message) -> Vec<Option<Stored>> {
    let blocks = m.content.as_array().map(Vec::as_slice).unwrap_or_default();
    let metas = m.meta["attachments"].as_array().map(Vec::as_slice).unwrap_or_default();
    metas
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let b = blocks.get(i)?;
            let kind = a["kind"].as_str()?;
            let source = (b["type"].as_str()?, b["source"]["type"].as_str()?);
            let fits = matches!((kind, source), ("image", ("image", "base64")) | ("pdf", ("document", "base64")) | ("text", ("document", "text")));
            fits.then(|| Stored {
                name: a["name"].as_str().unwrap_or("file").to_string(),
                kind: kind.to_string(),
                mime: a["mime"].as_str().unwrap_or(if kind == "pdf" { "application/pdf" } else { "text/plain" }).to_string(),
                data: b["source"]["data"].as_str().unwrap_or_default().to_string(),
                digest: a["digest"].is_object(),
            })
        })
        .collect()
}

/// The file `name` (or a path ending in it) attached to conversation
/// `conv_id` on the branch it shows, the latest when several have that name.
fn conversation_file(db: &Db, conv_id: &str, name: &str) -> AppResult<Stored> {
    let want = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let conv = db.conv_get(conv_id)?;
    let path = match &conv.current_leaf {
        Some(leaf) => db.path_to(leaf)?,
        None => vec![],
    };
    let mut names: Vec<String> = vec![];
    for m in path.iter().rev().filter(|m| m.role == "user") {
        for s in of_message(m).into_iter().flatten() {
            if s.name == want {
                return Ok(s);
            }
            if !names.contains(&s.name) {
                names.push(s.name);
            }
        }
    }
    let attached = if names.is_empty() { "No files are attached to this chat.".to_string() } else { format!("The files attached to this chat are: {}.", names.join(", ")) };
    Err(AppError::coded("artifact_file_not_found", json!({"name": name}), format!("No file named \"{name}\" is attached to this chat. {attached}")))
}

/// What window.fs.readFile(name, {encoding}) resolves to: `{text}` with
/// `encoding` "utf8", else `{base64}` of the bytes (the page gets a Uint8Array).
fn read_file(db: &Db, conv_id: &str, name: &str, encoding: Option<&str>) -> AppResult<Value> {
    let utf8 = match encoding.map(str::to_ascii_lowercase).as_deref() {
        None => false,
        Some("utf8" | "utf-8") => true,
        Some(other) => {
            let message = format!("window.fs.readFile reads bytes, or text with {{ encoding: 'utf8' }}, not \"{other}\".");
            return Err(AppError::coded("artifact_read_encoding", json!({"name": other}), message));
        }
    };
    let s = conversation_file(db, conv_id, name)?;
    let size = if s.kind == "text" { s.data.len() } else { s.data.len() / 4 * 3 };
    if size > MAX_READ_BYTES {
        let message = format!("\"{name}\" is too large to read in an artifact ({} at most).", crate::util::human_size(MAX_READ_BYTES as u64));
        return Err(AppError::coded("artifact_file_too_large", json!({"name": name}), message));
    }
    Ok(match (utf8, s.kind.as_str()) {
        (true, "text") => json!({"text": s.data}),
        (true, _) => json!({"text": String::from_utf8_lossy(&s.bytes()?)}),
        (false, "text") => json!({"base64": B64.encode(s.data.as_bytes())}),
        (false, _) => json!({"base64": s.data}),
    })
}

/// The attachment staged in the composer as `staged_id`, or the `index`th
/// of message `message_id`.
fn find(st: &AppState, staged_id: Option<&str>, message_id: Option<&str>, index: usize) -> AppResult<Stored> {
    let found = match (staged_id, message_id) {
        (Some(id), _) => st.staged.lock().get(id).map(Stored::of),
        (None, Some(m)) => of_message(&st.db.msg_get(m)?).into_iter().nth(index).flatten(),
        (None, None) => None,
    };
    found.ok_or_else(|| match message_id {
        // an imported chat lists files it has no data of
        Some(_) if staged_id.is_none() => AppError::coded("attachment_not_kept", Value::Null, "Only this file's name was kept with the message: the chat was imported without it."),
        _ => AppError::coded("attachment_gone", Value::Null, "An attachment is no longer available. Remove it and attach it again."),
    })
}

type St<'a> = State<'a, Arc<AppState>>;

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| AppError::Io(std::io::Error::other(e)))?
}

/// window.fs.readFile of an artifact shown in conversation `conversation_id`
/// (the app tells which: the page can't name another conversation).
#[tauri::command]
pub async fn artifact_read_file(state: St<'_>, conversation_id: String, name: String, encoding: Option<String>) -> AppResult<Value> {
    let st = state.inner().clone();
    blocking(move || read_file(&st.db, &conversation_id, &name, encoding.as_deref())).await
}

/// The token of the PDF last served for a preview: one at a time is kept.
static SERVED_PDF: parking_lot::Mutex<Option<String>> = parking_lot::const_mutex(None);

/// Keep `pdf` for the `artifact://` protocol (the app's page may frame
/// nothing else, so the WebView's PDF viewer shows it from there) in place
/// of the one served before; its token.
fn serve_pdf(served: &mut std::collections::HashMap<String, (String, Vec<u8>)>, pdf: Vec<u8>) -> String {
    // sorts after the artifacts' tokens: theirs go first when that map is full
    let token = format!("pdf-{}", crate::util::new_id());
    if let Some(old) = SERVED_PDF.lock().replace(token.clone()) {
        served.remove(&old);
    }
    served.insert(token.clone(), ("application/pdf".into(), pdf));
    token
}

/// An attachment for its preview: `{name, kind, mime, digest}` and its
/// `data` as in `Stored`, or for a PDF the `token` it's served under.
#[tauri::command]
pub async fn attachment_data(state: St<'_>, staged_id: Option<String>, message_id: Option<String>, index: Option<usize>) -> AppResult<Value> {
    let st = state.inner().clone();
    blocking(move || {
        let s = find(&st, staged_id.as_deref(), message_id.as_deref(), index.unwrap_or(0))?;
        let mut out = json!({"name": s.name, "kind": s.kind, "mime": s.mime, "digest": s.digest});
        if s.kind == "pdf" {
            out["token"] = json!(serve_pdf(&mut st.served.lock(), s.bytes()?));
        } else {
            out["data"] = json!(s.data);
        }
        Ok(out)
    })
    .await
}

/// Save an attachment where the person picks; false when they cancel.
#[tauri::command]
pub async fn attachment_save(app: AppHandle, state: St<'_>, staged_id: Option<String>, message_id: Option<String>, index: Option<usize>) -> AppResult<bool> {
    use tauri_plugin_dialog::DialogExt;
    let st = state.inner().clone();
    let s = blocking(move || find(&st, staged_id.as_deref(), message_id.as_deref(), index.unwrap_or(0))).await?;
    let bytes = s.bytes()?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_file_name(s.file_name()).save_file(move |p| {
        let _ = tx.send(p);
    });
    let Some(path) = rx.await.ok().flatten() else { return Ok(false) };
    let path = path.into_path().map_err(|e| AppError::Io(std::io::Error::other(e.to_string())))?;
    tokio::fs::write(path, bytes).await?;
    Ok(true)
}

/// The file Open writes for `s` in `dir` (a folder of its own under it): as
/// it is when its type is in `OPENS_AS_IS`, else as text (a .bat or .html
/// attachment would run, not show).
fn write_for_open(dir: &std::path::Path, s: &Stored) -> AppResult<std::path::PathBuf> {
    let mut name = s.file_name().replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "-");
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if !OPENS_AS_IS.contains(&ext.as_str()) {
        name.push_str(".txt");
    }
    let folder = dir.join(crate::util::new_id());
    std::fs::create_dir_all(&folder)?;
    let path = folder.join(name);
    std::fs::write(&path, s.bytes()?)?;
    Ok(path)
}

/// Remove what Open wrote over `OPENED_KEPT_MS` ago.
fn sweep_opened(dir: &std::path::Path) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_millis() > OPENED_KEPT_MS);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Open an attachment in the system's app for its type, from a copy in the app's data.
#[tauri::command]
pub async fn attachment_open(app: AppHandle, state: St<'_>, staged_id: Option<String>, message_id: Option<String>, index: Option<usize>) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let st = state.inner().clone();
    let path = blocking(move || {
        let s = find(&st, staged_id.as_deref(), message_id.as_deref(), index.unwrap_or(0))?;
        let dir = st.data_dir.join("opened");
        sweep_opened(&dir);
        write_for_open(&dir, &s)
    })
    .await?;
    crate::commands::os_open(&path.to_string_lossy(), || app.opener().open_path(path.to_string_lossy(), None::<&str>))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str, bytes: &[u8]) -> Attachment {
        crate::attachments::from_bytes(name, bytes).unwrap()
    }

    /// A user message carrying `atts` as `send_message` stores it, then a line of text.
    fn message(db: &Db, conv: &str, parent: Option<&str>, atts: &[Attachment]) -> String {
        let mut content: Vec<Value> = atts.iter().map(crate::attachments::to_block).collect();
        content.push(json!({"type": "text", "text": "look"}));
        let m = Message {
            id: crate::util::new_id(),
            conversation_id: conv.into(),
            parent_id: parent.map(str::to_string),
            role: "user".into(),
            content: Value::Array(content),
            meta: json!({"attachments": atts.iter().map(crate::attachments::display_meta).collect::<Vec<_>>()}),
            created_at: crate::util::now_ms(),
        };
        db.msg_insert(&m).unwrap();
        db.conv_set_leaf(conv, Some(&m.id)).unwrap();
        m.id
    }

    fn png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn read_file_answers_by_name_in_the_same_conversation_only() {
        let db = Db::open_in_memory().unwrap();
        let a = db.conv_create("chat", "m", None, None, &json!({})).unwrap().id;
        let b = db.conv_create("chat", "m", None, None, &json!({})).unwrap().id;
        let first = message(&db, &a, None, &[att("data.csv", b"x,y\n1,2\n"), att("pic.png", &png())]);
        message(&db, &a, Some(&first), &[att("data.csv", b"x,y\n3,4\n"), att("doc.pdf", b"%PDF-1.4 hi")]);
        message(&db, &b, None, &[att("secret.txt", b"other chat")]);
        // text as text, or its bytes; the latest file of a name; a path ending in the name
        assert_eq!(read_file(&db, &a, "data.csv", Some("utf8")).unwrap()["text"], "x,y\n3,4\n");
        assert_eq!(read_file(&db, &a, "/mnt/user-data/data.csv", Some("UTF-8")).unwrap()["text"], "x,y\n3,4\n");
        assert_eq!(read_file(&db, &a, "data.csv", None).unwrap()["base64"], B64.encode("x,y\n3,4\n"));
        // binary as its bytes
        assert_eq!(B64.decode(read_file(&db, &a, "doc.pdf", None).unwrap()["base64"].as_str().unwrap()).unwrap(), b"%PDF-1.4 hi");
        assert_eq!(read_file(&db, &a, "doc.pdf", Some("utf8")).unwrap()["text"], "%PDF-1.4 hi");
        assert!(B64.decode(read_file(&db, &a, "pic.png", None).unwrap()["base64"].as_str().unwrap()).unwrap().starts_with(b"\x89PNG"));
        // another conversation's files are not found, and the error names this one's
        let e = read_file(&db, &a, "secret.txt", None).unwrap_err();
        assert_eq!(e.kind(), "artifact_file_not_found");
        assert!(e.to_string().contains("data.csv, doc.pdf, pic.png") && !e.to_string().contains("other chat"), "{e}");
        assert!(read_file(&db, &b, "data.csv", None).is_err());
        // a conversation with no messages, an unknown one, an encoding not offered
        let empty = db.conv_create("chat", "m", None, None, &json!({})).unwrap().id;
        assert!(read_file(&db, &empty, "x", None).unwrap_err().to_string().contains("No files are attached"));
        assert!(read_file(&db, "nope", "data.csv", None).is_err());
        assert!(read_file(&db, &a, "data.csv", Some("latin1")).unwrap_err().to_string().contains("utf8"));
        // only the branch the conversation shows: back on the first message, the first data.csv
        db.conv_set_leaf(&a, Some(&first)).unwrap();
        assert_eq!(read_file(&db, &a, "data.csv", Some("utf8")).unwrap()["text"], "x,y\n1,2\n");
        assert!(read_file(&db, &a, "doc.pdf", None).is_err());
    }

    #[test]
    fn the_artifact_prompt_tells_claude_how_to_read_attached_files() {
        let info = crate::prompts::ARTIFACTS_INFO;
        assert!(info.contains("window.fs.readFile(name, { encoding: 'utf8' })") && info.contains("window.fs.readFile(name)"));
    }

    #[test]
    fn reads_are_capped() {
        let db = Db::open_in_memory().unwrap();
        let c = db.conv_create("chat", "m", None, None, &json!({})).unwrap().id;
        let mut big = att("big.pdf", b"%PDF-1.4");
        big.data = "A".repeat(MAX_READ_BYTES / 3 * 4 + 8);
        message(&db, &c, None, &[big]);
        assert!(read_file(&db, &c, "big.pdf", None).unwrap_err().to_string().contains("too large"));
    }

    #[test]
    fn message_attachments_pair_with_their_blocks() {
        let atts = [att("a.txt", b"hello"), att("b.pdf", b"%PDF-1.4")];
        let m = Message {
            id: "m".into(),
            conversation_id: "c".into(),
            parent_id: None,
            role: "user".into(),
            content: json!([crate::attachments::to_block(&atts[0]), crate::attachments::to_block(&atts[1]), {"type": "text", "text": "hi"}]),
            meta: json!({"attachments": [crate::attachments::display_meta(&atts[0]), crate::attachments::display_meta(&atts[1]), {"name": "gone.png", "kind": "file", "missing": true}]}),
            created_at: 0,
        };
        let got = of_message(&m);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].as_ref().unwrap().data, "hello");
        assert_eq!(got[1].as_ref().unwrap().kind, "pdf");
        // a listed file without data (an import's) has none, and never takes the text block's
        assert!(got[2].is_none());
        // a block of another kind than its listing isn't taken for it
        let mut odd = m.clone();
        odd.meta["attachments"][0]["kind"] = json!("image");
        assert!(of_message(&odd)[0].is_none());
        assert!(of_message(&Message { meta: json!({}), ..m }).is_empty());
    }

    #[test]
    fn one_previewed_pdf_is_served_at_a_time() {
        let mut served = std::collections::HashMap::new();
        served.insert("0000000000000001-x".to_string(), ("text/html".to_string(), vec![]));
        let first = serve_pdf(&mut served, b"%PDF-1".to_vec());
        let second = serve_pdf(&mut served, b"%PDF-2".to_vec());
        assert!(!served.contains_key(&first) && served[&second] == ("application/pdf".to_string(), b"%PDF-2".to_vec()));
        assert_eq!(served.len(), 2, "an artifact's document is left alone");
        assert!(served.keys().min().unwrap() < &second, "the artifacts' tokens go first when the map is full");
    }

    #[test]
    fn saved_and_opened_names() {
        let s = |name: &str, kind: &str, mime: &str, digest: bool| Stored { name: name.into(), kind: kind.into(), mime: mime.into(), data: String::new(), digest };
        assert_eq!(s("photo.png", "image", "image/jpeg", false).file_name(), "photo.jpg");
        assert_eq!(s("photo.JPEG", "image", "image/jpeg", false).file_name(), "photo.JPEG");
        assert_eq!(s("a.png", "image", "image/png", false).file_name(), "a.png");
        assert_eq!(s("r.pdf", "pdf", "application/pdf", false).file_name(), "r.pdf");
        assert_eq!(s("notes.md", "text", "text/markdown", false).file_name(), "notes.md");
        assert_eq!(s("README", "text", "text/plain", false).file_name(), "README");
        assert_eq!(s("report.docx", "text", "text/plain", false).file_name(), "report.docx.txt");
        assert_eq!(s("app.log", "text", "text/plain", true).file_name(), "app (digest).txt");
        // what Open writes never runs: a script opens as text
        let dir = std::env::temp_dir().join(format!("opened-{}", crate::util::new_id()));
        let open = |st: Stored| write_for_open(&dir, &st).unwrap();
        let bat = open(Stored { data: "echo hi".into(), ..s("run.bat", "text", "application/x-msdownload", false) });
        assert!(bat.ends_with("run.bat.txt") && std::fs::read_to_string(&bat).unwrap() == "echo hi");
        assert!(open(s("page.html", "text", "text/html", false)).ends_with("page.html.txt"));
        assert!(open(s("..\\..\\x.csv", "text", "text/csv", false)).file_name().unwrap() == "..-..-x.csv");
        let pdf = open(Stored { data: B64.encode(b"%PDF"), ..s("r.pdf", "pdf", "application/pdf", false) });
        assert_eq!(std::fs::read(&pdf).unwrap(), b"%PDF");
        assert!(pdf.starts_with(&dir));
        sweep_opened(&dir);
        assert!(pdf.exists(), "kept for an hour");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
