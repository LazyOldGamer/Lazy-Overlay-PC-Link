//! The socket on Windows: WinHTTP does TLS, proxies and the WebSocket.
//!
//! A reader thread hands frames over a channel; sends come from the caller.
//! Only the reader closes the socket, under the lock sends take, so no call
//! is made on a closed (possibly reused) handle.

use super::{Down, Link, PATH, Url};
use crate::json::MAX_TEXT;
use crate::os::wide;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Hinternet = *mut c_void;

const WINHTTP_ACCESS_TYPE_DEFAULT_PROXY: u32 = 0;
const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
const WINHTTP_OPTION_DISABLE_FEATURE: u32 = 63;
const WINHTTP_DISABLE_COOKIES: u32 = 0x1;
const WINHTTP_OPTION_SECURE_PROTOCOLS: u32 = 84;
const WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2: u32 = 0x800;
const WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3: u32 = 0x2000;
const WINHTTP_OPTION_REDIRECT_POLICY: u32 = 88;
const WINHTTP_OPTION_REDIRECT_POLICY_NEVER: u32 = 0;
const WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET: u32 = 114;
const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
const BINARY_MESSAGE: u32 = 0;
const BINARY_FRAGMENT: u32 = 1;
const UTF8_MESSAGE: u32 = 2;
const UTF8_FRAGMENT: u32 = 3;
const CLOSE: u32 = 4;
const TIMEOUT_MS: i32 = 10_000;

#[link(name = "winhttp")]
unsafe extern "system" {
    fn WinHttpOpen(agent: *const u16, access: u32, proxy: *const u16, bypass: *const u16, flags: u32) -> Hinternet;
    fn WinHttpConnect(session: Hinternet, server: *const u16, port: u16, reserved: u32) -> Hinternet;
    fn WinHttpOpenRequest(
        conn: Hinternet,
        verb: *const u16,
        object: *const u16,
        version: *const u16,
        referrer: *const u16,
        accept: *const *const u16,
        flags: u32,
    ) -> Hinternet;
    fn WinHttpSetOption(h: Hinternet, option: u32, buffer: *const c_void, len: u32) -> i32;
    fn WinHttpSetTimeouts(h: Hinternet, resolve: i32, connect: i32, send: i32, receive: i32) -> i32;
    fn WinHttpSendRequest(
        h: Hinternet,
        headers: *const u16,
        headers_len: u32,
        optional: *const c_void,
        optional_len: u32,
        total_len: u32,
        context: usize,
    ) -> i32;
    fn WinHttpReceiveResponse(h: Hinternet, reserved: *mut c_void) -> i32;
    fn WinHttpQueryHeaders(h: Hinternet, info: u32, name: *const u16, buffer: *mut c_void, len: *mut u32, index: *mut u32) -> i32;
    fn WinHttpWebSocketCompleteUpgrade(h: Hinternet, context: usize) -> Hinternet;
    fn WinHttpWebSocketSend(ws: Hinternet, kind: u32, buffer: *const c_void, len: u32) -> u32;
    fn WinHttpWebSocketReceive(ws: Hinternet, buffer: *mut c_void, len: u32, read: *mut u32, kind: *mut u32) -> u32;
    fn WinHttpWebSocketQueryCloseStatus(ws: Hinternet, status: *mut u16, reason: *mut c_void, len: u32, consumed: *mut u32) -> u32;
    fn WinHttpWebSocketShutdown(ws: Hinternet, status: u16, reason: *const c_void, len: u32) -> u32;
    fn WinHttpCloseHandle(h: Hinternet) -> i32;
}

/// A WinHTTP handle, closed once when dropped.
struct Handle(Hinternet);

// SAFETY: WinHTTP handles may be used from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a handle WinHTTP gave, closed exactly once.
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

