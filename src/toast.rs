//! Notifications: a title and a line, shown by the system. Windows: from
//! PC Link's icon in the taskbar's corner (tray.rs), as a toast. Linux: notify-send.

/// Shows a notification; `error` marks it as one.
pub fn show(title: &str, text: &str, error: bool) -> Result<(), String> {
    imp::show(title, text, error)
}

#[cfg(not(windows))]
mod imp {
    use std::process::{Command, Stdio};

    pub fn show(title: &str, text: &str, error: bool) -> Result<(), String> {
        let send = crate::os::tool("notify-send").ok_or("notify-send is not installed (libnotify-bin)")?;
        // Its body may be read as markup: shown as written, never a link.
        let text = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let child = Command::new(send)
            .args(["--app-name=Lazy Overlay PC Link", if error { "--urgency=critical" } else { "--urgency=normal" }, "--", title, &text])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not run notify-send: {e}"))?;
        crate::os::reap(child);
        Ok(())
    }
}

// Windows: from the icon by the clock (tray.rs).
#[cfg(windows)]
mod imp {
    pub fn show(title: &str, text: &str, error: bool) -> Result<(), String> {
        crate::tray::note(title, text, error)
    }
}
