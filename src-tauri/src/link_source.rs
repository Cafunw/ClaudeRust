//! "Add from a link": a Google Drive file, or a Google Doc, Sheet or Slides
//! deck shared as "anyone with the link", fetched through Google's public
//! download and export addresses and added as an attachment or project
//! knowledge. There is no sign-in: a file that isn't shared by link is
//! refused. Only Google's own hosts are asked.

use crate::attachments::{self, refuse};
use crate::error::{AppError, AppResult};
use crate::remote_files::{body_capped, clean_name, client, hand_over, under, unescape, Fence};
use crate::state::AppState;
use serde_json::Value;
use std::sync::Arc;
use url::Url;

type St<'a> = tauri::State<'a, Arc<AppState>>;

/// Drive's downloads and Docs' exports are sent on to the googleusercontent.com hosts.
const HOSTS: &[&str] = &["docs.google.com", "drive.google.com", "drive.usercontent.google.com", "*.googleusercontent.com"];
/// The most one link brings: what a file attachment can be (attachments.rs).
const MAX_BYTES: usize = 40 * 1024 * 1024;

#[derive(Debug, PartialEq)]
enum Link {
    File(String),
    Doc(String),
    /// with the sheet (`gid`) the link shows, if it names one
    Sheet(String, Option<String>),
    Slides(String),
}

/// Where Google is asked.
struct Google {
    docs: Url,
    drive: Url,
    fence: Fence,
}

impl Google {
    fn live() -> Google {
        // a stand-in for Google in debug builds: its /docs and /drive at CLAUDERUST_GOOGLE_BASE
        #[cfg(debug_assertions)]
        if let Some(base) = std::env::var("CLAUDERUST_GOOGLE_BASE").ok().and_then(|b| Url::parse(&b).ok()) {
            return Google::at(&base);
        }
        Google { docs: Url::parse("https://docs.google.com").expect("a URL"), drive: Url::parse("https://drive.google.com").expect("a URL"), fence: Fence::new(HOSTS, None) }
    }

    #[cfg(any(test, debug_assertions))]
    fn at(base: &Url) -> Google {
        Google { docs: under(base, ["docs"]), drive: under(base, ["drive"]), fence: Fence::new(HOSTS, Some(base)) }
    }
}

fn bad_link() -> AppError {
    refuse("gdrive_bad_url", "That isn't a link to a Google Drive file or a Google Doc, Sheet or Slides deck.".into())
}

fn private() -> AppError {
    refuse(
        "gdrive_private",
        "This file is not shared by link: signing in to Drive is not supported yet. Use Drive for desktop and attach the local file, or share it as “Anyone with the link”.".into(),
    )
}

/// What a drive.google.com or docs.google.com link points at.
fn parse_link(input: &str) -> AppResult<Link> {
    let s = input.trim();
    let s = if s.contains("://") { s.to_string() } else { format!("https://{s}") };
    let u = Url::parse(&s).map_err(|_| bad_link())?;
    let Some(url::Host::Domain(host @ ("drive.google.com" | "docs.google.com"))) = u.host() else { return Err(bad_link()) };
    if u.scheme() != "https" || u.port().is_some() || !u.username().is_empty() || u.password().is_some() {
        return Err(bad_link());
    }
    // the account a link was copied in (…/u/1/…) doesn't matter
    let all: Vec<&str> = u.path_segments().into_iter().flatten().filter(|s| !s.is_empty()).collect();
    let mut segs = vec![];
    let mut i = 0;
    while i < all.len() {
        if all[i] == "u" && all.get(i + 1).is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())) {
            i += 2;
        } else {
            segs.push(all[i]);
            i += 1;
        }
    }
    let query = |k: &str| u.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    let link = match (host, &segs[..]) {
        ("drive.google.com", ["file", "d", id, ..]) => Link::File(id.to_string()),
        ("drive.google.com", ["open" | "uc"]) => Link::File(query("id").ok_or_else(bad_link)?),
        ("drive.google.com", ["drive", "folders", ..]) => {
            return Err(refuse("gdrive_unsupported", "Folders can't be added: link a single file, document, spreadsheet or presentation.".into()))
        }
        ("docs.google.com", ["document", "d", id, ..]) => Link::Doc(id.to_string()),
        ("docs.google.com", ["spreadsheets", "d", id, ..]) => {
            let in_fragment = || u.fragment().and_then(|f| f.split('&').find_map(|p| p.strip_prefix("gid=")).map(String::from));
            Link::Sheet(id.to_string(), query("gid").or_else(in_fragment).filter(|g| !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit())))
        }
        ("docs.google.com", ["presentation", "d", id, ..]) => Link::Slides(id.to_string()),
        _ => return Err(bad_link()),
    };
    let (Link::File(id) | Link::Doc(id) | Link::Sheet(id, _) | Link::Slides(id)) = &link;
    if !(10..=200).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(bad_link());
    }
    Ok(link)
}