// WinHTTP's common errors, in words.
fn why(code: u32) -> String {
    match code {
        12002 => "the server did not answer in time".into(),
        12007 => "the server's name was not found".into(),
        12029 | 12030 => "could not reach the server".into(),
        12175 | 12044 | 12045 | 12037 | 12038 => "the server's certificate was not trusted".into(),
        n => format!("Windows error {n}"),
    }
}

fn last_error() -> String {
    why(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32)
}

fn set_u32(h: &Handle, option: u32, value: u32) -> bool {
    // SAFETY: the option's buffer is a DWORD that lives through the call.
    unsafe { WinHttpSetOption(h.0, option, (&value as *const u32).cast(), 4) != 0 }
}

fn opened(h: Hinternet, what: &str) -> Result<Handle, Down> {
    if h.is_null() { Err(Down::Failed(format!("{what}: {}", last_error()))) } else { Ok(Handle(h)) }
}

/// The socket, and whether the reader has closed it: sends and the drop look
/// under the lock, and the reader closes it under the lock.
struct Socket {
    ws: Hinternet,
    closed: bool,
}

// SAFETY: as Handle; every use of `ws` after the upgrade is under the Mutex,
// but for the reader's own receives, and only the reader closes it.
unsafe impl Send for Socket {}

pub struct WinLink {
    socket: Arc<Mutex<Socket>>,
    frames: Receiver<Result<String, Down>>,
}

pub fn connect(url: &Url, key: &str) -> Result<WinLink, Down> {
    let agent = wide(concat!("LazyOverlayPCLink/", env!("CARGO_PKG_VERSION")));
    // SAFETY (here and below): every pointer is a live, zero-terminated
    // UTF-16 buffer or null where allowed; every handle is checked.
    let session = unsafe {
        let h = WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0);
        if h.is_null() { WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, null(), null(), 0) } else { h }
    };
    let session = opened(session, "could not start WinHTTP")?;
    if !set_u32(&session, WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3) {
        // Windows 10 has no TLS 1.3.
        set_u32(&session, WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2);
    }
    unsafe { WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS) };

    let host = wide(&url.host);
    let conn = opened(unsafe { WinHttpConnect(session.0, host.as_ptr(), url.port, 0) }, "could not reach the server")?;
    let (verb, path) = (wide("GET"), wide(PATH));
    let flags = if url.secure { WINHTTP_FLAG_SECURE } else { 0 };
    let req = opened(
        unsafe { WinHttpOpenRequest(conn.0, verb.as_ptr(), path.as_ptr(), null(), null(), null(), flags) },
        "could not reach the server",
    )?;
    // No redirect follows the key elsewhere; no cookies.
    set_u32(&req, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER);
    set_u32(&req, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES);
    if unsafe { WinHttpSetOption(req.0, WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET, null(), 0) } == 0 {
        return Err(Down::Failed(format!("this Windows has no WebSocket support: {}", last_error())));
    }
    let headers = wide(&format!("Authorization: Bearer {key}\r\n"));
    if unsafe { WinHttpSendRequest(req.0, headers.as_ptr(), u32::MAX, null(), 0, 0, 0) } == 0
        || unsafe { WinHttpReceiveResponse(req.0, null_mut()) } == 0
    {
        return Err(Down::Failed(last_error()));
    }
    let mut status: u32 = 0;
    let mut len: u32 = 4;
    let got = unsafe {
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut len,
            null_mut(),
        )
    };
    if got == 0 || status != 101 {
        return Err(Down::Failed(format!("the server answered {status}")));
    }
    let ws = unsafe { WinHttpWebSocketCompleteUpgrade(req.0, 0) };
    if ws.is_null() {
        return Err(Down::Failed(format!("the server did not open a socket: {}", last_error())));
    }
    drop(req);

    let socket = Arc::new(Mutex::new(Socket { ws, closed: false }));
    // A few frames waiting at most: past that the reader waits too, and so,
    // through TCP, does the server.
    let (tx, frames) = sync_channel(16);
    let shared = socket.clone();
    std::thread::spawn(move || {
        // What the socket was opened through goes when the socket does.
        let (_conn, _session) = (conn, session);
        let ws = shared.lock().map(|s| s.ws).unwrap_or(null_mut());
        let said = read(ws, &tx);
        // Closed here alone, under the lock: a send waiting on it then sees it closed.
        if let Ok(mut s) = shared.lock() {
            s.closed = true;
            // SAFETY: the handle WinHTTP gave, closed exactly once.
            unsafe { WinHttpCloseHandle(s.ws) };
        }
        let _ = tx.send(Err(said));
    });
    Ok(WinLink { socket, frames })
}

