//! Files in what pclink.ini lists: one picked at random, and changes
//! watched. Only the file picked, or the one that changed, is told.

use crate::config::{FileWatch, Folder};
use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Files a watch looks at in one folder, subfolders included: more is not watched.
pub const MAX_FILES: usize = 5000;
/// A pick's time and file limits: the time ends a pick in a huge folder,
/// the count bounds memory.
pub const PICK_TIME: Duration = Duration::from_secs(3);
pub const PICK_FILES: usize = 250_000;
/// A watch's look, every 2 seconds: a folder taking longer is not watched.
const LOOK_TIME: Duration = Duration::from_millis(1500);
/// How deep subfolders go.
const MAX_DEPTH: usize = 8;

/// What a walk found, and whether that is all there is.
struct Found {
    files: Vec<PathBuf>,
    whole: bool,
}

// Files in `dir` (and subfolders) that `keep` takes: not hidden ones, not
// through a link to a folder. It stops at `max` of them, or at `until`.
fn walk(dir: &Path, subfolders: bool, max: usize, until: Instant, keep: impl Fn(&Path) -> bool) -> Found {
    let mut files = Vec::new();
    let mut todo = vec![(dir.to_path_buf(), 0)];
    while let Some((dir, depth)) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            if Instant::now() >= until {
                return Found { files, whole: false };
            }
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                if subfolders && depth < MAX_DEPTH {
                    todo.push((e.path(), depth + 1));
                }
            } else if (kind.is_file() || (kind.is_symlink() && e.path().is_file())) && keep(&e.path()) {
                if files.len() >= max {
                    return Found { files, whole: false };
                }
                files.push(e.path());
            }
        }
    }
    Found { files, whole: true }
}

// A random number below `n` (> 0). Not for secrets: which clip plays.
fn below(n: usize) -> usize {
    let mut h = RandomState::new().build_hasher();
    h.write_u128(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    (h.finish() % n as u64) as usize
}

/// Picks at random, keeping out a folder's last few picks.
#[derive(Default)]
pub struct Picker {
    recent: HashMap<String, VecDeque<PathBuf>>,
}

impl Picker {
    /// A file whose name has `matching` (lowercase, like .mp4), or why none:
    /// from what PICK_TIME finds, in a folder too big to look through whole.
    pub fn pick(&mut self, folder: &Folder, matching: &str) -> Result<PathBuf, String> {
        self.pick_within(folder, matching, PICK_TIME)
    }

    fn pick_within(&mut self, folder: &Folder, matching: &str, time: Duration) -> Result<PathBuf, String> {
        if !folder.path.is_dir() {
            // No path: this goes to the server too.
            return Err("the folder is not there".into());
        }
        let matches =
            |p: &Path| matching.is_empty() || p.file_name().is_some_and(|f| f.to_string_lossy().to_lowercase().contains(matching));
        let found = walk(&folder.path, folder.subfolders, PICK_FILES, Instant::now() + time, matches);
        let mut pool = found.files;
        if pool.is_empty() {
            return Err(if !found.whole {
                format!("nothing found in {} seconds: the folder is too big to look through", time.as_secs())
            } else if matching.is_empty() {
                "the folder is empty".into()
            } else {
                format!("no file has \"{matching}\"")
            });
        }
        pool.sort();
        let recent = self.recent.entry(folder.name.clone()).or_default();
        let keep_out = recent.len().min(pool.len() / 2);
        let fresh: Vec<&PathBuf> = pool.iter().filter(|p| !recent.iter().rev().take(keep_out).any(|r| r == *p)).collect();
        let file = if fresh.is_empty() { pool[below(pool.len())].clone() } else { fresh[below(fresh.len())].clone() };
        recent.push_back(file.clone());
        while recent.len() > 10 {
            recent.pop_front();
        }
        Ok(file)
    }
}

// --- watching ---------------------------------------------------------------------------

/// A file's size and last write.
pub type Stamp = (u64, Option<SystemTime>);

/// What happened to a file between two looks.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Created(PathBuf),
    Changed(PathBuf),
    Deleted(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}

impl Change {
    pub fn word(&self) -> &'static str {
        match self {
            Change::Created(_) => "created",
            Change::Changed(_) => "changed",
            Change::Deleted(_) => "deleted",
            Change::Renamed { .. } => "renamed",
        }
    }

    /// Where the file is now, or was.
    pub fn file(&self) -> &Path {
        match self {
            Change::Created(p) | Change::Changed(p) | Change::Deleted(p) => p,
            Change::Renamed { to, .. } => to,
        }
    }
}

