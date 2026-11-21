use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

pub fn hash64(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Truncate to at most `max` chars (not bytes), appending an ellipsis marker.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Truncate keeping the head and tail, used for long tool outputs.
pub fn truncate_middle(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let head = max * 2 / 3;
    let tail = max - head;
    let h: String = s.chars().take(head).collect();
    let t: String = s.chars().skip(n - tail).collect();
    format!(
        "{h}\n\n... [{} characters truncated] ...\n\n{t}",
        n - head - tail
    )
}

pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn human_size(bytes: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// Read at most `limit` bytes: `Ok(None)` when there is more, or when the
/// `declared` size (a zip entry's header, which can lie either way) is
/// already over it. Keeps a small zip that expands to gigabytes out of
/// memory.
pub fn read_capped(r: impl std::io::Read, declared: u64, limit: u64) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    if declared > limit {
        return Ok(None);
    }
    let mut buf = Vec::with_capacity(declared.min(limit).min(64 * 1024 * 1024) as usize);
    r.take(limit + 1).read_to_end(&mut buf)?;
    Ok((buf.len() as u64 <= limit).then_some(buf))
}

/// Where writing `path` really lands: symlinks are followed (dangling ones
/// too, whose target is then created), so the link itself is kept.
fn resolve_links(path: &Path) -> std::io::Result<PathBuf> {
    let mut p = path.to_path_buf();
    for _ in 0..40 {
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => {
                let target = std::fs::read_link(&p)?;
                p = if target.is_absolute() {
                    target
                } else {
                    p.parent().map(|d| d.join(&target)).unwrap_or(target)
                };
            }
            _ => return Ok(p),
        }
    }
    Err(std::io::Error::other(format!("{}: too many levels of symbolic links", path.display())))
}

/// Replace a file's content atomically: the bytes go to a temporary file in
/// the same folder, are flushed to disk, and the temporary file is renamed
/// over the target. A crash or a full disk leaves either the old or the new
/// content, never a truncated file. The file's permissions are kept; when
/// `path` is a symlink its target is written and the link stays. (Other
/// hard links to the file keep the old content, as with most editors.)
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let target = resolve_links(path)?;
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = target
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} is not a file path", path.display())))?
        .to_string_lossy()
        .to_string();
    let perms = match std::fs::metadata(&target) {
        Ok(m) if m.is_dir() => {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} is a folder", target.display())));
        }
        Ok(m) => {
            // a file we may not write stays as it is (a plain write would
            // fail too; the rename alone would get past a read-only file)
            std::fs::OpenOptions::new().write(true).open(&target)?;
            Some(m.permissions())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let tmp = dir.join(format!(".{name}.{}.tmp", &new_id()[..8]));
    let written = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        if let Some(p) = &perms {
            f.set_permissions(p.clone())?;
        }
        f.sync_all()
    })();
    if let Err(e) = written.and_then(|_| rename_over(&tmp, &target)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(&dir) {
        // the rename itself is durable once the folder is synced
        let _ = d.sync_all();
    }
    Ok(())
}

/// Rename `from` over `to`. On Windows a virus scanner or indexer briefly
/// holding the target makes the rename fail with "access denied": retried.
fn rename_over(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied && tries < 10 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            r => return r,
        }
    }
}

/// How a text file was encoded, from its byte-order mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

/// Decode UTF-16 code units (`be`: big-endian); `None` for an odd length or
/// unpaired surrogates.
pub fn utf16(bytes: &[u8], be: bool) -> Option<String> {
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units = bytes.chunks_exact(2).map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) });
    char::decode_utf16(units).collect::<Result<String, _>>().ok()
}

/// Decode text that is UTF-8 (with or without a byte-order mark) or UTF-16
/// with a byte-order mark (what Windows tools such as PowerShell often
/// write). `None` when the bytes are neither.
pub fn decode_text(bytes: &[u8]) -> Option<(String, TextEncoding)> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return std::str::from_utf8(rest).ok().map(|s| (s.to_string(), TextEncoding::Utf8Bom));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, false).map(|s| (s, TextEncoding::Utf16Le));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, true).map(|s| (s, TextEncoding::Utf16Be));
    }
    std::str::from_utf8(bytes).ok().map(|s| (s.to_string(), TextEncoding::Utf8))
}

