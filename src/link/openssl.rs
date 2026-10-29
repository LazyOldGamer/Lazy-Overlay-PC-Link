//! TLS on Linux: the system's OpenSSL 3 or 4, opened while running. The
//! certificate is checked against the system's roots (or SSL_CERT_FILE) and
//! the name, TLS 1.2 or newer.

use super::Down;
use super::dl::Lib;
use super::ws::Stream;
use std::ffi::{CString, c_char, c_int, c_long, c_ulong, c_void};
use std::io::{self, ErrorKind};
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::sync::OnceLock;
use std::time::Duration;

type P = *mut c_void;

const SSL_VERIFY_PEER: c_int = 1;
const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
const SSL_CTRL_SET_MIN_PROTO_VERSION: c_int = 123;
const TLS1_2_VERSION: c_long = 0x0303;
const SSL_ERROR_WANT_READ: c_int = 2;
const SSL_ERROR_WANT_WRITE: c_int = 3;
const SSL_ERROR_SYSCALL: c_int = 5;
const SSL_ERROR_ZERO_RETURN: c_int = 6;

struct Api {
    client_method: unsafe extern "C" fn() -> *const c_void,
    ctx_new: unsafe extern "C" fn(*const c_void) -> P,
    ctx_free: unsafe extern "C" fn(P),
    ctx_default_paths: unsafe extern "C" fn(P) -> c_int,
    ctx_set_verify: unsafe extern "C" fn(P, c_int, *const c_void),
    ctx_ctrl: unsafe extern "C" fn(P, c_int, c_long, P) -> c_long,
    new: unsafe extern "C" fn(P) -> P,
    free: unsafe extern "C" fn(P),
    set_fd: unsafe extern "C" fn(P, c_int) -> c_int,
    ctrl: unsafe extern "C" fn(P, c_int, c_long, P) -> c_long,
    set1_host: unsafe extern "C" fn(P, *const c_char) -> c_int,
    connect: unsafe extern "C" fn(P) -> c_int,
    get_error: unsafe extern "C" fn(P, c_int) -> c_int,
    read: unsafe extern "C" fn(P, P, c_int) -> c_int,
    write: unsafe extern "C" fn(P, *const c_void, c_int) -> c_int,
    verify_result: unsafe extern "C" fn(P) -> c_long,
    shutdown: unsafe extern "C" fn(P) -> c_int,
    err_get: unsafe extern "C" fn() -> c_ulong,
    err_clear: unsafe extern "C" fn(),
    err_string: unsafe extern "C" fn(c_ulong, *mut c_char, usize),
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        // OpenSSL 3 first; 4 keeps every call used here as 3 has it, so it is
        // taken once it says it is 3 or 4 (OpenSSL_version_num: the major
        // number in the top four bits).
        let lib = Lib::open(&["libssl.so.3", "libssl.so.4"])?;
        // SAFETY: OpenSSL_version_num's signature since 1.1.0.
        let version: unsafe extern "C" fn() -> c_ulong = unsafe { lib.get("OpenSSL_version_num")? };
        if !matches!(unsafe { version() } >> 28, 3 | 4) {
            return None;
        }
        // SAFETY: each type is the function's signature in OpenSSL 3 and 4.
        let api = unsafe {
            Api {
                client_method: lib.get("TLS_client_method")?,
                ctx_new: lib.get("SSL_CTX_new")?,
                ctx_free: lib.get("SSL_CTX_free")?,
                ctx_default_paths: lib.get("SSL_CTX_set_default_verify_paths")?,
                ctx_set_verify: lib.get("SSL_CTX_set_verify")?,
                ctx_ctrl: lib.get("SSL_CTX_ctrl")?,
                new: lib.get("SSL_new")?,
                free: lib.get("SSL_free")?,
                set_fd: lib.get("SSL_set_fd")?,
                ctrl: lib.get("SSL_ctrl")?,
                set1_host: lib.get("SSL_set1_host")?,
                connect: lib.get("SSL_connect")?,
                get_error: lib.get("SSL_get_error")?,
                read: lib.get("SSL_read")?,
                write: lib.get("SSL_write")?,
                verify_result: lib.get("SSL_get_verify_result")?,
                shutdown: lib.get("SSL_shutdown")?,
                err_get: lib.get("ERR_get_error")?,
                err_clear: lib.get("ERR_clear_error")?,
                err_string: lib.get("ERR_error_string_n")?,
            }
        };
        Some(api)
    })
    .as_ref()
}

