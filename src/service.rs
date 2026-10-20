//! Connect with what pclink.ini holds, serve, and reconnect when it drops,
//! backing off. Runs on its own thread for the window or console; reads the
//! file and never writes it.

use crate::app::{self, App, Emit, Event, REPLACED, REVOKED, Status};
use crate::config::SERVER;
use crate::link::{self, Down, Url};
use crate::{journal, toast};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Runs until the process ends, connected to lazyoverlay.com. Each line is
/// stamped with its time and kept in pclink.log.
pub fn run(path: PathBuf, emit: Emit) -> ! {
    journal::start(&path);
    let emit: Emit = Arc::new(move |e: Event| match e {
        Event::Log(line) => {
            let line = journal::stamp(&line);
            journal::keep(&line);
            emit(Event::Log(line))
        }
        e => emit(e),
    });
    emit(Event::Log(concat!("PC Link ", env!("CARGO_PKG_VERSION"), " started.").into()));
    run_to(&Url::parse(SERVER).expect("SERVER is an address"), path, emit)
}

/// Connection notifications: a drop that outlasts a retry, its return, a
/// dead code, another PC taking over.
#[derive(Default)]
pub struct Drops {
    connected: bool,
    offline: u32,
    told: bool,
}

impl Drops {
    /// The notification a status calls for, if any: title, line, whether an error.
    pub fn see(&mut self, s: &Status) -> Option<(&'static str, &'static str, bool)> {
        match s {
            Status::Connected(_) => {
                let back = std::mem::take(&mut self.told);
                (self.connected, self.offline) = (true, 0);
                back.then_some(("PC Link is connected again", "Your overlay can use this PC again.", false))
            }
            Status::Offline(_) if self.connected => {
                self.offline += 1;
                (self.offline == 2 && !self.told).then(|| {
                    self.told = true;
                    ("PC Link lost its connection", "It keeps trying. Your overlay can't use this PC until it is back.", true)
                })
            }
            Status::Revoked => {
                *self = Drops::default();
                Some(("PC Link's connection code no longer works", "Open PC Link and paste the new one from your dashboard.", true))
            }
            Status::Replaced => {
                *self = Drops::default();
                Some(("PC Link is running on another computer", "This one stopped. Only one can be connected at a time.", true))
            }
            _ => None,
        }
    }
}

/// As `run`, to another server: for the TLS check (examples/connect.rs).
pub fn run_to(url: &Url, path: PathBuf, emit: Emit) -> ! {
    // Told while [toasts] errors is on.
    let notify = Arc::new(AtomicBool::new(false));
    let emit: Emit = {
        let (notify, drops) = (notify.clone(), Mutex::new(Drops::default()));
        Arc::new(move |e: Event| {
            if let Event::Status(s) = &e
                && let Some((title, text, error)) = drops.lock().ok().and_then(|mut d| d.see(s))
                && notify.load(Ordering::Relaxed)
            {
                let _ = toast::show(title, text, error);
            }
            emit(e)
        })
    };
    let status = |s: Status| emit(Event::Status(s));
    let cfg = app::load(&path).unwrap_or_else(|errors| {
        status(Status::BadFile(errors));
        Default::default()
    });
    let mut app = App::new(path, cfg, emit.clone());
    let mut wait = Duration::from_secs(2);
    // Stopped until pclink.ini changes: no key, a refused key, or another copy took over.
    let mut paused = false;
    loop {
        notify.store(app.cfg.toasts.errors, Ordering::Relaxed);
        if paused {
            std::thread::sleep(Duration::from_secs(1));
            if app.refresh() {
                (paused, wait) = (false, Duration::from_secs(2));
            }
            continue;
        }
        let Some(key) = app.cfg.key.clone() else {
            status(Status::NoCode);
            paused = true;
            continue;
        };
        status(Status::Connecting(url.host.clone()));
        match link::connect(url, &key) {
            Ok(mut l) => {
                wait = Duration::from_secs(2);
                match app.session(&mut *l) {
                    Down::Closed(REVOKED, _) => {
                        emit(Event::Log("Your connection code no longer works: it was replaced or revoked on your dashboard.".into()));
                        status(Status::Revoked);
                        paused = true;
                        continue;
                    }
                    Down::Closed(REPLACED, _) => {
                        emit(Event::Log("PC Link connected on another PC, so this one stopped.".into()));
                        status(Status::Replaced);
                        paused = true;
                        continue;
                    }
                    down => {
                        emit(Event::Log(format!("Disconnected: {down}.")));
                        status(Status::Offline(down.to_string()));
                    }
                }
            }
            Err(down) => status(Status::Offline(down.to_string())),
        }
        // Jittered, and cut short when the settings change.
        let jitter = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_millis() as u64).unwrap_or(0);
        let until = Instant::now() + wait + Duration::from_millis(jitter);
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(250));
            if app.refresh() {
                break;
            }
        }
        wait = (wait * 2).min(Duration::from_secs(60));
    }
}

/// An event as a terminal line (`--console`).
pub fn print(e: Event) {
    match e {
        Event::Log(line) => println!("{line}"),
        Event::Status(Status::NoCode) => {
            println!("No connection code yet: run lazy-overlay-link --code <code>, with the code from PC Link on your dashboard.")
        }
        Event::Status(Status::Connecting(host)) => println!("Connecting to {host}..."),
        Event::Status(Status::Offline(why)) => println!("Not connected: {why}. Trying again soon."),
        Event::Status(Status::BadFile(errors)) => println!("The settings file has mistakes: {}", errors.join("; ")),
        Event::Status(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drop_is_told_once_it_outlasts_a_retry_and_again_when_back() {
        let mut d = Drops::default();
        let off = Status::Offline("x".into());
        assert_eq!(d.see(&off), None);
        assert_eq!(d.see(&Status::Connected("me".into())), None);
        assert_eq!(d.see(&off), None);
        assert_eq!(d.see(&Status::Connected("me".into())), None, "a blip");
        assert_eq!(d.see(&off), None);
        assert_eq!(d.see(&Status::Connecting("h".into())), None);
        assert!(d.see(&off).is_some_and(|(_, _, error)| error));
        assert_eq!(d.see(&off), None, "once");
        assert!(d.see(&Status::Connected("me".into())).is_some_and(|(_, _, error)| !error));
        assert!(d.see(&Status::Revoked).is_some());
        assert_eq!(d.see(&off), None);
    }
}
