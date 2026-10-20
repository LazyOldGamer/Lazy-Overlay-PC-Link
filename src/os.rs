//! Small system helpers shared by the modules.

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
