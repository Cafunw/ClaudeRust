//! "Add from GitHub": a repository's file tree through the GitHub REST API
//! (api.github.com), and the text files picked from it
//! (raw.githubusercontent.com) as chat attachments or project knowledge. An
//! optional personal access token is kept in the system's credential store
//! and goes to those two hosts only.

use crate::attachments::{self, refuse, Attachment};
use crate::error::{AppError, AppResult};
use crate::remote_files::{body_capped, clean_name, client, hand_over, under, unescape, Fence};
use crate::state::AppState;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use url::Url;

type St<'a> = tauri::State<'a, Arc<AppState>>;

const HOSTS: &[&str] = &["api.github.com", "raw.githubusercontent.com"];
/// The token's entry in the secret store.
const TOKEN_KEY: &str = "github-token";
/// What one add takes at most: files, bytes of them all, bytes of one.
const MAX_FILES: usize = 300;
const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// The most files a listing shows.
const MAX_LISTED: usize = 5000;
/// GitHub sends a tree of at most 7 MB.
const MAX_TREE_BYTES: usize = 32 * 1024 * 1024;
/// How many first segments after /tree/ are tried as the branch (its name can have slashes).
const MAX_REF_TRIES: usize = 6;
/// Files fetched at once.
const PARALLEL: usize = 6;

/// Folders of dependencies and build output: listed last, hidden unless asked for.
const VENDORED: &[&str] = &[
    ".git", "node_modules", "bower_components", "jspm_packages", "vendor", "Pods", "Carthage", ".venv", "venv", "__pycache__", ".tox",
    ".mypy_cache", ".pytest_cache", ".gradle", ".yarn", "target", "dist", "build", ".next", ".nuxt", "coverage",
];
/// Types that are never text (Word, Excel and PowerPoint files are: attachments.rs reads their text).
const BINARY: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "icns", "tif", "tiff", "psd", "heic", "avif", "pdf", "zip", "gz", "tgz", "bz2", "xz",
    "7z", "rar", "zst", "jar", "war", "class", "exe", "dll", "so", "dylib", "a", "o", "obj", "lib", "pdb", "bin", "wasm", "woff", "woff2",
    "ttf", "otf", "eot", "mp3", "mp4", "m4a", "mov", "avi", "mkv", "wav", "flac", "ogg", "webm", "pyc", "dmg", "iso", "apk", "ipa", "db",
    "sqlite", "sqlite3", "parquet", "npy", "npz", "pkl", "pt", "onnx", "h5", "doc", "xls", "ppt",
];

/// Where GitHub is asked.
struct Github {
    api: Url,
    raw: Url,
    fence: Fence,
}

impl Github {
    fn live() -> Github {
        // a stand-in for GitHub in debug builds: its /api and /raw at CLAUDERUST_GITHUB_BASE
        #[cfg(debug_assertions)]
        if let Some(base) = std::env::var("CLAUDERUST_GITHUB_BASE").ok().and_then(|b| Url::parse(&b).ok()) {
            return Github::at(&base);
        }
        Github { api: Url::parse("https://api.github.com").expect("a URL"), raw: Url::parse("https://raw.githubusercontent.com").expect("a URL"), fence: Fence::new(HOSTS, None) }
    }

    #[cfg(any(test, debug_assertions))]
    fn at(base: &Url) -> Github {
        Github { api: under(base, ["api"]), raw: under(base, ["raw"]), fence: Fence::new(HOSTS, Some(base)) }
    }
}

#[derive(Debug, PartialEq)]
struct Repo {
    owner: String,
    name: String,
    /// What follows /tree/ or /blob/: a branch, tag or commit, then maybe a path.
    rest: Vec<String>,
}

fn bad_url() -> AppError {
    refuse("github_bad_url", "That isn't a GitHub repository address: use https://github.com/owner/repo, optionally with /tree/branch/folder.".into())
}

fn not_found() -> AppError {
    refuse("github_not_found", "GitHub has no such repository, or it is private: add an access token that can read it.".into())
}

