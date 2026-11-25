//! Artifacts: parsing `<antArtifact …>` blocks out of assistant text and
//! building the sandboxed documents served on the `artifact://` protocol.

use crate::util::escape_html;
use std::sync::LazyLock;
use regex::Regex;
use serde::Serialize;

pub const OPEN_TAG: &str = "<antArtifact";
pub const CLOSE_TAG: &str = "</antArtifact>";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArtifactSpec {
    pub identifier: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub language: Option<String>,
    pub content: String,
    pub complete: bool,
    /// `command="update"`: `content` holds `<old_str>`/`<new_str>` pairs for
    /// the previous version, applied by the panel (ui/js/artifact_update.js).
    pub update: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Markdown(String),
    Artifact(ArtifactSpec),
}

static ATTR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"([A-Za-z_][\w-]*)\s*=\s*"([^"]*)""#).unwrap());

fn unescape_attr(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Length of the longest suffix of `s` that is a prefix of `pat` (for hiding
/// half-streamed tags).
fn partial_suffix(s: &str, pat: &str) -> usize {
    let max = pat.len().min(s.len());
    for n in (1..=max).rev() {
        if s.is_char_boundary(s.len() - n) && pat.starts_with(&s[s.len() - n..]) {
            return n;
        }
    }
    0
}

/// The typed artifacts, as Claude Desktop's Slides, Docs, Sheets and
/// Dashboard: data drawn by a runtime of the app's own (ui/js/artifact_types),
/// which the person can edit in place (an edit is a new version, `artifact_edit`).
pub const SLIDES: &str = "application/vnd.ant.slides";
pub const DOC: &str = "application/vnd.ant.document";
pub const SHEET: &str = "application/vnd.ant.sheet";
pub const DASHBOARD: &str = "application/vnd.ant.dashboard";

pub fn is_typed(kind: &str) -> bool {
    matches!(kind, SLIDES | DOC | SHEET | DASHBOARD)
}

fn normalize_kind(kind: &str, language: &Option<String>) -> String {
    match kind {
        "slides" | "deck" | "presentation" => SLIDES.into(),
        "doc" | "docs" | "document" => DOC.into(),
        "sheet" | "sheets" | "spreadsheet" => SHEET.into(),
        "dashboard" => DASHBOARD.into(),
        "" => {
            if language.is_some() {
                "application/vnd.ant.code".into()
            } else {
                "text/markdown".into()
            }
        }
        "html" => "text/html".into(),
        "svg" => "image/svg+xml".into(),
        "react" | "jsx" => "application/vnd.ant.react".into(),
        "mermaid" => "application/vnd.ant.mermaid".into(),
        "markdown" | "md" => "text/markdown".into(),
        "code" => "application/vnd.ant.code".into(),
        k => k.to_string(),
    }
}

/// Strip a single wrapping ``` fence that models sometimes put inside artifacts.
fn strip_fence(content: &str) -> String {
    let t = content.trim_matches('\n');
    if let Some(rest) = t.strip_prefix("```") {
        if let Some(nl) = rest.find('\n') {
            let body = &rest[nl + 1..];
            let body = body.trim_end();
            if let Some(b) = body.strip_suffix("```") {
                return b.trim_end_matches('\n').to_string();
            }
            return body.to_string();
        }
    }
    t.to_string()
}

/// Split assistant text into markdown and artifact segments. Works on
/// partially streamed text: an unterminated artifact is returned with
/// `complete: false`.
pub fn split(text: &str) -> Vec<Segment> {
    let mut out = vec![];
    let mut rest = text;
    loop {
        match rest.find(OPEN_TAG) {
            None => {
                let hide = partial_suffix(rest, OPEN_TAG);
                let md = &rest[..rest.len() - hide];
                if !md.is_empty() {
                    out.push(Segment::Markdown(md.to_string()));
                }
                break;
            }
            Some(i) => {
                if i > 0 {
                    out.push(Segment::Markdown(rest[..i].to_string()));
                }
                let after = &rest[i + OPEN_TAG.len()..];
                let (attrs_str, body, header_done) = match after.find('>') {
                    Some(j) => (&after[..j], &after[j + 1..], true),
                    None => (after, "", false),
                };
                let mut identifier = String::new();
                let mut kind = String::new();
                let mut title = String::new();
                let mut language = None;
                let mut update = false;
                for cap in ATTR_RE.captures_iter(attrs_str) {
                    let v = unescape_attr(&cap[2]);
                    match &cap[1] {
                        "identifier" => identifier = v,
                        "type" => kind = v,
                        "title" => title = v,
                        "language" => language = Some(v).filter(|s: &String| !s.is_empty()),
                        "command" => update = v == "update",
                        _ => {}
                    }
                }
                let kind = normalize_kind(&kind, &language);
                if identifier.is_empty() {
                    identifier = if title.is_empty() {
                        "artifact".into()
                    } else {
                        title
                            .to_lowercase()
                            .chars()
                            .map(|c| if c.is_alphanumeric() { c } else { '-' })
                            .collect::<String>()
                            .trim_matches('-')
                            .to_string()
                    };
                }
                if !header_done {
                    out.push(Segment::Artifact(ArtifactSpec {
                        identifier,
                        kind,
                        title,
                        language,
                        content: String::new(),
                        complete: false,
                        update,
                    }));
                    break;
                }
                // opened again before it closed (a reply stopped mid-artifact and continued in the same text,
                // engine.rs join_blocks, may start it over or update it): this one ends there, cut off
                if let Some(o) = body.find(OPEN_TAG).filter(|&o| body.find(CLOSE_TAG).is_none_or(|k| o < k)) {
                    out.push(Segment::Artifact(ArtifactSpec { identifier, kind, title, language, content: strip_fence(&body[..o]), complete: false, update }));
                    rest = &body[o..];
                    continue;
                }
                match body.find(CLOSE_TAG) {
                    Some(k) => {
                        let content = strip_fence(&body[..k]);
                        out.push(Segment::Artifact(ArtifactSpec {
                            identifier,
                            kind,
                            title,
                            language,
                            content,
                            complete: true,
                            update,
                        }));
                        rest = &body[k + CLOSE_TAG.len()..];
                    }
                    None => {
                        let hide = partial_suffix(body, CLOSE_TAG);
                        let content = strip_fence(&body[..body.len() - hide]);
                        out.push(Segment::Artifact(ArtifactSpec {
                            identifier,
                            kind,
                            title,
                            language,
                            content,
                            complete: false,
                            update,
                        }));
                        break;
                    }
                }
            }
        }
    }
    out
}

static EDIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<old_str>(.*?)</old_str>\s*<new_str>(.*?)</new_str>").unwrap());

/// `content` with the `<old_str>`/`<new_str>` edits of an update applied in
/// order, all or none, as ui/js/artifact_update.js applies them in the panel
/// (keep the two alike): the gallery shows the result too.
pub fn apply_update(content: &str, body: &str) -> Result<String, String> {
    // a newline right inside a tag is layout, not text
    let inner = |s: &str| {
        let s = s.strip_prefix("\r\n").or_else(|| s.strip_prefix('\n')).unwrap_or(s);
        s.strip_suffix("\r\n").or_else(|| s.strip_suffix('\n')).unwrap_or(s).to_string()
    };
    let edits: Vec<(String, String)> = EDIT_RE.captures_iter(body).map(|c| (inner(&c[1]), inner(&c[2]))).collect();
    if edits.is_empty() {
        return Err("The update has no <old_str>…</old_str><new_str>…</new_str> pair.".into());
    }
    // the model writes \n; an artifact that came in with \r\n is edited in kind
    let crlf = content.contains("\r\n");
    let eol = |s: &str| if crlf { s.replace("\r\n", "\n").replace('\n', "\r\n") } else { s.to_string() };
    let mut out = content.to_string();
    for (i, (old, new)) in edits.iter().enumerate() {
        let n = if edits.len() > 1 { format!(" {}", i + 1) } else { String::new() };
        let old = eol(old);
        if old.is_empty() {
            return Err(format!("old_str{n} is empty."));
        }
        let at = out.find(&old).ok_or_else(|| format!("old_str{n} was not found in the current version."))?;
        // overlapping matches count: "aa" is twice in "aaa"
        let next = at + out[at..].chars().next().map_or(1, char::len_utf8);
        if out[next..].contains(&old) {
            return Err(format!("old_str{n} appears more than once in the current version."));
        }
        out.replace_range(at..at + old.len(), &eol(new));
    }
    Ok(out)
}

/// `a` as a version of its own: an update's edits applied to `base`, the
/// version before it (None when there is none or they don't apply), taking
/// its type, and its title unless it has one, as ui/js/artifact_update.js does.
pub fn resolve(base: Option<&ArtifactSpec>, a: ArtifactSpec) -> Option<ArtifactSpec> {
    if !a.update {
        return Some(a);
    }
    let base = base?;
    let content = apply_update(&base.content, &a.content).ok()?;
    Some(ArtifactSpec { content, kind: base.kind.clone(), language: base.language.clone(), update: false, title: if a.title.is_empty() { base.title.clone() } else { a.title }, ..a })
}

/// The latest complete version of `identifier` in `texts` (a branch's
/// assistant messages, oldest first), its updates applied.
pub fn latest_version<I: IntoIterator<Item = String>>(texts: I, identifier: &str) -> Option<ArtifactSpec> {
    // what an update edits is the version before, finished or not (a reply stopped, then continued), as in the panel
    let mut base: Option<ArtifactSpec> = None;
    let mut last = None;
    for text in texts {
        for s in split(&text) {
            let Segment::Artifact(a) = s else { continue };
            if a.identifier != identifier {
                continue;
            }
            let Some(a) = resolve(base.as_ref(), a) else { continue };
            if a.complete {
                last = Some(a.clone());
            }
            base = Some(a);
        }
    }
    last
}

/// A version the person wrote (an edit in place): `a` with `content`, marked
/// `source="user"` so Claude sees who changed it.
pub fn user_version(a: &ArtifactSpec, content: &str) -> String {
    let lang = a.language.as_deref().map(|l| format!(" language=\"{}\"", escape_html(l))).unwrap_or_default();
    // the edit can't close the tag early
    let body = content.replace(CLOSE_TAG, "&lt;/antArtifact>");
    format!(
        "<antArtifact identifier=\"{}\" type=\"{}\" title=\"{}\"{lang} source=\"user\">\n{body}\n</antArtifact>",
        escape_html(&a.identifier),
        escape_html(&a.kind),
        escape_html(&a.title)
    )
}

/// Message content `content` with text `block` added: at the end, or before a
/// tool call (what follows one must be its result).
pub fn with_text_block(content: &serde_json::Value, block: String) -> serde_json::Value {
    use serde_json::{json, Value};
    let mut blocks = match content {
        Value::Array(a) => a.clone(),
        Value::String(s) if !s.is_empty() => vec![json!({"type": "text", "text": s})],
        _ => vec![],
    };
    let at = blocks.iter().position(|b| b["type"] == "tool_use").unwrap_or(blocks.len());
    blocks.insert(at, json!({"type": "text", "text": block}));
    Value::Array(blocks)
}

/// An edit in place is at most this long.
const MAX_EDIT_BYTES: usize = 2 * 1024 * 1024;

/// The person edited typed artifact `identifier` in place: `content` becomes
/// its newest version, added to the last reply of the branch shown, where
/// Claude reads it with the rest of the conversation (and the gallery keeps it).
#[tauri::command]
pub async fn artifact_edit(state: tauri::State<'_, std::sync::Arc<crate::state::AppState>>, conversation_id: String, identifier: String, content: String) -> crate::error::AppResult<()> {
    use crate::error::AppError;
    use serde_json::Value;
    if content.len() > MAX_EDIT_BYTES {
        return Err(AppError::coded("artifact_edit_too_large", Value::Null, "The edit is too large to save."));
    }
    if state.runs.lock().contains_key(&conversation_id) {
        return Err(AppError::coded("artifact_edit_busy", Value::Null, "Wait for Claude to finish, then save your edit."));
    }
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let missing = || AppError::coded("artifact_edit_missing", Value::Null, "The artifact is not in this conversation.");
        let conv = st.db.conv_get(&conversation_id)?;
        let path = st.db.path_to(&conv.current_leaf.ok_or_else(missing)?)?;
        let replies: Vec<_> = path.iter().filter(|m| m.role == "assistant").collect();
        let a = latest_version(replies.iter().map(|m| crate::render::plain_text(&m.content)), &identifier).ok_or_else(missing)?;
        if !is_typed(&a.kind) {
            return Err(AppError::coded("artifact_edit_untyped", Value::Null, "Only slides, docs, sheets and dashboards are edited in place."));
        }
        let last = replies.last().ok_or_else(missing)?;
        let new = with_text_block(&last.content, user_version(&a, &content));
        let mut meta = last.meta.clone();
        // the Claude account's CLI session holds the reply as it was: its next turn replays the conversation (claudecode.rs)
        if meta["cc_uuid"].is_string() {
            meta["cc_replay"] = serde_json::json!(true);
        }
        st.db.msg_update(&last.id, &new, &meta)?;
        let text = crate::render::plain_text(&new);
        crate::engine::save_artifacts(&st, &conversation_id, &last.id, &text);
        let _ = st.db.fts_index(&conversation_id, &last.id, &text);
        Ok(())
    })
    .await?
}

/// What exporting `content` (a `kind` artifact) as `format` writes: its file
/// extension and bytes. `source` is the artifact as it is; a Doc is also a web
/// page and a Word document, a deck a page that presents itself.
pub fn export_file(kind: &str, content: &str, title: &str, format: &str) -> Result<(String, Vec<u8>), String> {
    match (kind, format) {
        (_, "source") => Ok((extension(kind, None), content.as_bytes().to_vec())),
        (DOC, "html") => Ok(("html".into(), doc_page(content, title).into_bytes())),
        (DOC, "docx") => Ok(("docx".into(), crate::artifact_docx::docx(content, title)?)),
        (SLIDES, "html") => Ok(("html".into(), build_typed(SLIDES, content, false, "").into_bytes())),
        _ => Err(format!("A {} artifact can't be exported as {format}.", kind_label(kind, None))),
    }
}

/// A Doc as a web page of its own.
fn doc_page(md: &str, title: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title>{BASE_STYLE}<style>{TYPES_CSS}</style></head><body class=\"t-document light\"><article class=\"doc\">{}</article></body></html>",
        escape_html(title),
        doc_html(md)
    )
}

/// Save artifact `content` as `format` (export_file) where the person picks.
#[tauri::command]
pub async fn export_artifact(app: tauri::AppHandle, kind: String, content: String, title: String, format: String) -> crate::error::AppResult<bool> {
    use tauri_plugin_dialog::DialogExt;
    let (ext, bytes) = export_file(&kind, &content, &title, &format).map_err(|m| crate::error::AppError::coded("artifact_export_failed", serde_json::json!({ "format": format }), m))?;
    let base: String = title.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '-' } else { c }).collect();
    let name = format!("{}.{ext}", if base.trim().is_empty() { "artifact" } else { base.trim() });
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_file_name(&name).save_file(move |p| {
        let _ = tx.send(p.and_then(|p| p.into_path().ok()));
    });
    match rx.await.unwrap_or(None) {
        Some(p) => {
            tokio::fs::write(p, bytes).await?;
            Ok(true)
        }
        None => Ok(false),
    }
}