// Each frame from the socket, until it closes or breaks: how it ended.
fn read(ws: Hinternet, tx: &SyncSender<Result<String, Down>>) -> Down {
    let mut buf = vec![0u8; 16 * 1024];
    let mut msg: Vec<u8> = Vec::new();
    loop {
        let (mut read, mut kind) = (0u32, 0u32);
        // SAFETY: buf outlives the call and its length is given; the socket is
        // open, since only this thread closes it, after this returns.
        let err = unsafe { WinHttpWebSocketReceive(ws, buf.as_mut_ptr().cast(), buf.len() as u32, &mut read, &mut kind) };
        if err != 0 {
            return Down::Failed(format!("the connection broke: {}", why(err)));
        }
        let part = &buf[..(read as usize).min(buf.len())];
        match kind {
            UTF8_MESSAGE | UTF8_FRAGMENT => {
                msg.extend_from_slice(part);
                if msg.len() > MAX_TEXT {
                    return Down::Failed("the server sent too much at once".into());
                }
                if kind == UTF8_MESSAGE {
                    let text = String::from_utf8(std::mem::take(&mut msg)).unwrap_or_default();
                    if tx.send(Ok(text)).is_err() {
                        return Down::Failed("the connection ended".into());
                    }
                }
            }
            BINARY_MESSAGE | BINARY_FRAGMENT => msg.clear(),
            CLOSE => {
                let mut code: u16 = 1005;
                let mut reason = [0u8; 123];
                let mut used: u32 = 0;
                // SAFETY: reason is 123 bytes, the most a close frame holds.
                let ok =
                    unsafe { WinHttpWebSocketQueryCloseStatus(ws, &mut code, reason.as_mut_ptr().cast(), reason.len() as u32, &mut used) }
                        == 0;
                let text = if ok { String::from_utf8_lossy(&reason[..(used as usize).min(123)]).to_string() } else { String::new() };
                return Down::Closed(code, text);
            }
            _ => {}
        }
    }
}

impl Link for WinLink {
    fn send(&mut self, text: &str) -> Result<(), Down> {
        let s = self.socket.lock().map_err(|_| Down::Failed("the connection ended".into()))?;
        if s.closed {
            return Err(Down::Failed("the connection ended".into()));
        }
        // SAFETY: text outlives the call and its length is given; the socket
        // is open, and cannot be closed while the lock is held.
        let err = unsafe { WinHttpWebSocketSend(s.ws, UTF8_MESSAGE, text.as_ptr().cast(), text.len() as u32) };
        if err == 0 { Ok(()) } else { Err(Down::Failed(format!("could not send: {}", why(err)))) }
    }

    fn recv(&mut self, wait: Duration) -> Result<Option<String>, Down> {
        match self.frames.recv_timeout(wait) {
            Ok(Ok(text)) => Ok(Some(text)),
            Ok(Err(down)) => Err(down),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(Down::Failed("the connection ended".into())),
        }
    }
}

impl Drop for WinLink {
    fn drop(&mut self) {
        // The server is asked to close; its answer, or the connection ending,
        // ends the reader's read, and the reader closes the socket.
        if let Ok(s) = self.socket.lock()
            && !s.closed
        {
            // SAFETY: the socket is open, and cannot be closed while the lock is held.
            unsafe { WinHttpWebSocketShutdown(s.ws, 1000, null(), 0) };
        }
    }
}