/// Where `link`'s content is downloaded, the name it gets when Google gives
/// none, and the type the export is in.
fn source(g: &Google, link: &Link) -> (Url, String, Option<&'static str>) {
    let export = |kind: &str, id: &str, format: &str| {
        let mut u = under(&g.docs, [kind, "d", id, "export"]);
        u.query_pairs_mut().append_pair("format", format);
        u
    };
    match link {
        Link::File(id) => {
            let mut u = under(&g.drive, ["uc"]);
            u.query_pairs_mut().append_pair("export", "download").append_pair("id", id);
            (u, format!("drive-{}", &id[..8]), None)
        }
        Link::Doc(id) => (export("document", id, "md"), "Google Doc".into(), Some("md")),
        Link::Sheet(id, gid) => {
            let mut u = export("spreadsheets", id, "csv");
            if let Some(gid) = gid {
                u.query_pairs_mut().append_pair("gid", gid);
            }
            (u, "Google Sheet".into(), Some("csv"))
        }
        Link::Slides(id) => (under(&g.docs, ["presentation", "d", id, "export", "txt"]), "Google Slides".into(), Some("txt")),
    }
}

/// The file name a Content-Disposition header gives: its UTF-8 `filename*`, else its `filename`.
// shortcut: a quoted name with a ';' in it is cut there; parse quoted strings if names come out short.
fn disposition_name(d: &str) -> Option<String> {
    let parts: Vec<&str> = d.split(';').map(str::trim).collect();
    let value = |key: &str| parts.iter().find_map(|p| p.split_once('=').filter(|(k, _)| k.trim().eq_ignore_ascii_case(key)).map(|(_, v)| v.trim().trim_matches('"')));
    let extended = value("filename*").and_then(|v| {
        let (charset, rest) = v.split_once('\'')?;
        let (_, encoded) = rest.split_once('\'')?;
        charset.eq_ignore_ascii_case("utf-8").then(|| unescape(encoded)).flatten()
    });
    extended.or_else(|| value("filename").map(String::from))
}

/// The attachment's name: the one Google gives (without any folders), else
/// `fallback`, ending in `.ext` when the export has a type.
fn file_name(disposition: Option<&str>, fallback: &str, ext: Option<&str>) -> String {
    let given = disposition.and_then(disposition_name).map(|n| clean_name(n.rsplit(['/', '\\']).next().unwrap_or_default()));
    let mut name = given.filter(|n| !n.is_empty() && n != "." && n != "..").unwrap_or_else(|| fallback.to_string());
    if let Some(ext) = ext.filter(|e| !name.to_ascii_lowercase().ends_with(&format!(".{e}"))) {
        name = format!("{name}.{ext}");
    }
    name
}