/// The watched files now. None when that cannot be told (gone, unplugged, too
/// many), so nothing is said rather than every file deleted.
pub fn look(w: &FileWatch) -> Option<HashMap<PathBuf, Stamp>> {
    let stamp = |p: &Path| std::fs::metadata(p).ok().map(|m| (m.len(), m.modified().ok()));
    if w.path.is_dir() {
        let want = w.matching.to_lowercase();
        let matches = |p: &Path| want.is_empty() || p.file_name().is_some_and(|f| f.to_string_lossy().to_lowercase().contains(&want));
        let found = walk(&w.path, w.subfolders, MAX_FILES, Instant::now() + LOOK_TIME, matches);
        if !found.whole {
            return None;
        }
        // Each file's size and time, in the time a look has too (a slow share).
        let until = Instant::now() + LOOK_TIME;
        let mut out = HashMap::new();
        for p in found.files {
            if Instant::now() >= until {
                return None;
            }
            if let Some(s) = stamp(&p) {
                out.insert(p, s);
            }
        }
        return Some(out);
    }
    // One file: there or not, while its folder is.
    if !w.path.parent().is_some_and(Path::is_dir) {
        return None;
    }
    Some(stamp(&w.path).map(|s| (w.path.clone(), s)).into_iter().collect())
}

/// Looks, at most, for a file being written to settle.
const SETTLE: u32 = 15;

/// One watch's files between looks. A new or changed file is told once it
/// stops changing (a replay, once whole). One gone as another of the same
/// size and time appears was renamed.
#[derive(Debug, Default)]
pub struct Watcher {
    /// As last told, or as first seen.
    known: HashMap<PathBuf, Stamp>,
    /// Not told yet: its last stamp, and looks spent changing.
    moving: HashMap<PathBuf, (Stamp, u32)>,
    started: bool,
}

