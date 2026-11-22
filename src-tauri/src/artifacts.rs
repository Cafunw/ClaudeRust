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

fn normalize_kind(kind: &str, language: &Option<String>) -> String {
    match kind {
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

pub fn kind_label(kind: &str, language: Option<&str>) -> String {
    match kind {
        "text/html" => "HTML".into(),
        "image/svg+xml" => "SVG".into(),
        "application/vnd.ant.react" => "React".into(),
        "application/vnd.ant.mermaid" => "Diagram".into(),
        "text/markdown" => "Document".into(),
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
    matches!(
        kind,
        "text/html" | "image/svg+xml" | "application/vnd.ant.react" | "application/vnd.ant.mermaid"
    )
}

/// File extension used when downloading an artifact.
pub fn extension(kind: &str, language: Option<&str>) -> String {
    match kind {
        "text/html" => "html".into(),
        "image/svg+xml" => "svg".into(),
        "application/vnd.ant.react" => "jsx".into(),
        "application/vnd.ant.mermaid" => "mmd".into(),
        "text/markdown" => "md".into(),
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
const FRAGMENT_STYLE: &str = "<style>:root{color-scheme:light}body{background:#ffffff;color:#141413}</style>";

/// Content Security Policy for artifact documents: they may load common
/// libraries from public CDNs but run in an opaque, sandboxed origin.
pub const ARTIFACT_CSP: &str = "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' blob: https://cdn.jsdelivr.net https://unpkg.com https://cdnjs.cloudflare.com https://esm.sh https://cdn.tailwindcss.com; style-src 'unsafe-inline' https:; img-src * data: blob:; font-src * data:; connect-src https: data: blob:; media-src * data: blob:; frame-src 'none'; worker-src blob:";

/// The error overlay, and the APIs Claude's artifacts expect. The sandbox can
/// reach nothing but the app's page, so these ask it over postMessage (see
/// ui/js/artifacts.js): `window.storage` (ui/js/artifact_storage.js) and
/// `window.claude.complete` (asks the person first, then artifact_ai.rs).
/// Errors go to the app too, which offers to ask Claude for a fix. First of
/// all comes FRAME_GUARD.
pub const ERROR_HOOK: &str = concat!("<script>\n", include_str!("artifact_guard.js"), include_str!("artifact_hook.js"), "</script>");

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
        MCP_APP_KIND => insert_at_start(content, FRAME_GUARD),
        _ => format!(
            "<!doctype html><html><head><meta charset=\"utf-8\">{BASE_STYLE}</head><body><pre>{}</pre></body></html>",
            escape_html(content)
        ),
    }
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
    let deps = "?deps=react@18.3.1,react-dom@18.3.1";
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
 "react-dom":"https://esm.sh/react-dom@18.3.1{deps}",
 "react-dom/client":"https://esm.sh/react-dom@18.3.1/client{deps}",
 "lucide-react":"https://esm.sh/lucide-react@0.468.0{deps}",
 "recharts":"https://esm.sh/recharts@2.15.0{deps}",
 "lodash":"https://esm.sh/lodash@4.17.21",
 "d3":"https://esm.sh/d3@7.9.0",
 "three":"https://esm.sh/three@0.171.0",
 "mathjs":"https://esm.sh/mathjs@14.0.1",
 "papaparse":"https://esm.sh/papaparse@5.4.1",
 "chart.js":"https://esm.sh/chart.js@4.4.7",
 "framer-motion":"https://esm.sh/framer-motion@11.15.0{deps}",
 "clsx":"https://esm.sh/clsx@2.1.1",
 "tailwind-merge":"https://esm.sh/tailwind-merge@2.6.0",
 "class-variance-authority":"https://esm.sh/class-variance-authority@0.7.1",
 "date-fns":"https://esm.sh/date-fns@4.1.0"
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
            assert!(d.contains("window.parent.postMessage({ __artifact: 'error', message: message, stack: stack }, '*');"), "{kind}");
            assert!(d.contains("window.claude = { complete: function (prompt) { return call('complete', [prompt]); } };"), "{kind}");
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
}