pub fn kind_label(kind: &str, language: Option<&str>) -> String {
    match kind {
        "text/html" => "HTML".into(),
        "image/svg+xml" => "SVG".into(),
        "application/vnd.ant.react" => "React".into(),
        "application/vnd.ant.mermaid" => "Diagram".into(),
        "text/markdown" => "Document".into(),
        SLIDES => "Slides".into(),
        DOC => "Doc".into(),
        SHEET => "Sheet".into(),
        DASHBOARD => "Dashboard".into(),
        "application/vnd.ant.code" => language
            .map(|l| {
                let mut c = l.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => "Code".into(),
                }
            })
            .unwrap_or_else(|| "Code".into()),
        _ => "Artifact".into(),
    }
}

pub fn is_previewable(kind: &str) -> bool {
    is_typed(kind) || matches!(kind, "text/html" | "image/svg+xml" | "application/vnd.ant.react" | "application/vnd.ant.mermaid")
}

/// File extension used when downloading an artifact.
pub fn extension(kind: &str, language: Option<&str>) -> String {
    match kind {
        "text/html" => "html".into(),
        "image/svg+xml" => "svg".into(),
        "application/vnd.ant.react" => "jsx".into(),
        "application/vnd.ant.mermaid" => "mmd".into(),
        "text/markdown" | DOC => "md".into(),
        SLIDES => "html".into(),
        SHEET => "csv".into(),
        DASHBOARD => "json".into(),
        _ => match language.unwrap_or("txt").to_ascii_lowercase().as_str() {
            "python" | "py" => "py".into(),
            "javascript" | "js" => "js".into(),
            "typescript" | "ts" => "ts".into(),
            "rust" | "rs" => "rs".into(),
            "bash" | "sh" | "shell" => "sh".into(),
            "c++" | "cpp" => "cpp".into(),
            "csharp" | "c#" => "cs".into(),
            "golang" | "go" => "go".into(),
            "markdown" => "md".into(),
            "yaml" | "yml" => "yaml".into(),
            l if l.len() <= 6 && l.chars().all(|c| c.is_ascii_alphanumeric()) => l.into(),
            _ => "txt".into(),
        },
    }
}

