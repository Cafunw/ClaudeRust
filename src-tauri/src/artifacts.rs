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
                for cap in ATTR_RE.captures_iter(attrs_str) {
                    let v = unescape_attr(&cap[2]);
                    match &cap[1] {
                        "identifier" => identifier = v,
                        "type" => kind = v,
                        "title" => title = v,
                        "language" => language = Some(v).filter(|s: &String| !s.is_empty()),
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
                        }));
                        break;
                    }
                }
            }
        }
    }
    out
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

/// Content Security Policy for artifact documents: they may load common
/// libraries from public CDNs but run in an opaque, sandboxed origin.
pub const ARTIFACT_CSP: &str = "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' blob: https://cdn.jsdelivr.net https://unpkg.com https://cdnjs.cloudflare.com https://esm.sh https://cdn.tailwindcss.com; style-src 'unsafe-inline' https:; img-src * data: blob:; font-src * data:; connect-src https: data: blob:; media-src * data: blob:; frame-src 'none'; worker-src blob:";

/// The error overlay, and the APIs Claude's artifacts expect: `window.storage`
/// asks the app over postMessage (the sandbox has no storage of its own; see
/// ui/js/artifact_storage.js). `window.claude.complete` would spend the
/// user's API credit from model-written code with no prompt, so it only says
/// it isn't there.
const ERROR_HOOK: &str = r#"<script>
window.addEventListener('error', function (e) {
  var d = document.getElementById('__artifact_error') || document.createElement('pre');
  d.id = '__artifact_error';
  d.style.cssText = 'position:fixed;left:0;right:0;bottom:0;margin:0;padding:10px 14px;background:#fde8e4;color:#7a1f0e;font:12px/1.5 ui-monospace,monospace;white-space:pre-wrap;max-height:40%;overflow:auto;z-index:99999';
  d.textContent = 'Error: ' + (e.message || e.error || 'unknown');
  document.body && document.body.appendChild(d);
});
(function () {
  var seq = 0, waiting = {};
  window.addEventListener('message', function (e) {
    var d = e.data;
    if (e.source !== window.parent || !d || d.__artifactStorage !== 'reply' || !waiting[d.id]) return;
    var w = waiting[d.id];
    delete waiting[d.id];
    if (d.error) w[1](new Error(d.error)); else w[0](d.result);
  });
  function call(op, args) {
    return new Promise(function (ok, fail) {
      var id = ++seq;
      waiting[id] = [ok, fail];
      window.parent.postMessage({ __artifactStorage: 'call', id: id, op: op, args: args }, '*');
    });
  }
  window.storage = {
    get: function (key, shared) { return call('get', [key, !!shared]); },
    set: function (key, value, shared) { return call('set', [key, value, !!shared]); },
    delete: function (key, shared) { return call('delete', [key, !!shared]); },
    list: function (prefix, shared) { return call('list', [prefix, !!shared]); }
  };
  window.claude = { complete: function () { return Promise.reject(new Error('window.claude.complete is not available in ClaudeRust artifacts.')); } };
})();
</script>"#;

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
                // inject the error hook right after <head> if present
                if let Some(i) = lower.find("<head>") {
                    let mut s = content.to_string();
                    s.insert_str(i + 6, ERROR_HOOK);
                    return s;
                }
                format!("{ERROR_HOOK}{content}")
            } else {
                format!(
                    "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">{BASE_STYLE}{ERROR_HOOK}</head><body>{content}</body></html>"
                )
            }
        }
        "image/svg+xml" => format!(
            "<!doctype html><html><head><meta charset=\"utf-8\">{BASE_STYLE}{}<style>body{{display:flex;align-items:center;justify-content:center;min-height:100vh;background:transparent}}svg{{max-width:100%;height:auto}}</style></head><body>{content}</body></html>",
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
        _ => format!(
            "<!doctype html><html><head><meta charset=\"utf-8\">{BASE_STYLE}</head><body><pre>{}</pre></body></html>",
            escape_html(content)
        ),
    }
}

/// shadcn/ui stand-ins (and cn) for React artifacts; see the file.
const UI_SHIM: &str = include_str!("artifact_ui.js");

/// The `@/components/ui/<name>` modules React artifacts can import, all served by UI_SHIM.
const UI_MODULES: [&str; 16] = [
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
  window.dispatchEvent(new ErrorEvent('error', {{ message: e && e.message ? e.message : String(e) }}));
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
    fn documents_get_storage_and_stay_sandboxed() {
        for kind in ["text/html", "application/vnd.ant.react"] {
            let d = build_document(kind, "<p>x</p>", false);
            assert!(d.contains("window.storage = {"), "{kind}");
            assert!(d.contains("__artifactStorage: 'call'"), "{kind}");
        }
        // the policy is as tight as before: nothing by default, no 'self', no nested frames
        assert!(ARTIFACT_CSP.starts_with("default-src 'none';"));
        assert!(ARTIFACT_CSP.contains("frame-src 'none'"));
        assert!(!ARTIFACT_CSP.contains("'self'"));
    }
}
