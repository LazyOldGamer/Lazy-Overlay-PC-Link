//! Starting with Windows, only when ticked in the window or the installer: a
//! value in this user's Run key (no admin rights; Task Manager's Startup
//! apps shows it). The server cannot set it.

use std::path::Path;

/// The value's name, as the installer writes it (installer/pclink.iss).
pub const VALUE: &str = "Lazy Overlay PC Link";

pub const MINIMIZED: &str = "--minimized";

/// Windows only.
pub const fn supported() -> bool {
    cfg!(windows)
}

/// This program, minimized, with its settings file when not the usual one.
pub fn command(exe: &Path, settings: Option<&Path>) -> String {
    let mut line = format!("\"{}\" {MINIMIZED}", exe.display());
    if let Some(s) = settings {
        line.push_str(&format!(" --settings \"{}\"", s.display()));
    }
    line
}

/// Whether the saved value starts this command (paths ignore case on Windows).
pub fn starts(saved: Option<&str>, command: &str) -> bool {
    saved.is_some_and(|s| s.trim().eq_ignore_ascii_case(command))
}

pub fn saved() -> Option<String> {
    imp::get()
}

/// Starts `command` at sign-in, or nothing (None).
pub fn set(command: Option<&str>) -> std::io::Result<()> {
    match command {
        Some(c) => imp::set(c),
        None => imp::remove(),
    }
}

#[cfg(windows)]
mod imp {
    use super::VALUE;
    use crate::os::wide;
    use std::ffi::c_void;
    use std::io;

    type Hkey = *mut c_void;
    const HKEY_CURRENT_USER: Hkey = 0x8000_0001u32 as i32 as isize as Hkey;
    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const REG_SZ: u32 = 1;
    const RRF_RT_REG_SZ: u32 = 0x2;
    const ERROR_SUCCESS: i32 = 0;
    const ERROR_FILE_NOT_FOUND: i32 = 2;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            key: Hkey,
            sub: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut c_void,
            size: *mut u32,
        ) -> i32;
        fn RegSetKeyValueW(key: Hkey, sub: *const u16, value: *const u16, kind: u32, data: *const c_void, size: u32) -> i32;
        fn RegDeleteKeyValueW(key: Hkey, sub: *const u16, value: *const u16) -> i32;
    }

    fn result(code: i32) -> io::Result<()> {
        if code == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(code)) }
    }

    pub fn get() -> Option<String> {
        let (sub, value) = (wide(RUN), wide(VALUE));
        let mut buf = vec![0u16; 2048];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: zero-terminated strings; the buffer's size in bytes goes
        // with it, and RegGetValueW writes no more than that, zero-terminated.
        let code = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if code != ERROR_SUCCESS {
            return None;
        }
        buf.truncate(size as usize / 2);
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    pub fn set(command: &str) -> io::Result<()> {
        let (sub, value, data) = (wide(RUN), wide(VALUE), wide(command));
        // SAFETY: zero-terminated strings; the data's size in bytes includes its zero.
        result(unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, sub.as_ptr(), value.as_ptr(), REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32)
        })
    }

    pub fn remove() -> io::Result<()> {
        let (sub, value) = (wide(RUN), wide(VALUE));
        // SAFETY: zero-terminated strings.
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, sub.as_ptr(), value.as_ptr()) } {
            ERROR_FILE_NOT_FOUND => Ok(()),
            code => result(code),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;

    pub fn get() -> Option<String> {
        None
    }

    pub fn set(_: &str) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "starting with the system is on Windows only"))
    }

    pub fn remove() -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_starts_this_program_minimized() {
        let exe = Path::new(r"C:\Users\you\AppData\Local\Programs\Lazy Overlay PC Link\lazy-overlay-link.exe");
        let line = command(exe, None);
        assert_eq!(line, r#""C:\Users\you\AppData\Local\Programs\Lazy Overlay PC Link\lazy-overlay-link.exe" --minimized"#);
        let other = command(exe, Some(Path::new(r"D:\pc link\pclink.ini")));
        assert!(other.ends_with(r#" --minimized --settings "D:\pc link\pclink.ini""#));
        assert!(starts(Some(&line.to_uppercase()), &line));
        assert!(!starts(Some(&other), &line));
        assert!(!starts(None, &line));
    }
}