/// The repository (and branch and folder) a github.com address names.
fn parse_repo(input: &str) -> AppResult<Repo> {
    let s = input.trim();
    let s = if s.contains("://") { s.to_string() } else { format!("https://{s}") };
    let u = Url::parse(&s).map_err(|_| bad_url())?;
    let github = matches!(u.host(), Some(url::Host::Domain("github.com" | "www.github.com")));
    if u.scheme() != "https" || !github || u.port().is_some() || !u.username().is_empty() || u.password().is_some() {
        return Err(bad_url());
    }
    let segs = u
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .map(|s| unescape(s).filter(|s| plain_segment(s)).ok_or_else(bad_url))
        .collect::<AppResult<Vec<_>>>()?;
    let [owner, name, rest @ ..] = &segs[..] else { return Err(bad_url()) };
    let name = name.strip_suffix(".git").unwrap_or(name);
    let rest = match rest {
        [] => &[][..],
        [kind, r @ ..] if !r.is_empty() && (kind == "tree" || kind == "blob") => r,
        _ => return Err(bad_url()),
    };
    let owner_ok = owner.len() <= 39 && !owner.starts_with('-') && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let name_ok = !name.is_empty() && name.len() <= 100 && name != "." && name != ".." && name.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c));
    if !owner_ok || !name_ok {
        return Err(bad_url());
    }
    Ok(Repo { owner: owner.clone(), name: name.to_string(), rest: rest.to_vec() })
}

/// A plain name: not . or .., without separators or control characters.
fn plain_segment(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.chars().any(|c| c == '/' || c == '\\' || c.is_control())
}

fn valid_path(p: &str) -> bool {
    p.split('/').all(plain_segment)
}

fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn get(c: &reqwest::Client, url: Url, token: Option<&str>, accept: &str) -> reqwest::RequestBuilder {
    let rb = c.get(url).header(reqwest::header::ACCEPT, accept).header("X-GitHub-Api-Version", "2022-11-28");
    match token {
        Some(t) => rb.bearer_auth(t),
        None => rb,
    }
}

/// What GitHub's refusal `r` means. Its body is never shown: a server can put anything there.
fn status_error(r: &reqwest::Response, authed: bool) -> AppError {
    let s = r.status().as_u16();
    let spent = r.headers().get("x-ratelimit-remaining").is_some_and(|v| v.as_bytes() == b"0");
    match s {
        401 => refuse("github_bad_token", "GitHub didn't accept the access token: it may have expired or been revoked. Replace or remove it.".into()),
        403 | 429 if spent || s == 429 => {
            if authed {
                refuse("github_rate_limited_token", "GitHub's request limit for this token is used up. Try again later.".into())
            } else {
                refuse("github_rate_limited", "GitHub's limit of 60 requests an hour without a token is used up. Try again later, or add an access token (5,000 an hour).".into())
            }
        }
        403 => refuse("github_forbidden", "GitHub refused access to this repository. A token needs read access to its contents.".into()),
        404 => not_found(),
        409 => refuse("github_empty", "This repository is empty.".into()),
        _ => refuse("github_failed", format!("GitHub answered with an error (HTTP {s}). Try again later.")),
    }
}

/// The commit `r` names (a branch, tag or SHA; HEAD is the default branch's
/// head), or the status GitHub answered when it knows none.
async fn commit_sha(gh: &Github, c: &reqwest::Client, token: Option<&str>, repo: &Repo, r: &str) -> AppResult<Result<String, u16>> {
    let url = under(&gh.api, ["repos", &repo.owner, &repo.name, "commits"].into_iter().chain(r.split('/')));
    let resp = get(c, url, token, "application/vnd.github.sha").send().await?;
    match resp.status().as_u16() {
        s @ (404 | 422) => return Ok(Err(s)),
        _ if !resp.status().is_success() => return Err(status_error(&resp, token.is_some())),
        _ => {}
    }
    let sha = String::from_utf8_lossy(&body_capped(resp, 100).await?.unwrap_or_default()).trim().to_string();
    if is_sha(&sha) {
        Ok(Ok(sha))
    } else {
        Err(refuse("github_failed", "GitHub answered with something that isn't a commit.".into()))
    }
}