impl Watcher {
    /// What changed since the last look; nothing the first time.
    pub fn changes(&mut self, now: Option<HashMap<PathBuf, Stamp>>) -> Vec<Change> {
        let Some(now) = now else { return vec![] };
        if !self.started {
            self.started = true;
            self.known = now;
            return vec![];
        }
        let mut out = Vec::new();
        let mut gone: Vec<(PathBuf, Stamp)> =
            self.known.iter().filter(|(p, _)| !now.contains_key(*p)).map(|(p, s)| (p.clone(), *s)).collect();
        gone.sort();
        let mut fresh: Vec<(&PathBuf, &Stamp)> =
            now.iter().filter(|(p, _)| !self.known.contains_key(*p) && !self.moving.contains_key(*p)).collect();
        fresh.sort();
        for (to, stamp) in fresh {
            // Renamed: the same size and time as one just gone.
            if let Some(i) = gone.iter().position(|(_, s)| s == stamp && s.1.is_some()) {
                let (from, _) = gone.remove(i);
                self.known.remove(&from);
                self.known.insert(to.clone(), *stamp);
                out.push(Change::Renamed { from, to: to.clone() });
            }
        }
        for (p, _) in gone {
            self.known.remove(&p);
            out.push(Change::Deleted(p));
        }
        self.moving.retain(|p, _| now.contains_key(p));
        let mut settled = Vec::new();
        for (p, stamp) in &now {
            if self.known.get(p) == Some(stamp) {
                self.moving.remove(p);
                continue;
            }
            match self.moving.get_mut(p) {
                Some((was, looks)) if *was == *stamp || *looks + 1 >= SETTLE => settled.push((p.clone(), *stamp)),
                Some((was, looks)) => {
                    *was = *stamp;
                    *looks += 1;
                }
                None => {
                    self.moving.insert(p.clone(), (*stamp, 0));
                }
            }
        }
        settled.sort();
        for (p, stamp) in settled {
            self.moving.remove(&p);
            let change = if self.known.contains_key(&p) { Change::Changed(p.clone()) } else { Change::Created(p.clone()) };
            self.known.insert(p, stamp);
            out.push(change);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(secs: u64) -> Option<SystemTime> {
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
    }

    fn files(list: &[(&str, u64, u64)]) -> Option<HashMap<PathBuf, Stamp>> {
        Some(list.iter().map(|(p, len, t)| (PathBuf::from(p), (*len, at(*t)))).collect())
    }

    #[test]
    fn a_new_file_is_told_once_it_is_whole() {
        let mut w = Watcher::default();
        assert_eq!(w.changes(files(&[("a", 1, 1)])), vec![]);
        // Being written: seen growing, told when it stops.
        assert_eq!(w.changes(files(&[("a", 1, 1), ("b", 10, 2)])), vec![]);
        assert_eq!(w.changes(files(&[("a", 1, 1), ("b", 20, 3)])), vec![]);
        assert_eq!(w.changes(files(&[("a", 1, 1), ("b", 20, 3)])), vec![Change::Created("b".into())]);
        assert_eq!(w.changes(files(&[("a", 1, 1), ("b", 20, 3)])), vec![]);
        // Changed, deleted.
        assert_eq!(w.changes(files(&[("a", 2, 5), ("b", 20, 3)])), vec![]);
        assert_eq!(w.changes(files(&[("a", 2, 5)])), vec![Change::Deleted("b".into()), Change::Changed("a".into())]);
        assert_eq!(w.changes(files(&[("a", 2, 5)])), vec![]);
    }

    #[test]
    fn a_rename_is_one_change_told_at_once() {
        let mut w = Watcher::default();
        w.changes(files(&[("old", 5, 9), ("x", 1, 1)]));
        assert_eq!(w.changes(files(&[("new", 5, 9), ("x", 1, 1)])), vec![Change::Renamed { from: "old".into(), to: "new".into() }]);
        assert_eq!(w.changes(files(&[("new", 5, 9), ("x", 1, 1)])), vec![]);
    }

    #[test]
    fn a_folder_that_cannot_be_read_says_nothing() {
        let mut w = Watcher::default();
        w.changes(files(&[("a", 1, 1)]));
        assert_eq!(w.changes(None), vec![]);
        assert_eq!(w.changes(files(&[("a", 1, 1)])), vec![]);
    }

    #[test]
    fn a_file_that_comes_and_goes_unsettled_is_never_told() {
        let mut w = Watcher::default();
        w.changes(files(&[]));
        assert_eq!(w.changes(files(&[("tmp", 1, 1)])), vec![]);
        assert_eq!(w.changes(files(&[])), vec![]);
        assert_eq!(w.changes(files(&[])), vec![]);
    }

    #[test]
    fn picks_from_the_folder_and_not_twice_running() {
        let dir = std::env::temp_dir().join(format!("pclink-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["a.mp4", "b.mp4", "c.txt", ".hidden.mp4", "sub/d.mp4"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let mut folder = Folder { name: "Clips".into(), path: dir.clone(), subfolders: false };
        let mut p = Picker::default();
        let mut last = PathBuf::new();
        for _ in 0..20 {
            let got = p.pick(&folder, ".mp4").unwrap();
            assert!(got == dir.join("a.mp4") || got == dir.join("b.mp4"), "{got:?}");
            assert_ne!(got, last, "the same file twice running");
            last = got;
        }
        assert_eq!(p.pick(&folder, ".txt").unwrap(), dir.join("c.txt"));
        assert!(p.pick(&folder, ".mkv").unwrap_err().contains(".mkv"));
        folder.subfolders = true;
        assert!((0..40).any(|_| p.pick(&folder, "d.mp4").is_ok_and(|f| f == dir.join("sub").join("d.mp4"))));
        let missing = Folder { name: "Gone".into(), path: dir.join("nope"), subfolders: false };
        assert!(p.pick(&missing, "").unwrap_err().contains("is not there"));
        // Out of time before anything is found: said so, not "empty".
        folder.subfolders = false;
        assert!(p.pick_within(&folder, ".mp4", Duration::ZERO).unwrap_err().contains("too big"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_walk_stops_at_its_count_and_its_time() {
        let dir = std::env::temp_dir().join(format!("pclink-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..10 {
            std::fs::write(dir.join(format!("{i}.mp4")), b"x").unwrap();
        }
        let later = Instant::now() + Duration::from_secs(60);
        let all = walk(&dir, false, 100, later, |_| true);
        assert!(all.whole && all.files.len() == 10);
        let some = walk(&dir, false, 4, later, |_| true);
        assert!(!some.whole && some.files.len() == 4);
        let none = walk(&dir, false, 100, Instant::now(), |_| true);
        assert!(!none.whole && none.files.is_empty());
        // Only what is kept counts toward the limit.
        let kept = walk(&dir, false, 1, later, |p| p.ends_with("3.mp4"));
        assert!(kept.whole && kept.files == vec![dir.join("3.mp4")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn looks_at_one_file_or_a_folder() {
        let dir = std::env::temp_dir().join(format!("pclink-look-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::write(dir.join("clip.MP4"), b"x").unwrap();
        let mut folder = FileWatch { name: "F".into(), path: dir.clone(), ..Default::default() };
        assert_eq!(look(&folder).unwrap().len(), 2);
        folder.matching = ".mp4".into();
        assert_eq!(look(&folder).unwrap().into_keys().collect::<Vec<_>>(), [dir.join("clip.MP4")]);
        let one = FileWatch { name: "F".into(), path: dir.join("b.txt"), ..Default::default() };
        assert_eq!(look(&one).unwrap().len(), 0);
        let gone = FileWatch { name: "F".into(), path: dir.join("no").join("b.txt"), ..Default::default() };
        assert!(look(&gone).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