const BASE_STYLE: &str = r#"<style>:root{color-scheme:light dark}html,body{margin:0;padding:0}body{font-family:system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}</style>"#;

/// HTML is written against the browser's default of dark text on white, so a
/// fragment keeps that pair in the dark theme too: themed dark, a light box
/// it draws without a text colour of its own would come out white on white.
/// It also gets back the browser's own 8px margin (BASE_STYLE takes it away),
/// which a fragment is written against: without it, text touches the frame.
const FRAGMENT_STYLE: &str = "<style>:root{color-scheme:light}body{background:#ffffff;color:#141413}body{margin:8px}</style>";

/// Content Security Policy for artifact documents: they may load the
/// libraries the app names, each at one version (and Google Fonts), but reach
/// no other host, as in Claude Desktop, so a page can't send what it holds or
/// window.storage anywhere, nor pull in a package nobody chose. A source
/// ending in `/` is that package's folder, one without its file alone. The
/// esm.sh builds are the ones build_react maps, which pin their own
/// dependencies (a range would float to a version not listed here); the
/// cdnjs ones are those prompts.rs gives text/html artifacts. They run in an
/// opaque, sandboxed origin.
pub const ARTIFACT_CSP: &str = concat!(
    "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' blob:",
    " https://esm.sh/react@18.3.1 https://esm.sh/react@18.3.1/ https://esm.sh/react-dom@18.3.1 https://esm.sh/react-dom@18.3.1/ https://esm.sh/scheduler@0.23.2/",
    " https://esm.sh/lucide-react@0.468.0 https://esm.sh/lucide-react@0.468.0/ https://esm.sh/recharts@2.15.0 https://esm.sh/recharts@2.15.0/",
    " https://esm.sh/lodash@4.17.21 https://esm.sh/lodash@4.17.21/ https://esm.sh/d3@7.9.0 https://esm.sh/d3@7.9.0/ https://esm.sh/three@0.171.0 https://esm.sh/three@0.171.0/",
    " https://esm.sh/mathjs@14.0.1 https://esm.sh/mathjs@14.0.1/ https://esm.sh/node/ https://esm.sh/papaparse@5.4.1 https://esm.sh/papaparse@5.4.1/",
    " https://esm.sh/chart.js@4.4.7 https://esm.sh/chart.js@4.4.7/ https://esm.sh/framer-motion@11.15.0 https://esm.sh/framer-motion@11.15.0/",
    " https://esm.sh/clsx@2.1.1 https://esm.sh/clsx@2.1.1/ https://esm.sh/tailwind-merge@2.6.0 https://esm.sh/tailwind-merge@2.6.0/",
    " https://esm.sh/class-variance-authority@0.7.1 https://esm.sh/class-variance-authority@0.7.1/ https://esm.sh/date-fns@4.1.0 https://esm.sh/date-fns@4.1.0/",
    " https://unpkg.com/@babel/standalone@7.26.4/babel.min.js https://cdn.tailwindcss.com/3.4.16 https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/",
    " https://cdnjs.cloudflare.com/ajax/libs/Chart.js/4.4.1/ https://cdnjs.cloudflare.com/ajax/libs/d3/7.9.0/ https://cdnjs.cloudflare.com/ajax/libs/three.js/r128/",
    " https://cdnjs.cloudflare.com/ajax/libs/lodash.js/4.17.21/ https://cdnjs.cloudflare.com/ajax/libs/PapaParse/5.4.1/ https://cdnjs.cloudflare.com/ajax/libs/mathjs/13.2.0/",
    " https://cdnjs.cloudflare.com/ajax/libs/tone/14.8.49/ https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/;",
    " style-src 'unsafe-inline' https://fonts.googleapis.com; img-src data: blob:; font-src data: blob: https://fonts.gstatic.com;",
    " connect-src data: blob: https://fonts.googleapis.com https://fonts.gstatic.com; media-src data: blob:; frame-src 'none'; worker-src blob:; form-action 'none'; webrtc 'block'"
);

/// What an MCP App page is served as: its own policy (a meta tag mcp_apps.rs
/// writes first) narrows MCP_APP_CSP to the sources its server declared.
pub const MCP_APP_SERVED: &str = "text/html; charset=utf-8; profile=mcp-app";
const MCP_APP_CSP: &str = "default-src 'none'; script-src 'unsafe-inline' https:; style-src 'unsafe-inline' https:; img-src data: blob: https:; font-src data: https:; media-src data: blob: https:; connect-src https: wss:; frame-src 'none'; form-action 'none'";

/// The policy the `artifact://` protocol sends with a document served as `mime`.
pub fn protocol_csp(mime: &str) -> &'static str {
    if mime == MCP_APP_SERVED {
        MCP_APP_CSP
    } else {
        ARTIFACT_CSP
    }
}

/// The error overlay, and the APIs Claude's artifacts expect. The sandbox can
/// reach nothing but the app's page, so these ask it over postMessage (see
/// ui/js/artifacts.js): `window.storage` (ui/js/artifact_storage.js) and
/// `window.claude.complete` (asks the person first, then artifact_ai.rs),
/// `window.fs.readFile` (ui/js/artifact_files.js). Errors go to the app too,
/// which offers to ask Claude for a fix. First of all comes FRAME_GUARD.
pub const ERROR_HOOK: &str = concat!("<script>\n", include_str!("artifact_guard.js"), include_str!("artifact_hook.js"), include_str!("artifact_fs.js"), "</script>");

/// Undoes what the webview's start-up scripts change in every frame (alert,
/// confirm, link clicks; see the file); links ask the app.
pub const FRAME_GUARD: &str = concat!("<script>\n", include_str!("artifact_guard.js"), "</script>");

/// An MCP App's page (mcp_apps.rs builds it, its own CSP first). Not
/// previewable: an artifact of this type is shown as code.
pub const MCP_APP_KIND: &str = "text/html;profile=mcp-app";

/// `doc` with `first` before anything that can run, but after a doctype:
/// anything before that would put the page in quirks mode.
pub fn insert_at_start(doc: &str, first: &str) -> String {
    let lower = doc.to_ascii_lowercase();
    let html = lower.find("<html");
    let at = lower
        .find("<!doctype")
        .filter(|&i| html.is_none_or(|h| i < h))
        .and_then(|i| lower[i..].find('>').map(|j| i + j + 1))
        .unwrap_or(0);
    let mut s = doc.to_string();
    s.insert_str(at, first);
    s
}

