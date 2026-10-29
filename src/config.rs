//! `pclink.ini`: the only list of what may run, read on this PC alone.
//!
//! ```text
//! [start Spotify]          a program to start, by its full path
//! path = C:\...\Spotify.exe
//! arg = --minimized        one argument a line
//! [stop Spotify]           a program to close, by its process name
//! process = Spotify.exe
//! [watch Game]             a program whose starting and stopping is told
//! process = eldenring.exe
//! [folder Clips]           a folder a random file is picked from
//! path = C:\Clips
//! [filewatch Replays]      a file or folder whose changes are told
//! path = C:\Videos\Replays
//! match = .mp4             only files whose names have it (a folder's)
//! [toasts]                 the notifications the overlay may show
//! actions = yes
//! [window]                 how the window opens, closes and minimizes
//! close_to_tray = yes      closing it keeps PC Link by the clock (Windows)
//! minimize_to_tray = yes   so does minimizing it (Windows)
//! confirm_close = yes      ask before PC Link really ends
//! start_minimized = yes    open minimized (to the clock, when either is ticked)
//! size = 980, 640          as it was last, kept by the window
//! position = 100, 80
//! ```
//!
//! Strict: an unknown section or key, a repeated name or a relative path is
//! an error naming its line, and nothing loads.

use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "pclink.ini";
/// The one server PC Link connects to.
pub const SERVER: &str = "https://lazyoverlay.com";
pub const MAX_NAME: usize = 30;
/// The most entries of a kind.
pub const MAX_ENTRIES: usize = 50;
const MAX_ARGS: usize = 20;
const MAX_VALUE: usize = 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct Start {
    pub name: String,
    pub path: PathBuf,
    pub args: Vec<String>,
    pub dir: Option<PathBuf>,
    /// Not started again while running.
    pub once: bool,
    /// No window (Windows only).
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stop {
    pub name: String,
    pub process: String,
    /// Ended rather than asked to close.
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Watch {
    pub name: String,
    pub process: String,
}

/// A folder a random file is picked from.
#[derive(Debug, Clone, PartialEq)]
pub struct Folder {
    pub name: String,
    pub path: PathBuf,
    pub subfolders: bool,
}

/// A file, or a folder's files, whose changes are told.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FileWatch {
    pub name: String,
    pub path: PathBuf,
    pub subfolders: bool,
    /// A folder's: only files whose names have it, in any case; empty is all.
    pub matching: String,
}

/// The most a filewatch's match may be.
pub const MAX_MATCH: usize = 40;

/// Which notifications may show: from actions, and about failures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Toasts {
    pub actions: bool,
    pub errors: bool,
}

impl Default for Toasts {
    fn default() -> Self {
        Toasts { actions: true, errors: true }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub key: Option<String>,
    pub runs_per_minute: u32,
    pub starts: Vec<Start>,
    pub stops: Vec<Stop>,
    pub watches: Vec<Watch>,
    pub folders: Vec<Folder>,
    pub filewatches: Vec<FileWatch>,
    pub toasts: Toasts,
    pub window: Window,
}

/// How the window opens, closes and minimizes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Window {
    /// Closing it keeps PC Link running, as an icon by the clock.
    pub close_to_tray: bool,
    /// Minimizing it does too.
    pub minimize_to_tray: bool,
    /// Ask before PC Link really ends.
    pub confirm_close: bool,
    /// Open minimized: to the clock when it closes or minimizes there.
    pub start_minimized: bool,
    /// Its inner size and outer position as last left, in logical pixels.
    pub size: Option<(u32, u32)>,
    pub position: Option<(i32, i32)>,
}

// Two numbers, `a, b`, each in its range.
fn pair<T: std::str::FromStr + PartialOrd>(value: &str, range: std::ops::RangeInclusive<T>) -> Option<(T, T)> {
    let (a, b) = value.split_once(',')?;
    let (a, b) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (range.contains(&a) && range.contains(&b)).then_some((a, b))
}

impl Default for Config {
    fn default() -> Self {
        Config {
            key: None,
            runs_per_minute: 20,
            starts: vec![],
            stops: vec![],
            watches: vec![],
            folders: vec![],
            filewatches: vec![],
            toasts: Toasts::default(),
            window: Window::default(),
        }
    }
}

/// An entry, by its name.
trait Named {
    fn name(&self) -> &str;
}

macro_rules! named {
    ($($t:ty),*) => {$(impl Named for $t { fn name(&self) -> &str { &self.name } })*};
}
named!(Start, Stop, Watch, Folder, FileWatch);

/// How the server names an entry in its triggers: lowercase, spaces as `_`.
/// Two entries of a kind may not share one.
pub fn slug(name: &str) -> String {
    name.to_ascii_lowercase().replace(' ', "_")
}

