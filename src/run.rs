//! Starting and stopping what pclink.ini lists; the server only names an entry.

use crate::config::{Start, Stop};
use crate::procs;
use std::collections::VecDeque;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// At most `per_minute` in any minute.
pub struct Limit {
    per_minute: u32,
    recent: VecDeque<Instant>,
}

impl Limit {
    pub fn new(per_minute: u32) -> Self {
        Limit { per_minute, recent: VecDeque::new() }
    }

    pub fn set(&mut self, per_minute: u32) {
        self.per_minute = per_minute;
    }

    /// Whether one more may go now; counted if so.
    pub fn take(&mut self, now: Instant) -> bool {
        while self.recent.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60)) {
            self.recent.pop_front();
        }
        if self.recent.len() >= self.per_minute as usize {
            return false;
        }
        self.recent.push_back(now);
        true
    }
}

/// Why a start or stop was not done: `told` to the server (no path or
/// process), `why` for the window.
#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    pub told: String,
    pub why: String,
}

impl Refused {
    pub fn new(told: impl Into<String>, why: impl Into<String>) -> Self {
        Refused { told: told.into(), why: why.into() }
    }

    /// The same words for both.
    pub fn same(why: String) -> Self {
        Refused { told: why.clone(), why }
    }
}

/// The entry's program started, or why not.
pub fn start(s: &Start) -> Result<String, Refused> {
    let file = s.path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    if s.once && procs::is_running(&file) == Some(true) {
        return Ok("already running".into());
    }
    if !s.path.is_file() {
        return Err(Refused::new("the program is not on this PC", format!("{} is not there", s.path.display())));
    }
    let dir = s.dir.clone().or_else(|| s.path.parent().map(|p| p.to_path_buf()));
    let mut cmd = Command::new(&s.path);
    cmd.args(&s.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Its own console (or none) and process group: it outlives PC Link.
        const CREATE_NEW_CONSOLE: u32 = 0x10;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | if s.hidden { CREATE_NO_WINDOW } else { CREATE_NEW_CONSOLE });
    }
    let child =
        cmd.spawn().map_err(|e| Refused::new("the program could not start", format!("could not start {}: {e}", s.path.display())))?;
    crate::os::reap(child);
    Ok("started".into())
}

/// The entry's process asked to close (or ended, with force), or why not.
pub fn stop(s: &Stop) -> Result<String, Refused> {
    if procs::is_running(&s.process) == Some(false) {
        return Ok("not running".into());
    }
    stop_running(s)
}

// Windows: WM_CLOSE to its windows, as its X would, or with force ended.
#[cfg(windows)]
fn stop_running(s: &Stop) -> Result<String, Refused> {
    let ids = procs::ids(&s.process).map_err(|e| Refused::new("could not list programs", format!("could not list programs: {e}")))?;
    let done = if s.force { win::end(&ids) } else { win::ask_to_close(&ids) };
    match done {
        0 if s.force => Err(Refused::new("it could not be ended", format!("{} could not be ended", s.process))),
        0 => Err(Refused::new("it has no window to close", format!("{} has no window to close", s.process))),
        _ if s.force => Ok("ended".into()),
        _ => Ok("asked to close".into()),
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    type Hwnd = isize;
    const WM_CLOSE: u32 = 0x0010;
    const PROCESS_TERMINATE: u32 = 0x0001;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(callback: extern "system" fn(Hwnd, isize) -> i32, param: isize) -> i32;
        fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
        fn IsWindowVisible(hwnd: Hwnd) -> i32;
        fn PostMessageW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn TerminateProcess(process: *mut c_void, code: u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    struct Ask<'a> {
        ids: &'a [u32],
        asked: usize,
    }

    extern "system" fn each(hwnd: Hwnd, param: isize) -> i32 {
        // SAFETY: param is the &mut Ask ask_to_close passed, alive for the
        // whole EnumWindows call; pid is a local the call fills in.
        unsafe {
            let ask = &mut *(param as *mut Ask);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if ask.ids.contains(&pid) && IsWindowVisible(hwnd) != 0 && PostMessageW(hwnd, WM_CLOSE, 0, 0) != 0 {
                ask.asked += 1;
            }
        }
        1
    }

    /// How many windows were asked to close.
    pub fn ask_to_close(ids: &[u32]) -> usize {
        let mut ask = Ask { ids, asked: 0 };
        // SAFETY: the callback reads ask only while EnumWindows runs.
        unsafe { EnumWindows(each, &mut ask as *mut Ask as isize) };
        ask.asked
    }

    /// How many processes were ended.
    pub fn end(ids: &[u32]) -> usize {
        let mut ended = 0;
        for &id in ids {
            // SAFETY: a handle checked before use and closed once.
            unsafe {
                let h = OpenProcess(PROCESS_TERMINATE, 0, id);
                if !h.is_null() {
                    if TerminateProcess(h, 1) != 0 {
                        ended += 1;
                    }
                    CloseHandle(h);
                }
            }
        }
        ended
    }
}

// Linux: the processes is_running found, by id (its argument or its comm,
// procs.rs), sent TERM, or with force KILL.
#[cfg(not(windows))]
fn stop_running(s: &Stop) -> Result<String, Refused> {
    let kill = crate::os::tool("kill").ok_or_else(|| Refused::same("kill is not installed".into()))?;
    let ids = procs::ids(&s.process).map_err(|e| Refused::new("could not list programs", format!("could not list programs: {e}")))?;
    if ids.is_empty() {
        return Ok("not running".into());
    }
    let status = Command::new(&kill)
        .arg(if s.force { "-KILL" } else { "-TERM" })
        .arg("--")
        .args(ids.iter().map(u32::to_string))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| Refused::new("it could not be closed", format!("could not close {}: {e}", s.process)))?;
    if status.success() {
        Ok(if s.force { "ended" } else { "asked to close" }.into())
    } else {
        Err(Refused::new("it did not close", format!("{} did not close", s.process)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_holds_a_minute() {
        let mut l = Limit::new(2);
        let t = Instant::now();
        assert!(l.take(t));
        assert!(l.take(t + Duration::from_secs(1)));
        assert!(!l.take(t + Duration::from_secs(59)));
        assert!(l.take(t + Duration::from_secs(60)));
        assert!(!l.take(t + Duration::from_secs(60)));
    }

    #[test]
    fn a_missing_program_says_so() {
        let s = Start {
            name: "x".into(),
            path: std::env::temp_dir().join("no-such-program-pclink"),
            args: vec![],
            dir: None,
            once: false,
            hidden: false,
        };
        let refused = start(&s).unwrap_err();
        assert!(refused.why.contains("is not there"));
        // The server is told what happened, never where the program is.
        assert!(!refused.told.contains("no-such-program-pclink"));
    }

    #[test]
    fn stopping_what_is_not_running_is_no_failure() {
        let s = Stop { name: "x".into(), process: "no-such-process-pclink".into(), force: false };
        assert_eq!(stop(&s), Ok("not running".into()));
    }
}
