//! Settings → About → "Copy diagnostics": the end of the app's log file
//! (applog.rs). The UI redacts it before it goes to the clipboard.

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

/// At most this much of the log's end is read (the file itself is capped at 2 MB).
const TAIL_BYTES: u64 = 256 * 1024;

/// The last `n` lines of `<dir>/clauderust.log`; "" before anything was logged.
pub fn tail(dir: &Path, n: usize) -> AppResult<String> {
    let Ok(mut f) = std::fs::File::open(dir.join("clauderust.log")) else { return Ok(String::new()) };
    let start = f.metadata()?.len().saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.take(TAIL_BYTES).read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    // a read that starts mid-file starts mid-line
    let lines: Vec<&str> = text.lines().skip(usize::from(start > 0)).collect();
    Ok(lines[lines.len().saturating_sub(n)..].join("\n"))
}

#[tauri::command]
pub async fn log_tail(state: tauri::State<'_, Arc<AppState>>, lines: usize) -> AppResult<String> {
    let dir = state.data_dir.join("logs");
    tokio::task::spawn_blocking(move || tail(&dir, lines.min(1000))).await.map_err(|e| AppError::msg(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_lines_of_the_log() {
        let dir = std::env::temp_dir().join(format!("diag-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(tail(&dir, 100).unwrap(), "");
        std::fs::write(dir.join("clauderust.log"), "a\nb\nc\n").unwrap();
        assert_eq!(tail(&dir, 2).unwrap(), "b\nc");
        assert_eq!(tail(&dir, 100).unwrap(), "a\nb\nc");
        assert_eq!(tail(&dir, 0).unwrap(), "");
        // a big log: only its end is read, and the cut first line is dropped
        let long = (0..40_000).map(|i| format!("line {i:05}")).collect::<Vec<_>>().join("\n");
        std::fs::write(dir.join("clauderust.log"), &long).unwrap();
        let t = tail(&dir, 100_000).unwrap();
        assert!(t.ends_with("line 39999") && t.len() as u64 <= TAIL_BYTES, "{}", t.len());
        assert!(t.lines().all(|l| l.len() == 10), "a partial line was kept");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
