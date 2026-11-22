//! Code execution and file creation in chats: Anthropic's sandboxed code
//! execution tool with the pre-built document skills (xlsx, pptx, docx,
//! pdf). Files the code leaves in `$OUTPUT_DIR` come back as Files API ids;
//! they are downloaded into a folder of the conversation's under the app's
//! data (never opened by themselves) and listed in the reply's `meta.files`.

use crate::anthropic::Client;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub const TOOL_NAME: &str = "code_execution";

/// Files kept of one reply; the rest are listed as refused.
pub const MAX_FILES: usize = 10;
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

const SKILLS: [&str; 4] = ["xlsx", "pptx", "docx", "pdf"];

/// The tool (no beta header since the 2026-05-21 version).
pub fn definition() -> Value {
    json!({"type": "code_execution_20260521", "name": TOOL_NAME})
}

/// The request's `container`: the document skills, in the container `id` of
/// the conversation's last reply while it lives (its files are still there).
pub fn container(id: Option<&str>) -> Value {
    let skills: Vec<Value> = SKILLS.iter().map(|s| json!({"type": "anthropic", "skill_id": s, "version": "latest"})).collect();
    let mut c = json!({"skills": skills});
    if let Some(id) = id {
        c["id"] = json!(id);
    }
    c
}

/// The ids of the files the reply's commands left in `$OUTPUT_DIR`, in order.
pub fn output_file_ids(content: &[Value]) -> Vec<String> {
    content
        .iter()
        .filter(|b| b["type"] == "bash_code_execution_tool_result" && b["content"]["type"] == "bash_code_execution_result")
        .flat_map(|b| b["content"]["content"].as_array().into_iter().flatten())
        .filter_map(|o| o["file_id"].as_str().map(String::from))
        .collect()
}