/// `text` encoded the way a file read with `decode_text` was.
pub fn encode_text(text: &str, enc: TextEncoding) -> Vec<u8> {
    match enc {
        TextEncoding::Utf8 => text.as_bytes().to_vec(),
        TextEncoding::Utf8Bom => [&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat(),
        TextEncoding::Utf16Le => std::iter::once(0xFEFFu16).chain(text.encode_utf16()).flat_map(u16::to_le_bytes).collect(),
        TextEncoding::Utf16Be => std::iter::once(0xFEFFu16).chain(text.encode_utf16()).flat_map(u16::to_be_bytes).collect(),
    }
}

/// A string cache bounded by the total size of its values: values larger
/// than `max_entry` are not kept, and the oldest entries go first when the
/// total would pass `max_total`.
pub struct ByteCache {
    map: std::collections::HashMap<u64, String>,
    order: std::collections::VecDeque<u64>,
    bytes: usize,
    max_total: usize,
    max_entry: usize,
}

impl ByteCache {
    pub fn new(max_total: usize, max_entry: usize) -> Self {
        ByteCache { map: Default::default(), order: Default::default(), bytes: 0, max_total, max_entry: max_entry.min(max_total) }
    }

    pub fn get(&self, key: u64) -> Option<&String> {
        self.map.get(&key)
    }

    pub fn insert(&mut self, key: u64, value: String) {
        if value.len() > self.max_entry {
            return;
        }
        let len = value.len();
        match self.map.insert(key, value) {
            Some(old) => self.bytes -= old.len(),
            None => self.order.push_back(key),
        }
        self.bytes += len;
        while self.bytes > self.max_total {
            let Some(k) = self.order.pop_front() else { break };
            if let Some(v) = self.map.remove(&k) {
                self.bytes -= v.len();
            }
        }
    }

    /// Total size of the cached values, in bytes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cr-util-{tag}-{}", new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn atomic_writes_replace_content_and_leave_no_temp_files() {
        let d = tmp_dir("aw");
        let f = d.join("a.txt");
        write_atomic(&f, b"one").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"one");
        write_atomic(&f, b"two, longer").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"two, longer");
        let names: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "{names:?}");
        // a folder isn't replaced by a file
        assert!(write_atomic(&d, b"x").is_err());
        assert!(d.is_dir());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn failed_atomic_write_keeps_the_old_content() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp_dir("awf");
        let f = d.join("a.txt");
        std::fs::write(&f, "precious").unwrap();
        // a read-only file isn't replaced (as a plain write would fail)
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o444)).unwrap();
        let writable_anyway = std::fs::OpenOptions::new().write(true).open(&f).is_ok(); // running as root
        if !writable_anyway {
            assert!(write_atomic(&f, b"x").is_err());
            assert_eq!(std::fs::read_to_string(&f).unwrap(), "precious");
        }
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        // the temporary file can't be created: nothing changes
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap();
        if !writable_anyway {
            assert!(write_atomic(&f, b"x").is_err());
            assert_eq!(std::fs::read_to_string(&f).unwrap(), "precious");
        }
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_writes_keep_permissions_and_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp_dir("awl");
        let real = d.join("real.sh");
        std::fs::write(&real, "old").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o750)).unwrap();
        let link = d.join("link.sh");
        std::os::unix::fs::symlink("real.sh", &link).unwrap();
        write_atomic(&link, b"new").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link is kept");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
        assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o750);
        // a dangling link: its target is created
        let dangling = d.join("dangling");
        std::os::unix::fs::symlink("made.txt", &dangling).unwrap();
        write_atomic(&dangling, b"hi").unwrap();
        assert_eq!(std::fs::read_to_string(d.join("made.txt")).unwrap(), "hi");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn text_decoding_handles_boms() {
        let le = encode_text("héllo\r\n", TextEncoding::Utf16Le);
        assert_eq!(&le[..2], &[0xFF, 0xFE]);
        assert_eq!(decode_text(&le), Some(("héllo\r\n".to_string(), TextEncoding::Utf16Le)));
        let be = encode_text("😀 x", TextEncoding::Utf16Be);
        assert_eq!(decode_text(&be), Some(("😀 x".to_string(), TextEncoding::Utf16Be)));
        assert_eq!(decode_text(b"\xEF\xBB\xBFabc"), Some(("abc".to_string(), TextEncoding::Utf8Bom)));
        assert_eq!(decode_text(b"abc"), Some(("abc".to_string(), TextEncoding::Utf8)));
        assert_eq!(encode_text("abc", TextEncoding::Utf8Bom), b"\xEF\xBB\xBFabc");
        // cp1254 / latin-1 bytes, and broken UTF-16
        assert_eq!(decode_text(b"G\xFCnayd\xFDn"), None);
        assert_eq!(decode_text(&[0xFF, 0xFE, 0x00, 0xD8]), None, "unpaired surrogate");
        assert_eq!(decode_text(&[0xFF, 0xFE, 0x41]), None, "odd length");
    }

    #[test]
    fn byte_cache_is_bounded_by_size() {
        let mut c = ByteCache::new(1000, 400);
        c.insert(1, "x".repeat(500));
        assert!(c.get(1).is_none(), "too large to keep");
        for k in 0..10 {
            c.insert(k, "y".repeat(300));
        }
        assert!(c.bytes() <= 1000 && c.len() == 3, "{} bytes, {} entries", c.bytes(), c.len());
        assert!(c.get(9).is_some() && c.get(0).is_none(), "oldest go first");
        c.insert(9, "z".repeat(10));
        assert_eq!(c.get(9).map(|s| s.len()), Some(10));
        assert_eq!(c.bytes(), 610);
    }
}