fn theme_attr(dark: bool) -> &'static str {
    if dark {
        "<style>body{background:#262624;color:#f5f4ef}</style>"
    } else {
        "<style>body{background:#ffffff;color:#141413}</style>"
    }
}

/// Build the full HTML document for a previewable artifact.
pub fn build_document(kind: &str, content: &str, dark: bool) -> String {
    match kind {
        "text/html" => {
            let lower = content.to_ascii_lowercase();
            if lower.contains("<html") || lower.contains("<!doctype") {
                // the hook goes first, so the page's scripts find its APIs
                insert_at_start(content, ERROR_HOOK)
            } else {
                format!(
                    "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">{BASE_STYLE}{FRAGMENT_STYLE}{ERROR_HOOK}</head><body>{content}</body></html>"
                )
            }
        }
        "image/svg+xml" => format!(
            "<!doctype html><html><head><meta charset=\"utf-8\">{BASE_STYLE}{}{ERROR_HOOK}<style>body{{display:flex;align-items:center;justify-content:center;min-height:100vh;background:transparent}}svg{{max-width:100%;height:auto}}</style></head><body>{content}</body></html>",
            theme_attr(dark)
        ),
        "application/vnd.ant.mermaid" => format!(
            r#"<!doctype html><html><head><meta charset="utf-8">{BASE_STYLE}{theme}{ERROR_HOOK}<style>body{{display:flex;justify-content:center;padding:24px}}</style></head><body><pre class="mermaid">{code}</pre>
<script type="module">
import mermaid from 'https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.esm.min.mjs';
mermaid.initialize({{ startOnLoad: true, theme: '{mtheme}', securityLevel: 'strict' }});
</script></body></html>"#,
            theme = theme_attr(dark),
            code = escape_html(content),
            mtheme = if dark { "dark" } else { "default" }
        ),
        "application/vnd.ant.react" => build_react(content, dark),
        k if is_typed(k) => build_typed(k, content, dark, ERROR_HOOK),
        MCP_APP_KIND => insert_at_start(content, FRAME_GUARD),
        _ => format!(
            "<!doctype html><html><head><meta charset=\"utf-8\">{BASE_STYLE}</head><body><pre>{}</pre></body></html>",
            escape_html(content)
        ),
    }
}

const TYPES_CSS: &str = include_str!("artifact_types.css");

/// The page of a typed artifact: its runtime draws `content` (read from a
/// JSON string with every `<` escaped, so nothing in it can end the script
/// element). `hook` is ERROR_HOOK in the app, nothing in an exported file.
fn build_typed(kind: &str, content: &str, dark: bool, hook: &str) -> String {
    let (runtime, body) = match kind {
        // the deck as it is, for a frame without scripts (the gallery's
        // thumbnail); a scripted frame keeps it as text, which can't end early
        SLIDES => (include_str!("../../ui/js/artifact_types/slides.js"), format!("<noscript id=\"deck\">{}</noscript>", content.replace("</noscript", "<\\/noscript").replace("</NOSCRIPT", "<\\/NOSCRIPT"))),
        DOC => (include_str!("../../ui/js/artifact_types/doc.js"), format!("<article id=\"doc\" class=\"doc\">{}</article>", doc_html(content))),
        SHEET => (include_str!("../../ui/js/artifact_types/sheet.js"), String::new()),
        _ => (include_str!("../../ui/js/artifact_types/dashboard.js"), String::new()),
    };
    let data = serde_json::to_string(content).unwrap_or_else(|_| "\"\"".into()).replace('<', "\\u003c");
    let kind_name = kind.rsplit('.').next().unwrap_or("");
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">{BASE_STYLE}{hook}<style>{TYPES_CSS}</style></head><body class=\"t-{kind_name} {theme}\">{body}<script type=\"application/json\" id=\"art-data\" data-type=\"{kind_name}\">{data}</script><script type=\"module\">{runtime}</script></body></html>",
        theme = if dark { "dark" } else { "light" },
    )
}

/// A Doc's markdown as HTML. Raw HTML in it is shown as text and a link goes
/// nowhere but the web: the document is data, never markup that runs.
fn doc_html(md: &str) -> String {
    use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let safe = |u: &CowStr| ["http://", "https://", "mailto:", "#"].iter().any(|p| u.to_ascii_lowercase().starts_with(p));
    let events = Parser::new_ext(md, opts).map(|e| match e {
        Event::Html(s) | Event::InlineHtml(s) => Event::Text(s),
        Event::Start(Tag::Link { link_type, dest_url, title, id }) if !safe(&dest_url) => Event::Start(Tag::Link { link_type, dest_url: "#".into(), title, id }),
        Event::Start(Tag::Image { link_type, dest_url, title, id }) if !dest_url.starts_with("data:image/") => Event::Start(Tag::Image { link_type, dest_url: "".into(), title, id }),
        e => e,
    });
    let mut out = String::new();
    html::push_html(&mut out, events);
    out
}

/// shadcn/ui stand-ins (and cn) for React artifacts; see the file.
const UI_SHIM: &str = include_str!("artifact_ui.js");

/// The `@/components/ui/<name>` modules React artifacts can import, all served by UI_SHIM.
pub(crate) const UI_MODULES: [&str; 16] = [
    "alert", "badge", "button", "card", "checkbox", "dialog", "input", "label", "progress", "select", "separator", "slider",
    "switch", "tabs", "textarea", "tooltip",
];

/// shadcn's theme variables in the colours of the app, for UI_SHIM and the
/// artifact's own `bg-primary`, `text-muted-foreground`, ... classes.
const THEME_LIGHT: &str = "--background:0 0% 100%;--foreground:60 3% 8%;--card:0 0% 100%;--card-foreground:60 3% 8%;--popover:0 0% 100%;--popover-foreground:60 3% 8%;--primary:15 63% 60%;--primary-foreground:0 0% 100%;--secondary:48 25% 92%;--secondary-foreground:60 3% 8%;--muted:48 25% 92%;--muted-foreground:51 3% 44%;--accent:48 25% 92%;--accent-foreground:60 3% 8%;--destructive:0 52% 50%;--destructive-foreground:0 0% 100%;--border:50 14% 87%;--input:50 14% 87%;--ring:15 63% 60%;--radius:0.5rem";
const THEME_DARK: &str = "--background:60 3% 15%;--foreground:50 23% 95%;--card:60 3% 17%;--card-foreground:50 23% 95%;--popover:60 3% 17%;--popover-foreground:50 23% 95%;--primary:15 63% 60%;--primary-foreground:0 0% 100%;--secondary:60 2% 22%;--secondary-foreground:50 23% 95%;--muted:60 2% 22%;--muted-foreground:45 5% 63%;--accent:60 2% 22%;--accent-foreground:50 23% 95%;--destructive:0 78% 71%;--destructive-foreground:60 3% 8%;--border:60 2% 27%;--input:60 2% 27%;--ring:15 63% 60%;--radius:0.5rem";