/// The content at the link `input` (at most `cap` bytes) and its file name.
async fn fetch(g: &Google, input: &str, cap: usize) -> AppResult<(String, Vec<u8>)> {
    let link = parse_link(input)?;
    let (url, fallback, ext) = source(g, &link);
    let r = client(&g.fence)?.get(url).send().await?;
    let status = r.status();
    if status.is_redirection() {
        // the client stopped at an address outside Google's download hosts
        let to = r.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()).and_then(|l| r.url().join(l).ok());
        let host = to.as_ref().and_then(Url::host_str).unwrap_or_default().to_string();
        return Err(match host.as_str() {
            "accounts.google.com" => private(),
            _ => refuse("gdrive_redirect_refused", format!("Google sent the download to another site ({host}); it wasn't followed.")),
        });
    }
    match status.as_u16() {
        401 | 403 => return Err(private()),
        404 => return Err(refuse("gdrive_not_found", "Google has no file at that link: it may have been deleted.".into())),
        s if !status.is_success() => return Err(refuse("gdrive_failed", format!("Google answered with an error (HTTP {s}). Try again later."))),
        _ => {}
    }
    let header = |h| r.headers().get(h).and_then(|v| v.to_str().ok()).map(|v| v.trim().to_ascii_lowercase());
    let disposition = r.headers().get(reqwest::header::CONTENT_DISPOSITION).and_then(|v| v.to_str().ok()).map(String::from);
    let download = header(reqwest::header::CONTENT_DISPOSITION).is_some_and(|d| d.starts_with("attachment"));
    // a sign-in page, or a page about the file, instead of the file
    // shortcut: Drive's "can't scan for viruses" page (files over about 100 MB) says private too; the cap refuses those anyway.
    if !download && header(reqwest::header::CONTENT_TYPE).is_some_and(|t| t.starts_with("text/html")) {
        return Err(private());
    }
    let name = file_name(disposition.as_deref(), &fallback, ext);
    let bytes = body_capped(r, cap)
        .await?
        .ok_or_else(|| refuse("gdrive_too_large", format!("The file is over {}: download it and attach it instead.", crate::util::human_size(cap as u64))))?;
    Ok((name, bytes))
}

