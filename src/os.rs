//! Small system helpers shared by the modules.

use std::sync::atomic::{AtomicBool, Ordering};

/// Windows: DLLs only from the system's own folder, never from beside the
/// program (a Downloads folder) or the current one, so a DLL planted there
/// is not loaded. The ones it imports are kept there by its build (build.rs).
pub fn harden() {
    #[cfg(windows)]
    {
        const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x800;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetDefaultDllDirectories(flags: u32) -> i32;
            fn SetDllDirectoryW(path: *const u16) -> i32;
        }
        let empty = [0u16];
        // SAFETY: a flag, and a zero-terminated empty string (the current folder left out).
        unsafe {
            SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
            SetDllDirectoryW(empty.as_ptr());
        }
    }
}

/// Asked from outside to end: the window closed by another program (a
/// script's CloseMainWindow, Task Manager), or SIGTERM, SIGINT or SIGHUP.
static ENDING: AtomicBool = AtomicBool::new(false);

/// Whether PC Link was asked from outside to end; it then ends at once, not
/// kept by the clock and without asking.
pub fn ending() -> bool {
    ENDING.load(Ordering::SeqCst)
}

/// Marks PC Link as asked from outside to end.
pub fn end_asked() {
    ENDING.store(true, Ordering::SeqCst);
}

/// Linux: SIGTERM, SIGINT and SIGHUP mark it asked to end, and `wake` is
/// called (from a thread of its own) so the window ends as when closed. A
/// second signal ends it at once.
#[cfg(unix)]
pub fn end_on_signal(wake: impl Fn() + Send + 'static) {
    unsafe extern "C" {
        fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
        fn _exit(code: i32) -> !;
    }
    // Only what is safe in a signal handler: an atomic, and _exit.
    extern "C" fn heard(_: i32) {
        if ENDING.swap(true, Ordering::SeqCst) {
            // SAFETY: async-signal-safe; asked twice, it ends now.
            unsafe { _exit(1) }
        }
    }
    // SAFETY: the handler above, for three plain signals.
    unsafe {
        for sig in [1, 2, 15] {
            signal(sig, heard);
        }
    }
    std::thread::spawn(move || {
        while !ending() {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        wake();
    });
}

/// Windows: the window's close is heard before the window sees it. One from
/// another program (no SC_CLOSE first: not its close button, Alt+F4 or the
/// taskbar's Close) marks PC Link asked to end. Called once, on the
/// window's own thread.
#[cfg(windows)]
pub fn end_on_close(hwnd: isize) {
    use std::sync::atomic::AtomicIsize;
    const GWLP_WNDPROC: i32 = -4;
    const WM_CLOSE: u32 = 0x0010;
    const WM_SYSCOMMAND: u32 = 0x0112;
    const SC_CLOSE: usize = 0xF060;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
        fn CallWindowProcW(prev: isize, hwnd: isize, msg: u32, w: usize, l: isize) -> isize;
    }
    /// The window's own procedure, called after this one.
    static PREVIOUS: AtomicIsize = AtomicIsize::new(0);
    /// Inside the window's SC_CLOSE: the close that follows is the streamer's.
    static BY_HAND: AtomicBool = AtomicBool::new(false);
    unsafe extern "system" fn heard(hwnd: isize, msg: u32, w: usize, l: isize) -> isize {
        let previous = PREVIOUS.load(Ordering::SeqCst);
        // SAFETY: the window's own procedure, with the arguments Windows gave.
        let pass = || unsafe { CallWindowProcW(previous, hwnd, msg, w, l) };
        if msg == WM_SYSCOMMAND && w & 0xfff0 == SC_CLOSE {
            BY_HAND.store(true, Ordering::SeqCst);
            let result = pass();
            BY_HAND.store(false, Ordering::SeqCst);
            return result;
        }
        if msg == WM_CLOSE && !BY_HAND.load(Ordering::SeqCst) {
            end_asked();
        }
        pass()
    }
    if hwnd == 0 || PREVIOUS.load(Ordering::SeqCst) != 0 {
        return;
    }
    let hook: unsafe extern "system" fn(isize, u32, usize, isize) -> isize = heard;
    // SAFETY: this thread's own window, so no message comes in between: its
    // procedure is kept as ours takes its place, and every message is passed
    // on to it.
    unsafe { PREVIOUS.store(SetWindowLongPtrW(hwnd, GWLP_WNDPROC, hook as usize as isize), Ordering::SeqCst) };
}

/// A zero-terminated UTF-16 string, for Windows calls.
#[cfg(windows)]
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A system program by its usual full path (never a PATH search).
#[cfg(unix)]
pub fn tool(name: &str) -> Option<std::path::PathBuf> {
    ["/usr/bin", "/bin"].iter().map(|d| std::path::Path::new(d).join(name)).find(|p| p.is_file())
}

/// Waits for a started process on a thread of its own, so it never lingers as a zombie.
pub fn reap(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Whether a window's top left (logical pixels) is on a screen now. Windows
/// only; elsewhere the system places it.
pub fn on_screen(left: i32, top: i32) -> bool {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct Point {
            x: i32,
            y: i32,
        }
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MonitorFromPoint(point: Point, flags: u32) -> *mut std::ffi::c_void;
            fn GetDpiForSystem() -> u32;
        }
        // SAFETY: plain values in and out; null is no screen there.
        unsafe {
            let scale = GetDpiForSystem().max(96) as f32 / 96.0;
            // A little into the title bar, so a window just off an edge still counts.
            let at = |x: i32, y: i32| Point { x: ((x + 40) as f32 * scale) as i32, y: ((y + 10) as f32 * scale) as i32 };
            !MonitorFromPoint(at(left, top), 0).is_null()
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (left, top);
        false
    }
}

/// The local date and time now, as `2026-10-20 14:03:22`.
pub fn local_time() -> String {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct SystemTime {
            year: u16,
            month: u16,
            weekday: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            ms: u16,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetLocalTime(out: *mut SystemTime);
        }
        let mut t = SystemTime { year: 0, month: 0, weekday: 0, day: 0, hour: 0, minute: 0, second: 0, ms: 0 };
        // SAFETY: a local the call fills.
        unsafe { GetLocalTime(&mut t) };
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
    }
    #[cfg(not(windows))]
    {
        // glibc's struct tm.
        #[repr(C)]
        struct Tm {
            sec: i32,
            min: i32,
            hour: i32,
            mday: i32,
            mon: i32,
            year: i32,
            wday: i32,
            yday: i32,
            isdst: i32,
            gmtoff: i64,
            zone: *const std::ffi::c_char,
        }
        unsafe extern "C" {
            fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let mut t = Tm { sec: 0, min: 0, hour: 0, mday: 0, mon: 0, year: 0, wday: 0, yday: 0, isdst: 0, gmtoff: 0, zone: std::ptr::null() };
        // SAFETY: both pointers are locals; localtime_r fills `t` alone (it is
        // the thread-safe one), or returns null and leaves the zeros.
        unsafe { localtime_r(&now, &mut t) };
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.year + 1900, t.mon + 1, t.mday, t.hour, t.min, t.sec)
    }
}
