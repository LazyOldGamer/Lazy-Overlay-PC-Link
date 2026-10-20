//! One connection: say what this PC offers, do what is asked (only an entry
//! of pclink.ini, by name, so many a minute), and report watched programs
//! and files. Everything is told as an Event to the window or console.

use crate::config::{self, Config};
use crate::files::{self, Picker, Watcher};
use crate::link::{Down, Link};
use crate::procs;
use crate::protocol::{self, Incoming, Kind};
use crate::run::{self, Limit, Refused};
use crate::toast;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Where the connection stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    NoCode,
    /// Connecting to this host.
    Connecting(String),
    /// Connected to this login's overlay.
    Connected(String),
    /// Not connected, and why; it retries.
    Offline(String),
    /// The key was replaced or revoked on the dashboard.
    Revoked,
    /// Another PC Link connected for the channel.
    Replaced,
    BadFile(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A line for the activity list.
    Log(String),
    Status(Status),
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

/// How often watched programs, files and pclink.ini are looked at.
const POLL: Duration = Duration::from_secs(2);
const PING: Duration = Duration::from_secs(30);
/// A server silent this long is gone.
const SILENT: Duration = Duration::from_secs(90);

const TOASTS_PER_MINUTE: u32 = 10;
const FILES_PER_MINUTE: u32 = 60;
/// Lines for refusals and server messages, so a flood cannot bury what was done.
const NOTES_PER_MINUTE: u32 = 30;

/// Close code: the key was revoked or replaced.
pub const REVOKED: u16 = 4401;
/// Close code: another PC Link connected for the channel.
pub const REPLACED: u16 = 4409;

/// The settings; no file yet is the defaults.
pub fn load(path: &Path) -> Result<Config, Vec<String>> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(path).map_err(|e| vec![format!("could not read {}: {e}", path.display())])?;
    config::parse(&text)
}

/// When a file was last written.
pub fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// What a look at pclink.ini found.
#[derive(Debug, PartialEq)]
enum Saved {
    No,
    /// Saved: what the server is told changed.
    Told,
    /// Saved, the server's part the same.
    Quiet,
    /// Saved, only [window] changed.
    WindowOnly,
}

pub struct App {
    path: PathBuf,
    pub cfg: Config,
    stamp: Option<SystemTime>,
    limit: Limit,
    toasts: Limit,
    file_limit: Limit,
    notes: Limit,
    /// Notes left out since the last one shown.
    muted: u32,
    picker: Picker,
    /// By name, path and match: a new match starts afresh, rather than its
    /// files told deleted.
    watchers: HashMap<(String, PathBuf, String), Watcher>,
    /// Watches already said to be unreadable.
    unreadable: HashSet<String>,
    next_id: u64,
    emit: Emit,
}

impl App {
    pub fn new(path: PathBuf, cfg: Config, emit: Emit) -> Self {
        let stamp = modified(&path);
        let limit = Limit::new(cfg.runs_per_minute);
        App {
            path,
            cfg,
            stamp,
            limit,
            toasts: Limit::new(TOASTS_PER_MINUTE),
            file_limit: Limit::new(FILES_PER_MINUTE),
            notes: Limit::new(NOTES_PER_MINUTE),
            muted: 0,
            picker: Picker::default(),
            watchers: HashMap::new(),
            unreadable: HashSet::new(),
            next_id: 0,
            emit,
        }
    }

    fn say(&self, msg: impl Into<String>) {
        (self.emit)(Event::Log(msg.into()));
    }

    // A line the server can cause: past NOTES_PER_MINUTE only counted.
    fn note(&mut self, msg: impl Into<String>) {
        if !self.notes.take(Instant::now()) {
            self.muted += 1;
            return;
        }
        self.say_muted();
        self.say(msg);
    }

