//! Which programs are running, by file name. The list stays on this PC; only
//! listed names are told (app.rs).

use std::collections::HashSet;
use std::io;

/// How a name is compared: lowercase, `.exe` or not. `OBS64.EXE` is `obs64`.
pub fn key(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    lower.strip_suffix(".exe").map(str::to_string).unwrap_or(lower)
}

/// The file names of every running process, as `key` writes them.
pub fn running() -> io::Result<HashSet<String>> {
    imp::running()
}

/// Whether a process of this file name runs; None when that cannot be told.
pub fn is_running(name: &str) -> Option<bool> {
    running().ok().map(|r| r.contains(&key(name)))
}

/// Programs open now, by full path, for the window's picker: once each, by
/// name, less the system's own and PC Link itself.
pub fn open_programs() -> Vec<std::path::PathBuf> {
    let me = std::env::current_exe().ok();
    let mut out: Vec<std::path::PathBuf> =
        imp::paths().into_iter().filter(|p| p.is_absolute() && Some(p) != me.as_ref() && !system(p)).collect();
    // By name, then path, so two copies of one path sit together and go as one.
    out.sort_by_cached_key(|p| (p.file_name().map(|f| f.to_string_lossy().to_lowercase()), p.clone()));
    out.dedup();
    out
}

fn system(p: &std::path::Path) -> bool {
    let s = p.to_string_lossy().to_lowercase().replace('\\', "/");
    if cfg!(windows) {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()).to_lowercase().replace('\\', "/");
        s.starts_with(&format!("{root}/")) || s.contains("/windowsapps/microsoft.windows")
    } else {
        ["/usr/lib/", "/usr/libexec/", "/lib/", "/usr/sbin/", "/sbin/", "/snap/core"].iter().any(|d| s.starts_with(d))
    }
}

/// The ids of every process with this file name, matched as `running` does.
pub fn ids(name: &str) -> io::Result<Vec<u32>> {
    let want = key(name);
    let mut ids: Vec<u32> = imp::all()?.into_iter().filter(|(_, n)| *n == want).map(|(id, _)| id).collect();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

#[cfg(windows)]
mod imp {
    use super::key;
    use std::collections::HashSet;
    use std::ffi::c_void;
    use std::io;

    const TH32CS_SNAPPROCESS: u32 = 0x2;
    const INVALID_HANDLE_VALUE: isize = -1;
    const MAX_PATH: usize = 260;

    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; MAX_PATH],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut c_void;
        fn Process32FirstW(snapshot: *mut c_void, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: *mut c_void, entry: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn QueryFullProcessImageNameW(process: *mut c_void, flags: u32, name: *mut u16, size: *mut u32) -> i32;
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    /// Each process's program, where Windows lets this user see it.
    pub fn paths() -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for (pid, _) in all().unwrap_or_default() {
            // SAFETY: a handle checked before use and closed once; the buffer
            // and its length go together.
            unsafe {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if h.is_null() {
                    continue;
                }
                let mut buf = [0u16; 1024];
                let mut len = buf.len() as u32;
                if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0 {
                    out.push(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])));
                }
                CloseHandle(h);
            }
        }
        out
    }

    pub fn running() -> io::Result<HashSet<String>> {
        Ok(all()?.into_iter().map(|(_, name)| name).collect())
    }

    /// Every process's id and file name, as `key` writes it.
    pub fn all() -> io::Result<Vec<(u32, String)>> {
        // SAFETY: a snapshot handle is checked before use and closed once;
        // the entry is zeroed with dw_size set, as Process32FirstW requires,
        // and only read up to its terminating zero.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap as isize == INVALID_HANDLE_VALUE || snap.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut out = Vec::new();
            let mut entry: ProcessEntry32W = std::mem::zeroed();
            entry.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut entry);
            while ok != 0 {
                let len = entry.sz_exe_file.iter().position(|&c| c == 0).unwrap_or(MAX_PATH);
                out.push((entry.th32_process_id, key(&String::from_utf16_lossy(&entry.sz_exe_file[..len]))));
                ok = Process32NextW(snap, &mut entry);
            }
            CloseHandle(snap);
            Ok(out)
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::key;
    use std::collections::HashSet;
    use std::{fs, io};

    /// Each process's program, where this user may read it.
    pub fn paths() -> Vec<std::path::PathBuf> {
        fs::read_dir("/proc").map(|d| d.flatten().filter_map(|e| fs::read_link(e.path().join("exe")).ok()).collect()).unwrap_or_default()
    }

    pub fn running() -> io::Result<HashSet<String>> {
        Ok(all()?.into_iter().map(|(_, n)| n).collect())
    }

    // Each process's names, as `key` writes them, with its id: its first
    // argument's last part, and its comm.
    pub fn all() -> io::Result<Vec<(u32, String)>> {
        let mut out = Vec::new();
        for entry in fs::read_dir("/proc")?.flatten() {
            let name = entry.file_name();
            let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
                continue;
            };
            let dir = entry.path();
            // Its first 4 KB: a command line can be megabytes, read every 2 seconds.
            let mut c = Vec::new();
            let read = fs::File::open(dir.join("cmdline")).and_then(|f| io::Read::read_to_end(&mut io::Read::take(f, 4096), &mut c));
            let argv0 = read.ok().and_then(|_| {
                let first = c.split(|&b| b == 0).next()?.to_vec();
                let first = String::from_utf8_lossy(&first).to_string();
                let base = first.rsplit('/').next()?.to_string();
                (!base.is_empty()).then_some(base)
            });
            if let Some(n) = argv0 {
                out.push((pid, key(&n)));
            }
            if let Ok(comm) = fs::read_to_string(dir.join("comm")) {
                out.push((pid, key(comm.trim_end())));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_without_case_or_exe() {
        assert_eq!(key("OBS64.EXE"), "obs64");
        assert_eq!(key("obs64"), "obs64");
        assert_eq!(key(" Game.exe "), "game");
    }

    #[test]
    fn lists_this_test_among_what_runs() {
        let me = std::env::current_exe().unwrap();
        let mine = key(&me.file_name().unwrap().to_string_lossy());
        let all = running().unwrap();
        assert!(all.len() > 1);
        assert!(all.contains(&mine) || all.iter().any(|n| mine.starts_with(n.as_str())), "{mine} not in {all:?}");
    }
}
