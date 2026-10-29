//! `pclink.log` beside the settings: each line with its local time. Past
//! 1 MB, at start or while running, it becomes `pclink.log.old`.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 1024 * 1024;

/// The open log, and where it is.
static FILE: OnceLock<Mutex<(std::fs::File, PathBuf)>> = OnceLock::new();

/// The log beside a settings file.
pub fn path_for(settings: &Path) -> PathBuf {
    settings.with_extension("log")
}

/// Opens the log for this run (once; later calls do nothing). Without one,
/// lines are only shown.
pub fn start(settings: &Path) {
    let path = path_for(settings);
    if let Some(file) = open(&path) {
        let _ = FILE.set(Mutex::new((file, path)));
    }
}

// The log, a new one when it is past MAX_BYTES (the old one kept as .old).
fn open(path: &Path) -> Option<std::fs::File> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    let mut open = std::fs::OpenOptions::new();
    open.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Never through a link someone put in its place.
        // Linux's value, which differs by architecture.
        #[cfg(any(target_arch = "aarch64", target_arch = "arm"))]
        const O_NOFOLLOW: i32 = 0o100000;
        #[cfg(not(any(target_arch = "aarch64", target_arch = "arm")))]
        const O_NOFOLLOW: i32 = 0o400000;
        open.mode(0o600).custom_flags(O_NOFOLLOW);
    }
    open.open(path).ok()
}

/// `line` with the date and time now, as shown and kept: one line, plain,
/// whatever a file name or the server put in it.
pub fn stamp(line: &str) -> String {
    format!("{}  {}", crate::os::local_time(), crate::protocol::plain(line, MAX_LINE))
}

/// The most a line keeps.
const MAX_LINE: usize = 2000;

/// Keeps a stamped line, when the log is open.
/// Past MAX_BYTES it starts afresh, so a busy watched folder cannot fill the disk.
pub fn keep(stamped: &str) {
    if let Some(Ok(mut log)) = FILE.get().map(Mutex::lock) {
        let (file, path) = &mut *log;
        let _ = file.write_all(format!("{stamped}{}", if cfg!(windows) { "\r\n" } else { "\n" }).as_bytes());
        if file.metadata().is_ok_and(|m| m.len() > MAX_BYTES)
            && let Some(fresh) = open(path)
        {
            *file = fresh;
        }
    }
}

/// The log's last `n` lines, oldest first: read from its last 64 KB.
pub fn tail(settings: &Path, n: usize) -> Vec<String> {
    let Ok(mut f) = std::fs::File::open(path_for(settings)) else { return vec![] };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(64 * 1024);
    let mut bytes = Vec::new();
    if f.seek(SeekFrom::Start(from)).is_err() || f.read_to_end(&mut bytes).is_err() {
        return vec![];
    }
    let text = String::from_utf8_lossy(&bytes);
    // A cut first line is left out.
    let lines: Vec<&str> = text.lines().skip(usize::from(from > 0)).filter(|l| !l.trim().is_empty()).collect();
    // Plain, whatever the file holds.
    lines[lines.len().saturating_sub(n)..].iter().map(|l| crate::protocol::plain(l, MAX_LINE + 30)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_is_the_last_lines_oldest_first() {
        let dir = std::env::temp_dir().join(format!("pclink-journal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("pclink.ini");
        let lines: Vec<String> = (0..300).map(|i| format!("line {i}")).collect();
        std::fs::write(path_for(&settings), lines.join("\n") + "\n").unwrap();
        assert_eq!(tail(&settings, 3), ["line 297", "line 298", "line 299"]);
        assert_eq!(tail(&dir.join("none.ini"), 3), Vec::<String>::new());
        let s = stamp("x");
        assert!(s.ends_with("  x") && s.len() == "2026-10-20 14:03:22  x".len(), "{s}");
        assert!(stamp("a\nfake line\u{202e}").ends_with("  afake line"), "a file name cannot add a line");
        // Past MAX_BYTES, a new log; the old one kept beside it.
        let log = path_for(&settings);
        std::fs::write(&log, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert!(open(&log).is_some());
        assert_eq!(std::fs::metadata(&log).unwrap().len(), 0);
        assert!(log.with_extension("log.old").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