/// 1 to 30 letters, digits, spaces, `_` or `-`, no space at either end.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name == name.trim()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '_' || c == '-')
}

fn valid_key(key: &str) -> bool {
    (20..=100).contains(&key.len()) && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A process's file name, never a path: `obs64.exe`.
pub fn valid_process(p: &str) -> bool {
    !p.is_empty() && p.len() <= 100 && p != "." && p != ".." && !p.chars().any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
}

/// A value the file reads back the same: no control character, no space at either end.
fn valid_value(v: &str) -> bool {
    v.len() <= MAX_VALUE && v == v.trim() && !v.chars().any(char::is_control)
}

/// A full path the file can hold.
pub fn valid_path(p: &Path) -> bool {
    p.is_absolute() && p.to_str().is_some_and(valid_value)
}

/// yes or no, in any case.
pub fn yes_no(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" | "1" => Some(true),
        "no" | "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

enum Section {
    None,
    Link,
    Toasts,
    Window,
    Start(Start, usize),
    Stop(Stop, usize),
    Watch(Watch, usize),
    Folder(Folder, usize),
    FileWatch(FileWatch, usize),
}

/// The settings, or every error found.
pub fn parse(text: &str) -> Result<Config, Vec<String>> {
    let mut cfg = Config::default();
    let mut errors = Vec::new();
    let mut section = Section::None;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(head) = line.strip_prefix('[') {
            finish(&mut cfg, std::mem::replace(&mut section, Section::None), &mut errors);
            let Some(head) = head.strip_suffix(']') else {
                errors.push(format!("line {n}: a section is written [kind name]"));
                continue;
            };
            let head = head.trim();
            let (kind, name) = head.split_once(' ').map(|(k, v)| (k, v.trim())).unwrap_or((head, ""));
            section = match (kind, name.is_empty()) {
                ("link", true) => Section::Link,
                ("toasts", true) => Section::Toasts,
                ("window", true) => Section::Window,
                ("start" | "stop" | "watch" | "folder" | "filewatch", _) if !valid_name(name) => {
                    errors.push(format!("line {n}: a name is 1 to {MAX_NAME} letters, digits, spaces, _ or -"));
                    continue;
                }
                ("start", _) => Section::Start(
                    Start { name: name.into(), path: PathBuf::new(), args: vec![], dir: None, once: false, hidden: false },
                    n,
                ),
                ("stop", _) => Section::Stop(Stop { name: name.into(), process: String::new(), force: false }, n),
                ("watch", _) => Section::Watch(Watch { name: name.into(), process: String::new() }, n),
                ("folder", _) => Section::Folder(Folder { name: name.into(), path: PathBuf::new(), subfolders: false }, n),
                ("filewatch", _) => Section::FileWatch(FileWatch { name: name.into(), ..Default::default() }, n),
                _ => {
                    let kind = crate::protocol::plain(kind, 30);
                    errors.push(format!(
                        "line {n}: [{kind}] is not a section here: use [start name], [stop name], [watch name], [folder name], \
                         [filewatch name], [toasts], [window] or [link]"
                    ));
                    continue;
                }
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            errors.push(format!("line {n}: expected key = value"));
            continue;
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        // As the errors name it: shown in the window and printed to a terminal.
        let shown = crate::protocol::plain(&key, 30);
        if value.len() > MAX_VALUE || value.chars().any(char::is_control) {
            errors.push(format!("line {n}: the value is too long or has a control character"));
            continue;
        }
        let bad = |what: &str| format!("line {n}: {shown} {what}");
        let flag = |target: &mut bool, errors: &mut Vec<String>| match yes_no(value) {
            Some(v) => *target = v,
            None => errors.push(bad("is yes or no")),
        };
        let full = |errors: &mut Vec<String>, what: &str| {
            let ok = Path::new(value).is_absolute();
            if !ok {
                errors.push(bad(what));
            }
            ok.then(|| PathBuf::from(value))
        };
        let process = |target: &mut String, errors: &mut Vec<String>| {
            if valid_process(value) {
                *target = value.to_string();
            } else {
                errors.push(bad("is a process's file name, like obs64.exe, not a path"));
            }
        };
        match &mut section {
            Section::None => errors.push(format!("line {n}: {key} is outside any section")),
            Section::Link => match key.as_str() {
                "key" if value.is_empty() => cfg.key = None,
                "key" if valid_key(value) => cfg.key = Some(value.to_string()),
                "key" => errors.push(bad("is not a PC Link key: copy it again from the dashboard")),
                "runs_per_minute" => match value.parse::<u32>() {
                    Ok(v) if (1..=120).contains(&v) => cfg.runs_per_minute = v,
                    _ => errors.push(bad("is a number from 1 to 120")),
                },
                _ => errors.push(bad("is not a setting of [link]: key or runs_per_minute")),
            },
            Section::Toasts => match key.as_str() {
                "actions" => flag(&mut cfg.toasts.actions, &mut errors),
                "errors" => flag(&mut cfg.toasts.errors, &mut errors),
                _ => errors.push(bad("is not a setting of [toasts]: actions or errors")),
            },
            Section::Window => match key.as_str() {
                "close_to_tray" => flag(&mut cfg.window.close_to_tray, &mut errors),
                "minimize_to_tray" => flag(&mut cfg.window.minimize_to_tray, &mut errors),
                "confirm_close" => flag(&mut cfg.window.confirm_close, &mut errors),
                "start_minimized" => flag(&mut cfg.window.start_minimized, &mut errors),
                "size" => match pair(value, 200..=20000) {
                    Some(v) => cfg.window.size = Some(v),
                    None => errors.push(bad("is a width and height, like 980, 640")),
                },
                "position" => match pair(value, -50000..=50000) {
                    Some(v) => cfg.window.position = Some(v),
                    None => errors.push(bad("is a left and top, like 100, 80")),
                },
                _ => errors.push(bad(
                    "is not a setting of [window]: close_to_tray, minimize_to_tray, confirm_close, start_minimized, size or position",
                )),
            },
            Section::Folder(Folder { path, subfolders, .. }, _) => match key.as_str() {
                "path" => *path = full(&mut errors, "must be a full path").unwrap_or_default(),
                "subfolders" => flag(subfolders, &mut errors),
                _ => errors.push(bad("is not a setting of [folder]: path or subfolders")),
            },
            Section::FileWatch(f, _) => match key.as_str() {
                "path" => f.path = full(&mut errors, "must be a full path").unwrap_or_default(),
                "subfolders" => flag(&mut f.subfolders, &mut errors),
                "match" if value.chars().count() <= MAX_MATCH => f.matching = value.to_string(),
                "match" => errors.push(bad(&format!("is at most {MAX_MATCH} characters"))),
                _ => errors.push(bad("is not a setting of [filewatch]: path, subfolders or match")),
            },
            Section::Start(s, _) => match key.as_str() {
                "path" => s.path = full(&mut errors, "must be the program's full path").unwrap_or_default(),
                "arg" if s.args.len() < MAX_ARGS => s.args.push(value.to_string()),
                "arg" => errors.push(format!("line {n}: at most {MAX_ARGS} arguments")),
                "dir" => s.dir = full(&mut errors, "must be a full path"),
                "once" => flag(&mut s.once, &mut errors),
                "hidden" => flag(&mut s.hidden, &mut errors),
                _ => errors.push(bad("is not a setting of [start]: path, arg, dir, once or hidden")),
            },
            Section::Stop(s, _) => match key.as_str() {
                "process" => process(&mut s.process, &mut errors),
                "force" => flag(&mut s.force, &mut errors),
                _ => errors.push(bad("is not a setting of [stop]: process or force")),
            },
            Section::Watch(w, _) => match key.as_str() {
                "process" => process(&mut w.process, &mut errors),
                _ => errors.push(bad("is not a setting of [watch]: process")),
            },
        }
    }
    finish(&mut cfg, section, &mut errors);
    if errors.is_empty() { Ok(cfg) } else { Err(errors) }
}

// A section whose lines are read: added, unless incomplete or a second of its name.
fn finish(cfg: &mut Config, section: Section, errors: &mut Vec<String>) {
    let missing = |empty: bool, what: &'static str| empty.then_some(what);
    match section {
        Section::None | Section::Link | Section::Toasts | Section::Window => {}
        Section::Start(s, n) => {
            let m = missing(s.path.as_os_str().is_empty(), "path");
            add(&mut cfg.starts, s, n, "start", m, errors)
        }
        Section::Stop(s, n) => {
            let m = missing(s.process.is_empty(), "process");
            add(&mut cfg.stops, s, n, "stop", m, errors)
        }
        Section::Watch(w, n) => {
            let m = missing(w.process.is_empty(), "process");
            add(&mut cfg.watches, w, n, "watch", m, errors)
        }
        Section::Folder(f, n) => {
            let m = missing(f.path.as_os_str().is_empty(), "path");
            add(&mut cfg.folders, f, n, "folder", m, errors)
        }
        Section::FileWatch(f, n) => {
            let m = missing(f.path.as_os_str().is_empty(), "path");
            add(&mut cfg.filewatches, f, n, "filewatch", m, errors)
        }
    }
}

fn add<T: Named>(list: &mut Vec<T>, item: T, n: usize, kind: &str, missing: Option<&str>, errors: &mut Vec<String>) {
    let name = item.name().to_string();
    let error = if let Some(what) = missing {
        format!("[{kind} {name}] has no {what}")
    } else if list.iter().any(|x| slug(x.name()) == slug(&name)) {
        format!("a second [{kind} {name}]")
    } else if list.len() >= MAX_ENTRIES {
        format!("at most {MAX_ENTRIES} [{kind}] entries")
    } else {
        list.push(item);
        return;
    };
    errors.push(format!("line {n}: {error}"));
}

/// The user's own settings folder: `%APPDATA%\Lazy Overlay\PC Link` on
/// Windows, `~/.config/lazy-overlay-link` elsewhere. Never beside the
/// program, where a downloaded pclink.ini could land.
pub fn settings_path() -> Option<PathBuf> {
    let full = |var: &str| std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute());
    let dir = if cfg!(windows) {
        full("APPDATA")?.join("Lazy Overlay").join("PC Link")
    } else {
        full("XDG_CONFIG_HOME").or_else(|| full("HOME").map(|h| h.join(".config")))?.join("lazy-overlay-link")
    };
    Some(dir.join(FILE_NAME))
}

/// The settings folder, made when missing: this user's alone on Linux.
pub fn make_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

/// Saved whole or not at all (a temporary file renamed over it); this user's
/// alone on Linux, since it holds the key.
pub fn save(path: &Path, cfg: &Config) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        make_dir(dir)?;
    }
    let tmp = path.with_extension("ini.tmp");
    // A leftover from a failed save is never reused.
    let _ = std::fs::remove_file(&tmp);
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600);
    }
    let mut file = open.open(&tmp)?;
    file.write_all(cfg.to_ini().as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

// --- the connection code --------------------------------------------------------------

/// The dashboard's connection code: `pclink1:` and base64url of the server's
/// address, a line break and the key.
pub const CODE_PREFIX: &str = "pclink1:";

/// The key in a connection code, or why it is not one PC Link can use.
pub fn parse_code(code: &str) -> Result<String, String> {
    const NOT: &str = "That is not a PC Link connection code. Copy it again from your dashboard.";
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64url(code.strip_prefix(CODE_PREFIX).ok_or(NOT)?).ok_or(NOT)?;
    let text = String::from_utf8(bytes).map_err(|_| NOT)?;
    let (server, key) = text.split_once('\n').ok_or(NOT)?;
    if !valid_key(key) {
        return Err(NOT.into());
    }
    if server.trim_end_matches('/') != SERVER {
        let from = crate::protocol::plain(server, 100);
        return Err(format!("That code is from {from}. PC Link connects only to {SERVER}."));
    }
    Ok(key.to_string())
}

fn base64url(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

// --- programs, as the window shows them ---------------------------------------------------

/// One program in the window. In the file, its [start], [stop] and [watch]
/// entries, sharing the name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub name: String,
    /// The program itself, to start it.
    pub path: Option<PathBuf>,
    /// Its process's file name, to close or watch it.
    pub process: String,
    pub start: bool,
    pub stop: bool,
    pub watch: bool,
    // Beside the list in the window, below its ticks.
    pub args: Vec<String>,
    pub dir: Option<PathBuf>,
    pub once: bool,
    pub hidden: bool,
    pub force: bool,
}

impl Program {
    /// It as a [start] entry, when it can be one (ticked or not).
    pub fn as_start(&self) -> Option<Start> {
        let args: Vec<String> = self.args.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
        let fits = args.len() <= MAX_ARGS && args.iter().all(|a| valid_value(a)) && self.dir.as_deref().is_none_or(valid_path);
        let path = self.path.as_ref().filter(|path| fits && valid_path(path))?;
        Some(Start { name: self.name.clone(), path: path.clone(), args, dir: self.dir.clone(), once: self.once, hidden: self.hidden })
    }

    /// It as a [stop] entry, when it can be one (ticked or not).
    pub fn as_stop(&self) -> Option<Stop> {
        valid_process(&self.process).then(|| Stop { name: self.name.clone(), process: self.process.clone(), force: self.force })
    }

    /// A program picked on this PC, named after its file: started (once), and
    /// watched unless another entry watches it already.
    pub fn from_path(path: &Path, taken: &[Program]) -> Program {
        let file = |p: Option<&std::ffi::OsStr>| p.map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let names: Vec<&str> = taken.iter().map(|p| p.name.as_str()).collect();
        let process = file(path.file_name());
        Program {
            name: free_name(&file(path.file_stem()), &names),
            path: Some(path.to_path_buf()),
            watch: !taken.iter().any(|p| p.watch && crate::procs::key(&p.process) == crate::procs::key(&process)),
            process,
            start: true,
            once: true,
            ..Program::default()
        }
    }

    /// Another entry for the same program, to start it another way (other
    /// arguments): its own name, neither closing nor watching it, as this one may.
    /// A copy of a copy is numbered as the first's: `Spotify 3`, not `Spotify 2 2`.
    pub fn another(&self, taken: &[Program]) -> Program {
        let names: Vec<&str> = taken.iter().map(|p| p.name.as_str()).collect();
        let base = match self.name.rsplit_once(' ') {
            Some((base, n)) if !base.trim().is_empty() && n.parse::<u32>().is_ok() => base,
            _ => &self.name,
        };
        Program { name: free_name(base, &names), stop: false, watch: false, ..self.clone() }
    }
}

/// Whether the `i`th of these names is saved: valid, and the first of its slug.
pub fn kept_name(names: &[&str], i: usize) -> bool {
    valid_name(names[i]) && !names[..i].iter().any(|n| slug(n) == slug(names[i]))
}

/// `wanted` made into a valid name that none of `taken` has.
pub fn free_name(wanted: &str, taken: &[&str]) -> String {
    let base: String = wanted.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { ' ' }).collect();
    let mut base = base.split_whitespace().collect::<Vec<_>>().join(" ");
    base.truncate(MAX_NAME - 3);
    let base = match base.trim() {
        "" => "Program".to_string(),
        b => b.to_string(),
    };
    let free = |n: &str| !taken.iter().any(|t| slug(t) == slug(n));
    if free(&base) {
        return base;
    }
    (2..100).map(|i| format!("{base} {i}")).find(|n| free(n)).unwrap_or(base)
}