/// The commit the address points at, the branch, tag or commit it names, and
/// the folder or file in it.
async fn resolve(gh: &Github, c: &reqwest::Client, token: Option<&str>, repo: &Repo) -> AppResult<(String, Option<String>, String)> {
    if repo.rest.is_empty() {
        return match commit_sha(gh, c, token, repo, "HEAD").await? {
            Ok(sha) => Ok((sha, None, String::new())),
            Err(_) => Err(not_found()),
        };
    }
    // GitHub says 422 for a ref it doesn't know, 404 for a repository it doesn't show
    let mut unknown_ref = false;
    for n in 1..=repo.rest.len().min(MAX_REF_TRIES) {
        let r = repo.rest[..n].join("/");
        match commit_sha(gh, c, token, repo, &r).await? {
            Ok(sha) => return Ok((sha, Some(r), repo.rest[n..].join("/"))),
            Err(status) => unknown_ref |= status == 422,
        }
    }
    Err(if unknown_ref { refuse("github_ref_not_found", "That branch, tag or commit isn't in the repository.".into()) } else { not_found() })
}

#[derive(Deserialize)]
struct Tree {
    tree: Vec<Entry>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Deserialize)]
struct Entry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    mode: String,
    #[serde(default)]
    size: u64,
}

fn vendored(path: &str) -> bool {
    path.split('/').rev().skip(1).any(|dir| VENDORED.contains(&dir))
}

/// Why a file can't be picked: never text, or over the cap for one.
fn skip(e: &Entry) -> Option<&'static str> {
    let file = e.path.rsplit('/').next().unwrap_or(&e.path);
    let ext = file.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase()).unwrap_or_default();
    if BINARY.contains(&ext.as_str()) {
        Some("binary")
    } else if e.size > MAX_FILE_BYTES {
        Some("large")
    } else {
        None
    }
}

/// The picker's files of `tree` under `under` (a folder or one file; "" is
/// all): the project's own first, then the vendored ones, at most
/// `MAX_LISTED`; and whether some were left out. Symbolic links and
/// submodules are not files to read.
fn listing(tree: Vec<Entry>, under: &str) -> (Vec<Value>, bool) {
    let inside = |p: &str| under.is_empty() || p.strip_prefix(under).is_some_and(|r| r.is_empty() || r.starts_with('/'));
    let mut files: Vec<(bool, Entry)> = tree
        .into_iter()
        .filter(|e| e.kind == "blob" && e.mode != "120000" && inside(&e.path) && valid_path(&e.path))
        .map(|e| (vendored(&e.path), e))
        .collect();
    files.sort_by(|a, b| (a.0, &a.1.path).cmp(&(b.0, &b.1.path)));
    let cut = files.len() > MAX_LISTED;
    files.truncate(MAX_LISTED);
    let out = files.into_iter().map(|(v, e)| json!({"path": e.path, "size": e.size, "skip": skip(&e), "vendored": v})).collect();
    (out, cut)
}

/// The repository at `input`: where it points and its files to pick from.
async fn list(gh: &Github, token: Option<&str>, input: &str) -> AppResult<Value> {
    let repo = parse_repo(input)?;
    let c = client(&gh.fence)?;
    let (sha, r, path) = resolve(gh, &c, token, &repo).await?;
    let mut url = under(&gh.api, ["repos", &repo.owner, &repo.name, "git", "trees", &sha]);
    url.set_query(Some("recursive=1"));
    let resp = get(&c, url, token, "application/vnd.github+json").send().await?;
    if !resp.status().is_success() {
        return Err(status_error(&resp, token.is_some()));
    }
    let tree = body_capped(resp, MAX_TREE_BYTES)
        .await?
        .and_then(|b| serde_json::from_slice::<Tree>(&b).ok())
        .ok_or_else(|| refuse("github_failed", "GitHub's answer isn't a file tree this app can read.".into()))?;
    let (files, cut) = listing(tree.tree, &path);
    if files.is_empty() && !path.is_empty() {
        return Err(refuse("github_path_not_found", format!("There's no folder or file “{path}” on that branch.")));
    }
    Ok(json!({
        "owner": repo.owner, "repo": repo.name, "ref": r, "sha": sha, "path": path,
        "files": files, "truncated": tree.truncated || cut,
        "limits": {"files": MAX_FILES, "bytes": MAX_TOTAL_BYTES, "file_bytes": MAX_FILE_BYTES},
    }))
}