    fn say_muted(&mut self) {
        if self.muted > 0 {
            self.say(format!(
                "{} more requests not done or messages from the server not shown (over {NOTES_PER_MINUTE} a minute)",
                self.muted
            ));
            self.muted = 0;
        }
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn running(&self) -> HashSet<String> {
        if self.cfg.watches.is_empty() {
            return HashSet::new();
        }
        procs::running().unwrap_or_default()
    }

    fn watched(&self, all: &HashSet<String>) -> HashMap<String, bool> {
        self.cfg.watches.iter().map(|w| (w.name.clone(), all.contains(&procs::key(&w.process)))).collect()
    }

    fn hello(&mut self, link: &mut dyn Link) -> Result<HashMap<String, bool>, Down> {
        let all = self.running();
        let id = self.id();
        link.send(&protocol::hello(id, &self.cfg, &|p| all.contains(&procs::key(p))))?;
        Ok(self.watched(&all))
    }

    /// Runs one connection until it ends, and says why.
    pub fn session(&mut self, link: &mut dyn Link) -> Down {
        match self.serve(link) {
            Err(down) => down,
            Ok(()) => Down::Failed("ended".into()),
        }
    }

    fn serve(&mut self, link: &mut dyn Link) -> Result<(), Down> {
        let mut seen = self.hello(link)?;
        let (mut heard, mut pinged, mut polled) = (Instant::now(), Instant::now(), Instant::now());
        loop {
            if let Some(frame) = link.recv(Duration::from_millis(250))? {
                heard = Instant::now();
                self.handle(link, &frame)?;
            }
            let now = Instant::now();
            if now.duration_since(polled) >= POLL {
                polled = now;
                if self.muted > 0 && self.notes.take(now) {
                    self.say_muted();
                }
                if self.reload()? == Saved::Told {
                    seen = self.hello(link)?;
                }
                let now_running = self.watched(&self.running());
                for w in self.cfg.watches.clone() {
                    let on = now_running.get(&w.name).copied().unwrap_or(false);
                    if seen.get(&w.name).copied() != Some(on) {
                        self.say(format!("{} {}", w.name, if on { "opened" } else { "closed" }));
                        let id = self.id();
                        link.send(&protocol::watch(id, &w.name, on))?;
                    }
                }
                seen = now_running;
                self.look_at_files(link)?;
            }
            if now.duration_since(pinged) >= PING {
                pinged = now;
                link.send(&protocol::ping())?;
            }
            if now.duration_since(heard) >= SILENT {
                return Err(Down::Failed("the server went quiet".into()));
            }
        }
    }

    // What changed in each [filewatch], told up to FILES_PER_MINUTE.
    fn look_at_files(&mut self, link: &mut dyn Link) -> Result<(), Down> {
        let key = |w: &config::FileWatch| (w.name.clone(), w.path.clone(), w.matching.to_lowercase());
        let keep: HashSet<_> = self.cfg.filewatches.iter().map(key).collect();
        self.watchers.retain(|k, _| keep.contains(k));
        for w in self.cfg.filewatches.clone() {
            let now = files::look(&w);
            if now.is_none() {
                if self.unreadable.insert(w.name.clone()) {
                    self.say(format!(
                        "{}: {} is not there, has more than {} files, or takes too long to look through, so its changes are not told",
                        w.name,
                        w.path.display(),
                        files::MAX_FILES
                    ));
                }
            } else {
                self.unreadable.remove(&w.name);
            }
            let changes = self.watchers.entry(key(&w)).or_default().changes(now);
            // Past the limit, counted in one line a look rather than a line each.
            let mut over = 0;
            for c in changes {
                let file = c.file().display().to_string();
                let old = match &c {
                    files::Change::Renamed { from, .. } => Some(from.display().to_string()),
                    _ => None,
                };
                if !self.file_limit.take(Instant::now()) {
                    over += 1;
                    continue;
                }
                self.say(format!("{}: {} {}", w.name, c.word(), file));
                let id = self.id();
                link.send(&protocol::file(id, &w.name, c.word(), &file, old.as_deref()))?;
            }
            if over > 0 {
                self.say(format!("{}: {over} more changes not told (over {FILES_PER_MINUTE} a minute)", w.name));
            }
        }
        Ok(())
    }

    /// Takes pclink.ini again if it was saved; whether it was, beyond how the
    /// window looks (a save of the same settings counts: "Use PC Link on this
    /// computer").
    pub fn refresh(&mut self) -> bool {
        let before = self.stamp;
        let window_only = matches!(self.reload(), Ok(Saved::WindowOnly));
        self.stamp != before && !window_only
    }

    // pclink.ini saved: taken if it reads. The server is told again, and the
    // line says so, only when what it is told changed; a new key ends the
    // connection.
    fn reload(&mut self) -> Result<Saved, Down> {
        let now = modified(&self.path);
        if now == self.stamp {
            return Ok(Saved::No);
        }
        self.stamp = now;
        match load(&self.path) {
            Ok(cfg) => {
                let offer = |c: &Config| protocol::hello(0, c, &|_| false);
                let (moved, told) = (cfg.key != self.cfg.key, offer(&cfg) != offer(&self.cfg));
                let window_only = cfg.window != self.cfg.window && Config { window: self.cfg.window, ..cfg.clone() } == self.cfg;
                self.limit.set(cfg.runs_per_minute);
                self.cfg = cfg;
                if moved {
                    return Err(Down::Failed("the key changed".into()));
                }
                if told {
                    self.say(format!("Settings saved. {}", summary(&self.cfg)));
                    return Ok(Saved::Told);
                }
                Ok(if window_only { Saved::WindowOnly } else { Saved::Quiet })
            }
            Err(errors) => {
                self.say(format!("{} has mistakes, so the last settings stay: {}", config::FILE_NAME, errors.join("; ")));
                (self.emit)(Event::Status(Status::BadFile(errors)));
                Ok(Saved::Quiet)
            }
        }
    }

    fn handle(&mut self, link: &mut dyn Link, frame: &str) -> Result<(), Down> {
        match protocol::decode(frame) {
            Incoming::Hello { login } => {
                self.note(format!("Connected to {login}'s overlay. {}", summary(&self.cfg)));
                (self.emit)(Event::Status(Status::Connected(login)));
            }
            Incoming::Run { kind, name, id } => {
                let outcome = self.run(kind, &name);
                let what = match kind {
                    Kind::Start => "start",
                    Kind::Stop => "close",
                };
                match &outcome {
                    Ok(s) => self.say(format!("Your overlay asked to {what} {name}: {s}")),
                    Err(e) => self.note(format!("Your overlay asked to {what} {name}, but it was not done: {}", e.why)),
                }
                let told = outcome.map_err(|e| e.told);
                let rid = self.id();
                link.send(&protocol::ran(rid, &id, kind, &name, &told))?;
            }
            Incoming::Pick { name, id, matching } => {
                let outcome = self.pick(&name, &matching);
                match &outcome {
                    Ok(file) => self.say(format!("Your overlay picked a file from {name}: {file}")),
                    Err(e) => self.note(format!("Your overlay asked for a file from {name}, but got none: {e}")),
                }
                let rid = self.id();
                link.send(&protocol::picked(rid, &id, &name, &outcome))?;
            }
            Incoming::Toast { title, text, error } => self.toast(&title, &text, error),
            Incoming::Ack { ok: false, error } => self.note(format!("The server said: {error}")),
            Incoming::Error(e) => self.note(format!("The server said: {e}")),
            Incoming::Ack { .. } | Incoming::Pong | Incoming::Ignored => {}
        }
        Ok(())
    }

    fn pick(&mut self, name: &str, matching: &str) -> Result<String, String> {
        let Some(folder) = self.cfg.folders.iter().find(|f| f.name == name) else {
            return Err(format!("{name} is not a folder in PC Link"));
        };
        if !self.limit.take(Instant::now()) {
            return Err(format!("over {} a minute", self.cfg.runs_per_minute));
        }
        let file = self.picker.pick(folder, matching)?;
        file.to_str().map(str::to_string).ok_or_else(|| "the file's name is not text".to_string())
    }

    fn toast(&mut self, title: &str, text: &str, error: bool) {
        let what = if error { "a problem" } else { "a notification" };
        let allowed = if error { self.cfg.toasts.errors } else { self.cfg.toasts.actions };
        if !allowed {
            self.note(format!("Your overlay sent {what}, not shown (turned off here): {title} {text}"));
            return;
        }
        if !self.toasts.take(Instant::now()) {
            self.note(format!("Your overlay sent {what}, not shown (over {TOASTS_PER_MINUTE} a minute): {title} {text}"));
            return;
        }
        match toast::show(title, text, error) {
            Ok(()) => self.say(format!("Your overlay sent {what}: {title} {text}")),
            Err(e) => self.say(format!("Your overlay sent {what}, but it could not be shown ({e}): {title} {text}")),
        }
    }

    fn run(&mut self, kind: Kind, name: &str) -> Result<String, Refused> {
        let (cfg, limit) = (&self.cfg, &mut self.limit);
        let missing = |verb: &str| Refused::same(format!("{name} is not allowed to {verb} in PC Link"));
        let mut take = || match limit.take(Instant::now()) {
            true => Ok(()),
            false => Err(Refused::same(format!("over {} a minute", cfg.runs_per_minute))),
        };
        match kind {
            Kind::Start => {
                let s = cfg.starts.iter().find(|s| s.name == name).ok_or_else(|| missing("start"))?;
                take()?;
                run::start(s)
            }
            Kind::Stop => {
                let s = cfg.stops.iter().find(|s| s.name == name).ok_or_else(|| missing("close"))?;
                take()?;
                run::stop(s)
            }
        }
    }
}

/// What the file lets the overlay do, in a line.
pub fn summary(cfg: &Config) -> String {
    let list = |names: Vec<&str>| if names.is_empty() { "nothing".to_string() } else { names.join(", ") };
    format!(
        "May start: {}. May stop: {}. Watching: {}. Folders: {}. Watching files: {}.",
        list(cfg.starts.iter().map(|s| s.name.as_str()).collect()),
        list(cfg.stops.iter().map(|s| s.name.as_str()).collect()),
        list(cfg.watches.iter().map(|w| w.name.as_str()).collect()),
        list(cfg.folders.iter().map(|f| f.name.as_str()).collect()),
        list(cfg.filewatches.iter().map(|f| f.name.as_str()).collect())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sink;
    impl Link for Sink {
        fn send(&mut self, _: &str) -> Result<(), Down> {
            Ok(())
        }
        fn recv(&mut self, _: Duration) -> Result<Option<String>, Down> {
            Ok(None)
        }
    }

    #[test]
    fn a_flood_of_refusals_cannot_bury_what_was_done() {
        let lines = Arc::new(std::sync::Mutex::new(0));
        let count = lines.clone();
        let emit: Emit = Arc::new(move |_| *count.lock().unwrap() += 1);
        let mut app = App::new("/nonexistent/pclink.ini".into(), Config::default(), emit);
        for i in 0..500 {
            let frame = format!(r#"{{"t":"event","type":"pclink","data":{{"command":"start","name":"Nope","id":"r-{i}"}}}}"#);
            app.handle(&mut Sink, &frame).unwrap();
            app.handle(&mut Sink, r#"{"t":"error","error":"noise"}"#).unwrap();
        }
        assert_eq!(*lines.lock().unwrap(), NOTES_PER_MINUTE as usize);
        assert_eq!(app.muted, 1000 - NOTES_PER_MINUTE);
    }

    #[test]
    fn a_window_only_save_is_not_told_and_does_not_wake_a_stopped_link() {
        let path = std::env::temp_dir().join(format!("pclink-reload-{}.ini", std::process::id()));
        let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = lines.clone();
        let emit: Emit = Arc::new(move |e| seen.lock().unwrap().push(e));
        let mut cfg = Config::default();
        config::save(&path, &cfg).unwrap();
        let mut app = App::new(path.clone(), cfg.clone(), emit);
        // Each save a second later than the last, so its stamp is new.
        let saves = std::cell::Cell::new(0);
        let save = |cfg: &Config| {
            config::save(&path, cfg).unwrap();
            saves.set(saves.get() + 1);
            let t = SystemTime::now() + Duration::from_secs(saves.get());
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(t).unwrap();
        };
        cfg.window.size = Some((700, 800));
        save(&cfg);
        assert!(!app.refresh(), "only [window]");
        save(&cfg);
        assert!(app.refresh(), "the same again: a nudge");
        cfg.toasts.actions = false;
        save(&cfg);
        assert_eq!(app.reload().unwrap(), Saved::Told);
        assert_eq!(lines.lock().unwrap().iter().filter(|e| matches!(e, Event::Log(l) if l.starts_with("Settings saved"))).count(), 1);
        let _ = std::fs::remove_file(&path);
    }
}