// The ones kept: a valid name and path, the first of a name, at most MAX_ENTRIES.
fn kept<T: Named + Clone>(items: &[T], path: impl Fn(&T) -> &Path) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for item in items.iter().take(MAX_ENTRIES) {
        if valid_name(item.name()) && valid_path(path(item)) && !out.iter().any(|x| slug(x.name()) == slug(item.name())) {
            out.push(item.clone());
        }
    }
    out
}

impl Config {
    /// The entries as programs, one a name, by name.
    pub fn programs(&self) -> Vec<Program> {
        let mut out: Vec<Program> = Vec::new();
        fn at<'a>(out: &'a mut Vec<Program>, name: &str) -> &'a mut Program {
            let i = match out.iter().position(|p| slug(&p.name) == slug(name)) {
                Some(i) => i,
                None => {
                    out.push(Program { name: name.to_string(), ..Program::default() });
                    out.len() - 1
                }
            };
            &mut out[i]
        }
        for s in &self.starts {
            let p = at(&mut out, &s.name);
            p.start = true;
            p.path = Some(s.path.clone());
            (p.args, p.dir, p.once, p.hidden) = (s.args.clone(), s.dir.clone(), s.once, s.hidden);
            if p.process.is_empty() {
                p.process = s.path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            }
        }
        for s in &self.stops {
            let p = at(&mut out, &s.name);
            (p.stop, p.force, p.process) = (true, s.force, s.process.clone());
        }
        for w in &self.watches {
            let p = at(&mut out, &w.name);
            (p.watch, p.process) = (true, w.process.clone());
        }
        out.sort_by_cached_key(|p| p.name.to_lowercase());
        out
    }

    /// The entries made from these programs, less what the file would refuse.
    pub fn set_programs(&mut self, programs: &[Program]) {
        (self.starts, self.stops, self.watches) = (vec![], vec![], vec![]);
        let mut seen: Vec<String> = Vec::new();
        for p in programs.iter().take(MAX_ENTRIES) {
            if !valid_name(&p.name) || seen.contains(&slug(&p.name)) {
                continue;
            }
            seen.push(slug(&p.name));
            if let Some(s) = p.as_start().filter(|_| p.start) {
                self.starts.push(s);
            }
            if let Some(s) = p.as_stop().filter(|_| p.stop) {
                self.stops.push(s);
            }
            if p.watch && valid_process(&p.process) {
                self.watches.push(Watch { name: p.name.clone(), process: p.process.clone() });
            }
        }
    }

    pub fn set_folders(&mut self, folders: &[Folder]) {
        self.folders = kept(folders, |f| &f.path);
    }

    pub fn set_filewatches(&mut self, watches: &[FileWatch]) {
        let tidy: Vec<FileWatch> = watches
            .iter()
            .map(|w| FileWatch { matching: w.matching.trim().chars().take(MAX_MATCH).collect(), ..w.clone() })
            .filter(|w| valid_value(&w.matching))
            .collect();
        self.filewatches = kept(&tidy, |f| &f.path);
    }

    /// The file, as `parse` reads it back.
    pub fn to_ini(&self) -> String {
        let mut o = String::from(
            "# Lazy Overlay PC Link: what your overlay may do on this PC.\n\
             # Kept by the PC Link window; you may edit it by hand too.\n\n[link]\n",
        );
        if let Some(k) = &self.key {
            o += &format!("key = {k}\n");
        }
        o += &format!("runs_per_minute = {}\n", self.runs_per_minute);
        let yes = |b: bool| if b { "yes" } else { "no" };
        o += &format!("\n[toasts]\nactions = {}\nerrors = {}\n", yes(self.toasts.actions), yes(self.toasts.errors));
        let w = &self.window;
        o += &format!(
            "\n[window]\nclose_to_tray = {}\nminimize_to_tray = {}\nconfirm_close = {}\nstart_minimized = {}\n",
            yes(w.close_to_tray),
            yes(w.minimize_to_tray),
            yes(w.confirm_close),
            yes(w.start_minimized)
        );
        if let Some((width, height)) = w.size {
            o += &format!("size = {width}, {height}\n");
        }
        if let Some((left, top)) = w.position {
            o += &format!("position = {left}, {top}\n");
        }
        for s in &self.starts {
            o += &format!("\n[start {}]\npath = {}\n", s.name, s.path.display());
            for a in &s.args {
                o += &format!("arg = {a}\n");
            }
            if let Some(d) = &s.dir {
                o += &format!("dir = {}\n", d.display());
            }
            o += &format!("once = {}\nhidden = {}\n", yes(s.once), yes(s.hidden));
        }
        for s in &self.stops {
            o += &format!("\n[stop {}]\nprocess = {}\nforce = {}\n", s.name, s.process, yes(s.force));
        }
        for w in &self.watches {
            o += &format!("\n[watch {}]\nprocess = {}\n", w.name, w.process);
        }
        for f in &self.folders {
            o += &format!("\n[folder {}]\npath = {}\nsubfolders = {}\n", f.name, f.path.display(), yes(f.subfolders));
        }
        for f in &self.filewatches {
            o += &format!("\n[filewatch {}]\npath = {}\nsubfolders = {}\n", f.name, f.path.display(), yes(f.subfolders));
            if !f.matching.is_empty() {
                o += &format!("match = {}\n", f.matching);
            }
        }
        if cfg!(windows) { o.replace('\n', "\r\n") } else { o }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    const PROGRAM: &str = r"C:\Program Files\App\app.exe";
    #[cfg(not(windows))]
    const PROGRAM: &str = "/usr/bin/app";

    #[test]
    fn reads_every_kind_of_entry() {
        let text = format!(
            "\u{feff}# a note\r\n[link]\r\nkey = {}\r\nruns_per_minute = 5\r\n\r\n\
             [start My App]\npath = {PROGRAM}\narg = --one\narg = two words\nonce = yes\nhidden = no\n\
             [stop My App]\nprocess = app.exe\nforce = yes\n; another note\n[watch Game]\nprocess = game.exe\n",
            "k".repeat(43)
        );
        let cfg = parse(&text).unwrap();
        assert_eq!(cfg.key.as_deref(), Some("k".repeat(43).as_str()));
        assert_eq!(cfg.runs_per_minute, 5);
        assert_eq!(
            cfg.starts,
            vec![Start {
                name: "My App".into(),
                path: PROGRAM.into(),
                args: vec!["--one".into(), "two words".into()],
                dir: None,
                once: true,
                hidden: false
            }]
        );
        assert_eq!(cfg.stops, vec![Stop { name: "My App".into(), process: "app.exe".into(), force: true }]);
        assert_eq!(cfg.watches, vec![Watch { name: "Game".into(), process: "game.exe".into() }]);
    }

    #[test]
    fn what_it_writes_reads_back() {
        let mut cfg = Config { key: Some("k".repeat(43)), ..Config::default() };
        let mut obs = Program::from_path(Path::new(PROGRAM), &[]);
        obs.stop = true;
        let mut game = Program::from_path(Path::new(PROGRAM), &[obs.clone()]);
        (game.start, game.watch) = (false, true);
        game.args = vec!["--one".into()];
        cfg.set_programs(&[obs.clone(), game.clone()]);
        let back = parse(&cfg.to_ini()).unwrap();
        assert_eq!(back, cfg);
        assert!(!cfg.to_ini().contains("server"));
        let programs = back.programs();
        assert_eq!(programs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["app", "app 2"]);
        assert_eq!((programs[0].start, programs[0].stop, programs[0].watch), (true, true, true));
        assert_eq!((programs[1].start, programs[1].stop, programs[1].watch, programs[1].name.as_str()), (false, false, true, "app 2"));
        assert_eq!(programs[1].process, "app".to_string() + if cfg!(windows) { ".exe" } else { "" });
    }

    #[test]
    fn reads_and_writes_folders_file_watches_toasts_and_the_tray() {
        let dir = if cfg!(windows) { r"C:\Clips" } else { "/home/me/clips" };
        let text = format!(
            "[toasts]\nerrors = no\n[window]\nclose_to_tray = yes\nconfirm_close = yes\nstart_minimized = yes\nsize = 700, 800\nposition = -1200, 40\n[folder Clips]\npath = {dir}\nsubfolders = yes\n[filewatch Replays]\npath = {dir}\nmatch = .MP4\n"
        );
        let cfg = parse(&text).unwrap();
        assert_eq!(cfg.toasts, Toasts { actions: true, errors: false });
        let window = Window {
            close_to_tray: true,
            confirm_close: true,
            start_minimized: true,
            size: Some((700, 800)),
            position: Some((-1200, 40)),
            ..Default::default()
        };
        assert_eq!((cfg.window, Config::default().window), (window, Window::default()));
        assert_eq!(cfg.folders, vec![Folder { name: "Clips".into(), path: dir.into(), subfolders: true }]);
        assert_eq!(
            cfg.filewatches,
            vec![FileWatch { name: "Replays".into(), path: dir.into(), subfolders: false, matching: ".MP4".into() }]
        );
        assert_eq!(parse(&cfg.to_ini()).unwrap(), cfg);

        let mut tidy = Config::default();
        let bad = Folder { name: "a/b".into(), path: dir.into(), subfolders: false };
        let relative = Folder { name: "Rel".into(), path: "clips".into(), subfolders: false };
        let twice = Folder { name: "clips".into(), path: dir.into(), subfolders: false };
        tidy.set_folders(&[cfg.folders[0].clone(), bad, relative, twice]);
        assert_eq!(tidy.folders, cfg.folders);

        for (text, want) in [
            ("[folder X]\n", "has no path"),
            ("[folder X]\npath = clips\n", "line 2: path must be a full path"),
            ("[filewatch X]\nprocess = x.exe\n", "line 2: process is not a setting of [filewatch]"),
            ("[folder X]\nmatch = .mp4\n", "line 2: match is not a setting of [folder]"),
            ("[toasts]\nall = yes\n", "line 2: all is not a setting of [toasts]"),
            ("[toasts x]\n", "line 1: [toasts]"),
            ("[window]\nclose_to_tray = maybe\n", "line 2: close_to_tray is yes or no"),
            ("[window]\nstart_minimized = sometimes\n", "line 2: start_minimized is yes or no"),
            ("[window]\nsize = 640\n", "line 2: size is a width and height"),
            ("[window]\nsize = 10, 760\n", "line 2: size is a width and height"),
            ("[window]\nposition = a, 1\n", "line 2: position is a left and top"),
            ("[window]\nwidth = 2\n", "line 2: width is not a setting of [window]"),
        ] {
            let errors = parse(text).expect_err(text);
            assert!(errors.iter().any(|e| e.contains(want)), "{text:?} gave {errors:?}");
        }
    }

    #[test]
    fn one_program_can_be_listed_twice_with_other_arguments() {
        let first = Program::from_path(Path::new(PROGRAM), &[]);
        let mut second = first.another(std::slice::from_ref(&first));
        assert_eq!((second.name.as_str(), second.path.as_ref(), second.start, second.watch), ("app 2", first.path.as_ref(), true, false));
        second.args = vec!["--safe".into()];
        let third = Program::from_path(Path::new(PROGRAM), &[first.clone(), second.clone()]);
        assert!(!third.watch, "watched once already");
        let mut cfg = Config::default();
        cfg.set_programs(&[first, second, third]);
        let back = parse(&cfg.to_ini()).unwrap();
        assert_eq!(back.starts.len(), 3);
        assert_eq!(back.starts[1].args, vec!["--safe".to_string()]);
        assert_eq!(back.watches.len(), 1);
        let fourth = back.programs()[1].another(&back.programs());
        assert_eq!(fourth.name, "app 4");
    }

    #[test]
    fn programs_are_listed_by_name() {
        let text =
            format!("[watch zed]\nprocess = z\n[start Beta]\npath = {PROGRAM}\n[stop alpha]\nprocess = a\n[watch Beta]\nprocess = b\n");
        let names: Vec<String> = parse(&text).unwrap().programs().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["alpha", "Beta", "zed"]);
    }

    #[test]
    fn a_name_is_kept_when_it_fits_and_comes_first() {
        let names = ["Spotify", "spotify", "a/b", "Spotify 2"];
        let kept: Vec<bool> = (0..names.len()).map(|i| kept_name(&names, i)).collect();
        assert_eq!(kept, [true, false, false, true]);
    }

    #[test]
    fn names_are_made_to_fit() {
        assert_eq!(free_name("Spotify", &[]), "Spotify");
        assert_eq!(free_name("obs64.exe (1)", &[]), "obs64 exe 1");
        assert_eq!(free_name("...", &[]), "Program");
        assert_eq!(free_name("spotify", &["Spotify"]), "spotify 2");
        assert!(valid_name(&free_name(&"x".repeat(80), &[])));
    }

    #[test]
    fn a_path_the_file_cannot_hold_is_not_written() {
        let mut cfg = Config::default();
        let evil = Program::from_path(Path::new(&format!("{PROGRAM}\n[start Evil]\npath = {PROGRAM}")), &[]);
        let spaced = Program::from_path(Path::new(&format!("{PROGRAM} ")), std::slice::from_ref(&evil));
        cfg.set_programs(&[evil, spaced]);
        assert!(cfg.starts.is_empty());
        assert!(parse(&cfg.to_ini()).is_ok());
    }

    fn code(server: &str, key: &str) -> String {
        let b64 = crate::link::hash::base64(format!("{server}\n{key}").as_bytes());
        format!("{CODE_PREFIX}{}", b64.replace('+', "-").replace('/', "_").trim_end_matches('='))
    }

    #[test]
    fn a_connection_code_gives_the_key_for_lazyoverlay_com_only() {
        let key = "abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE";
        assert_eq!(parse_code(&format!("  {}\n", code(SERVER, key))), Ok(key.into()));
        assert_eq!(parse_code(&code("https://lazyoverlay.com/", key)), Ok(key.into()));
        for bad in ["", "pclink1:", "pclink1:!!!", SERVER, &code(SERVER, "short")] {
            assert!(parse_code(bad).is_err(), "{bad}");
        }
        for other in ["https://overlay.example.com", "http://lazyoverlay.com", "https://lazyoverlay.com.evil.example"] {
            let why = parse_code(&code(other, key)).unwrap_err();
            assert!(why.contains("connects only to https://lazyoverlay.com"), "{other}: {why}");
        }
    }

    #[test]
    fn refuses_anything_it_does_not_know_naming_the_line() {
        let cases = [
            ("[run X]\n", "line 1: [run]"),
            ("[start]\n", "line 1: a name"),
            ("[start a/b]\n", "line 1: a name"),
            (&format!("[start {}]\n", "x".repeat(31)), "line 1: a name"),
            ("path = /x\n", "outside any section"),
            ("[start X]\npath = app.exe\n", "line 2: path must be the program's full path"),
            ("[start X]\n", "has no path"),
            ("[stop X]\nprocess = C:\\app.exe\n", "line 2: process is a process's file name"),
            ("[watch X]\nprocess = ../x\n", "line 2: process"),
            ("[watch X]\nprocess = x.exe\nforce = yes\n", "line 3: force is not a setting of [watch]"),
            ("[link]\nkey = short\n", "line 2: key is not a PC Link key"),
            ("[link]\nserver = https://overlay.example.com\n", "line 2: server is not a setting of [link]"),
            ("[link]\nruns_per_minute = 500\n", "line 2: runs_per_minute"),
            ("[link]\nupdate = https://evil\n", "line 2: update is not a setting"),
            ("[watch A b]\nprocess = a\n[watch a_B]\nprocess = b\n", "line 3: a second [watch a_B]"),
            ("[stop X]\nprocess = x\nforce = maybe\n", "line 3: force is yes or no"),
            ("[link\n", "line 1: a section"),
            ("just words\n", "line 1: expected key = value"),
            ("[link]\nk\u{1b}[2Jey = 1\n", "line 2: k[2jey is not a setting"),
            ("[st\u{1b}art x]\n", "line 1: [start] is not a section"),
        ];
        for (text, want) in cases {
            let errors = parse(text).expect_err(text);
            assert!(errors.iter().any(|e| e.contains(want)), "{text:?} gave {errors:?}");
        }
    }

    #[test]
    fn names_become_the_slugs_the_server_keys_triggers_by() {
        assert_eq!(slug("My Game-2"), "my_game-2");
        assert!(valid_name("Spotify") && !valid_name(" Spotify") && !valid_name("a.b"));
    }
}