fn too_much() -> AppError {
    refuse("github_selection_too_large", format!("The files picked add up to more than {} MB of text; pick fewer.", MAX_TOTAL_BYTES >> 20))
}

/// Refuse a pick over the caps, or with a path that isn't a plain one.
fn check_pick(paths: &[String]) -> AppResult<()> {
    if paths.len() > MAX_FILES {
        return Err(refuse("github_too_many_files", format!("Pick at most {MAX_FILES} files at a time.")));
    }
    if paths.iter().any(|p| !valid_path(p)) {
        return Err(refuse("github_bad_path", "One of the files picked has a name that can't be fetched.".into()));
    }
    Ok(())
}

/// The bytes of `path` at commit `sha`; `used` counts what all of the pick took so far.
async fn fetch_one(gh: &Github, c: &reqwest::Client, token: Option<&str>, repo: &Repo, sha: &str, path: &str, used: &AtomicU64) -> AppResult<Vec<u8>> {
    let room = MAX_TOTAL_BYTES.saturating_sub(used.load(Ordering::Relaxed));
    let url = under(&gh.raw, [repo.owner.as_str(), &repo.name, sha].into_iter().chain(path.split('/')));
    let resp = get(c, url, token, "*/*").send().await?;
    if !resp.status().is_success() {
        return Err(status_error(&resp, token.is_some()));
    }
    let cap = MAX_FILE_BYTES.min(room);
    let Some(bytes) = body_capped(resp, cap as usize).await? else {
        return Err(if cap < MAX_FILE_BYTES { too_much() } else { refuse("github_file_too_large", format!("“{path}” is over {} MB.", MAX_FILE_BYTES >> 20)) });
    };
    // (files fetched at once can each pass the check above; this one counts them all)
    let n = bytes.len() as u64;
    if used.fetch_add(n, Ordering::Relaxed) + n > MAX_TOTAL_BYTES {
        used.fetch_sub(n, Ordering::Relaxed);
        return Err(too_much());
    }
    Ok(bytes)
}

/// The files `paths` of the repository at `input` at commit `sha`, named
/// `repo/path`, each with its bytes or why it couldn't be fetched.
async fn fetch(gh: &Github, token: Option<&str>, input: &str, sha: &str, paths: &[String]) -> AppResult<Vec<(String, AppResult<Vec<u8>>)>> {
    let repo = parse_repo(input)?;
    if !is_sha(sha) {
        return Err(AppError::msg("Load the repository again."));
    }
    check_pick(paths)?;
    let c = client(&gh.fence)?;
    let used = AtomicU64::new(0);
    // (made first, not in the stream's map: a closure there makes the command's future not Send)
    let jobs: Vec<_> = paths.iter().map(|p| fetch_one(gh, &c, token, &repo, sha, p, &used)).collect();
    let fetched: Vec<_> = futures_util::stream::iter(jobs).buffered(PARALLEL).collect().await;
    Ok(paths.iter().map(|p| format!("{}/{p}", repo.name)).zip(fetched).collect())
}

/// A picked file as an attachment: text only, named by its place in the repository.
fn attach(name: &str, bytes: &[u8]) -> AppResult<Attachment> {
    let file = name.rsplit('/').next().unwrap_or(name);
    let mut a = attachments::from_bytes(file, bytes)?;
    if a.kind != "text" {
        return Err(refuse("github_not_text", format!("“{name}” isn't a text file.")));
    }
    a.name = clean_name(name);
    Ok(a)
}

fn saved_token() -> Option<String> {
    crate::mcp_oauth::secret_get(TOKEN_KEY).filter(|t| !t.is_empty())
}

/// GitHub's tokens (classic, fine-grained, and the old 40 hex digits) are letters, digits and _.
fn check_token(t: &str) -> AppResult<()> {
    if (20..=255).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        Ok(())
    } else {
        Err(refuse("github_token_invalid", "That doesn't look like a GitHub access token: it has letters, digits and _ only.".into()))
    }
}