/// OpenSSL's last error, in its words.
fn last(api: &Api) -> String {
    // SAFETY: a 256-byte buffer, its length given.
    unsafe {
        let code = (api.err_get)();
        if code == 0 {
            return "TLS failed".into();
        }
        let mut buf = [0 as c_char; 256];
        (api.err_string)(code, buf.as_mut_ptr(), buf.len());
        std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().to_string()
    }
}

pub struct Tls {
    api: &'static Api,
    ctx: P,
    ssl: P,
    tcp: TcpStream,
}

impl Drop for Tls {
    fn drop(&mut self) {
        // SAFETY: both made in connect, freed once here.
        unsafe {
            (self.api.shutdown)(self.ssl);
            (self.api.free)(self.ssl);
            (self.api.ctx_free)(self.ctx);
        }
    }
}

/// TLS on `tcp`, for `host`, verified.
pub fn connect(host: &str, tcp: TcpStream) -> Result<Tls, Down> {
    let api = api().ok_or_else(|| Down::Failed("OpenSSL 3 or 4 (libssl.so.3 or libssl.so.4) is not installed on this PC".into()))?;
    let name = CString::new(host).map_err(|_| Down::Failed("bad host".into()))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| Down::Failed(e.to_string()))?;
    // SAFETY: every pointer is checked before use and freed by Tls's drop
    // (or here, before a Tls exists); the name outlives the calls.
    unsafe {
        let ctx = (api.ctx_new)((api.client_method)());
        if ctx.is_null() {
            return Err(Down::Failed(last(api)));
        }
        // On the context before the SSL is made from it: SSL_new copies them.
        (api.ctx_set_verify)(ctx, SSL_VERIFY_PEER, std::ptr::null());
        if (api.ctx_default_paths)(ctx) != 1
            || (api.ctx_ctrl)(ctx, SSL_CTRL_SET_MIN_PROTO_VERSION, TLS1_2_VERSION, std::ptr::null_mut()) != 1
        {
            (api.ctx_free)(ctx);
            return Err(Down::Failed(last(api)));
        }
        let ssl = (api.new)(ctx);
        if ssl.is_null() {
            (api.ctx_free)(ctx);
            return Err(Down::Failed(last(api)));
        }
        let tls = Tls { api, ctx, ssl, tcp };
        let ok = (api.set_fd)(ssl, tls.tcp.as_raw_fd()) == 1
            && (api.ctrl)(ssl, SSL_CTRL_SET_TLSEXT_HOSTNAME, 0, name.as_ptr() as P) == 1
            && (api.set1_host)(ssl, name.as_ptr()) == 1;
        if !ok {
            return Err(Down::Failed(last(api)));
        }
        // An error left from before would be read as this call's (SSL_get_error).
        (api.err_clear)();
        if (api.connect)(ssl) != 1 {
            let verify = (api.verify_result)(ssl);
            if verify != 0 {
                return Err(Down::Failed(format!("the server's certificate was not trusted (OpenSSL check {verify})")));
            }
            return Err(Down::Failed(format!("TLS failed: {}", last(api))));
        }
        Ok(tls)
    }
}

impl Stream for Tls {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(c_int::MAX as usize) as c_int;
        // SAFETY: buf outlives the call and its length is given.
        // SAFETY: as above; the queue is cleared first, so SSL_get_error reads this call's.
        let r = unsafe {
            (self.api.err_clear)();
            (self.api.read)(self.ssl, buf.as_mut_ptr().cast(), n)
        };
        if r > 0 {
            return Ok(r as usize);
        }
        let os = io::Error::last_os_error();
        // SAFETY: a live SSL and the result just returned.
        match unsafe { (self.api.get_error)(self.ssl, r) } {
            SSL_ERROR_ZERO_RETURN => Ok(0),
            SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => Err(ErrorKind::WouldBlock.into()),
            SSL_ERROR_SYSCALL if matches!(os.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Err(os),
            SSL_ERROR_SYSCALL if r == 0 => Ok(0),
            _ => Err(io::Error::other(last(self.api))),
        }
    }

    fn write_all(&mut self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let n = buf.len().min(c_int::MAX as usize) as c_int;
            // SAFETY: buf outlives the call and its length is given.
            let r = unsafe {
                (self.api.err_clear)();
                (self.api.write)(self.ssl, buf.as_ptr().cast(), n)
            };
            if r <= 0 {
                return Err(io::Error::other(last(self.api)));
            }
            buf = &buf[r as usize..];
        }
        Ok(())
    }

    fn set_read_timeout(&mut self, wait: Duration) -> io::Result<()> {
        self.tcp.set_read_timeout(Some(wait.max(Duration::from_millis(1))))
    }
}
