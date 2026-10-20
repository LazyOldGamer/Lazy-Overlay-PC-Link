//! Lazy Overlay PC Link: a window by default, `--console` for a terminal.

// No console window behind the PC Link window on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod gui;

use pclink::config::{self, SERVER};
use pclink::{app, service, tray};
use std::path::Path;
use std::sync::Arc;

const HELP: &str = "\
Lazy Overlay PC Link: your overlay's way onto this PC, limited to what you allow.

  lazy-overlay-link                   open the PC Link window
  lazy-overlay-link --console         run in this terminal instead (no window)
  lazy-overlay-link --code <code>     save a connection code from your dashboard, and stop
  lazy-overlay-link --check           say what the settings allow, and stop
  lazy-overlay-link --settings <file> use that settings file
  lazy-overlay-link --minimized       open the window minimized, as when Windows starts it
  lazy-overlay-link --close-to-tray yes|no
                                      keep running by the clock when the window closes, and stop
  lazy-overlay-link --version         say which version this is

Settings: pclink.ini in your settings folder, unless --settings names another.";

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

fn main() {
    pclink::os::harden();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    if flag("--help") || flag("-h") || flag("/?") {
        return println!("{HELP}");
    }
    if flag("--version") {
        return println!(concat!("lazy-overlay-link ", env!("CARGO_PKG_VERSION")));
    }
    // Made full, so the start-with-Windows command names the same file
    // whatever folder it starts in.
    let path = value("--settings")
        .map(|s| std::path::absolute(&s).unwrap_or_else(|e| fail(format!("--settings {s}: {e}"))))
        .or_else(config::settings_path)
        .unwrap_or_else(|| fail("No settings folder (APPDATA or HOME is not set): use --settings <file>."));

    let code = value("--code");
    let close_to_tray = value("--close-to-tray");
    if code.is_some() || close_to_tray.is_some() {
        let mut cfg = app::load(&path).unwrap_or_else(|errors| fail(format!("{} has mistakes: {}", path.display(), errors.join("; "))));
        if let Some(code) = code {
            cfg.key = Some(config::parse_code(&code).unwrap_or_else(|e| fail(e)));
        }
        // From the installer, when ticked there.
        if let Some(on) = close_to_tray {
            cfg.window.close_to_tray = config::yes_no(&on).unwrap_or_else(|| fail("--close-to-tray is yes or no."));
        }
        config::save(&path, &cfg).unwrap_or_else(|e| fail(format!("Could not save {}: {e}", path.display())));
        return println!("Saved to {}.", path.display());
    }
    if flag("--check") {
        return match app::load(&path) {
            Ok(cfg) => {
                println!("Using {}. {}", path.display(), app::summary(&cfg));
                println!(
                    "{}",
                    if cfg.key.is_some() {
                        format!("Connects to {SERVER} (the key is not shown).")
                    } else {
                        "No connection code yet.".into()
                    }
                );
            }
            Err(errors) => println!("{} has mistakes: {}", path.display(), errors.join("; ")),
        };
    }
    let window = !flag("--console") && gui::can_open();
    if !only_copy(&path) {
        // Its window brought forward (from the clock too), or told where to look.
        if window && tray::wake_other() {
            return;
        }
        if window {
            gui::already_open();
        }
        fail("PC Link is already running on this PC.");
    }
    if !window {
        println!(concat!("Lazy Overlay PC Link ", env!("CARGO_PKG_VERSION")));
        println!("Settings: {}", path.display());
        service::run(path, Arc::new(service::print));
    }
    gui::open(path, flag("--minimized"));
}

// One copy per settings file, or two would take the connection in turns. The
// lock goes when the process ends.
fn only_copy(path: &Path) -> bool {
    let lock = path.with_extension("lock");
    if let Some(dir) = lock.parent() {
        let _ = config::make_dir(dir);
    }
    let Ok(file) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock) else {
        return true;
    };
    match file.try_lock() {
        Ok(()) => {
            std::mem::forget(file);
            true
        }
        Err(std::fs::TryLockError::WouldBlock) => false,
        Err(_) => true,
    }
}