/// An id as the Files API gives them ("file_011CNha8…"): it goes into a URL path.
fn valid_file_id(id: &str) -> bool {
    id.len() <= 128 && id.strip_prefix("file_").is_some_and(|r| !r.is_empty() && r.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'))
}

/// `name` as a file name on every system, or None when it isn't a plain file
/// name (a path, control characters, what Windows refuses). A device name
/// (CON, NUL, COM1…) gets a leading `_`.
pub fn safe_name(name: &str) -> Option<String> {
    // and no invisible or direction marks: "photo\u{202E}gnp.exe" would show as "photoexe.png"
    let bad = |c: char| {
        c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
    };
    // a trailing dot or space covers "." and ".." too
    if name.is_empty() || name.chars().count() > 200 || name.chars().any(bad) || name.starts_with(' ') || name.ends_with(['.', ' ']) {
        return None;
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let numbered = (stem.starts_with("COM") || stem.starts_with("LPT")) && matches!(&stem[3..], "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³");
    let device = numbered || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$");
    Some(if device { format!("_{name}") } else { name.to_string() })
}

/// The folder of a conversation's downloaded files.
pub fn outputs_dir(data_dir: &Path, conv_id: &str) -> PathBuf {
    data_dir.join("outputs").join(conv_id)
}

/// Remove a conversation's downloaded files (when it is deleted).
pub fn remove_outputs(data_dir: &Path, conv_id: &str) {
    if plain(conv_id) {
        let _ = std::fs::remove_dir_all(outputs_dir(data_dir, conv_id));
    }
}

/// A single path component of our own (a conversation id, a saved name).
fn plain(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.contains(['/', '\\', ':'])
}

/// Write `bytes` as `name` in `dir` without replacing a file there: a taken
/// name becomes "name (2).ext" and so on. The name it got.
fn write_new(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<String> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    };
    for n in 1..1000 {
        let candidate = if n == 1 { name.to_string() } else { format!("{stem} ({n}){ext}") };
        let p = dir.join(&candidate);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(bytes) {
                    drop(f);
                    let _ = std::fs::remove_file(&p);
                    return Err(e);
                }
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many files with this name"))
}

/// Download the files `ids` into `dir`: per file `{file_id, name, size, mime,
/// path}`, or `{file_id, name, error}` with an error code the UI words
/// (`bad_id`, `bad_name`, `too_large`, `too_many`, `not_downloadable`, `failed`).
pub async fn save_outputs(client: &Client, dir: &Path, ids: &[String], cancel: &CancellationToken) -> Vec<Value> {
    let mut out = vec![];
    for (i, id) in ids.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        out.push(if i < MAX_FILES { save_one(client, dir, id, cancel).await } else { json!({"file_id": id, "name": null, "error": "too_many"}) });
    }
    out
}

async fn save_one(client: &Client, dir: &Path, id: &str, cancel: &CancellationToken) -> Value {
    let fail = |name: Option<String>, code: &str| json!({"file_id": if valid_file_id(id) { id } else { "" }, "name": name, "error": code});
    if !valid_file_id(id) {
        return fail(None, "bad_id");
    }
    let Ok(meta) = client.get_json(&format!("/v1/files/{id}"), cancel).await else { return fail(None, "failed") };
    let given = meta["filename"].as_str().unwrap_or("");
    let shown: String = given.chars().filter(|c| !c.is_control()).take(200).collect();
    let Some(name) = safe_name(given) else { return fail(Some(shown), "bad_name") };
    if meta["downloadable"] == false {
        return fail(Some(name), "not_downloadable");
    }
    if meta["size_bytes"].as_u64().unwrap_or(0) > MAX_FILE_BYTES {
        return fail(Some(name), "too_large");
    }
    let bytes = match client.download(&format!("/v1/files/{id}/content"), MAX_FILE_BYTES, cancel).await {
        Ok(Some(b)) => b,
        Ok(None) => return fail(Some(name), "too_large"),
        Err(_) => return fail(Some(name), "failed"),
    };
    let size = bytes.len();
    let (d, n) = (dir.to_path_buf(), name.clone());
    match tokio::task::spawn_blocking(move || write_new(&d, &n, &bytes)).await {
        Ok(Ok(saved)) => json!({"file_id": id, "name": saved, "size": size, "mime": meta["mime_type"], "path": dir.join(&saved).display().to_string()}),
        Ok(Err(e)) => {
            log::warn!("saving a file Claude made failed: {e}");
            fail(Some(name), "failed")
        }
        Err(_) => fail(Some(name), "failed"),
    }
}

/// Save a copy of a file a chat's code made (`meta.files[].name`) where the
/// user picks; false when the dialog was cancelled.
#[tauri::command]
pub async fn save_output_file(app: tauri::AppHandle, state: tauri::State<'_, Arc<AppState>>, conversation_id: String, name: String) -> AppResult<bool> {
    use tauri_plugin_dialog::DialogExt;
    let src = outputs_dir(&state.data_dir, &conversation_id).join(&name);
    if !plain(&conversation_id) || !plain(&name) || !src.is_file() {
        return Err(AppError::Api { status: 404, kind: "output_missing".into(), message: "That file is no longer there.".into() });
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_file_name(&name).save_file(move |p| {
        let _ = tx.send(p.and_then(|p| p.into_path().ok()));
    });
    match rx.await.unwrap_or(None) {
        Some(dest) => {
            tokio::fs::copy(&src, dest).await?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_plain_and_never_a_path() {
        assert_eq!(safe_name("report.xlsx").as_deref(), Some("report.xlsx"));
        assert_eq!(safe_name("Q3 summary (final).pdf").as_deref(), Some("Q3 summary (final).pdf"));
        assert_eq!(safe_name("çizelge.xlsx").as_deref(), Some("çizelge.xlsx"));
        for hostile in ["../../evil.exe", "..\\evil.bat", "/etc/passwd", "C:evil", "a\u{0}b", "line\nbreak", "..", ".", "", "x.", "trailing ", " lead", "a|b", "what?.txt", &"x".repeat(201)] {
            assert_eq!(safe_name(hostile), None, "{hostile:?}");
        }
        assert_eq!(safe_name("CON").as_deref(), Some("_CON"));
        assert_eq!(safe_name("nul.txt").as_deref(), Some("_nul.txt"));
        assert_eq!(safe_name("com1.log").as_deref(), Some("_com1.log"));
        assert_eq!(safe_name("console.txt").as_deref(), Some("console.txt"));
        assert_eq!(safe_name("COM¹.txt").as_deref(), Some("_COM¹.txt"));
        assert_eq!(safe_name("conout$").as_deref(), Some("_conout$"));
        assert_eq!(safe_name("COM10.txt").as_deref(), Some("COM10.txt"));
        for spoof in ["photo\u{202E}gnp.exe", "a\u{200B}b.txt", "\u{FEFF}x.pdf", "x\u{2066}y.docx"] {
            assert_eq!(safe_name(spoof), None, "{spoof:?}");
        }
    }

    #[test]
    fn file_ids_are_checked_before_they_go_into_a_url() {
        assert!(valid_file_id("file_011CNha8iCJcU1wXNR6q4V8w"));
        for bad in ["file_", "file_../x", "file_a/b", "file_a?b=1", "fil_x", "file_a b", &format!("file_{}", "a".repeat(200))] {
            assert!(!valid_file_id(bad), "{bad}");
        }
    }

    #[test]
    fn a_saved_file_never_replaces_another() {
        let dir = std::env::temp_dir().join(format!("cr-outputs-{}", crate::util::new_id()));
        assert_eq!(write_new(&dir, "a.txt", b"1").unwrap(), "a.txt");
        assert_eq!(write_new(&dir, "a.txt", b"2").unwrap(), "a (2).txt");
        assert_eq!(write_new(&dir, "a.txt", b"3").unwrap(), "a (3).txt");
        assert_eq!(write_new(&dir, "noext", b"4").unwrap(), "noext");
        assert_eq!(write_new(&dir, "noext", b"5").unwrap(), "noext (2)");
        assert_eq!(std::fs::read(dir.join("a.txt")).unwrap(), b"1");
        assert_eq!(std::fs::read(dir.join("a (2).txt")).unwrap(), b"2");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn output_files_are_read_from_bash_results_only() {
        let content = vec![
            json!({"type": "server_tool_use", "id": "s1", "name": "bash_code_execution", "input": {"command": "ls"}}),
            json!({"type": "bash_code_execution_tool_result", "tool_use_id": "s1", "content": {"type": "bash_code_execution_result", "stdout": "", "stderr": "", "return_code": 0,
                "content": [{"type": "bash_code_execution_output", "file_id": "file_a"}, {"type": "bash_code_execution_output", "file_id": "file_b"}]}}),
            json!({"type": "bash_code_execution_tool_result", "tool_use_id": "s2", "content": {"type": "bash_code_execution_tool_result_error", "error_code": "unavailable"}}),
            json!({"type": "text_editor_code_execution_tool_result", "tool_use_id": "s3", "content": {"type": "text_editor_code_execution_create_result", "is_file_update": false}}),
        ];
        assert_eq!(output_file_ids(&content), ["file_a", "file_b"]);
        let c = container(Some("container_1"));
        assert_eq!(c["id"], "container_1");
        assert_eq!(c["skills"].as_array().unwrap().iter().map(|s| s["skill_id"].as_str().unwrap()).collect::<Vec<_>>(), SKILLS);
        assert!(container(None).get("id").is_none());
    }
}