fn build_react(code: &str, dark: bool) -> String {
    // The component source is embedded as a JSON string so no escaping issue
    // can break out of the script element.
    let src = serde_json::to_string(code).unwrap_or_else(|_| "\"\"".into()).replace("</", "<\\/");
    let shim = serde_json::to_string(UI_SHIM).unwrap_or_else(|_| "\"\"".into()).replace("</", "<\\/");
    let modules = serde_json::to_string(&UI_MODULES).unwrap_or_else(|_| "[]".into());
    let vars = if dark { THEME_DARK } else { THEME_LIGHT };
    // whole builds (`bundle`) whose own dependencies are pinned, so every
    // module they load is under a path ARTIFACT_CSP names
    let deps = "?bundle&deps=react@18.3.1,react-dom@18.3.1";
    let dom = "?deps=react@18.3.1,scheduler@0.23.2";
    // The import map is written by a script: the shadcn modules are a blob:
    // URL made here (the CSP allows blob: scripts; a data: URL it would not).
    format!(
        r##"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
{BASE_STYLE}{theme}{ERROR_HOOK}<style>:root{{{vars}}}</style>
<script src="https://cdn.tailwindcss.com/3.4.16"></script>
<script>
(function () {{
  if (!window.tailwind) return;
  var c = function (n) {{ return 'hsl(var(--' + n + ') / <alpha-value>)'; }};
  var colors = {{ border: c('border'), input: c('input'), ring: c('ring'), background: c('background'), foreground: c('foreground') }};
  ['primary', 'secondary', 'destructive', 'muted', 'accent', 'popover', 'card'].forEach(function (n) {{ colors[n] = {{ DEFAULT: c(n), foreground: c(n + '-foreground') }}; }});
  window.tailwind.config = {{ theme: {{ extend: {{ colors: colors, borderColor: {{ DEFAULT: 'hsl(var(--border))' }}, borderRadius: {{ lg: 'var(--radius)', md: 'calc(var(--radius) - 2px)', sm: 'calc(var(--radius) - 4px)' }} }} }} }};
}})();
</script>
<script>
(function () {{
var imports = {{
 "react":"https://esm.sh/react@18.3.1",
 "react/jsx-runtime":"https://esm.sh/react@18.3.1/jsx-runtime",
 "react-dom":"https://esm.sh/react-dom@18.3.1{dom}",
 "react-dom/client":"https://esm.sh/react-dom@18.3.1/client{dom}",
 "lucide-react":"https://esm.sh/lucide-react@0.468.0{deps}",
 "recharts":"https://esm.sh/recharts@2.15.0{deps}",
 "lodash":"https://esm.sh/lodash@4.17.21?bundle",
 "d3":"https://esm.sh/d3@7.9.0?bundle",
 "three":"https://esm.sh/three@0.171.0?bundle",
 "mathjs":"https://esm.sh/mathjs@14.0.1?bundle",
 "papaparse":"https://esm.sh/papaparse@5.4.1?bundle",
 "chart.js":"https://esm.sh/chart.js@4.4.7?bundle",
 "framer-motion":"https://esm.sh/framer-motion@11.15.0{deps},motion-dom@11.18.1,motion-utils@11.18.1",
 "clsx":"https://esm.sh/clsx@2.1.1?bundle",
 "tailwind-merge":"https://esm.sh/tailwind-merge@2.6.0?bundle",
 "class-variance-authority":"https://esm.sh/class-variance-authority@0.7.1?bundle",
 "date-fns":"https://esm.sh/date-fns@4.1.0?bundle"
}};
var ui = URL.createObjectURL(new Blob([{shim}], {{ type: 'text/javascript' }}));
{modules}.forEach(function (m) {{ imports['@/components/ui/' + m] = ui; }});
imports['@/lib/utils'] = ui;
var map = document.createElement('script');
map.type = 'importmap';
map.textContent = JSON.stringify({{ imports: imports }});
document.currentScript.after(map);
}})();
</script>
<script src="https://unpkg.com/@babel/standalone@7.26.4/babel.min.js"></script>
</head><body><div id="root"></div>
<script type="module">
const source = {src};
try {{
  const out = Babel.transform(source, {{ presets: [['react', {{ runtime: 'automatic' }}]], filename: 'artifact.jsx', sourceType: 'module' }}).code;
  const url = URL.createObjectURL(new Blob([out], {{ type: 'text/javascript' }}));
  const mod = await import(url);
  const React = await import('react');
  const {{ createRoot }} = await import('react-dom/client');
  const Comp = mod.default || Object.values(mod).find(v => typeof v === 'function');
  if (!Comp) throw new Error('The artifact has no default export to render.');
  createRoot(document.getElementById('root')).render(React.createElement(Comp));
}} catch (e) {{
  window.dispatchEvent(new ErrorEvent('error', {{ message: e && e.message ? e.message : String(e), error: e }}));
}}
</script></body></html>"##,
        theme = theme_attr(dark),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_complete_and_partial() {
        let t = "Intro\n<antArtifact identifier=\"calc\" type=\"application/vnd.ant.react\" title=\"Calc\">\nexport default () => <div/>\n</antArtifact>\nOutro";
        let s = split(t);
        assert_eq!(s.len(), 3);
        match &s[1] {
            Segment::Artifact(a) => {
                assert_eq!(a.identifier, "calc");
                assert!(a.complete);
                assert_eq!(a.content, "export default () => <div/>");
            }
            _ => panic!(),
        }
        // streaming: tag not finished
        let s = split("Hello <antArt");
        assert_eq!(s, vec![Segment::Markdown("Hello ".into())]);
        let s = split("Hi <antArtifact identifier=\"x\" type=\"text/html\" title=\"T\">abc</antArt");
        match &s[1] {
            Segment::Artifact(a) => {
                assert!(!a.complete);
                assert_eq!(a.content, "abc");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn fence_strip() {
        assert_eq!(strip_fence("```python\nprint(1)\n```\n"), "print(1)");
    }

    #[test]
    fn react_doc_escapes_script_close() {
        let d = build_document("application/vnd.ant.react", "const s = '</script>'; export default () => null", false);
        assert!(!d.contains("'</script>'"));
    }

    #[test]
    fn react_doc_maps_shadcn_modules_to_the_shim() {
        let d = build_document("application/vnd.ant.react", "export default () => null", true);
        assert!(d.contains("imports['@/components/ui/' + m] = ui;"));
        assert!(d.contains("imports['@/lib/utils'] = ui;"));
        assert!(d.contains(&serde_json::to_string(&UI_MODULES).unwrap()));
        assert!(d.contains("\"tailwind-merge\":\"https://esm.sh/tailwind-merge@"));
        assert!(d.contains(THEME_DARK) && !d.contains(THEME_LIGHT));
        // the shim is embedded whole, and can't close the script element it sits in
        assert!(d.contains("export function cn("));
        assert!(!UI_SHIM.contains("</script"));
    }

    #[test]
    fn shim_exports_every_mapped_component() {
        let exports: [(&str, &[&str]); 16] = [
            ("alert", &["Alert", "AlertTitle", "AlertDescription"]),
            ("badge", &["Badge", "badgeVariants"]),
            ("button", &["Button", "buttonVariants"]),
            ("card", &["Card", "CardHeader", "CardTitle", "CardDescription", "CardContent", "CardFooter"]),
            ("checkbox", &["Checkbox"]),
            ("dialog", &["Dialog", "DialogTrigger", "DialogContent", "DialogHeader", "DialogFooter", "DialogTitle", "DialogDescription", "DialogClose"]),
            ("input", &["Input"]),
            ("label", &["Label"]),
            ("progress", &["Progress"]),
            ("select", &["Select", "SelectTrigger", "SelectValue", "SelectContent", "SelectItem", "SelectGroup", "SelectLabel", "SelectSeparator"]),
            ("separator", &["Separator"]),
            ("slider", &["Slider"]),
            ("switch", &["Switch"]),
            ("tabs", &["Tabs", "TabsList", "TabsTrigger", "TabsContent"]),
            ("textarea", &["Textarea"]),
            ("tooltip", &["Tooltip", "TooltipTrigger", "TooltipContent", "TooltipProvider"]),
        ];
        for ((module, names), mapped) in exports.iter().zip(UI_MODULES) {
            assert_eq!(*module, mapped);
            for n in *names {
                let found = [format!("export const {n} ="), format!("export function {n}(")].iter().any(|p| UI_SHIM.contains(p.as_str()));
                assert!(found, "@/components/ui/{module} lacks {n}");
            }
        }
    }

    #[test]
    fn split_reads_the_update_command() {
        let t = "<antArtifact identifier=\"calc\" type=\"text/html\" title=\"Calc\" command=\"update\">\n<old_str>a</old_str>\n<new_str>b</new_str>\n</antArtifact>";
        match &split(t)[0] {
            Segment::Artifact(a) => {
                assert!(a.update && a.complete);
                assert_eq!(a.content, "<old_str>a</old_str>\n<new_str>b</new_str>");
            }
            _ => panic!(),
        }
        match &split("<antArtifact identifier=\"calc\" type=\"text/html\" title=\"Calc\">x</antArtifact>")[0] {
            Segment::Artifact(a) => assert!(!a.update),
            _ => panic!(),
        }
    }

    #[test]
    fn an_artifact_opened_again_before_it_closed_was_cut_off_there() {
        // a reply stopped mid-artifact, continued with an update of it in the same text
        let t = "<antArtifact identifier=\"s\" type=\"text/markdown\" title=\"S\">\nOnce upon<antArtifact identifier=\"s\" command=\"update\">\n<old_str>Once upon</old_str>\n<new_str>Once upon a time.</new_str>\n</antArtifact> Done.";
        let arts: Vec<ArtifactSpec> = split(t).into_iter().filter_map(|s| if let Segment::Artifact(a) = s { Some(a) } else { None }).collect();
        assert_eq!(arts.iter().map(|a| (a.content.as_str(), a.complete, a.update)).collect::<Vec<_>>(), [("Once upon", false, false), ("<old_str>Once upon</old_str>\n<new_str>Once upon a time.</new_str>", true, true)]);
        assert_eq!(split(t).last(), Some(&Segment::Markdown(" Done.".into())));
        // the update edits the version cut off, and keeps its type
        let v = latest_version([t.to_string()], "s").unwrap();
        assert_eq!((v.content.as_str(), v.kind.as_str(), v.title.as_str(), v.complete), ("Once upon a time.", "text/markdown", "S", true));
        // still open as it streams: not cut off
        assert!(matches!(&split("<antArtifact identifier=\"s\" type=\"text/markdown\">\nOnce</antArt")[0], Segment::Artifact(a) if !a.complete && a.content == "Once"));
    }

    fn edit(pairs: &[(&str, &str)]) -> String {
        pairs.iter().map(|(o, n)| format!("<old_str>{o}</old_str>\n<new_str>{n}</new_str>")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn updates_apply_as_in_the_panel() {
        assert_eq!(apply_update("<h1>Hi</h1><p>x</p>", &edit(&[("<h1>Hi</h1>", "<h1>Hello</h1>")])).unwrap(), "<h1>Hello</h1><p>x</p>");
        assert_eq!(apply_update("a b", &edit(&[("b", "$& $1 $$")])).unwrap(), "a $& $1 $$", "text, not a pattern");
        assert!(apply_update("one two", &edit(&[("three", "3")])).unwrap_err().contains("not found"));
        assert!(apply_update("x = 1; y = 1;", &edit(&[("= 1", "= 2")])).unwrap_err().contains("more than once"));
        assert!(apply_update("aaa", &edit(&[("aa", "b")])).unwrap_err().contains("more than once"), "overlapping");
        assert!(apply_update("şşş", &edit(&[("şş", "b")])).unwrap_err().contains("more than once"), "by characters");
        assert!(apply_update("abc", &edit(&[("", "b")])).unwrap_err().contains("empty"));
        assert!(apply_update("abc", "just text").unwrap_err().contains("no <old_str>"));
        // in order, all or none
        assert_eq!(apply_update("let a = 1;", &edit(&[("a = 1", "b = 1"), ("b = 1;", "b = 2;")])).unwrap(), "let b = 2;");
        assert!(apply_update("alpha beta", &edit(&[("alpha", "ALPHA"), ("gamma", "GAMMA")])).unwrap_err().contains("old_str 2 was not found"));
        // layout newlines, CRLF content, and a pair still streaming
        assert_eq!(apply_update("a\nline 1\nline 2\nb", "<old_str>\nline 1\nline 2\n</old_str><new_str>\nline 1\n</new_str>").unwrap(), "a\nline 1\nb");
        assert_eq!(apply_update("a\r\nb\r\nc\r\n", &edit(&[("b\nc", "x\ny")])).unwrap(), "a\r\nx\r\ny\r\n");
        assert_eq!(apply_update("a c", "<old_str>a</old_str><new_str>b</new_str>\n<old_str>c</old_str><new_str>d").unwrap(), "b c");
    }

    /// WCAG contrast ratio of two `#rrggbb` colours.
    fn contrast(a: &str, b: &str) -> f64 {
        let lum = |hex: &str| {
            let c: Vec<f64> = (0..3)
                .map(|i| u8::from_str_radix(&hex[1 + 2 * i..3 + 2 * i], 16).unwrap() as f64 / 255.0)
                .map(|v| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) })
                .collect();
            0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// The colours the page's body ends up with: the last `body{background;color}` and `color-scheme` set.
    fn page_colours(doc: &str) -> (String, String, String) {
        let pair = Regex::new(r"body\{background:(#[0-9a-f]{6});color:(#[0-9a-f]{6})\}").unwrap();
        let scheme = Regex::new(r"color-scheme:([a-z ]+)").unwrap();
        let c = pair.captures_iter(doc).last().expect("a background and text colour");
        let s = scheme.captures_iter(doc).last().expect("a color-scheme");
        (c[1].to_string(), c[2].to_string(), s[1].to_string())
    }

    #[test]
    fn html_fragments_are_readable_in_both_themes() {
        for dark in [false, true] {
            let d = build_document("text/html", "<h1>Hello</h1><p>Some text.</p>", dark);
            let (bg, fg, scheme) = page_colours(&d);
            // light only: in `light dark` the browser's default text turns white in the dark theme
            assert_eq!(scheme, "light", "dark={dark}");
            assert!(contrast(&bg, &fg) >= 7.0, "dark={dark}: {fg} on {bg}");
        }
        // the themed documents follow the app's theme, readable in both too
        for kind in ["application/vnd.ant.react", "application/vnd.ant.mermaid"] {
            for dark in [false, true] {
                let (bg, fg, _) = page_colours(&build_document(kind, "x", dark));
                assert!(contrast(&bg, &fg) >= 7.0, "{kind} dark={dark}: {fg} on {bg}");
            }
        }
    }

    #[test]
    fn a_fragment_gets_a_page_with_the_hook_before_its_own_code() {
        let content = "<p id=x>hi</p><script>window.storage.get('k')</script>";
        let d = build_document("text/html", content, true);
        assert!(d.starts_with("<!doctype html>"));
        let at = |s: &str| d.find(s).unwrap_or_else(|| panic!("{s} missing"));
        // the colours, then the APIs, then the artifact's own code (which may call them right away)
        assert!(at(BASE_STYLE) < at(FRAGMENT_STYLE));
        assert!(at(FRAGMENT_STYLE) < at(ERROR_HOOK));
        assert!(at(ERROR_HOOK) < at(content));
        assert!(d.ends_with(&format!("<body>{content}</body></html>")));
        // the browser's margin, which BASE_STYLE took away, comes back after it: text doesn't touch the frame
        assert!(FRAGMENT_STYLE.contains("margin:8px"));
    }

    #[test]
    fn a_full_document_keeps_its_colours_and_gets_the_hook_after_its_doctype() {
        let page = "<!DOCTYPE html>\n<html lang=\"en\"><head lang=\"en\"><style>body{background:#000;color:#0f0}</style></head><body><script>window.storage.get('k')</script></body></html>";
        for dark in [false, true] {
            let d = build_document("text/html", page, dark);
            // the doctype stays first (no quirks mode), and nothing of ours but the hook is added
            assert!(d.starts_with("<!DOCTYPE html><script>"), "dark={dark}");
            assert_eq!(d.replacen(ERROR_HOOK, "", 1), page);
            assert!(d.find("window.storage = {").unwrap() < d.find("window.storage.get('k')").unwrap());
        }
        // a comment may come before the doctype
        let d = build_document("text/html", "<!-- made by Claude --><!doctype html><html><body>x</body></html>", false);
        assert!(d.starts_with(&format!("<!-- made by Claude --><!doctype html>{ERROR_HOOK}<html>")));
        // no doctype: quirks mode already, the hook goes first
        let d = build_document("text/html", "<html><body>x</body></html>", false);
        assert!(d.starts_with(ERROR_HOOK));
    }

    #[test]
    fn script_documents_report_errors_to_the_app_and_bridge_complete() {
        for (kind, content) in [
            ("text/html", "<p>x</p>"),
            ("text/html", "<!doctype html><html><body>x</body></html>"),
            ("application/vnd.ant.react", "export default () => null"),
            ("application/vnd.ant.mermaid", "graph TD; A-->B"),
        ] {
            let d = build_document(kind, content, false);
            assert!(d.contains("window.parent.postMessage({ __artifact: 'error', message: message, stack: stack, blocked: blocked || null }, '*');"), "{kind}");
            assert!(d.contains("complete: function (prompt) { return call('complete', [prompt]); },"), "{kind}");
            assert!(d.contains("callMcpTool: function (name, args) { return call('callMcpTool', [name, args]); }"), "{kind}");
            assert!(d.contains("window.parent.postMessage({ __artifact: 'escape' }, '*')"), "{kind}");
        }
        // React's own errors (Babel, a missing export) carry their stack to the app
        assert!(build_document("application/vnd.ant.react", "x", false).contains("String(e), error: e }"));
    }

    #[test]
    fn documents_get_storage_and_stay_sandboxed() {
        for kind in ["text/html", "application/vnd.ant.react"] {
            let d = build_document(kind, "<p>x</p>", false);
            assert!(d.contains("window.storage = {"), "{kind}");
            assert!(d.contains("__artifact: 'call'"), "{kind}");
        }
        // the policy is as tight as before: nothing by default, no 'self', no nested frames
        assert!(ARTIFACT_CSP.starts_with("default-src 'none';"));
        assert!(ARTIFACT_CSP.contains("frame-src 'none'"));
        assert!(!ARTIFACT_CSP.contains("'self'"));
    }

    fn sources<'a>(csp: &'a str, directive: &str) -> Vec<&'a str> {
        let d = csp.split(';').map(str::trim).find(|d| d.split(' ').next() == Some(directive));
        d.unwrap_or_else(|| panic!("no {directive} in {csp}")).split(' ').skip(1).collect()
    }

    /// Whether `url` loads under the `directive` of `csp` (a source with no
    /// path is its whole host, one ending in `/` its folder, else that file;
    /// the query never counts), as the browser matches it.
    fn allowed(csp: &str, directive: &str, url: &str) -> bool {
        let url = url.split(['?', '#']).next().unwrap();
        sources(csp, directive).iter().any(|s| {
            let Some(rest) = s.strip_prefix("https://") else { return false };
            let (host, path) = rest.split_once('/').map_or((rest, None), |(h, p)| (h, Some(p)));
            let Some(u) = url.strip_prefix(&format!("https://{host}")) else { return false };
            match path {
                None => u.is_empty() || u.starts_with('/'),
                Some(p) if p.is_empty() || p.ends_with('/') => u.starts_with(&format!("/{p}")),
                Some(p) => u == format!("/{p}"),
            }
        })
    }

    #[test]
    fn artifacts_reach_no_host_but_the_libraries_and_mcp_apps_what_they_declare() {
        // no whole host: each library at one version, a folder or a file of it
        for s in sources(ARTIFACT_CSP, "script-src") {
            // (esm.sh's own stand-ins for Node's modules: the mathjs build imports its crypto)
            if matches!(s, "'unsafe-inline'" | "'unsafe-eval'" | "blob:" | "https://esm.sh/node/") {
                continue;
            }
            let path = s.strip_prefix("https://").and_then(|r| r.split_once('/')).map(|(_, p)| p).unwrap_or_default();
            assert!(path.len() > 3 && path.chars().any(|c| c.is_ascii_digit()), "a whole host or no version: {s}");
        }
        for d in ["connect-src", "img-src", "media-src", "font-src", "style-src"] {
            for s in sources(ARTIFACT_CSP, d) {
                assert!(matches!(s, "'unsafe-inline'" | "data:" | "blob:" | "https://fonts.googleapis.com" | "https://fonts.gstatic.com"), "{d}: {s}");
            }
        }
        assert_eq!(sources(ARTIFACT_CSP, "connect-src"), ["data:", "blob:", "https://fonts.googleapis.com", "https://fonts.gstatic.com"]);
        assert_eq!(sources(ARTIFACT_CSP, "form-action"), ["'none'"]);
        assert_eq!(sources(ARTIFACT_CSP, "webrtc"), ["'block'"]);
        // what React and Mermaid documents load still loads, with the files those builds import (esm.sh, Mermaid's chunks)
        let urls = Regex::new(r#"https://[^"'\s)]+"#).unwrap();
        for kind in ["application/vnd.ant.react", "application/vnd.ant.mermaid"] {
            let doc = build_document(kind, "x", false);
            for u in urls.find_iter(&doc).map(|m| m.as_str()) {
                assert!(allowed(ARTIFACT_CSP, "script-src", u), "{kind}: {u} is blocked");
            }
        }
        for u in [
            "https://esm.sh/react@18.3.1/es2022/react.mjs",
            "https://esm.sh/react@18.3.1/es2022/jsx-runtime.mjs",
            "https://esm.sh/react-dom@18.3.1/X-ZHJlYWN0QDE4LjMuMSxzY2hlZHVsZXJAMC4yMy4y/es2022/client.mjs",
            "https://esm.sh/scheduler@0.23.2/es2022/scheduler.mjs",
            "https://esm.sh/recharts@2.15.0/X-ZHJlYWN0LWRvbUAxOC4zLjEscmVhY3RAMTguMy4x/es2022/recharts.bundle.mjs",
            "https://esm.sh/node/crypto.mjs",
            "https://esm.sh/node/chunk-ETRHX7GZ.mjs",
            "https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/chunks/mermaid.esm.min/chunk-6CAUGOKZ.mjs",
            "https://cdnjs.cloudflare.com/ajax/libs/Chart.js/4.4.1/chart.umd.min.js",
        ] {
            assert!(allowed(ARTIFACT_CSP, "script-src", u), "{u} is blocked");
        }
        // and nothing else: another package, another version, a floating range, another file
        for u in [
            "https://esm.sh/left-pad@1.3.0",
            "https://esm.sh/react@18.2.0",
            "https://esm.sh/react@18.3.10/es2022/react.mjs",
            "https://esm.sh/lodash@^4.17.21?target=es2022",
            "https://unpkg.com/@babel/standalone@7.26.4/other.js",
            "https://cdn.jsdelivr.net/npm/mermaid@11.4.1/package.json",
            "https://cdn.jsdelivr.net/npm/evil@1.0.0/x.js",
            "https://cdnjs.cloudflare.com/ajax/libs/jquery/3.7.1/jquery.min.js",
            "https://cdn.tailwindcss.com",
            "https://example.com/x.js",
        ] {
            assert!(!allowed(ARTIFACT_CSP, "script-src", u), "{u} loads");
        }
        // the libraries text/html artifacts are told of
        for u in urls.find_iter(crate::prompts::ARTIFACTS_INFO).map(|m| m.as_str().trim_end_matches(['.', ',', ';'])) {
            assert!(allowed(ARTIFACT_CSP, "script-src", u) || allowed(ARTIFACT_CSP, "style-src", u), "the prompt names {u}, which is blocked");
        }
        assert_eq!(protocol_csp("text/html; charset=utf-8"), ARTIFACT_CSP);
        // an MCP App's page narrows the protocol's policy with its own, so this one has room for every source it may declare
        let app = protocol_csp(MCP_APP_SERVED);
        for d in ["script-src", "style-src", "img-src", "font-src", "media-src", "connect-src"] {
            assert!(sources(app, d).contains(&"https:"), "{d}");
        }
        assert!(sources(app, "connect-src").contains(&"wss:"));
        assert!(app.starts_with("default-src 'none';") && app.contains("frame-src 'none'") && app.contains("form-action 'none'"));
    }

    fn artifact(text: &str) -> ArtifactSpec {
        match split(text).into_iter().next() {
            Some(Segment::Artifact(a)) => a,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn typed_artifacts_are_named_previewed_and_saved_by_their_type() {
        for (alias, kind, label, ext) in [("slides", SLIDES, "Slides", "html"), ("doc", DOC, "Doc", "md"), ("sheet", SHEET, "Sheet", "csv"), ("dashboard", DASHBOARD, "Dashboard", "json")] {
            for t in [alias, kind] {
                assert_eq!(artifact(&format!("<antArtifact identifier=\"a\" type=\"{t}\" title=\"A\">x</antArtifact>")).kind, kind);
            }
            assert!(is_typed(kind) && is_previewable(kind));
            assert_eq!(kind_label(kind, None), label);
            assert_eq!(extension(kind, None), ext);
            // the prompt tells Claude the type and what goes in it
            assert!(crate::prompts::ARTIFACTS_INFO.contains(&format!("- {kind}: ")), "{kind}");
        }
        assert!(!is_typed("text/html") && !is_typed("text/markdown"));
    }

    #[test]
    fn a_typed_page_has_the_hook_first_its_runtime_and_its_data_as_inert_json() {
        let hostile = "a</script><script>window.pwned=1</script><!--<script>";
        for kind in [SLIDES, DOC, SHEET, DASHBOARD] {
            for dark in [false, true] {
                let d = build_document(kind, hostile, dark);
                assert_eq!(d.find("<script"), d.find(ERROR_HOOK), "{kind}: the hook runs first");
                let name = kind.rsplit('.').next().unwrap();
                assert!(d.contains(&format!("<body class=\"t-{name} {}\">", if dark { "dark" } else { "light" })));
                assert!(d.contains(&format!("<script type=\"application/json\" id=\"art-data\" data-type=\"{name}\">\"a\\u003c/script>")), "{kind}");
                // (a deck's own copy for frames without scripts is text where scripts run, see below)
                let outside = match d.find("<noscript id=\"deck\">") {
                    Some(i) => format!("{}{}", &d[..i], &d[i + d[i..].find("</noscript>").unwrap()..]),
                    None => d.clone(),
                };
                assert!(!outside.contains("<script>window.pwned"), "{kind}: the data escaped its element");
                assert!(d.contains(&format!("DATA.dataset.type === '{name}'")), "{kind}: its runtime");
            }
        }
        // a deck without scripts (the gallery's thumbnail) shows its slides; with them, they stay text
        let d = build_document(SLIDES, "<section><h1>Hi</h1></section></noscript><img src=x onerror=alert(1)>", false);
        assert!(d.contains("<noscript id=\"deck\"><section><h1>Hi</h1></section><\\/noscript><img src=x onerror=alert(1)></noscript>"));
        // an exported deck presents itself, with nothing that asks the app
        let page = String::from_utf8(export_file(SLIDES, "<section>x</section>", "Deck", "html").unwrap().1).unwrap();
        assert!(!page.contains(ERROR_HOOK) && page.contains("data-type=\"slides\""));
    }

    #[test]
    fn a_doc_is_markup_that_runs_nothing() {
        let html = doc_html("# T\n\n<script>alert(1)</script> <b onclick=x>b</b>\n\n[x](javascript:alert(1)) [y](https://example.com) ![i](https://example.com/a.png) ![d](data:image/png;base64,AA==)\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done");
        assert!(html.contains("<h1>T</h1>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;") && html.contains("&lt;b onclick=x&gt;"));
        assert!(html.contains("<a href=\"#\">x</a>") && html.contains("<a href=\"https://example.com\">y</a>"));
        assert!(html.contains("<img src=\"\" alt=\"i\"") && html.contains("src=\"data:image/png;base64,AA==\""));
        assert!(html.contains("<table>") && html.contains("type=\"checkbox\""));
        let page = String::from_utf8(export_file(DOC, "# T", "Q3 <plan>", "html").unwrap().1).unwrap();
        assert!(page.contains("<title>Q3 &lt;plan&gt;</title>") && page.contains("<h1>T</h1>") && !page.contains("<script"));
        assert_eq!(export_file(DOC, "# T", "x", "source").unwrap(), ("md".to_string(), b"# T".to_vec()));
        assert_eq!(export_file(SHEET, "a,b", "x", "source").unwrap().0, "csv");
        assert!(export_file(DOC, "# T", "x", "docx").unwrap().1.starts_with(b"PK"));
        assert!(export_file(SHEET, "a", "x", "docx").unwrap_err().contains("Sheet"));
    }

    /// A Doc's task item shows its checkbox in the bullet's place, not both: the page's CSS drops the bullet of
    /// an item whose checkbox comes first in it (a tight list) or in its first paragraph (a loose one).
    #[test]
    fn a_docs_task_items_have_no_bullet() {
        assert!(doc_html("- [ ] todo\n- [x] done\n").contains("<li><input disabled=\"\" type=\"checkbox\"/>\ntodo</li>"));
        assert!(doc_html("- [ ] todo\n\n- [x] done\n").contains("<li>\n<p><input disabled=\"\" type=\"checkbox\"/>\ntodo</p>"));
        assert!(TYPES_CSS.contains(".doc li:has(> input[type=\"checkbox\"]:first-child, > p:first-child > input[type=\"checkbox\"]:first-child) { list-style: none; }"));
    }

    #[test]
    fn an_edit_in_place_is_a_new_version_claude_reads_as_the_users() {
        let first = "<antArtifact identifier=\"budget\" type=\"sheet\" title=\"Budget\">\nItem,Cost\nRent,1000\n</antArtifact>".to_string();
        let update = "<antArtifact identifier=\"budget\" type=\"sheet\" title=\"\" command=\"update\">\n<old_str>Rent,1000</old_str>\n<new_str>Rent,1200</new_str>\n</antArtifact>".to_string();
        let a = latest_version([first.clone(), "no artifact here".into(), update.clone()], "budget").unwrap();
        assert_eq!((a.content.as_str(), a.kind.as_str(), a.title.as_str(), a.update), ("Item,Cost\nRent,1200", SHEET, "Budget", false));
        assert!(latest_version([first.clone()], "other").is_none());
        // a broken update leaves the version before it
        let broken = update.replace("Rent,1000", "Gas,1");
        assert_eq!(latest_version([first.clone(), broken], "budget").unwrap().content, "Item,Cost\nRent,1000");
        // the person's version: the same artifact, marked theirs, that can't close early
        let mine = user_version(&a, "Item,Cost\nRent,900\nNote,\"a </antArtifact> b\"");
        let v = artifact(&mine);
        assert_eq!((v.identifier.as_str(), v.kind.as_str(), v.title.as_str(), v.complete), ("budget", SHEET, "Budget", true));
        assert_eq!(v.content, "Item,Cost\nRent,900\nNote,\"a &lt;/antArtifact> b\"");
        assert!(mine.contains(" source=\"user\">"));
        let quoted = ArtifactSpec { title: "A \"quoted\" <title>".into(), language: Some("x\"y".into()), ..a.clone() };
        let v = artifact(&user_version(&quoted, "x"));
        assert_eq!((v.title.as_str(), v.language.as_deref()), ("A \"quoted\" <title>", Some("x\"y")));
        // it's the newest version on the branch, and a later update applies to it
        assert_eq!(latest_version([first, mine.clone()], "budget").unwrap().content, "Item,Cost\nRent,900\nNote,\"a &lt;/antArtifact> b\"");
        // added to the reply as a text block of its own, before a tool call (its result must follow it)
        use serde_json::json;
        let reply = json!([{"type": "thinking", "thinking": "t", "signature": "s"}, {"type": "text", "text": "Here."}]);
        assert_eq!(with_text_block(&reply, "B".into()), json!([{"type": "thinking", "thinking": "t", "signature": "s"}, {"type": "text", "text": "Here."}, {"type": "text", "text": "B"}]));
        let calling = json!([{"type": "text", "text": "Let me look."}, {"type": "tool_use", "id": "t1", "name": "x", "input": {}}]);
        assert_eq!(with_text_block(&calling, "B".into())[1], json!({"type": "text", "text": "B"}));
        assert_eq!(with_text_block(&json!("old"), "B".into()), json!([{"type": "text", "text": "old"}, {"type": "text", "text": "B"}]));
    }
}
