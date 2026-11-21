//! The app's log file, `<data dir>/logs/clauderust.log`: a release build has
//! no console, so `log::warn!`/`error!` and panics would otherwise vanish.
//! One file of at most 2 MB plus the previous one (`clauderust.old.log`).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 2 * 1024 * 1024;

struct FileLog {
    path: PathBuf,
    max: u64,
    /// The user's home folder as messages spell it, written as `~`: the log
    /// is meant to be attached to bug reports.
    homes: Vec<String>,
    file: parking_lot::Mutex<Option<(File, u64)>>,
}

impl FileLog {
    fn new(dir: &Path, max: u64, home: Option<&Path>) -> Self {
        let mut homes = vec![];
        if let Some(h) = home.filter(|h| h.file_name().is_some()) {
            let h = h.display().to_string();
            homes.push(h.replace('\\', "/"));
            if h.contains('\\') {
                homes.push(h);
            }
        }
        Self { path: dir.join("clauderust.log"), max, homes, file: Default::default() }
    }

    fn line(&self, record: &log::Record) -> String {
        let mut msg = record.args().to_string();
        for h in &self.homes {
            msg = msg.replace(h.as_str(), "~");
        }
        format!("{} {:<5} {}: {msg}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"), record.level(), record.target())
    }
}

impl log::Log for FileLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        // the app's own notes; only problems from the libraries
        m.level() <= log::Level::Warn || (m.level() <= log::Level::Info && m.target().starts_with("clauderust"))
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = self.line(record);
        let mut slot = self.file.lock();
        if slot.as_ref().is_some_and(|(_, len)| len + line.len() as u64 > self.max) {
            // closed before the rename: Windows can't rename an open file
            *slot = None;
            let _ = std::fs::rename(&self.path, self.path.with_extension("old.log"));
        }
        if slot.is_none() {
            *slot = OpenOptions::new().create(true).append(true).open(&self.path).ok().map(|f| {
                let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                (f, len)
            });
        }
        if let Some((f, len)) = slot.as_mut() {
            if f.write_all(line.as_bytes()).is_ok() {
                *len += line.len() as u64;
            }
        }
    }

    fn flush(&self) {}
}

/// Log to `dir` from now on, panics included (with a backtrace).
pub fn init(dir: &Path) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if log::set_boxed_logger(Box::new(FileLog::new(dir, MAX_BYTES, dirs::home_dir().as_deref()))).is_err() {
        return;
    }
    log::set_max_level(log::LevelFilter::Info);
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        log::error!("thread '{}' {info}\n{}", thread.name().unwrap_or("<unnamed>"), std::backtrace::Backtrace::force_capture());
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log;

    fn write(l: &FileLog, level: log::Level, target: &str, msg: &str) {
        l.log(&log::Record::builder().level(level).target(target).args(format_args!("{msg}")).build());
    }

    #[test]
    fn keeps_app_notes_and_library_problems_and_rotates() {
        let dir = std::env::temp_dir().join(format!("cr-applog-{}", crate::util::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let home = if cfg!(windows) { PathBuf::from(r"C:\Users\someone") } else { PathBuf::from("/home/someone") };
        let l = FileLog::new(&dir, 300, Some(&home));
        write(&l, log::Level::Info, "clauderust_lib::updates", "update check: offline");
        write(&l, log::Level::Info, "reqwest::connect", "chatty library");
        write(&l, log::Level::Warn, "tao::platform", "library problem");
        let file = format!("{}/x.db", home.display());
        write(&l, log::Level::Error, "clauderust_lib", &format!("opening {file} failed; also {}", file.replace('\\', "/")));
        let text = std::fs::read_to_string(dir.join("clauderust.log")).unwrap();
        assert!(text.contains("INFO  clauderust_lib::updates: update check: offline"), "{text}");
        assert!(!text.contains("chatty") && text.contains("library problem"), "{text}");
        assert!(text.contains("opening ~/x.db failed; also ~/x.db") && !text.contains("someone"), "{text}");
        // past the limit the file starts over, the previous one kept
        write(&l, log::Level::Warn, "clauderust_lib", &"y".repeat(200));
        let old = std::fs::read_to_string(dir.join("clauderust.old.log")).unwrap();
        assert!(old.contains("update check"), "{old}");
        let text = std::fs::read_to_string(dir.join("clauderust.log")).unwrap();
        assert!(text.contains(&"y".repeat(200)) && !text.contains("update check"), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }

    // the only test that installs the process-wide logger and panic hook
    #[test]
    fn panics_are_logged_with_where_they_happened() {
        let dir = std::env::temp_dir().join(format!("cr-applog-panic-{}", crate::util::new_id()));
        init(&dir);
        let _ = std::panic::catch_unwind(|| panic!("boom in a test"));
        let text = std::fs::read_to_string(dir.join("clauderust.log")).unwrap();
        assert!(text.contains("ERROR") && text.contains("panicked at") && text.contains("applog.rs") && text.contains("boom in a test"), "{text}");
    }

    #[test]
    fn a_root_home_is_not_redacted() {
        let l = FileLog::new(Path::new("."), 1, Some(Path::new("/")));
        assert!(l.homes.is_empty());
    }
}