/// Fetch what the shared link `url` points at and stage it for the
/// composer, or add it to project `project_id`.
#[tauri::command]
pub async fn link_add(state: St<'_>, url: String, project_id: Option<String>) -> AppResult<Value> {
    let (name, bytes) = fetch(&Google::live(), &url, MAX_BYTES).await?;
    hand_over(state.inner().clone(), project_id, vec![(name, Ok(bytes))], attachments::from_bytes).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_files::mock;

    const ID: &str = "1AbC_d-EfGhIjKlMnOp";

    #[test]
    fn shared_links() {
        let sheet = |gid: Option<&str>| Link::Sheet(ID.into(), gid.map(String::from));
        for (url, want) in [
            (format!("https://drive.google.com/file/d/{ID}/view?usp=sharing"), Link::File(ID.into())),
            (format!("drive.google.com/file/d/{ID}"), Link::File(ID.into())),
            (format!("https://drive.google.com/file/u/1/d/{ID}/view"), Link::File(ID.into())),
            (format!("https://drive.google.com/open?id={ID}"), Link::File(ID.into())),
            (format!("https://drive.google.com/uc?export=download&id={ID}"), Link::File(ID.into())),
            (format!("https://docs.google.com/document/d/{ID}/edit?usp=sharing"), Link::Doc(ID.into())),
            (format!("https://docs.google.com/document/u/0/d/{ID}/"), Link::Doc(ID.into())),
            (format!("https://docs.google.com/spreadsheets/d/{ID}/edit#gid=42"), sheet(Some("42"))),
            (format!("https://docs.google.com/spreadsheets/d/{ID}/edit?gid=7#gid=42"), sheet(Some("7"))),
            (format!("https://docs.google.com/spreadsheets/d/{ID}/edit#gid=x"), sheet(None)),
            (format!("https://docs.google.com/presentation/d/{ID}/edit#slide=id.p"), Link::Slides(ID.into())),
        ] {
            assert_eq!(parse_link(&url).unwrap(), want, "{url}");
        }
        for url in [
            "".to_string(),
            "https://drive.google.com/".into(),
            format!("http://drive.google.com/file/d/{ID}/view"),
            format!("https://drive.google.com:8443/file/d/{ID}/view"),
            format!("https://me@drive.google.com/file/d/{ID}/view"),
            format!("https://drive.google.com@evil.example/file/d/{ID}/view"),
            format!("https://drive.google.com.evil.example/file/d/{ID}/view"),
            format!("https://evil.example/drive.google.com/file/d/{ID}/view"),
            format!("https://docs.google.com.evil.example/document/d/{ID}/edit"),
            format!("https://142.250.185.78/file/d/{ID}/view"),
            format!("https://[2a00:1450::200e]/file/d/{ID}/view"),
            format!("https://sites.google.com/document/d/{ID}"),
            format!("https://drive.usercontent.google.com/download?id={ID}"),
            format!("https://docs.google.com/forms/d/{ID}/viewform"),
            "https://docs.google.com/document/d/e/2PACX-1vRabc/pub".into(),
            "https://docs.google.com/document/d/short/edit".into(),
            "https://docs.google.com/document/d/abc%2F..%2Fdefghij/edit".into(),
            "https://drive.google.com/open?id=".into(),
            "https://drive.google.com/open".into(),
            format!("https://drive.google.com/open?id={ID}%26x"),
        ] {
            assert_eq!(parse_link(&url).unwrap_err().kind(), "gdrive_bad_url", "{url}");
        }
        let folder = parse_link(&format!("https://drive.google.com/drive/u/0/folders/{ID}")).unwrap_err();
        assert_eq!(folder.kind(), "gdrive_unsupported");
    }

    #[test]
    fn names_from_the_download() {
        assert_eq!(disposition_name("attachment; filename=\"Plan.md\"; filename*=UTF-8''Pl%C3%A2n.md").as_deref(), Some("Plân.md"));
        assert_eq!(disposition_name("attachment; FILENAME=report.pdf").as_deref(), Some("report.pdf"));
        assert_eq!(disposition_name("inline").as_deref(), None);
        assert_eq!(disposition_name("attachment; filename*=ISO-8859-1''x.txt").as_deref(), None);
        assert_eq!(file_name(Some("attachment; filename=\"Deck\""), "Google Slides", Some("txt")), "Deck.txt");
        assert_eq!(file_name(Some("attachment; filename=\"Notes.MD\""), "Google Doc", Some("md")), "Notes.MD");
        assert_eq!(file_name(Some("attachment; filename=\"../../x\\\\evil\u{202E}fdp.exe\""), "f", None), "evilfdp.exe");
        assert_eq!(file_name(Some("attachment; filename=\"..\""), "drive-1AbC_d-E", None), "drive-1AbC_d-E");
        assert_eq!(file_name(None, "Google Sheet", Some("csv")), "Google Sheet.csv");
    }

    /// A Google stand-in: what each file id answers.
    async fn google() -> (Google, Arc<parking_lot::Mutex<Vec<mock::Seen>>>) {
        let (elsewhere, _) = mock::serve(|_| (200, vec![], b"not here".to_vec())).await;
        let (base, seen) = mock::serve(move |r| {
            let id = r.path.split(['/', '=', '&', '?']).find(|s| s.len() == 10 && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())).unwrap_or("");
            let html = ("content-type", "text/html; charset=utf-8".to_string());
            match id {
                "DOCUMENT00" => (200, vec![("content-disposition", "attachment; filename=\"Plan.md\"; filename*=UTF-8''Pl%C3%A2n.md".into())], b"# Plan\n".to_vec()),
                "SHEET00000" => (200, vec![("content-type", "text/csv".into())], b"a,b\n1,2\n".to_vec()),
                "SLIDES0000" => (200, vec![("content-disposition", "attachment; filename=\"Deck\"".into())], b"Slide one".to_vec()),
                "FILE000000" if r.path.starts_with("/drive/uc") => (303, vec![("location", "/drive/usercontent/FILE000000".into())], vec![]),
                "FILE000000" => (200, vec![("content-disposition", "attachment; filename=\"../notes.txt\"".into()), html], b"<p>shared html</p>".to_vec()),
                "SIGNIN0000" => (302, vec![("location", "https://accounts.google.com/ServiceLogin?continue=x".into())], vec![]),
                "LOGINPAGE0" => (200, vec![html], b"<html>Sign in</html>".to_vec()),
                "FORBIDDEN0" => (403, vec![], vec![]),
                "GONE000000" => (404, vec![], vec![]),
                "BROKEN0000" => (500, vec![], vec![]),
                "EVIL000000" => (302, vec![("location", "https://evil.example/steal".into())], vec![]),
                "LOCAL00000" => (302, vec![("location", format!("{elsewhere}x"))], vec![]),
                "HUGE000000" => (200, vec![("content-disposition", "attachment; filename=big.txt".into())], vec![b'z'; 2048]),
                _ => (404, vec![], vec![]),
            }
        })
        .await;
        (Google::at(&base), seen)
    }

    #[tokio::test]
    async fn shared_files_are_fetched_from_their_export() {
        let (g, seen) = google().await;
        let (name, bytes) = fetch(&g, "https://docs.google.com/document/d/DOCUMENT00/edit", 1024).await.unwrap();
        assert_eq!((name.as_str(), &bytes[..]), ("Plân.md", &b"# Plan\n"[..]));
        assert_eq!(seen.lock().last().unwrap().path, "/docs/document/d/DOCUMENT00/export?format=md");
        let (name, _) = fetch(&g, "https://docs.google.com/spreadsheets/d/SHEET00000/edit#gid=3", 1024).await.unwrap();
        assert_eq!(name, "Google Sheet.csv");
        assert_eq!(seen.lock().last().unwrap().path, "/docs/spreadsheets/d/SHEET00000/export?format=csv&gid=3");
        let (name, bytes) = fetch(&g, "https://docs.google.com/presentation/d/SLIDES0000/edit", 1024).await.unwrap();
        assert_eq!((name.as_str(), &bytes[..]), ("Deck.txt", &b"Slide one"[..]));
        assert_eq!(seen.lock().last().unwrap().path, "/docs/presentation/d/SLIDES0000/export/txt");
        // a Drive file: sent on within Google, an HTML file that is a download
        let (name, bytes) = fetch(&g, "https://drive.google.com/file/d/FILE000000/view", 1024).await.unwrap();
        assert_eq!((name.as_str(), &bytes[..]), ("notes.txt", &b"<p>shared html</p>"[..]));
        let asked = seen.lock();
        assert_eq!(asked[asked.len() - 2].path, "/drive/uc?export=download&id=FILE000000");
    }

    #[tokio::test]
    async fn private_missing_and_redirected_files_are_refused() {
        let (g, _) = google().await;
        let kind = |id: &'static str| {
            let g = &g;
            async move { fetch(g, &format!("https://drive.google.com/file/d/{id}/view"), 1024).await.unwrap_err().kind().to_string() }
        };
        assert_eq!(kind("SIGNIN0000").await, "gdrive_private");
        assert_eq!(kind("LOGINPAGE0").await, "gdrive_private");
        assert_eq!(kind("FORBIDDEN0").await, "gdrive_private");
        assert_eq!(kind("GONE000000").await, "gdrive_not_found");
        assert_eq!(kind("BROKEN0000").await, "gdrive_failed");
        assert_eq!(kind("EVIL000000").await, "gdrive_redirect_refused");
        assert_eq!(kind("LOCAL00000").await, "gdrive_redirect_refused", "another server on this machine");
        assert_eq!(kind("HUGE000000").await, "gdrive_too_large");
        let e = fetch(&g, "https://drive.google.com/file/d/EVIL000000/view", 1024).await.unwrap_err();
        assert!(e.to_string().contains("evil.example"), "{e}");
        assert!(fetch(&g, "https://drive.google.com/file/d/HUGE000000/view", 2048).await.is_ok(), "up to the cap");
    }
}