/// The repository at `url` and its files to pick from.
#[tauri::command]
pub async fn github_list(url: String) -> AppResult<Value> {
    list(&Github::live(), saved_token().as_deref(), &url).await
}

/// Fetch `paths` of the repository at `url` (commit `sha`, from `github_list`)
/// and stage them for the composer, or add them to project `project_id`.
#[tauri::command]
pub async fn github_add(state: St<'_>, url: String, sha: String, paths: Vec<String>, project_id: Option<String>) -> AppResult<Value> {
    let files = fetch(&Github::live(), saved_token().as_deref(), &url, &sha, &paths).await?;
    hand_over(state.inner().clone(), project_id, files, attach).await
}

/// Whether a token is saved, and whether the store keeps it across restarts.
#[tauri::command]
pub async fn github_token_status() -> AppResult<Value> {
    Ok(json!({"saved": saved_token().is_some(), "persists": crate::mcp_oauth::STORE_PERSISTS}))
}

/// Keep the personal access token in the secret store; none (or empty) removes it.
#[tauri::command]
pub async fn github_token_set(token: Option<String>) -> AppResult<Value> {
    match token.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        None => crate::mcp_oauth::secret_delete(TOKEN_KEY),
        Some(t) => {
            check_token(t)?;
            crate::mcp_oauth::secret_set(TOKEN_KEY, t).map_err(|e| AppError::msg(format!("Keeping the token in the system's credential store failed: {e}")))?;
        }
    }
    github_token_status().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_files::mock;

    const TOKEN: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn repo(owner: &str, name: &str, rest: &[&str]) -> Repo {
        Repo { owner: owner.into(), name: name.into(), rest: rest.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn repository_addresses() {
        for (url, want) in [
            ("https://github.com/rust-lang/rust", repo("rust-lang", "rust", &[])),
            ("github.com/rust-lang/rust/", repo("rust-lang", "rust", &[])),
            ("  https://www.github.com/a/b.git  ", repo("a", "b", &[])),
            ("https://github.com:443/a/b?tab=readme#x", repo("a", "b", &[])),
            ("https://github.com/a/b/tree/main", repo("a", "b", &["main"])),
            ("https://github.com/a/b/tree/feature/x/src/lib", repo("a", "b", &["feature", "x", "src", "lib"])),
            ("https://github.com/a/b/blob/v1.0/README.md", repo("a", "b", &["v1.0", "README.md"])),
            ("https://github.com/a/b/tree/main/my%20dir", repo("a", "b", &["main", "my dir"])),
            ("https://GitHub.com/A-1/x_y.z", repo("A-1", "x_y.z", &[])),
        ] {
            assert_eq!(parse_repo(url).unwrap(), want, "{url}");
        }
        for url in [
            "",
            "https://github.com/",
            "https://github.com/only-owner",
            "http://github.com/a/b",
            "ftp://github.com/a/b",
            "https://github.com:8443/a/b",
            "https://user:pass@github.com/a/b",
            "https://token@github.com/a/b",
            "https://github.com@evil.example/a/b",
            "https://github.com.evil.example/a/b",
            "https://evil.example/github.com/a/b",
            "https://gist.github.com/a/b",
            "https://140.82.112.3/a/b",
            "https://[::1]/a/b",
            "https://github.com/a/b/issues/3",
            "https://github.com/a/b/tree",
            "https://github.com/a/b/tree/main/a%2Fb",
            "https://github.com/a/b/tree/main/a%2F..%2F..%2Fb",
            "https://github.com/a/b/tree/main/a%5Cb",
            "https://github.com/a/b/tree/main/%0a",
            "https://github.com/a/b/tree/main/%zz",
            "https://github.com/-a/b",
            "https://github.com/a b/c",
            "https://github.com/a/..",
            "https://github.com/a/b%3Fc",
            "javascript:alert(1)",
        ] {
            assert_eq!(parse_repo(url).unwrap_err().kind(), "github_bad_url", "{url}");
        }
    }

    fn entry(path: &str, size: u64) -> Entry {
        Entry { path: path.into(), kind: "blob".into(), mode: "100644".into(), size }
    }

    #[test]
    fn the_picker_lists_text_files_under_the_folder() {
        let tree = vec![
            entry("src/main.rs", 10),
            entry("node_modules/x/index.js", 5),
            entry("logo.png", 3),
            entry("data/big.csv", MAX_FILE_BYTES + 1),
            entry("src/lib.rs", 20),
            entry("src.rs", 1),
            Entry { path: "link".into(), kind: "blob".into(), mode: "120000".into(), size: 4 },
            Entry { path: "sub".into(), kind: "commit".into(), mode: "160000".into(), size: 0 },
            Entry { path: "src".into(), kind: "tree".into(), mode: "040000".into(), size: 0 },
            entry("bad/\u{7}name", 1),
            entry("build.rs", 1),
        ];
        let (all, cut) = listing(tree, "");
        let paths: Vec<&str> = all.iter().map(|f| f["path"].as_str().unwrap()).collect();
        assert_eq!(paths, ["build.rs", "data/big.csv", "logo.png", "src.rs", "src/lib.rs", "src/main.rs", "node_modules/x/index.js"]);
        assert!(!cut);
        let by = |p: &str| all.iter().find(|f| f["path"] == p).unwrap().clone();
        assert_eq!(by("logo.png")["skip"], "binary");
        assert_eq!(by("data/big.csv")["skip"], "large");
        assert_eq!(by("src/main.rs")["skip"], Value::Null);
        assert_eq!(by("node_modules/x/index.js")["vendored"], true);
        assert_eq!(by("build.rs")["vendored"], false, "a file named like a vendored folder");
        let (src, _) = listing(vec![entry("src/main.rs", 1), entry("src.rs", 1), entry("srcx/a", 1)], "src");
        assert_eq!(src.len(), 1);
        let (one, _) = listing(vec![entry("src/main.rs", 1), entry("src/main.rs.bak", 1)], "src/main.rs");
        assert_eq!(one.len(), 1, "a /blob/ address lists its file");
        let many: Vec<Entry> = (0..MAX_LISTED + 1).map(|i| entry(&format!("f{i}.txt"), 1)).collect();
        let (shown, cut) = listing(many, "");
        assert_eq!((shown.len(), cut), (MAX_LISTED, true));
    }

    #[test]
    fn a_pick_is_checked_against_the_caps() {
        let ok: Vec<String> = (0..MAX_FILES).map(|i| format!("src/f{i}.rs")).collect();
        assert!(check_pick(&ok).is_ok());
        let over: Vec<String> = (0..=MAX_FILES).map(|i| format!("f{i}")).collect();
        assert_eq!(check_pick(&over).unwrap_err().kind(), "github_too_many_files");
        for bad in ["../etc/passwd", "/abs", "a//b", "a/./b", "a\\b", "a/\u{0}", ""] {
            assert_eq!(check_pick(&[bad.to_string()]).unwrap_err().kind(), "github_bad_path", "{bad:?}");
        }
    }

    #[test]
    fn tokens_are_checked_before_they_are_kept() {
        assert!(check_token(TOKEN).is_ok());
        assert!(check_token(&format!("github_pat_{}", "A1_".repeat(27))).is_ok());
        assert!(check_token("0123456789abcdef0123456789abcdef01234567").is_ok());
        for bad in ["short", "ghp_abc def0123456789012345678", "ghp_abc\r\nX-Evil: 1aaaaaaaaaaaaa", &"a".repeat(256)] {
            assert_eq!(check_token(bad).unwrap_err().kind(), "github_token_invalid");
        }
    }

    #[tokio::test]
    async fn the_token_is_kept_in_the_secret_store() {
        assert_eq!(github_token_set(Some(format!("  {TOKEN} "))).await.unwrap()["saved"], true);
        assert_eq!(saved_token().as_deref(), Some(TOKEN));
        let err = github_token_set(Some("not a token".into())).await.unwrap_err();
        assert_eq!(err.kind(), "github_token_invalid");
        assert_eq!(saved_token().as_deref(), Some(TOKEN), "a bad one doesn't replace it");
        assert_eq!(github_token_set(Some(" ".into())).await.unwrap()["saved"], false);
        assert_eq!(saved_token(), None);
    }

    /// A GitHub stand-in: repository a/b at SHA, branch feature/x, a few files;
    /// every answer that fails echoes the Authorization header back.
    async fn github() -> (Github, Arc<parking_lot::Mutex<Vec<mock::Seen>>>) {
        let (base, seen) = mock::serve(|r| {
            let echo = r.auth.clone().unwrap_or_default().into_bytes();
            let tree = json!({"truncated": false, "tree": [
                {"path": "README.md", "type": "blob", "mode": "100644", "size": 7},
                {"path": "src", "type": "tree", "mode": "040000"},
                {"path": "src/a.rs", "type": "blob", "mode": "100644", "size": 9},
                {"path": "src/pic.txt", "type": "blob", "mode": "100644", "size": 8},
                {"path": "src/big.txt", "type": "blob", "mode": "100644", "size": 10},
            ]});
            let mut png = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(2, 2).write_to(&mut png, image::ImageFormat::Png).unwrap();
            match r.path.as_str() {
                "/api/repos/a/b/commits/HEAD" | "/api/repos/a/b/commits/feature/x" => (200, vec![], SHA.into()),
                "/api/repos/a/b/commits/feature" | "/api/repos/a/b/commits/nope" => (422, vec![], echo),
                p if p.starts_with("/api/repos/moved/") => (301, vec![("location", p.replacen("/moved/", "/a/", 1))], vec![]),
                "/api/repos/limited/b/commits/HEAD" => (403, vec![("x-ratelimit-remaining", "0".into())], echo),
                "/api/repos/secret/b/commits/HEAD" => (403, vec![], echo),
                "/api/repos/expired/b/commits/HEAD" => (401, vec![], echo),
                "/api/repos/empty/b/commits/HEAD" => (409, vec![], echo),
                "/api/repos/down/b/commits/HEAD" => (502, vec![], echo),
                p if p == format!("/api/repos/a/b/git/trees/{SHA}?recursive=1") => (200, vec![], tree.to_string().into_bytes()),
                p if p == format!("/raw/a/b/{SHA}/README.md") => (200, vec![], b"# Hello".to_vec()),
                p if p == format!("/raw/a/b/{SHA}/src/a.rs") => (200, vec![], b"fn a() {}".to_vec()),
                p if p == format!("/raw/a/b/{SHA}/src/pic.txt") => (200, vec![], png.into_inner()),
                p if p == format!("/raw/a/b/{SHA}/src/big.txt") => (200, vec![], vec![b'x'; MAX_FILE_BYTES as usize + 1]),
                p if p.starts_with(&format!("/raw/a/b/{SHA}/f")) => (200, vec![], vec![b'y'; MAX_FILE_BYTES as usize]),
                _ => (404, vec![], echo),
            }
        })
        .await;
        (Github::at(&base), seen)
    }

    #[tokio::test]
    async fn a_repository_is_listed_and_its_files_fetched() {
        let (gh, seen) = github().await;
        let l = list(&gh, Some(TOKEN), "https://github.com/a/b").await.unwrap();
        assert_eq!((l["sha"].as_str(), l["ref"].as_str(), l["path"].as_str()), (Some(SHA), None, Some("")));
        assert_eq!(l["files"].as_array().unwrap().len(), 4);
        assert!(!l.to_string().contains(TOKEN), "the listing never carries the token");
        assert!(seen.lock().iter().all(|r| r.auth.as_deref() == Some(&format!("Bearer {TOKEN}")[..])), "{:?}", seen.lock());
        assert_eq!(seen.lock()[0].accept.as_deref(), Some("application/vnd.github.sha"));
        // a branch with a slash: the shortest that exists, the rest is the folder
        let l = list(&gh, None, "https://github.com/a/b/tree/feature/x/src").await.unwrap();
        assert_eq!((l["ref"].as_str(), l["path"].as_str()), (Some("feature/x"), Some("src")));
        assert_eq!(l["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect::<Vec<_>>(), ["src/a.rs", "src/big.txt", "src/pic.txt"]);
        assert_eq!(list(&gh, None, "https://github.com/a/b/tree/nope").await.unwrap_err().kind(), "github_ref_not_found");
        assert_eq!(list(&gh, None, "https://github.com/a/b/tree/feature/x/missing").await.unwrap_err().kind(), "github_path_not_found");
        // a renamed repository's redirect within GitHub is followed
        assert_eq!(list(&gh, None, "https://github.com/moved/b").await.unwrap()["sha"], SHA);

        let paths: Vec<String> = ["README.md", "src/a.rs", "src/pic.txt", "src/big.txt"].map(String::from).to_vec();
        let got = fetch(&gh, Some(TOKEN), "https://github.com/a/b", SHA, &paths).await.unwrap();
        assert_eq!(got[0].0, "b/README.md");
        assert_eq!(got[0].1.as_deref().unwrap(), b"# Hello");
        assert_eq!(attach(&got[1].0, got[1].1.as_ref().unwrap()).unwrap().name, "b/src/a.rs");
        assert_eq!(attach(&got[2].0, got[2].1.as_ref().unwrap()).unwrap_err().kind(), "github_not_text", "a picture named .txt");
        assert_eq!(got[3].1.as_ref().unwrap_err().kind(), "github_file_too_large");
        assert!(seen.lock().iter().filter(|r| r.path.starts_with("/raw/")).all(|r| r.auth.is_some()), "private repositories' files need it too");
        assert!(fetch(&gh, None, "https://github.com/a/b", "main", &paths).await.is_err(), "only the SHA the listing gave");
    }

    #[tokio::test]
    async fn the_pick_stops_at_the_total_cap() {
        let (gh, _) = github().await;
        let paths: Vec<String> = (0..6).map(|i| format!("f{i}.txt")).collect();
        let got = fetch(&gh, None, "https://github.com/a/b", SHA, &paths).await.unwrap();
        let ok = got.iter().filter(|(_, r)| r.is_ok()).count();
        assert_eq!(ok as u64, MAX_TOTAL_BYTES / MAX_FILE_BYTES, "{:?}", got.iter().map(|(_, r)| r.as_ref().map(|b| b.len()).map_err(|e| e.kind().to_string())).collect::<Vec<_>>());
        assert!(got.iter().filter_map(|(_, r)| r.as_ref().err()).all(|e| e.kind() == "github_selection_too_large"));
    }

    #[tokio::test]
    async fn refusals_are_named_and_never_carry_the_token() {
        let (gh, _) = github().await;
        for (owner, token, kind) in [
            ("limited", None, "github_rate_limited"),
            ("limited", Some(TOKEN), "github_rate_limited_token"),
            ("secret", Some(TOKEN), "github_forbidden"),
            ("expired", Some(TOKEN), "github_bad_token"),
            ("empty", Some(TOKEN), "github_empty"),
            ("down", Some(TOKEN), "github_failed"),
            ("nobody", Some(TOKEN), "github_not_found"),
        ] {
            let e = list(&gh, token, &format!("https://github.com/{owner}/b")).await.unwrap_err();
            assert_eq!(e.kind(), kind, "{owner}");
            assert!(!e.to_string().contains(TOKEN) && !format!("{e:?}").contains(TOKEN), "{e:?}");
            assert!(!serde_json::to_string(&e).unwrap().contains(TOKEN));
        }
        let e = fetch(&gh, Some(TOKEN), "https://github.com/a/b", SHA, &["nothing".into()]).await.unwrap()[0].1.as_ref().unwrap_err().to_string();
        assert!(!e.contains(TOKEN), "{e}");
    }

    #[tokio::test]
    async fn a_redirect_off_github_is_not_followed() {
        let (elsewhere, asked) = mock::serve(|_| (200, vec![], SHA.into())).await;
        let to = format!("{elsewhere}api/repos/a/b/commits/HEAD");
        let (base, _) = mock::serve(move |_| (302, vec![("location", to.clone())], vec![])).await;
        let e = list(&Github::at(&base), Some(TOKEN), "https://github.com/a/b").await.unwrap_err();
        assert_eq!(e.kind(), "github_failed");
        assert!(asked.lock().is_empty(), "the token went nowhere else");
    }
}
