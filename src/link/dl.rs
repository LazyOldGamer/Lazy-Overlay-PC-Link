//! A system library opened while running (dlopen): PC Link links no TLS code
//! at build time and uses the system's own.

use std::ffi::{CString, c_char, c_int, c_void};

#[cfg_attr(target_os = "linux", link(name = "dl"))]
unsafe extern "C" {
    fn dlopen(file: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;

/// An open library, never closed.
pub struct Lib(*mut c_void);

// SAFETY: a dlopen handle may be used from any thread.
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

impl Lib {
    /// The first of `names` that opens.
    pub fn open(names: &[&str]) -> Option<Lib> {
        names.iter().find_map(|n| {
            let c = CString::new(*n).ok()?;
            // SAFETY: a zero-terminated name; the handle is checked.
            let h = unsafe { dlopen(c.as_ptr(), RTLD_NOW) };
            (!h.is_null()).then_some(Lib(h))
        })
    }

    /// The function `name`, as the pointer type `T`.
    ///
    /// # Safety
    /// `T` must be an `extern "C"` function pointer type matching the
    /// function's real signature.
    pub unsafe fn get<T: Copy>(&self, name: &str) -> Option<T> {
        assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
        let c = CString::new(name).ok()?;
        // SAFETY: a live handle and a zero-terminated name.
        let p = unsafe { dlsym(self.0, c.as_ptr()) };
        // SAFETY: the caller vouches for T; a null symbol is never cast.
        (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
    }
}
